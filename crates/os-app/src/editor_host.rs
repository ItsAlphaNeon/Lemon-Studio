// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The interface's plugin editor windows (control-plane step 7.3b): each hosted instance's window, by its `hosted`
//! key, and what is kept of it while closed. The host (`crate::plugins`) keeps the instances and records what
//! happens to them (`EditorEvent`); `EditorHost::sync` replays that before every call here, so a window moves and goes
//! with its instance. The editor holds its plugin, so an instance whose window is open lives until the window closes.

#![allow(unused_imports)]
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use os_host::SharedPlugin;
#[cfg(plugin_windows)]
use crate::plugin_window::{ChannelInfo, FrameEvent, FrameWindow};
use crate::plugins::*;

/// An instance's editor window, and what is kept of it while it is closed (`Plugins::editors`, by the instance's key).
/// Apart from the instance: the plugin keeps playing whether its editor is open or not.
#[derive(Default)]
pub struct Editor {
    #[cfg(plugin_windows)]
    pub window: Option<FrameWindow>,
    /// Set while the open editor belongs to a Lemon plugin that joined the strip (host link).
    #[cfg(plugin_windows)]
    pub link: Option<os_hostlink::Link>,
    /// The instance whose editor is open in `window` (closed through it when the editor goes, `Drop`).
    #[cfg(plugin_windows)]
    plugin: Option<SharedPlugin>,
    /// Its window opens detached: set when the user adds it while "Open new plugin windows floating" is on (`Plugins::added`),
    /// or chosen for it (window menu > Detached, Switch all plugin editors to ▸), then the mode its window had when it
    /// last closed (spec: ui/settings-choices.md "Miscellaneous: naming, detached plugins and startup").
    #[cfg_attr(not(plugin_windows), allow(dead_code))]
    pub detached: bool,
    /// Where its detached window was when it last closed (screen px): reopened, it goes back there (spec: as above).
    #[cfg(plugin_windows)]
    pub placed: Option<(i32, i32)>,
    /// When its open window opened (`Plugins::opens`): the limit on open editors closes the earliest first.
    #[cfg(plugin_windows)]
    opened: u64,
}

impl Editor {
    /// Its window is open.
    pub fn open(&self) -> bool {
        #[cfg(plugin_windows)]
        return self.window.is_some();
        #[cfg(not(plugin_windows))]
        false
    }

    /// The open editor is a Lemon plugin's that joined the strip through the host link.
    #[cfg(plugin_windows)]
    fn linked(&self) -> bool {
        self.link.is_some()
    }

    /// Closes its window, the editor first (a frame destroyed under an open editor took the editor's window with it,
    /// and the plugin's own close then crashed at exit).
    fn close(&mut self) {
        #[cfg(plugin_windows)]
        {
            if let (Some(plugin), true) = (self.plugin.take(), self.window.is_some()) {
                if let Ok(mut p) = plugin.lock() {
                    p.close_editor();
                }
            }
            self.window = None;
            self.link = None;
        }
    }
}

impl Drop for Editor {
    /// An editor going (its instance gone, the program ending) closes first, before its frame window goes.
    fn drop(&mut self) {
        self.close();
    }
}

