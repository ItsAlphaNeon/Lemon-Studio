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
    clap_event_header, clap_event_note, clap_event_param_gesture, clap_event_param_mod, clap_event_param_value, clap_input_events,
    clap_output_events, CLAP_CORE_EVENT_SPACE_ID, CLAP_EVENT_NOTE_CHOKE, CLAP_EVENT_NOTE_OFF, CLAP_EVENT_NOTE_ON,
    CLAP_EVENT_PARAM_GESTURE_BEGIN, CLAP_EVENT_PARAM_GESTURE_END, CLAP_EVENT_PARAM_MOD, CLAP_EVENT_PARAM_VALUE,
};
use clap_sys::ext::gui::{clap_plugin_gui, clap_window, clap_window_handle, CLAP_EXT_GUI, CLAP_WINDOW_API_WIN32};
use clap_sys::ext::latency::{clap_plugin_latency, CLAP_EXT_LATENCY};
use clap_sys::ext::params::{
    clap_host_params, clap_param_info, clap_plugin_params, CLAP_EXT_PARAMS, CLAP_PARAM_IS_AUTOMATABLE, CLAP_PARAM_IS_BYPASS,
    CLAP_PARAM_IS_ENUM, CLAP_PARAM_IS_HIDDEN, CLAP_PARAM_IS_MODULATABLE_PER_NOTE_ID, CLAP_PARAM_IS_READONLY, CLAP_PARAM_IS_STEPPED,
};
use clap_sys::ext::state::{clap_plugin_state, CLAP_EXT_STATE};
use clap_sys::factory::plugin_factory::{clap_plugin_factory, CLAP_PLUGIN_FACTORY_ID};
use clap_sys::host::clap_host;
use clap_sys::plugin::clap_plugin;
use clap_sys::process::{clap_process, CLAP_PROCESS_CONTINUE, CLAP_PROCESS_CONTINUE_IF_NOT_QUIET, CLAP_PROCESS_TAIL};
use clap_sys::stream::{clap_istream, clap_ostream};
use clap_sys::version::{clap_version_is_compatible, CLAP_VERSION};

/// A plugin a library offers (the hosting layer's descriptor, format `Clap`).
pub use os_host::{set_waker, wake, Descriptor};

/// What CLAP features say a plugin is: "instrument" makes an instrument, else "audio-effect" an insert effect.
fn kind_of(features: &[String]) -> os_host::Kind {
    if features.iter().any(|f| f == "instrument") {
        os_host::Kind::Instrument
    } else if features.iter().any(|f| f == "audio-effect") {
        os_host::Kind::Effect
    } else {
        os_host::Kind::Other
    }
}

