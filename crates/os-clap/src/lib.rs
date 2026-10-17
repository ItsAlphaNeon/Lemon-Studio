// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Studio's CLAP host: loads `.clap` libraries, creates instances, plays them as engine instruments
//! (`os_engine::Instrument`), saves / loads their state and shows their editors.
//!
//! Threading: an instance is shared as `Arc<Mutex<Plugin>>`. The UI thread creates it, activates it, opens its
//! editor, loads / saves state and calls `idle` every tick; the audio thread only calls `Instrument` methods through
//! the engine (with `try_lock`).
// spec: plugins/plugin-formats.md (hosting inside Lemon Studio)

use std::ffi::{c_char, c_void, CStr, CString};
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use clap_sys::audio_buffer::clap_audio_buffer;
use clap_sys::entry::clap_plugin_entry;
use clap_sys::events::{
    clap_event_header, clap_event_note, clap_event_note_expression, clap_event_param_gesture, clap_event_param_mod, clap_event_param_value,
    clap_event_transport, clap_input_events, clap_output_events, CLAP_CORE_EVENT_SPACE_ID, CLAP_EVENT_NOTE_CHOKE, CLAP_EVENT_NOTE_EXPRESSION,
    CLAP_EVENT_NOTE_END, CLAP_EVENT_NOTE_OFF, CLAP_EVENT_NOTE_ON, CLAP_NOTE_EXPRESSION_PAN, CLAP_NOTE_EXPRESSION_TUNING, CLAP_NOTE_EXPRESSION_VOLUME, CLAP_EVENT_PARAM_GESTURE_BEGIN, CLAP_EVENT_PARAM_GESTURE_END,
    CLAP_EVENT_PARAM_MOD, CLAP_EVENT_PARAM_VALUE, CLAP_EVENT_TRANSPORT, CLAP_TRANSPORT_HAS_BEATS_TIMELINE, CLAP_TRANSPORT_HAS_SECONDS_TIMELINE,
    CLAP_TRANSPORT_HAS_TEMPO, CLAP_TRANSPORT_HAS_TIME_SIGNATURE, CLAP_TRANSPORT_IS_LOOP_ACTIVE, CLAP_TRANSPORT_IS_PLAYING,
};
use clap_sys::fixedpoint::{CLAP_BEATTIME_FACTOR, CLAP_SECTIME_FACTOR};
use clap_sys::ext::audio_ports::{clap_audio_port_info, clap_plugin_audio_ports, CLAP_EXT_AUDIO_PORTS};
use clap_sys::ext::gui::{clap_host_gui, clap_plugin_gui, clap_window, clap_window_handle, CLAP_EXT_GUI, CLAP_WINDOW_API_COCOA, CLAP_WINDOW_API_WIN32, CLAP_WINDOW_API_X11};
use clap_sys::ext::latency::{clap_plugin_latency, CLAP_EXT_LATENCY};
use clap_sys::ext::params::{
    clap_host_params, clap_param_info, clap_plugin_params, CLAP_EXT_PARAMS, CLAP_PARAM_IS_AUTOMATABLE, CLAP_PARAM_IS_BYPASS,
    CLAP_PARAM_IS_ENUM, CLAP_PARAM_IS_HIDDEN, CLAP_PARAM_IS_MODULATABLE_PER_NOTE_ID, CLAP_PARAM_IS_READONLY, CLAP_PARAM_IS_STEPPED,
};
use clap_sys::ext::render::{clap_plugin_render, CLAP_EXT_RENDER, CLAP_RENDER_OFFLINE, CLAP_RENDER_REALTIME};
use clap_sys::ext::state::{clap_plugin_state, CLAP_EXT_STATE};
use clap_sys::ext::voice_info::{clap_host_voice_info, clap_plugin_voice_info, clap_voice_info, CLAP_EXT_VOICE_INFO};
use clap_sys::factory::plugin_factory::{clap_plugin_factory, CLAP_PLUGIN_FACTORY_ID};
use clap_sys::host::clap_host;
use clap_sys::plugin::clap_plugin;
use clap_sys::process::{clap_process, CLAP_PROCESS_CONTINUE, CLAP_PROCESS_CONTINUE_IF_NOT_QUIET, CLAP_PROCESS_TAIL};
use clap_sys::stream::{clap_istream, clap_ostream};
use clap_sys::version::{clap_version_is_compatible, CLAP_VERSION};
use os_engine::voices::VoiceLedger;

mod context_menu;
mod params;

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
    /// Loads plugin `path`: a `.clap` file, or on macOS a `.clap` bundle (its `init` gets the bundle's path).
    pub fn load(path: &Path) -> Result<Arc<Library>, String> {
        unsafe {
            let lib = os_host::load_module(&module_file(path))?;
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
        // A plugin file whose code crashed is left loaded, as the crash left it.
        let guard = os_host::guard::module_of(self.entry.cast());
        if guard.crashed().is_none() {
            let entry = self.entry;
            let _ = os_host::guard::call(&guard, || unsafe {
                if let Some(f) = (*entry).deinit {
                    f();
                }
            });
        }
        if guard.crashed().is_some() {
            std::mem::forget(self._lib.take());
        }
    }
}

/// The window system editors are embedded in: Win32 on Windows, Cocoa on macOS, X11 elsewhere (under Wayland too,
/// through XWayland: plugins do not embed in Wayland surfaces).
const WINDOW_API: &CStr = if cfg!(windows) {
    CLAP_WINDOW_API_WIN32
} else if cfg!(target_os = "macos") {
    CLAP_WINDOW_API_COCOA
} else {
    CLAP_WINDOW_API_X11
};

/// A host window as CLAP passes it for [`WINDOW_API`]: the handle itself, or on X11 the window id it carries.
fn window_handle(parent: *mut c_void) -> clap_window_handle {
    if cfg!(windows) {
        clap_window_handle { win32: parent }
    } else if cfg!(target_os = "macos") {
        clap_window_handle { cocoa: parent }
    } else {
        clap_window_handle { x11: parent as usize as std::ffi::c_ulong }
    }
}

/// The standard CLAP folders, per user then system (CLAP's `entry.h`), and the folders in `CLAP_PATH`.
pub fn standard_dirs() -> Vec<PathBuf> {
    let mut v = Vec::new();
    #[cfg(windows)]
    {
        if let Some(l) = std::env::var_os("LOCALAPPDATA") {
            v.push(PathBuf::from(l).join("Programs").join("Common").join("CLAP"));
        }
        if let Some(c) = std::env::var_os("COMMONPROGRAMFILES") {
            v.push(PathBuf::from(c).join("CLAP"));
        }
    }
    #[cfg(target_os = "macos")]
    {
        if let Some(h) = std::env::var_os("HOME") {
            v.push(PathBuf::from(h).join("Library/Audio/Plug-Ins/CLAP"));
        }
        v.push(PathBuf::from("/Library/Audio/Plug-Ins/CLAP"));
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if let Some(h) = std::env::var_os("HOME") {
            v.push(PathBuf::from(h).join(".clap"));
        }
        v.push(PathBuf::from("/usr/lib/clap"));
        v.push(PathBuf::from("/usr/local/lib/clap"));
    }
    if let Some(extra) = std::env::var_os("CLAP_PATH") {
        v.extend(std::env::split_paths(&extra).filter(|p| p.is_absolute()));
    }
    v
}

/// Whether `p` is a CLAP plugin: a `.clap` file, or on macOS a `.clap` bundle folder (`Name.clap/Contents/MacOS/...`).
pub fn is_plugin(p: &Path) -> bool {
    p.extension().is_some_and(|x| x.eq_ignore_ascii_case("clap")) && (p.is_file() || (cfg!(target_os = "macos") && p.is_dir()))
}

/// The module file of plugin `path`: on macOS a bundle's executable (`os_host::bundle`), else the file itself.
pub fn module_file(path: &Path) -> PathBuf {
    #[cfg(target_os = "macos")]
    return os_host::bundle::module_file(path);
    #[cfg(not(target_os = "macos"))]
    path.to_path_buf()
}