/// What a plugin window asks of the app.
#[derive(Clone, Debug, PartialEq)]
pub enum Request {
    Close(usize),
    ToggleOn(usize),
    Volume(usize, f64),
    Pan(usize, f64),
    PitchCents(usize, i32),
    /// The strip's RANGE box (semitones) and TRACK box (mixer track); a double click on TRACK shows the mixer there.
    PitchRange(usize, i32),
    Track(usize, u16),
    ShowTrack(usize),
    /// A preview note from a Lemon plugin's own keyboard (host link): row, key, velocity, on.
    Note(usize, u8, u8, bool),
    /// "Create automation clip" from a Lemon plugin's own parameter menu (host link): row, parameter index.
    #[cfg_attr(not(plugin_windows), allow(dead_code))]
    Automate(usize, usize),
    /// A sample file a Lemon plugin sent (host link `Audio`, Tesla's Send): to the selected channel (true) or the
    /// playlist.
    #[cfg_attr(not(plugin_windows), allow(dead_code))]
    Audio(String, os_hostlink::AudioTo),
    /// Notes a Lemon plugin sent for its channel's current pattern (host link `Notes`, Lemon Slicer's dump): row,
    /// notes.
    #[cfg_attr(not(plugin_windows), allow(dead_code))]
    Notes(usize, Vec<os_hostlink::NoteOut>),
    /// The window was clicked (or docked / detached): it goes to the front of the app's window stack.
    Raise(usize),
    /// The menu arrow was clicked: open the window menu at this screen point.
    Menu(usize, i32, i32),
    /// The title-bar Presets box was clicked: open the preset list at this screen point (spec: plugins/presets.md).
    Presets(usize, i32, i32),
    /// The Presets box arrows: next (+1) / previous (-1) preset.
    PresetStep(usize, i32),
    /// A file dropped on the window's frame.
    #[cfg_attr(not(windows), allow(dead_code))]
    DropFile(usize, PathBuf),
    /// A Misc tab value changed (spec: ui/plugin-window.md "Misc tab").
    Misc(usize, crate::misc_page::M, f32),
    /// A Misc tab list asks for its menu at this screen point.
    MiscMenu(usize, crate::misc_page::M, i32, i32),
    /// A strip knob of channel row `.0` asks for its menu at this screen point.
    KnobMenu(usize, crate::knobs::Control, i32, i32),
    /// A greyed-out control of the window was clicked: the app says it is not available yet.
    NotAvailable(&'static str),
    /// The plugin asked for a menu over its editor (VST 3 context menu): shown at this screen point (spec:
    /// plugins/vst3-hosting.md §9).
    PluginMenu(usize, os_host::Menu, (i32, i32)),
    /// The instance stopped after a crash (its window is closed): what to tell the user.
    Stopped(usize, String),
}

/// Of the open windows (`hosted` key, when it opened), the one to close before another opens, so that no more than
/// `max` are open: the one opened earliest, while `max` or more are open.
#[cfg_attr(not(plugin_windows), allow(dead_code))]
pub fn earliest_over(open: impl Iterator<Item = (usize, u64)>, max: usize) -> Option<usize> {
    let open: Vec<(usize, u64)> = open.collect();
    open.iter().min_by_key(|o| o.1).filter(|_| open.len() >= max).map(|o| o.0)
}

/// Sizes window `w` to its freshly opened editor (`size` in device px at `scale`). A Lemon plugin answers the host
/// link's hello: its page tabs go into the strip row and its editor shrinks to what is below them.
#[cfg(plugin_windows)]
pub fn fit_editor(w: &FrameWindow, (ew, eh): (u32, u32), scale: f32) -> Option<os_hostlink::Link> {
    let link = os_hostlink::Link::connect(w.area() as isize);
    match &link {
        Some(l) => {
            w.set_pages(l.pages.clone(), l.page_now());
            let (lw, lh) = l.size().unwrap_or(((ew as f32 / scale) as u32, (eh as f32 / scale) as u32));
            w.set_editor_size((lw as f32, lh as f32));
        }
        None => w.set_editor_size((ew as f32 / scale, eh as f32 / scale)),
    }
    link
}

/// Posts `WM_DROPFILES` for `path` to the window at screen point (x, y), or its nearest ancestor that accepts files
/// (`DragAcceptFiles`). The drop handle is ours: the receiver's `DragFinish` frees it. Same process only.
#[cfg(windows)]
/// `frame` bounds the search: a docked frame's parent is the main window, which takes files itself.
pub fn post_drop(x: i32, y: i32, path: &str, frame: isize) -> bool {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::Foundation::GlobalFree;
    use windows_sys::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GHND};
    use windows_sys::Win32::UI::Shell::DROPFILES;
    use windows_sys::Win32::UI::WindowsAndMessaging::*;
    unsafe {
        let mut w = WindowFromPoint(POINT { x, y });
        while !w.is_null() {
            // The frame takes files itself (presets); it is not where a sample for the plugin goes.
            if w as isize == frame {
                return false;
            }
            if GetWindowLongW(w, GWL_EXSTYLE) as u32 & WS_EX_ACCEPTFILES != 0 {
                break;
            }
            w = GetAncestor(w, GA_PARENT);
            if w == GetDesktopWindow() {
                return false;
            }
        }
        if w.is_null() {
            return false;
        }
        let mut pt = POINT { x, y };
        windows_sys::Win32::Graphics::Gdi::ScreenToClient(w, &mut pt);
        let wide: Vec<u16> = path.encode_utf16().chain([0, 0]).collect();
        let head = std::mem::size_of::<DROPFILES>();
        let mem = GlobalAlloc(GHND, head + wide.len() * 2);
        if mem.is_null() {
            return false;
        }
        let p = GlobalLock(mem) as *mut u8;
        if p.is_null() {
            GlobalFree(mem);
            return false;
        }
        *(p as *mut DROPFILES) = DROPFILES { pFiles: head as u32, pt, fNC: 0, fWide: 1 };
        std::ptr::copy_nonoverlapping(wide.as_ptr() as *const u8, p.add(head), wide.len() * 2);
        GlobalUnlock(mem);
        if PostMessageW(w, WM_DROPFILES, mem as usize, 0) == 0 {
            GlobalFree(mem);
            return false;
        }
        true
    }
}

#[cfg(plugin_windows)]
pub type ChannelInfoT = ChannelInfo;

#[cfg(not(plugin_windows))]
pub type ChannelInfoT = ();

/// The open editor windows and what is kept of the closed ones, by `hosted` key.
pub struct EditorHost {
    pub editors: BTreeMap<usize, Editor>,
    /// The program's window system for the editors' frames (control-plane step 10).
    #[cfg(plugin_windows)]
    pub frames: Box<dyn crate::plugin_window::Frames>,
    /// Plugin windows opened so far (stamps `Editor::opened`).
    #[cfg(plugin_windows)]
    opens: u64,
}

