//! Hosted generator plugins (CLAP): scanning, one instance per plugin channel, their windows, and the
//! instrument list the transport plays.
//!
// spec: plugins/plugin-formats.md (hosting, where to find plugins, Open Sampler state), ui/plugin-window.md,
// plugins/host-link.md (Open plugins join the window strip)

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use os_clap::{Descriptor, Library, Plugin, SharedPlugin};
use os_engine::{SharedEffect, SharedInstrument};

#[cfg(windows)]
use crate::plugin_window::{ChannelInfo, FrameEvent, FrameWindow};

/// A plugin offered in the picker.
#[derive(Clone)]
pub struct Available {
    pub lib: Arc<Library>,
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
    /// The window was clicked (or docked / detached): it goes to the front of the app's window stack.
    Raise(usize),
    /// The menu arrow was clicked: open the window menu at this screen point.
    Menu(usize, i32, i32),
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
    rate: u32,
}

/// Folders searched for `.clap` files: the standard CLAP folders, then `bundled/` next to the build folders of this
/// workspace (where `cargo run -p bundle-plugins` puts OpenStudio's own plugins).
fn search_dirs() -> Vec<PathBuf> {
    let mut dirs = os_clap::standard_dirs();
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
    static LIB: OnceLock<Option<Arc<Library>>> = OnceLock::new();
    let lib = LIB.get_or_init(|| {
        let path = std::env::current_exe().unwrap_or_default();
        let entry = &open_sampler::clap_entry as *const _ as *const std::ffi::c_void;
        // SAFETY: `clap_entry` is the linked plugin's static CLAP entry table.
        match unsafe { Library::from_entry(entry, &path) } {
            Ok(l) => Some(l),
            Err(e) => {
                eprintln!("built-in Open Sampler: {e}");
                None
            }
        }
    });
    lib.iter().flat_map(|l| l.plugins.iter().map(|d| Available { lib: l.clone(), desc: d.clone() })).collect()
}

impl Plugins {
    /// The built-in Open Sampler, then the plugin folders' instruments (first file wins for a plugin id). Open Sampler
    /// `.clap` files are skipped: a second copy in the process would share the built-in one's editor window class.
    pub fn scan() -> Plugins {
        let mut available: Vec<Available> = builtin();
        let mut effects: Vec<Available> = Vec::new();
        let files = os_clap::find_files(&search_dirs());
        for f in files.into_iter().filter(|f| f.file_stem().is_none_or(|s| s != OPEN_SAMPLER)) {
            match Library::load(&f) {
                Ok(lib) => {
                    for d in lib.plugins.iter() {
                        let list = if d.is_instrument() {
                            &mut available
                        } else if d.is_effect() {
                            &mut effects
                        } else {
                            continue;
                        };
                        if !list.iter().any(|a| a.desc.id == d.id) {
                            list.push(Available { lib: lib.clone(), desc: d.clone() });
                        }
                    }
                }
                Err(e) => eprintln!("plugin scan: {e}"),
            }
        }
        effects.sort_by_key(|a| a.desc.name.to_lowercase());
        Plugins { available, effects, scale: 1.0, ..Plugins::default() }
    }

    pub fn names(&self) -> Vec<String> {
        self.available.iter().map(|a| a.desc.name.clone()).collect()
    }

    /// Creates an instance of available plugin `index` for rack row `row` and activates it at the device rate.
    pub fn create(&mut self, row: usize, index: usize) -> Result<String, String> {
        self.create_with(row, index, None)
    }

