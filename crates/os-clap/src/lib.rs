//! OpenStudio's CLAP host: loads `.clap` libraries, creates instances, plays them as engine instruments
//! (`os_engine::Instrument`), saves / loads their state and shows their editors.
//!
//! Threading: an instance is shared as `Arc<Mutex<Plugin>>`. The UI thread creates it, activates it, opens its
//! editor, loads / saves state and calls `idle` every tick; the audio thread only calls `Instrument` methods through
//! the engine (with `try_lock`).
// spec: plugins/plugin-formats.md (hosting inside OpenStudio)

use std::ffi::{c_char, c_void, CStr, CString};
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use clap_sys::audio_buffer::clap_audio_buffer;
use clap_sys::entry::clap_plugin_entry;
use clap_sys::events::{
    clap_event_header, clap_event_note, clap_input_events, clap_output_events, CLAP_CORE_EVENT_SPACE_ID, CLAP_EVENT_NOTE_OFF,
    CLAP_EVENT_NOTE_ON,
};
use clap_sys::ext::gui::{clap_plugin_gui, clap_window, clap_window_handle, CLAP_EXT_GUI, CLAP_WINDOW_API_WIN32};
use clap_sys::ext::state::{clap_plugin_state, CLAP_EXT_STATE};
use clap_sys::factory::plugin_factory::{clap_plugin_factory, CLAP_PLUGIN_FACTORY_ID};
use clap_sys::host::clap_host;
use clap_sys::plugin::clap_plugin;
use clap_sys::process::clap_process;
use clap_sys::stream::{clap_istream, clap_ostream};
use clap_sys::version::{clap_version_is_compatible, CLAP_VERSION};

/// A plugin a library offers.
#[derive(Debug, Clone, PartialEq)]
pub struct Descriptor {
    pub id: String,
    pub name: String,
    pub vendor: String,
    pub features: Vec<String>,
}

impl Descriptor {
    pub fn is_instrument(&self) -> bool {
        self.features.iter().any(|f| f == "instrument")
    }
}

/// A loaded `.clap` file.
pub struct Library {
    entry: *const clap_plugin_entry,
    factory: *const clap_plugin_factory,
    pub path: PathBuf,
    pub plugins: Vec<Descriptor>,
    _lib: libloading::Library,
}

// SAFETY: the entry and factory are immutable tables; CLAP allows calling the factory from any thread.
unsafe impl Send for Library {}
unsafe impl Sync for Library {}

unsafe fn cstr(p: *const c_char) -> String {
    if p.is_null() { String::new() } else { unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned() }
}

impl Library {
    pub fn load(path: &Path) -> Result<Arc<Library>, String> {
        unsafe {
            let lib = libloading::Library::new(path).map_err(|e| format!("{}: {e}", path.display()))?;
            let entry: *const clap_plugin_entry = *lib.get::<*const clap_plugin_entry>(b"clap_entry\0").map_err(|e| format!("{}: no clap_entry ({e})", path.display()))?;
            if entry.is_null() || !clap_version_is_compatible((*entry).clap_version) {
                return Err(format!("{}: incompatible CLAP version", path.display()));
            }
            let p = CString::new(path.to_string_lossy().as_bytes()).unwrap_or_default();
            if !(*entry).init.is_some_and(|f| f(p.as_ptr())) {
                return Err(format!("{}: init failed", path.display()));
            }
            let factory = (*entry).get_factory.map_or(ptr::null(), |f| f(CLAP_PLUGIN_FACTORY_ID.as_ptr())) as *const clap_plugin_factory;
            if factory.is_null() {
                return Err(format!("{}: no plugin factory", path.display()));
            }
            let count = (*factory).get_plugin_count.map_or(0, |f| f(factory));
            let mut plugins = Vec::new();
            for i in 0..count {
                let Some(d) = (*factory).get_plugin_descriptor.map(|f| f(factory, i)).filter(|d| !d.is_null()) else { continue };
                let mut features = Vec::new();
                let mut fp = (*d).features;
                while !fp.is_null() && !(*fp).is_null() {
                    features.push(cstr(*fp));
                    fp = fp.add(1);
                }
                plugins.push(Descriptor { id: cstr((*d).id), name: cstr((*d).name), vendor: cstr((*d).vendor), features });
            }
            Ok(Arc::new(Library { entry, factory, path: path.to_owned(), plugins, _lib: lib }))
        }
    }
}