impl Default for EditorHost {
    /// No frames until the program gives its window system's (`plugin_window::NoFrames`).
    fn default() -> Self {
        EditorHost {
            editors: BTreeMap::new(),
            #[cfg(plugin_windows)]
            frames: Box::new(crate::plugin_window::NoFrames),
            #[cfg(plugin_windows)]
            opens: 0,
        }
    }
}

impl EditorHost {
    /// Replays what the host did to its instances since the last call (`Plugins::editor_events`).
    pub fn sync(&mut self, pl: &mut Plugins) {
        for e in std::mem::take(&mut pl.editor_events) {
            match e {
                EditorEvent::Detached(key, on) => self.editors.entry(key).or_default().detached = on,
                EditorEvent::Drop(key) => {
                    #[cfg(plugin_windows)]
                    self.close_window_now(key);
                    self.editors.remove(&key);
                }
                EditorEvent::PresetShown(_key) => {
                    #[cfg(plugin_windows)]
                    if let Some(w) = self.editors.get(&_key).and_then(|e| e.window.as_ref()) {
                        w.set_preset(pl.shown_preset(_key));
                    }
                }
                EditorEvent::FxMoves(moves) => {
                    let taken: Vec<(usize, Option<Editor>)> = moves.iter().map(|&(from, to)| (to, self.editors.remove(&from))).collect();
                    for (to, e) in taken {
                        // An editor already at the new key closes (its instance was replaced).
                        let old = self.editors.remove(&to);
                        if let Some(e) = e {
                            self.editors.insert(to, e);
                        }
                        drop(old);
                    }
                }
                EditorEvent::Remap(map) => {
                    let mut editors = self.editors.split_off(&FX_KEY);
                    let mut rows = std::mem::take(&mut self.editors);
                    for (row, to) in map {
                        match to {
                            Some(n) => {
                                let old = editors.remove(&n);
                                if let Some(e) = rows.remove(&row) {
                                    editors.insert(n, e);
                                }
                                drop(old);
                            }
                            None => drop(rows.remove(&row)),
                        }
                    }
                    self.editors = editors;
                }
                EditorEvent::Clear(_keys) => {
                    #[cfg(plugin_windows)]
                    for row in _keys {
                        self.close_window_now(row);
                    }
                    self.editors.clear();
                }
            }
        }
    }

    /// The keys whose window is open (`Plugins::editors_open`).
    pub fn open_keys(&self) -> std::collections::BTreeSet<usize> {
        self.editors.iter().filter(|(_, e)| e.open()).map(|(&k, _)| k).collect()
    }

    /// `close_window` without a sync (the replay's own).
    #[cfg(plugin_windows)]
    fn close_window_now(&mut self, row: usize) {
        if let Some(e) = self.editors.get_mut(&row) {
            if let Some(w) = &e.window {
                e.detached = w.detached();
                #[cfg(windows)]
                if e.detached {
                    e.placed = w.position().or(e.placed);
                }
            }
            e.close();
        }
    }

    /// The hosted row whose plugin window is at screen point (x, y).
    #[cfg(windows)]
    pub fn row_at_screen(&self, x: i32, y: i32) -> Option<usize> {
        use windows_sys::Win32::Foundation::POINT;
        use windows_sys::Win32::UI::WindowsAndMessaging::{GetAncestor, WindowFromPoint, GA_PARENT};
        // Walk up from the window under the point: a docked frame is a child of the main window, a detached one is a
        // top-level window.
        let mut w = unsafe { WindowFromPoint(POINT { x, y }) };
        while !w.is_null() {
            if let Some((&r, _)) = self.editors.iter().find(|(_, e)| e.window.as_ref().is_some_and(|f| f.hwnd() == w as isize)) {
                return Some(r);
            }
            w = unsafe { GetAncestor(w, GA_PARENT) };
        }
        None
    }

    /// Drops file `path` on row `row`'s plugin window at screen point (x, y), as Explorer would: the window under the
    /// point that accepts files gets `WM_DROPFILES`. Lemon Sampler rows without one (the strip, the frame) load the sample
    /// through their state. Returns whether the plugin got the file.
    #[cfg(windows)]
    pub fn drop_file(&mut self, pl: &mut Plugins, row: usize, x: i32, y: i32, path: &str, sampler: bool) -> bool {
        self.sync(pl);
        if self.editors.get(&row).and_then(|e| e.window.as_ref()).is_some_and(|f| post_drop(x, y, path, f.hwnd())) {
            return true;
        }
        sampler && pl.load_sample(row, path)
    }

    /// The hosted row whose plugin window is at screen point (x, y).
    #[cfg(target_os = "macos")]
    pub fn row_at_screen(&self, x: i32, y: i32) -> Option<usize> {
        let frame = self.frames.row_window_at(x, y)?;
        self.editors.iter().find(|(_, e)| e.window.as_ref().is_some_and(|f| f.hwnd() == frame)).map(|(&r, _)| r)
    }

