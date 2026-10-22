// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! VST 3 plugin hosting: loads `.vst3` modules, lists their audio classes and runs instances behind Lemon Studio's
//! format-neutral hosting layer (`os_host::Library` / `os_host::Plugin`), like `os-clap` does for CLAP.
//!
//! Only the documented VST 3 interfaces are used (bindings: `vst3-sys`). Threading follows `os_host`: the UI thread
//! creates, activates, edits and saves an instance; the audio thread only calls the `Instrument` / `HostedEffect`
//! methods, under the instance's mutex.
// spec: plugins/vst3-hosting.md

mod host;
pub mod preset;

use std::ffi::c_char;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Instant;

pub use os_host::{set_waker, wake, Descriptor};

use host::{from_utf16, null_shared, null_static, Com, EventList, Frame, Handler, HostApp, ParamChanges, Shared, Stream};
use vst3_sys::base::{kResultOk, kResultTrue, IPluginBase, IPluginFactory, IPluginFactory2, IPluginFactory3, PClassInfo, PClassInfo2, PClassInfoW, PFactoryInfo};
use vst3_sys::gui::{IPlugView, IPlugViewContentScaleSupport, ViewRect};
use vst3_sys::utils::SharedVstPtr;
use vst3_sys::vst::{
    AudioBusBuffers, BusInfo, Event, EventData, IAudioProcessor, IComponent, IConnectionPoint, IEditController, IMidiMapping, IUnitInfo,
    NoteExpressionValueEvent, NoteOffEvent, NoteOnEvent, ParameterInfo, ProcessContext, ProcessData, ProcessSetup, UnitInfo,
};
use vst3_sys::{c_void, ComInterface, VstPtr, IID};

/// Largest block handed to a plug-in; longer host blocks are split.
const MAX_FRAMES: u32 = 4096;

/// Most automation values waiting for a process call (the audio thread never grows the queue).
const MAX_AUTO: usize = 2048;

// Media types, bus directions (spec §4).
const AUDIO: i32 = 0;
const EVENT: i32 = 1;
const INPUT: i32 = 0;
const OUTPUT: i32 = 1;
const STEREO: u64 = 3;

// `restartComponent` flags (spec §7).
const RELOAD_COMPONENT: i32 = 1;
const IO_CHANGED: i32 = 2;
const LATENCY_CHANGED: i32 = 8;
const PARAM_TITLES_CHANGED: i32 = 16;

// `ProcessContext::state` flags (spec §5).
const PLAYING: u32 = 1 << 1;
const CYCLE_ACTIVE: u32 = 1 << 2;
const PROJECT_TIME_MUSIC_VALID: u32 = 1 << 9;
const TEMPO_VALID: u32 = 1 << 10;
const BAR_POSITION_VALID: u32 = 1 << 11;
const CYCLE_VALID: u32 = 1 << 12;
const TIME_SIG_VALID: u32 = 1 << 13;
const CONT_TIME_VALID: u32 = 1 << 17;

/// `getTailSamples`: the plug-in sounds forever.
const INFINITE_TAIL: u32 = u32::MAX;

/// Parameter flags (spec §7).
const CAN_AUTOMATE: i32 = 1;
const IS_READ_ONLY: i32 = 2;
const IS_LIST: i32 = 8;
const IS_HIDDEN: i32 = 16;
const IS_BYPASS: i32 = 0x10000;

fn debug() -> bool {
    os_host::debug()
}

/// The standard VST 3 folders (per user, then system) of the platform (spec §2).
pub fn standard_dirs() -> Vec<PathBuf> {
    let mut v = Vec::new();
    #[cfg(windows)]
    {
        if let Some(l) = std::env::var_os("LOCALAPPDATA") {
            v.push(PathBuf::from(l).join("Programs").join("Common").join("VST3"));
        }
        if let Some(c) = std::env::var_os("COMMONPROGRAMFILES") {
            v.push(PathBuf::from(c).join("VST3"));
        }
    }
    #[cfg(target_os = "macos")]
    {
        if let Some(h) = std::env::var_os("HOME") {
            v.push(PathBuf::from(h).join("Library/Audio/Plug-Ins/VST3"));
        }
        v.push(PathBuf::from("/Library/Audio/Plug-Ins/VST3"));
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if let Some(h) = std::env::var_os("HOME") {
            v.push(PathBuf::from(h).join(".vst3"));
        }
        v.push(PathBuf::from("/usr/lib/vst3"));
        v.push(PathBuf::from("/usr/local/lib/vst3"));
    }
    v
}

fn is_vst3(p: &Path) -> bool {
    p.extension().is_some_and(|x| x.eq_ignore_ascii_case("vst3"))
}

/// The plug-ins in the given folders, searched recursively: bundle folders (`Name.vst3\`, not searched inside) and
/// single-file modules (`Name.vst3`).
pub fn find_bundles(dirs: &[PathBuf]) -> Vec<PathBuf> {
    fn walk(d: &Path, depth: u32, out: &mut Vec<PathBuf>) {
        let Ok(rd) = std::fs::read_dir(d) else { return };
        let mut entries: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
        entries.sort();
        for p in entries {
            if is_vst3(&p) && (p.is_dir() || p.is_file()) {
                out.push(p);
            } else if p.is_dir() && depth < 8 {
                walk(&p, depth + 1, out);
            }
        }
    }
    let mut out = Vec::new();
    for d in dirs {
        walk(d, 0, &mut out);
    }
    out
}

/// The window system editors are embedded in (`IPlugView` platform type): an HWND on Windows, an NSView on macOS, an X11
/// window id elsewhere (under Wayland too, through XWayland).
const PLATFORM_TYPE: &std::ffi::CStr = if cfg!(windows) {
    c"HWND"
} else if cfg!(target_os = "macos") {
    c"NSView"
} else {
    c"X11EmbedWindowID"
};

/// The bundle folder holding this platform's module: `Contents/<architecture>-<platform>` (spec §2), `MacOS` on macOS.
pub const BUNDLE_ARCH: &str = if cfg!(target_os = "macos") {
    "MacOS"
} else if cfg!(windows) {
    if cfg!(target_arch = "aarch64") { "arm64-win" } else if cfg!(target_arch = "x86") { "x86-win" } else { "x86_64-win" }
} else if cfg!(target_arch = "aarch64") {
    "aarch64-linux"
} else if cfg!(target_arch = "x86") {
    "i386-linux"
} else {
    "x86_64-linux"
};

/// The module of a plug-in path: inside a bundle `Contents/<BUNDLE_ARCH>/` the bundle's own name on Windows, its name
/// without `.vst3` plus `.so` on Linux, and on macOS the executable its `Info.plist` names (else its name without
/// `.vst3`); else the file itself.
pub fn module_file(path: &Path) -> PathBuf {
    if !path.is_dir() {
        return path.to_path_buf();
    }
    #[cfg(target_os = "macos")]
    return os_host::bundle::module_file(path);
    #[cfg(not(target_os = "macos"))]
    {
        let dir = path.join("Contents").join(BUNDLE_ARCH);
        if cfg!(windows) {
            return dir.join(path.file_name().unwrap_or_default());
        }
        let mut name = path.file_stem().unwrap_or_default().to_os_string();
        name.push(".so");
        dir.join(name)
    }
}