impl Drop for Library {
    fn drop(&mut self) {
        unsafe {
            if let Some(f) = (*self.entry).deinit {
                f();
            }
        }
    }
}

/// The standard CLAP folders (per user, then system) on Windows.
pub fn standard_dirs() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(l) = std::env::var_os("LOCALAPPDATA") {
        v.push(PathBuf::from(l).join("Programs").join("Common").join("CLAP"));
    }
    if let Some(c) = std::env::var_os("COMMONPROGRAMFILES") {
        v.push(PathBuf::from(c).join("CLAP"));
    }
    v
}

/// `.clap` files directly inside the given folders (and one level of subfolders).
pub fn find_files(dirs: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for d in dirs {
        let Ok(rd) = std::fs::read_dir(d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("clap")) && p.is_file() {
                out.push(p);
            } else if p.is_dir() {
                out.extend(find_files(&[p]));
            }
        }
    }
    out
}

/// Host-side state the plugin's callbacks reach through `clap_host::host_data`.
struct HostData {
    callback: AtomicBool,
    restart: AtomicBool,
}

static HOST_NAME: &CStr = c"OpenStudio";
static HOST_URL: &CStr = c"";
static HOST_VERSION: &CStr = c"0.0.1";

unsafe extern "C" fn host_get_extension(_host: *const clap_host, _id: *const c_char) -> *const c_void {
    ptr::null()
}
unsafe extern "C" fn host_request_restart(host: *const clap_host) {
    unsafe { (*((*host).host_data as *const HostData)).restart.store(true, Ordering::Release) };
}
unsafe extern "C" fn host_request_process(_host: *const clap_host) {}
unsafe extern "C" fn host_request_callback(host: *const clap_host) {
    unsafe { (*((*host).host_data as *const HostData)).callback.store(true, Ordering::Release) };
}

/// One queued note event.
#[derive(Clone, Copy)]
struct Ev {
    time: u32,
    on: bool,
    key: u8,
    velocity: f32,
}

/// A plugin instance.
pub struct Plugin {
    pub desc: Descriptor,
    plugin: *const clap_plugin,
    gui: *const clap_plugin_gui,
    state: *const clap_plugin_state,
    /// Kept alive for the plugin, which holds a pointer to it.
    _host: Box<clap_host>,
    host_data: Box<HostData>,
    /// Sample rate it is activated at (0 = not active).
    rate: u32,
    max_frames: u32,
    processing: bool,
    /// A rate the audio thread asked for (the UI reactivates at it on the next `idle`).
    wanted_rate: AtomicU32,
    queue: Vec<Ev>,
    notes: Vec<clap_event_note>,
    steady: i64,
    dbg_peak: f32,
    dbg_frames: u64,
    /// The editor is open (parented into a host window).
    editor: bool,
    _lib: Arc<Library>,
}

// SAFETY: CLAP instances may be used from the main thread and the audio thread; access is serialised by the
// surrounding mutex and the main-thread-only calls are only made by the UI thread.
unsafe impl Send for Plugin {}

pub type SharedPlugin = Arc<Mutex<Plugin>>;

const MAX_FRAMES: u32 = 4096;

fn debug() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("OS_DEBUG").is_some())
}