    /// Drops file `path` on row `row`'s plugin window: macOS plugins take files only from a drag session of their own,
    /// so only Lemon Sampler rows load it (through their state). Returns whether the plugin got the file.
    #[cfg(target_os = "macos")]
    pub fn drop_file(&mut self, pl: &mut Plugins, row: usize, _x: i32, _y: i32, path: &str, sampler: bool) -> bool {
        self.sync(pl);
        sampler && pl.load_sample(row, path)
    }

    /// Whether a plugin's editor is open, so the transport keeps the plugins processed while nothing plays
    /// (`Transport::listen`): an instrument's own keyboard plays it without the host, and an effect may make sound of
    /// its own (Tesla plays its sample) or show what it hears.
    pub fn editor_open(&self) -> bool {
        #[cfg(plugin_windows)]
        return self.editors.values().any(|e| e.window.is_some());
        #[cfg(not(plugin_windows))]
        false
    }

    /// Opens (or raises) row `row`'s plugin window.
    #[cfg(plugin_windows)]
    pub fn open_window(&mut self, pl: &mut Plugins, row: usize, info: ChannelInfo) -> Result<(), String> {
        self.sync(pl);
        let (owner, scale) = (pl.owner, pl.scale_for(row, pl.scale));
        if !pl.hosted.contains_key(&row) {
            return Err("not a plugin channel".into());
        }
        if let Some(w) = self.editors.get(&row).and_then(|e| e.window.as_ref()) {
            w.raise();
            return Ok(());
        }
        // At most `MAX_EDITORS` open: the one opened earliest closes (its plugin keeps running). Spec:
        // ui/settings-choices.md "Miscellaneous: naming, detached plugins and startup".
        if crate::prefs::check(&crate::prefs::LIMIT_EDITORS) {
            while let Some(k) = earliest_over(self.editors.iter().filter(|(_, e)| e.window.is_some()).map(|(&k, e)| (k, e.opened)), MAX_EDITORS) {
                self.close_window(pl, k);
            }
        }
        let Some(h) = pl.hosted.get(&row) else { return Err("not a plugin channel".into()) };
        let e = self.editors.entry(row).or_default();
        // A detached window reopens where it was; the first time, on a monitor as the settings say (spec: as above).
        let pos = match (e.detached, e.placed) {
            (true, Some(at)) => at,
            (true, None) => self.frames.first_position(owner, scale, crate::prefs::check(&crate::prefs::DETACHED_WITH_MAIN)),
            (false, _) => self.frames.default_position(owner, scale),
        };
        // Create the frame at a provisional size, embed the editor, then fit the frame to the editor's real size.
        let w = self.frames.open(owner, info, (560.0, 437.0), scale, pos, e.detached)?;
        // Mixer effects have no channel strip (spec: ui/mixer.md §7).
        if row >= FX_KEY {
            w.set_strip(false);
        }
        // The editor takes the frame's scale (on Linux the X server's, not the main window's).
        let k = w.scale();
        // The instance is locked only while its editor opens: the audio thread skips an instance it cannot lock, so
        // everything done under the lock is silence for that channel. Held through the frame's creation, layout,
        // painting and raise as well, opening a plugin window made a playing song drop out on that channel.
        let size = {
            let mut p = h.plugin.lock().map_err(|_| "plugin busy")?;
            p.open_editor(w.area(), k)?
        };
        e.link = fit_editor(&w, size, k);
        // On top: the app stacks it above its other windows (`State::raise`), and a new child window starts below
        // the open ones.
        w.raise();
        e.window = Some(w);
        e.plugin = Some(h.plugin.clone());
        self.opens += 1;
        e.opened = self.opens;
        let name = pl.shown_preset(row);
        if let Some(w) = self.editors.get(&row).and_then(|e| e.window.as_ref()) {
            w.set_preset(name);
        }
        Ok(())
    }

    /// Scaling ▸ for row `row` (`None`: Main, the program's scale): an open window is drawn again at once.
    pub fn set_editor_scale(&mut self, pl: &mut Plugins, row: usize, scale: Option<f32>) {
        self.sync(pl);
        match scale {
            Some(k) => pl.editor_scales.insert(row, k),
            None => pl.editor_scales.remove(&row),
        };
        #[cfg(any(windows, target_os = "macos"))]
        if self.editors.get(&row).is_some_and(|e| e.window.is_some()) {
            let k = pl.scale_for(row, pl.scale);
            self.rescale_window(pl, row, k);
        }
    }

    /// The main window's DPI scale changed: docked windows follow it (detached ones follow their own monitor,
    /// `FrameEvent::Dpi`). Spec: ui/ui-scaling.md §3.
    pub fn set_scale(&mut self, pl: &mut Plugins, scale: f32) {
        self.sync(pl);
        if scale <= 0.0 || scale == pl.scale {
            return;
        }
        pl.scale = scale;
        #[cfg(any(windows, target_os = "macos"))]
        for row in self.docked() {
            self.rescale_window(pl, row, pl.scale_for(row, scale));
        }
    }

