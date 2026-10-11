//! Hosted plugins (any format, through the `os-host` layer): scanning, one instance per plugin channel or mixer slot,
//! their windows, and the instrument / effect lists the transport plays.
//!
// spec: plugins/plugin-formats.md (hosting, where to find plugins, Open Sampler state), ui/plugin-window.md,
// plugins/host-link.md (Open plugins join the window strip)

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use os_host::{Descriptor, Library, SharedPlugin};
use os_engine::{SharedEffect, SharedInstrument};

#[cfg(windows)]
use crate::plugin_window::{ChannelInfo, FrameEvent, FrameWindow};

/// A plugin offered in the picker.
#[derive(Clone)]
pub struct Available {
    pub lib: Arc<dyn Library>,
    pub desc: Descriptor,
}

pub struct Hosted {
    pub plugin: SharedPlugin,
    #[cfg(windows)]
    pub window: Option<FrameWindow>,
    /// Set while the open editor belongs to an Open plugin that joined the strip (host link).
    #[cfg(windows)]
    pub link: Option<os_hostlink::Link>,
}

/// What a plugin window asks of the app.
#[derive(Clone, Debug, PartialEq)]
pub enum Request {
    Close(usize),
    ToggleOn(usize),
    Volume(usize, f64),
    Pan(usize, f64),
    PitchCents(usize, i32),
    /// A preview note from an Open plugin's own keyboard (host link): row, key, velocity, on.
    Note(usize, u8, u8, bool),
    /// "Create automation clip" from an Open plugin's own parameter menu (host link): row, parameter index.
    Automate(usize, usize),
    /// The window was clicked (or docked / detached): it goes to the front of the app's window stack.
    Raise(usize),
    /// The menu arrow was clicked: open the window menu at this screen point.
    Menu(usize, i32, i32),
    /// The title-bar Presets box was clicked: open the preset list at this screen point (spec: plugins/presets.md).
    Presets(usize, i32, i32),
    /// The Presets box arrows: next (+1) / previous (-1) preset.
    PresetStep(usize, i32),
    /// A Misc tab value changed (spec: ui/plugin-window.md "Misc tab").
    Misc(usize, crate::misc_page::M, f32),
    /// A Misc tab list asks for its menu at this screen point.
    MiscMenu(usize, crate::misc_page::M, i32, i32),
}

/// Keys from here up are mixer slots, not rack rows: `FX_KEY + track * 16 + slot` (spec: ui/mixer.md §7).
pub const FX_KEY: usize = 1 << 24;

/// The `hosted` key of mixer track `track`'s slot `slot`.
pub fn fx_key(track: u16, slot: u8) -> usize {
    FX_KEY + usize::from(track) * 16 + usize::from(slot)
}

/// The mixer (track, slot) of a `hosted` key, `None` for rack rows.
pub fn fx_of(key: usize) -> Option<(u16, u8)> {
    (key >= FX_KEY).then(|| (((key - FX_KEY) / 16) as u16, ((key - FX_KEY) % 16) as u8))
}

#[derive(Default)]
pub struct Plugins {
    pub available: Vec<Available>,
    /// Effect plugins found (offered for mixer slots).
    pub effects: Vec<Available>,
    /// By rack row, or by `fx_key` for mixer slots.
    pub hosted: BTreeMap<usize, Hosted>,
    /// Main window handle (owner of plugin windows).
    pub owner: isize,
    pub scale: f32,
    /// New plugin windows open detached (settings "Detach all new plugins"; off = docked like FL's default).
    pub detach_new: bool,
    /// The preset file each instance last loaded or saved (spec: plugins/presets.md), by `hosted` key.
    pub presets: BTreeMap<usize, PathBuf>,
    rate: u32,
}

/// Folders searched for plugin files: a format's standard folders (`standard`), then `bundled/` next to the build
/// folders of this workspace (where `cargo run -p bundle-plugins` puts OpenStudio's own plugins).
fn search_dirs(standard: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut dirs = standard;
    if let Ok(exe) = std::env::current_exe() {
        // target*/release/openstudio.exe -> target*/bundled, and the sibling build folders.
        if let Some(target) = exe.parent().and_then(|p| p.parent()) {
            dirs.push(target.join("bundled"));
            if let Some(ws) = target.parent() {
                for t in ["target", "target-dev"] {
                    dirs.push(ws.join(t).join("bundled"));
                }
            }
        }
    }
    dirs.dedup();
    dirs
}

/// Open Sampler compiled into OpenStudio and hosted through its own CLAP entry, so sampler channels never depend on
/// finding a `.clap` file (spec: plugins/open-sampler.md §7).
pub fn builtin() -> Vec<Available> {
    static LIB: OnceLock<Option<Arc<os_clap::Library>>> = OnceLock::new();
    linked(&LIB, &open_sampler::LINKED_CLAP_ENTRY as *const _ as *const std::ffi::c_void, OPEN_SAMPLER)
}

/// Open Parametric EQ 2, compiled in the same way: offered for every mixer slot, and it plays the slots a project
/// saved with the original EQ (spec: plugins/open-parametric-eq-2.md §8).
pub fn builtin_effects() -> Vec<Available> {
    static LIB: OnceLock<Option<Arc<os_clap::Library>>> = OnceLock::new();
    linked(&LIB, &open_parametric_eq_2::LINKED_CLAP_ENTRY as *const _ as *const std::ffi::c_void, OPEN_EQ)
}

/// The plugins of a CLAP entry table linked into this program.
fn linked(cell: &OnceLock<Option<Arc<os_clap::Library>>>, entry: *const std::ffi::c_void, name: &str) -> Vec<Available> {
    let lib = cell.get_or_init(|| {
        let path = std::env::current_exe().unwrap_or_default();
        // SAFETY: `entry` is a linked plugin's static CLAP entry table.
        match unsafe { os_clap::Library::from_entry(entry, &path) } {
            Ok(l) => Some(l),
            Err(e) => {
                eprintln!("built-in {name}: {e}");
                None
            }
        }
    });
    lib.iter().flat_map(|l| l.plugins.iter().map(|d| Available { lib: l.clone(), desc: d.clone() })).collect()
}

/// One plugin format's files in its search folders, and how to load one.
type Scan = (Vec<PathBuf>, fn(&std::path::Path) -> Result<Arc<dyn Library>, String>);

/// The plugin files of every hosted format (spec: plugins/vst3-hosting.md §2). The user's installed
/// FL native plugins are included only when the experimental native host is opted into
/// (`os_native::enabled()`, Settings > File or `OPENSTUDIO_NATIVE=1`): they are loaded through an
/// unverified interop path (spec plugins/fruity-abi.md), so they stay off by default.
fn plugin_files() -> Vec<Scan> {
    let mut scans: Vec<Scan> = vec![
        (os_clap::find_files(&search_dirs(os_clap::standard_dirs())), |p| os_clap::Library::load(p).map(|l| l as Arc<dyn Library>)),
        (os_vst3::find_bundles(&search_dirs(os_vst3::standard_dirs())), |p| os_vst3::Module::load(p).map(|l| l as Arc<dyn Library>)),
    ];
    if os_native::enabled() {
        // Effects only: native generators are not offered in the UI yet (some abort on load; their
        // output level is not yet right). An in-process native crash would take the app down.
        scans.push((os_native::effect_dll_files(), |p| os_native::NativeLibrary::load(p).map(|l| l as Arc<dyn Library>)));
    }
    scans
}

/// A format's name as the UI shows it.
pub fn format_name(f: os_host::Format) -> &'static str {
    match f {
        os_host::Format::Clap => "CLAP",
        os_host::Format::Vst3 => "VST3",
        os_host::Format::Native => "native",
    }
}

/// Picker labels of found plugins: the name, plus the format when plugins of other formats have the same name.
fn labels(list: &[Available]) -> Vec<String> {
    list.iter()
        .map(|a| {
            let twin = list.iter().any(|b| b.desc.name == a.desc.name && b.desc.format != a.desc.format);
            if twin { format!("{} ({})", a.desc.name, format_name(a.desc.format)) } else { a.desc.name.clone() }
        })
        .collect()
}

impl Plugins {
    /// The built-in Open Sampler, then the plugin folders' instruments (first file wins for a plugin id). Files of the
    /// built-in plugins are skipped (any format): a second copy in the process would share the built-in one's editor
    /// window class.
    pub fn scan() -> Plugins {
        let mut available: Vec<Available> = builtin();
        let mut effects: Vec<Available> = builtin_effects();
        let files = plugin_files().into_iter().flat_map(|(files, load)| files.into_iter().map(move |f| (f, load)));
        for (f, load) in files.filter(|(f, _)| f.file_stem().is_none_or(|s| s != OPEN_SAMPLER && s != OPEN_EQ)) {
            match load(&f) {
                Ok(lib) => {
                    for d in lib.plugins().iter() {
                        let list = if d.is_instrument() {
                            &mut available
                        } else if d.is_effect() {
                            &mut effects
                        } else {
                            continue;
                        };
                        if !list.iter().any(|a| a.desc.uid() == d.uid()) {
                            list.push(Available { lib: lib.clone(), desc: d.clone() });
                        }
                    }
                }
                Err(e) => eprintln!("plugin scan: {e}"),
            }
        }
        effects.sort_by_key(|a| a.desc.name.to_lowercase());
        if std::env::var_os("OS_DEBUG").is_some() {
            let native = effects.iter().filter(|a| a.desc.format == os_host::Format::Native).count();
            eprintln!("[plugins] {} instruments, {} effects ({native} native)", available.len(), effects.len());
        }
        Plugins { available, effects, scale: 1.0, ..Plugins::default() }
    }

    /// Picker labels of the instruments (`labels`), in `available` order.
    pub fn names(&self) -> Vec<String> {
        labels(&self.available)
    }

    /// Picker labels of the effects, in `effects` order.
    pub fn effect_names(&self) -> Vec<String> {
        labels(&self.effects)
    }

    /// Creates an instance of available plugin `index` for rack row `row` and activates it at the device rate.
    pub fn create(&mut self, row: usize, index: usize) -> Result<String, String> {
        self.create_with(row, index, None)
    }