impl Plugin {
    /// Creates and initialises plugin `id` from `lib` (UI thread).
    pub fn new(lib: &Arc<Library>, id: &str) -> Result<Plugin, String> {
        let desc = lib.plugins.iter().find(|d| d.id == id).cloned().ok_or_else(|| format!("{}: no plugin {id}", lib.path.display()))?;
        let host_data = Box::new(HostData { callback: AtomicBool::new(false), restart: AtomicBool::new(false) });
        let host = Box::new(clap_host {
            clap_version: CLAP_VERSION,
            host_data: &*host_data as *const HostData as *mut c_void,
            name: HOST_NAME.as_ptr(),
            vendor: HOST_NAME.as_ptr(),
            url: HOST_URL.as_ptr(),
            version: HOST_VERSION.as_ptr(),
            get_extension: Some(host_get_extension),
            request_restart: Some(host_request_restart),
            request_process: Some(host_request_process),
            request_callback: Some(host_request_callback),
        });
        let cid = CString::new(id).map_err(|e| e.to_string())?;
        unsafe {
            let f = lib.factory;
            let plugin = (*f).create_plugin.map_or(ptr::null(), |c| c(f, &*host, cid.as_ptr()));
            if plugin.is_null() {
                return Err(format!("{}: could not create {id}", lib.path.display()));
            }
            if !(*plugin).init.is_some_and(|i| i(plugin)) {
                if let Some(d) = (*plugin).destroy {
                    d(plugin);
                }
                return Err(format!("{id}: init failed"));
            }
            let ext = |name: &CStr| (*plugin).get_extension.map_or(ptr::null(), |g| g(plugin, name.as_ptr()));
            let gui = ext(CLAP_EXT_GUI) as *const clap_plugin_gui;
            let state = ext(CLAP_EXT_STATE) as *const clap_plugin_state;
            Ok(Plugin {
                desc,
                plugin,
                gui,
                state,
                _host: host,
                host_data,
                rate: 0,
                max_frames: MAX_FRAMES,
                processing: false,
                wanted_rate: AtomicU32::new(0),
                queue: Vec::with_capacity(256),
                notes: Vec::with_capacity(256),
                steady: 0,
                dbg_peak: 0.0,
                dbg_frames: 0,
                editor: false,
                _lib: lib.clone(),
            })
        }
    }

    /// Activates at `rate` (UI thread); reactivates when the rate changes.
    pub fn activate(&mut self, rate: u32) -> bool {
        if self.rate == rate {
            return true;
        }
        self.deactivate();
        let ok = unsafe { (*self.plugin).activate.is_some_and(|a| a(self.plugin, f64::from(rate), 1, self.max_frames)) };
        if ok {
            self.rate = rate;
        }
        ok
    }

    fn deactivate(&mut self) {
        if self.rate == 0 {
            return;
        }
        unsafe {
            if self.processing {
                if let Some(s) = (*self.plugin).stop_processing {
                    s(self.plugin);
                }
                self.processing = false;
            }
            if let Some(d) = (*self.plugin).deactivate {
                d(self.plugin);
            }
        }
        self.rate = 0;
    }

    /// UI-thread housekeeping: callbacks the plugin asked for, restarts, rate changes, a closed editor window.
    pub fn idle(&mut self) {
        unsafe {
            if self.host_data.callback.swap(false, Ordering::AcqRel) {
                if let Some(f) = (*self.plugin).on_main_thread {
                    f(self.plugin);
                }
            }
        }
        let wanted = self.wanted_rate.swap(0, Ordering::AcqRel);
        let restart = self.host_data.restart.swap(false, Ordering::AcqRel);
        if wanted != 0 && wanted != self.rate {
            self.activate(wanted);
        } else if restart && self.rate != 0 {
            let r = self.rate;
            self.deactivate();
            self.activate(r);
        }
    }

    pub fn has_editor(&self) -> bool {
        !self.gui.is_null() && unsafe { (*self.gui).is_api_supported.is_some_and(|f| f(self.plugin, CLAP_WINDOW_API_WIN32.as_ptr(), false)) }
    }

    pub fn editor_open(&self) -> bool {
        self.editor
    }

    /// Embeds the plugin's editor in `parent` (a host window handle: HWND on Windows); returns its size in pixels.
    /// The host draws the window frame around it (spec: ui/plugin-window.md).
    pub fn open_editor(&mut self, parent: *mut c_void) -> Result<(u32, u32), String> {
        if !self.has_editor() {
            return Err(format!("{} has no editor", self.desc.name));
        }
        self.close_editor();
        unsafe {
            let g = &*self.gui;
            let api = CLAP_WINDOW_API_WIN32.as_ptr();
            if !g.create.is_some_and(|c| c(self.plugin, api, false)) {
                return Err("the plugin could not create its editor".into());
            }
            let (mut w, mut h) = (0u32, 0u32);
            g.get_size.map(|f| f(self.plugin, &mut w, &mut h));
            let cw = clap_window { api, specific: clap_window_handle { win32: parent } };
            if !g.set_parent.is_some_and(|f| f(self.plugin, &cw)) {
                g.destroy.map(|d| d(self.plugin));
                return Err("the plugin refused the editor window".into());
            }
            g.show.map(|f| f(self.plugin));
            self.editor = true;
            Ok((w, h))
        }
    }