    /// Redraws row `row`'s window at `scale`: the frame resizes and the editor is reopened, since plugins take their
    /// scale when the editor opens (on macOS its sizes are measured at it). The plugin's page is kept.
    #[cfg(any(windows, target_os = "macos"))]
    fn rescale_window(&mut self, pl: &mut Plugins, row: usize, scale: f32) {
        self.sync(pl);
        let (Some(Hosted { plugin, .. }), Some(Editor { window: Some(w), link, .. })) = (pl.hosted.get(&row), self.editors.get_mut(&row)) else { return };
        if w.scale() == scale {
            return;
        }
        let Ok(mut p) = plugin.lock() else { return };
        let page = link.as_ref().map(|l| l.page_now());
        p.close_editor();
        w.set_scale(scale);
        *link = match p.open_editor(w.area(), scale) {
            Ok(size) => fit_editor(w, size, scale),
            Err(e) => {
                eprintln!("lemonstudio: plugin editor at the new scale: {e}");
                None
            }
        };
        if let (Some(l), Some(page)) = (link.as_mut(), page) {
            l.set_page(page);
            w.set_page(l.page_now());
        }
    }

    #[cfg(plugin_windows)]
    fn close_window(&mut self, pl: &mut Plugins, row: usize) {
        self.sync(pl);
        if let Some(e) = self.editors.get_mut(&row) {
            // The mode and place it reopens in (spec: ui/settings-choices.md "Miscellaneous: naming, detached plugins
            // and startup").
            if let Some(w) = &e.window {
                e.detached = w.detached();
                #[cfg(windows)]
                if e.detached {
                    e.placed = w.position().or(e.placed);
                }
            }
            e.close();
        }
    }

    /// Row `row`'s open window (native frame handle) and whether it is detached.
    #[cfg(plugin_windows)]
    pub fn window_of(&self, row: usize) -> Option<(isize, bool)> {
        self.editors.get(&row).and_then(|e| e.window.as_ref()).map(|w| (w.hwnd(), w.detached()))
    }

    /// Docks or detaches row `row`'s window (window menu > Detached); closed, it opens that way next time.
    #[cfg(plugin_windows)]
    pub fn set_detached(&mut self, pl: &mut Plugins, row: usize, on: bool) {
        self.sync(pl);
        let owner = pl.owner;
        if pl.hosted.contains_key(&row) {
            let e = self.editors.entry(row).or_default();
            e.detached = on;
            if let Some(w) = &e.window {
                w.set_detached(owner, on);
            }
        }
    }

    /// Switch all plugin editors to ▸ (spec: ui/channel-rack.md §17): every instance's window, rack rows' and mixer
    /// slots', docked or detached; an open one at once, a closed one when it opens next.
    #[cfg(plugin_windows)]
    pub fn set_all_detached(&mut self, pl: &mut Plugins, on: bool) {
        self.sync(pl);
        let keys: Vec<usize> = pl.hosted.keys().copied().collect();
        for key in keys {
            self.set_detached(pl, key, on);
        }
    }

    /// Rows whose plugin window is open and docked in the main window.
    #[cfg(plugin_windows)]
    pub fn docked(&self) -> Vec<usize> {
        self.editors.iter().filter(|(_, e)| e.window.as_ref().is_some_and(|w| !w.detached())).map(|(&r, _)| r).collect()
    }

    /// Stacks the docked windows of `rows` (bottom to top, as in `State::z`) in that order: docked windows clip each
    /// other by their native order, so it must match the app's.
    #[cfg(plugin_windows)]
    pub fn stack(&self, pl: &Plugins, rows: &[usize]) {
        let hwnds: Vec<isize> = rows.iter().filter_map(|r| self.window_of(*r)).filter(|w| !w.1).map(|w| w.0).collect();
        self.frames.stack(pl.owner, &hwnds);
    }

    /// VIEW > Into workspace for the docked windows: one partly outside the workspace moves fully back in
    /// (`FrameWindow::move_into_workspace`).
    #[cfg(all(plugin_windows, windows))]
    pub fn move_into_workspace(&self) {
        for w in self.editors.values().filter_map(|e| e.window.as_ref()) {
            w.move_into_workspace();
        }
    }

    /// Moves row `row`'s docked window to `(x, y)` (main-window client px; VIEW > Align all channel editors).
    #[cfg(all(plugin_windows, windows))]
    pub fn move_docked(&self, row: usize, x: i32, y: i32) {
        if let Some(w) = self.editors.get(&row).and_then(|e| e.window.as_ref()) {
            w.move_to(x, y);
        }
    }

    /// Clips row `row`'s docked window to `work` minus `covers` (main-window client px).
    #[cfg(plugin_windows)]
    pub fn clip(&self, row: usize, work: [i32; 4], covers: &[[i32; 4]]) {
        if let Some(w) = self.editors.get(&row).and_then(|e| e.window.as_ref()) {
            w.clip_to(work, covers);
        }
    }

    /// Closes row `row`'s plugin window when it is open; returns whether it was.
    pub fn close_if_open(&mut self, pl: &mut Plugins, row: usize) -> bool {
        self.sync(pl);
        #[cfg(plugin_windows)]
        if self.editors.get(&row).is_some_and(|e| e.window.is_some()) {
            self.close_window(pl, row);
            return true;
        }
        let _ = row;
        false
    }