/// CLAP plugins directly inside the given folders (and in their subfolders; a macOS bundle is not searched inside).
pub fn find_files(dirs: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for d in dirs {
        let Ok(rd) = std::fs::read_dir(d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if is_plugin(&p) {
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
        // A plugin compiled into this program gets a guard of its own per instance (`module_of`).
        let guard = os_host::guard::module_of(self.entry.cast());
        let name = self.plugins.iter().find(|d| d.id == id).map_or(id, |d| d.name.as_str());
        let plugin = os_host::guard::call(&guard, || Plugin::new(&self, id)).map_err(|c| os_host::guard::stopped_text(name, c))??;
        Ok(os_host::share(plugin, guard))
    }
}

/// Host-side state the plugin's callbacks reach through `clap_host::host_data`.
struct HostData {
    /// Raised with every request below and when the audio thread leaves work for the UI thread.
    attention: Arc<os_host::Attention>,
    callback: AtomicBool,
    restart: AtomicBool,
    /// The plugin asked for a parameter flush (`clap_host_params.request_flush`): an edit from its editor waits in its
    /// output queue and only takes effect when the host flushes (or processes). The host's waker
    /// (`os_host::set_waker`) makes `idle` run at once instead of on the next timer tick.
    flush: AtomicBool,
    /// `clap_host_params.rescan` flags not handled yet (the parameter list changed).
    rescan: AtomicU32,
    /// Parameter ids picked for "Create automation clip" in the plugin's own menus (`context_menu.rs`).
    automate: Mutex<Vec<u32>>,
    /// The editor asked for this size (`clap_host_gui.request_resize`, in its own units: device px, points on macOS);
    /// applied on the UI tick.
    resize: Mutex<Option<(u32, u32)>>,
    /// The plugin's voice count changed (`clap_host_voice_info.changed`): read again on the UI tick.
    voice_info: AtomicBool,
    /// The plugin asked to be processed (`clap_host.request_process`), e.g. a note from its editor's keyboard: a
    /// sleeping instance runs the next block.
    process: AtomicBool,
    /// The plugin, for the run loop's callbacks (set before `init`, which may already register).
    #[cfg(all(unix, not(target_os = "macos")))]
    plugin: std::sync::atomic::AtomicPtr<clap_plugin>,
    /// What the plugin registered with the UI run loop (Linux): timer id or descriptor, whether a timer, token.
    #[cfg(all(unix, not(target_os = "macos")))]
    run: Mutex<Vec<(u32, bool, u64)>>,
}

static HOST_NAME: &CStr = c"Lemon Studio";
static HOST_URL: &CStr = c"";
static HOST_VERSION: &CStr = c"0.0.1";

unsafe extern "C" fn host_get_extension(_host: *const clap_host, id: *const c_char) -> *const c_void {
    if id.is_null() {
        return ptr::null();
    }
    let id = unsafe { CStr::from_ptr(id) };
    if id == CLAP_EXT_PARAMS {
        return &HOST_PARAMS as *const clap_host_params as *const c_void;
    }
    if id == CLAP_EXT_GUI {
        return &HOST_GUI as *const clap_host_gui as *const c_void;
    }
    if id == CLAP_EXT_VOICE_INFO {
        return &HOST_VOICE_INFO as *const clap_host_voice_info as *const c_void;
    }
    if id == clap_sys::ext::context_menu::CLAP_EXT_CONTEXT_MENU || id == clap_sys::ext::context_menu::CLAP_EXT_CONTEXT_MENU_COMPAT {
        return &context_menu::HOST_CONTEXT_MENU as *const clap_sys::ext::context_menu::clap_host_context_menu as *const c_void;
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    if let Some(ext) = run_loop::extension(id) {
        return ext;
    }
    ptr::null()
}

/// Linux: plugin editors have no message loop of their own; they register timers and descriptors (their X11
/// connection) with the host (`timer-support`, `posix-fd-support`) and get the callbacks on the UI thread through
/// `os_host::runloop`. Not offered on macOS: editors there run on the main thread's run loop, which the app's event
/// loop (winit's) runs.
#[cfg(all(unix, not(target_os = "macos")))]
mod run_loop {
    use super::*;
    use clap_sys::ext::posix_fd_support::{clap_host_posix_fd_support, clap_plugin_posix_fd_support, clap_posix_fd_flags, CLAP_EXT_POSIX_FD_SUPPORT};
    use clap_sys::ext::timer_support::{clap_host_timer_support, clap_plugin_timer_support, CLAP_EXT_TIMER_SUPPORT};
    use clap_sys::id::clap_id;
    use os_host::runloop;

    pub(super) fn extension(id: &CStr) -> Option<*const c_void> {
        if id == CLAP_EXT_TIMER_SUPPORT {
            return Some(&HOST_TIMER as *const clap_host_timer_support as *const c_void);
        }
        if id == CLAP_EXT_POSIX_FD_SUPPORT {
            return Some(&HOST_POSIX_FD as *const clap_host_posix_fd_support as *const c_void);
        }
        None
    }

    static HOST_TIMER: clap_host_timer_support = clap_host_timer_support { register_timer: Some(register_timer), unregister_timer: Some(unregister_timer) };
    static HOST_POSIX_FD: clap_host_posix_fd_support =
        clap_host_posix_fd_support { register_fd: Some(register_fd), modify_fd: Some(modify_fd), unregister_fd: Some(unregister_fd) };

    /// Timer ids the host hands out.
    static NEXT_TIMER: AtomicU32 = AtomicU32::new(1);

    unsafe fn data<'a>(host: *const clap_host) -> &'a HostData {
        unsafe { &*((*host).host_data as *const HostData) }
    }

    /// The plugin and its extension `id`, once it is created.
    unsafe fn plugin_ext<T>(d: &HostData, id: &CStr) -> Option<(*const clap_plugin, *const T)> {
        let plugin = d.plugin.load(Ordering::Acquire) as *const clap_plugin;
        if plugin.is_null() {
            return None;
        }
        let ext = unsafe { (*plugin).get_extension.map_or(ptr::null(), |g| g(plugin, id.as_ptr())) } as *const T;
        (!ext.is_null()).then_some((plugin, ext))
    }

    unsafe extern "C" fn register_timer(host: *const clap_host, period_ms: u32, timer_id: *mut clap_id) -> bool {
        let d = unsafe { data(host) };
        let Some((plugin, ext)) = (unsafe { plugin_ext::<clap_plugin_timer_support>(d, CLAP_EXT_TIMER_SUPPORT) }) else { return false };
        let Some(on_timer) = (unsafe { (*ext).on_timer }) else { return false };
        if timer_id.is_null() {
            return false;
        }
        let id = NEXT_TIMER.fetch_add(1, Ordering::Relaxed);
        let token = runloop::add_timer(std::time::Duration::from_millis(u64::from(period_ms)), move || unsafe { on_timer(plugin, id) });
        if let Ok(mut r) = d.run.lock() {
            r.push((id, true, token));
        }
        unsafe { *timer_id = id };
        true
    }

    unsafe extern "C" fn unregister_timer(host: *const clap_host, timer_id: clap_id) -> bool {
        remove(unsafe { data(host) }, timer_id, true)
    }

    unsafe extern "C" fn register_fd(host: *const clap_host, fd: i32, flags: clap_posix_fd_flags) -> bool {
        let d = unsafe { data(host) };
        let Some((plugin, ext)) = (unsafe { plugin_ext::<clap_plugin_posix_fd_support>(d, CLAP_EXT_POSIX_FD_SUPPORT) }) else { return false };
        let Some(on_fd) = (unsafe { (*ext).on_fd }) else { return false };
        // The run loop's flags are CLAP's.
        let token = runloop::add_fd(fd, flags, move |fd, got| unsafe { on_fd(plugin, fd, got) });
        if let Ok(mut r) = d.run.lock() {
            r.push((fd as u32, false, token));
        }
        true
    }

    unsafe extern "C" fn modify_fd(host: *const clap_host, fd: i32, flags: clap_posix_fd_flags) -> bool {
        let d = unsafe { data(host) };
        let Ok(r) = d.run.lock() else { return false };
        r.iter().find(|e| e.0 == fd as u32 && !e.1).is_some_and(|e| runloop::modify_fd(e.2, flags))
    }

    unsafe extern "C" fn unregister_fd(host: *const clap_host, fd: i32) -> bool {
        remove(unsafe { data(host) }, fd as u32, false)
    }

    fn remove(d: &HostData, key: u32, timer: bool) -> bool {
        let Ok(mut r) = d.run.lock() else { return false };
        let before = r.len();
        r.retain(|&(k, t, token)| {
            let hit = k == key && t == timer;
            if hit {
                runloop::remove(token);
            }
            !hit
        });
        before != r.len()
    }

    /// Removes whatever the plugin left registered (after it was destroyed: its callbacks must never run again).
    pub(super) fn clear(d: &HostData) {
        if let Ok(mut r) = d.run.lock() {
            for (_, _, token) in r.drain(..) {
                runloop::remove(token);
            }
        }
    }
}
unsafe extern "C" fn host_params_rescan(host: *const clap_host, flags: u32) {
    unsafe { (*((*host).host_data as *const HostData)).rescan.fetch_or(flags, Ordering::AcqRel) };
    unsafe { attend(host) };
}
unsafe extern "C" fn host_params_clear(_host: *const clap_host, _id: u32, _flags: u32) {}
unsafe extern "C" fn host_params_request_flush(host: *const clap_host) {
    unsafe { (*((*host).host_data as *const HostData)).flush.store(true, Ordering::Release) };
    unsafe { attend(host) };
}
/// The editor's requests to the host window. A resize is applied on the UI tick (`Plugin::idle`): the plugin gets its
/// size and the frame follows `editor_size`. The editor is always embedded, so show / hide / closed do not apply.
static HOST_GUI: clap_host_gui = clap_host_gui {
    resize_hints_changed: Some(host_gui_hints),
    request_resize: Some(host_gui_request_resize),
    request_show: Some(host_gui_show_hide),
    request_hide: Some(host_gui_show_hide),
    closed: Some(host_gui_closed),
};
unsafe extern "C" fn host_gui_hints(_host: *const clap_host) {}
unsafe extern "C" fn host_gui_request_resize(host: *const clap_host, width: u32, height: u32) -> bool {
    if let Ok(mut r) = unsafe { (*((*host).host_data as *const HostData)).resize.lock() } {
        *r = Some((width, height));
    }
    unsafe { attend(host) };
    true
}
unsafe extern "C" fn host_gui_show_hide(_host: *const clap_host) -> bool {
    false
}
unsafe extern "C" fn host_gui_closed(_host: *const clap_host, _was_destroyed: bool) {}
static HOST_PARAMS: clap_host_params =
    clap_host_params { rescan: Some(host_params_rescan), clear: Some(host_params_clear), request_flush: Some(host_params_request_flush) };
/// The plugin's voice count changed (a patch with another polyphony): the voice list's limit is read again on the UI
/// tick (spec: audio/voices.md §2.7).
static HOST_VOICE_INFO: clap_host_voice_info = clap_host_voice_info { changed: Some(host_voice_info_changed) };
unsafe extern "C" fn host_voice_info_changed(host: *const clap_host) {
    unsafe { (*((*host).host_data as *const HostData)).voice_info.store(true, Ordering::Release) };
    unsafe { attend(host) };
}
unsafe extern "C" fn host_request_restart(host: *const clap_host) {
    unsafe { (*((*host).host_data as *const HostData)).restart.store(true, Ordering::Release) };
    unsafe { attend(host) };
}
/// Below this level a block is quiet (CLAP's CONTINUE_IF_NOT_QUIET; about -100 dB).
const QUIET: f32 = 1e-5;

unsafe extern "C" fn host_request_process(host: *const clap_host) {
    unsafe { (*((*host).host_data as *const HostData)).process.store(true, Ordering::Release) };
}
unsafe extern "C" fn host_request_callback(host: *const clap_host) {
    unsafe { (*((*host).host_data as *const HostData)).callback.store(true, Ordering::Release) };
    unsafe { attend(host) };
}

/// The plugin left work for the UI thread: its attention count goes up and the UI wakes.
pub(crate) unsafe fn attend(host: *const clap_host) {
    unsafe { (*((*host).host_data as *const HostData)).attention.raise() };
    wake();
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    On,
    Off,
    Choke,
    /// Tuning of a key's voices (note expression); `velocity` holds the semitones.
    Tune,
    /// A named voice's gain (note expression); `velocity` holds the gain.
    Gain,
}

/// One entry of a chunk's event order: an index into `notes`, `mods` or `exprs`.
#[derive(Clone, Copy)]
enum Seq {
    Note(usize),
    Mod(usize),
    Expr(usize),
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
    /// The note's own pan and mod X / Y (`use_note_levels`).
    levels: os_engine::NoteLevels,
    /// The engine's name for the voice (`Instrument::voice_start`): its note gets a note id, which its gains and its
    /// choke go to.
    voice: Option<u32>,
}

impl Ev {
    fn new(time: u32, kind: Kind, key: u8, velocity: f32) -> Ev {
        Ev { time, kind, key, velocity, start: 0.0, levels: os_engine::NoteLevels::default(), voice: None }
    }
}

/// Named voices kept for their gains and chokes; the oldest go beyond this.
const NAMED_VOICES: usize = 512;

/// How an instance takes a note's own levels (`Plugin::use_note_levels`): the pan always (note expression), the mod X /
/// mod Y as per-note modulation of these parameters (id, the span of its range) where it has them.
#[derive(Clone, Copy, Default)]
struct NoteParams {
    mods: [Option<(u32, f64)>; 2],
}

/// A voice started with note levels: its note id and key, the note's mod X / Y, the modulation amounts it was sent.
#[derive(Clone, Copy)]
struct LevelVoice {
    id: i32,
    key: u8,
    mods: [f32; 2],
    sent: [f64; 2],
}

/// Voices kept for resending their mod amounts; the oldest go beyond this.
const LEVEL_VOICES: usize = 512;

/// A per-note modulation event of `param` by `amount` for note `id` on `key` at `time`.
fn param_mod(time: u32, param: u32, id: i32, key: u8, amount: f64) -> clap_event_param_mod {
    clap_event_param_mod {
        header: clap_event_header {
            size: std::mem::size_of::<clap_event_param_mod>() as u32,
            time,
            space_id: CLAP_CORE_EVENT_SPACE_ID,
            type_: CLAP_EVENT_PARAM_MOD,
            flags: 0,
        },
        param_id: param,
        cookie: ptr::null_mut(),
        note_id: id,
        port_index: 0,
        channel: 0,
        key: i16::from(key),
        amount,
    }
}

/// A plugin instance.
pub struct Plugin {
    pub desc: Descriptor,
    plugin: *const clap_plugin,
    gui: *const clap_plugin_gui,
    state: *const clap_plugin_state,
    params: *const clap_plugin_params,
    latency: *const clap_plugin_latency,
    /// The render extension (offline or realtime), if the plugin has one.
    render: *const clap_plugin_render,
    offline: bool,
    /// The voice-info extension (the plugin's polyphony), if it has one.
    voice_info: *const clap_plugin_voice_info,
    /// The voices it plays, from the notes sent and the voice ends it reports (spec: audio/voices.md §2).
    ledger: VoiceLedger,
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
    exprs: Vec<clap_event_note_expression>,
    /// This chunk's events in time order (pointers into `notes` / `mods`).
    order: Vec<*const clap_event_header>,
    /// Parameter that takes a note's sample start as per-note modulation (`use_start_param`).
    start_param: Option<u32>,
    /// Notes carry their own pan and mod X / Y (`use_note_levels`).
    note_levels: Option<NoteParams>,
    /// The channel's mod X / Y, added to each note's (`Instrument::channel_levels`).
    chan_mods: [f32; 2],
    /// Voices started with note levels, until the plugin reports their end.
    level_voices: Vec<LevelVoice>,
    /// Mod amounts of sounding voices to send at the start of the next block (note id, key, parameter, amount).
    mod_resend: Vec<(i32, u8, u32, f64)>,
    /// Voices get their gains as volume note expressions (`use_voice_gains`).
    voice_gains: bool,
    /// The voices the engine named (`Instrument::voice_start`): its name, the note id they were sent with, the key.
    named: Vec<(u32, i32, u8)>,
    next_note_id: i32,
    steady: i64,
    /// The song position for the next process call (the engine gives it while playing), and the current chunk's.
    song: Option<os_engine::TransportInfo>,
    chunk_song: Option<os_engine::TransportInfo>,
    /// The transport event of the process call being made.
    transport_ev: clap_event_transport,
    /// The plugin's last process status says it still has sound to deliver (`Instrument::active`).
    sounding: bool,
    dbg_peak: f32,
    dbg_frames: u64,
    /// The editor is open (parented into a host window).
    editor: bool,
    /// Device pixels per unit of the open editor's sizes: 1, except on macOS, where editors measure in points
    /// (`open_editor`).
    editor_scale: f32,
    /// Input copies for effect processing (mixer slots).
    fx_in: (Vec<f32>, Vec<f32>),
    /// The audio-ports extension, if the plugin has one.
    audio_ports: *const clap_plugin_audio_ports,
    /// Channels of the main input and output ports, read at activation: 0 without such a port. A plugin without the
    /// audio-ports extension is taken as stereo in and out.
    ins: usize,
    outs: usize,
    /// Buffers for channels the host has no audio for (a mono input's mix of both sides, channels beyond two), and the
    /// channel pointers of the current process call: sized at activation, so the audio thread never allocates.
    scratch: Vec<f32>,
    in_ptrs: Vec<*mut f32>,
    out_ptrs: Vec<*mut f32>,
    /// Parameter ranges in list order (id, min, max, stepped), read after init and on a rescan (values cross the
    /// hosting layer normalized).
    ranges: Vec<params::Range>,
    /// Host parameter changes (id, plain value) for the next process call or flush.
    param_queue: Vec<(u32, f64)>,
    /// Automation values (frame offset, id, plain value) for the next process call (`params.rs`).
    auto: Vec<(u32, u32, f64)>,
    /// A process call ran since the last `idle`.
    processed: bool,
    /// This chunk's parameter events (sent before the notes, at time 0).
    pvals: Vec<clap_event_param_value>,
    /// Changes the plugin reported (its editor): gesture begin / end and values, plain.
    param_out: Vec<OutEv>,
    _lib: Arc<Library>,
}

/// A parameter event the plugin sent the host, or a voice end (key, note id; taken right after the call).
#[derive(Clone, Copy)]
enum OutEv {
    Begin(u32),
    Value(u32, f64),
    End(u32),
    NoteEnd(i16, i32),
}

/// Most plugin -> host parameter events kept until the app takes them.
const MAX_OUT: usize = 4096;
/// Room for voice ends on top of a full parameter queue (one call's worth; they never stay in the queue).
const MAX_ENDS: usize = 4096;

/// `clap_input_events` over the `Vec<*const clap_event_header>` behind `ctx`.
unsafe extern "C" fn events_size(l: *const clap_input_events) -> u32 {
    unsafe { (*((*l).ctx as *const Vec<*const clap_event_header>)).len() as u32 }
}

unsafe extern "C" fn events_get(l: *const clap_input_events, i: u32) -> *const clap_event_header {
    let v = unsafe { &*((*l).ctx as *const Vec<*const clap_event_header>) };
    v.get(i as usize).copied().unwrap_or(ptr::null())
}

/// `clap_output_events.try_push` collecting parameter events and voice ends into the `Vec<OutEv>` behind `ctx`.
unsafe extern "C" fn push_out(l: *const clap_output_events, e: *const clap_event_header) -> bool {
    unsafe {
        let v = &mut *((*l).ctx as *mut Vec<OutEv>);
        if e.is_null() || (*e).space_id != CLAP_CORE_EVENT_SPACE_ID {
            return true;
        }
        if (*e).type_ == CLAP_EVENT_NOTE_END {
            if v.len() < MAX_OUT + MAX_ENDS {
                let n = &*(e as *const clap_event_note);
                v.push(OutEv::NoteEnd(n.key, n.note_id));
            }
            return true;
        }
        if v.len() >= MAX_OUT {
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
        let host_data = Box::new(HostData {
            attention: Arc::default(),
            callback: AtomicBool::new(false),
            restart: AtomicBool::new(false),
            flush: AtomicBool::new(false),
            rescan: AtomicU32::new(0),
            automate: Mutex::new(Vec::new()),
            resize: Mutex::new(None),
            voice_info: AtomicBool::new(false),
            process: AtomicBool::new(false),
            #[cfg(all(unix, not(target_os = "macos")))]
            plugin: std::sync::atomic::AtomicPtr::new(ptr::null_mut()),
            #[cfg(all(unix, not(target_os = "macos")))]
            run: Mutex::new(Vec::new()),
        });
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
            #[cfg(all(unix, not(target_os = "macos")))]
            host_data.plugin.store(plugin as *mut clap_plugin, Ordering::Release);
            if !(*plugin).init.is_some_and(|i| i(plugin)) {
                if let Some(d) = (*plugin).destroy {
                    d(plugin);
                }
                #[cfg(all(unix, not(target_os = "macos")))]
                run_loop::clear(&host_data);
                return Err(format!("{id}: init failed"));
            }
            let ext = |name: &CStr| (*plugin).get_extension.map_or(ptr::null(), |g| g(plugin, name.as_ptr()));
            let gui = ext(CLAP_EXT_GUI) as *const clap_plugin_gui;
            let state = ext(CLAP_EXT_STATE) as *const clap_plugin_state;
            let params = ext(CLAP_EXT_PARAMS) as *const clap_plugin_params;
            let latency = ext(CLAP_EXT_LATENCY) as *const clap_plugin_latency;
            let render = ext(CLAP_EXT_RENDER) as *const clap_plugin_render;
            let voice_info = ext(CLAP_EXT_VOICE_INFO) as *const clap_plugin_voice_info;
            let audio_ports = ext(CLAP_EXT_AUDIO_PORTS) as *const clap_plugin_audio_ports;
            let ranges = params::read_ranges(params, plugin);
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
                exprs: Vec::with_capacity(64),
                order: Vec::with_capacity(320),
                start_param: None,
                note_levels: None,
                chan_mods: [0.0; 2],
                level_voices: Vec::new(),
                mod_resend: Vec::new(),
                voice_gains: false,
                named: Vec::new(),
                next_note_id: 0,
                steady: 0,
                song: None,
                chunk_song: None,
                transport_ev: transport_event(&os_engine::TransportInfo {
                    playing: false,
                    tempo: 120.0,
                    frame: 0,
                    beats: 0.0,
                    bar_start: 0.0,
                    bar: 0,
                    numerator: 4,
                    denominator: 4,
                    looping: None,
                }),
                sounding: false,
                dbg_peak: 0.0,
                dbg_frames: 0,
                editor: false,
                editor_scale: 1.0,
                fx_in: (Vec::new(), Vec::new()),
                audio_ports,
                ins: 2,
                outs: 2,
                scratch: Vec::new(),
                in_ptrs: Vec::new(),
                out_ptrs: Vec::new(),
                latency,
                render,
                offline: false,
                voice_info,
                ledger: VoiceLedger::new(),
                ranges,
                param_queue: Vec::new(),
                auto: Vec::with_capacity(params::MAX_AUTO),
                processed: false,
                pvals: Vec::with_capacity(params::MAX_AUTO + 64),
                param_out: Vec::new(),
                _lib: lib.clone(),
            })
        }
    }

    /// Switches the plugin between offline rendering (an export) and live playback through the CLAP render extension
    /// (UI thread). An active instance is reactivated around the switch, since plugins may read the mode when they
    /// are activated. False when the plugin has no such switch or refused it.
    pub fn set_offline(&mut self, offline: bool) -> bool {
        if self.render.is_null() || self.offline == offline {
            return false;
        }
        let rate = self.rate;
        self.deactivate();
        let mode = if offline { CLAP_RENDER_OFFLINE } else { CLAP_RENDER_REALTIME };
        let ok = unsafe { (*self.render).set.is_some_and(|s| s(self.plugin, mode)) };
        if ok {
            self.offline = offline;
        }
        if rate != 0 {
            self.activate(rate);
        }
        ok
    }

    /// Activates at `rate` (UI thread); reactivates when the rate changes.
    pub fn activate(&mut self, rate: u32) -> bool {
        if self.rate == rate {
            return true;
        }
        self.deactivate();
        self.read_ports();
        let ok = unsafe { (*self.plugin).activate.is_some_and(|a| a(self.plugin, f64::from(rate), 1, self.max_frames)) };
        if ok {
            self.rate = rate;
            self.read_voice_info();
        }
        ok
    }

    /// The channels of the plugin's main input and output ports (main thread, inactive: ports only change then), and
    /// buffers for them. Without the audio-ports extension the plugin is taken as stereo in and out.
    fn read_ports(&mut self) {
        let main = |is_input: bool| -> Option<usize> {
            let ports = unsafe { self.audio_ports.as_ref() }?;
            if unsafe { ports.count.map_or(0, |c| c(self.plugin, is_input)) } == 0 {
                return Some(0);
            }
            // SAFETY: plain data the plugin fills.
            let mut info: clap_audio_port_info = unsafe { std::mem::zeroed() };
            let ok = unsafe { ports.get.is_some_and(|g| g(self.plugin, 0, is_input, &mut info)) };
            Some(if ok { (info.channel_count as usize).min(64) } else { 2 })
        };
        (self.ins, self.outs) = (main(true).unwrap_or(2), main(false).unwrap_or(2));
        self.scratch = vec![0.0; (self.ins + self.outs) * self.max_frames as usize];
        self.in_ptrs = Vec::with_capacity(self.ins);
        self.out_ptrs = Vec::with_capacity(self.outs);
    }

    /// The voice list's limit from the plugin's voice info (main thread, active): the voices its patch can use.
    fn read_voice_info(&mut self) {
        if self.voice_info.is_null() {
            return;
        }
        let mut info = clap_voice_info { voice_count: 0, voice_capacity: 0, flags: 0 };
        let ok = unsafe { (*self.voice_info).get.is_some_and(|g| g(self.plugin, &mut info)) };
        let n = if info.voice_count > 0 { info.voice_count } else { info.voice_capacity };
        self.ledger.set_capacity(ok.then_some(n as usize));
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
        self.ledger.clear();
        self.level_voices.clear();
        self.mod_resend.clear();
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
        let before = self.param_out.len();
        unsafe {
            if let Some(f) = (*self.params).flush {
                f(self.plugin, &ins, &outs);
            }
        }
        self.take_note_ends(before);
        self.order.clear();
    }

    /// Moves the voice ends the plugin reported since `param_out` held `from` events into the voice list, keeping the
    /// parameter events for the app (spec: audio/voices.md §2.4).
    fn take_note_ends(&mut self, from: usize) {
        let mut w = from;
        for r in from..self.param_out.len() {
            match self.param_out[r] {
                OutEv::NoteEnd(key, id) => {
                    self.ledger.ended(key, id);
                    if id >= 0 {
                        self.level_voices.retain(|v| v.id != id);
                        self.named.retain(|v| v.1 != id);
                    }
                }
                e => {
                    self.param_out[w] = e;
                    w += 1;
                }
            }
        }
        self.param_out.truncate(w);
        if w > from {
            self.host_data.attention.raise();
        }
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
        self.ranges.iter().find(|r| r.0 == id).map(|&(_, lo, hi, _)| (lo, hi))
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
    /// `name`, which must be modulatable per note id (UI thread; spec: plugins/lemon-sampler.md §7). False if the
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

    /// Sends each note's own levels (`Instrument::note_start`; UI thread): its pan as a CLAP pan note expression
    /// `(pan + 1) / 2` right after the note on, with the note's id, and its mod X / Y plus the channel's as per-note
    /// modulation of the parameters named `os_host::NOTE_MOD_X` / `NOTE_MOD_Y` when the plugin has them modulatable
    /// per note id, the amount being the mod value times the parameter's range (an OpenStudio choice: a mod of 1 is
    /// the knob's whole range). Sounding voices get the new amounts when the channel's mods change.
    pub fn use_note_levels(&mut self) -> bool {
        let mut np = NoteParams::default();
        if !self.params.is_null() {
            unsafe {
                let count = (*self.params).count.map_or(0, |c| c(self.plugin));
                for i in 0..count {
                    let mut info: clap_param_info = std::mem::zeroed();
                    if !(*self.params).get_info.is_some_and(|g| g(self.plugin, i, &mut info)) {
                        continue;
                    }
                    if info.flags & CLAP_PARAM_IS_MODULATABLE_PER_NOTE_ID == 0 {
                        continue;
                    }
                    let n = CStr::from_ptr(info.name.as_ptr()).to_string_lossy();
                    let k = if n == os_host::NOTE_MOD_X {
                        0
                    } else if n == os_host::NOTE_MOD_Y {
                        1
                    } else {
                        continue;
                    };
                    np.mods[k] = Some((info.id, info.max_value - info.min_value));
                }
            }
        }
        self.note_levels = Some(np);
        true
    }

    /// Sends the gains the engine gives audio clip voices (`Instrument::voice_gain`; UI thread) as CLAP volume note
    /// expressions with the voice's note id: for Lemon Sampler, which takes the first at once and glides to each later
    /// one over `os_engine::instrument::VOICE_GAIN_FRAMES` frames (a Lemon Studio choice: CLAP says nothing of how a
    /// plugin moves to a new volume). Without it the engine puts the clips' fades on the instance's whole output.
    pub fn use_voice_gains(&mut self) -> bool {
        self.voice_gains = true;
        true
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
        // The editor asked for a new size: it gets it (its own adjustment wins), and the frame follows `editor_size`.
        let resize = self.host_data.resize.lock().ok().and_then(|mut r| r.take());
        if let (Some((w, h)), true) = (resize, self.editor) {
            unsafe {
                let g = &*self.gui;
                let (mut w, mut h) = (w, h);
                g.adjust_size.map(|f| f(self.plugin, &mut w, &mut h));
                g.set_size.map(|f| f(self.plugin, w, h));
            }
        }
        self.idle_params();
        if self.host_data.flush.swap(false, Ordering::AcqRel) || !self.param_queue.is_empty() {
            self.flush_params();
        }
        if self.host_data.voice_info.swap(false, Ordering::AcqRel) && self.rate != 0 {
            self.read_voice_info();
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
        !self.gui.is_null() && unsafe { (*self.gui).is_api_supported.is_some_and(|f| f(self.plugin, WINDOW_API.as_ptr(), false)) }
    }

    pub fn editor_open(&self) -> bool {
        self.editor
    }

    /// Embeds the plugin's editor in `parent` (a host window handle: HWND on Windows, an NSView on macOS, an X11 window
    /// id on Linux); returns its size in pixels.
    /// The host draws the window frame around it (spec: ui/plugin-window.md). `scale` is the parent's DPI factor:
    /// a plugin that takes it draws at that size and reports the scaled size (spec: ui/ui-scaling.md §3). On macOS the
    /// editor measures in points and follows its window's backing scale itself (CLAP: no `set_scale` with Cocoa), so
    /// its sizes are turned into pixels at `scale` here, and back for the plugin.
    pub fn open_editor(&mut self, parent: *mut c_void, scale: f32) -> Result<(u32, u32), String> {
        if !self.has_editor() {
            return Err(format!("{} has no editor", self.desc.name));
        }
        self.close_editor();
        self.editor_scale = if cfg!(target_os = "macos") && scale > 0.0 { scale } else { 1.0 };
        unsafe {
            let g = &*self.gui;
            let api = WINDOW_API.as_ptr();
            if !g.create.is_some_and(|c| c(self.plugin, api, false)) {
                return Err("the plugin could not create its editor".into());
            }
            // CLAP: the scale goes between create and set_parent; a plugin that refuses it handles DPI itself.
            if scale > 0.0 && !cfg!(target_os = "macos") {
                g.set_scale.map(|f| f(self.plugin, f64::from(scale)));
            }
            let (mut w, mut h) = (0u32, 0u32);
            g.get_size.map(|f| f(self.plugin, &mut w, &mut h));
            let cw = clap_window { api, specific: window_handle(parent) };
            if !g.set_parent.is_some_and(|f| f(self.plugin, &cw)) {
                g.destroy.map(|d| d(self.plugin));
                return Err("the plugin refused the editor window".into());
            }
            g.show.map(|f| f(self.plugin));
            self.editor = true;
            Ok(self.to_px((w, h)))
        }
    }

    /// Editor size in pixels (when open).
    pub fn editor_size(&self) -> Option<(u32, u32)> {
        if !self.editor {
            return None;
        }
        let (mut w, mut h) = (0u32, 0u32);
        unsafe { (*self.gui).get_size.is_some_and(|f| f(self.plugin, &mut w, &mut h)) }.then(|| self.to_px((w, h)))
    }

    /// An editor size as the plugin gives it (points on macOS) in pixels.
    fn to_px(&self, (w, h): (u32, u32)) -> (u32, u32) {
        let px = |v: u32| (v as f32 * self.editor_scale).round() as u32;
        (px(w), px(h))
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

    /// The plugin reported that it is done (no CONTINUE / TAIL, and quiet output with CONTINUE_IF_NOT_QUIET) and
    /// nothing is queued for it: no notes, no parameter changes, no automation, no request to be processed. CLAP lets
    /// the host stop calling such an instance until something arrives (or, for an effect, until its input is not
    /// quiet), so an idle instrument or effect costs nothing per block. One with its editor open stays awake: notes
    /// from the editor's own keyboard reach it without the host, and not every plugin asks to be processed for them.
    fn asleep(&self) -> bool {
        !self.sounding
            && !self.editor
            && !self.host_data.process.load(Ordering::Acquire)
            && self.queue.is_empty() && self.param_queue.is_empty() && self.auto.is_empty() && self.mod_resend.is_empty()
    }

    /// `process_chunk` with an audio input (effects): `input` holds the frames to process, `left` / `right` receive
    /// the output.
    fn process_chunk_in(&mut self, left: &mut [f32], right: &mut [f32], input: Option<(&mut [f32], &mut [f32])>) {
        let n = left.len() as u32;
        // This block answers a request to be processed; one made while it runs asks for the next.
        self.host_data.process.store(false, Ordering::Release);
        self.notes.clear();
        self.mods.clear();
        self.exprs.clear();
        let mut rest = Vec::new();
        // Event order; a note's modulation follows its note on at the same time.
        let mut seq: Vec<Seq> = Vec::new();
        // Sounding voices' new mod amounts (the channel's mods changed) go first.
        for (id, key, param, amount) in self.mod_resend.drain(..) {
            self.mods.push(param_mod(0, param, id, key, amount));
            seq.push(Seq::Mod(self.mods.len() - 1));
        }
        for e in self.queue.drain(..) {
            if e.time < n && e.kind == Kind::Tune {
                // Portamento glides (spec: audio/channel-voices.md): CLAP tuning expression, semitones.
                self.exprs.push(clap_event_note_expression {
                    header: clap_event_header {
                        size: std::mem::size_of::<clap_event_note_expression>() as u32,
                        time: e.time,
                        space_id: CLAP_CORE_EVENT_SPACE_ID,
                        type_: CLAP_EVENT_NOTE_EXPRESSION,
                        flags: 0,
                    },
                    expression_id: CLAP_NOTE_EXPRESSION_TUNING,
                    note_id: -1,
                    port_index: 0,
                    channel: 0,
                    key: i16::from(e.key),
                    value: f64::from(e.velocity),
                });
                seq.push(Seq::Expr(self.exprs.len() - 1));
            } else if e.time < n && e.kind == Kind::Gain {
                // A named voice's gain (`use_voice_gains`): CLAP volume expression with its note id; a voice that has
                // ended is not sent one.
                let Some(&(_, id, key)) = self.named.iter().find(|v| Some(v.0) == e.voice) else { continue };
                self.exprs.push(clap_event_note_expression {
                    header: clap_event_header {
                        size: std::mem::size_of::<clap_event_note_expression>() as u32,
                        time: e.time,
                        space_id: CLAP_CORE_EVENT_SPACE_ID,
                        type_: CLAP_EVENT_NOTE_EXPRESSION,
                        flags: 0,
                    },
                    expression_id: CLAP_NOTE_EXPRESSION_VOLUME,
                    note_id: id,
                    port_index: 0,
                    channel: 0,
                    key: i16::from(key),
                    value: f64::from(e.velocity).clamp(0.0, 4.0),
                });
                seq.push(Seq::Expr(self.exprs.len() - 1));
            } else if e.time < n {
                // A note with a sample start or its own levels gets an id, so its modulation reaches only that voice;
                // so does a voice the engine named, whose gains and choke go to it alone. Its choke takes that id.
                let fresh = e.kind == Kind::On && ((self.start_param.is_some() && e.start > 0.0) || self.note_levels.is_some() || e.voice.is_some());
                let id = if fresh {
                    self.next_note_id = self.next_note_id.wrapping_add(1) & 0x7fff_ffff;
                    self.next_note_id
                } else if e.kind == Kind::Choke {
                    let at = self.named.iter().position(|v| Some(v.0) == e.voice);
                    at.map_or(-1, |i| self.named.remove(i).1)
                } else {
                    -1
                };
                if let (Kind::On, Some(v)) = (e.kind, e.voice) {
                    if self.named.len() >= NAMED_VOICES {
                        self.named.remove(0);
                    }
                    self.named.push((v, id, e.key));
                }
                match e.kind {
                    Kind::On => self.ledger.start(e.key, id),
                    Kind::Off => self.ledger.release(e.key),
                    Kind::Choke if id >= 0 => self.ledger.choke_voice(id),
                    Kind::Choke | Kind::Tune | Kind::Gain => self.ledger.choke(e.key),
                }
                self.notes.push(clap_event_note {
                    header: clap_event_header {
                        size: std::mem::size_of::<clap_event_note>() as u32,
                        time: e.time,
                        space_id: CLAP_CORE_EVENT_SPACE_ID,
                        type_: match e.kind {
                            Kind::On => CLAP_EVENT_NOTE_ON,
                            Kind::Off => CLAP_EVENT_NOTE_OFF,
                            Kind::Choke | Kind::Tune | Kind::Gain => CLAP_EVENT_NOTE_CHOKE,
                        },
                        flags: 0,
                    },
                    note_id: id,
                    port_index: 0,
                    channel: 0,
                    key: i16::from(e.key),
                    velocity: f64::from(e.velocity),
                });
                seq.push(Seq::Note(self.notes.len() - 1));
                if let (Some(param), true) = (self.start_param, id >= 0 && e.start > 0.0) {
                    self.mods.push(param_mod(e.time, param, id, e.key, f64::from(e.start)));
                    seq.push(Seq::Mod(self.mods.len() - 1));
                }
                // The note's own levels (spec: audio/voice-levels.md "The note's own levels"): its pan as the voice's
                // pan expression, its mod X / Y plus the channel's as per-note modulation.
                if let (Some(np), true) = (self.note_levels, id >= 0) {
                    let pan = e.levels.pan;
                    if pan != 0.0 {
                        self.exprs.push(clap_event_note_expression {
                            header: clap_event_header {
                                size: std::mem::size_of::<clap_event_note_expression>() as u32,
                                time: e.time,
                                space_id: CLAP_CORE_EVENT_SPACE_ID,
                                type_: CLAP_EVENT_NOTE_EXPRESSION,
                                flags: 0,
                            },
                            expression_id: CLAP_NOTE_EXPRESSION_PAN,
                            note_id: id,
                            port_index: 0,
                            channel: 0,
                            key: i16::from(e.key),
                            value: f64::from((pan + 1.0) * 0.5).clamp(0.0, 1.0),
                        });
                        seq.push(Seq::Expr(self.exprs.len() - 1));
                    }
                    let own = [e.levels.mod_x, e.levels.mod_y];
                    let mut sent = [0.0; 2];
                    for k in 0..2 {
                        let Some((param, span)) = np.mods[k] else { continue };
                        sent[k] = f64::from(own[k] + self.chan_mods[k]) * span;
                        if sent[k] != 0.0 {
                            self.mods.push(param_mod(e.time, param, id, e.key, sent[k]));
                            seq.push(Seq::Mod(self.mods.len() - 1));
                        }
                    }
                    if np.mods.iter().any(Option::is_some) {
                        if self.level_voices.len() >= LEVEL_VOICES {
                            self.level_voices.remove(0);
                        }
                        self.level_voices.push(LevelVoice { id, key: e.key, mods: own, sent });
                    }
                }
            } else {
                rest.push(Ev { time: e.time - n, ..e });
            }
        }
        self.queue = rest;
        // `queue` is sorted by time, so `seq` already is. Host parameter changes go first, at time 0, then automation
        // in time order; parameter values go before the notes at the same frame.
        self.fill_param_events();
        self.take_automation(n);
        self.order.clear();
        let mut pi = 0;
        for &s in &seq {
            let h = match s {
                Seq::Mod(m) => &self.mods[m].header as *const clap_event_header,
                Seq::Note(i) => &self.notes[i].header as *const clap_event_header,
                Seq::Expr(x) => &self.exprs[x].header as *const clap_event_header,
            };
            let t = unsafe { (*h).time };
            while pi < self.pvals.len() && self.pvals[pi].header.time <= t {
                self.order.push(&self.pvals[pi].header as *const clap_event_header);
                pi += 1;
            }
            self.order.push(h);
        }
        self.order.extend(self.pvals[pi..].iter().map(|e| &e.header as *const clap_event_header));
        self.processed = true;
        let ins = clap_input_events { ctx: &self.order as *const Vec<*const clap_event_header> as *mut c_void, size: Some(events_size), get: Some(events_get) };
        let outs = clap_output_events { ctx: &mut self.param_out as *mut Vec<OutEv> as *mut c_void, try_push: Some(push_out) };
        // Channels as the plugin's main ports have them: the host's two sides, a mono input their average, others
        // scratch (zeroed inputs, unheard outputs).
        let frames = n as usize;
        let max = self.max_frames as usize;
        let base = self.scratch.as_mut_ptr();
        // SAFETY: scratch holds `max_frames` frames for every input and output channel.
        let spare = |k: usize| unsafe {
            let s = base.add(k * max);
            std::ptr::write_bytes(s, 0, frames);
            s
        };
        self.out_ptrs.clear();
        for c in 0..self.outs {
            self.out_ptrs.push(match c {
                0 => left.as_mut_ptr(),
                1 => right.as_mut_ptr(),
                _ => spare(self.ins + c),
            });
        }
        self.in_ptrs.clear();
        let has_in = input.is_some() && self.ins > 0;
        if let Some((l, r)) = input.filter(|_| self.ins > 0) {
            if self.ins == 1 {
                let s = spare(0);
                // SAFETY: `spare` gave `frames` zeroed frames.
                let mono = unsafe { std::slice::from_raw_parts_mut(s, frames) };
                mono.iter_mut().zip(l.iter().zip(r.iter())).for_each(|(m, (a, b))| *m = (a + b) * 0.5);
                self.in_ptrs.push(s);
            } else {
                self.in_ptrs.push(l.as_mut_ptr());
                self.in_ptrs.push(r.as_mut_ptr());
                for c in 2..self.ins {
                    self.in_ptrs.push(spare(c));
                }
            }
        }
        let mut buf = clap_audio_buffer { data32: self.out_ptrs.as_mut_ptr(), data64: ptr::null_mut(), channel_count: self.outs as u32, latency: 0, constant_mask: 0 };
        let in_buf = clap_audio_buffer { data32: self.in_ptrs.as_mut_ptr(), data64: ptr::null_mut(), channel_count: self.ins as u32, latency: 0, constant_mask: 0 };
        // The engine's transport while playing; without one the host is free running (spec: plugins/vst3-hosting.md §1).
        let transport = match self.chunk_song {
            Some(t) => {
                self.transport_ev = transport_event(&t);
                &self.transport_ev as *const clap_event_transport
            }
            None => ptr::null(),
        };
        let p = clap_process {
            steady_time: self.steady,
            frames_count: n,
            transport,
            audio_inputs: if has_in { &in_buf } else { ptr::null() },
            audio_outputs: if self.outs > 0 { &mut buf } else { ptr::null_mut() },
            audio_inputs_count: u32::from(has_in),
            audio_outputs_count: u32::from(self.outs > 0),
            in_events: &ins,
            out_events: &outs,
        };
        let before = self.param_out.len();
        let status = unsafe { (*self.plugin).process.map_or(CLAP_PROCESS_CONTINUE, |f| f(self.plugin, &p)) };
        // A mono output is heard on both sides.
        if self.outs == 1 {
            right.copy_from_slice(left);
        }
        // CONTINUE / TAIL: the plugin says it is still going; CONTINUE_IF_NOT_QUIET: while its output is not silent.
        let loud = left.iter().chain(right.iter()).any(|v| v.abs() > QUIET);
        self.sounding = status == CLAP_PROCESS_CONTINUE || status == CLAP_PROCESS_TAIL || (status == CLAP_PROCESS_CONTINUE_IF_NOT_QUIET && loud);
        self.steady += i64::from(n);
        // Voices: the ends it reported, none once it sleeps, released voices ringing out (spec: audio/voices.md §2).
        self.take_note_ends(before);
        if !self.sounding {
            self.ledger.quiet(true);
        }
        self.ledger.advance(f64::from(n) / f64::from(self.rate.max(1)));
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

    fn open_editor(&mut self, parent: *mut c_void, scale: f32) -> Result<(u32, u32), String> {
        Plugin::open_editor(self, parent, scale)
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

    fn use_note_levels(&mut self) -> bool {
        Plugin::use_note_levels(self)
    }

    fn use_voice_gains(&mut self) -> bool {
        Plugin::use_voice_gains(self)
    }

    fn set_offline(&mut self, offline: bool) {
        Plugin::set_offline(self, offline);
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
        self.host_data.attention.raise();
        os_host::wake();
        true
    }

    fn take_automation_requests(&mut self) -> Vec<u32> {
        self.automation_requests()
    }

    fn take_param_events(&mut self) -> Vec<os_host::ParamEvent> {
        let raw = std::mem::take(&mut self.param_out);
        raw.into_iter()
            .filter_map(|e| match e {
                OutEv::Begin(id) => Some(os_host::ParamEvent::Begin(id)),
                OutEv::Value(id, v) => Some(os_host::ParamEvent::Value(id, self.normalize(id, v))),
                OutEv::End(id) => Some(os_host::ParamEvent::End(id)),
                OutEv::NoteEnd(..) => None,
            })
            .collect()
    }

    fn attention(&self) -> Option<Arc<os_host::Attention>> {
        Some(self.host_data.attention.clone())
    }
}

impl os_engine::Instrument for Plugin {
    fn active(&self) -> bool {
        self.sounding
    }

    fn voices(&self) -> usize {
        self.ledger.count()
    }

    fn reset(&mut self) {
        self.queue.clear();
        self.sounding = false;
        self.ledger.clear();
        self.level_voices.clear();
        self.mod_resend.clear();
        self.named.clear();
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
        self.queue.push(Ev::new(offset, Kind::On, key, velocity));
    }

    fn note_off(&mut self, offset: u32, key: u8) {
        self.queue.push(Ev::new(offset, Kind::Off, key, 0.0));
    }

    fn note_on_from(&mut self, offset: u32, key: u8, velocity: f32, start: f32) {
        self.queue.push(Ev { start: start.clamp(0.0, 1.0), ..Ev::new(offset, Kind::On, key, velocity) });
    }

    fn note_start(&mut self, offset: u32, key: u8, velocity: f32, start: f32, levels: os_engine::NoteLevels) {
        self.queue.push(Ev { start: start.clamp(0.0, 1.0), levels, ..Ev::new(offset, Kind::On, key, velocity) });
    }

    fn choke(&mut self, offset: u32, key: u8) {
        self.queue.push(Ev::new(offset, Kind::Choke, key, 0.0));
    }

    fn voice_start(&mut self, offset: u32, key: u8, velocity: f32, start: f32, levels: os_engine::NoteLevels, voice: u32) {
        self.queue.push(Ev { start: start.clamp(0.0, 1.0), levels, voice: Some(voice), ..Ev::new(offset, Kind::On, key, velocity) });
    }

    fn voice_gains(&self) -> bool {
        self.voice_gains
    }

    fn voice_gain(&mut self, offset: u32, voice: u32, gain: f32) {
        if self.voice_gains {
            self.queue.push(Ev { voice: Some(voice), ..Ev::new(offset, Kind::Gain, 0, gain) });
        }
    }

    fn voice_choke(&mut self, offset: u32, key: u8, voice: u32) {
        self.queue.push(Ev { voice: Some(voice), ..Ev::new(offset, Kind::Choke, key, 0.0) });
    }

    fn tune(&mut self, offset: u32, key: u8, semitones: f32) {
        self.queue.push(Ev::new(offset, Kind::Tune, key, semitones));
    }

    /// The engine still applies the channel's volume and pan to the output (false); the channel's mod X / Y join each
    /// note's in its modulation, and sounding voices are sent their new amounts (spec: audio/voice-levels.md "Changes
    /// while a note sounds").
    fn channel_levels(&mut self, levels: os_engine::ChannelLevels) -> bool {
        let mods = [levels.mod_x, levels.mod_y];
        if mods == self.chan_mods {
            return false;
        }
        self.chan_mods = mods;
        let Some(np) = self.note_levels else { return false };
        for v in &mut self.level_voices {
            for k in 0..2 {
                let Some((param, span)) = np.mods[k] else { continue };
                let amount = f64::from(v.mods[k] + mods[k]) * span;
                if amount != v.sent[k] {
                    v.sent[k] = amount;
                    self.mod_resend.push((v.id, v.key, param, amount));
                }
            }
        }
        false
    }

    fn param(&mut self, offset: u32, index: u32, value: f64) {
        self.automate(offset, index, value);
    }

    fn transport(&mut self, info: &os_engine::TransportInfo) {
        self.song = Some(*info);
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
            self.host_data.attention.raise();
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
        if self.asleep() {
            self.song = None;
            self.steady += left.len() as i64;
            return;
        }
        self.queue.sort_by_key(|e| e.time);
        let song = self.song.take();
        let max = self.max_frames as usize;
        let mut at = 0;
        while at < left.len() {
            let n = (left.len() - at).min(max);
            self.chunk_song = song.map(|s| s.advanced(at as u64, rate));
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
            self.host_data.attention.raise();
            return;
        }
        if !self.processing {
            self.processing = unsafe { (*self.plugin).start_processing.is_none_or(|s| s(self.plugin)) };
            if !self.processing {
                return;
            }
        }
        // Quiet input to a sleeping effect passes through as it is.
        if self.asleep() && left.iter().chain(right.iter()).all(|v| v.abs() <= QUIET) {
            self.song = None;
            self.steady += left.len() as i64;
            return;
        }
        let (mut il, mut ir) = std::mem::take(&mut self.fx_in);
        let song = self.song.take();
        let max = self.max_frames as usize;
        let mut at = 0;
        while at < left.len() {
            let n = (left.len() - at).min(max);
            self.chunk_song = song.map(|s| s.advanced(at as u64, rate));
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

    fn param_fx(&mut self, offset: u32, index: u32, value: f64) {
        self.automate(offset, index, value);
    }

    fn transport_fx(&mut self, info: &os_engine::TransportInfo) {
        self.song = Some(*info);
    }
}

/// The CLAP transport event of an engine transport (spec: plugins/vst3-hosting.md §1): tempo, beat and seconds
/// timelines, time signature, bar start and number, playing, the loop.
fn transport_event(t: &os_engine::TransportInfo) -> clap_event_transport {
    let beats = |b: f64| (b * CLAP_BEATTIME_FACTOR as f64).round() as i64;
    let secs = |b: f64| (b * 60.0 / t.tempo.max(1.0) * CLAP_SECTIME_FACTOR as f64).round() as i64;
    let mut flags = CLAP_TRANSPORT_HAS_TEMPO | CLAP_TRANSPORT_HAS_BEATS_TIMELINE | CLAP_TRANSPORT_HAS_SECONDS_TIMELINE | CLAP_TRANSPORT_HAS_TIME_SIGNATURE;
    if t.playing {
        flags |= CLAP_TRANSPORT_IS_PLAYING;
    }
    if t.looping.is_some() {
        flags |= CLAP_TRANSPORT_IS_LOOP_ACTIVE;
    }
    let (ls, le) = t.looping.unwrap_or((0.0, 0.0));
    clap_event_transport {
        header: clap_event_header {
            size: std::mem::size_of::<clap_event_transport>() as u32,
            time: 0,
            space_id: CLAP_CORE_EVENT_SPACE_ID,
            type_: CLAP_EVENT_TRANSPORT,
            flags: 0,
        },
        flags,
        song_pos_beats: beats(t.beats),
        song_pos_seconds: secs(t.beats),
        tempo: t.tempo,
        tempo_inc: 0.0,
        loop_start_beats: beats(ls),
        loop_end_beats: beats(le),
        loop_start_seconds: secs(ls),
        loop_end_seconds: secs(le),
        bar_start: beats(t.bar_start),
        bar_number: t.bar,
        tsig_num: t.numerator,
        tsig_denom: t.denominator,
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
        #[cfg(all(unix, not(target_os = "macos")))]
        run_loop::clear(&self.host_data);
    }
}

#[cfg(test)]
mod mono_mock;
#[cfg(test)]
mod tests {
    use super::*;
    use os_engine::Instrument;

    /// A plugin whose main ports are mono (DPF's Kars has a mono output) gets one channel each way: its output is
    /// heard on both sides, and an effect's input reaches it as the average of both sides. Without the audio-ports
    /// extension a plugin still gets two.
    #[test]
    fn mono_ports_get_one_channel() {
        use crate::mono_mock::{ENTRY, SEEN_IN, SEEN_OUT};
        use os_engine::HostedEffect as _;
        use std::sync::atomic::Ordering::Relaxed;
        let entry = &ENTRY as *const _ as *const std::ffi::c_void;
        let lib = unsafe { Library::from_entry(entry, Path::new("Mono Test.clap")) }.expect("mock");
        let mut p = Plugin::new(&lib, &lib.plugins[0].id).unwrap();
        assert!(os_host::Plugin::activate(&mut p, 48000));
        assert_eq!((p.ins, p.outs), (1, 1));
        // As an effect: left 1, right 0 reach it as 0.5; its output (0.5 * 0.5 + 0.25) is heard on both sides.
        let (mut l, mut r) = (vec![1.0f32; 64], vec![0.0f32; 64]);
        p.process_fx(48000, &mut l, &mut r);
        assert_eq!((SEEN_IN.load(Relaxed), SEEN_OUT.load(Relaxed)), (1, 1));
        assert!(l.iter().chain(&r).all(|v| (v - 0.5).abs() < 1e-6), "{:?} {:?}", &l[..2], &r[..2]);
        // As an instrument: no input, the one output on both sides.
        let (mut l, mut r) = (vec![0.0f32; 64], vec![0.0f32; 64]);
        p.process(48000, &mut l, &mut r);
        assert_eq!((SEEN_IN.load(Relaxed), SEEN_OUT.load(Relaxed)), (0, 1));
        assert!(l.iter().chain(&r).all(|v| (v - 0.25).abs() < 1e-6));
        // A plugin without the extension (taken away from the mock here) is stereo in and out.
        p.audio_ports = ptr::null();
        p.read_ports();
        assert_eq!((p.ins, p.outs), (2, 2));
    }

    /// Loads the bundled Lemon Sampler (built by `cargo run -p bundle-plugins`) when it is there.
    fn lemon_sampler() -> Option<Arc<Library>> {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        // This test's own build folder (target*/debug/deps/x.exe -> target*), then the usual ones.
        let own = std::env::current_exe().ok().and_then(|e| Some(e.parent()?.parent()?.parent()?.to_path_buf()));
        for t in own.into_iter().chain(["target-dev", "target"].map(|t| root.join(t))) {
            let p = t.join("bundled").join("Lemon Sampler.clap");
            if p.exists() {
                return Some(Library::load(&p).expect("load"));
            }
        }
        None
    }

    /// The built-in Lemon Parametric EQ, linked into the test binary.
    fn lemon_eq() -> os_host::SharedPlugin {
        let entry = &lemon_parametric_eq_2::LINKED_CLAP_ENTRY as *const _ as *const std::ffi::c_void;
        let lib = unsafe { Library::from_entry(entry, Path::new("Lemon Parametric EQ.clap")) }.expect("linked EQ");
        os_host::Library::create(lib.clone() as Arc<dyn os_host::Library>, &lib.plugins[0].id).unwrap()
    }

    /// A note's own levels (`use_note_levels`): the pan as a pan note expression with the note's id right after its
    /// note on, the mods (plus the channel's) as per-note modulation of the mod parameters; when the channel's mods
    /// change, the sounding voice is sent its new amount. Without `use_note_levels` nothing of it is sent.
    #[test]
    fn note_levels_go_with_the_note() {
        use os_engine::HostedEffect as _;
        let entry = &lemon_parametric_eq_2::LINKED_CLAP_ENTRY as *const _ as *const std::ffi::c_void;
        let lib = unsafe { Library::from_entry(entry, Path::new("Lemon Parametric EQ.clap")) }.expect("linked EQ");
        let mut p = Plugin::new(&lib, &lib.plugins[0].id).unwrap();
        assert!(os_host::Plugin::activate(&mut p, 48000));
        let (mut l, mut r) = (vec![0.0f32; 64], vec![0.0f32; 64]);
        let own = os_engine::NoteLevels { pan: 0.5, mod_x: 0.25, mod_y: -0.5 };
        // Off: a plain note.
        p.note_start(3, 60, 0.8, 0.0, own);
        p.process_fx(48000, &mut l, &mut r);
        assert_eq!((p.notes.len(), p.notes[0].note_id, p.exprs.len(), p.mods.len()), (1, -1, 0, 0));
        // On; the EQ has no mod parameters, so this test lends it one (its first parameter, range 0..1 plain).
        assert!(p.use_note_levels());
        assert!(p.note_levels.is_some_and(|n| n.mods == [None, None]));
        let param = os_host::Plugin::param_info(&p, 0).unwrap().id;
        p.note_levels = Some(NoteParams { mods: [Some((param, 2.0)), None] });
        p.channel_levels(os_engine::ChannelLevels { mod_x: 0.25, ..Default::default() });
        p.note_start(5, 61, 0.8, 0.0, own);
        p.process_fx(48000, &mut l, &mut r);
        let id = p.notes[0].note_id;
        assert!(id >= 0);
        assert_eq!(p.exprs.len(), 1);
        let x = p.exprs[0];
        assert_eq!((x.expression_id, x.note_id, x.key, x.header.time, x.value), (CLAP_NOTE_EXPRESSION_PAN, id, 61, 5, 0.75));
        assert_eq!(p.mods.len(), 1);
        let m = p.mods[0];
        assert_eq!((m.param_id, m.note_id, m.header.time, m.amount), (param, id, 5, (0.25 + 0.25) * 2.0));
        // In the event order the note on comes first.
        assert_eq!(p.order.first().copied(), Some(&p.notes[0].header as *const clap_event_header));
        // The channel's mod changes: the sounding voice gets its new amount at the next block's start.
        p.channel_levels(os_engine::ChannelLevels { mod_x: -0.25, ..Default::default() });
        p.channel_levels(os_engine::ChannelLevels { mod_x: -0.25, ..Default::default() });
        p.process_fx(48000, &mut l, &mut r);
        assert_eq!(p.mods.len(), 1);
        assert_eq!((p.mods[0].note_id, p.mods[0].header.time, p.mods[0].amount), (id, 0, 0.0));
        // The voice ends: nothing more to resend.
        p.param_out.push(OutEv::NoteEnd(61, id));
        p.take_note_ends(0);
        p.channel_levels(os_engine::ChannelLevels { mod_x: 0.5, ..Default::default() });
        assert!(p.mod_resend.is_empty());
    }

    /// The UI locks an instance only when it has work (spec: audio/implementing.md "Playing live"): host edits waiting
    /// for a flush, a stream at another rate and the plugin's own requests raise its attention count; an idle with
    /// nothing left to do leaves it alone, so a quiet instance is not locked again and again.
    #[test]
    fn work_for_the_ui_thread_raises_the_attention_count() {
        let shared = lemon_eq();
        let mut p = shared.lock().unwrap();
        let a = p.attention().expect("a CLAP instance has a count");
        assert!(p.activate(48000));
        p.idle();
        let quiet = a.count();
        p.idle();
        assert_eq!(a.count(), quiet, "nothing to do");
        let id = p.param_info(0).unwrap().id;
        assert!(p.set_param(id, 0.25));
        assert!(a.count() > quiet, "an edit waits for the flush");
        p.idle();
        let flushed = a.count();
        p.idle();
        assert_eq!(a.count(), flushed, "flushed: nothing left");
        let (mut l, mut r) = (vec![0.0f32; 64], vec![0.0f32; 64]);
        p.process_fx(44100, &mut l, &mut r);
        assert!(a.count() > flushed, "another rate: the UI thread reactivates it");
    }

    // spec: plugins/vst3-hosting.md §1 (transport)
    #[test]
    fn transport_events_follow_the_engine() {
        let song = os_engine::TransportInfo {
            playing: true,
            tempo: 120.0,
            frame: 120000,
            beats: 5.0,
            bar_start: 4.0,
            bar: 1,
            numerator: 3,
            denominator: 4,
            looping: Some((0.0, 8.0)),
        };
        let e = transport_event(&song);
        assert_eq!((e.header.type_, e.header.space_id, e.header.time), (CLAP_EVENT_TRANSPORT, CLAP_CORE_EVENT_SPACE_ID, 0));
        assert_eq!(e.header.size as usize, std::mem::size_of::<clap_event_transport>());
        let all = CLAP_TRANSPORT_HAS_TEMPO
            | CLAP_TRANSPORT_HAS_BEATS_TIMELINE
            | CLAP_TRANSPORT_HAS_SECONDS_TIMELINE
            | CLAP_TRANSPORT_HAS_TIME_SIGNATURE
            | CLAP_TRANSPORT_IS_PLAYING
            | CLAP_TRANSPORT_IS_LOOP_ACTIVE;
        assert_eq!(e.flags, all);
        // Beats and seconds are fixed point (2^31 per unit); 5 beats at 120 BPM = 2.5 s.
        assert_eq!((e.song_pos_beats, e.song_pos_seconds), (5 * CLAP_BEATTIME_FACTOR, 5 * CLAP_SECTIME_FACTOR / 2));
        assert_eq!((e.loop_start_beats, e.loop_end_beats, e.loop_end_seconds), (0, 8 * CLAP_BEATTIME_FACTOR, 4 * CLAP_SECTIME_FACTOR));
        assert_eq!((e.bar_start, e.bar_number, e.tsig_num, e.tsig_denom), (4 * CLAP_BEATTIME_FACTOR, 1, 3, 4));
        let stopped = transport_event(&os_engine::TransportInfo { playing: false, looping: None, ..song });
        assert_eq!(stopped.flags & (CLAP_TRANSPORT_IS_PLAYING | CLAP_TRANSPORT_IS_LOOP_ACTIVE), 0);
        // A plugin processes with it (and without it on the next block).
        let p = lemon_eq();
        let mut p = p.lock().unwrap();
        assert!(p.activate(48000));
        let (mut l, mut r) = (vec![0.1f32; 5000], vec![0.1f32; 5000]);
        p.transport_fx(&song);
        p.process_fx(48000, &mut l, &mut r);
        p.process_fx(48000, &mut l, &mut r);
    }

    // spec: plugins/plugin-parameters.md §1 (list order = automation index), parametric-eq-2.md (parameter order)
    #[test]
    fn parameter_list_follows_the_plugin_order() {
        let p = lemon_eq();
        let p = p.lock().unwrap();
        let infos: Vec<os_host::ParamInfo> = (0..p.param_count()).filter_map(|i| p.param_info(i)).collect();
        assert_eq!(infos.len(), 36);
        let names: Vec<&str> = infos.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(
            (names[0], names[7], names[14], names[21], names[28], names[35]),
            ("Band 1 level", "Band 1 freq", "Band 1 width", "Band 1 type", "Band 1 order", "Main level")
        );
        assert!(infos.iter().all(|i| i.automatable && !i.hidden));
        assert_eq!(infos[21].steps, 7);
        assert_eq!(p.param_value(infos[0].id), Some(0.5), "0 dB is the middle of -18..18");
        assert_eq!(p.param_text(infos[0].id, 1.0).as_deref(), Some("18.0dB"));
        assert_eq!(p.param_text(infos[21].id, 6.0 / 7.0).as_deref(), Some("Peaking"));
    }

    /// Automation reaches the plugin inside `process` by parameter index and normalized value, at its frame offset (a
    /// stepped parameter gets its nearest step); values no process call took are flushed on `idle`.
    // spec: plugins/plugin-parameters.md §2
    #[test]
    fn parameter_automation_reaches_the_plugin() {
        let shared = lemon_eq();
        let mut p = shared.lock().unwrap();
        assert!(p.activate(48000));
        let ids: Vec<u32> = (0..p.param_count()).filter_map(|i| p.param_info(i)).map(|i| i.id).collect();
        let near = |p: &dyn os_host::Plugin, id: u32, want: f64| (p.param_value(id).unwrap() - want).abs() < 1e-6;
        let (mut l, mut r) = (vec![0.1f32; 256], vec![0.1f32; 256]);
        p.param_fx(0, 35, 1.0);
        p.param_fx(100, 21, 0.4); // 0.4 x 7 = 2.8 -> type 3
        p.param_fx(300, 0, 0.0); // past this block: the next one
        p.param_fx(0, 999, 1.0); // unknown: ignored
        p.process_fx(48000, &mut l, &mut r);
        assert!(near(&*p, ids[35], 1.0));
        assert!(near(&*p, ids[21], 3.0 / 7.0), "{:?}", p.param_value(ids[21]));
        assert!(near(&*p, ids[0], 0.5), "not yet");
        p.process_fx(48000, &mut l, &mut r);
        assert!(near(&*p, ids[0], 0.0));
        // Nothing processes it (a bypassed slot): the next idle hands the newest value over.
        os_host::Plugin::idle(&mut *p);
        p.param_fx(0, 7, 0.1);
        p.param_fx(64, 7, 0.2);
        os_host::Plugin::idle(&mut *p);
        assert!(near(&*p, ids[7], 0.2), "{:?}", p.param_value(ids[7]));
    }

    /// The whole path: a playlist automation clip on "Insert 1, slot 1, Main level" moves the hosted EQ's parameter
    /// in song playback (spec: plugins/plugin-parameters.md §2-3).
    #[test]
    fn automation_clip_moves_a_slot_plugin_parameter() {
        use os_model::{AutoPoint, Automation, Channel, ChannelKind, Clip, ClipSource, Project};
        let shared = lemon_eq();
        assert!(shared.lock().unwrap().activate(48000));
        let main = shared.lock().unwrap().param_info(35).unwrap().id;
        let auto_ch = Channel {
            index: 0, kind: ChannelKind::Automation, name: None, plugin: None, color: None, enabled: true, volume: 0.78125,
            pan: 0.0, pitch_cents: 0, mixer_track: 0, sample_path: None, layer_children: vec![], envelopes: Default::default(),
            rack: Default::default(), plugin_state: None, misc: Default::default(), sound: Default::default(),
        };
        // Main level from 0 (-18 dB) to 1 (+18 dB) over 4 beats (120 BPM: 2 s).
        let auto = Automation {
            channel: 0,
            targets: vec![os_model::target::slot_param(1, 0, 35)],
            points: vec![AutoPoint::new(0.0, 0.0), AutoPoint::new(4.0, 1.0)],
            header: vec![0; 21],
            tail: vec![],
        };
        let project = Project {
            ppq: 96,
            tempo: Some(120.0),
            channels: vec![auto_ch],
            automations: vec![auto],
            arrangements: vec![os_model::Arrangement { clips: vec![Clip::new(0, 4 * 96, 0, ClipSource::Channel(0))], ..Default::default() }],
            mixer: (0..3).map(os_model::MixerTrack::new).collect(),
            ..Default::default()
        };
        let mut lib = os_engine::SampleLibrary::default();
        let mut e = os_engine::Engine::new(&project, &mut lib, 48000, os_engine::PlayMode::Song);
        e.set_effect(1, 0, shared.clone());
        let mut out = vec![0.0f32; 2 * 480];
        for _ in 0..100 {
            e.process(&mut out); // 1 s: half way
        }
        let v = shared.lock().unwrap().param_value(main).unwrap();
        assert!((v - 0.5).abs() < 0.01, "half way through the ramp: {v}");
    }

    /// The hosting layer's parameter surface on a CLAP instance: info, normalized get / set, text, and the edit
    /// reaching the plugin's state.
    #[test]
    fn parameters_through_the_hosting_layer() {
        let Some(lib) = lemon_sampler() else { return };
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
    fn hosts_lemon_sampler() {
        let Some(lib) = lemon_sampler() else {
            eprintln!("Lemon Sampler.clap not built; skipped");
            return;
        };
        let d = &lib.plugins[0];
        assert_eq!(d.name, "Lemon Sampler");
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

    /// The whole path Lemon Studio uses: a pattern on a plugin channel, played by the engine through the hosted .clap.
    #[test]
    fn engine_plays_a_pattern_through_lemon_sampler() {
        let Some(lib) = lemon_sampler() else { return };
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
            index: 0, kind: os_model::ChannelKind::Plugin, name: None, plugin: Some("Lemon Sampler".into()), plugin_state: None, color: None,
            enabled: true, volume: 0.78125, pan: 0.0, pitch_cents: 0, mixer_track: 0, sample_path: None,
            layer_children: vec![], envelopes: Default::default(), rack: Default::default(), misc: Default::default(), sound: Default::default(),
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
        assert!(peak > 0.05, "engine output through Lemon Sampler (peak {peak})");
    }

    /// An MP3 sample (os-engine's fixture: click of 0.9 at 0.1 s) loads and plays through Lemon Sampler at 48 kHz.
    #[test]
    fn lemon_sampler_plays_an_mp3() {
        let Some(lib) = lemon_sampler() else { return };
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
            index: 0, kind: os_model::ChannelKind::Plugin, name: None, plugin: Some("Lemon Sampler".into()), plugin_state: None, color: None,
            enabled: true, volume: 0.78125, pan: 0.0, pitch_cents: 0, mixer_track: 0, sample_path: None,
            layer_children: vec![], envelopes: Default::default(), rack: Default::default(), misc: Default::default(), sound: Default::default(),
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
        let mut out = vec![0.0f32; 2 * 16000];
        for c in out.chunks_mut(1024) {
            e.process(c);
        }
        let left: Vec<f32> = out.iter().step_by(2).copied().collect();
        let peak = (0..left.len()).max_by(|&a, &b| left[a].abs().total_cmp(&left[b].abs())).unwrap();
        assert!(left[peak].abs() > 0.2, "mp3 click heard (peak {})", left[peak]);
        // The click is at source frame 4410 (0.1 s at 44.1 kHz); nothing is trimmed, so it plays after the info
        // frame's 1152 frames of silence and the decoder's delay (1105), resampled to 48 kHz: about frame 7257
        // (spec: audio/sample-formats.md "Other formats").
        let at = (4410.0 + 1152.0 + 1105.0) * 48000.0 / 44100.0;
        assert!((peak as f64 - at).abs() < 25.0, "click at frame {peak}, expected about {at:.0}");
        // The sine starts at source frame 8820 (0.2 s), shifted the same way: about frame 12057.
        assert!(left[13000..15000].iter().any(|v| v.abs() > 0.05), "sine after 0.2 s");
    }

    /// A held-then-released live note of a long sample, as the typing keyboard plays it: in tune, and it keeps
    /// sounding past the engine's 5 s idle tail until the sample ends.
    #[test]
    fn long_live_note_is_in_tune_and_not_cut_off() {
        let Some(lib) = lemon_sampler() else { return };
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
            index: 0, kind: os_model::ChannelKind::Plugin, name: None, plugin: Some("Lemon Sampler".into()), plugin_state: None, color: None,
            enabled: true, volume: 0.78125, pan: 0.0, pitch_cents: 0, mixer_track: 0, sample_path: None,
            layer_children: vec![], envelopes: Default::default(), rack: Default::default(), misc: Default::default(), sound: Default::default(),
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
        let Some(lib) = lemon_sampler() else { return };
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
            index: 0, kind: os_model::ChannelKind::Plugin, name: None, plugin: Some("Lemon Sampler".into()), plugin_state: None, color: None,
            enabled: true, volume: 0.78125, pan: 0.0, pitch_cents: 0, mixer_track: 0, sample_path: None,
            layer_children: vec![], envelopes: Default::default(), rack: Default::default(), misc: Default::default(), sound: Default::default(),
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
    /// Every sound control of Lemon Sampler must change what a note sounds like (state-set parameters, rendered here).
    #[test]
    fn lemon_sampler_parameters_change_the_sound() {
        let Some(lib) = lemon_sampler() else { return };
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
        // PITCH does nothing in Resample, the default method (spec: audio/time-stretch.md): it is tried in Stretch (a
        // realtime method) and in e3 generic (stretched first). Knob positions are 0..1 (0.9 = +960 cents, 0.75 = x2);
        // the last stretch case is out of range, as a saved state may hold, and must play clamped, not as NaN.
        // A realtime method alone has nothing to stretch at the root key and plays as Resample (spec:
        // audio/time-stretch.md "Lemon Studio"), so it is checked the other way round below.
        let cases = [
            ("start_offset", "{\"f32\":0.5}"), ("ts_mul", "{\"f32\":0.75}"),
            ("ts_pitch", "{\"f32\":0.9},\"ts_mode\":{\"i32\":1}"), ("ts_pitch", "{\"f32\":0.9},\"ts_mode\":{\"i32\":3}"),
            ("ts_mul", "{\"f32\":0.75},\"ts_mode\":{\"i32\":3}"), ("ts_pitch", "{\"f32\":700.0},\"ts_mode\":{\"i32\":3}"),
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
            // f32::max skips NaN: a NaN render would pass as "no change".
            assert!(out.iter().all(|x| x.is_finite()), "{id} = {v}: the output is not finite");
            let diff = base.iter().zip(&out).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
            eprintln!("{id:>16} = {v:<16} max diff {diff:.4}");
            if diff < 1e-3 {
                dead.push(id);
            }
        }
        assert!(dead.is_empty(), "no change in the sound: {dead:?}");
        let same = render("\"ts_mode\":{\"i32\":1}");
        let diff = base.iter().zip(&same).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
        assert!(same.iter().all(|x| x.is_finite()) && diff < 1e-3, "Stretch with nothing to stretch: max diff {diff}");
    }

    fn serde_string(s: &str) -> String {
        let inner = format!("\"{}\"", s.replace('\\', "\\\\"));
        format!("\"{}\"", inner.replace('\\', "\\\\").replace('"', "\\\""))
    }

    /// Mono + portamento on the channel (the Misc tab): a second, overlapping note does not start a new attack; the
    /// sounding note glides to the new key through CLAP tuning expressions, which Lemon Sampler follows.
    // spec: audio/channel-voices.md
    #[test]
    fn mono_portamento_glides_through_lemon_sampler() {
        let entry = &lemon_sampler::LINKED_CLAP_ENTRY as *const _ as *const std::ffi::c_void;
        let lib = unsafe { Library::from_entry(entry, Path::new("Lemon Sampler.clap")) }.expect("linked Lemon Sampler");
        let mut p = Plugin::new(&lib, &lib.plugins[0].id).unwrap();
        // 4 s of a 441 Hz sine at 48 kHz.
        let wav = std::env::temp_dir().join("os-clap-porta.wav");
        let tone: Vec<f32> = (0..48000 * 4).flat_map(|i| { let v = (i as f32 / 48000.0 * 441.0 * std::f32::consts::TAU).sin() * 0.5; [v, v] }).collect();
        os_engine::wav::write_f32_stereo(&mut std::fs::File::create(&wav).unwrap(), 48000, &tone).unwrap();
        let state = format!("{{\"version\":\"0.0.1\",\"params\":{{}},\"fields\":{{\"sample-path\":{}}}}}", serde_string(&wav.to_string_lossy()));
        let mut blob = (state.len() as u64).to_le_bytes().to_vec();
        blob.extend_from_slice(state.as_bytes());
        assert!(p.load_state(&blob));
        assert!(p.activate(48000));
        let shared: os_engine::SharedInstrument = Arc::new(Mutex::new(p));
        let misc = os_model::ChannelMisc { mono: true, porta: true, slide: 1.0, ..Default::default() };
        let ch = os_model::Channel {
            index: 0, kind: os_model::ChannelKind::Plugin, name: None, plugin: Some("Lemon Sampler".into()), plugin_state: None, color: None,
            enabled: true, volume: 0.78125, pan: 0.0, pitch_cents: 0, mixer_track: 0, sample_path: None,
            layer_children: vec![], envelopes: Default::default(), rack: Default::default(), misc, sound: Default::default(),
        };
        let project = os_model::Project { ppq: 96, tempo: Some(130.0), channels: vec![ch], ..Default::default() };
        let mut slib = os_engine::SampleLibrary::default();
        let mut e = os_engine::Engine::new(&project, &mut slib, 48000, os_engine::PlayMode::Pattern(1));
        e.clear_schedule();
        e.set_instrument(0, shared);
        let mut out = Vec::new();
        let block = 480;
        for b in 0..(48000 * 3 / block) {
            match b {
                10 => e.note_on(0, 60, 100),
                // An octave up at 1 s, while 60 is still held.
                100 => e.note_on(0, 72, 100),
                _ => {}
            }
            let mut buf = vec![0.0f32; block * 2];
            e.process(&mut buf);
            out.extend(buf.chunks_exact(2).map(|f| f[0]));
        }
        let window = |from_s: f32, to_s: f32| &out[(from_s * 48000.0) as usize..(to_s * 48000.0) as usize];
        let crossings = |w: &[f32]| w.windows(2).filter(|p| p[0] <= 0.0 && p[1] > 0.0).count() as f32 / (w.len() as f32 / 48000.0);
        let before = crossings(window(0.5, 0.95));
        // One step per tick, n + 1 ticks at 130 BPM and 96 PPQ.
        let glide_s = (os_engine::laws::glide_ticks(1.0, 96) + 1) as f32 * 60.0 / (130.0 * 96.0);
        let after = crossings(window(1.0 + glide_s + 0.05, 2.0));
        let mid = crossings(window(1.0 + glide_s * 0.4, 1.0 + glide_s * 0.6));
        eprintln!("before {before:.1} Hz, mid-glide {mid:.1} Hz, after {after:.1} Hz (glide {glide_s:.3} s)");
        assert!((before - 441.0).abs() < 4.0, "first note in tune: {before}");
        assert!((after - 882.0).abs() < 8.0, "glides to the octave: {after}");
        assert!(mid > 500.0 && mid < 800.0, "between the two keys mid-glide: {mid}");
        // Legato: the sample keeps playing from where it was (no restart at 1 s): the level stays up through the glide.
        let peak = |w: &[f32]| w.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        let (p0, p1) = (peak(window(0.9, 0.99)), peak(window(1.0, 1.05)));
        assert!(p1 > p0 * 0.8, "no retrigger gap: {p0} -> {p1}");
    }
    /// A 2 s DC sample of 0.25 at 48 kHz.
    fn dc_wav(name: &str) -> PathBuf {
        let wav = std::env::temp_dir().join(format!("os-clap-{name}-{}.wav", std::process::id()));
        os_engine::wav::write_f32_stereo(&mut std::fs::File::create(&wav).unwrap(), 48000, &vec![0.25; 2 * 96000]).unwrap();
        wav
    }

    /// The built-in Lemon Sampler playing `wav` (the state Lemon Studio gives a sampler channel: its sample), activated
    /// at 48 kHz.
    fn sampler_of(wav: &Path) -> Plugin {
        let entry = &lemon_sampler::LINKED_CLAP_ENTRY as *const _ as *const std::ffi::c_void;
        let lib = unsafe { Library::from_entry(entry, Path::new("Lemon Sampler.clap")) }.expect("linked Lemon Sampler");
        let mut p = Plugin::new(&lib, &lib.plugins[0].id).unwrap();
        let state = format!("{{\"version\":\"0.0.1\",\"params\":{{}},\"fields\":{{\"sample-path\":{}}}}}", serde_string(&wav.to_string_lossy()));
        let mut blob = (state.len() as u64).to_le_bytes().to_vec();
        blob.extend_from_slice(state.as_bytes());
        assert!(p.load_state(&blob));
        assert!(p.activate(48000));
        p
    }

    /// Voices the engine names (`use_voice_gains`): each note gets its own id, the voice's gains go to it as volume
    /// note expressions, its choke takes that id, and a voice that has ended gets no more gains. Without the switch
    /// the engine is told the instance takes none.
    #[test]
    fn named_voices_take_their_gains_and_chokes_by_note_id() {
        use os_engine::Instrument as _;
        let wav = dc_wav("named");
        let mut p = sampler_of(&wav);
        assert!(!p.voice_gains());
        assert!(p.use_voice_gains() && p.voice_gains());
        let (mut l, mut r) = (vec![0.0f32; 64], vec![0.0f32; 64]);
        let levels = os_engine::NoteLevels::default();
        p.voice_start(0, 60, 0.8, 0.0, levels, 7);
        p.voice_gain(0, 7, 0.0);
        p.voice_gain(0, 7, 0.25);
        p.voice_start(10, 60, 0.8, 0.0, levels, 8);
        p.voice_gain(10, 8, 1.0);
        p.process(48000, &mut l, &mut r);
        let notes: Vec<(u16, i32, i16, u32)> = p.notes.iter().map(|n| (n.header.type_, n.note_id, n.key, n.header.time)).collect();
        let (a, b) = (notes[0].1, notes[1].1);
        assert!(a >= 0 && b >= 0 && a != b, "{notes:?}");
        assert_eq!(notes, vec![(CLAP_EVENT_NOTE_ON, a, 60, 0), (CLAP_EVENT_NOTE_ON, b, 60, 10)]);
        let exprs: Vec<(i32, i32, i16, u32, f64)> = p.exprs.iter().map(|x| (x.expression_id, x.note_id, x.key, x.header.time, x.value)).collect();
        assert_eq!(exprs, vec![(CLAP_NOTE_EXPRESSION_VOLUME, a, 60, 0, 0.0), (CLAP_NOTE_EXPRESSION_VOLUME, a, 60, 0, 0.25), (CLAP_NOTE_EXPRESSION_VOLUME, b, 60, 10, 1.0)]);
        // In the event order a voice's note on comes before its gains.
        assert_eq!(p.order.first().copied(), Some(&p.notes[0].header as *const clap_event_header));
        p.voice_choke(5, 60, 7);
        p.voice_gain(6, 7, 0.5);
        p.voice_gain(6, 8, 0.5);
        p.process(48000, &mut l, &mut r);
        let notes: Vec<(u16, i32, i16, u32)> = p.notes.iter().map(|n| (n.header.type_, n.note_id, n.key, n.header.time)).collect();
        assert_eq!(notes, vec![(CLAP_EVENT_NOTE_CHOKE, a, 60, 5)]);
        assert_eq!(p.exprs.iter().map(|x| (x.note_id, x.value)).collect::<Vec<_>>(), vec![(b, 0.5)]);
        let _ = std::fs::remove_file(&wav);
    }

    /// Two overlapping audio clips of one sample through Lemon Sampler, as Lemon Studio plays them: one audio clip
    /// channel, one key. With automatic crossfades each voice has its own fades, so the level stays flat through the
    /// overlap and the second clip plays to its own end (spec: audio/audio-clips.md "Automatic crossfades").
    #[test]
    fn lemon_sampler_crossfades_two_clips_of_one_sample() {
        use os_model::{Clip, ClipSource};
        let wav = dc_wav("crossfade");
        let render = |clips: Vec<Clip>| {
            let mut p = sampler_of(&wav);
            p.use_note_levels();
            p.use_voice_gains();
            let shared: os_engine::SharedInstrument = Arc::new(Mutex::new(p));
            let ch = os_model::Channel {
                index: 0, kind: os_model::ChannelKind::AudioClip, name: None, plugin: None, plugin_state: None, color: None,
                enabled: true, volume: 0.78125, pan: 0.0, pitch_cents: 0, mixer_track: 0, sample_path: Some(wav.to_string_lossy().into_owned()),
                layer_children: vec![], envelopes: Default::default(), rack: Default::default(), misc: Default::default(), sound: Default::default(),
            };
            let arrangement = os_model::Arrangement { clips, tracks: (1..=4).map(os_model::PlaylistTrack::new).collect(), ..Default::default() };
            let project = os_model::Project { ppq: 96, tempo: Some(130.0), channels: vec![ch], arrangements: vec![arrangement], auto_crossfades: Some(true), ..Default::default() };
            let mut slib = os_engine::SampleLibrary::default();
            let mut e = os_engine::Engine::new(&project, &mut slib, 48000, os_engine::PlayMode::Song);
            e.set_instrument(0, shared);
            let out = e.render_all(480);
            out.iter().step_by(2).copied().collect::<Vec<f32>>()
        };
        let at = |t: u64| os_engine::Clock::frame_of_tick(48000, 96, 130.0, t) as usize;
        let first = Clip::new(0, 384, 0, ClipSource::Channel(0));
        let alone = render(vec![first]);
        let level = alone[24000];
        assert!(level > 0.1, "{level}");
        let both = render(vec![first, Clip::new(192, 384, 0, ClipSource::Channel(0))]);
        let (worst, at_worst) = (0..at(576)).map(|f| ((both[f] - level).abs(), f)).fold((0.0f32, 0), |m, x| if x.0 > m.0 { x } else { m });
        assert!(worst < 1e-3 * level, "flat through the overlap and after the first clip: off by {worst} at frame {at_worst} (level {level})");
        // The render goes on to the first quiet tick after the song: the second clip's voice has closed by then.
        assert!(both.len() > at(576) && both[both.len() - 100..].iter().all(|v| v.abs() < 1e-6), "silent after the second clip");
        let _ = std::fs::remove_file(&wav);
    }
}