    /// Editor size in pixels (when open).
    pub fn editor_size(&self) -> Option<(u32, u32)> {
        if !self.editor {
            return None;
        }
        let (mut w, mut h) = (0u32, 0u32);
        unsafe { (*self.gui).get_size.is_some_and(|f| f(self.plugin, &mut w, &mut h)) }.then_some((w, h))
    }

    pub fn close_editor(&mut self) {
        if self.editor {
            unsafe {
                let g = &*self.gui;
                g.hide.map(|f| f(self.plugin));
                g.destroy.map(|d| d(self.plugin));
            }
            self.editor = false;
        }
    }

    /// The plugin's saved state (UI thread).
    pub fn save_state(&self) -> Option<Vec<u8>> {
        if self.state.is_null() {
            return None;
        }
        unsafe extern "C" fn write(s: *const clap_ostream, buf: *const c_void, size: u64) -> i64 {
            let v = unsafe { &mut *((*s).ctx as *mut Vec<u8>) };
            v.extend_from_slice(unsafe { std::slice::from_raw_parts(buf as *const u8, size as usize) });
            size as i64
        }
        let mut out: Vec<u8> = Vec::new();
        let stream = clap_ostream { ctx: &mut out as *mut Vec<u8> as *mut c_void, write: Some(write) };
        let ok = unsafe { (*self.state).save.is_some_and(|f| f(self.plugin, &stream)) };
        ok.then_some(out)
    }

    /// Restores a saved state (UI thread).
    pub fn load_state(&mut self, data: &[u8]) -> bool {
        if self.state.is_null() {
            return false;
        }
        struct Reader<'a>(&'a [u8], usize);
        unsafe extern "C" fn read(s: *const clap_istream, buf: *mut c_void, size: u64) -> i64 {
            let r = unsafe { &mut *((*s).ctx as *mut Reader) };
            let n = (size as usize).min(r.0.len() - r.1);
            unsafe { ptr::copy_nonoverlapping(r.0.as_ptr().add(r.1), buf as *mut u8, n) };
            r.1 += n;
            n as i64
        }
        let mut r = Reader(data, 0);
        let stream = clap_istream { ctx: &mut r as *mut Reader as *mut c_void, read: Some(read) };
        unsafe { (*self.state).load.is_some_and(|f| f(self.plugin, &stream)) }
    }

    /// Processes `n` frames (≤ max) with the queued events whose time falls inside them.
    fn process_chunk(&mut self, left: &mut [f32], right: &mut [f32]) {
        let n = left.len() as u32;
        self.notes.clear();
        let mut rest = Vec::new();
        for e in self.queue.drain(..) {
            if e.time < n {
                self.notes.push(clap_event_note {
                    header: clap_event_header {
                        size: std::mem::size_of::<clap_event_note>() as u32,
                        time: e.time,
                        space_id: CLAP_CORE_EVENT_SPACE_ID,
                        type_: if e.on { CLAP_EVENT_NOTE_ON } else { CLAP_EVENT_NOTE_OFF },
                        flags: 0,
                    },
                    note_id: -1,
                    port_index: 0,
                    channel: 0,
                    key: i16::from(e.key),
                    velocity: f64::from(e.velocity),
                });
            } else {
                rest.push(Ev { time: e.time - n, ..e });
            }
        }
        self.queue = rest;
        unsafe extern "C" fn size(l: *const clap_input_events) -> u32 {
            unsafe { (*((*l).ctx as *const Vec<clap_event_note>)).len() as u32 }
        }
        unsafe extern "C" fn get(l: *const clap_input_events, i: u32) -> *const clap_event_header {
            let v = unsafe { &*((*l).ctx as *const Vec<clap_event_note>) };
            v.get(i as usize).map_or(ptr::null(), |e| &e.header as *const clap_event_header)
        }
        unsafe extern "C" fn push(_l: *const clap_output_events, _e: *const clap_event_header) -> bool {
            true
        }
        let ins = clap_input_events { ctx: &self.notes as *const Vec<clap_event_note> as *mut c_void, size: Some(size), get: Some(get) };
        let outs = clap_output_events { ctx: ptr::null_mut(), try_push: Some(push) };
        let mut chans = [left.as_mut_ptr(), right.as_mut_ptr()];
        let mut buf = clap_audio_buffer { data32: chans.as_mut_ptr(), data64: ptr::null_mut(), channel_count: 2, latency: 0, constant_mask: 0 };
        let p = clap_process {
            steady_time: self.steady,
            frames_count: n,
            transport: ptr::null(),
            audio_inputs: ptr::null(),
            audio_outputs: &mut buf,
            audio_inputs_count: 0,
            audio_outputs_count: 1,
            in_events: &ins,
            out_events: &outs,
        };
        unsafe {
            if let Some(f) = (*self.plugin).process {
                f(self.plugin, &p);
            }
        }
        self.steady += i64::from(n);
    }
}

