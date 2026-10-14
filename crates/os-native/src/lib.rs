// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

//! Hosting plugins written against the FL Studio native plugin SDK, loaded from the user's own installation.
//!
//! Implemented from the spec `plugins/fruity-abi.md` (public SDK interface).
//! Windows x64 only for now.
//
// spec: plugins/fruity-abi.md
#![allow(unsafe_op_in_unsafe_fn)]

pub mod abi;
pub mod adapter;
pub mod check;
pub mod guard;
pub mod host;
pub mod library;
pub mod lock;
pub mod stream;
pub mod stub;
pub mod voicing;

pub use library::{dll_files, dll_files_in, NativeLibrary};

use std::ffi::{c_char, CString};
use std::mem::ManuallyDrop;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use abi::{flags, IntPtr, PluginObject, VoiceParams};
use host::HostObject;
use stream::MemStream;

/// Default FL Studio install folder (data path): `OPENSTUDIO_FACTORY` if set, else the usual path.
pub fn default_fl_dir() -> PathBuf {
    std::env::var_os("OPENSTUDIO_FACTORY")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Program Files\Image-Line\FL Studio 2026"))
}

/// Whether the experimental native-plugin host is opted into for this process, via the
/// `OPENSTUDIO_NATIVE` environment variable (`1`/`true`/`on`). The app also has a settings
/// toggle (shown only with `settings_shown`); it sets this so the whole process agrees.
pub fn enabled() -> bool {
    matches!(
        std::env::var("OPENSTUDIO_NATIVE").as_deref(),
        Ok("1") | Ok("true") | Ok("on") | Ok("yes")
    )
}

/// Whether the app's settings show the native host toggles at all, via the `OPENSTUDIO_NATIVE_SETTINGS` environment
/// variable (`1`/`true`/`on`/`yes`). Hidden by default; `OPENSTUDIO_NATIVE` still turns the host on without them.
pub fn settings_shown() -> bool {
    matches!(std::env::var("OPENSTUDIO_NATIVE_SETTINGS").as_deref(), Ok("1") | Ok("true") | Ok("on") | Ok("yes"))
}

/// Turns the opt-in on or off for this process (so a settings toggle and `enabled()` agree).
pub fn set_enabled(on: bool) {
    // SAFETY: set once at startup / from the UI thread before plugins are scanned.
    unsafe {
        if on {
            std::env::set_var("OPENSTUDIO_NATIVE", "1");
        } else {
            std::env::remove_var("OPENSTUDIO_NATIVE");
        }
    }
}

/// Whether an opened project's native plugins are played by the installed plugins even where a built-in recreation
/// would take them (spec: plugins/fruity-abi.md "Opened projects"): only while the native host is on. Set by a
/// second, hidden settings toggle or `OPENSTUDIO_NATIVE_FIRST`; session only, like the host toggle.
pub fn prefer_native() -> bool {
    enabled() && matches!(std::env::var("OPENSTUDIO_NATIVE_FIRST").as_deref(), Ok("1") | Ok("true") | Ok("on") | Ok("yes"))
}

/// The stored choice behind `prefer_native`, whether or not the host is on.
pub fn prefer_native_set() -> bool {
    std::env::var_os("OPENSTUDIO_NATIVE_FIRST").is_some()
}

/// Turns the preference on or off for this process.
pub fn set_prefer_native(on: bool) {
    // SAFETY: changed from the UI thread only; read by the same thread.
    unsafe {
        if on {
            std::env::set_var("OPENSTUDIO_NATIVE_FIRST", "1");
        } else {
            std::env::remove_var("OPENSTUDIO_NATIVE_FIRST");
        }
    }
}

/// One installed FL native plugin found on disk.
#[derive(Debug, Clone)]
pub struct Installed {
    pub name: String,
    pub is_generator: bool,
    pub path: PathBuf,
}