    /// UI tick: plugin housekeeping, then the windows' requests (a closed window is closed here).
    #[cfg_attr(not(plugin_windows), allow(unused_variables, unused_mut))]
    pub fn idle(&mut self, pl: &mut Plugins, info: impl Fn(usize) -> Option<ChannelInfoT>) -> Vec<Request> {
        self.sync(pl);
        sweep_retired();
        os_host::guard::watch_windows(false);
        // Linux: the frames' X11 events, then the editors' descriptors and timers (`os_host::runloop`).
        #[cfg(target_os = "linux")]
        {
            self.frames.pump();
            os_host::runloop::dispatch();
        }
        let mut out = Vec::new();
        let mut close: Vec<usize> = Vec::new();
        #[cfg(any(windows, target_os = "macos"))]
        let mut rescale: Vec<(usize, f32)> = Vec::new();
        // Plugin -> host link messages (a woken tick polls the link below anyway; notes become requests).
        #[cfg(plugin_windows)]
        for (area, m) in self.frames.take_link() {
            let row = self.editors.iter().find(|(_, e)| e.window.as_ref().is_some_and(|w| w.area() as isize == area)).map(|(&r, _)| r);
            match (row, m) {
                (Some(row), crate::plugin_window::LinkMsg::Note(k, v, on)) => out.push(Request::Note(row, k, v, on)),
                (Some(row), crate::plugin_window::LinkMsg::Automate(i)) => out.push(Request::Automate(row, i)),
                (_, crate::plugin_window::LinkMsg::Audio(_)) => out.extend(os_hostlink::take_audio().into_iter().map(|(p, ch)| Request::Audio(p, ch))),
                (Some(row), crate::plugin_window::LinkMsg::Notes(token)) => out.push(Request::Notes(row, os_hostlink::take_notes(token))),
                (None, crate::plugin_window::LinkMsg::Notes(token)) => drop(os_hostlink::take_notes(token)),
                _ => {}
            }
        }
        for (&row, h) in &mut pl.hosted {
            #[cfg_attr(not(plugin_windows), allow(unused_mut, unused_variables))]
            let mut ed = self.editors.get_mut(&row);
            let open = ed.as_ref().is_some_and(|e| e.open());
            #[cfg(plugin_windows)]
            let window = ed.as_deref().and_then(|e| e.window.as_ref().map(|w| (w, e.linked())));
            #[cfg(plugin_windows)]
            let menu_at = |m: &os_host::Menu| {
                window.map(|(w, _)| {
                    let k = w.scale();
                    w.editor_to_screen(m.at.0 as f32 / k, m.at.1 as f32 / k)
                })
            };
            #[cfg(not(plugin_windows))]
            let menu_at = |_: &os_host::Menu| -> Option<(i32, i32)> { None };
            #[cfg(plugin_windows)]
            let size = |(ew, eh): (u32, u32)| {
                if let Some((w, false)) = window {
                    let k = w.scale();
                    w.set_editor_size((ew as f32 / k, eh as f32 / k));
                }
            };
            #[cfg(not(plugin_windows))]
            let size = |_: (u32, u32)| {};
            let (stopped, menu) = Plugins::tick_row(h, row, open, menu_at, size);
            if let Some(said) = stopped {
                out.push(Request::Stopped(row, said));
                close.push(row);
            }
            #[cfg(plugin_windows)]
            if let Some(Editor { window: Some(w), link, .. }) = ed.as_deref_mut() {
                if let Some(i) = info(row) {
                    w.set_info(i);
                }
                if let Some(l) = link.as_mut() {
                    w.set_page(l.page());
                    w.set_hint(l.hint());
                    if let Some((lw, lh)) = l.size() {
                        w.set_editor_size((lw as f32, lh as f32));
                    }
                    // A resizable Lemon plugin (Tesla): the frame's edges resize it.
                    w.set_resizable(l.resizable().map(|(mw, mh)| (mw as f32, mh as f32)));
                }
                for e in w.take_events() {
                    match (&e, &mut *link) {
                        (FrameEvent::Page(n), Some(l)) => {
                            l.set_page(*n);
                            continue;
                        }
                        (FrameEvent::Resize(ew, eh), Some(l)) => {
                            l.set_size(*ew, *eh);
                            continue;
                        }
                        _ => {}
                    }
                    out.push(match e {
                        FrameEvent::Page(_) | FrameEvent::Resize(..) => continue,
                        FrameEvent::Close => {
                            close.push(row);
                            Request::Close(row)
                        }
                        FrameEvent::ToggleOn => Request::ToggleOn(row),
                        FrameEvent::Volume(v) => Request::Volume(row, v),
                        FrameEvent::Pan(v) => Request::Pan(row, v),
                        FrameEvent::PitchCents(c) => Request::PitchCents(row, c),
                        FrameEvent::PitchRange(r) => Request::PitchRange(row, r),
                        FrameEvent::Track(t) => Request::Track(row, t),
                        FrameEvent::ShowTrack => Request::ShowTrack(row),
                        FrameEvent::Raise => Request::Raise(row),
                        FrameEvent::Menu(x, y) => Request::Menu(row, x, y),
                        FrameEvent::Presets(x, y) => Request::Presets(row, x, y),
                        FrameEvent::PresetStep(d) => Request::PresetStep(row, d),
                        FrameEvent::DropFile(p) => Request::DropFile(row, p),
                        FrameEvent::Misc(m, v) => Request::Misc(row, m, v),
                        FrameEvent::MiscMenu(m, x, y) => Request::MiscMenu(row, m, x, y),
                        FrameEvent::KnobMenu(k, x, y) => {
                            use crate::knobs::{Control, KnobId};
                            use crate::plugin_window::Knob;
                            let c = match k {
                                Knob::Pan => Control::Knob(KnobId::ChanPan(row)),
                                Knob::Vol => Control::Knob(KnobId::ChanVol(row)),
                                Knob::Pitch => Control::ChanPitch(row),
                                Knob::Range | Knob::Track => continue,
                            };
                            Request::KnobMenu(row, c, x, y)
                        }
                        FrameEvent::Note(k, v, on) => Request::Note(row, k, v, on),
                        FrameEvent::NotAvailable(name) => Request::NotAvailable(name),
                        FrameEvent::Dpi(k) => {
                            // A Main GUI scaling percentage replaces the monitor's scale (spec: ui/ui-scaling.md §1).
                            #[cfg(any(windows, target_os = "macos"))]
                            rescale.push((row, crate::gui_scale(f64::from(k)) as f32));
                            #[cfg(not(any(windows, target_os = "macos")))]
                            let _ = k;
                            continue;
                        }
                    });
                }
            }
            // After the window's events: the click in the editor that opened the menu raised the window, which closes
            // open popups.
            if let Some((m, at)) = menu {
                out.push(Request::PluginMenu(row, m, at));
            }
        }
        #[cfg(plugin_windows)]
        for row in close {
            self.close_window(pl, row);
        }
        #[cfg(any(windows, target_os = "macos"))]
        for (row, k) in rescale {
            self.rescale_window(pl, row, pl.scale_for(row, k));
        }
        #[cfg(target_os = "linux")]
        self.frames.paint_dirty();
        out
    }
}