impl os_engine::Instrument for Plugin {
    fn note_on(&mut self, offset: u32, key: u8, velocity: f32) {
        self.queue.push(Ev { time: offset, on: true, key, velocity });
    }

    fn note_off(&mut self, offset: u32, key: u8) {
        self.queue.push(Ev { time: offset, on: false, key, velocity: 0.0 });
    }

    fn process(&mut self, rate: u32, left: &mut [f32], right: &mut [f32]) {
        left.fill(0.0);
        right.fill(0.0);
        if self.rate != rate {
            // Activation is a main-thread call: ask the UI to do it and stay silent until then.
            if debug() && self.wanted_rate.load(Ordering::Acquire) != rate {
                eprintln!("[os-clap] {} asks for {rate} Hz (active at {})", self.desc.name, self.rate);
            }
            self.wanted_rate.store(rate, Ordering::Release);
            return;
        }
        if !self.processing {
            self.processing = unsafe { (*self.plugin).start_processing.is_none_or(|s| s(self.plugin)) };
            if debug() {
                eprintln!("[os-clap] {} start_processing: {}", self.desc.name, self.processing);
            }
            if !self.processing {
                return;
            }
        }
        self.queue.sort_by_key(|e| e.time);
        let max = self.max_frames as usize;
        let mut at = 0;
        while at < left.len() {
            let n = (left.len() - at).min(max);
            let (l, r) = (&mut left[at..at + n], &mut right[at..at + n]);
            self.process_chunk(l, r);
            at += n;
        }
        if debug() {
            // OS_DEBUG: the hosted plugin's peak level about once a second (stderr).
            self.dbg_peak = left.iter().chain(right.iter()).fold(self.dbg_peak, |m, v| m.max(v.abs()));
            self.dbg_frames += left.len() as u64;
            if self.dbg_frames >= u64::from(rate) {
                eprintln!("[os-clap] {} peak {:.3}", self.desc.name, self.dbg_peak);
                (self.dbg_frames, self.dbg_peak) = (0, 0.0);
            }
        }
    }
}