/// A class id as the documented 32-character FUID string (spec §3), uppercase hex. The string is the same on every
/// platform; the bytes in memory are not: on Windows a class id is a COM GUID (Data1, Data2, Data3 little-endian
/// numbers, then 8 bytes), elsewhere the 16 bytes are in string order.
pub fn fuid_string(id: &IID) -> String {
    let d = &id.data;
    let ordered: [u8; 16] = if cfg!(windows) {
        [d[3], d[2], d[1], d[0], d[5], d[4], d[7], d[6], d[8], d[9], d[10], d[11], d[12], d[13], d[14], d[15]]
    } else {
        *d
    };
    ordered.iter().map(|b| format!("{b:02X}")).collect()
}

/// A NUL-terminated `char8` array.
fn c_arr(a: &[c_char]) -> String {
    let bytes: Vec<u8> = a.iter().take_while(|&&c| c != 0).map(|&c| c as u8).collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Instrument / effect from a class's subcategories (spec §3).
fn kind_of(features: &[String]) -> os_host::Kind {
    if features.iter().any(|f| f.starts_with("Instrument")) {
        os_host::Kind::Instrument
    } else if features.iter().any(|f| f.starts_with("Fx")) {
        os_host::Kind::Effect
    } else {
        os_host::Kind::Other
    }
}

/// A `SharedVstPtr` parameter for an interface we hold.
fn shared_of<I: ComInterface + ?Sized>(p: &VstPtr<I>) -> SharedVstPtr<I> {
    // SAFETY: `SharedVstPtr` is a transparent wrapper of the interface pointer; the callee adds its own reference.
    unsafe { std::mem::transmute_copy(&p.as_ptr()) }
}

/// An interface pointer a call handed out with a reference for us.
unsafe fn owned<I: ComInterface + ?Sized>(p: *mut c_void) -> Option<VstPtr<I>> {
    unsafe { VstPtr::owned(p as *mut *mut _) }
}

/// A loaded VST 3 module (one `.vst3` bundle or file) and the audio classes it offers.
pub struct Module {
    path: PathBuf,
    plugins: Vec<Descriptor>,
    /// Class ids, parallel to `plugins`.
    classes: Vec<IID>,
    factory: Option<VstPtr<dyn IPluginFactory>>,
    host: Com<HostApp>,
    lib: Option<libloading::Library>,
    /// An address in the module's code (its factory's method table): which module the crash guard stops when it
    /// crashes.
    code: usize,
}

// SAFETY: the factory and host objects are only used from the UI thread (creating instances); VST 3 factories may be
// called from any thread.
unsafe impl Send for Module {}
unsafe impl Sync for Module {}

impl Module {
    /// Loads a plug-in (bundle folder or module file) and lists its audio classes (spec §3).
    pub fn load(path: &Path) -> Result<Arc<Module>, String> {
        let file = module_file(path);
        unsafe {
            let lib = os_host::load_module(&file)?;
            let lib = module_entry(lib, path).map_err(|e| format!("{}: {e}", file.display()))?;
            let get = lib
                .get::<unsafe extern "system" fn() -> *mut c_void>(b"GetPluginFactory\0")
                .map_err(|_| format!("{}: not a VST 3 module (no GetPluginFactory)", file.display()))?;
            let factory = get();
            Module::from_factory(path, factory, Some(lib))
        }
    }

    /// The module of a factory (`GetPluginFactory`'s result, whose reference it takes over) and its audio classes.
    /// `lib` is the loaded module file, unloaded after the factory is released.
    pub(crate) unsafe fn from_factory(path: &Path, factory: *mut c_void, lib: Option<libloading::Library>) -> Result<Arc<Module>, String> {
        let file = module_file(path);
        unsafe {
            let code = if factory.is_null() { 0 } else { *(factory as *const usize) };
            let factory = owned::<dyn IPluginFactory>(factory);
            let mut m = Module { path: path.to_path_buf(), plugins: Vec::new(), classes: Vec::new(), factory: None, host: HostApp::create(), lib, code };
            let Some(factory) = factory else { return Err(format!("{}: no plug-in factory", file.display())) };
            if let Some(f3) = factory.cast::<dyn IPluginFactory3>() {
                f3.set_host_context(m.host.raw());
            }
            let mut finfo: PFactoryInfo = std::mem::zeroed();
            let fvendor = if factory.get_factory_info(&mut finfo) == kResultOk { c_arr(&finfo.vendor) } else { String::new() };
            let f2 = factory.cast::<dyn IPluginFactory2>();
            let f3 = factory.cast::<dyn IPluginFactory3>();
            for i in 0..factory.count_classes() {
                let mut ci: PClassInfo = std::mem::zeroed();
                if factory.get_class_info(i, &mut ci) != kResultOk || c_arr(&ci.category) != "Audio Module Class" {
                    continue;
                }
                let (mut name, mut vendor, mut version, mut subcats) = (c_arr(&ci.name), fvendor.clone(), String::new(), String::new());
                if let Some(f2) = &f2 {
                    let mut c2: PClassInfo2 = std::mem::zeroed();
                    if f2.get_class_info2(i, &mut c2) == kResultOk {
                        subcats = c_arr(&c2.subcategories);
                        version = c_arr(&c2.version);
                        if c2.vendor[0] != 0 {
                            vendor = c_arr(&c2.vendor);
                        }
                    }
                }
                if let Some(f3) = &f3 {
                    let mut cw: PClassInfoW = std::mem::zeroed();
                    if f3.get_class_info_unicode(i, &mut cw) == kResultOk {
                        let n = from_utf16(&cw.name);
                        if !n.is_empty() {
                            name = n;
                        }
                        let v = from_utf16(&cw.vendor);
                        if !v.is_empty() {
                            vendor = v;
                        }
                    }
                }
                let features: Vec<String> = subcats.split('|').filter(|s| !s.is_empty()).map(str::to_owned).collect();
                m.plugins.push(Descriptor {
                    format: os_host::Format::Vst3,
                    id: fuid_string(&ci.cid),
                    name,
                    vendor,
                    version,
                    kind: kind_of(&features),
                    features,
                    path: path.to_path_buf(),
                });
                m.classes.push(ci.cid);
            }
            m.factory = Some(factory);
            Ok(Arc::new(m))
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn plugins(&self) -> &[Descriptor] {
        &self.plugins
    }

    fn factory(&self) -> &VstPtr<dyn IPluginFactory> {
        self.factory.as_ref().expect("factory lives as long as the module")
    }

    /// Creates class `iid`-interface object `cid`.
    unsafe fn create<I: ComInterface + ?Sized>(&self, cid: &IID) -> Option<VstPtr<I>> {
        let mut obj: *mut c_void = std::ptr::null_mut();
        let r = unsafe { self.factory().create_instance(cid, &I::IID, &mut obj) };
        if r != kResultOk {
            return None;
        }
        unsafe { owned::<I>(obj) }
    }
}

impl Drop for Module {
    fn drop(&mut self) {
        // Every instance holds an `Arc` of its module, so all objects are gone: release the factory, then let the
        // module clean up before it is unloaded. A module whose code crashed is left loaded, as the crash left it.
        let guard = os_host::guard::module_of(self.code as *const c_void);
        if guard.crashed().is_none() {
            let (factory, lib) = (&mut self.factory, &self.lib);
            let _ = os_host::guard::call(&guard, || {
                *factory = None;
                if let Some(lib) = lib {
                    unsafe { module_exit(lib) };
                }
            });
        }
        if guard.crashed().is_some() {
            std::mem::forget(self.factory.take());
            std::mem::forget(self.lib.take());
        }
    }
}

/// The module's own set-up after loading, before `GetPluginFactory` (spec §3): `InitDll` on Windows, `ModuleEntry`
/// with the module's `dlopen` handle on Linux, `bundleEntry` with the bundle (`path`, a `CFBundleRef`) on macOS. All
/// are optional; a module whose set-up fails is not used.
#[cfg(windows)]
unsafe fn module_entry(lib: libloading::Library, _path: &Path) -> Result<libloading::Library, String> {
    unsafe {
        if let Ok(init) = lib.get::<unsafe extern "system" fn() -> bool>(b"InitDll\0") {
            if !init() {
                return Err("InitDll failed".into());
            }
        }
    }
    Ok(lib)
}

#[cfg(all(unix, not(target_os = "macos")))]
unsafe fn module_entry(lib: libloading::Library, _path: &Path) -> Result<libloading::Library, String> {
    unsafe {
        let handle = libloading::os::unix::Library::from(lib).into_raw();
        let lib: libloading::Library = libloading::os::unix::Library::from_raw(handle).into();
        if let Ok(entry) = lib.get::<unsafe extern "C" fn(*mut c_void) -> bool>(b"ModuleEntry\0") {
            if !entry(handle) {
                return Err("ModuleEntry failed".into());
            }
        }
        Ok(lib)
    }
}

/// The module keeps its own reference to the bundle (the SDK's `bundleEntry` retains it until `bundleExit`).
#[cfg(target_os = "macos")]
unsafe fn module_entry(lib: libloading::Library, path: &Path) -> Result<libloading::Library, String> {
    unsafe {
        if let Ok(entry) = lib.get::<unsafe extern "C" fn(*mut c_void) -> bool>(b"bundleEntry\0") {
            let bundle = os_host::bundle::Bundle::open(path).ok_or("not a bundle")?;
            if !entry(bundle.as_ptr()) {
                return Err("bundleEntry failed".into());
            }
        }
        Ok(lib)
    }
}

/// The module's clean-up before it is unloaded: `ExitDll` on Windows, `ModuleExit` on Linux, `bundleExit` on macOS.
unsafe fn module_exit(lib: &libloading::Library) {
    unsafe {
        #[cfg(windows)]
        if let Ok(exit) = lib.get::<unsafe extern "system" fn() -> bool>(b"ExitDll\0") {
            exit();
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        if let Ok(exit) = lib.get::<unsafe extern "C" fn() -> bool>(b"ModuleExit\0") {
            exit();
        }
        #[cfg(target_os = "macos")]
        if let Ok(exit) = lib.get::<unsafe extern "C" fn() -> bool>(b"bundleExit\0") {
            exit();
        }
    }
}

impl os_host::Library for Module {
    fn path(&self) -> &Path {
        &self.path
    }

    fn plugins(&self) -> &[Descriptor] {
        &self.plugins
    }

    fn create(self: Arc<Self>, id: &str) -> Result<os_host::SharedPlugin, String> {
        let guard = os_host::guard::module_of(self.code as *const c_void);
        let name = self.plugins.iter().find(|d| d.id.eq_ignore_ascii_case(id)).map_or(id, |d| d.name.as_str());
        let plugin = os_host::guard::call(&guard, || Plugin::new(&self, id)).map_err(|c| os_host::guard::stopped_text(name, c))??;
        Ok(os_host::share(plugin, guard))
    }
}

/// What a queued note event does.
#[derive(Clone, Copy, PartialEq)]
enum NoteKind {
    On,
    Off,
    /// Tuning of the key's sounding note, `velocity` = semitones.
    Tune,
}

/// A note event waiting for the next `process` call.
#[derive(Clone, Copy)]
struct Note {
    time: u32,
    kind: NoteKind,
    key: u8,
    velocity: f32,
}

// Event types and the tuning note expression (spec §5).
const NOTE_ON_EVENT: u16 = 0;
const NOTE_OFF_EVENT: u16 = 1;
const NOTE_EXPRESSION_VALUE_EVENT: u16 = 4;
const TUNING_TYPE_ID: u32 = 2;

/// A running VST 3 instance.
pub struct Plugin {
    // Interface pointers first: they are released (in this order) before the module.
    view: Option<VstPtr<dyn IPlugView>>,
    /// (component's, controller's) connection points of a separate controller, connected to each other.
    points: Option<(VstPtr<dyn IConnectionPoint>, VstPtr<dyn IConnectionPoint>)>,
    controller: Option<VstPtr<dyn IEditController>>,
    processor: VstPtr<dyn IAudioProcessor>,
    component: VstPtr<dyn IComponent>,
    /// The controller is its own object (initialised and terminated by the host).
    separate: bool,
    handler: Com<Handler>,
    frame: Com<Frame>,
    events: Com<EventList>,
    params_in: Com<ParamChanges>,
    params_out: Com<ParamChanges>,
    shared: Arc<Shared>,
    desc: Descriptor,
    editor: bool,
    /// Device pixels per unit of the open editor's sizes: 1, except on macOS, where editors measure in points
    /// (`open_editor`).
    editor_scale: f32,
    /// The plug-in can show an editor in a host window.
    can_edit: bool,
    /// Active at this rate (0 = inactive).
    rate: u32,
    wanted_rate: AtomicU32,
    processing: bool,
    max_frames: u32,
    /// Channel count of each audio input / output bus, and whether the plug-in takes notes.
    ins: Vec<usize>,
    outs: Vec<usize>,
    takes_notes: bool,
    /// Zeroed channels for inputs the host has no audio for, and output channels it does not use.
    scratch: Vec<f32>,
    ptrs: Vec<*mut c_void>,
    bus_in: Vec<AudioBusBuffers>,
    bus_out: Vec<AudioBusBuffers>,
    queue: Vec<Note>,
    note_ids: NoteIds,
    fx_in: (Vec<f32>, Vec<f32>),
    /// Values the processor reported, for the controller on the next idle.
    from_processor: Vec<(u32, f64)>,
    /// Host / editor edits have waited for a `process` call since then (a flush is due without a stream).
    pending_since: Option<Instant>,
    /// (id, step count) of each parameter in list order: the position is the automation index. A parameter whose
    /// info could not be read keeps its place with id `u32::MAX` (VST 3's "no parameter").
    param_ids: Vec<(u32, i32)>,
    /// The parameter the controller maps each MIDI controller to (IMidiMapping, MIDI channel 1): controllers 0..127,
    /// then channel pressure (128) and pitch bend (129); `u32::MAX` = none (spec: ui/midi-settings.md §4).
    midi_map: Vec<u32>,
    /// Automation values (frame offset, id, normalized) for the next process call.
    auto: Vec<(u32, u32, f64)>,
    /// Unit id -> group path (IUnitInfo).
    units: Vec<(i32, String)>,
    latency: u32,
    tail: u32,
    sounding: bool,
    /// The voices it plays as far as the host can tell: VST 3 reports no voice ends, so a released voice lasts the
    /// stated tail or until the output is quiet (spec: audio/voices.md §2.5-2.6).
    ledger: os_engine::voices::VoiceLedger,
    steady: i64,
    context: ProcessContext,
    /// The song position for the next process call (the engine gives it while playing).
    song: Option<os_engine::TransportInfo>,
    dbg_peak: f32,
    dbg_frames: u64,
    _module: Arc<Module>,
}

// SAFETY: the instance is used behind a mutex (`os_host::SharedPlugin`); main-thread calls come from the UI thread only.
unsafe impl Send for Plugin {}

impl Plugin {
    /// Creates and initialises class `id` of `module` (spec §4.1-4.4); buses are set up, not active yet.
    pub fn new(module: &Arc<Module>, id: &str) -> Result<Plugin, String> {
        let i = module.plugins.iter().position(|d| d.id.eq_ignore_ascii_case(id)).ok_or_else(|| format!("no plug-in {id} in {}", module.path.display()))?;
        let desc = module.plugins[i].clone();
        let cid = module.classes[i];
        let host = module.host.raw();
        unsafe {
            let component = module.create::<dyn IComponent>(&cid).ok_or_else(|| format!("{}: could not create the component", desc.name))?;
            if component.initialize(host) != kResultOk {
                return Err(format!("{}: the component did not initialise", desc.name));
            }
            let Some(processor) = component.cast::<dyn IAudioProcessor>() else {
                component.terminate();
                return Err(format!("{}: no audio processor", desc.name));
            };
            // Single-component plug-ins implement the controller on the component; others name a controller class.
            let (controller, separate) = match component.cast::<dyn IEditController>() {
                Some(c) => (Some(c), false),
                None => {
                    let mut ccid: IID = std::mem::zeroed();
                    let c = if component.get_controller_class_id(&mut ccid) == kResultOk { module.create::<dyn IEditController>(&ccid) } else { None };
                    match c {
                        Some(c) if c.initialize(host) == kResultOk => (Some(c), true),
                        _ => (None, false),
                    }
                }
            };
            let shared = Arc::new(Shared::default());
            let mut p = Plugin {
                view: None,
                points: None,
                controller,
                processor,
                component,
                separate,
                handler: Handler::create(shared.clone()),
                frame: Frame::create(shared.clone()),
                events: EventList::create(),
                params_in: ParamChanges::create(),
                params_out: ParamChanges::create(),
                shared,
                desc,
                editor: false,
                editor_scale: 1.0,
                can_edit: false,
                rate: 0,
                wanted_rate: AtomicU32::new(0),
                processing: false,
                max_frames: MAX_FRAMES,
                ins: Vec::new(),
                outs: Vec::new(),
                takes_notes: false,
                scratch: Vec::new(),
                ptrs: Vec::new(),
                bus_in: Vec::new(),
                bus_out: Vec::new(),
                queue: Vec::with_capacity(256),
                note_ids: NoteIds { ids: [-1; 128], next: 0 },
                fx_in: (Vec::new(), Vec::new()),
                from_processor: Vec::with_capacity(256),
                pending_since: None,
                param_ids: Vec::new(),
                midi_map: Vec::new(),
                auto: Vec::with_capacity(MAX_AUTO),
                units: Vec::new(),
                latency: 0,
                tail: 0,
                sounding: false,
                ledger: os_engine::voices::VoiceLedger::new(),
                steady: 0,
                context: ProcessContext::default(),
                song: None,
                dbg_peak: 0.0,
                dbg_frames: 0,
                _module: module.clone(),
            };
            if let Some(c) = &p.controller {
                if separate {
                    if let (Some(a), Some(b)) = (p.component.cast::<dyn IConnectionPoint>(), c.cast::<dyn IConnectionPoint>()) {
                        a.connect(shared_of(&b));
                        b.connect(shared_of(&a));
                        p.points = Some((a, b));
                    }
                }
                c.set_component_handler(p.handler.shared());
                if separate {
                    // The controller starts from the processor's state.
                    let s = Stream::create(Vec::new());
                    if p.component.get_state(s.shared()) == kResultOk {
                        s.get().rewind();
                        c.set_component_state(s.shared());
                    }
                }
                // An editor that can live in a host window.
                if let Some(v) = owned::<dyn IPlugView>(c.create_view(c"editor".as_ptr())) {
                    p.can_edit = v.is_platform_type_supported(PLATFORM_TYPE.as_ptr()) == kResultTrue;
                }
                p.read_units();
            }
            p.refresh_param_ids();
            p.setup_buses();
            Ok(p)
        }
    }

    pub fn desc(&self) -> &Descriptor {
        &self.desc
    }

    /// Bus arrangements and activation (spec §4.5): stereo main buses, the first event input.
    fn setup_buses(&mut self) {
        unsafe {
            let c = &self.component;
            let n_in = c.get_bus_count(AUDIO, INPUT).max(0) as usize;
            let n_out = c.get_bus_count(AUDIO, OUTPUT).max(0) as usize;
            let mut arr_in = vec![STEREO; n_in.max(1)];
            let mut arr_out = vec![STEREO; n_out.max(1)];
            for (dir, arr, n) in [(INPUT, &mut arr_in, n_in), (OUTPUT, &mut arr_out, n_out)] {
                for (b, a) in arr.iter_mut().enumerate().take(n).skip(1) {
                    let mut cur = STEREO;
                    if self.processor.get_bus_arrangement(dir, b as i32, &mut cur) == kResultOk {
                        *a = cur;
                    }
                }
            }
            let r = self.processor.set_bus_arrangements(arr_in.as_mut_ptr(), n_in as i32, arr_out.as_mut_ptr(), n_out as i32);
            if debug() && r != kResultOk {
                eprintln!("[os-vst3] {} keeps its own bus arrangement", self.desc.name);
            }
            if n_in > 0 {
                c.activate_bus(AUDIO, INPUT, 0, 1);
            }
            if n_out > 0 {
                c.activate_bus(AUDIO, OUTPUT, 0, 1);
            }
            self.takes_notes = c.get_bus_count(EVENT, INPUT) > 0;
            if self.takes_notes {
                c.activate_bus(EVENT, INPUT, 0, 1);
            }
            let channels = |dir: i32, n: usize| -> Vec<usize> {
                (0..n)
                    .map(|b| {
                        let mut info: BusInfo = std::mem::zeroed();
                        if c.get_bus_info(AUDIO, dir, b as i32, &mut info) == kResultOk { info.channel_count.max(0) as usize } else { 2 }
                    })
                    .collect()
            };
            self.ins = channels(INPUT, n_in);
            self.outs = channels(OUTPUT, n_out);
        }
        let total: usize = self.ins.iter().chain(&self.outs).sum();
        self.scratch = vec![0.0; total * self.max_frames as usize];
        self.ptrs = Vec::with_capacity(total);
        self.bus_in = Vec::with_capacity(self.ins.len());
        self.bus_out = Vec::with_capacity(self.outs.len());
        // An effect's input copy, so the audio thread does not allocate.
        let max = self.max_frames as usize;
        self.fx_in = (Vec::with_capacity(max), Vec::with_capacity(max));
    }

    /// Group names of the controller's units (IUnitInfo): parent path, root unit left out.
    fn read_units(&mut self) {
        let Some(u) = self.controller.as_ref().and_then(|c| c.cast::<dyn IUnitInfo>()) else { return };
        let mut raw: Vec<(i32, i32, String)> = Vec::new();
        unsafe {
            for i in 0..u.get_unit_count() {
                let mut info: UnitInfo = std::mem::zeroed();
                if u.get_unit_info(i, &mut info) == kResultOk {
                    raw.push((info.id, info.parent_unit_id, from_utf16(&info.name)));
                }
            }
        }
        self.units = raw
            .iter()
            .map(|(id, _, _)| {
                let mut parts = Vec::new();
                let mut at = *id;
                for _ in 0..16 {
                    let Some((_, parent, name)) = raw.iter().find(|u| u.0 == at) else { break };
                    if at == 0 {
                        break;
                    }
                    parts.push(name.clone());
                    at = *parent;
                }
                parts.reverse();
                (*id, parts.join("/"))
            })
            .collect();
    }

    fn known(&self, id: u32) -> bool {
        id != u32::MAX && self.param_ids.iter().any(|p| p.0 == id)
    }

    fn refresh_param_ids(&mut self) {
        self.param_ids.clear();
        let Some(c) = &self.controller else { return };
        unsafe {
            for i in 0..c.get_parameter_count() {
                let mut info: ParameterInfo = std::mem::zeroed();
                let ok = c.get_parameter_info(i, &mut info) == kResultOk;
                self.param_ids.push(if ok { (info.id, info.step_count.max(0)) } else { (u32::MAX, 0) });
            }
        }
        self.midi_map.clear();
        if let Some(m) = c.cast::<dyn IMidiMapping>() {
            for cc in 0..130i16 {
                let mut id = u32::MAX;
                let ok = unsafe { m.get_midi_controller_assignment(0, 0, cc, &mut id) } == kResultOk;
                self.midi_map.push(if ok { id } else { u32::MAX });
            }
        }
    }

    /// Activates at `rate` (UI thread; spec §4.5); reactivates when the rate changes.
    pub fn activate(&mut self, rate: u32) -> bool {
        if self.rate == rate {
            return true;
        }
        self.deactivate();
        let setup = ProcessSetup { process_mode: 0, symbolic_sample_size: 0, max_samples_per_block: self.max_frames as i32, sample_rate: f64::from(rate) };
        unsafe {
            if self.processor.setup_processing(&setup) != kResultOk {
                if debug() {
                    eprintln!("[os-vst3] {} refused {rate} Hz", self.desc.name);
                }
                return false;
            }
            if self.component.set_active(1) != kResultOk {
                return false;
            }
            self.latency = self.processor.get_latency_samples();
            self.tail = self.processor.get_tail_samples();
        }
        self.rate = rate;
        self.ledger.clear();
        self.ledger.set_tail(match self.tail {
            0 | INFINITE_TAIL => None,
            t => Some(f64::from(t) / f64::from(rate)),
        });
        self.context = ProcessContext {
            state: CONT_TIME_VALID,
            sample_rate: f64::from(rate),
            // Flagged valid once the engine has given a transport.
            tempo: 120.0,
            time_sig_num: 4,
            time_sig_den: 4,
            ..ProcessContext::default()
        };
        if debug() {
            eprintln!("[os-vst3] {} active at {rate} Hz (latency {}, ins {:?}, outs {:?})", self.desc.name, self.latency, self.ins, self.outs);
        }
        true
    }

    fn deactivate(&mut self) {
        if self.rate == 0 {
            return;
        }
        unsafe {
            if self.processing {
                self.processor.set_processing(0);
                self.processing = false;
            }
            self.component.set_active(0);
        }
        self.rate = 0;
    }

    /// Starts processing before the first block after activation or a reset (spec §4.6).
    fn start_processing(&mut self) {
        if !self.processing {
            // Plug-ins without anything to do here answer kNotImplemented: processing starts all the same.
            unsafe { self.processor.set_processing(1) };
            self.processing = true;
        }
    }

    /// UI-thread housekeeping: restart requests, rate changes, the processor's parameter output, edits waiting for a
    /// stream.
    pub fn idle(&mut self) {
        let flags = self.shared.restart.swap(0, Ordering::AcqRel);
        if flags != 0 && debug() {
            eprintln!("[os-vst3] {} restart flags {flags:#x}", self.desc.name);
        }
        let wanted = self.wanted_rate.swap(0, Ordering::AcqRel);
        if flags & (RELOAD_COMPONENT | IO_CHANGED | LATENCY_CHANGED) != 0 {
            let r = if wanted != 0 { wanted } else { self.rate };
            self.deactivate();
            if flags & (RELOAD_COMPONENT | IO_CHANGED) != 0 {
                self.setup_buses();
            }
            if r != 0 {
                self.activate(r);
            }
        } else if wanted != 0 && wanted != self.rate {
            self.activate(wanted);
        }
        if flags & (RELOAD_COMPONENT | PARAM_TITLES_CHANGED) != 0 {
            self.refresh_param_ids();
        }
        if let Some(c) = &self.controller {
            for (id, v) in self.from_processor.drain(..) {
                unsafe { c.set_param_normalized(id, v) };
            }
        }
        // Edits only reach a processor in `process`: without a stream, flush them with an empty block so the sound
        // and the saved state follow (spec §5).
        let waiting = !self.auto.is_empty() || self.shared.to_processor.lock().is_ok_and(|q| !q.is_empty());
        if !waiting {
            self.pending_since = None;
        } else if self.rate != 0 {
            match self.pending_since {
                None => self.pending_since = Some(Instant::now()),
                Some(t) if t.elapsed().as_millis() >= 50 => {
                    self.flush();
                    self.pending_since = None;
                }
                _ => {}
            }
        }
        // Edits still waiting for their flush: look again on the next tick.
        if self.pending_since.is_some() {
            self.shared.attention.raise();
        }
    }

    /// A `process` call without audio, carrying the waiting parameter changes.
    fn flush(&mut self) {
        self.start_processing();
        self.fill_params();
        // Automation that waited: the newest value of each parameter, now.
        for &(_, id, v) in &self.auto {
            self.params_in.get().add(id, 0, v);
        }
        self.auto.clear();
        self.params_out.get().clear();
        let mut data = ProcessData {
            process_mode: 0,
            symbolic_sample_size: 0,
            num_samples: 0,
            num_inputs: 0,
            num_outputs: 0,
            inputs: std::ptr::null_mut(),
            outputs: std::ptr::null_mut(),
            input_param_changes: self.params_in.static_ptr(),
            output_param_changes: self.params_out.static_ptr(),
            input_events: null_static(),
            output_events: null_static(),
            context: &mut self.context,
        };
        unsafe { self.processor.process(&mut data) };
        self.take_outputs();
    }

    /// Host and editor edits into this block's input parameter changes (offset 0).
    fn fill_params(&mut self) {
        let pin = self.params_in.get();
        pin.clear();
        if let Ok(mut q) = self.shared.to_processor.try_lock() {
            for (id, v) in q.drain(..) {
                pin.add(id, 0, v);
            }
        }
    }

    /// Automation (audio thread): parameter `index` of the list to normalized `value`, `offset` frames into the next
    /// process call (spec §5). A stepped parameter gets its nearest step; unknown indexes are ignored.
    fn automate(&mut self, offset: u32, index: u32, value: f64) {
        let Some(&(id, steps)) = self.param_ids.get(index as usize) else { return };
        if id == u32::MAX || self.auto.len() >= MAX_AUTO {
            return;
        }
        let mut v = value.clamp(0.0, 1.0);
        if steps > 0 {
            v = (v * f64::from(steps)).round() / f64::from(steps);
        }
        self.auto.push((offset, id, v));
        self.shared.attention.raise();
    }

    /// This chunk's automation (offset < `n`) into the input parameter changes, after the host edits at offset 0; the
    /// controller hears the last values too. Later values move on by `n` frames. No allocation.
    fn fill_auto(&mut self, n: u32) {
        let pin = self.params_in.get();
        let out = &mut self.from_processor;
        let mut kept = 0;
        for i in 0..self.auto.len() {
            let (t, id, v) = self.auto[i];
            if t < n {
                pin.add(id, t as i32, v);
                if let Some(e) = out.iter_mut().find(|e| e.0 == id) {
                    e.1 = v;
                } else if out.len() < out.capacity() {
                    out.push((id, v));
                }
            } else {
                self.auto[kept] = (t - n, id, v);
                kept += 1;
            }
        }
        self.auto.truncate(kept);
    }

    /// Parameter values the processor reported, for the controller.
    fn take_outputs(&mut self) {
        let out = &mut self.from_processor;
        self.params_out.get().for_each_last(|id, v| {
            if let Some(e) = out.iter_mut().find(|e| e.0 == id) {
                e.1 = v;
            } else if out.len() < out.capacity() {
                out.push((id, v));
            }
        });
        // The controller hears them on the UI thread.
        if !self.from_processor.is_empty() {
            self.shared.attention.raise();
        }
    }

    pub fn has_editor(&self) -> bool {
        self.can_edit
    }

    /// Embeds the editor in `parent` (an HWND on Windows, an NSView on macOS, an X11 window id on Linux; spec §6); returns
    /// its size in pixels. `scale` is the parent's DPI factor, given to editors that support content scaling (spec:
    /// ui/ui-scaling.md §3). On macOS editors measure in points and follow their window's backing scale themselves (no
    /// content scaling there), so their sizes are turned into pixels at `scale` here.
    pub fn open_editor(&mut self, parent: *mut c_void, scale: f32) -> Result<(u32, u32), String> {
        if !self.can_edit {
            return Err(format!("{} has no editor", self.desc.name));
        }
        self.close_editor();
        self.editor_scale = if cfg!(target_os = "macos") && scale > 0.0 { scale } else { 1.0 };
        let c = self.controller.as_ref().ok_or("no edit controller")?;
        unsafe {
            let view = owned::<dyn IPlugView>(c.create_view(c"editor".as_ptr())).ok_or("the plug-in could not create its editor")?;
            if view.is_platform_type_supported(PLATFORM_TYPE.as_ptr()) != kResultTrue {
                return Err("the editor cannot be shown in a window".into());
            }
            view.set_frame(self.frame.raw());
            // Content scale before attaching, so the editor opens at its final size (optional interface).
            if scale > 0.0 && !cfg!(target_os = "macos") {
                if let Some(s) = view.cast::<dyn IPlugViewContentScaleSupport>() {
                    s.set_scale_factor(scale);
                }
            }
            if view.attached(parent, PLATFORM_TYPE.as_ptr()) != kResultOk {
                view.set_frame(std::ptr::null_mut());
                return Err("the plug-in refused the editor window".into());
            }
            let mut r = ViewRect::default();
            view.get_size(&mut r);
            self.view = Some(view);
            self.editor = true;
            if let Ok(mut s) = self.shared.size.lock() {
                *s = None;
            }
            Ok(self.to_px(r.right - r.left, r.bottom - r.top))
        }
    }

    /// The editor's size in pixels (when open): what it asked for last, else what it reports.
    pub fn editor_size(&self) -> Option<(u32, u32)> {
        let v = self.view.as_ref()?;
        if let Some((w, h)) = self.shared.size.lock().ok().and_then(|s| *s) {
            return Some(self.to_px(w, h));
        }
        let mut r = ViewRect::default();
        (unsafe { v.get_size(&mut r) } == kResultOk).then(|| self.to_px(r.right - r.left, r.bottom - r.top))
    }

    /// An editor size as the plug-in gives it (points on macOS) in pixels.
    fn to_px(&self, w: i32, h: i32) -> (u32, u32) {
        let px = |v: i32| (v.max(0) as f32 * self.editor_scale).round() as u32;
        (px(w), px(h))
    }

    pub fn close_editor(&mut self) {
        if let Some(v) = self.view.take() {
            unsafe {
                v.removed();
                v.set_frame(std::ptr::null_mut());
            }
        }
        self.frame.get().clear_run_loop();
        self.editor = false;
    }

    /// The instance's state as `.vstpreset` bytes (spec §8).
    pub fn save_state(&self) -> Option<Vec<u8>> {
        unsafe {
            let s = Stream::create(Vec::new());
            if self.component.get_state(s.shared()) != kResultOk {
                return None;
            }
            let mut p = preset::Preset { class_id: self.desc.id.clone(), component: s.get().bytes(), controller: Vec::new() };
            // A single-component plug-in's controller is the same object: its state is the component's.
            if let (Some(c), true) = (&self.controller, self.separate) {
                let s = Stream::create(Vec::new());
                if c.get_state(s.shared()) == kResultOk {
                    p.controller = s.get().bytes();
                }
            }
            Some(p.to_bytes())
        }
    }

    /// Restores `save_state` bytes (also accepts a bare component state). A preset of another class is refused.
    pub fn load_state(&mut self, data: &[u8]) -> bool {
        let p = if data.starts_with(b"VST3") {
            match preset::Preset::parse(data) {
                Some(p) if p.class_id.eq_ignore_ascii_case(&self.desc.id) => p,
                _ => return false,
            }
        } else {
            preset::Preset { class_id: self.desc.id.clone(), component: data.to_vec(), controller: Vec::new() }
        };
        unsafe {
            let s = Stream::create(p.component.clone());
            if self.component.set_state(s.shared()) != kResultOk {
                return false;
            }
            if let (Some(c), true) = (&self.controller, self.separate) {
                let s = Stream::create(p.component);
                c.set_component_state(s.shared());
                if !p.controller.is_empty() {
                    let s = Stream::create(p.controller);
                    c.set_state(s.shared());
                }
            }
        }
        true
    }

    /// The process context of a chunk (spec §5): continuous time always; with the engine's transport (`song`, at the
    /// chunk's first frame) also playing, tempo, time signature, musical position, bar start and the loop. Without
    /// one the transport is stopped where it was.
    fn set_context(&mut self, song: Option<os_engine::TransportInfo>) {
        let c = &mut self.context;
        c.continuous_time_samples = self.steady;
        c.state &= !(PLAYING | CYCLE_ACTIVE);
        let Some(t) = song else {
            if c.state & TEMPO_VALID == 0 {
                c.project_time_samples = self.steady;
            }
            return;
        };
        c.state |= CONT_TIME_VALID | TEMPO_VALID | TIME_SIG_VALID | PROJECT_TIME_MUSIC_VALID | BAR_POSITION_VALID;
        if t.playing {
            c.state |= PLAYING;
        }
        c.tempo = t.tempo;
        c.time_sig_num = i32::from(t.numerator);
        c.time_sig_den = i32::from(t.denominator);
        c.project_time_samples = t.frame as i64;
        c.project_time_music = t.beats;
        c.bar_position_music = t.bar_start;
        match t.looping {
            Some((a, b)) => {
                c.state |= CYCLE_ACTIVE | CYCLE_VALID;
                c.cycle_start_music = a;
                c.cycle_end_music = b;
            }
            None => c.state &= !CYCLE_VALID,
        }
    }

    /// Processes up to `max_frames` frames: queued notes inside them, waiting parameter changes, the given audio.
    fn process_chunk(&mut self, left: &mut [f32], right: &mut [f32], input: Option<(&mut [f32], &mut [f32])>) {
        let n = left.len();
        let max = self.max_frames as usize;
        // Notes of this chunk; later ones move on.
        let ev = self.events.get();
        ev.clear();
        let mut kept = 0;
        for i in 0..self.queue.len() {
            let e = self.queue[i];
            if (e.time as usize) < n {
                if self.takes_notes {
                    if let Some(event) = self.note_ids.event(e) {
                        ev.push(event);
                    }
                    match e.kind {
                        NoteKind::On => self.ledger.start(e.key, -1),
                        NoteKind::Off => self.ledger.release(e.key),
                        NoteKind::Tune => {}
                    }
                }
            } else {
                self.queue[kept] = Note { time: e.time - n as u32, ..e };
                kept += 1;
            }
        }
        self.queue.truncate(kept);
        self.fill_params();
        self.fill_auto(n as u32);
        self.params_out.get().clear();
        // Buffers: the main buses carry the host's audio, every other channel a scratch buffer.
        self.ptrs.clear();
        self.bus_in.clear();
        self.bus_out.clear();
        let base = self.scratch.as_mut_ptr();
        let mut k = 0;
        let (il, ir) = match input {
            Some((l, r)) => (l.as_mut_ptr(), r.as_mut_ptr()),
            None => (std::ptr::null_mut(), std::ptr::null_mut()),
        };
        for (b, &ch) in self.ins.iter().enumerate() {
            let start = self.ptrs.len();
            for c in 0..ch {
                let host = if b == 0 && !il.is_null() { [il, ir].get(c).copied() } else { None };
                let p = host.unwrap_or_else(|| {
                    // SAFETY: scratch holds max_frames frames for every channel.
                    let s = unsafe { base.add(k * max) };
                    unsafe { std::ptr::write_bytes(s, 0, n) };
                    s
                });
                k += 1;
                self.ptrs.push(p as *mut c_void);
            }
            // SAFETY: `ptrs` has room for every channel, so it never reallocates while these point into it.
            self.bus_in.push(AudioBusBuffers { num_channels: ch as i32, silence_flags: 0, buffers: unsafe { self.ptrs.as_mut_ptr().add(start) } });
        }
        let (ol, or) = (left.as_mut_ptr(), right.as_mut_ptr());
        for (b, &ch) in self.outs.iter().enumerate() {
            let start = self.ptrs.len();
            for c in 0..ch {
                let host = if b == 0 { [ol, or].get(c).copied() } else { None };
                let p = host.unwrap_or_else(|| unsafe { base.add(k * max) });
                k += 1;
                self.ptrs.push(p as *mut c_void);
            }
            self.bus_out.push(AudioBusBuffers { num_channels: ch as i32, silence_flags: 0, buffers: unsafe { self.ptrs.as_mut_ptr().add(start) } });
        }
        let mut data = ProcessData {
            process_mode: 0,
            symbolic_sample_size: 0,
            num_samples: n as i32,
            num_inputs: self.bus_in.len() as i32,
            num_outputs: self.bus_out.len() as i32,
            inputs: if self.bus_in.is_empty() { std::ptr::null_mut() } else { self.bus_in.as_mut_ptr() },
            outputs: if self.bus_out.is_empty() { std::ptr::null_mut() } else { self.bus_out.as_mut_ptr() },
            input_param_changes: self.params_in.static_ptr(),
            output_param_changes: self.params_out.static_ptr(),
            input_events: if self.takes_notes { self.events.static_ptr() } else { null_static() },
            output_events: null_static(),
            context: &mut self.context,
        };
        unsafe { self.processor.process(&mut data) };
        if self.outs.first() == Some(&1) {
            right.copy_from_slice(left);
        }
        self.take_outputs();
        let loud = left.iter().chain(right.iter()).any(|v| v.abs() > 1e-5);
        self.sounding = self.tail == INFINITE_TAIL || loud;
        self.steady += n as i64;
        // Quiet output ends the released voices (not with an infinite tail: such a plug-in may sound later).
        if !self.sounding {
            self.ledger.quiet(false);
        }
        self.ledger.advance(n as f64 / f64::from(self.rate.max(1)));
    }

    fn debug_level(&mut self, rate: u32, left: &[f32], right: &[f32]) {
        if debug() {
            self.dbg_peak = left.iter().chain(right.iter()).fold(self.dbg_peak, |m, v| m.max(v.abs()));
            self.dbg_frames += left.len() as u64;
            if self.dbg_frames >= u64::from(rate) {
                eprintln!("[os-vst3] {} peak {:.3}", self.desc.name, self.dbg_peak);
                (self.dbg_frames, self.dbg_peak) = (0, 0.0);
            }
        }
    }
}

/// Note id of each key's latest note on (-1 = none), and the next id to give.
struct NoteIds {
    ids: [i32; 128],
    next: i32,
}

impl NoteIds {
    /// The VST 3 event of a queued note (spec §5). A note on gets a note id (so its tuning can address it); a note off
    /// names only its key (id -1), so it ends that key's note as before; a tuning goes to the key's latest note and is
    /// dropped when the key has none.
    fn event(&mut self, n: Note) -> Option<Event> {
        let key = usize::from(n.key.min(127));
        let (type_, event) = match n.kind {
            NoteKind::On => {
                let id = self.next;
                self.next = (id + 1) & 0x7fff_ffff;
                self.ids[key] = id;
                let on = NoteOnEvent { channel: 0, pitch: i16::from(n.key), tuning: 0.0, velocity: n.velocity, length: 0, note_id: id };
                (NOTE_ON_EVENT, EventData { note_on: on })
            }
            NoteKind::Off => {
                (NOTE_OFF_EVENT, EventData { note_off: NoteOffEvent { channel: 0, pitch: i16::from(n.key), velocity: 0.0, note_id: -1, tuning: 0.0 } })
            }
            NoteKind::Tune => {
                let id = self.ids[key];
                if id < 0 {
                    return None;
                }
                // Normalized tuning: 0.5 = none, plain semitones = 240 * (v - 0.5).
                let value = (f64::from(n.velocity) / 240.0 + 0.5).clamp(0.0, 1.0);
                (NOTE_EXPRESSION_VALUE_EVENT, EventData { note_expression_value: NoteExpressionValueEvent { type_id: TUNING_TYPE_ID, note_id: id, value } })
            }
        };
        Some(Event { bus_index: 0, sample_offset: n.time as i32, ppq_position: 0.0, flags: 0, type_, event })
    }
}

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
        self.can_edit
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
        if self.rate == 0 { 0 } else { self.latency }
    }

    fn param_count(&self) -> usize {
        self.controller.as_ref().map_or(0, |c| unsafe { c.get_parameter_count() }.max(0) as usize)
    }

    fn param_info(&self, index: usize) -> Option<os_host::ParamInfo> {
        let c = self.controller.as_ref()?;
        unsafe {
            let mut info: ParameterInfo = std::mem::zeroed();
            if c.get_parameter_info(index as i32, &mut info) != kResultOk {
                return None;
            }
            let f = info.flags;
            Some(os_host::ParamInfo {
                id: info.id,
                name: from_utf16(&info.title),
                unit: from_utf16(&info.units),
                group: self.units.iter().find(|u| u.0 == info.unit_id).map(|u| u.1.clone()).unwrap_or_default(),
                min: c.normalized_param_to_plain(info.id, 0.0),
                max: c.normalized_param_to_plain(info.id, 1.0),
                default: info.default_normalized_value,
                steps: info.step_count.max(0) as u32,
                automatable: f & CAN_AUTOMATE != 0,
                read_only: f & IS_READ_ONLY != 0,
                hidden: f & IS_HIDDEN != 0,
                bypass: f & IS_BYPASS != 0,
                enumerated: f & IS_LIST != 0,
            })
        }
    }

    fn param_value(&self, id: u32) -> Option<f64> {
        let c = self.controller.as_ref()?;
        self.known(id).then(|| unsafe { c.get_param_normalized(id) })
    }

    fn param_text(&self, id: u32, value: f64) -> Option<String> {
        let c = self.controller.as_ref()?;
        let mut buf = [0i16; 128];
        (unsafe { c.get_param_string_by_value(id, value, buf.as_mut_ptr()) } == kResultOk).then(|| from_utf16(&buf))
    }

    fn set_param(&mut self, id: u32, value: f64) -> bool {
        let Some(c) = &self.controller else { return false };
        if !self.known(id) {
            return false;
        }
        let v = value.clamp(0.0, 1.0);
        unsafe { c.set_param_normalized(id, v) };
        // The processor hears it in its next block (spec §7).
        self.shared.push_value(id, v);
        os_host::wake();
        true
    }

    fn take_param_events(&mut self) -> Vec<os_host::ParamEvent> {
        self.shared.events.lock().map(|mut v| std::mem::take(&mut *v)).unwrap_or_default()
    }

    fn attention(&self) -> Option<Arc<os_host::Attention>> {
        Some(self.shared.attention.clone())
    }
}

impl os_engine::Instrument for Plugin {
    fn active(&self) -> bool {
        self.sounding
    }

    fn voices(&self) -> usize {
        self.ledger.count()
    }

    /// Stop (spec §5): queued notes are dropped and processing restarts, which resets the plug-in's voices and tails.
    /// Held notes also get their note offs with the next block, for plug-ins that keep voices across the restart
    /// (spec: audio/voices.md §3). Note offs still waiting for that block stay queued: Stop resets, and Play resets
    /// again before any block has run.
    fn reset(&mut self) {
        self.queue.retain(|n| matches!(n.kind, NoteKind::Off));
        self.sounding = false;
        let mut sent = [false; 128];
        for n in &mut self.queue {
            n.time = 0;
            sent[usize::from(n.key.min(127))] = true;
        }
        let held: Vec<u8> = self.ledger.held().collect();
        for key in held {
            if !std::mem::replace(&mut sent[usize::from(key.min(127))], true) {
                self.queue.push(Note { time: 0, kind: NoteKind::Off, key, velocity: 0.0 });
            }
        }
        self.ledger.clear();
        // Automation still waiting is a value the parameter should have: it is kept for the next block / flush.
        if self.processing {
            unsafe { self.processor.set_processing(0) };
            self.processing = false;
        }
    }

    fn note_on(&mut self, offset: u32, key: u8, velocity: f32) {
        self.queue.push(Note { time: offset, kind: NoteKind::On, key, velocity: velocity.clamp(0.0, 1.0) });
    }

    fn note_off(&mut self, offset: u32, key: u8) {
        self.queue.push(Note { time: offset, kind: NoteKind::Off, key, velocity: 0.0 });
    }

    /// A controller message (spec: ui/midi-settings.md §4): VST3 takes controllers, channel pressure and pitch bend as
    /// the parameters its controller maps them to (IMidiMapping); other messages, and controllers it maps nowhere, are
    /// dropped.
    fn midi(&mut self, offset: u32, bytes: [u8; 3]) {
        let (d1, d2) = (bytes[1] & 0x7F, bytes[2] & 0x7F);
        let (cc, v) = match bytes[0] & 0xF0 {
            0xB0 => (usize::from(d1), f64::from(d2) / 127.0),
            0xD0 => (128, f64::from(d1) / 127.0),
            0xE0 => (129, f64::from(u16::from(d1) | u16::from(d2) << 7) / 16383.0),
            _ => return,
        };
        let Some(&id) = self.midi_map.get(cc) else { return };
        if id == u32::MAX || self.auto.len() >= MAX_AUTO {
            return;
        }
        self.auto.push((offset, id, v));
        self.shared.attention.raise();
    }

    /// Portamento glides (spec: audio/channel-voices.md): the tuning note expression of the key's sounding note.
    fn tune(&mut self, offset: u32, key: u8, semitones: f32) {
        self.queue.push(Note { time: offset, kind: NoteKind::Tune, key, velocity: semitones });
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
            // Activation is a UI-thread call: ask for it and stay silent until then.
            self.wanted_rate.store(rate, Ordering::Release);
            self.shared.attention.raise();
            os_host::wake();
            return;
        }
        self.start_processing();
        // Stable sort: notes at the same time keep their order (a note off before the next note on).
        self.queue.sort_by_key(|e| e.time);
        let song = self.song.take();
        let max = self.max_frames as usize;
        let mut at = 0;
        while at < left.len() {
            let n = (left.len() - at).min(max);
            self.set_context(song.map(|s| s.advanced(at as u64, rate)));
            self.process_chunk(&mut left[at..at + n], &mut right[at..at + n], None);
            at += n;
        }
        self.debug_level(rate, left, right);
    }
}

// spec: ui/mixer.md §7 (effect slots): until the UI thread has activated the effect at the stream's rate, the sound
// passes through unchanged.
impl os_engine::HostedEffect for Plugin {
    fn process_fx(&mut self, rate: u32, left: &mut [f32], right: &mut [f32]) {
        if self.rate != rate {
            self.wanted_rate.store(rate, Ordering::Release);
            self.shared.attention.raise();
            os_host::wake();
            return;
        }
        self.start_processing();
        let (mut il, mut ir) = std::mem::take(&mut self.fx_in);
        let song = self.song.take();
        let max = self.max_frames as usize;
        let mut at = 0;
        while at < left.len() {
            let n = (left.len() - at).min(max);
            self.set_context(song.map(|s| s.advanced(at as u64, rate)));
            il.clear();
            ir.clear();
            il.extend_from_slice(&left[at..at + n]);
            ir.extend_from_slice(&right[at..at + n]);
            self.process_chunk(&mut left[at..at + n], &mut right[at..at + n], Some((&mut il[..], &mut ir[..])));
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

impl Drop for Plugin {
    /// Teardown in the documented order (spec §4.7).
    fn drop(&mut self) {
        self.close_editor();
        self.deactivate();
        unsafe {
            if let Some((a, b)) = self.points.take() {
                a.disconnect(shared_of(&b));
                b.disconnect(shared_of(&a));
            }
            if let Some(c) = self.controller.take() {
                c.set_component_handler(null_shared());
                if self.separate {
                    c.terminate();
                }
            }
            self.component.terminate();
        }
    }
}

#[cfg(test)]
mod mock;
#[cfg(test)]
mod tests;