/// A loaded `.clap` file.
pub struct Library {
    entry: *const clap_plugin_entry,
    factory: *const clap_plugin_factory,
    pub path: PathBuf,
    pub plugins: Vec<Descriptor>,
    /// `None` for a plugin linked into this program ([`Library::from_entry`]).
    _lib: Option<libloading::Library>,
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
            Self::open(entry, path, Some(lib))
        }
    }

    /// A plugin compiled into this program: `entry` points at its `clap_entry` table (same layout in every CLAP
    /// binding), `path` is what its `init` receives.
    ///
    /// # Safety
    /// `entry` must point at a valid, `'static` `clap_plugin_entry`.
    pub unsafe fn from_entry(entry: *const std::ffi::c_void, path: &Path) -> Result<Arc<Library>, String> {
        unsafe { Self::open(entry as *const clap_plugin_entry, path, None) }
    }

    unsafe fn open(entry: *const clap_plugin_entry, path: &Path, lib: Option<libloading::Library>) -> Result<Arc<Library>, String> {
        unsafe {
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
                plugins.push(Descriptor {
                    format: os_host::Format::Clap,
                    id: cstr((*d).id),
                    name: cstr((*d).name),
                    vendor: cstr((*d).vendor),
                    version: cstr((*d).version),
                    kind: kind_of(&features),
                    features,
                    path: path.to_owned(),
                });
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

impl os_host::Library for Library {
    fn path(&self) -> &Path {
        &self.path
    }

    fn plugins(&self) -> &[Descriptor] {
        &self.plugins
    }

    fn create(self: Arc<Self>, id: &str) -> Result<os_host::SharedPlugin, String> {
        Ok(Arc::new(Mutex::new(Plugin::new(&self, id)?)))
    }
}

/// Host-side state the plugin's callbacks reach through `clap_host::host_data`.
struct HostData {
    callback: AtomicBool,
    restart: AtomicBool,
    /// The plugin asked for a parameter flush (`clap_host_params.request_flush`): an edit from its editor waits in its
    /// output queue and only takes effect when the host flushes (or processes). The host's waker
    /// (`os_host::set_waker`) makes `idle` run at once instead of on the next timer tick.
    flush: AtomicBool,
}

static HOST_NAME: &CStr = c"OpenStudio";
static HOST_URL: &CStr = c"";
static HOST_VERSION: &CStr = c"0.0.1";

unsafe extern "C" fn host_get_extension(_host: *const clap_host, id: *const c_char) -> *const c_void {
    if !id.is_null() && unsafe { CStr::from_ptr(id) } == CLAP_EXT_PARAMS {
        return &HOST_PARAMS as *const clap_host_params as *const c_void;
    }
    ptr::null()
}
unsafe extern "C" fn host_params_rescan(_host: *const clap_host, _flags: u32) {}
unsafe extern "C" fn host_params_clear(_host: *const clap_host, _id: u32, _flags: u32) {}
unsafe extern "C" fn host_params_request_flush(host: *const clap_host) {
    unsafe { (*((*host).host_data as *const HostData)).flush.store(true, Ordering::Release) };
    wake();
}
static HOST_PARAMS: clap_host_params =
    clap_host_params { rescan: Some(host_params_rescan), clear: Some(host_params_clear), request_flush: Some(host_params_request_flush) };
unsafe extern "C" fn host_request_restart(host: *const clap_host) {
    unsafe { (*((*host).host_data as *const HostData)).restart.store(true, Ordering::Release) };
    wake();
}
unsafe extern "C" fn host_request_process(_host: *const clap_host) {}
unsafe extern "C" fn host_request_callback(host: *const clap_host) {
    unsafe { (*((*host).host_data as *const HostData)).callback.store(true, Ordering::Release) };
    wake();
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    On,
    Off,
    Choke,
}

/// One queued note event.
#[derive(Clone, Copy)]
struct Ev {
    time: u32,
    kind: Kind,
    key: u8,
    velocity: f32,
    /// Sample start (0..1) sent as per-note modulation of `Plugin::start_param` (0 = none).
    start: f32,
}

/// A plugin instance.
pub struct Plugin {
    pub desc: Descriptor,
    plugin: *const clap_plugin,
    gui: *const clap_plugin_gui,
    state: *const clap_plugin_state,
    params: *const clap_plugin_params,
    latency: *const clap_plugin_latency,
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
    mods: Vec<clap_event_param_mod>,
    /// This chunk's events in time order (pointers into `notes` / `mods`).
    order: Vec<*const clap_event_header>,
    /// Parameter that takes a note's sample start as per-note modulation (`use_start_param`).
    start_param: Option<u32>,
    next_note_id: i32,
    steady: i64,
    /// The plugin's last process status says it still has sound to deliver (`Instrument::active`).
    sounding: bool,
    dbg_peak: f32,
    dbg_frames: u64,
    /// The editor is open (parented into a host window).
    editor: bool,
    /// Input copies for effect processing (mixer slots).
    fx_in: (Vec<f32>, Vec<f32>),
    /// Parameter ranges (id, min, max), read once after init (values cross the hosting layer normalized).
    ranges: Vec<(u32, f64, f64)>,
    /// Host parameter changes (id, plain value) for the next process call or flush.
    param_queue: Vec<(u32, f64)>,
    /// This chunk's parameter events (sent before the notes, at time 0).
    pvals: Vec<clap_event_param_value>,
    /// Changes the plugin reported (its editor): gesture begin / end and values, plain.
    param_out: Vec<OutEv>,
    _lib: Arc<Library>,
}

/// A parameter event the plugin sent the host.
#[derive(Clone, Copy)]
enum OutEv {
    Begin(u32),
    Value(u32, f64),
    End(u32),
}

/// Most plugin -> host parameter events kept until the app takes them.
const MAX_OUT: usize = 4096;

/// `clap_input_events` over the `Vec<*const clap_event_header>` behind `ctx`.
unsafe extern "C" fn events_size(l: *const clap_input_events) -> u32 {
    unsafe { (*((*l).ctx as *const Vec<*const clap_event_header>)).len() as u32 }
}

unsafe extern "C" fn events_get(l: *const clap_input_events, i: u32) -> *const clap_event_header {
    let v = unsafe { &*((*l).ctx as *const Vec<*const clap_event_header>) };
    v.get(i as usize).copied().unwrap_or(ptr::null())
}

/// `clap_output_events.try_push` collecting parameter events into the `Vec<OutEv>` behind `ctx`.
unsafe extern "C" fn push_out(l: *const clap_output_events, e: *const clap_event_header) -> bool {
    unsafe {
        let v = &mut *((*l).ctx as *mut Vec<OutEv>);
        if e.is_null() || (*e).space_id != CLAP_CORE_EVENT_SPACE_ID || v.len() >= MAX_OUT {
            return true;
        }
        match (*e).type_ {
            CLAP_EVENT_PARAM_VALUE => {
                let p = &*(e as *const clap_event_param_value);
                v.push(OutEv::Value(p.param_id, p.value));
            }
            CLAP_EVENT_PARAM_GESTURE_BEGIN => v.push(OutEv::Begin((*(e as *const clap_event_param_gesture)).param_id)),
            CLAP_EVENT_PARAM_GESTURE_END => v.push(OutEv::End((*(e as *const clap_event_param_gesture)).param_id)),
            _ => {}
        }
        true
    }
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
        let host_data = Box::new(HostData { callback: AtomicBool::new(false), restart: AtomicBool::new(false), flush: AtomicBool::new(false) });
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
            let params = ext(CLAP_EXT_PARAMS) as *const clap_plugin_params;
            let latency = ext(CLAP_EXT_LATENCY) as *const clap_plugin_latency;
            let mut ranges = Vec::new();
            if !params.is_null() {
                for i in 0..(*params).count.map_or(0, |c| c(plugin)) {
                    let mut info: clap_param_info = std::mem::zeroed();
                    if (*params).get_info.is_some_and(|g| g(plugin, i, &mut info)) {
                        ranges.push((info.id, info.min_value, info.max_value));
                    }
                }
            }
            Ok(Plugin {
                desc,
                plugin,
                gui,
                state,
                params,
                _host: host,
                host_data,
                rate: 0,
                max_frames: MAX_FRAMES,
                processing: false,
                wanted_rate: AtomicU32::new(0),
                queue: Vec::with_capacity(256),
                notes: Vec::with_capacity(256),
                mods: Vec::with_capacity(64),
                order: Vec::with_capacity(320),
                start_param: None,
                next_note_id: 0,
                steady: 0,
                sounding: false,
                dbg_peak: 0.0,
                dbg_frames: 0,
                editor: false,
                fx_in: (Vec::new(), Vec::new()),
                latency,
                ranges,
                param_queue: Vec::new(),
                pvals: Vec::with_capacity(64),
                param_out: Vec::new(),
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

    /// Lets the plugin apply the parameter edits its editor queued (and drops the events it reports back: the
    /// plugin already holds the values). Without this an edit made while no audio stream is processing the plugin
    /// never takes effect, and the queue fills up. The caller holds the instance's mutex, so this never overlaps
    /// `process`.
    fn flush_params(&mut self) {
        if self.params.is_null() {
            return;
        }
        // Host parameter changes waiting for a process call go in now; parameter events the plugin sends back are
        // collected for `take_param_events`.
        self.fill_param_events();
        self.order.clear();
        self.order.extend(self.pvals.iter().map(|e| &e.header as *const clap_event_header));
        let ins = clap_input_events { ctx: &self.order as *const Vec<*const clap_event_header> as *mut c_void, size: Some(events_size), get: Some(events_get) };
        let outs = clap_output_events { ctx: &mut self.param_out as *mut Vec<OutEv> as *mut c_void, try_push: Some(push_out) };
        unsafe {
            if let Some(f) = (*self.params).flush {
                f(self.plugin, &ins, &outs);
            }
        }
        self.order.clear();
    }

    /// Moves the queued host parameter changes into `pvals` (events at time 0).
    fn fill_param_events(&mut self) {
        self.pvals.clear();
        for (id, value) in self.param_queue.drain(..) {
            self.pvals.push(clap_event_param_value {
                header: clap_event_header {
                    size: std::mem::size_of::<clap_event_param_value>() as u32,
                    time: 0,
                    space_id: CLAP_CORE_EVENT_SPACE_ID,
                    type_: CLAP_EVENT_PARAM_VALUE,
                    flags: 0,
                },
                param_id: id,
                cookie: ptr::null_mut(),
                note_id: -1,
                port_index: -1,
                channel: -1,
                key: -1,
                value,
            });
        }
    }

    /// (min, max) of parameter `id`.
    fn range(&self, id: u32) -> Option<(f64, f64)> {
        self.ranges.iter().find(|r| r.0 == id).map(|&(_, lo, hi)| (lo, hi))
    }

    fn normalize(&self, id: u32, plain: f64) -> f64 {
        match self.range(id) {
            Some((lo, hi)) if hi > lo => ((plain - lo) / (hi - lo)).clamp(0.0, 1.0),
            _ => plain,
        }
    }

    fn plain(&self, id: u32, norm: f64) -> f64 {
        match self.range(id) {
            Some((lo, hi)) => lo + norm.clamp(0.0, 1.0) * (hi - lo),
            None => norm,
        }
    }

    /// Sends each note's sample start (`Instrument::note_on_from`) as per-note modulation of the parameter named
    /// `name`, which must be modulatable per note id (UI thread; spec: plugins/open-sampler.md §7). False if the
    /// plugin has no such parameter (notes then play from the top).
    pub fn use_start_param(&mut self, name: &str) -> bool {
        self.start_param = None;
        if self.params.is_null() {
            return false;
        }
        unsafe {
            let count = (*self.params).count.map_or(0, |c| c(self.plugin));
            for i in 0..count {
                let mut info: clap_param_info = std::mem::zeroed();
                if !(*self.params).get_info.is_some_and(|g| g(self.plugin, i, &mut info)) {
                    continue;
                }
                let n = CStr::from_ptr(info.name.as_ptr()).to_string_lossy();
                if n == name && info.flags & CLAP_PARAM_IS_MODULATABLE_PER_NOTE_ID != 0 {
                    self.start_param = Some(info.id);
                    return true;
                }
            }
        }
        false
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
        if self.host_data.flush.swap(false, Ordering::AcqRel) || !self.param_queue.is_empty() {
            self.flush_params();
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
        self.process_chunk_in(left, right, None);
    }

    /// `process_chunk` with an audio input (effects): `input` holds the frames to process, `left` / `right` receive
    /// the output.
    fn process_chunk_in(&mut self, left: &mut [f32], right: &mut [f32], input: Option<(&mut [f32], &mut [f32])>) {
        let n = left.len() as u32;
        self.notes.clear();
        self.mods.clear();
        let mut rest = Vec::new();
        // Event order: (index into notes, index into mods); a note's modulation follows its note on at the same time.
        let mut seq: Vec<(usize, Option<usize>)> = Vec::new();
        for e in self.queue.drain(..) {
            if e.time < n {
                // A note with a sample start gets an id, so its modulation reaches only that voice.
                let id = match (e.kind, self.start_param) {
                    (Kind::On, Some(_)) if e.start > 0.0 => {
                        self.next_note_id = self.next_note_id.wrapping_add(1) & 0x7fff_ffff;
                        self.next_note_id
                    }
                    _ => -1,
                };
                self.notes.push(clap_event_note {
                    header: clap_event_header {
                        size: std::mem::size_of::<clap_event_note>() as u32,
                        time: e.time,
                        space_id: CLAP_CORE_EVENT_SPACE_ID,
                        type_: match e.kind {
                            Kind::On => CLAP_EVENT_NOTE_ON,
                            Kind::Off => CLAP_EVENT_NOTE_OFF,
                            Kind::Choke => CLAP_EVENT_NOTE_CHOKE,
                        },
                        flags: 0,
                    },
                    note_id: id,
                    port_index: 0,
                    channel: 0,
                    key: i16::from(e.key),
                    velocity: f64::from(e.velocity),
                });
                seq.push((self.notes.len() - 1, None));
                if let (Some(param), true) = (self.start_param, id >= 0) {
                    self.mods.push(clap_event_param_mod {
                        header: clap_event_header {
                            size: std::mem::size_of::<clap_event_param_mod>() as u32,
                            time: e.time,
                            space_id: CLAP_CORE_EVENT_SPACE_ID,
                            type_: CLAP_EVENT_PARAM_MOD,
                            flags: 0,
                        },
                        param_id: param,
                        cookie: ptr::null_mut(),
                        note_id: id,
                        port_index: 0,
                        channel: 0,
                        key: i16::from(e.key),
                        amount: f64::from(e.start),
                    });
                    seq.push((0, Some(self.mods.len() - 1)));
                }
            } else {
                rest.push(Ev { time: e.time - n, ..e });
            }
        }
        self.queue = rest;
        // `queue` is sorted by time, so `seq` already is. Host parameter changes go first, at time 0.
        self.fill_param_events();
        self.order.clear();
        self.order.extend(self.pvals.iter().map(|e| &e.header as *const clap_event_header));
        for &(ni, mi) in &seq {
            self.order.push(match mi {
                Some(m) => &self.mods[m].header as *const clap_event_header,
                None => &self.notes[ni].header as *const clap_event_header,
            });
        }
        let ins = clap_input_events { ctx: &self.order as *const Vec<*const clap_event_header> as *mut c_void, size: Some(events_size), get: Some(events_get) };
        let outs = clap_output_events { ctx: &mut self.param_out as *mut Vec<OutEv> as *mut c_void, try_push: Some(push_out) };
        let mut chans = [left.as_mut_ptr(), right.as_mut_ptr()];
        let mut buf = clap_audio_buffer { data32: chans.as_mut_ptr(), data64: ptr::null_mut(), channel_count: 2, latency: 0, constant_mask: 0 };
        let has_in = input.is_some();
        let mut in_chans = match input {
            Some((l, r)) => [l.as_mut_ptr(), r.as_mut_ptr()],
            None => [ptr::null_mut(), ptr::null_mut()],
        };
        let in_buf = clap_audio_buffer { data32: in_chans.as_mut_ptr(), data64: ptr::null_mut(), channel_count: 2, latency: 0, constant_mask: 0 };
        let p = clap_process {
            steady_time: self.steady,
            frames_count: n,
            transport: ptr::null(),
            audio_inputs: if has_in { &in_buf } else { ptr::null() },
            audio_outputs: &mut buf,
            audio_inputs_count: u32::from(has_in),
            audio_outputs_count: 1,
            in_events: &ins,
            out_events: &outs,
        };
        let status = unsafe { (*self.plugin).process.map_or(CLAP_PROCESS_CONTINUE, |f| f(self.plugin, &p)) };
        // CONTINUE / TAIL: the plugin says it is still going; CONTINUE_IF_NOT_QUIET: while its output is not silent.
        let loud = left.iter().chain(right.iter()).any(|v| v.abs() > 1e-5);
        self.sounding = status == CLAP_PROCESS_CONTINUE || status == CLAP_PROCESS_TAIL || (status == CLAP_PROCESS_CONTINUE_IF_NOT_QUIET && loud);
        self.steady += i64::from(n);
    }
}

// The hosting layer's view of a CLAP instance (the inherent methods above do the work).
impl os_host::Plugin for Plugin {
    fn desc(&self) -> &Descriptor {
        &self.desc
    }

    fn activate(&mut self, rate: u32) -> bool {
        Plugin::activate(self, rate)
    }

    fn idle(&mut self) {
        Plugin::idle(self)
    }

    fn has_editor(&self) -> bool {
        Plugin::has_editor(self)
    }

    fn editor_open(&self) -> bool {
        self.editor
    }

    fn open_editor(&mut self, parent: *mut c_void) -> Result<(u32, u32), String> {
        Plugin::open_editor(self, parent)
    }

    fn editor_size(&self) -> Option<(u32, u32)> {
        Plugin::editor_size(self)
    }

    fn close_editor(&mut self) {
        Plugin::close_editor(self)
    }

    fn save_state(&self) -> Option<Vec<u8>> {
        Plugin::save_state(self)
    }

    fn load_state(&mut self, data: &[u8]) -> bool {
        Plugin::load_state(self, data)
    }

    fn latency(&self) -> u32 {
        if self.latency.is_null() || self.rate == 0 {
            return 0;
        }
        unsafe { (*self.latency).get.map_or(0, |g| g(self.plugin)) }
    }

    fn use_start_param(&mut self, name: &str) -> bool {
        Plugin::use_start_param(self, name)
    }

    fn param_count(&self) -> usize {
        if self.params.is_null() { 0 } else { unsafe { (*self.params).count.map_or(0, |c| c(self.plugin)) as usize } }
    }

    fn param_info(&self, index: usize) -> Option<os_host::ParamInfo> {
        if self.params.is_null() {
            return None;
        }
        unsafe {
            let mut info: clap_param_info = std::mem::zeroed();
            if !(*self.params).get_info.is_some_and(|g| g(self.plugin, index as u32, &mut info)) {
                return None;
            }
            let (lo, hi) = (info.min_value, info.max_value);
            let f = info.flags;
            Some(os_host::ParamInfo {
                id: info.id,
                name: CStr::from_ptr(info.name.as_ptr()).to_string_lossy().into_owned(),
                unit: String::new(),
                group: CStr::from_ptr(info.module.as_ptr()).to_string_lossy().into_owned(),
                min: lo,
                max: hi,
                default: if hi > lo { ((info.default_value - lo) / (hi - lo)).clamp(0.0, 1.0) } else { 0.0 },
                steps: if f & CLAP_PARAM_IS_STEPPED != 0 { (hi - lo).round().max(0.0) as u32 } else { 0 },
                automatable: f & CLAP_PARAM_IS_AUTOMATABLE != 0,
                read_only: f & CLAP_PARAM_IS_READONLY != 0,
                hidden: f & CLAP_PARAM_IS_HIDDEN != 0,
                bypass: f & CLAP_PARAM_IS_BYPASS != 0,
                enumerated: f & CLAP_PARAM_IS_ENUM != 0,
            })
        }
    }

    fn param_value(&self, id: u32) -> Option<f64> {
        if self.params.is_null() {
            return None;
        }
        // A change still in the queue is the value the plugin will have.
        if let Some(&(_, v)) = self.param_queue.iter().rev().find(|q| q.0 == id) {
            return Some(self.normalize(id, v));
        }
        let mut v = 0.0;
        unsafe { (*self.params).get_value.is_some_and(|g| g(self.plugin, id, &mut v)) }.then(|| self.normalize(id, v))
    }

    fn param_text(&self, id: u32, value: f64) -> Option<String> {
        if self.params.is_null() {
            return None;
        }
        let mut buf = [0 as c_char; 128];
        let plain = self.plain(id, value);
        let ok = unsafe { (*self.params).value_to_text.is_some_and(|f| f(self.plugin, id, plain, buf.as_mut_ptr(), buf.len() as u32)) };
        ok.then(|| unsafe { CStr::from_ptr(buf.as_ptr()) }.to_string_lossy().into_owned())
    }

    fn set_param(&mut self, id: u32, value: f64) -> bool {
        if self.range(id).is_none() {
            return false;
        }
        let plain = self.plain(id, value);
        self.param_queue.retain(|q| q.0 != id);
        self.param_queue.push((id, plain));
        os_host::wake();
        true
    }

    fn take_param_events(&mut self) -> Vec<os_host::ParamEvent> {
        let raw = std::mem::take(&mut self.param_out);
        raw.into_iter()
            .map(|e| match e {
                OutEv::Begin(id) => os_host::ParamEvent::Begin(id),
                OutEv::Value(id, v) => os_host::ParamEvent::Value(id, self.normalize(id, v)),
                OutEv::End(id) => os_host::ParamEvent::End(id),
            })
            .collect()
    }
}

impl os_engine::Instrument for Plugin {
    fn active(&self) -> bool {
        self.sounding
    }

    fn reset(&mut self) {
        self.queue.clear();
        self.sounding = false;
        if self.rate == 0 {
            return;
        }
        // CLAP: stop processing, then `reset` clears the plugin's voices and tails; processing restarts on the next block.
        unsafe {
            if self.processing {
                if let Some(s) = (*self.plugin).stop_processing {
                    s(self.plugin);
                }
                self.processing = false;
            }
            if let Some(r) = (*self.plugin).reset {
                r(self.plugin);
            }
        }
    }

    fn note_on(&mut self, offset: u32, key: u8, velocity: f32) {
        self.queue.push(Ev { time: offset, kind: Kind::On, key, velocity, start: 0.0 });
    }

    fn note_off(&mut self, offset: u32, key: u8) {
        self.queue.push(Ev { time: offset, kind: Kind::Off, key, velocity: 0.0, start: 0.0 });
    }

    fn note_on_from(&mut self, offset: u32, key: u8, velocity: f32, start: f32) {
        self.queue.push(Ev { time: offset, kind: Kind::On, key, velocity, start: start.clamp(0.0, 1.0) });
    }

    fn choke(&mut self, offset: u32, key: u8) {
        self.queue.push(Ev { time: offset, kind: Kind::Choke, key, velocity: 0.0, start: 0.0 });
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

// spec: ui/mixer.md §7 (effect slots): a CLAP effect processes a mixer track in place. Until the UI thread has
// activated it at the stream's rate the sound passes through unchanged.
impl os_engine::HostedEffect for Plugin {
    fn process_fx(&mut self, rate: u32, left: &mut [f32], right: &mut [f32]) {
        if self.rate != rate {
            self.wanted_rate.store(rate, Ordering::Release);
            return;
        }
        if !self.processing {
            self.processing = unsafe { (*self.plugin).start_processing.is_none_or(|s| s(self.plugin)) };
            if !self.processing {
                return;
            }
        }
        let (mut il, mut ir) = std::mem::take(&mut self.fx_in);
        let max = self.max_frames as usize;
        let mut at = 0;
        while at < left.len() {
            let n = (left.len() - at).min(max);
            il.clear();
            ir.clear();
            il.extend_from_slice(&left[at..at + n]);
            ir.extend_from_slice(&right[at..at + n]);
            self.process_chunk_in(&mut left[at..at + n], &mut right[at..at + n], Some((&mut il[..], &mut ir[..])));
            at += n;
        }
        self.fx_in = (il, ir);
    }

    fn reset_fx(&mut self) {
        os_engine::Instrument::reset(self);
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
        // This test's own build folder (target*/debug/deps/x.exe -> target*), then the usual ones.
        let own = std::env::current_exe().ok().and_then(|e| Some(e.parent()?.parent()?.parent()?.to_path_buf()));
        for t in own.into_iter().chain(["target-dev", "target"].map(|t| root.join(t))) {
            let p = t.join("bundled").join("Open Sampler.clap");
            if p.exists() {
                return Some(Library::load(&p).expect("load"));
            }
        }
        None
    }

    /// The hosting layer's parameter surface on a CLAP instance: info, normalized get / set, text, and the edit
    /// reaching the plugin's state.
    #[test]
    fn parameters_through_the_hosting_layer() {
        let Some(lib) = open_sampler() else { return };
        let shared = os_host::Library::create(lib.clone() as Arc<dyn os_host::Library>, &lib.plugins[0].id).unwrap();
        let mut p = shared.lock().unwrap();
        assert!(p.param_count() > 20);
        let infos: Vec<os_host::ParamInfo> = (0..p.param_count()).filter_map(|i| p.param_info(i)).collect();
        let start = infos.iter().find(|i| i.name == "Sample start offset").expect("start offset parameter");
        assert!(start.automatable && start.steps == 0);
        assert!((p.param_value(start.id).unwrap() - start.default).abs() < 1e-6);
        assert!(p.set_param(start.id, 0.25));
        assert!(!p.set_param(0xdead_beef, 0.5), "unknown ids are refused");
        p.idle();
        assert!((p.param_value(start.id).unwrap() - 0.25).abs() < 1e-3, "{:?}", p.param_value(start.id));
        assert!(p.param_text(start.id, 0.25).is_some_and(|t| !t.is_empty()));
        let saved = String::from_utf8_lossy(&p.save_state().unwrap()[8..]).into_owned();
        assert!(saved.contains("\"start_offset\":{\"f32\":0.25"), "{saved}");
        assert_eq!(p.desc().uid(), "clap:org.openstudio.open-sampler");
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

    /// A CLAP effect in a mixer slot (spec: ui/mixer.md §7): `OS_CLAP_FX=<effect>.clap` names one (skipped when unset).
    /// The effect gets the track's audio as its input; a unity-gain effect gives it back unchanged.
    #[test]
    fn hosts_an_effect() {
        let Some(path) = std::env::var_os("OS_CLAP_FX") else { return };
        let lib = Library::load(std::path::Path::new(&path)).expect("load");
        let d = lib.plugins.iter().find(|d| d.is_effect()).expect("an audio effect");
        let mut p = Plugin::new(&lib, &d.id).unwrap();
        assert!(p.activate(48000));
        let src: Vec<f32> = (0..2048).map(|i| (i as f32 * 0.02).sin() * 0.5).collect();
        let (mut l, mut r) = (src.clone(), src.clone());
        // The first call may only start processing; run a few blocks.
        for _ in 0..3 {
            l.copy_from_slice(&src);
            r.copy_from_slice(&src);
            os_engine::HostedEffect::process_fx(&mut p, 48000, &mut l, &mut r);
        }
        let peak = l.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(peak > 0.1, "{} passes the input through (peak {peak})", d.name);
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
            layer_children: vec![], envelopes: Default::default(), rack: Default::default(),
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

    /// An MP3 sample (os-engine's fixture: click of 0.9 at 0.1 s) loads and plays through Open Sampler at 48 kHz.
    #[test]
    fn open_sampler_plays_an_mp3() {
        let Some(lib) = open_sampler() else { return };
        let mut p = Plugin::new(&lib, &lib.plugins[0].id).unwrap();
        let mp3 = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../os-engine/tests/data/click_sine.mp3");
        let mp3 = mp3.canonicalize().unwrap().to_string_lossy().trim_start_matches(r"\\?\").to_string();
        let state = format!("{{\"version\":\"0.0.1\",\"params\":{{}},\"fields\":{{\"sample-path\":{}}}}}", serde_string(&mp3));
        let mut blob = (state.len() as u64).to_le_bytes().to_vec();
        blob.extend_from_slice(state.as_bytes());
        assert!(p.load_state(&blob));
        assert!(p.activate(48000));
        let shared: os_engine::SharedInstrument = Arc::new(Mutex::new(p));
        let ch = os_model::Channel {
            index: 0, kind: os_model::ChannelKind::Plugin, name: None, plugin: Some("Open Sampler".into()), color: None,
            enabled: true, volume: 0.78125, pan: 0.0, pitch_cents: 0, mixer_track: 0, sample_path: None,
            layer_children: vec![], envelopes: Default::default(), rack: Default::default(),
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
        let mut out = vec![0.0f32; 2 * 12000];
        for c in out.chunks_mut(1024) {
            e.process(c);
        }
        let left: Vec<f32> = out.iter().step_by(2).copied().collect();
        let peak = (0..left.len()).max_by(|&a, &b| left[a].abs().total_cmp(&left[b].abs())).unwrap();
        assert!(left[peak].abs() > 0.2, "mp3 click heard (peak {})", left[peak]);
        assert!((4780..4840).contains(&peak), "click at 0.1 s (frame {peak})");
        assert!(left[10000..12000].iter().any(|v| v.abs() > 0.05), "sine after 0.2 s");
    }

    /// A held-then-released live note of a long sample, as the typing keyboard plays it: in tune, and it keeps
    /// sounding past the engine's 5 s idle tail until the sample ends.
    #[test]
    fn long_live_note_is_in_tune_and_not_cut_off() {
        let Some(lib) = open_sampler() else { return };
        let mut p = Plugin::new(&lib, &lib.plugins[0].id).unwrap();
        // 30 s of a 441 Hz sine at 44.1 kHz.
        let wav = std::env::temp_dir().join("os-clap-long.wav");
        let tone: Vec<f32> = (0..44100 * 30).flat_map(|i| { let v = (i as f32 / 44100.0 * 441.0 * std::f32::consts::TAU).sin() * 0.5; [v, v] }).collect();
        os_engine::wav::write_f32_stereo(&mut std::fs::File::create(&wav).unwrap(), 44100, &tone).unwrap();
        let state = format!("{{\"version\":\"0.0.1\",\"params\":{{}},\"fields\":{{\"sample-path\":{}}}}}", serde_string(&wav.to_string_lossy()));
        let mut blob = (state.len() as u64).to_le_bytes().to_vec();
        blob.extend_from_slice(state.as_bytes());
        assert!(p.load_state(&blob));
        assert!(p.activate(48000));
        let shared: os_engine::SharedInstrument = Arc::new(Mutex::new(p));
        let ch = os_model::Channel {
            index: 0, kind: os_model::ChannelKind::Plugin, name: None, plugin: Some("Open Sampler".into()), color: None,
            enabled: true, volume: 0.78125, pan: 0.0, pitch_cents: 0, mixer_track: 0, sample_path: None,
            layer_children: vec![], envelopes: Default::default(), rack: Default::default(),
        };
        let project = os_model::Project { ppq: 96, tempo: Some(130.0), channels: vec![ch], ..Default::default() };
        let mut lib = os_engine::SampleLibrary::default();
        let mut e = os_engine::Engine::new(&project, &mut lib, 48000, os_engine::PlayMode::Pattern(1));
        e.clear_schedule();
        e.set_instrument(0, shared);
        let mut out = Vec::new();
        let block = 1440; // 30 ms, what a WASAPI callback typically asks for
        for b in 0..(48000 * 12 / block) {
            if b == 3 {
                e.note_on(0, 60, 100);
            }
            if b == 40 {
                e.note_off(0, 60); // key released after ~1.2 s
            }
            let mut buf = vec![0.0f32; block * 2];
            if !e.finished() {
                e.process(&mut buf);
            }
            out.extend(buf.chunks_exact(2).map(|f| f[0]));
        }
        let window = |from_s: f32, to_s: f32| &out[(from_s * 48000.0) as usize..(to_s * 48000.0) as usize];
        let peak = |w: &[f32]| w.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        let crossings = |w: &[f32]| w.windows(2).filter(|p| p[0] <= 0.0 && p[1] > 0.0).count() as f32 / (w.len() as f32 / 48000.0);
        let f = crossings(window(0.5, 1.0));
        eprintln!("frequency {f:.1} Hz; peaks 0.5-1 s {:.3}, 6-7 s {:.3}, 9-10 s {:.3}", peak(window(0.5, 1.0)), peak(window(6.0, 7.0)), peak(window(9.0, 10.0)));
        assert!((f - 441.0).abs() < 4.0, "in tune: {f} Hz");
        assert!(peak(window(6.0, 7.0)) > 0.1, "still playing after the 5 s idle tail");
        assert!(peak(window(9.0, 10.0)) > 0.1, "still playing at 9 s");
    }

    /// One note_on through the engine (as the typing keyboard plays it) must fire the sample exactly once.
    #[test]
    fn one_note_on_fires_the_sample_once() {
        let Some(lib) = open_sampler() else { return };
        let mut p = Plugin::new(&lib, &lib.plugins[0].id).unwrap();
        // A single-unit impulse then silence: every voice start is one spike.
        let wav = std::env::temp_dir().join("os-clap-impulse.wav");
        let mut frame = vec![0.0f32; 48000 * 2];
        frame[0] = 1.0;
        frame[1] = 1.0;
        os_engine::wav::write_f32_stereo(&mut std::fs::File::create(&wav).unwrap(), 48000, &frame).unwrap();
        let state = format!("{{\"version\":\"0.0.1\",\"params\":{{}},\"fields\":{{\"sample-path\":{}}}}}", serde_string(&wav.to_string_lossy()));
        let mut blob = (state.len() as u64).to_le_bytes().to_vec();
        blob.extend_from_slice(state.as_bytes());
        assert!(p.load_state(&blob));
        assert!(p.activate(48000));
        let shared: os_engine::SharedInstrument = Arc::new(Mutex::new(p));
        let ch = os_model::Channel {
            index: 0, kind: os_model::ChannelKind::Plugin, name: None, plugin: Some("Open Sampler".into()), color: None,
            enabled: true, volume: 0.78125, pan: 0.0, pitch_cents: 0, mixer_track: 0, sample_path: None,
            layer_children: vec![], envelopes: Default::default(), rack: Default::default(),
        };
        let project = os_model::Project { ppq: 96, tempo: Some(130.0), channels: vec![ch], ..Default::default() };
        let mut lib = os_engine::SampleLibrary::default();
        let mut e = os_engine::Engine::new(&project, &mut lib, 48000, os_engine::PlayMode::Pattern(1));
        e.clear_schedule();
        e.set_instrument(0, shared);
        let mut out = Vec::new();
        let block = 1440;
        for b in 0..30 {
            if b == 1 {
                e.note_on(0, 60, 100);
            }
            let mut buf = vec![0.0f32; block * 2];
            if !e.finished() {
                e.process(&mut buf);
            }
            out.extend(buf.chunks_exact(2).map(|f| f[0]));
        }
        // One spike per voice start (> 0.05 after at least 100 quiet frames).
        let (mut starts, mut quiet) = (0, 1000);
        for v in &out {
            if v.abs() > 0.05 {
                if quiet > 100 {
                    starts += 1;
                }
                quiet = 0;
            } else {
                quiet += 1;
            }
        }
        assert_eq!(starts, 1, "one note_on fired the sample {starts} times");
    }

    /// JSON string literal holding `s` as a JSON string (nih-plug stores persisted fields as serialised JSON).
    /// Every sound control of Open Sampler must change what a note sounds like (state-set parameters, rendered here).
    #[test]
    fn open_sampler_parameters_change_the_sound() {
        let Some(lib) = open_sampler() else { return };
        // A 2 s decaying saw-ish tone at 44.1 kHz, stereo with different channels.
        let wav = std::env::temp_dir().join("os-clap-params.wav");
        let n = 44100 * 2;
        let tone: Vec<f32> = (0..n)
            .flat_map(|i| {
                let t = i as f32 / 44100.0;
                let ph = (t * 220.0).fract();
                let e = (-t * 1.5).exp() * 0.5;
                [(ph * 2.0 - 1.0) * e + 0.05, (t * 330.0 * std::f32::consts::TAU).sin() * e]
            })
            .collect();
        os_engine::wav::write_f32_stereo(&mut std::fs::File::create(&wav).unwrap(), 44100, &tone).unwrap();
        let path = serde_string(&wav.to_string_lossy());
        let render = |params: &str| -> Vec<f32> {
            let mut p = Plugin::new(&lib, &lib.plugins[0].id).unwrap();
            let state = format!("{{\"version\":\"0.0.1\",\"params\":{{{params}}},\"fields\":{{\"sample-path\":{path}}}}}");
            let mut blob = (state.len() as u64).to_le_bytes().to_vec();
            blob.extend_from_slice(state.as_bytes());
            assert!(p.load_state(&blob));
            assert!(p.activate(48000));
            let mut out = Vec::new();
            for b in 0..150 {
                // Precomputed effects run on a background task: give it time before the note.
                if b < 20 {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                if b == 20 {
                    p.note_on(0, 60, 100.0 / 128.0);
                }
                if b == 60 {
                    p.note_off(0, 60);
                }
                let (mut l, mut r) = (vec![0.0f32; 480], vec![0.0f32; 480]);
                p.process(48000, &mut l, &mut r);
                out.extend(l.iter().zip(&r).flat_map(|(a, b)| [*a, *b]));
            }
            out
        };
        let base = render("");
        assert!(base.iter().any(|v| v.abs() > 0.05), "the default plays");
        let cases = [
            ("start_offset", "{\"f32\":0.5}"), ("ts_pitch", "{\"f32\":700.0}"), ("ts_mul", "{\"f32\":2.0}"),
            ("fx_dc", "{\"bool\":true}"), ("fx_normalize", "{\"bool\":true}"), ("fx_reverse", "{\"bool\":true}"),
            ("fx_polarity", "{\"bool\":true}"), ("fx_fade_stereo", "{\"bool\":true}"), ("fx_swap", "{\"bool\":true}"),
            ("fx_start", "{\"f32\":0.3}"), ("fx_length", "{\"f32\":0.3}"), ("fx_in", "{\"f32\":0.5}"), ("fx_out", "{\"f32\":0.5}"),
            ("fx_boost", "{\"f32\":0.5}"), ("fx_eq", "{\"f32\":0.8}"), ("fx_rm_mix", "{\"f32\":0.8}"), ("fx_cut", "{\"f32\":0.3}"),
            ("fx_reverb", "{\"f32\":0.8}"), ("fx_sdelay", "{\"f32\":0.5}"), ("fx_pogo", "{\"f32\":0.8}"),
            ("f_cut", "{\"f32\":0.3}"), ("fine", "{\"i32\":50}"), ("root", "{\"i32\":55}"),
            ("envvol_on", "{\"bool\":true}"), ("envpitch_on", "{\"bool\":true},\"envpitch_amt\":{\"f32\":0.5}"),
            ("envpan_lfo_amt", "{\"f32\":0.8}"), ("envvol_lfo_amt", "{\"f32\":0.8}"), ("envpitch_lfo_amt", "{\"f32\":0.5}"),
            ("envmodx_lfo_amt", "{\"f32\":-0.8}"), ("f_type", "{\"i32\":3}"),
        ];
        let mut dead = Vec::new();
        for (id, v) in cases {
            let out = render(&format!("\"{id}\":{v}"));
            let diff = base.iter().zip(&out).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
            eprintln!("{id:>16} = {v:<16} max diff {diff:.4}");
            if diff < 1e-3 {
                dead.push(id);
            }
        }
        assert!(dead.is_empty(), "no change in the sound: {dead:?}");
    }

    fn serde_string(s: &str) -> String {
        let inner = format!("\"{}\"", s.replace('\\', "\\\\"));
        format!("\"{}\"", inner.replace('\\', "\\\\").replace('"', "\\\""))
    }
}