    /// `create` with a starting state (the plugin's own saved state format).
    pub fn create_with(&mut self, row: usize, index: usize, state: Option<&[u8]>) -> Result<String, String> {
        let a = self.available.get(index).ok_or("no such plugin")?.clone();
        let plugin = a.lib.clone().create(&a.desc.id)?;
        let mut p = plugin.lock().map_err(|_| "plugin busy")?;
        if let Some(st) = state {
            if !p.load_state(st) {
                eprintln!("[plugins] {} did not take its starting state", a.desc.name);
            }
        }
        if self.rate == 0 {
            self.rate = crate::transport::Transport::device_rate().unwrap_or(48000);
        }
        if std::env::var_os("OS_DEBUG").is_some() {
            eprintln!("[plugins] {} activated at {} Hz", a.desc.name, self.rate);
        }
        if !p.activate(self.rate) {
            return Err(format!("{} could not start", a.desc.name));
        }
        // Trimmed audio clips start their note further into the sample (spec: plugins/open-sampler.md §7).
        p.use_start_param(SAMPLE_START);
        drop(p);
        self.hosted.insert(row, Hosted { plugin, #[cfg(windows)] window: None, #[cfg(windows)] link: None });
        Ok(a.desc.name)
    }

    /// Creates effect `index` (of `effects`) in mixer track `track`'s slot `slot`, replacing what was there.
    pub fn create_effect(&mut self, track: u16, slot: u8, index: usize, state: Option<&[u8]>) -> Result<String, String> {
        let a = self.effects.get(index).ok_or("no such effect")?.clone();
        let key = fx_key(track, slot);
        self.drop_key(key);
        let plugin = a.lib.clone().create(&a.desc.id)?;
        let mut p = plugin.lock().map_err(|_| "plugin busy")?;
        if let Some(st) = state {
            p.load_state(st);
        }
        if self.rate == 0 {
            self.rate = crate::transport::Transport::device_rate().unwrap_or(48000);
        }
        if !p.activate(self.rate) {
            return Err(format!("{} could not start", a.desc.name));
        }
        drop(p);
        self.hosted.insert(key, Hosted { plugin, #[cfg(windows)] window: None, #[cfg(windows)] link: None });
        Ok(a.desc.name)
    }

    /// Removes the instance under `key` (closing its window first).
    pub fn drop_key(&mut self, key: usize) {
        #[cfg(windows)]
        self.close_window(key);
        self.hosted.remove(&key);
        self.presets.remove(&key);
    }

    /// Remembers the preset the instance under `key` holds and shows its name in the window's Presets box.
    pub fn set_preset(&mut self, key: usize, path: Option<PathBuf>) {
        match path {
            Some(p) => self.presets.insert(key, p),
            None => self.presets.remove(&key),
        };
        #[cfg(windows)]
        if let Some(w) = self.hosted.get(&key).and_then(|h| h.window.as_ref()) {
            w.set_preset(self.preset_name(key));
        }
    }

    /// The name of the preset the instance under `key` holds.
    pub fn preset_name(&self, key: usize) -> Option<String> {
        self.presets.get(&key).map(|p| crate::presets::shown_name(p))
    }

    /// Moves mixer slot instances: `m(track, slot)` gives the new place, `None` drops the instance.
    pub fn remap_fx(&mut self, m: &dyn Fn(u16, u8) -> Option<(u16, u8)>) {
        let keys: Vec<usize> = self.hosted.keys().copied().filter(|&k| k >= FX_KEY).collect();
        let mut moved = Vec::new();
        for k in keys {
            let (t, s) = fx_of(k).unwrap();
            match m(t, s) {
                Some((nt, ns)) if (nt, ns) == (t, s) => {}
                Some((nt, ns)) => {
                    if let Some(p) = self.presets.remove(&k) {
                        self.presets.insert(fx_key(nt, ns), p);
                    }
                    moved.push((fx_key(nt, ns), self.hosted.remove(&k).unwrap()))
                }
                None => self.drop_key(k),
            }
        }
        for (k, h) in moved {
            self.hosted.insert(k, h);
        }
    }

    /// The mixer slot instances for the transport: (track, slot, effect).
    pub fn effects(&self) -> Vec<(usize, usize, SharedEffect)> {
        self.hosted
            .range(FX_KEY..)
            .map(|(&k, h)| {
                let (t, s) = fx_of(k).unwrap();
                (usize::from(t), usize::from(s), h.plugin.clone() as SharedEffect)
            })
            .collect()
    }

    /// Index of the available plugin called `name`.
    pub fn index_of(&self, name: &str) -> Option<usize> {
        // A picker label first (it names the format of a plugin found in several), then the plain name.
        labels(&self.available).iter().position(|l| l == name).or_else(|| self.available.iter().position(|a| a.desc.name == name))
    }

    /// Index of the effect plugin called `name`.
    pub fn effect_index(&self, name: &str) -> Option<usize> {
        self.effects.iter().position(|a| a.desc.name == name)
    }

    /// Index of the available instrument with identity `uid` (`Descriptor::uid`, e.g. `vst3:<class id>`).
    pub fn index_of_uid(&self, uid: &str) -> Option<usize> {
        self.available.iter().position(|a| a.desc.uid() == uid)
    }

    /// Index of the effect plugin with identity `uid`.
    pub fn effect_index_of_uid(&self, uid: &str) -> Option<usize> {
        self.effects.iter().position(|a| a.desc.uid() == uid)
    }

    /// What the instance under `key` (rack row or `fx_key`) is: format, id, name, file (for project files).
    pub fn descriptor(&self, key: usize) -> Option<Descriptor> {
        Some(self.hosted.get(&key)?.plugin.lock().ok()?.desc().clone())
    }

    /// The saved state of the instance under `key`, in its format's own bytes.
    pub fn state(&self, key: usize) -> Option<Vec<u8>> {
        self.hosted.get(&key)?.plugin.lock().ok()?.save_state()
    }

    /// Restores a `state` blob into the instance under `key`.
    pub fn set_state(&self, key: usize, data: &[u8]) -> bool {
        self.hosted.get(&key).and_then(|h| h.plugin.lock().ok()).is_some_and(|mut p| p.load_state(data))
    }

    /// Sets the parameter named `name` of the instance under `key` to plain value `plain` (the plugin's own range).
    /// False when the instance or the parameter is missing.
    pub fn set_param_named(&self, key: usize, name: &str, plain: f64) -> bool {
        let Some(mut p) = self.hosted.get(&key).and_then(|h| h.plugin.lock().ok()) else { return false };
        let Some(info) = (0..p.param_count()).filter_map(|i| p.param_info(i)).find(|i| i.name == name) else { return false };
        let n = if info.max > info.min { (plain - info.min) / (info.max - info.min) } else { 0.0 };
        p.set_param(info.id, n.clamp(0.0, 1.0))
    }

    /// The project-file state of the Open Parametric EQ 2 in mixer slot `key` (its plugin state converted back),
    /// `None` when the slot hosts something else.
    pub fn eq_file_state(&self, key: usize) -> Option<Vec<u8>> {
        let h = self.hosted.get(&key)?;
        let p = h.plugin.lock().ok()?;
        if p.desc().name != OPEN_EQ {
            return None;
        }
        open_parametric_eq_2::params::fl_from_plugin_state(&p.save_state()?)
    }

    /// Loads sample `path` into row `row`'s Open Sampler: its current state with a new `sample-path`, so every other
    /// setting stays. False when the row hosts nothing or the plugin refused the state.
    pub fn load_sample(&mut self, row: usize, path: &str) -> bool {
        let Some(h) = self.hosted.get(&row) else { return false };
        let Ok(mut p) = h.plugin.lock() else { return false };
        let Some(new) = p.save_state().and_then(|b| with_sample_path(&b, path)) else { return false };
        p.load_state(&new)
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
            if let Some((&r, _)) = self.hosted.iter().find(|(_, h)| h.window.as_ref().is_some_and(|f| f.hwnd() == w as isize)) {
                return Some(r);
            }
            w = unsafe { GetAncestor(w, GA_PARENT) };
        }
        None
    }

    /// Drops file `path` on row `row`'s plugin window at screen point (x, y), as Explorer would: the window under the
    /// point that accepts files gets `WM_DROPFILES`. Open Sampler rows without one (the strip, the frame) load the sample
    /// through their state. Returns whether the plugin got the file.
    #[cfg(windows)]
    pub fn drop_file(&mut self, row: usize, x: i32, y: i32, path: &str, sampler: bool) -> bool {
        if self.hosted.get(&row).and_then(|h| h.window.as_ref()).is_some_and(|f| post_drop(x, y, path, f.hwnd())) {
            return true;
        }
        sampler && self.load_sample(row, path)
    }

    /// Closes every window and drops every instance (a new or opened project).
    pub fn clear(&mut self) {
        #[cfg(windows)]
        for row in self.hosted.keys().copied().collect::<Vec<_>>() {
            self.close_window(row);
        }
        self.hosted.clear();
        self.presets.clear();
    }

    /// The sample file an Open Sampler instance holds (its persisted `sample-path`), `Some("")` when it has none.
    pub fn sampler_path(&self, row: usize) -> Option<String> {
        let h = self.hosted.get(&row)?;
        let blob = h.plugin.lock().ok()?.save_state()?;
        sample_path_of(&blob)
    }

    /// The instances for the transport.
    pub fn instruments(&self) -> Vec<(usize, SharedInstrument)> {
        self.hosted.range(..FX_KEY).map(|(&r, h)| (r, h.plugin.clone() as SharedInstrument)).collect()
    }

    /// Opens (or raises) row `row`'s plugin window.
    #[cfg(windows)]
    pub fn open_window(&mut self, row: usize, info: ChannelInfo) -> Result<(), String> {
        let (owner, scale) = (self.owner, self.scale);
        let h = self.hosted.get_mut(&row).ok_or("not a plugin channel")?;
        if let Some(w) = &h.window {
            w.raise();
            return Ok(());
        }
        let mut p = h.plugin.lock().map_err(|_| "plugin busy")?;
        let pos = crate::plugin_window::default_position(owner);
        // Create the frame at a provisional size, embed the editor, then fit the frame to the editor's real size.
        let w = FrameWindow::new(owner, info, (560.0, 437.0), scale, pos, self.detach_new)?;
        // Mixer effects have no channel strip (spec: ui/mixer.md §7).
        if row >= FX_KEY {
            w.set_strip(false);
        }
        let (ew, eh) = p.open_editor(w.area())?;
        // An Open plugin answers the hello: its page tabs go into the strip row and its editor shrinks to what is below.
        h.link = os_hostlink::Link::connect(w.area() as isize);
        match &h.link {
            Some(l) => {
                w.set_pages(l.pages.clone(), l.page_now());
                let (lw, lh) = l.size().unwrap_or((ew, eh));
                w.set_editor_size((lw as f32, lh as f32));
            }
            None => w.set_editor_size((ew as f32 / scale, eh as f32 / scale)),
        }
        w.show();
        drop(p);
        h.window = Some(w);
        let name = self.preset_name(row);
        if let Some(w) = self.hosted.get(&row).and_then(|h| h.window.as_ref()) {
            w.set_preset(name);
        }
        Ok(())
    }

    #[cfg(windows)]
    fn close_window(&mut self, row: usize) {
        if let Some(h) = self.hosted.get_mut(&row) {
            if let Ok(mut p) = h.plugin.lock() {
                p.close_editor();
            }
            h.window = None;
            h.link = None;
        }
    }

    /// Row `row`'s open window (native frame handle) and whether it is detached.
    #[cfg(windows)]
    pub fn window_of(&self, row: usize) -> Option<(isize, bool)> {
        self.hosted.get(&row).and_then(|h| h.window.as_ref()).map(|w| (w.hwnd(), w.detached()))
    }

    /// Docks or detaches row `row`'s window (window menu > Detached).
    #[cfg(windows)]
    pub fn set_detached(&self, row: usize, on: bool) {
        if let Some(w) = self.hosted.get(&row).and_then(|h| h.window.as_ref()) {
            w.set_detached(self.owner, on);
        }
    }

    /// Rows whose plugin window is open and docked in the main window.
    #[cfg(windows)]
    pub fn docked(&self) -> Vec<usize> {
        self.hosted.iter().filter(|(_, h)| h.window.as_ref().is_some_and(|w| !w.detached())).map(|(&r, _)| r).collect()
    }

    /// Clips row `row`'s docked window to `work` minus `covers` (main-window client px).
    #[cfg(windows)]
    pub fn clip(&self, row: usize, work: [i32; 4], covers: &[[i32; 4]]) {
        if let Some(w) = self.hosted.get(&row).and_then(|h| h.window.as_ref()) {
            w.clip_to(work, covers);
        }
    }

    /// Rack rows were renumbered (`m(old) = new`; `None` drops the instance and its window).
    pub fn remap(&mut self, m: &dyn Fn(usize) -> Option<usize>) {
        for (row, h) in std::mem::take(&mut self.hosted) {
            if row >= FX_KEY {
                self.hosted.insert(row, h);
            } else if let Some(n) = m(row) {
                self.hosted.insert(n, h);
            }
        }
    }

    /// Closes row `row`'s plugin window when it is open; returns whether it was.
    pub fn close_if_open(&mut self, row: usize) -> bool {
        #[cfg(windows)]
        if self.hosted.get(&row).is_some_and(|h| h.window.is_some()) {
            self.close_window(row);
            return true;
        }
        let _ = row;
        false
    }

    /// UI tick: plugin housekeeping, then the windows' requests (a closed window is closed here).
    pub fn idle(&mut self, info: impl Fn(usize) -> Option<ChannelInfoT>) -> Vec<Request> {
        let mut out = Vec::new();
        let mut close: Vec<usize> = Vec::new();
        // Plugin -> host link messages (a woken tick polls the link below anyway; notes become requests).
        #[cfg(windows)]
        for (area, m) in crate::plugin_window::take_link() {
            let row = self.hosted.iter().find(|(_, h)| h.window.as_ref().is_some_and(|w| w.area() as isize == area)).map(|(&r, _)| r);
            match (row, m) {
                (Some(row), crate::plugin_window::LinkMsg::Note(k, v, on)) => out.push(Request::Note(row, k, v, on)),
                (Some(row), crate::plugin_window::LinkMsg::Automate(i)) => out.push(Request::Automate(row, i)),
                _ => {}
            }
        }
        for (&row, h) in &mut self.hosted {
            if let Ok(mut p) = h.plugin.try_lock() {
                p.idle();
                #[cfg(windows)]
                if let (Some(w), None, Some((ew, eh))) = (&h.window, &h.link, p.editor_size()) {
                    w.set_editor_size((ew as f32 / self.scale, eh as f32 / self.scale));
                }
            }
            #[cfg(windows)]
            if let Some(w) = &h.window {
                if let Some(i) = info(row) {
                    w.set_info(i);
                }
                if let Some(l) = &mut h.link {
                    w.set_page(l.page());
                    w.set_hint(l.hint());
                    if let Some((lw, lh)) = l.size() {
                        w.set_editor_size((lw as f32, lh as f32));
                    }
                }
                for e in w.take_events() {
                    if let (FrameEvent::Page(n), Some(l)) = (&e, &mut h.link) {
                        l.set_page(*n);
                        continue;
                    }
                    out.push(match e {
                        FrameEvent::Page(_) => continue,
                        FrameEvent::Close => {
                            close.push(row);
                            Request::Close(row)
                        }
                        FrameEvent::ToggleOn => Request::ToggleOn(row),
                        FrameEvent::Volume(v) => Request::Volume(row, v),
                        FrameEvent::Pan(v) => Request::Pan(row, v),
                        FrameEvent::PitchCents(c) => Request::PitchCents(row, c),
                        FrameEvent::Raise => Request::Raise(row),
                        FrameEvent::Menu(x, y) => Request::Menu(row, x, y),
                        FrameEvent::Presets(x, y) => Request::Presets(row, x, y),
                        FrameEvent::PresetStep(d) => Request::PresetStep(row, d),
                        FrameEvent::Misc(m, v) => Request::Misc(row, m, v),
                        FrameEvent::MiscMenu(m, x, y) => Request::MiscMenu(row, m, x, y),
                        FrameEvent::Note(k, v, on) => Request::Note(row, k, v, on),
                    });
                }
            }
        }
        #[cfg(windows)]
        for row in close {
            self.close_window(row);
        }
        out
    }
}

impl Drop for Hosted {
    /// The editor closes before its frame window goes (a frame destroyed under an open editor took the editor's
    /// window with it, and the plugin's own close then crashed at exit).
    fn drop(&mut self) {
        #[cfg(windows)]
        if self.window.is_some() {
            if let Ok(mut p) = self.plugin.lock() {
                p.close_editor();
            }
        }
    }
}

/// Name of OpenStudio's sampler plugin: it plays and edits every sampler channel (spec: plugins/open-sampler.md §7).
pub const OPEN_SAMPLER: &str = "Open Sampler";
/// Name of OpenStudio's parametric EQ plugin.
pub const OPEN_EQ: &str = "Open Parametric EQ 2";
/// The plugin name a project file stores for the parametric EQ (file-format identifier, kept for interop): slots
/// with it are played by Open Parametric EQ 2 (spec: plugins/open-parametric-eq-2.md §8).
pub const EQ_FILE_NAME: &str = "Fruity Parametric EQ 2";

/// What the mixer shows for a slot's stored plugin name.
pub fn slot_label(name: &str) -> &str {
    if name == EQ_FILE_NAME { OPEN_EQ } else { name }
}

/// A new slot for effect `name`: what the project stores (Open Parametric EQ 2 as the file's EQ with its default
/// state, so the file opens elsewhere; other effects by name).
pub fn new_slot(name: &str) -> os_model::SlotPlugin {
    if name == OPEN_EQ {
        os_model::SlotPlugin { name: EQ_FILE_NAME.into(), state: open_parametric_eq_2::params::default_state() }
    } else {
        os_model::SlotPlugin { name: name.into(), state: Vec::new() }
    }
}
/// The sampler parameter a note's sample start modulates per note (Open Sampler's START OFFSET).
const SAMPLE_START: &str = "Sample start offset";

/// Open Sampler's starting state for a sampler channel of the project: its sample and its five envelope records
/// (nih-plug JSON state with the u64 length prefix of its CLAP state; spec: plugins/open-sampler.md §6, §7).
pub fn sampler_state(ch: &os_model::Channel) -> Vec<u8> {
    let mut params = Vec::new();
    // Model records: panning, volume, cutoff (Mod X), resonance (Mod Y), pitch.
    for (e, pre) in ch.envelopes.iter().zip(["envpan", "envvol", "envmodx", "envmody", "envpitch"]) {
        let f = |v: f32| format!("{{\"f32\":{v}}}");
        let time = |field: i32| f(field.max(0) as f32 / 65536.0);
        params.push(format!("\"{pre}_on\":{{\"bool\":{}}}", e.enabled));
        for (id, v) in [
            ("delay", time(e.delay)),
            ("att", time(e.attack)),
            ("hold", time(e.hold)),
            ("dec", time(e.decay)),
            ("sus", f(e.sustain.clamp(0, 128) as f32 / 128.0)),
            ("rel", time(e.release)),
            ("att_t", f(e.attack_tension.clamp(-128, 128) as f32 / 128.0)),
            ("dec_t", f(e.decay_tension.clamp(-128, 128) as f32 / 128.0)),
            ("rel_t", f(e.release_tension.clamp(-128, 128) as f32 / 128.0)),
            ("amt", f(e.amount.clamp(-128, 128) as f32 / 128.0)),
        ] {
            params.push(format!("\"{pre}_{id}\":{v}"));
        }
    }
    // Factory placeholders (`%FLStudioFactoryData%`...) become real paths: the plugin opens files directly.
    let path = resolve_sample(ch.sample_path.as_deref().unwrap_or(""));
    let path = path.as_str();
    // The persisted field is a JSON string holding JSON.
    let inner = json_string(path);
    let state = format!("{{\"version\":\"0.0.1\",\"params\":{{{}}},\"fields\":{{\"sample-path\":{}}}}}", params.join(","), json_string(&inner));
    let mut blob = (state.len() as u64).to_le_bytes().to_vec();
    blob.extend_from_slice(state.as_bytes());
    blob
}

/// A project sample path as a file path (placeholders resolved against the install folder, else unchanged).
pub fn resolve_sample(path: &str) -> String {
    if path.is_empty() {
        return String::new();
    }
    os_engine::SampleLibrary::new(os_engine::SampleLibrary::default_factory_dir())
        .resolve(path)
        .map_or_else(|| path.to_string(), |p| p.to_string_lossy().into_owned())
}

fn json_string(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

/// `blob` (an Open Sampler state) with its `sample-path` field replaced by `path`.
pub(crate) fn with_sample_path(blob: &[u8], path: &str) -> Option<Vec<u8>> {
    let prefixed = blob.len() > 8 && blob[0] != b'{';
    let json = if prefixed { &blob[8..] } else { blob };
    let mut v: serde_json::Value = serde_json::from_slice(json).ok()?;
    let inner = serde_json::to_string(path).ok()?;
    v.get_mut("fields")?.as_object_mut()?.insert("sample-path".into(), serde_json::Value::String(inner));
    let out = serde_json::to_vec(&v).ok()?;
    let mut b = if prefixed { (out.len() as u64).to_le_bytes().to_vec() } else { Vec::new() };
    b.extend_from_slice(&out);
    Some(b)
}

/// Posts `WM_DROPFILES` for `path` to the window at screen point (x, y), or its nearest ancestor that accepts files
/// (`DragAcceptFiles`). The drop handle is ours: the receiver's `DragFinish` frees it. Same process only.
#[cfg(windows)]
/// `frame` bounds the search: a docked frame's parent is the main window, which takes files itself.
fn post_drop(x: i32, y: i32, path: &str, frame: isize) -> bool {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::Foundation::GlobalFree;
    use windows_sys::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GHND};
    use windows_sys::Win32::UI::Shell::DROPFILES;
    use windows_sys::Win32::UI::WindowsAndMessaging::*;
    unsafe {
        let mut w = WindowFromPoint(POINT { x, y });
        while !w.is_null() && GetWindowLongW(w, GWL_EXSTYLE) as u32 & WS_EX_ACCEPTFILES == 0 {
            if w as isize == frame {
                return false;
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

/// The `sample-path` field of an Open Sampler state blob.
fn sample_path_of(blob: &[u8]) -> Option<String> {
    let json = blob.get(8..).filter(|_| blob.len() > 8 && blob[0] != b'{').unwrap_or(blob);
    let v: serde_json::Value = serde_json::from_slice(json).ok()?;
    let field = v.get("fields")?.get("sample-path")?.as_str()?;
    serde_json::from_str::<String>(field).ok()
}

#[cfg(windows)]
pub type ChannelInfoT = ChannelInfo;
#[cfg(not(windows))]
pub type ChannelInfoT = ();

/// Opens the bundled Open Sampler in a real plugin window, clicks through the strip and captures the window
/// (`OS_SHOT_DIR=<dir> OS_CLAP=<Open Sampler.clap> cargo test -p os-app plugin_window_shots -- --ignored`).
#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::ffi::c_void;
    use std::time::{Duration, Instant};
    use windows_sys::Win32::Foundation::{HWND, RECT};
    use windows_sys::Win32::Graphics::Gdi::*;
    use windows_sys::Win32::Storage::Xps::PrintWindow;
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    fn pump(plugins: &mut Plugins, ms: u64) {
        let end = Instant::now() + Duration::from_millis(ms);
        while Instant::now() < end {
            unsafe {
                let mut m: MSG = std::mem::zeroed();
                while PeekMessageW(&mut m, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                    TranslateMessage(&m);
                    DispatchMessageW(&m);
                }
            }
            plugins.idle(|_| None);
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn click(hwnd: isize, x: i32, y: i32) {
        unsafe { SendMessageW(hwnd as HWND, WM_LBUTTONDOWN, 1, (x | (y << 16)) as isize) };
        unsafe { SendMessageW(hwnd as HWND, WM_LBUTTONUP, 0, (x | (y << 16)) as isize) };
    }

    /// PrintWindow of `hwnd` into a top-down 32-bit BMP.
    fn capture(hwnd: isize, path: &std::path::Path) {
        unsafe {
            let mut r: RECT = std::mem::zeroed();
            GetWindowRect(hwnd as HWND, &mut r);
            let (w, h) = (r.right - r.left, r.bottom - r.top);
            let dc = GetDC(std::ptr::null_mut());
            let mem = CreateCompatibleDC(dc);
            let mut bmi: BITMAPINFO = std::mem::zeroed();
            bmi.bmiHeader = BITMAPINFOHEADER { biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32, biWidth: w, biHeight: -h, biPlanes: 1, biBitCount: 32, biCompression: BI_RGB, ..std::mem::zeroed() };
            let mut bits: *mut c_void = std::ptr::null_mut();
            let bmp = CreateDIBSection(dc, &bmi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
            let old = SelectObject(mem, bmp);
            PrintWindow(hwnd as HWND, mem, 2);
            let px = std::slice::from_raw_parts(bits as *const u8, (w * h * 4) as usize);
            let mut f = Vec::new();
            f.extend_from_slice(b"BM");
            f.extend_from_slice(&((54 + px.len()) as u32).to_le_bytes());
            f.extend_from_slice(&[0, 0, 0, 0]);
            f.extend_from_slice(&54u32.to_le_bytes());
            f.extend_from_slice(&40u32.to_le_bytes());
            f.extend_from_slice(&w.to_le_bytes());
            f.extend_from_slice(&(-h).to_le_bytes());
            f.extend_from_slice(&1u16.to_le_bytes());
            f.extend_from_slice(&32u16.to_le_bytes());
            f.extend_from_slice(&[0; 24]);
            f.extend_from_slice(px);
            std::fs::write(path, f).unwrap();
            SelectObject(mem, old);
            DeleteObject(bmp);
            DeleteDC(mem);
            ReleaseDC(std::ptr::null_mut(), dc);
        }
    }

    #[test]
    #[ignore]
    fn plugin_window_shots() {
        let (Some(dir), Some(clap)) = (std::env::var_os("OS_SHOT_DIR"), std::env::var_os("OS_CLAP")) else { return };
        let dir = std::path::PathBuf::from(dir);
        let lib = os_clap::Library::load(std::path::Path::new(&clap)).expect("load clap");
        let desc = lib.plugins[0].clone();
        let mut pl = Plugins { available: vec![Available { lib, desc }], scale: 1.0, ..Plugins::default() };
        pl.create(0, 0).unwrap();
        let info = ChannelInfo { name: "Open Sampler".into(), target: "Insert 1".into(), enabled: true, volume: 0.78, pan: 0.0, pitch_cents: 0, range: 2, track: 1, misc: None };
        pl.open_window(0, info).unwrap();
        pump(&mut pl, 400);
        let hwnd = pl.hosted[&0].window.as_ref().unwrap().hwnd();
        assert!(pl.hosted[&0].link.is_some(), "Open Sampler should join the strip");
        capture(hwnd, &dir.join("win_sample.bmp"));
        click(hwnd, 68, 47); // second page tab
        pump(&mut pl, 300);
        capture(hwnd, &dir.join("win_envelope.bmp"));
        click(hwnd, 111, 47); // Misc wrench
        pump(&mut pl, 300);
        capture(hwnd, &dir.join("win_misc.bmp"));
        click(hwnd, 24, 12); // gear: hide the strip
        pump(&mut pl, 300);
        capture(hwnd, &dir.join("win_nostrip.bmp"));
    }

    /// The built-in Open Parametric EQ 2 in a mixer slot window: the frame without a strip, the editor inside it, and a
    /// click on a band's switch reaching the plugin state.
    /// `OS_SHOT_DIR=.. cargo test -p os-app --bin openstudio eq_window_shots -- --ignored`
    #[test]
    #[ignore]
    fn eq_window_shots() {
        let Some(dir) = std::env::var_os("OS_SHOT_DIR") else { return };
        let dir = std::path::PathBuf::from(dir);
        let mut pl = Plugins { effects: builtin_effects(), scale: 1.0, rate: 48000, ..Plugins::default() };
        let index = pl.effect_index(OPEN_EQ).expect("built-in EQ");
        pl.create_effect(1, 0, index, None).unwrap();
        let key = fx_key(1, 0);
        let info = ChannelInfo { name: OPEN_EQ.into(), target: "Insert 1".into(), enabled: true, volume: 0.8, pan: 0.0, pitch_cents: 0, range: 2, track: 1, misc: None };
        pl.open_window(key, info).unwrap();
        pump(&mut pl, 400);
        let frame = pl.hosted[&key].window.as_ref().unwrap().hwnd();
        capture(frame, &dir.join("eq_window.bmp"));
        let editor = unsafe { GetWindow(GetWindow(frame as HWND, GW_CHILD), GW_CHILD) } as isize;
        assert!(editor != 0, "the editor is a child of the frame");
        let mask = |pl: &Plugins| pl.eq_file_state(key).map(|b| b[318]);
        assert_eq!(mask(&pl), Some(0x7f));
        // Band 3's switch (editor coordinates, spec: plugins/open-parametric-eq-2.md §3): mutes band 3.
        click(editor, 549 + 21 * 2 + 6, 296);
        pump(&mut pl, 200);
        capture(frame, &dir.join("eq_window_muted.bmp"));
        assert_eq!(mask(&pl), Some(0x7f & !4));
    }

    /// Random clicks, drags and wheel turns on the editor; a probe toggle must keep changing the plugin state.
    /// `OS_SHOT_DIR=.. OS_CLAP=.. cargo test -p os-app plugin_editor_stress -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn plugin_editor_stress() {
        let Some(clap) = std::env::var_os("OS_CLAP") else { return };
        // Right clicks and list buttons open modal menus; the plugin skips them under this switch.
        unsafe { std::env::set_var("OPENSAMPLER_NO_MODAL", "1") };
        let lib = os_clap::Library::load(std::path::Path::new(&clap)).expect("load clap");
        let desc = lib.plugins[0].clone();
        let mut pl = Plugins { available: vec![Available { lib, desc }], scale: 1.0, ..Plugins::default() };
        pl.create(0, 0).unwrap();
        let info = ChannelInfo { name: "Open Sampler".into(), target: "Insert 1".into(), enabled: true, volume: 0.78, pan: 0.0, pitch_cents: 0, range: 2, track: 1, misc: None };
        pl.open_window(0, info).unwrap();
        pump(&mut pl, 300);
        let frame = pl.hosted[&0].window.as_ref().unwrap().hwnd();
        let editor = unsafe { GetWindow(GetWindow(frame as HWND, GW_CHILD), GW_CHILD) } as isize;
        assert!(editor != 0);
        let mut seed = 0x2545F4914F6CDD1Du64;
        let mut rnd = move |n: i32| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % n as u64) as i32
        };
        let state = |pl: &Plugins| pl.hosted[&0].plugin.lock().unwrap().save_state().unwrap();
        let send = |msg: u32, wp: usize, x: i32, y: i32| unsafe { SendMessageW(editor as HWND, msg, wp, (x | (y << 16)) as isize) };
        // Probe on the sample page: the "Normalize" radio must change the plugin state, twice (on, off).
        let probe = |pl: &mut Plugins| -> bool {
            pl.hosted.get_mut(&0).unwrap().link.as_mut().unwrap().set_page(0);
            pump(pl, 40);
            let before = state(pl);
            send(WM_LBUTTONDOWN, 1, 298, 149);
            send(WM_LBUTTONUP, 0, 298, 149);
            pump(pl, 40);
            let changed = state(pl) != before;
            send(WM_LBUTTONDOWN, 1, 298, 149);
            send(WM_LBUTTONUP, 0, 298, 149);
            pump(pl, 40);
            changed
        };
        assert!(probe(&mut pl), "the probe does not work on a fresh editor");
        let mut failed = None;
        for i in 0..4000 {
            // Not the File row (y < 45): its open button pops a modal file dialog.
            let (x, y) = (rnd(560), 45 + rnd(345));
            match rnd(8) {
                0 | 1 => {
                    send(WM_LBUTTONDOWN, 1, x, y);
                    send(WM_LBUTTONUP, 0, x, y);
                }
                2 => {
                    send(WM_LBUTTONDOWN, 1, x, y);
                    for _ in 0..rnd(30) {
                        send(WM_MOUSEMOVE, 1, x + rnd(5), y - rnd(80));
                    }
                    send(WM_LBUTTONUP, 0, x, y);
                }
                3 => {
                    send(WM_LBUTTONDBLCLK, 1, x, y);
                    send(WM_LBUTTONUP, 0, x, y);
                }
                4 => {
                    send(WM_RBUTTONDOWN, 2, x, y);
                    send(WM_RBUTTONUP, 0, x, y);
                }
                5 => {
                    send(WM_MBUTTONDOWN, 16, x, y);
                    send(WM_MBUTTONUP, 0, x, y);
                }
                6 => {
                    // Switch page through the host link.
                    if let Some(l) = &mut pl.hosted.get_mut(&0).unwrap().link {
                        l.set_page(rnd(2) as usize);
                    }
                }
                _ => {
                    send(WM_MOUSEMOVE, 0, x, y);
                }
            }
            if i % 20 == 0 {
                pump(&mut pl, 15);
            }
            if i % 100 == 99 && !probe(&mut pl) && failed.is_none() {
                failed = Some(i);
            }
        }
        assert!(failed.is_none(), "the editor stopped responding after {:?} events", failed);
    }

    /// Real mouse input (moves the desktop cursor): dragging the START OFFSET knob hides the cursor and holds it at the
    /// press point, the value follows the movement, and releasing shows the cursor at the same place.
    /// `OS_CLAP=.. cargo test -p os-app plugin_knob_drag_holds_the_cursor -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn plugin_knob_drag_holds_the_cursor() {
        use windows_sys::Win32::Foundation::POINT;
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::{SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MOVE, MOUSEINPUT};
        let Some(clap) = std::env::var_os("OS_CLAP") else { return };
        let lib = os_clap::Library::load(std::path::Path::new(&clap)).expect("load clap");
        let desc = lib.plugins[0].clone();
        let mut pl = Plugins { available: vec![Available { lib, desc }], scale: 1.0, ..Plugins::default() };
        pl.create(0, 0).unwrap();
        let info = ChannelInfo { name: "Open Sampler".into(), target: "Insert 1".into(), enabled: true, volume: 0.78, pan: 0.0, pitch_cents: 0, range: 2, track: 1, misc: None };
        pl.open_window(0, info).unwrap();
        pump(&mut pl, 300);
        let frame = pl.hosted[&0].window.as_ref().unwrap().hwnd();
        let editor = unsafe { GetWindow(GetWindow(frame as HWND, GW_CHILD), GW_CHILD) };
        unsafe { SetForegroundWindow(frame as HWND) };
        pump(&mut pl, 200);
        let state = |pl: &Plugins| String::from_utf8_lossy(&pl.hosted[&0].plugin.lock().unwrap().save_state().unwrap()).to_string();
        let offset = |s: &str| -> f32 {
            let i = s.find("\"start_offset\"").expect("start_offset in state");
            let rest = &s[i + 14..];
            let rest = &rest[rest.find(':').map_or(0, |j| j + 1)..];
            let rest = &rest[rest.find(':').map_or(0, |j| j + 1)..];
            let num: String = rest.chars().skip_while(|c| !(c.is_ascii_digit() || *c == '-')).take_while(|c| c.is_ascii_digit() || matches!(c, '.' | '-' | 'e')).collect();
            num.parse().unwrap_or(-1.0)
        };
        let before = offset(&state(&pl));
        // START OFFSET knob: FL (45, 286) -> editor client (41, 214) with the host's strip row.
        let mut p = POINT { x: 41, y: 214 };
        unsafe { ClientToScreen(editor, &mut p) };
        let input = |flags: u32, dx: i32, dy: i32| unsafe {
            let i = INPUT { r#type: INPUT_MOUSE, Anonymous: INPUT_0 { mi: MOUSEINPUT { dx, dy, mouseData: 0, dwFlags: flags, time: 0, dwExtraInfo: 0 } } };
            SendInput(1, &i, std::mem::size_of::<INPUT>() as i32);
        };
        let cursor = || unsafe {
            let mut c = POINT { x: 0, y: 0 };
            GetCursorPos(&mut c);
            (c.x, c.y)
        };
        let shown = || unsafe {
            let mut ci: CURSORINFO = std::mem::zeroed();
            ci.cbSize = std::mem::size_of::<CURSORINFO>() as u32;
            GetCursorInfo(&mut ci);
            ci.flags & CURSOR_SHOWING != 0
        };
        unsafe { SetCursorPos(p.x, p.y) };
        pump(&mut pl, 50);
        input(MOUSEEVENTF_LEFTDOWN, 0, 0);
        pump(&mut pl, 50);
        let mut worst = 0;
        let t0 = Instant::now();
        for _ in 0..40 {
            input(MOUSEEVENTF_MOVE, 0, -3);
            pump(&mut pl, 12);
            let (x, y) = cursor();
            worst = worst.max((x - p.x).abs() + (y - p.y).abs());
        }
        let took = t0.elapsed();
        let hidden = !shown();
        let during = offset(&state(&pl));
        input(MOUSEEVENTF_LEFTUP, 0, 0);
        pump(&mut pl, 80);
        let after = cursor();
        println!("offset {before} -> {during}, worst cursor drift {worst} px, cursor hidden while dragging: {hidden}, 40 moves in {took:?}");
        assert!(during > before + 0.01, "the knob did not follow the drag ({before} -> {during})");
        assert!(worst <= 3, "the cursor was not held in place (drift {worst} px)");
        assert!(hidden, "the cursor should be hidden while dragging a knob");
        assert!(shown(), "the cursor should show again after the drag");
        assert_eq!(after, (p.x, p.y), "the cursor should be back at the press point");
    }

    /// Real mouse input on the hosted editor must change what the plugin plays: drag the filter's MOD X knob down with
    /// the real cursor, then render a note through the same instance the engine uses.
    /// `OS_CLAP=.. cargo test -p os-app plugin_knob_edit_changes_the_sound -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn plugin_knob_edit_changes_the_sound() {
        use windows_sys::Win32::Foundation::POINT;
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::{SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MOVE, MOUSEINPUT};
        let Some(clap) = std::env::var_os("OS_CLAP") else { return };
        let lib = os_clap::Library::load(std::path::Path::new(&clap)).expect("load clap");
        let desc = lib.plugins[0].clone();
        let mut pl = Plugins { available: vec![Available { lib, desc }], scale: 1.0, ..Plugins::default() };
        pl.create(0, 0).unwrap();
        let info = ChannelInfo { name: "Open Sampler".into(), target: "Insert 1".into(), enabled: true, volume: 0.78, pan: 0.0, pitch_cents: 0, range: 2, track: 1, misc: None };
        pl.open_window(0, info).unwrap();
        pump(&mut pl, 300);
        // A bright 2 s saw so a low-pass is audible.
        let wav = std::env::temp_dir().join("os-app-knob-sound.wav");
        let tone: Vec<f32> = (0..88200).flat_map(|i| { let v = ((i as f32 / 44100.0 * 220.0).fract() * 2.0 - 1.0) * 0.4; [v, v] }).collect();
        os_engine::wav::write_f32_stereo(&mut std::fs::File::create(&wav).unwrap(), 44100, &tone).unwrap();
        // The persisted field is a JSON string holding JSON: forward slashes keep it free of escapes.
        let path = format!("\"\\\"{}\\\"\"", wav.to_string_lossy().replace('\\', "/"));
        let st = format!("{{\"version\":\"0.0.1\",\"params\":{{}},\"fields\":{{\"sample-path\":{path}}}}}");
        let mut blob = (st.len() as u64).to_le_bytes().to_vec();
        blob.extend_from_slice(st.as_bytes());
        assert!(pl.hosted[&0].plugin.lock().unwrap().load_state(&blob));
        pump(&mut pl, 300);
        let rate = pl.rate;
        let render = |pl: &mut Plugins| -> Vec<f32> {
            let plugin = pl.hosted[&0].plugin.clone();
            let mut p = plugin.lock().unwrap();
            let mut out = Vec::new();
            for b in 0..60 {
                if b == 2 {
                    p.note_on(0, 60, 0.78);
                }
                if b == 40 {
                    p.note_off(0, 60);
                }
                let (mut l, mut r) = (vec![0.0f32; 480], vec![0.0f32; 480]);
                p.process(rate, &mut l, &mut r);
                out.extend(l);
            }
            p.reset();
            out
        };
        // Let the plugin pick up its processed sample first.
        render(&mut pl);
        let before = render(&mut pl);
        assert!(before.iter().any(|v| v.abs() > 0.05), "the sample plays");
        pl.hosted.get_mut(&0).unwrap().link.as_mut().unwrap().set_page(1);
        pump(&mut pl, 100);
        let frame = pl.hosted[&0].window.as_ref().unwrap().hwnd();
        let editor = unsafe { GetWindow(GetWindow(frame as HWND, GW_CHILD), GW_CHILD) };
        unsafe { SetForegroundWindow(frame as HWND) };
        pump(&mut pl, 200);
        // Filter MOD X: FL (492.5, 168.5) -> editor client (488, 96) with the host's strip row.
        let mut p = POINT { x: 488, y: 96 };
        unsafe { ClientToScreen(editor, &mut p) };
        let input = |flags: u32, dx: i32, dy: i32| unsafe {
            let i = INPUT { r#type: INPUT_MOUSE, Anonymous: INPUT_0 { mi: MOUSEINPUT { dx, dy, mouseData: 0, dwFlags: flags, time: 0, dwExtraInfo: 0 } } };
            SendInput(1, &i, std::mem::size_of::<INPUT>() as i32);
        };
        unsafe { SetCursorPos(p.x, p.y) };
        pump(&mut pl, 50);
        input(MOUSEEVENTF_LEFTDOWN, 0, 0);
        pump(&mut pl, 50);
        for _ in 0..60 {
            input(MOUSEEVENTF_MOVE, 0, 4);
            pump(&mut pl, 10);
        }
        input(MOUSEEVENTF_LEFTUP, 0, 0);
        pump(&mut pl, 200);
        let state = String::from_utf8_lossy(&pl.hosted[&0].plugin.lock().unwrap().save_state().unwrap()).to_string();
        let i = state.find("\"f_cut\"").unwrap_or(0);
        println!("state: {}", &state[i..(i + 40).min(state.len())]);
        let after = render(&mut pl);
        let diff = before.iter().zip(&after).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
        let rms = |v: &[f32]| (v.iter().map(|x| x * x).sum::<f32>() / v.len() as f32).sqrt();
        println!("max diff {diff}, rms {} -> {}", rms(&before), rms(&after));
        assert!(diff > 0.05, "the MOD X drag did not change the sound");
    }

    /// The app's own path (needs an audio device): a plugin channel on the transport's live stream, a held note, a real
    /// mouse drag of MOD X in the editor; the output level must follow.
    /// `OS_CLAP=.. cargo test -p os-app app_hears_plugin_knob_edits -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn app_hears_plugin_knob_edits() {
        use windows_sys::Win32::Foundation::POINT;
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::{SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MOVE, MOUSEINPUT};
        let Some(clap) = std::env::var_os("OS_CLAP") else { return };
        let lib = os_clap::Library::load(std::path::Path::new(&clap)).expect("load clap");
        let desc = lib.plugins[0].clone();
        let mut pl = Plugins { available: vec![Available { lib, desc }], scale: 1.0, ..Plugins::default() };
        pl.create(0, 0).unwrap();
        let info = ChannelInfo { name: "Open Sampler".into(), target: "Insert 1".into(), enabled: true, volume: 0.78, pan: 0.0, pitch_cents: 0, range: 2, track: 1, misc: None };
        pl.open_window(0, info).unwrap();
        pump(&mut pl, 300);
        // A bright 2 s saw so a low-pass is audible.
        let wav = std::env::temp_dir().join("os-app-knob-sound.wav");
        let tone: Vec<f32> = (0..88200).flat_map(|i| { let v = ((i as f32 / 44100.0 * 220.0).fract() * 2.0 - 1.0) * 0.4; [v, v] }).collect();
        os_engine::wav::write_f32_stereo(&mut std::fs::File::create(&wav).unwrap(), 44100, &tone).unwrap();
        // The persisted field is a JSON string holding JSON: forward slashes keep it free of escapes.
        let path = format!("\"\\\"{}\\\"\"", wav.to_string_lossy().replace('\\', "/"));
        let st = format!("{{\"version\":\"0.0.1\",\"params\":{{}},\"fields\":{{\"sample-path\":{path}}}}}");
        let mut blob = (st.len() as u64).to_le_bytes().to_vec();
        blob.extend_from_slice(st.as_bytes());
        assert!(pl.hosted[&0].plugin.lock().unwrap().load_state(&blob));
        pump(&mut pl, 300);
        let mut project = crate::app::test_project();
        project.channels.clear();
        let row = crate::app::add_plugin_channel(&mut project, "Open Sampler");
        assert_eq!(row, 0);
        let transport = crate::transport::Transport::new(None);
        transport.update(&project);
        transport.set_instruments(pl.instruments());
        // Hold a note 400 ms, keep the UI side ticking, return the loudest output peak.
        let level = |pl: &mut Plugins| -> f32 {
            transport.take_peaks();
            transport.note_on(0, 60, 100);
            let mut peak = 0.0f32;
            let t0 = Instant::now();
            while t0.elapsed() < Duration::from_millis(400) {
                pump(pl, 20);
                let (l, r) = transport.take_peaks();
                peak = peak.max(l.max(r));
            }
            transport.note_off(0, 60);
            pump(pl, 300);
            peak
        };
        level(&mut pl);
        let before = level(&mut pl);
        println!("level before {before}");
        assert!(before > 0.05, "the note is heard");
        pl.hosted.get_mut(&0).unwrap().link.as_mut().unwrap().set_page(1);
        pump(&mut pl, 100);
        let frame = pl.hosted[&0].window.as_ref().unwrap().hwnd();
        let editor = unsafe { GetWindow(GetWindow(frame as HWND, GW_CHILD), GW_CHILD) };
        unsafe { SetForegroundWindow(frame as HWND) };
        pump(&mut pl, 200);
        // Filter MOD X: FL (492.5, 168.5) -> editor client (488, 96) with the host's strip row.
        let mut p = POINT { x: 488, y: 96 };
        unsafe { ClientToScreen(editor, &mut p) };
        let input = |flags: u32, dx: i32, dy: i32| unsafe {
            let i = INPUT { r#type: INPUT_MOUSE, Anonymous: INPUT_0 { mi: MOUSEINPUT { dx, dy, mouseData: 0, dwFlags: flags, time: 0, dwExtraInfo: 0 } } };
            SendInput(1, &i, std::mem::size_of::<INPUT>() as i32);
        };
        unsafe { SetCursorPos(p.x, p.y) };
        pump(&mut pl, 50);
        input(MOUSEEVENTF_LEFTDOWN, 0, 0);
        pump(&mut pl, 50);
        for _ in 0..120 {
            input(MOUSEEVENTF_MOVE, 0, 4);
            pump(&mut pl, 10);
        }
        input(MOUSEEVENTF_LEFTUP, 0, 0);
        pump(&mut pl, 200);
        let state = String::from_utf8_lossy(&pl.hosted[&0].plugin.lock().unwrap().save_state().unwrap()).to_string();
        let i = state.find("\"f_cut\"").unwrap_or(0);
        println!("state: {}", &state[i..(i + 40).min(state.len())]);
        let after = level(&mut pl);
        println!("level after {after}");
        assert!(after < before * 0.5, "MOD X down did not change what the app plays ({before} -> {after})");
    }

    /// Keys typed while the plugin window has the focus must reach the app (the typing keyboard keeps playing).
    #[test]
    #[ignore]
    fn keys_in_a_plugin_window_reach_the_app() {
        let Some(clap) = std::env::var_os("OS_CLAP") else { return };
        let lib = os_clap::Library::load(std::path::Path::new(&clap)).expect("load clap");
        let desc = lib.plugins[0].clone();
        let mut pl = Plugins { available: vec![Available { lib, desc }], scale: 1.0, ..Plugins::default() };
        pl.create(0, 0).unwrap();
        let info = ChannelInfo { name: "Open Sampler".into(), target: "Insert 1".into(), enabled: true, volume: 0.78, pan: 0.0, pitch_cents: 0, range: 2, track: 1, misc: None };
        pl.open_window(0, info).unwrap();
        pump(&mut pl, 300);
        let frame = pl.hosted[&0].window.as_ref().unwrap().hwnd();
        let editor = unsafe { GetWindow(GetWindow(frame as HWND, GW_CHILD), GW_CHILD) };
        unsafe {
            SetForegroundWindow(frame as HWND);
            // A click inside the editor gives it the focus, as for a user.
            SendMessageW(editor, WM_LBUTTONDOWN, 1, (300 | (60 << 16)) as isize);
            SendMessageW(editor, WM_LBUTTONUP, 0, (300 | (60 << 16)) as isize);
        }
        eprintln!("focus {:?} editor {:?} frame {:?} foreground {:?}", unsafe { windows_sys::Win32::UI::Input::KeyboardAndMouse::GetFocus() }, editor, frame, unsafe { GetForegroundWindow() });
        crate::plugin_window::take_keys();
        let q = 0x10isize; // scancode of Q
        // Real key input (the hook sees what the system queue delivers; posted messages bypass it), only while our
        // frame is in front so the key cannot land in another program.
        assert_eq!(unsafe { GetForegroundWindow() }, frame as HWND, "the plugin window must be in front");
        unsafe {
            use windows_sys::Win32::UI::Input::KeyboardAndMouse::{keybd_event, KEYEVENTF_KEYUP};
            keybd_event(0x51, q as u8, 0, 0);
            keybd_event(0x51, q as u8, KEYEVENTF_KEYUP, 0);
        }
        pump(&mut pl, 100);
        let keys = crate::plugin_window::take_keys();
        assert_eq!(keys.len(), 2, "{keys:?}");
        assert!(keys[0].pressed && !keys[1].pressed && keys[0].scancode == 0x10);
    }

    fn fixture_mp3() -> String {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../os-engine/tests/data/click_sine.mp3");
        p.canonicalize().unwrap().to_string_lossy().trim_start_matches(r"\\?\").to_string()
    }

    /// Main swing delays an off-beat step played by a hosted Open Sampler (spec: flp/events/project-info.md "Swing timing").
    #[test]
    fn swing_reaches_hosted_sampler() {
        let wav = std::env::temp_dir().join(format!("os-swing-click-{}.wav", std::process::id()));
        let click: Vec<f32> = (0..48000).flat_map(|i| { let v = if i < 200 { 0.8 } else { 0.0 }; [v, v] }).collect();
        os_engine::wav::write_f32_stereo(&mut std::fs::File::create(&wav).unwrap(), 48000, &click).unwrap();
        let onsets = |swing: u8| -> Vec<usize> {
            let mut pl = Plugins { available: builtin(), scale: 1.0, rate: 48000, ..Plugins::default() };
            let index = pl.index_of(OPEN_SAMPLER).unwrap();
            let mut p = crate::app::test_project();
            p.channels.truncate(1);
            p.patterns = vec![os_model::Pattern { number: 1, notes: vec![os_model::Note::step(0, 0), os_model::Note::step(0, 24)], ..Default::default() }];
            p.swing = Some(swing);
            pl.create_with(0, index, Some(&sampler_state(&p.channels[0]))).unwrap();
            assert!(pl.load_sample(0, &wav.to_string_lossy()));
            let inst = pl.hosted[&0].plugin.clone() as SharedInstrument;
            let mut lib = os_engine::SampleLibrary::default();
            let mut e = os_engine::Engine::new(&p, &mut lib, 48000, os_engine::PlayMode::Pattern(1));
            e.set_instrument(0, inst);
            let mut out = Vec::new();
            let mut buf = vec![0.0f32; 480 * 2];
            for _ in 0..40 {
                e.process(&mut buf);
                out.extend(buf.iter().step_by(2).copied());
            }
            let mut on = Vec::new();
            let mut quiet = 1000;
            for (i, v) in out.iter().enumerate() {
                if v.abs() > 0.05 {
                    if quiet > 500 {
                        on.push(i);
                    }
                    quiet = 0;
                } else {
                    quiet += 1;
                }
            }
            on
        };
        let straight = onsets(0);
        let swung = onsets(128);
        let _ = std::fs::remove_file(&wav);
        eprintln!("straight {straight:?} swung {swung:?}");
        assert_eq!(straight.len(), 2);
        assert_eq!(swung.len(), 2);
        // Half a step at 130 BPM, 48 kHz: 24 ticks = 5538 frames, so about 2769 frames later.
        let shift = swung[1] as i64 - straight[1] as i64;
        assert!((shift - 2769).abs() < 20, "{shift}");
    }

    /// Open Sampler is compiled in: no `.clap` file is needed, and a sample loads into a running instance without
    /// touching its other settings.
    #[test]
    fn builtin_open_sampler_takes_a_sample() {
        let mut pl = Plugins { available: builtin(), scale: 1.0, rate: 48000, ..Plugins::default() };
        let index = pl.index_of(OPEN_SAMPLER).expect("built-in Open Sampler");
        let mut ch = crate::app::test_project().channels.remove(0);
        ch.envelopes[1].enabled = true;
        ch.envelopes[1].attack = 65536 / 2;
        pl.create_with(0, index, Some(&sampler_state(&ch))).unwrap();
        assert_eq!(pl.sampler_path(0).as_deref(), Some(""));
        let mp3 = fixture_mp3();
        assert!(pl.load_sample(0, &mp3));
        assert_eq!(pl.sampler_path(0).as_deref(), Some(mp3.as_str()));
        let st = pl.hosted[&0].plugin.lock().unwrap().save_state().unwrap();
        let v: serde_json::Value = serde_json::from_slice(&st[8..]).unwrap();
        assert_eq!(v["params"]["envvol_on"], serde_json::json!({"bool": true}), "other settings kept");
        // And it sounds.
        let inst = pl.hosted[&0].plugin.clone() as SharedInstrument;
        let mut i = inst.lock().unwrap();
        i.note_on(0, 60, 0.8);
        let mut peak = 0.0f32;
        for _ in 0..50 {
            let (mut l, mut r) = (vec![0.0f32; 480], vec![0.0f32; 480]);
            i.process(48000, &mut l, &mut r);
            peak = l.iter().fold(peak, |m, v| m.max(v.abs()));
        }
        // The 0.5 s volume attack kept from the channel still fades the click (0.9 at 0.1 s) in.
        assert!(peak > 0.01 && peak < 0.3, "the mp3 plays through the kept envelope (peak {peak})");
    }

    /// The built-in Open Parametric EQ 2 starts from a project file's EQ state, plays it, and gives the state back
    /// (spec: plugins/open-parametric-eq-2.md §8).
    #[test]
    fn builtin_parametric_eq_takes_and_gives_the_file_state() {
        use open_parametric_eq_2::params::{default_state, field, plugin_state_from_fl, set_field};
        let mut pl = Plugins { effects: builtin_effects(), scale: 1.0, rate: 48000, ..Plugins::default() };
        let index = pl.effect_index(OPEN_EQ).expect("built-in EQ");
        // Band 4 (default frequency: the middle of the scale) at +12 dB.
        let mut fl = default_state();
        set_field(&mut fl, 4, 1200);
        pl.create_effect(3, 2, index, Some(&plugin_state_from_fl(&fl))).unwrap();
        let key = fx_key(3, 2);
        let back = pl.eq_file_state(key).expect("EQ state");
        assert_eq!(field(&back, 4), 1200);
        assert_eq!(back, fl, "the whole file state comes back");
        // A sine at the band's frequency comes out about 4 times louder.
        let hz = os_engine::effects::eq::freq_hz(os_engine::effects::eq::default_freq(3));
        let fx = pl.hosted[&key].plugin.clone() as SharedEffect;
        let mut f = fx.lock().unwrap();
        let mut peak = 0.0f32;
        for block in 0..40 {
            let mut l: Vec<f32> = (0..480).map(|i| (0.1 * (std::f64::consts::TAU * hz * f64::from(block * 480 + i) / 48000.0).sin()) as f32).collect();
            let mut r = l.clone();
            f.process_fx(48000, &mut l, &mut r);
            if block > 20 {
                peak = l.iter().fold(peak, |m, v| m.max(v.abs()));
            }
        }
        assert!((peak - 0.398).abs() < 0.03, "+12 dB at the band (peak {peak})");
        assert_eq!(slot_label(EQ_FILE_NAME), OPEN_EQ);
        assert_eq!(new_slot(OPEN_EQ).name, EQ_FILE_NAME);
    }

    /// Playlist clips through Open Sampler (spec: plugins/open-sampler.md §7): a note can start partway into the sample
    /// (per-note modulation of the start offset) and a choke ends it at once.
    #[test]
    fn open_sampler_starts_late_and_chokes() {
        let mut pl = Plugins { available: builtin(), scale: 1.0, rate: 48000, ..Plugins::default() };
        let index = pl.index_of(OPEN_SAMPLER).unwrap();
        pl.create_with(0, index, None).unwrap();
        // One second: silent first half, 0.5 in the second.
        let wav = std::env::temp_dir().join("os-app-start-late.wav");
        let frames: Vec<f32> = (0..48000).flat_map(|i| if i < 24000 { [0.0; 2] } else { [0.5; 2] }).collect();
        os_engine::wav::write_f32_stereo(&mut std::fs::File::create(&wav).unwrap(), 48000, &frames).unwrap();
        assert!(pl.load_sample(0, &wav.to_string_lossy()));
        let inst = pl.hosted[&0].plugin.clone() as SharedInstrument;
        let mut i = inst.lock().unwrap();
        let mut block = |i: &mut dyn os_engine::Instrument| {
            let (mut l, mut r) = (vec![0.0f32; 480], vec![0.0f32; 480]);
            i.process(48000, &mut l, &mut r);
            l.iter().fold(0.0f32, |m, v| m.max(v.abs()))
        };
        // From the top: the first 100 ms are silent.
        i.note_on(0, 60, 0.78);
        assert!((0..10).all(|_| block(&mut *i) < 1e-4), "plays from the top");
        i.choke(0, 60);
        for _ in 0..10 {
            block(&mut *i);
        }
        // Started halfway: sound at once (after the declick fade-in).
        i.note_on_from(0, 60, 0.78, 0.5);
        let first: Vec<f32> = (0..5).map(|_| block(&mut *i)).collect();
        assert!(first.iter().any(|&p| p > 0.1), "starts in the loud half: {first:?}");
        // A choke silences it within the declick out time.
        i.choke(0, 60);
        let after: Vec<f32> = (0..10).map(|_| block(&mut *i)).collect();
        assert!(after[3..].iter().all(|&p| p < 1e-4), "choked: {after:?}");
    }

    /// A file dropped on an open Open Sampler window (what a browser drag released over it does) reaches its editor
    /// the way an Explorer drop does: `OS_CLAP` not needed (`cargo test -p os-app drop_on -- --ignored`).
    #[test]
    #[ignore]
    fn drop_on_plugin_window_loads_the_sample() {
        use windows_sys::Win32::Foundation::RECT;
        let mut pl = Plugins { available: builtin(), scale: 1.0, rate: 48000, ..Plugins::default() };
        pl.create(0, pl.index_of(OPEN_SAMPLER).unwrap()).unwrap();
        let info = ChannelInfo { name: "Open Sampler".into(), target: "Insert 1".into(), enabled: true, volume: 0.78, pan: 0.0, pitch_cents: 0, range: 2, track: 1, misc: None };
        pl.open_window(0, info).unwrap();
        pump(&mut pl, 300);
        let frame = pl.hosted[&0].window.as_ref().unwrap().hwnd();
        let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        unsafe {
            SetForegroundWindow(frame as HWND);
            GetWindowRect(frame as HWND, &mut r);
        }
        pump(&mut pl, 100);
        let (x, y) = ((r.left + r.right) / 2, (r.top + r.bottom) / 2);
        assert_eq!(pl.row_at_screen(x, y), Some(0), "the window is found under the point");
        let mp3 = fixture_mp3();
        assert!(post_drop(x, y, &mp3, frame), "the editor accepts files");
        pump(&mut pl, 300);
        assert_eq!(pl.sampler_path(0).as_deref(), Some(mp3.as_str()));
        // The strip (no editor under the point): loaded through the state instead.
        let wav = std::env::temp_dir().join("os-app-drop.wav");
        os_engine::wav::write_f32_stereo(&mut std::fs::File::create(&wav).unwrap(), 44100, &[0.5; 2000]).unwrap();
        let wav = wav.to_string_lossy().into_owned();
        assert!(pl.drop_file(0, r.left + 12, r.top + 40, &wav, true));
        pump(&mut pl, 200);
        assert_eq!(pl.sampler_path(0).as_deref(), Some(wav.as_str()));
    }

    /// `<target>/bundled/<file>` of this test's build folder (`cargo run -p bundle-plugins`), if built.
    fn bundled(file: &str) -> Option<PathBuf> {
        let target = std::env::current_exe().ok()?.parent()?.parent()?.parent()?.to_path_buf();
        Some(target.join("bundled").join(file)).filter(|p| p.exists())
    }

    /// A VST3 instrument hosts like a CLAP one (spec: plugins/vst3-hosting.md): created, given state bytes, played;
    /// the same plugin in both formats gets format labels in the picker and each label finds its own entry.
    #[test]
    fn vst3_plugins_host_like_clap_ones() {
        let (Some(clap), Some(vst3)) = (bundled("Open Sampler.clap"), bundled("Open Sampler.vst3")) else { return };
        let available = |p: &std::path::Path, load: fn(&std::path::Path) -> Result<Arc<dyn Library>, String>| {
            let lib = load(p).unwrap();
            Available { desc: lib.plugins()[0].clone(), lib }
        };
        let mut pl = Plugins {
            available: vec![
                available(&clap, |p| os_clap::Library::load(p).map(|l| l as Arc<dyn Library>)),
                available(&vst3, |p| os_vst3::Module::load(p).map(|l| l as Arc<dyn Library>)),
            ],
            scale: 1.0,
            rate: 48000,
            ..Plugins::default()
        };
        assert_eq!(pl.names(), ["Open Sampler (CLAP)", "Open Sampler (VST3)"]);
        assert_eq!(pl.index_of("Open Sampler (VST3)"), Some(1));
        assert_eq!(pl.index_of("Open Sampler"), Some(0), "a plain name still finds the first");
        pl.create(0, 1).unwrap();
        let d = pl.descriptor(0).unwrap();
        assert_eq!(d.format, os_host::Format::Vst3);
        assert!(d.uid().starts_with("vst3:") && d.uid().len() == 5 + 32);
        let wav = std::env::temp_dir().join("os-app-vst3.wav");
        os_engine::wav::write_f32_stereo(&mut std::fs::File::create(&wav).unwrap(), 48000, &[0.5; 9600]).unwrap();
        // A VST3 instance takes a bare component state too (here Open Sampler's JSON with the sample).
        let json = format!("{{\"version\":\"0.0.1\",\"params\":{{}},\"fields\":{{\"sample-path\":{}}}}}", json_string(&json_string(&wav.to_string_lossy())));
        assert!(pl.set_state(0, json.as_bytes()), "the sample reaches the VST3 instance");
        let state = pl.state(0).unwrap();
        assert!(state.starts_with(b"VST3"), "state bytes are a .vstpreset");
        assert!(pl.set_state(0, &state));
        let (_, inst) = pl.instruments().pop().unwrap();
        let mut i = inst.lock().unwrap();
        let (mut l, mut r) = (vec![0.0f32; 512], vec![0.0f32; 512]);
        i.note_on(0, 60, 0.8);
        i.process(48000, &mut l, &mut r);
        assert!(l.iter().any(|v| v.abs() > 0.1), "plays");
    }

    /// A VST3 instrument channel and a VST3 effect in a mixer slot are saved as wrapped plugins naming the class id, and
    /// the reopened project hosts the same classes with the saved state (spec: plugins/vst3-hosting.md §8,
    /// flp/events/plugins.md §4).
    #[test]
    fn vst3_plugins_save_and_reopen() {
        let (Some(smp), Some(eq)) = (bundled("Open Sampler.vst3"), bundled("Open Parametric EQ 2.vst3")) else { return };
        let load = |p: &std::path::Path| {
            let lib = os_vst3::Module::load(p).map(|l| l as Arc<dyn Library>).unwrap();
            Available { desc: lib.plugins()[0].clone(), lib }
        };
        let path = std::env::temp_dir().join(format!("openstudio-vst3-{}.flp", std::process::id()));
        let wav = std::env::temp_dir().join("os-app-vst3-save.wav");
        os_engine::wav::write_f32_stereo(&mut std::fs::File::create(&wav).unwrap(), 48000, &[0.25; 4800]).unwrap();
        let mut a = crate::app::App::new(None);
        a.state.project = crate::app::test_project();
        a.state.file = None;
        a.state.path = None;
        a.plugins.available = vec![load(&smp)];
        a.plugins.effects = vec![load(&eq)];
        let row = crate::app::add_plugin_channel(&mut a.state.project, "Open Sampler");
        a.plugins.create(row, 0).unwrap();
        a.state.hosted_rows.push(row);
        let json = format!("{{\"version\":\"0.0.1\",\"params\":{{}},\"fields\":{{\"sample-path\":{}}}}}", json_string(&json_string(&wav.to_string_lossy())));
        assert!(a.plugins.set_state(row, json.as_bytes()));
        a.state.project.mixer[3].slots[2] = Some(os_model::SlotPlugin { name: "Open Parametric EQ 2".into(), state: Vec::new() });
        a.plugins.create_effect(3, 2, 0, None).unwrap();
        // An edit of the effect, through the hosting layer, flushed into its processor.
        {
            let mut fx = a.plugins.hosted[&fx_key(3, 2)].plugin.lock().unwrap();
            let id = (0..fx.param_count()).filter_map(|i| fx.param_info(i)).find(|p| !p.read_only && !p.hidden && p.automatable).unwrap().id;
            assert!(fx.set_param(id, 0.3));
            assert!(fx.activate(48000));
            fx.idle();
            std::thread::sleep(Duration::from_millis(60));
            fx.idle();
        }
        let (ch_state, fx_state) = (a.plugins.state(row).unwrap(), a.plugins.state(fx_key(3, 2)).unwrap());
        assert!(ch_state.starts_with(b"VST3") && fx_state.starts_with(b"VST3"));
        a.save_to(path.clone());
        assert!(a.state.message.as_deref().is_some_and(|m| m.starts_with("Saved")), "{:?}", a.state.message);

        // The file names each class by its id.
        let saved = os_flp::project::load(&os_flp::FlpFile::parse(&std::fs::read(&path).unwrap()).unwrap()).unwrap();
        let r = crate::project_plugins::wrapped_channel(&saved.channels[row]).unwrap();
        assert_eq!((r.format.as_str(), r.id.as_str(), r.name.as_str()), ("vst3", a.plugins.available[0].desc.id.as_str(), "Open Sampler"));
        assert_eq!(r.state, ch_state);
        let s = crate::project_plugins::wrapped_slot(saved.mixer[3].slots[2].as_ref().unwrap()).unwrap();
        assert_eq!((s.format.as_str(), s.id.as_str()), ("vst3", a.plugins.effects[0].desc.id.as_str()));
        assert_eq!(s.state, fx_state);

        // Reopened: found by class id even when the plugin file is named differently now.
        let mut b = crate::app::App::new(None);
        let (mut i, mut e) = (load(&smp), load(&eq));
        i.desc.path = "D:/moved/Renamed.vst3".into();
        e.desc.path = "D:/moved/Other.vst3".into();
        b.plugins.available = vec![i];
        b.plugins.effects = vec![e];
        b.open(path.clone());
        b.sync_wrapped();
        let _ = std::fs::remove_file(&path);
        assert!(b.state.hosted_rows.contains(&row), "{:?}", b.state.message);
        assert_eq!(b.plugins.descriptor(row).unwrap().format, os_host::Format::Vst3);
        assert_eq!(b.plugins.state(row).as_deref(), Some(&ch_state[..]));
        assert_eq!(b.plugins.state(fx_key(3, 2)).as_deref(), Some(&fx_state[..]));
        assert_eq!(crate::mixer::slot_name(&b.state, 3, 2).as_deref(), Some("Open Parametric EQ 2"));
    }

    /// VST3 editors inside OpenStudio's plugin frame, moved off every screen and captured with PrintWindow:
    /// `OS_SHOT_DIR=<dir> cargo test -p os-app --bin openstudio vst3_window_shots -- --ignored`.
    #[test]
    #[ignore]
    fn vst3_window_shots() {
        let Some(dir) = std::env::var_os("OS_SHOT_DIR") else { return };
        let dir = std::path::PathBuf::from(dir);
        for (file, effect) in [("Open Sampler.vst3", false), ("Open Parametric EQ 2.vst3", true)] {
            let Some(path) = bundled(file) else { continue };
            let lib = os_vst3::Module::load(&path).unwrap();
            let a = Available { desc: lib.plugins()[0].clone(), lib };
            let name = a.desc.name.clone();
            let mut pl = Plugins { scale: 1.0, rate: 48000, ..Plugins::default() };
            let key = if effect {
                pl.effects = vec![a];
                pl.create_effect(1, 0, 0, None).unwrap();
                fx_key(1, 0)
            } else {
                pl.available = vec![a];
                pl.create(0, 0).unwrap();
                0
            };
            let info = ChannelInfo { name: name.clone(), target: "Insert 1".into(), enabled: true, volume: 0.78, pan: 0.0, pitch_cents: 0, range: 2, track: 1, misc: None };
            pl.open_window(key, info).unwrap();
            let frame = pl.hosted[&key].window.as_ref().unwrap().hwnd();
            unsafe { SetWindowPos(frame as HWND, std::ptr::null_mut(), -20000, -20000, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE) };
            pump(&mut pl, 400);
            let editor = unsafe { GetWindow(GetWindow(frame as HWND, GW_CHILD), GW_CHILD) };
            assert!(!editor.is_null(), "{name}: the editor is a child of the frame");
            let out = dir.join(format!("vst3_{}.bmp", name.replace(' ', "_")));
            // The first capture of a window that never reached the screen makes it paint.
            capture(frame, &out);
            pump(&mut pl, 200);
            capture(frame, &out);
        }
    }

    /// The user's installed FL native effects show their own editors inside OpenStudio's plugin frame (spec
    /// plugins/fruity-abi.md, "Editors"): frame moved off every screen, captured with PrintWindow, closed and opened
    /// again. Loads FL DLLs in-process, so ignored: `OS_SHOT_DIR=<dir> cargo test -p os-app --bin openstudio
    /// native_window_shots -- --ignored` (shots only when OS_SHOT_DIR is set). Skips without FL.
    #[test]
    #[ignore]
    fn native_window_shots() {
        let fl = os_native::default_fl_dir();
        let dir = std::env::var_os("OS_SHOT_DIR").map(std::path::PathBuf::from);
        for name in ["Fruity Balance", "Fruity Parametric EQ 2", "Fruity Delay 3", "Fruity Limiter"] {
            let Some(dll) = os_native::find_plugin(&fl, name) else { continue };
            let lib = os_native::NativeLibrary::load(&dll).unwrap() as Arc<dyn Library>;
            let a = Available { desc: lib.plugins()[0].clone(), lib };
            let mut pl = Plugins { scale: 1.0, rate: 48000, effects: vec![a], ..Plugins::default() };
            pl.create_effect(1, 0, 0, None).unwrap();
            let key = fx_key(1, 0);
            let info = ChannelInfo { name: name.into(), target: "Insert 1".into(), enabled: true, volume: 0.78, pan: 0.0, pitch_cents: 0, range: 2, track: 1, misc: None };
            for round in 0..2 {
                pl.open_window(key, info.clone()).unwrap();
                let frame = pl.hosted[&key].window.as_ref().unwrap().hwnd();
                unsafe { SetWindowPos(frame as HWND, std::ptr::null_mut(), -20000, -20000, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE) };
                pump(&mut pl, 300);
                let area = pl.hosted[&key].window.as_ref().unwrap().area() as HWND;
                let editor = unsafe { GetWindow(area, GW_CHILD) };
                assert!(!editor.is_null(), "{name}: the editor is a child of the frame's editor area");
                let (mut ra, mut re): (RECT, RECT) = unsafe { (std::mem::zeroed(), std::mem::zeroed()) };
                unsafe { GetWindowRect(area, &mut ra) };
                unsafe { GetWindowRect(editor, &mut re) };
                assert_eq!((ra.right - ra.left, ra.bottom - ra.top), (re.right - re.left, re.bottom - re.top), "{name}: the frame fits the editor");
                assert_eq!((ra.left, ra.top), (re.left, re.top), "{name}: the editor sits at the area's corner");
                if let Some(dir) = &dir {
                    let out = dir.join(format!("native_{}_{round}.bmp", name.replace(' ', "_")));
                    capture(frame, &out);
                    pump(&mut pl, 200);
                    capture(frame, &out);
                }
                assert!(pl.close_if_open(key));
                pump(&mut pl, 100);
            }
        }
    }

    /// With the experimental native host opted in, the scan offers the user's installed FL native
    /// effects in the effect picker (spec plugins/fruity-abi.md). Skips when FL is not installed.
    /// Ignored in the default run: it mutates the process `OPENSTUDIO_NATIVE` env var, which is
    /// unsound to do while other parallel tests read the environment. Run with
    /// `cargo test -p os-app native_effects_listed_when_opted_in -- --ignored`.
    #[test]
    #[ignore]
    fn native_effects_listed_when_opted_in() {
        let fl = os_native::default_fl_dir();
        if !fl.join("Plugins").join("Fruity").join("Effects").is_dir() {
            return;
        }
        // SAFETY: test-only; `OPENSTUDIO_NATIVE` is read only by this scan path.
        unsafe { std::env::set_var("OPENSTUDIO_NATIVE", "1") };
        let opted = Plugins::scan();
        unsafe { std::env::remove_var("OPENSTUDIO_NATIVE") };
        let baseline = Plugins::scan();

        let native: Vec<_> = opted.effects.iter().filter(|a| a.desc.format == os_host::Format::Native).collect();
        assert!(!native.is_empty(), "native effects should be listed when opted in");
        assert!(native.iter().any(|a| a.desc.name == "Fruity Balance"));
        assert!(
            baseline.effects.iter().all(|a| a.desc.format != os_host::Format::Native),
            "native effects must not appear without the opt-in"
        );
    }
}