/// Lists the installed FL native plugins under `<fl>\Plugins\Fruity\{Generators,Effects}`.
/// Each plugin is a `<Name>\<Name>_x64.dll`. Nothing is loaded; the kind comes from the folder.
pub fn installed(fl_dir: &Path) -> Vec<Installed> {
    let mut out = Vec::new();
    for (kind, is_gen) in [("Generators", true), ("Effects", false)] {
        let dir = fl_dir.join("Plugins").join("Fruity").join(kind);
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for e in entries.flatten() {
            if !e.path().is_dir() {
                continue;
            }
            let name = e.file_name().to_string_lossy().into_owned();
            let dll = e.path().join(format!("{name}_x64.dll"));
            if dll.is_file() {
                out.push(Installed { name, is_generator: is_gen, path: dll });
            }
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

#[derive(Debug, thiserror::Error)]
pub enum NativeError {
    #[error("{0}: {1}")]
    Load(PathBuf, String),
    #[error("{0}: CreatePlugInstance returned null")]
    Create(PathBuf),
    /// The plugin raised an error it did not handle while it was being created (spec: "Crash containment").
    #[error("{0}: {1} while loading")]
    Crashed(PathBuf, String),
}

/// Static information about a loaded plugin.
#[derive(Debug, Clone)]
pub struct PluginInfo {
    pub sdk_version: i32,
    pub long_name: String,
    pub short_name: String,
    pub flags: i32,
    pub num_params: i32,
    pub def_poly: i32,
}

impl PluginInfo {
    pub fn is_generator(&self) -> bool {
        self.flags & flags::GENERATOR != 0
    }
    pub fn uses_voice_render(&self) -> bool {
        self.flags & flags::USE_SAMPLER != 0
    }
    /// The plugin processes no audio buffers at all (controllers, visual plugins): the host never calls its render
    /// methods (spec: flags, NoProcess).
    pub fn no_process(&self) -> bool {
        self.flags & flags::NO_PROCESS != 0
    }
}

/// Creating and destroying instances is serialised process-wide: the plugins' runtime is not made for two threads
/// constructing instances at once (two parallel loads crashed; one GUI thread, as in the app, never does).
static LIFECYCLE: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// While alive, the process's current folder is the plugin's own `Data` folder, when it has one: some plugins open
/// their data files by a path relative to the current folder while they are created (spec: "Data files").
struct DataDir(Option<PathBuf>);

impl DataDir {
    fn enter(dll: &Path) -> DataDir {
        let Some(data) = dll.parent().map(|d| d.join("Data")).filter(|d| d.is_dir()) else { return DataDir(None) };
        let Ok(prev) = std::env::current_dir() else { return DataDir(None) };
        if std::env::set_current_dir(&data).is_err() {
            return DataDir(None);
        }
        DataDir(Some(prev))
    }
}

impl Drop for DataDir {
    fn drop(&mut self) {
        if let Some(prev) = &self.0 {
            let _ = std::env::set_current_dir(prev);
        }
    }
}

/// A plugin instance. Not thread-safe: call from one thread.
///
/// Every call into the plugin goes through `guard` (spec: "Crash containment"). When the plugin raises an error it does
/// not handle, the instance is stopped: it is never called again, and its DLL and host object stay loaded for the rest
/// of the process (its code may still run timers or threads that use them).
pub struct Plugin {
    obj: *mut PluginObject,
    // Freed in this order in `drop`: the instance, then the host, then the DLL (never for a stopped plugin).
    host: ManuallyDrop<Box<HostObject>>,
    lib: ManuallyDrop<libloading::Library>,
    pub info: PluginInfo,
    /// Exception code of the error that stopped the plugin, 0 while it runs.
    stopped: AtomicU32,
    /// What stopped it, for the user (`guard::describe_with`).
    reason: std::sync::OnceLock<String>,
}

impl Plugin {
    /// Loads `dll` and creates one instance. `fl_dir` is the FL install folder (data path).
    pub fn load(dll: &Path, fl_dir: &Path, sample_rate: u32, max_block: usize) -> Result<Plugin, NativeError> {
        let err = |e: String| NativeError::Load(dll.to_owned(), e);
        let trace = |m: &str| {
            if host::trace_enabled() {
                eprintln!("[load] {m}");
            }
        };
        let _one = LIFECYCLE.lock().unwrap_or_else(|e| e.into_inner());
        trace("LoadLibrary");
        let lib = unsafe { libloading::Library::new(dll) }.map_err(|e| err(e.to_string()))?;
        let create: abi::CreatePlugInstance =
            *unsafe { lib.get::<abi::CreatePlugInstance>(b"CreatePlugInstance\0") }.map_err(|e| err(e.to_string()))?;
        trace("got CreatePlugInstance");
        let mut host = HostObject::new(&fl_dir.to_string_lossy(), sample_rate, max_block);
        let host_ptr = &mut *host as *mut HostObject as IntPtr;
        let data = DataDir::enter(dll);
        trace("calling CreatePlugInstance");
        let obj = match unsafe { guard::call(create as *const (), [host_ptr, 0, 0, 0, 0, 0]) } {
            Ok(obj) => obj as *mut PluginObject,
            Err(code) => {
                trace(&format!("CreatePlugInstance raised {code:#x}"));
                // Half built: its code may still hold the host and run, so neither is ever freed.
                std::mem::forget(host);
                std::mem::forget(lib);
                return Err(NativeError::Crashed(dll.to_owned(), guard::describe_with(code, &guard::last_message())));
            }
        };
        trace(&format!("instance at {obj:p}"));
        if obj.is_null() {
            return Err(NativeError::Create(dll.to_owned()));
        }
        let info = unsafe {
            let i = &*(*obj).info;
            PluginInfo {
                sdk_version: i.sdk_version,
                long_name: host::cstr(i.long_name),
                short_name: host::cstr(i.short_name),
                flags: i.flags,
                num_params: i.num_params,
                def_poly: i.def_poly,
            }
        };
        trace(&format!("info read: {} flags {:#x}", info.long_name, info.flags));
        let p = Plugin {
            obj,
            host: ManuallyDrop::new(host),
            lib: ManuallyDrop::new(lib),
            info,
            stopped: AtomicU32::new(0),
            reason: std::sync::OnceLock::new(),
        };
        p.dispatch(abi::dispatch::SET_SAMPLE_RATE, 0, sample_rate as IntPtr);
        p.dispatch(abi::dispatch::SET_BLOCK_SIZE, 0, max_block as IntPtr);
        p.dispatch(abi::dispatch::PROCESS_MODE, 0, 0);
        drop(data);
        if let Some(why) = p.stop_reason() {
            // Stopped: never freed (and its drop would wait for `LIFECYCLE`, held here).
            std::mem::forget(p);
            return Err(NativeError::Crashed(dll.to_owned(), why));
        }
        Ok(p)
    }

    fn vt(&self) -> &abi::PluginVtbl {
        unsafe { &*(*self.obj).vtable }
    }

    /// The exception code of the error that stopped the plugin (`None` while it runs).
    pub fn stopped(&self) -> Option<u32> {
        Some(self.stopped.load(Ordering::Relaxed)).filter(|&c| c != 0)
    }

    /// What stopped the plugin, for the user: "plugin error", with the plugin's own text when it gave a short one.
    pub fn stop_reason(&self) -> Option<String> {
        let code = self.stopped()?;
        Some(self.reason.get().cloned().unwrap_or_else(|| guard::describe(code)))
    }

    /// Calls plugin method `f` with the instance and `args` through the guard. `None` when the plugin is stopped, or
    /// stops in this call (it raised an error it did not handle).
    fn call(&self, f: *const (), args: [IntPtr; 5]) -> Option<IntPtr> {
        if self.stopped().is_some() {
            return None;
        }
        let [a, b, c, d, e] = args;
        match unsafe { guard::call(f, [self.obj as IntPtr, a, b, c, d, e]) } {
            Ok(r) => Some(r),
            Err(code) => {
                let why = guard::describe_with(code, &guard::last_message());
                eprintln!("[native] {} stopped in {}: {why}", self.info.long_name, self.method_name(f));
                let _ = self.reason.set(why);
                self.stopped.store(code.max(1), Ordering::Relaxed);
                None
            }
        }
    }

    /// The plugin methods that share one function with another method: what a plugin leaves unimplemented (they all
    /// point at its runtime's "abstract" stub). For diagnostics.
    pub fn shared_methods(&self) -> Vec<&'static str> {
        let table = self.vt() as *const abi::PluginVtbl as *const *const ();
        let slot = |i: usize| unsafe { *table.add(i) };
        (0..METHOD_NAMES.len()).filter(|&i| (0..METHOD_NAMES.len()).any(|j| j != i && slot(j) == slot(i))).map(|i| METHOD_NAMES[i]).collect()
    }

    /// The name of plugin method `f` (its place in the method table), for messages.
    fn method_name(&self, f: *const ()) -> &'static str {
        const NAMES: [&str; 20] = METHOD_NAMES;
        let table = self.vt() as *const abi::PluginVtbl as *const *const ();
        (0..NAMES.len()).find(|&i| unsafe { *table.add(i) } == f).map_or("a plugin method", |i| NAMES[i])
    }

    /// Runs `f` holding the plugin's render lock (audio-thread calls; see `lock`).
    fn locked<R>(&self, f: impl FnOnce(&Self) -> R) -> R {
        let lock: *const lock::PluginLock = self.host.lock();
        // SAFETY: the lock lives in the host context, which outlives this call and never moves.
        let _g = unsafe { &*lock }.guard();
        f(self)
    }

    pub fn dispatch(&self, id: IntPtr, index: IntPtr, value: IntPtr) -> IntPtr {
        self.call(self.vt().dispatcher as *const (), [id, index, value, 0, 0]).unwrap_or(0)
    }

    /// Sets the tempo (BPM) and samples per tick.
    pub fn set_tempo(&mut self, bpm: f32, ppq: u16) {
        let sr = self.host.context().sample_rate as f32;
        let spt = sr * 60.0 / bpm / f32::from(ppq);
        let tempo = [abi::event::TEMPO as IntPtr, bpm.to_bits() as IntPtr, spt as i32 as IntPtr, 0, 0];
        self.call(self.vt().process_event as *const (), tempo);
        self.dispatch(abi::dispatch::SET_SAMPLES_PER_TICK, 0, spt.to_bits() as IntPtr);
    }

    /// The parameter count: NumParams, or the count the plugin set later with host dispatcher 27 SetNumParams (spec:
    /// "Parameters").
    pub fn param_count(&self) -> usize {
        self.host.num_params().unwrap_or(self.info.num_params).max(0) as usize
    }

    /// The name of parameter `index` (GetName section 0).
    pub fn param_name(&self, index: i32) -> String {
        self.get_name(abi::name::PARAM, index)
    }

    /// Parameter `index`'s current value as the plugin shows it (GetName section 1).
    pub fn param_text(&self, index: i32) -> String {
        self.get_name(abi::name::PARAM_VALUE, index)
    }

    fn get_name(&self, section: i32, index: i32) -> String {
        let mut buf = [0 as c_char; 512];
        let args = [section as IntPtr, index as IntPtr, 0, buf.as_mut_ptr() as IntPtr, 0];
        self.call(self.vt().get_name as *const (), args);
        buf[buf.len() - 1] = 0;
        host::cstr(buf.as_ptr())
    }

    /// Current value of a parameter (plugin-specific integer scale).
    pub fn param_value(&mut self, index: i32) -> i32 {
        self.call(self.vt().process_param as *const (), [index as IntPtr, 0, abi::rec::GET_VALUE as IntPtr, 0, 0]).map_or(0, |v| v as i32)
    }

    /// Moves parameter `index` to `midi` (0..2^30, which the plugin maps to its control's range):
    /// `ProcessParam(index, midi, update value | from MIDI)` under the render lock (audio thread; spec: "Parameters").
    pub fn set_param_midi(&mut self, index: i32, midi: i32) {
        let flags = (abi::rec::UPDATE_VALUE | abi::rec::FROM_MIDI) as IntPtr;
        self.locked(|p| p.call(p.vt().process_param as *const (), [index as IntPtr, midi as IntPtr, flags, 0, 0]));
    }

    /// Makes the editor's control of parameter `index` show `midi`: `ProcessParam(index, midi, update control | from
    /// MIDI)` (interface thread).
    pub fn show_param_midi(&mut self, index: i32, midi: i32) {
        let flags = (abi::rec::UPDATE_CONTROL | abi::rec::FROM_MIDI) as IntPtr;
        self.call(self.vt().process_param as *const (), [index as IntPtr, midi as IntPtr, flags, 0, 0]);
    }

    /// Saves the plugin state into a byte vector (`None` when the plugin stops while saving). Takes `&self`: the FFI
    /// call drives the plugin through its raw object pointer and does not need Rust `&mut`.
    pub fn save_state(&self) -> Option<Vec<u8>> {
        let mut s = MemStream::new(Vec::new());
        self.call(self.vt().save_restore_state as *const (), [s.as_ptr() as IntPtr, 1, 0, 0, 0])?;
        Some(std::mem::take(&mut s.data))
    }

    /// Restores a previously saved plugin state; false when the plugin stops while reading it.
    pub fn restore_state(&mut self, data: &[u8]) -> bool {
        let mut s = MemStream::new(data.to_vec());
        self.call(self.vt().save_restore_state as *const (), [s.as_ptr() as IntPtr, 0, 0, 0, 0]).is_some()
    }

    /// Starts a voice with the note's own levels `init` and the final levels `fin` (the note's and its channel's);
    /// `velocity` (0..1) is what the plugin is told when it asks for the note's velocity. `tag` names the voice in the
    /// plugin's requests (`take_voice_requests`). `None` when the plugin started no voice (it answers -1, e.g. a sample
    /// player without a sample) or is stopped.
    pub fn trigger(&mut self, init: abi::LevelParams, fin: abi::LevelParams, velocity: f32, tag: IntPtr) -> Option<Voice> {
        self.host.voices().velocity.push((tag, velocity));
        let params = Box::into_raw(Box::new(VoiceParams { init, fin }));
        match self.locked(|p| p.call(p.vt().trigger_voice as *const (), [params as IntPtr, tag, 0, 0, 0])) {
            Some(NO_VOICE) => {
                // No voice to release or kill (the plugin's Voice_Kill fails on this handle).
                self.host.voices().velocity.retain(|v| v.0 != tag);
                drop(unsafe { Box::from_raw(params) });
                None
            }
            Some(handle) => Some(Voice { tag, handle, params }),
            // Stopped while starting it: the plugin may hold the address, so the levels are never freed.
            None => None,
        }
    }

    /// Note off for `voice`: the plugin goes into its release and asks for the kill when it is done.
    pub fn release(&mut self, voice: &Voice) {
        self.locked(|p| p.call(p.vt().voice_release as *const (), [voice.handle, 0, 0, 0, 0]));
    }

    /// Ends `voice` at once and frees it.
    pub fn kill(&mut self, voice: Voice) {
        self.locked(|p| p.call(p.vt().voice_kill as *const (), [voice.handle, 0, 0, 0, 0]));
        self.host.voices().velocity.retain(|v| v.0 != voice.tag);
        if self.stopped().is_some() {
            // The plugin may still hold the address of the levels.
            std::mem::forget(voice);
        }
    }

    /// Lets go of `voice` without calling the plugin: it said it freed the voice itself.
    pub fn forget(&mut self, voice: Voice) {
        self.host.voices().velocity.retain(|v| v.0 != voice.tag);
    }

    /// What the plugin asked of its voices since the last call: (voices to kill: the tag, and whether the host frees
    /// it with `kill` (else `forget`); tags to release).
    pub fn take_voice_requests(&mut self) -> (Vec<(IntPtr, bool)>, Vec<IntPtr>) {
        let mut b = self.host.voices();
        (std::mem::take(&mut b.kill), std::mem::take(&mut b.release))
    }

    /// Changes the sample rate the plugin works at (dispatcher `SetSampleRate`).
    pub fn set_sample_rate(&mut self, rate: u32) {
        self.host.context().sample_rate = rate;
        self.dispatch(abi::dispatch::SET_SAMPLE_RATE, 0, rate as IntPtr);
    }

    /// Full generators: renders up to `out.len()/2` frames, returns frames rendered.
    pub fn gen_render(&mut self, out: &mut [f32]) -> usize {
        let mut n = (out.len() / 2) as i32;
        let args = [out.as_mut_ptr() as IntPtr, &mut n as *mut i32 as IntPtr, 0, 0, 0];
        match self.locked(|p| p.call(p.vt().gen_render as *const (), args)) {
            Some(_) => n.max(0) as usize,
            None => 0,
        }
    }

    /// Hybrid generators: renders one voice; returns false when it has no more data.
    pub fn voice_render(&mut self, voice: &Voice, out: &mut [f32]) -> (usize, bool) {
        let mut n = (out.len() / 2) as i32;
        let args = [voice.handle, out.as_mut_ptr() as IntPtr, &mut n as *mut i32 as IntPtr, 0, 0];
        match self.locked(|p| p.call(p.vt().voice_render as *const (), args)) {
            Some(r) => (n.max(0) as usize, r as i32 == 0),
            None => (0, false),
        }
    }

    /// Effects: processes `buf` (interleaved stereo) in place. False when the plugin stopped (the buffer may then hold
    /// anything).
    pub fn eff_render(&mut self, buf: &mut [f32]) -> bool {
        let n = (buf.len() / 2) as IntPtr;
        let b = buf.as_mut_ptr() as IntPtr;
        self.locked(|p| p.call(p.vt().eff_render as *const (), [b, b, n, 0, 0])).is_some()
    }

    pub fn idle(&mut self) {
        self.call(self.vt().idle as *const (), [0; 5]);
    }

    /// Advances the plugin's internal clock one tick. Generators that set the `WantNewTick` flag
    /// expect this between buffers; without it their envelopes and timing do not progress.
    pub fn new_tick(&mut self) {
        self.locked(|p| p.call(p.vt().new_tick as *const (), [0; 5]));
    }

    /// Tells the plugin whether the song is playing (dispatcher `SetPlaying`).
    pub fn set_playing(&mut self, playing: bool) {
        self.dispatch(abi::dispatch::SET_PLAYING, 0, playing as IntPtr);
    }

    /// Enables or bypasses the plugin (dispatcher `SetEnabled`).
    pub fn set_enabled(&mut self, enabled: bool) {
        self.dispatch(abi::dispatch::SET_ENABLED, 0, enabled as IntPtr);
    }

    /// The editor window the plugin created after `ShowEditor` (the plugin object's `EditorHandle`
    /// field), 0 when it has none.
    pub fn editor_handle(&self) -> IntPtr {
        unsafe { (*self.obj).editor_handle as IntPtr }
    }

    pub fn host_name(&self) -> CString {
        CString::default()
    }
}

impl Drop for Plugin {
    fn drop(&mut self) {
        let _one = LIFECYCLE.lock().unwrap_or_else(|e| e.into_inner());
        self.call(self.vt().destroy_object as *const (), [0; 5]);
        if self.stopped().is_some() {
            // Never freed (see `Plugin`).
            return;
        }
        // SAFETY: dropped once, here, after the instance is gone.
        unsafe {
            ManuallyDrop::drop(&mut self.host);
            ManuallyDrop::drop(&mut self.lib);
        }
    }
}

/// What `TriggerVoice` answers when it started no voice.
const NO_VOICE: IntPtr = -1;

/// The plugin methods in method-table order.
const METHOD_NAMES: [&str; 20] = [
    "DestroyObject", "Dispatcher", "Idle", "SaveRestoreState", "GetName", "ProcessEvent", "ProcessParam", "Eff_Render",
    "Gen_Render", "TriggerVoice", "Voice_Release", "Voice_Kill", "Voice_ProcessEvent", "Voice_Render", "NewTick",
    "MIDITick", "MIDIIn", "MsgIn", "OutputVoice_ProcessEvent", "OutputVoice_Kill",
];

/// A voice a generator plays. The plugin keeps the address of its levels and reads the final ones while the voice
/// sounds, so they stay here, at one address, until the voice is killed (spec: "Voices").
pub struct Voice {
    /// The host's name for the voice in the plugin's requests.
    pub tag: IntPtr,
    handle: IntPtr,
    params: *mut VoiceParams,
}

// SAFETY: the levels are only touched by the thread driving the plugin, between its calls.
unsafe impl Send for Voice {}

impl Voice {
    /// The plugin's own handle for the voice.
    pub fn handle(&self) -> IntPtr {
        self.handle
    }

    /// The levels the voice started with.
    pub fn init(&self) -> abi::LevelParams {
        unsafe { (*self.params).init }
    }

    /// Changes the final levels the plugin reads from now on (a glide, the channel's pitch).
    pub fn set_final(&mut self, levels: abi::LevelParams) {
        unsafe { (*self.params).fin = levels };
    }
}

impl Drop for Voice {
    fn drop(&mut self) {
        // SAFETY: made by `Box::into_raw` in `trigger`; the plugin let go of it (killed, or destroyed).
        unsafe { drop(Box::from_raw(self.params)) };
    }
}

/// Finds `<fl>\Plugins\Fruity\{Generators,Effects}\<name>\<name>_x64.dll`.
pub fn find_plugin(fl_dir: &Path, name: &str) -> Option<PathBuf> {
    ["Generators", "Effects"].iter().find_map(|kind| {
        let p = fl_dir.join("Plugins").join("Fruity").join(kind).join(name).join(format!("{name}_x64.dll"));
        p.is_file().then_some(p)
    })
}

/// Default block size handed to a plugin at load.
pub const DEFAULT_BLOCK: usize = adapter::MAX_BLOCK;

/// Loads one plugin DLL and wraps it as a shared, activated `os_host::Plugin`. `is_generator`
/// selects the descriptor kind (instrument vs insert effect).
pub fn load_plugin(dll: &Path, fl_dir: &Path, rate: u32, is_generator: bool) -> Result<os_host::SharedPlugin, NativeError> {
    use os_host::{Descriptor, Format, Kind, Plugin as _};
    let plugin = Plugin::load(dll, fl_dir, rate, DEFAULT_BLOCK)?;
    let name = dll.parent().and_then(|p| p.file_name()).map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let desc = Descriptor {
        format: Format::Native,
        id: name.clone(),
        name,
        vendor: String::new(),
        version: String::new(),
        features: Vec::new(),
        kind: if is_generator { Kind::Instrument } else { Kind::Effect },
        path: dll.to_path_buf(),
    };
    let mut p = adapter::NativePlugin::new(plugin, desc);
    p.activate(rate);
    Ok(std::sync::Arc::new(std::sync::Mutex::new(p)))
}

/// Loads an effect plugin as a shared engine insert effect.
pub fn load_effect(dll: &Path, fl_dir: &Path, rate: u32) -> Result<os_engine::instrument::SharedEffect, NativeError> {
    Ok(load_plugin(dll, fl_dir, rate, false)? as os_engine::instrument::SharedEffect)
}

/// Loads a generator plugin as a shared engine instrument (experimental; level not yet correct).
pub fn load_instrument(dll: &Path, fl_dir: &Path, rate: u32) -> Result<os_engine::instrument::SharedInstrument, NativeError> {
    Ok(load_plugin(dll, fl_dir, rate, true)? as os_engine::instrument::SharedInstrument)
}

#[cfg(test)]
mod tests {
    use super::*;

    // These tests need the user's FL install. They skip (pass) when it is absent, and are only
    // meaningful on Windows x64. No plugin DLL is bundled or committed.
    fn fl() -> Option<PathBuf> {
        let d = default_fl_dir();
        d.join("Plugins").join("Fruity").exists().then_some(d)
    }

    #[test]
    fn enumerates_installed_plugins() {
        let Some(fl) = fl() else { return };
        let list = installed(&fl);
        assert!(!list.is_empty(), "expected some Fruity plugins");
        assert!(list.iter().any(|p| !p.is_generator), "expected at least one effect");
    }

    // Loads a real FL plugin DLL in-process (skips without FL). Instance creation is serialised
    // (`LIFECYCLE`), so these run in parallel with the other tests.
    #[test]
    #[cfg(windows)]
    fn effect_loads_and_passes_audio() {
        let Some(fl) = fl() else { return };
        let Some(dll) = find_plugin(&fl, "Fruity Balance") else { return };
        let rate = 44_100;
        let fx = load_effect(&dll, &fl, rate).expect("load Fruity Balance");
        // A 1 kHz sine through default Balance should come back finite and roughly unchanged.
        let mut l: Vec<f32> = (0..1024).map(|s| (std::f32::consts::TAU * 1000.0 * s as f32 / rate as f32).sin() * 0.5).collect();
        let mut r = l.clone();
        let before = l.clone();
        fx.lock().unwrap().process_fx(rate, &mut l, &mut r);
        assert!(l.iter().all(|s| s.is_finite()), "output must be finite");
        let peak = l.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(peak > 0.1 && peak < 2.0, "default Balance near unity, got peak {peak}");
        // Not a null-test, but it should not be wildly different from the input.
        let diff: f32 = l.iter().zip(&before).map(|(a, b)| (a - b).abs()).sum::<f32>() / l.len() as f32;
        assert!(diff < 0.2, "default Balance should roughly pass through, mean abs diff {diff}");
    }

    #[test]
    #[cfg(windows)]
    fn effect_hosts_through_os_host_library() {
        use os_host::Library as _;
        let Some(fl) = fl() else { return };
        let Some(dll) = find_plugin(&fl, "Fruity Balance") else { return };
        let lib = NativeLibrary::load(&dll).expect("build library");
        assert_eq!(lib.plugins().len(), 1);
        let d = lib.plugins()[0].clone();
        assert!(d.is_effect());
        assert_eq!(d.uid(), "native:Fruity Balance");
        let p = lib.create(&d.id).expect("create instance");
        {
            let mut pl = p.lock().unwrap();
            assert!(pl.activate(44_100));
            assert!(pl.save_state().is_some(), "effect should report its state");
        }
        let rate = 44_100u32;
        let mut l: Vec<f32> = (0..1024).map(|s| (std::f32::consts::TAU * 1000.0 * s as f32 / rate as f32).sin() * 0.5).collect();
        let mut r = l.clone();
        (p as os_engine::SharedEffect).lock().unwrap().process_fx(rate, &mut l, &mut r);
        assert!(l.iter().all(|s| s.is_finite()));
        let peak = l.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(peak > 0.1 && peak < 2.0, "near unity, got {peak}");
    }

    /// Parameters are listed and moved through os_host (spec: plugins/fruity-abi.md "Parameters"): the low-pass's
    /// cutoff, closed, takes the highs out of white noise; open, it lets them through. Ignored: native plugins can pop
    /// modal message boxes, so run it on its own (hidden desktop).
    #[test]
    #[ignore]
    #[cfg(windows)]
    fn native_parameters_list_and_move() {
        let Some(fl) = fl() else { return };
        let Some(dll) = find_plugin(&fl, "Fruity Fast LP") else { return };
        let rate = 44_100u32;
        let p = load_plugin(&dll, &fl, rate, false).expect("load");
        let pl = p.lock().unwrap();
        let count = pl.param_count();
        assert!(count > 0, "no parameters");
        let infos: Vec<_> = (0..count).filter_map(|i| pl.param_info(i)).collect();
        assert_eq!(infos.len(), count);
        for i in &infos {
            println!("parameter {}: {:?}", i.id, i.name);
        }
        assert!(infos.iter().all(|i| !i.name.is_empty() && !i.name.starts_with('^')), "every parameter has a name");
        let cutoff = infos.iter().find(|i| i.name.to_lowercase().contains("cut")).map_or(0, |i| i.id);
        assert!(pl.tick_params_fx());
        drop(pl);
        // White noise (a fixed LCG), measured by the energy of its first difference (the highs).
        let mut seed = 1u32;
        let noise: Vec<f32> = (0..rate as usize)
            .map(|_| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (seed >> 8) as f32 / (1u32 << 24) as f32 - 0.5
            })
            .collect();
        let highs = |v: f64| {
            let fx = p.clone() as os_engine::SharedEffect;
            let mut fx = fx.lock().unwrap();
            fx.param_fx(0, cutoff, v);
            let mut sum = 0.0f64;
            for block in noise.chunks(512) {
                let (mut l, mut r) = (block.to_vec(), block.to_vec());
                fx.process_fx(rate, &mut l, &mut r);
                sum += l.windows(2).map(|w| f64::from(w[1] - w[0]).powi(2)).sum::<f64>();
            }
            sum
        };
        let closed = highs(0.0);
        let open = highs(1.0);
        println!("highs closed {closed:.3} open {open:.3}");
        assert!(open > closed * 4.0, "closed {closed} open {open}");
        let pl = p.lock().unwrap();
        assert_eq!(pl.param_value(cutoff), Some(1.0));
        let text = pl.param_text(cutoff, 1.0);
        println!("value text {text:?}");
        assert!(pl.param_text(cutoff, 0.5).is_none(), "no text for a value it does not have");
    }

    /// The channel's envelopes, LFOs and filter on a native generator's voices (spec: plugins/fruity-abi.md "Voices"),
    /// on the installed 3x Osc: prints whether it renders voice by voice (UseSampler); a new channel's settings change
    /// nothing; with the volume envelope on, a released note plays on through the envelope's release and its voice then
    /// ends; a square volume LFO takes the level to 0 and back; a closed filter quietens a plugin the host filters.
    /// Ignored: native plugins can pop modal message boxes, so run it on its own (hidden desktop).
    #[test]
    #[ignore]
    #[cfg(windows)]
    fn native_generator_follows_the_channel_voicing() {
        use os_dsp::articulator::{knob_ticks, Record};
        use os_engine::instrument::{ChannelVoicing, Instrument};
        use os_host::Plugin as _;
        let Some(fl) = fl() else { return };
        let Some(dll) = find_plugin(&fl, "3x Osc") else { return };
        let rate = 48_000u32;
        let plugin = Plugin::load(&dll, &fl, rate, DEFAULT_BLOCK).expect("load 3x Osc");
        let voice_render = plugin.info.uses_voice_render();
        println!("3x Osc flags {:#x}: UseSampler {voice_render}", plugin.info.flags);
        let desc = os_host::Descriptor {
            format: os_host::Format::Native,
            id: "3x Osc".into(),
            name: "3x Osc".into(),
            vendor: String::new(),
            version: String::new(),
            features: Vec::new(),
            kind: os_host::Kind::Instrument,
            path: dll.clone(),
        };
        let mut p = adapter::NativePlugin::new(plugin, desc);
        assert!(p.activate(rate));
        // Ticks at the adapter's 130 BPM and PPQ 96: the voicing each tick, a note held for 48 ticks; per tick the
        // output's RMS and the voice count.
        let spt = f64::from(rate) * 60.0 / 130.0 / 96.0;
        let mut play = |v: Option<ChannelVoicing>, ticks: usize| -> Vec<(f64, usize)> {
            let mut out = Vec::new();
            let mut at = 0.0f64;
            for t in 0..ticks {
                if let Some(v) = &v {
                    p.channel_voicing(v);
                }
                if t == 0 {
                    p.note_on(0, 60, 100.0 / 128.0);
                }
                if t == 48 {
                    p.note_off(0, 60);
                }
                let n = ((at + spt).floor() - at.floor()) as usize;
                at += spt;
                let (mut l, mut r) = (vec![0.0f32; n], vec![0.0f32; n]);
                p.process(rate, &mut l, &mut r);
                let rms = (l.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / n as f64).sqrt();
                out.push((rms, Instrument::voices(&p)));
            }
            p.reset();
            out
        };
        let last = |run: &[(f64, usize)]| run.iter().rposition(|t| t.1 > 0).unwrap_or(0);
        let energy = |run: &[(f64, usize)]| run.iter().map(|t| t.0 * t.0).sum::<f64>();
        let plain = play(None, 300);
        println!("plain: rms {:.4} at tick 20, voice until tick {}", plain[20].0, last(&plain));
        assert!(plain[20].0 > 0.01, "the note sounds");
        let neutral = play(Some(ChannelVoicing::default()), 300);
        println!("new channel's settings: voice until tick {}", last(&neutral));
        let (a, b) = (energy(&plain[..48]), energy(&neutral[..48]));
        assert!((a - b).abs() <= 0.01 * a, "a new channel's settings change nothing: {a} vs {b}");
        assert_eq!(last(&plain), last(&neutral));

        let mut env = ChannelVoicing::default();
        let vol = &mut env.records[Record::VOLUME];
        (vol.on, vol.attack, vol.hold, vol.sustain, vol.release) = (true, 100, 100, 128, 20000);
        let release = knob_ticks(20000, 96, 130.0, false) as usize;
        let run = play(Some(env), 300);
        println!(
            "volume envelope (release {release} ticks): rms {:.4} at tick 20, {:.4} at tick 78, voice until tick {}",
            run[20].0,
            run[78].0,
            last(&run)
        );
        assert!((run[20].0 - plain[20].0).abs() < 0.05 * plain[20].0, "held at full level");
        assert!(run[48 + 30].0 > 0.05 * plain[20].0, "the release goes on after the note off");
        assert!(last(&run) >= 48 + release - 2 && last(&run) <= 48 + release + 4, "the voice ends after the release: {}", last(&run));

        let mut lfo = ChannelVoicing::default();
        let vol = &mut lfo.records[Record::VOLUME];
        (vol.lfo_amount, vol.lfo_shape, vol.lfo_speed, vol.lfo_attack) = (128, 2, 8700, 0);
        let run = play(Some(lfo), 60);
        let held: Vec<f64> = run[10..48].iter().map(|t| t.0).collect();
        let (lo, hi) = (held.iter().cloned().fold(f64::MAX, f64::min), held.iter().cloned().fold(0.0, f64::max));
        println!("square volume LFO: rms {lo:.4} .. {hi:.4}");
        assert!(lo < 0.2 * hi && hi > 1.5 * plain[20].0, "{lo} {hi}");

        let closed = ChannelVoicing { cutoff: 30, ..ChannelVoicing::default() };
        let run = play(Some(closed), 40);
        let (open, shut) = (energy(&plain[5..40]), energy(&run[5..40]));
        println!("closed filter: energy {shut:.4} vs {open:.4}");
        if voice_render {
            assert!(shut < 0.2 * open, "the host filters its voices");
        }
    }

    #[test]
    fn dll_files_lists_plugins_when_fl_present() {
        let Some(_) = fl() else { return };
        assert!(!dll_files().is_empty(), "expected native plugin DLLs");
    }
}