    /// `create` with a starting state (the plugin's own saved state format).
    pub fn create_with(&mut self, row: usize, index: usize, state: Option<&[u8]>) -> Result<String, String> {
        let a = self.available.get(index).ok_or("no such plugin")?.clone();
        let mut p = Plugin::new(&a.lib, &a.desc.id)?;
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
        self.hosted.insert(row, Hosted { plugin: Arc::new(Mutex::new(p)), #[cfg(windows)] window: None, #[cfg(windows)] link: None });
        Ok(a.desc.name)
    }

    /// Creates effect `index` (of `effects`) in mixer track `track`'s slot `slot`, replacing what was there.
    pub fn create_effect(&mut self, track: u16, slot: u8, index: usize, state: Option<&[u8]>) -> Result<String, String> {
        let a = self.effects.get(index).ok_or("no such effect")?.clone();
        let key = fx_key(track, slot);
        self.drop_key(key);
        let mut p = Plugin::new(&a.lib, &a.desc.id)?;
        if let Some(st) = state {
            p.load_state(st);
        }
        if self.rate == 0 {
            self.rate = crate::transport::Transport::device_rate().unwrap_or(48000);
        }
        if !p.activate(self.rate) {
            return Err(format!("{} could not start", a.desc.name));
        }
        self.hosted.insert(key, Hosted { plugin: Arc::new(Mutex::new(p)), #[cfg(windows)] window: None, #[cfg(windows)] link: None });
        Ok(a.desc.name)
    }

    /// Removes the instance under `key` (closing its window first).
    pub fn drop_key(&mut self, key: usize) {
        #[cfg(windows)]
        self.close_window(key);
        self.hosted.remove(&key);
    }

    /// Moves mixer slot instances: `m(track, slot)` gives the new place, `None` drops the instance.
    pub fn remap_fx(&mut self, m: &dyn Fn(u16, u8) -> Option<(u16, u8)>) {
        let keys: Vec<usize> = self.hosted.keys().copied().filter(|&k| k >= FX_KEY).collect();
        let mut moved = Vec::new();
        for k in keys {
            let (t, s) = fx_of(k).unwrap();
            match m(t, s) {
                Some((nt, ns)) if (nt, ns) == (t, s) => {}
                Some((nt, ns)) => moved.push((fx_key(nt, ns), self.hosted.remove(&k).unwrap())),
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
        self.available.iter().position(|a| a.desc.name == name)
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
            if let crate::plugin_window::LinkMsg::Note(k, v, on) = m {
                let row = self.hosted.iter().find(|(_, h)| h.window.as_ref().is_some_and(|w| w.area() as isize == area)).map(|(&r, _)| r);
                if let Some(row) = row {
                    out.push(Request::Note(row, k, v, on));
                }
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
fn with_sample_path(blob: &[u8], path: &str) -> Option<Vec<u8>> {
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
        let lib = Library::load(std::path::Path::new(&clap)).expect("load clap");
        let desc = lib.plugins[0].clone();
        let mut pl = Plugins { available: vec![Available { lib, desc }], scale: 1.0, ..Plugins::default() };
        pl.create(0, 0).unwrap();
        let info = ChannelInfo { name: "Open Sampler".into(), target: "Insert 1".into(), enabled: true, volume: 0.78, pan: 0.0, pitch_cents: 0, range: 2, track: 1 };
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

    /// Random clicks, drags and wheel turns on the editor; a probe toggle must keep changing the plugin state.
    /// `OS_SHOT_DIR=.. OS_CLAP=.. cargo test -p os-app plugin_editor_stress -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn plugin_editor_stress() {
        let Some(clap) = std::env::var_os("OS_CLAP") else { return };
        // Right clicks and list buttons open modal menus; the plugin skips them under this switch.
        unsafe { std::env::set_var("OPENSAMPLER_NO_MODAL", "1") };
        let lib = Library::load(std::path::Path::new(&clap)).expect("load clap");
        let desc = lib.plugins[0].clone();
        let mut pl = Plugins { available: vec![Available { lib, desc }], scale: 1.0, ..Plugins::default() };
        pl.create(0, 0).unwrap();
        let info = ChannelInfo { name: "Open Sampler".into(), target: "Insert 1".into(), enabled: true, volume: 0.78, pan: 0.0, pitch_cents: 0, range: 2, track: 1 };
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
        let lib = Library::load(std::path::Path::new(&clap)).expect("load clap");
        let desc = lib.plugins[0].clone();
        let mut pl = Plugins { available: vec![Available { lib, desc }], scale: 1.0, ..Plugins::default() };
        pl.create(0, 0).unwrap();
        let info = ChannelInfo { name: "Open Sampler".into(), target: "Insert 1".into(), enabled: true, volume: 0.78, pan: 0.0, pitch_cents: 0, range: 2, track: 1 };
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
        use os_engine::Instrument;
        use windows_sys::Win32::Foundation::POINT;
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::{SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MOVE, MOUSEINPUT};
        let Some(clap) = std::env::var_os("OS_CLAP") else { return };
        let lib = Library::load(std::path::Path::new(&clap)).expect("load clap");
        let desc = lib.plugins[0].clone();
        let mut pl = Plugins { available: vec![Available { lib, desc }], scale: 1.0, ..Plugins::default() };
        pl.create(0, 0).unwrap();
        let info = ChannelInfo { name: "Open Sampler".into(), target: "Insert 1".into(), enabled: true, volume: 0.78, pan: 0.0, pitch_cents: 0, range: 2, track: 1 };
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
        let lib = Library::load(std::path::Path::new(&clap)).expect("load clap");
        let desc = lib.plugins[0].clone();
        let mut pl = Plugins { available: vec![Available { lib, desc }], scale: 1.0, ..Plugins::default() };
        pl.create(0, 0).unwrap();
        let info = ChannelInfo { name: "Open Sampler".into(), target: "Insert 1".into(), enabled: true, volume: 0.78, pan: 0.0, pitch_cents: 0, range: 2, track: 1 };
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
        let lib = Library::load(std::path::Path::new(&clap)).expect("load clap");
        let desc = lib.plugins[0].clone();
        let mut pl = Plugins { available: vec![Available { lib, desc }], scale: 1.0, ..Plugins::default() };
        pl.create(0, 0).unwrap();
        let info = ChannelInfo { name: "Open Sampler".into(), target: "Insert 1".into(), enabled: true, volume: 0.78, pan: 0.0, pitch_cents: 0, range: 2, track: 1 };
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

    /// A file dropped on an open Open Sampler window (what a browser drag released over it does) reaches its editor
    /// the way an Explorer drop does: `OS_CLAP` not needed (`cargo test -p os-app drop_on -- --ignored`).
    #[test]
    #[ignore]
    fn drop_on_plugin_window_loads_the_sample() {
        use windows_sys::Win32::Foundation::RECT;
        let mut pl = Plugins { available: builtin(), scale: 1.0, rate: 48000, ..Plugins::default() };
        pl.create(0, pl.index_of(OPEN_SAMPLER).unwrap()).unwrap();
        let info = ChannelInfo { name: "Open Sampler".into(), target: "Insert 1".into(), enabled: true, volume: 0.78, pan: 0.0, pitch_cents: 0, range: 2, track: 1 };
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
}