/// Frames that record what the editor host asks of them, for tests without a display (control-plane step 10).
#[cfg(all(test, plugin_windows))]
pub mod test_frames {
    use crate::plugin_window::{ChannelInfo, Frame, FrameEvent, FrameWindow, Frames, LinkMsg};
    use os_hostlink::PageIcon;
    use std::cell::RefCell;
    use std::rc::Rc;

    /// What was asked, one line per call.
    pub type Log = Rc<RefCell<Vec<String>>>;

    pub struct Recording(pub Log);

    struct Recorded {
        log: Log,
        hwnd: isize,
        editor: RefCell<(f32, f32)>,
        detached: RefCell<bool>,
    }

    impl Drop for Recorded {
        fn drop(&mut self) {
            self.log.borrow_mut().push(format!("close {}", self.hwnd));
        }
    }

    impl Frame for Recorded {
        fn detached(&self) -> bool {
            *self.detached.borrow()
        }
        fn set_detached(&self, _owner: isize, on: bool) {
            *self.detached.borrow_mut() = on;
            self.log.borrow_mut().push(format!("detached {on}"));
        }
        fn clip_to(&self, _work: [i32; 4], _covers: &[[i32; 4]]) {}
        fn hwnd(&self) -> isize {
            self.hwnd
        }
        fn area(&self) -> *mut std::ffi::c_void {
            std::ptr::null_mut()
        }
        fn raise(&self) {
            self.log.borrow_mut().push(format!("raise {}", self.hwnd));
        }
        fn set_info(&self, info: ChannelInfo) {
            self.log.borrow_mut().push(format!("info {}", info.name));
        }
        fn set_pages(&self, _pages: Vec<PageIcon>, _active: usize) {}
        fn set_resizable(&self, _min: Option<(f32, f32)>) {}
        fn set_page(&self, _page: usize) {}
        fn set_hint(&self, _hint: String) {}
        fn set_preset(&self, _name: Option<String>) {}
        fn scale(&self) -> f32 {
            1.0
        }
        fn editor_to_screen(&self, x: f32, y: f32) -> (i32, i32) {
            (x as i32, y as i32)
        }
        fn set_editor_size(&self, editor: (f32, f32)) {
            *self.editor.borrow_mut() = editor;
            self.log.borrow_mut().push(format!("editor {}x{}", editor.0, editor.1));
        }
        fn set_strip(&self, on: bool) {
            self.log.borrow_mut().push(format!("strip {on}"));
        }
        fn take_events(&self) -> Vec<FrameEvent> {
            Vec::new()
        }
    }