impl Drop for Plugin {
    fn drop(&mut self) {
        self.close_editor();
        self.deactivate();
        unsafe {
            if let Some(d) = (*self.plugin).destroy {
                d(self.plugin);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use os_engine::Instrument;

    /// Loads the bundled Open Sampler (built by `cargo run -p bundle-plugins`) when it is there.
    fn open_sampler() -> Option<Arc<Library>> {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        for t in ["target-dev", "target"] {
            let p = root.join(t).join("bundled").join("Open Sampler.clap");
            if p.exists() {
                return Some(Library::load(&p).expect("load"));
            }
        }
        None
    }

    #[test]
    fn hosts_open_sampler() {
        let Some(lib) = open_sampler() else {
            eprintln!("Open Sampler.clap not built; skipped");
            return;
        };
        let d = &lib.plugins[0];
        assert_eq!(d.name, "Open Sampler");
        assert!(d.is_instrument());
        let mut p = Plugin::new(&lib, &d.id).unwrap();
        assert!(p.has_editor());
        // Give it a sample through its state (nih-plug JSON: persisted fields are JSON strings).
        let wav = std::env::temp_dir().join("os-clap-test.wav");
        let tone: Vec<f32> = (0..4800).flat_map(|i| { let v = (i as f32 * 0.05).sin() * 0.5; [v, v] }).collect();
        os_engine::wav::write_f32_stereo(&mut std::fs::File::create(&wav).unwrap(), 48000, &tone).unwrap();
        let path = serde_string(&wav.to_string_lossy());
        let state = format!("{{\"version\":\"0.0.1\",\"params\":{{}},\"fields\":{{\"sample-path\":{path}}}}}");
        // nih-plug prefixes its CLAP state with the JSON length (u64, little-endian).
        let mut blob = (state.len() as u64).to_le_bytes().to_vec();
        blob.extend_from_slice(state.as_bytes());
        assert!(p.load_state(&blob));
        assert!(p.activate(48000));
        let (mut l, mut r) = (vec![0.0f32; 512], vec![0.0f32; 512]);
        p.note_on(10, 60, 100.0 / 128.0);
        p.process(48000, &mut l, &mut r);
        assert_eq!(l[9], 0.0);
        let peak = l.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(peak > 0.05, "plays the sample (peak {peak})");
        let saved = String::from_utf8_lossy(&p.save_state().unwrap()[8..]).into_owned();
        assert!(saved.contains("os-clap-test.wav"), "{saved}");
    }

    /// The whole path OpenStudio uses: a pattern on a plugin channel, played by the engine through the hosted .clap.
    #[test]
    fn engine_plays_a_pattern_through_open_sampler() {
        let Some(lib) = open_sampler() else { return };
        let mut p = Plugin::new(&lib, &lib.plugins[0].id).unwrap();
        let wav = std::env::temp_dir().join("os-clap-engine.wav");
        let tone: Vec<f32> = (0..9600).flat_map(|i| { let v = (i as f32 * 0.03).sin() * 0.5; [v, v] }).collect();
        os_engine::wav::write_f32_stereo(&mut std::fs::File::create(&wav).unwrap(), 48000, &tone).unwrap();
        let state = format!("{{\"version\":\"0.0.1\",\"params\":{{}},\"fields\":{{\"sample-path\":{}}}}}", serde_string(&wav.to_string_lossy()));
        let mut blob = (state.len() as u64).to_le_bytes().to_vec();
        blob.extend_from_slice(state.as_bytes());
        assert!(p.load_state(&blob));
        assert!(p.activate(48000));
        let shared: os_engine::SharedInstrument = Arc::new(Mutex::new(p));
        let ch = os_model::Channel {
            index: 0, kind: os_model::ChannelKind::Plugin, name: None, plugin: Some("Open Sampler".into()), color: None,
            enabled: true, volume: 0.78125, pan: 0.0, pitch_cents: 0, mixer_track: 0, sample_path: None,
            layer_children: vec![], envelopes: Default::default(),
        };
        let note = os_model::Note { length: 24, ..os_model::Note::step(0, 0) };
        let project = os_model::Project {
            ppq: 96, tempo: Some(130.0), channels: vec![ch],
            patterns: vec![os_model::Pattern { number: 1, notes: vec![note], ..Default::default() }],
            ..Default::default()
        };
        let mut lib = os_engine::SampleLibrary::default();
        let mut e = os_engine::Engine::new(&project, &mut lib, 48000, os_engine::PlayMode::Pattern(1));
        e.set_instrument(0, shared);
        let mut out = vec![0.0f32; 1024];
        e.process(&mut out);
        let peak = out.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(peak > 0.05, "engine output through Open Sampler (peak {peak})");
    }

    /// JSON string literal holding `s` as a JSON string (nih-plug stores persisted fields as serialised JSON).
    fn serde_string(s: &str) -> String {
        let inner = format!("\"{}\"", s.replace('\\', "\\\\"));
        format!("\"{}\"", inner.replace('\\', "\\\\").replace('"', "\\\""))
    }
}