    impl Frames for Recording {
        fn open(&self, _owner: isize, info: ChannelInfo, editor: (f32, f32), _scale: f32, pos: (i32, i32), detached: bool) -> Result<FrameWindow, String> {
            let hwnd = self.0.borrow().iter().filter(|l| l.starts_with("open")).count() as isize + 1;
            self.0.borrow_mut().push(format!("open {hwnd} {} {}x{} at {},{} detached {detached}", info.name, editor.0, editor.1, pos.0, pos.1));
            Ok(Box::new(Recorded { log: self.0.clone(), hwnd, editor: RefCell::new(editor), detached: RefCell::new(detached) }))
        }
        fn default_position(&self, _owner: isize, _scale: f32) -> (i32, i32) {
            (40, 30)
        }
        fn stack(&self, _parent: isize, hwnds: &[isize]) {
            self.0.borrow_mut().push(format!("stack {hwnds:?}"));
        }
        fn take_exposed(&self) -> bool {
            false
        }
        fn take_link(&self) -> Vec<(isize, LinkMsg)> {
            Vec::new()
        }
    }
}

#[cfg(all(test, plugin_windows))]
mod frame_tests {
    use super::test_frames::{Log, Recording};
    use super::*;
    use std::sync::{Arc, Mutex};

    /// A plugin whose editor opens at 300×200 in whatever area it is given.
    struct Editing {
        desc: os_host::Descriptor,
        open: bool,
    }
    impl os_engine::Instrument for Editing {
        fn note_on(&mut self, _: u32, _: u8, _: f32) {}
        fn note_off(&mut self, _: u32, _: u8) {}
        fn process(&mut self, _: u32, _: &mut [f32], _: &mut [f32]) {}
    }
    impl os_engine::HostedEffect for Editing {
        fn process_fx(&mut self, _: u32, _: &mut [f32], _: &mut [f32]) {}
    }
    impl os_host::Plugin for Editing {
        fn desc(&self) -> &os_host::Descriptor {
            &self.desc
        }
        fn activate(&mut self, _: u32) -> bool {
            true
        }
        fn idle(&mut self) {}
        fn has_editor(&self) -> bool {
            true
        }
        fn editor_open(&self) -> bool {
            self.open
        }
        fn open_editor(&mut self, _: *mut std::ffi::c_void, _: f32) -> Result<(u32, u32), String> {
            self.open = true;
            Ok((300, 200))
        }
        fn editor_size(&self) -> Option<(u32, u32)> {
            self.open.then_some((300, 200))
        }
        fn close_editor(&mut self) {
            self.open = false;
        }
        fn save_state(&self) -> Option<Vec<u8>> {
            None
        }
        fn load_state(&mut self, _: &[u8]) -> bool {
            false
        }
        fn param_count(&self) -> usize {
            0
        }
        fn param_info(&self, _: usize) -> Option<os_host::ParamInfo> {
            None
        }
    }

    fn info(name: &str) -> ChannelInfo {
        ChannelInfo { name: name.into(), icon: None, target: "Bus 1".into(), enabled: true, volume: 0.78, pan: 0.0, pitch_cents: 0, range: 2, track: 1, max_track: 125, misc: None }
    }

    /// The editor host opens a plugin's window through the frames it was given, puts the editor in it at the editor's
    /// own size, raises it when asked again, and lets go of it when the window closes: all without a display.
    #[test]
    fn editor_windows_open_through_the_frames_given() {
        let log: Log = Default::default();
        let mut host = EditorHost::default();
        host.frames = Box::new(Recording(log.clone()));
        let mut pl = Plugins::default();
        pl.scale = 1.0;
        let desc = crate::plugins::builtin()[0].desc.clone();
        let p = Arc::new(Mutex::new(Editing { desc, open: false }));
        pl.hosted.insert(0, Hosted::new(p.clone()));

        assert_eq!(host.open_window(&mut pl, 0, info("Keys")), Ok(()));
        assert!(p.lock().unwrap().open, "the editor opened in the frame");
        let seen = log.borrow().clone();
        assert!(seen[0].starts_with("open 1 Keys 560x437 at 40,30 detached false"), "{seen:?}");
        assert!(seen.iter().any(|l| l == "editor 300x200"), "the frame fits the editor: {seen:?}");
        assert!(host.editor_open());

        host.open_window(&mut pl, 0, info("Keys")).unwrap();
        assert_eq!(log.borrow().iter().filter(|l| l.starts_with("open")).count(), 1, "open again raises the same window");
        assert!(log.borrow().iter().any(|l| l == "raise 1"));

        host.close_window(&mut pl, 0);
        assert!(!p.lock().unwrap().open, "the editor closed with its window");
        assert!(log.borrow().iter().any(|l| l == "close 1"), "{:?}", log.borrow());
        assert!(!host.editor_open());
    }

    /// Without the program's frames (`NoFrames`) a window does not open, and the host says so.
    #[test]
    fn without_frames_no_window_opens() {
        let mut host = EditorHost::default();
        let mut pl = Plugins::default();
        let desc = crate::plugins::builtin()[0].desc.clone();
        pl.hosted.insert(0, Hosted::new(Arc::new(Mutex::new(Editing { desc, open: false }))));
        assert!(host.open_window(&mut pl, 0, info("Keys")).is_err());
        assert!(!host.editor_open());
    }
}
