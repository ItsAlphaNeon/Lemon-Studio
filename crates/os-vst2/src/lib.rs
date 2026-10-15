// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

//! VST 2 plugin hosting: loads VST 2 plugin files (Windows DLLs, Linux shared objects, macOS `.vst` bundles), lists the
//! plugins they offer and runs instances behind Lemon Studio's format-neutral hosting layer (`os_host::Library` /
//! `os_host::Plugin`), like `os-vst3` and `os-clap` do for their formats.
//!
//! The interface comes from clean-room sources only ([`abi`]): Steinberg's VST 2 SDK is neither used nor needed.
//! Threading follows `os_host`: the UI thread creates, activates, edits and saves an instance; the audio thread only
//! calls the `Instrument` / `HostedEffect` methods, under the instance's mutex. Plugins call back into [`host`] from any
//! thread.

pub mod abi;
mod files;
mod host;
pub mod state;

use std::ffi::{c_void, CString};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub use files::{exports, has_extension, is_plugin, macho_cpus, module_file, pe_machine, standard_dirs, unsupported, CPU_ARM64, CPU_X86_64, ENTRY_POINTS, EXTENSION};
pub use os_host::{set_waker, wake, Descriptor};

use abi::{category, flags, op, Effect, Events, Main, MidiEvent, ParameterProperties, Rect, TimeInfo};
use host::Shared;
use state::Saved;

/// Largest block handed to a plugin (its block size); longer host blocks are split.
const MAX_FRAMES: u32 = 4096;
/// Automation splits a block where a value changes, but never into pieces shorter than this (a later value waits).
const MIN_SPLIT: u32 = 32;
/// Most automation values waiting for a process call (the audio thread never grows the queue).
const MAX_AUTO: usize = 2048;
/// Most note events waiting, and sent with one process call.
const MAX_NOTES: usize = 1024;
const MAX_EVENTS: usize = 512;
/// How long describing a shell's plugins may take; the rest are listed as effects without being created.
const SHELL_BUDGET: Duration = Duration::from_secs(15);

fn debug() -> bool {
    os_host::debug()
}

/// The id Lemon Studio knows a VST 2 plugin by: its unique id as 8 uppercase hex digits (`'4FTP'` = `34465450`).
pub fn id_text(unique_id: i32) -> String {
    format!("{:08X}", unique_id as u32)
}

/// A NUL-terminated text a plugin wrote.
fn text(b: &[u8]) -> String {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..end]).trim().to_string()
}

/// A plugin's version as the plugin manager shows it: the vendor version (else the effect's own version field) read as
/// `0xMMmmpp`, major, minor and patch, at every size. Small numbers are read the same way: DPF's plugins answer 256 for
/// the version their VST3 copies call 0.1.0, as 65536 is 1.0.0 and 0x3020A is 3.2.10 (spec: ui/plugin-manager.md §4,
/// "VST2 versions").
///
/// Older plugins count in decimal instead, four digits as VST's own version numbers do (Dimension Expander's 1005 is
/// 1.0.0.5, VST Speek's 1000 is 1.0.0.0). A four-digit number whose hex reading ends above patch 99 is read that way:
/// no hex version has such a patch, while decimal ones mostly do. One that ends lower (1100, read 0.4.76) stays hex.
fn version_text(vendor: i32, effect: i32) -> String {
    let v = if vendor > 0 { vendor } else { effect };
    if v <= 0 {
        return String::new();
    }
    if (1000..=9999).contains(&v) && v & 0xFF > 99 {
        return format!("{}.{}.{}.{}", v / 1000, v / 100 % 10, v / 10 % 10, v % 10);
    }
    format!("{}.{}.{}", v >> 16, (v >> 8) & 0xFF, v & 0xFF)
}

/// Instrument, effect or neither, from the plugin's flags, category and outputs.
fn kind_of(effect_flags: i32, cat: isize, outputs: i32) -> os_host::Kind {
    if outputs <= 0 {
        os_host::Kind::Other
    } else if effect_flags & flags::INSTRUMENT != 0 || cat == category::INSTRUMENT {
        os_host::Kind::Instrument
    } else if matches!(cat, category::OFFLINE | category::CONTAINER) {
        os_host::Kind::Other
    } else {
        os_host::Kind::Effect
    }
}

/// The feature words the plugin menus sort by: VST 3's "Instrument" / "Fx", and the category's menu word.
fn features_of(kind: os_host::Kind, cat: isize) -> Vec<String> {
    let mut f = vec![if kind == os_host::Kind::Instrument { "Instrument" } else { "Fx" }.to_string()];
    let word = match cat {
        category::METERING => Some("Analyzer"),
        category::MASTERING => Some("Mastering"),
        category::SPATIAL | category::EXTERNAL => Some("Spatial"),
        category::DELAY_OR_ECHO => Some("Reverb"),
        category::RESTORATION => Some("Restoration"),
        category::WAVE_GENERATOR => Some("Generator"),
        _ => None,
    };
    f.extend(word.map(str::to_string));
    f
}

/// One plugin object a plugin file made (an effect) and its host side. Closing it frees it.
struct Instance {
    effect: *mut Effect,
    shared: Arc<Shared>,
}

// SAFETY: an instance is used by one thread at a time (the instance's mutex, or the module's `spare` lock).
unsafe impl Send for Instance {}

impl Instance {
    /// Calls the plugin file's entry point for plugin `shell_id` (0 = the file's own) and opens the effect it gives.
    unsafe fn create(main: Main, shell_id: i32, directory: &CString) -> Result<Instance, String> {
        let shared = Arc::new(Shared::new(shell_id, directory.clone()));
        let effect = host::creating(&shared, || unsafe { main(host::callback) });
        if effect.is_null() {
            return Err("the plugin did not start".into());
        }
        if unsafe { (*effect).magic } != abi::MAGIC {
            return Err("not a VST 2 plugin (its entry point gave something else)".into());
        }
        unsafe { host::attach(effect, &shared) };
        let i = Instance { effect, shared };
        i.dispatch(op::OPEN, 0, 0, std::ptr::null_mut(), 0.0);
        Ok(i)
    }

    fn dispatch(&self, opcode: i32, index: i32, value: isize, ptr: *mut c_void, opt: f32) -> isize {
        // SAFETY: the effect lives until `drop` closes it.
        unsafe {
            match (*self.effect).control {
                Some(f) => f(self.effect, opcode, index, value, ptr, opt),
                None => 0,
            }
        }
    }

    /// A text the plugin writes into a buffer (larger than any the interface names: plugins overrun them).
    fn text(&self, opcode: i32, index: i32) -> String {
        let mut buf = [0u8; 256];
        self.dispatch(opcode, index, 0, buf.as_mut_ptr().cast(), 0.0);
        text(&buf)
    }

    // The effect's fields, read where the plugin keeps them (it may change them, e.g. after `IO_CHANGED`).
    fn flags(&self) -> i32 {
        unsafe { (*self.effect).flags }
    }

    fn unique_id(&self) -> i32 {
        unsafe { (*self.effect).unique_id }
    }

    fn num_params(&self) -> u32 {
        unsafe { (*self.effect).num_params }.max(0) as u32
    }

    fn num_programs(&self) -> i32 {
        unsafe { (*self.effect).num_programs }
    }

    fn channels(&self) -> (usize, usize) {
        let (i, o) = unsafe { ((*self.effect).num_inputs, (*self.effect).num_outputs) };
        (i.clamp(0, 64) as usize, o.clamp(0, 64) as usize)
    }

    fn delay(&self) -> u32 {
        unsafe { (*self.effect).delay }.max(0) as u32
    }

    fn get(&self, index: u32) -> f32 {
        unsafe {
            match (*self.effect).get_parameter {
                Some(f) => f(self.effect, index as i32),
                None => 0.0,
            }
        }
    }

    /// Sets a parameter; the plugin's `AUTOMATE` echo of it is not reported as an edit.
    fn set(&self, index: u32, value: f32) {
        if let Some(f) = unsafe { (*self.effect).set_parameter } {
            host::host_edit(|| unsafe { f(self.effect, index as i32, value) });
        }
    }

    /// The plugin's description, as the plugin lists show it (`name`: a shell's name for it).
    fn describe(&self, path: &Path, name: Option<String>) -> Descriptor {
        let cat = self.dispatch(op::CATEGORY, 0, 0, std::ptr::null_mut(), 0.0);
        let mut name = name.filter(|n| !n.is_empty()).unwrap_or_else(|| self.text(op::EFFECT_NAME, 0));
        if name.is_empty() {
            name = self.text(op::PRODUCT_NAME, 0);
        }
        if name.is_empty() {
            name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        }
        let vendor_version = self.dispatch(op::VENDOR_VERSION, 0, 0, std::ptr::null_mut(), 0.0) as i32;
        let kind = kind_of(self.flags(), cat, self.channels().1 as i32);
        Descriptor {
            format: os_host::Format::Vst2,
            id: id_text(self.unique_id()),
            name,
            vendor: self.text(op::VENDOR_NAME, 0),
            version: version_text(vendor_version, unsafe { (*self.effect).version }),
            features: features_of(kind, cat),
            kind,
            path: path.to_path_buf(),
        }
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        // The plugin frees the effect; its host side (`shared`) goes after.
        self.dispatch(op::CLOSE, 0, 0, std::ptr::null_mut(), 0.0);
    }
}

/// A loaded VST 2 plugin file and the plugins it offers: its own, or a shell's.
pub struct Module {
    path: PathBuf,
    plugins: Vec<Descriptor>,
    /// Each plugin's id in its shell (0: the file's own plugin), parallel to `plugins`.
    shell_ids: Vec<i32>,
    main: Main,
    /// The plugin's folder, for plugins that ask (`DIRECTORY`).
    directory: CString,
    /// The instance made to describe the file's own plugin, given to the first `create` of it.
    spare: Mutex<Option<Instance>>,
    lib: Option<libloading::Library>,
    /// An address in the file's code (its entry point): which module the crash guard stops when it crashes.
    code: usize,
}

// SAFETY: the entry point may be called from any thread; the spare instance is behind a lock.
unsafe impl Send for Module {}
unsafe impl Sync for Module {}

impl Module {
    /// Loads plugin file `path` (a DLL, a shared object or a `.vst` bundle) and lists its plugins. Listing creates the
    /// plugin once (VST 2 has no other way to describe one), and a shell's plugins one by one.
    pub fn load(path: &Path) -> Result<Arc<Module>, String> {
        let file = module_file(path);
        if let Some(why) = unsupported(path) {
            return Err(format!("{}: {why}", file.display()));
        }
        unsafe {
            let lib = os_host::load_module(&file)?;
            let main = ["VSTPluginMain\0", "main_macho\0", "main\0"]
                .iter()
                .find_map(|n| lib.get::<Main>(n.as_bytes()).ok().map(|s| *s))
                .ok_or_else(|| format!("{}: not a VST 2 plugin (no VSTPluginMain)", file.display()))?;
            Module::from_main(path, main, Some(lib))
        }
    }

    /// The module of entry point `main` (of the loaded file `lib`, kept loaded with the module) and its plugins.
    ///
    /// # Safety
    /// `main` is a VST 2 entry point.
    pub unsafe fn from_main(path: &Path, main: Main, lib: Option<libloading::Library>) -> Result<Arc<Module>, String> {
        let folder = path.parent().unwrap_or(path).to_string_lossy().into_owned();
        let directory = CString::new(folder).unwrap_or_default();
        let first = unsafe { Instance::create(main, 0, &directory) }.map_err(|e| format!("{}: {e}", path.display()))?;
        let mut m = Module {
            path: path.to_path_buf(),
            plugins: Vec::new(),
            shell_ids: Vec::new(),
            main,
            directory,
            spare: Mutex::new(None),
            lib,
            code: main as usize,
        };
        if first.dispatch(op::CATEGORY, 0, 0, std::ptr::null_mut(), 0.0) != category::CONTAINER {
            m.plugins.push(first.describe(path, None));
            m.shell_ids.push(0);
            m.spare = Mutex::new(Some(first));
            return Ok(Arc::new(m));
        }
        // A shell: its plugins' ids and names, then each one created to tell instruments from effects (as long as
        // that takes; later ones are listed as effects).
        let vendor = first.text(op::VENDOR_NAME, 0);
        let mut subs = Vec::new();
        for _ in 0..4096 {
            let mut buf = [0u8; 256];
            let id = first.dispatch(op::CONTAINER_NEXT_EFFECT_ID, 0, 0, buf.as_mut_ptr().cast(), 0.0) as i32;
            if id == 0 {
                break;
            }
            subs.push((id, text(&buf)));
        }
        drop(first);
        let start = Instant::now();
        for (id, name) in subs {
            let made = if start.elapsed() < SHELL_BUDGET { unsafe { Instance::create(main, id, &m.directory) }.ok() } else { None };
            let d = match made.filter(|i| i.unique_id() == id) {
                Some(i) => i.describe(path, Some(name)),
                None => Descriptor {
                    format: os_host::Format::Vst2,
                    id: id_text(id),
                    name,
                    vendor: vendor.clone(),
                    version: String::new(),
                    features: features_of(os_host::Kind::Effect, category::UNKNOWN),
                    kind: os_host::Kind::Effect,
                    path: path.to_path_buf(),
                },
            };
            if !d.name.is_empty() && !m.shell_ids.contains(&id) {
                m.plugins.push(d);
                m.shell_ids.push(id);
            }
        }
        Ok(Arc::new(m))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn plugins(&self) -> &[Descriptor] {
        &self.plugins
    }
}

impl Drop for Module {
    fn drop(&mut self) {
        // Every instance holds an `Arc` of its module, so only the spare instance is left. A module whose code crashed
        // is left loaded, as the crash left it.
        let guard = os_host::guard::module_of(self.code as *const c_void);
        if guard.crashed().is_none() {
            let spare = self.spare.get_mut().ok().and_then(Option::take);
            let _ = os_host::guard::call(&guard, || drop(spare));
        }
        if guard.crashed().is_some() {
            std::mem::forget(self.spare.get_mut().ok().and_then(Option::take));
            std::mem::forget(self.lib.take());
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

/// What a queued event does.
#[derive(Clone, Copy, PartialEq, Debug)]
enum NoteKind {
    On,
    Off,
    /// A controller change: `key` is the controller.
    Control,
}

/// A MIDI event waiting for its process call.
#[derive(Clone, Copy, Debug)]
struct Note {
    /// Frames into the next process call.
    time: u32,
    kind: NoteKind,
    key: u8,
    value: u8,
}

impl Note {
    fn bytes(&self) -> [u8; 3] {
        match self.kind {
            NoteKind::On => [0x90, self.key, self.value],
            NoteKind::Off => [0x80, self.key, self.value],
            NoteKind::Control => [0xB0, self.key, self.value],
        }
    }
}

// MIDI controllers a stop sends: all sound off, all notes off.
const ALL_SOUND_OFF: u8 = 120;
const ALL_NOTES_OFF: u8 = 123;

/// A running VST 2 instance.
pub struct Plugin {
    // The effect first: it is closed before the module goes.
    inst: Instance,
    desc: Descriptor,
    /// Channel counts the buffers are set up for.
    ins: usize,
    outs: usize,
    /// Active at this rate (0 = suspended).
    rate: u32,
    wanted_rate: AtomicU32,
    processing: bool,
    editor: bool,
    /// Device pixels per unit of the open editor's sizes: 1, except on macOS, where editors measure in points.
    editor_scale: f32,
    queue: Vec<Note>,
    /// Automation values (frame offset, parameter index, value) for the next process call.
    auto: Vec<(u32, u32, f32)>,
    /// The events of the current process call, and the list pointing at them.
    midi: Box<[MidiEvent; MAX_EVENTS]>,
    list: Box<Events<MAX_EVENTS>>,
    /// Zeroed channels for inputs the host has no audio for, and outputs it does not use.
    scratch: Vec<f32>,
    in_ptrs: Vec<*const f32>,
    out_ptrs: Vec<*mut f32>,
    fx_in: (Vec<f32>, Vec<f32>),
    /// Each parameter's value right after the plugin was created: its default.
    defaults: Vec<f32>,
    latency: u32,
    /// The tail the plugin reports, in frames (0: none or not known).
    tail: u32,
    sounding: bool,
    /// The voices it plays as far as the host can tell: a released voice lasts the stated tail or until the output is
    /// quiet (spec: audio/voices.md §2.5-2.6).
    ledger: os_engine::voices::VoiceLedger,
    /// Frames processed since activation.
    steady: u64,
    /// The song position for the next process call (the engine gives it while playing).
    song: Option<os_engine::TransportInfo>,
    /// The transport stopped (`reset`): the next piece the plugin processes reports a transport change, whatever it
    /// says, since the plugin may last have seen it playing.
    restarted: bool,
    dbg_peak: f32,
    dbg_frames: u64,
    _module: Arc<Module>,
}

// SAFETY: the instance is used behind a mutex (`os_host::SharedPlugin`); main-thread calls come from the UI thread only.
unsafe impl Send for Plugin {}

impl Plugin {
    /// Creates plugin `id` of `module` (suspended: call `activate`).
    pub fn new(module: &Arc<Module>, id: &str) -> Result<Plugin, String> {
        let i = module.plugins.iter().position(|d| d.id.eq_ignore_ascii_case(id)).ok_or_else(|| format!("no plugin {id} in {}", module.path.display()))?;
        let desc = module.plugins[i].clone();
        let shell_id = module.shell_ids[i];
        let spare = if shell_id == 0 { module.spare.lock().ok().and_then(|mut s| s.take()) } else { None };
        let inst = match spare {
            Some(inst) => inst,
            None => unsafe { Instance::create(module.main, shell_id, &module.directory) }.map_err(|e| format!("{}: {e}", desc.name))?,
        };
        if shell_id != 0 && inst.unique_id() != shell_id {
            return Err(format!("{}: its shell did not create it", desc.name));
        }
        let defaults = (0..inst.num_params()).map(|p| inst.get(p)).collect();
        let mut p = Plugin {
            inst,
            desc,
            ins: 0,
            outs: 0,
            rate: 0,
            wanted_rate: AtomicU32::new(0),
            processing: false,
            editor: false,
            editor_scale: 1.0,
            queue: Vec::with_capacity(MAX_NOTES),
            auto: Vec::with_capacity(MAX_AUTO),
            midi: Box::new([MidiEvent::default(); MAX_EVENTS]),
            list: Box::new(Events { count: 0, reserved: 0, events: [std::ptr::null_mut(); MAX_EVENTS] }),
            scratch: Vec::new(),
            in_ptrs: Vec::new(),
            out_ptrs: Vec::new(),
            fx_in: (Vec::new(), Vec::new()),
            defaults,
            latency: 0,
            tail: 0,
            sounding: false,
            ledger: os_engine::voices::VoiceLedger::new(),
            steady: 0,
            song: None,
            restarted: false,
            dbg_peak: 0.0,
            dbg_frames: 0,
            _module: module.clone(),
        };
        p.setup_buffers();
        Ok(p)
    }

    pub fn desc(&self) -> &Descriptor {
        &self.desc
    }

    /// Buffers for the plugin's current inputs and outputs (allocated here, never on the audio thread).
    fn setup_buffers(&mut self) {
        (self.ins, self.outs) = self.inst.channels();
        let max = MAX_FRAMES as usize;
        self.scratch = vec![0.0; (self.ins + self.outs) * max];
        self.in_ptrs = Vec::with_capacity(self.ins);
        self.out_ptrs = Vec::with_capacity(self.outs);
        self.fx_in = (Vec::with_capacity(max), Vec::with_capacity(max));
    }

    /// Sets the plugin up for `rate` and resumes it (UI thread); a new rate suspends and resumes it again.
    pub fn activate(&mut self, rate: u32) -> bool {
        if self.rate == rate {
            return true;
        }
        self.deactivate();
        let none = std::ptr::null_mut();
        self.inst.dispatch(op::SET_SAMPLE_RATE, 0, 0, none, rate as f32);
        self.inst.dispatch(op::SET_BLOCK_SIZE, 0, MAX_FRAMES as isize, none, 0.0);
        self.inst.shared.rate.store(rate, Ordering::Relaxed);
        self.inst.shared.block.store(MAX_FRAMES, Ordering::Relaxed);
        self.inst.dispatch(op::SUSPEND_RESUME, 0, 1, none, 0.0);
        // A resumed plugin may have changed its channels and latency.
        self.setup_buffers();
        self.latency = self.inst.delay();
        self.tail = match self.inst.dispatch(op::TAIL_SAMPLES, 0, 0, none, 0.0) {
            t @ 2.. => t.min(i32::MAX as isize) as u32,
            _ => 0,
        };
        self.rate = rate;
        self.ledger.clear();
        self.ledger.set_tail((self.tail > 0).then(|| f64::from(self.tail) / f64::from(rate)));
        let t = self.inst.shared.time();
        // The tempo and meter are valid once the engine has given a transport.
        self.inst.shared.set_time(TimeInfo { sample_rate: f64::from(rate), tempo: if t.tempo > 0.0 { t.tempo } else { 120.0 }, time_sig_numerator: 4, time_sig_denominator: 4, ..t });
        if debug() {
            eprintln!("[os-vst2] {} active at {rate} Hz (latency {}, ins {}, outs {})", self.desc.name, self.latency, self.ins, self.outs);
        }
        true
    }

    fn deactivate(&mut self) {
        if self.rate == 0 {
            return;
        }
        if self.processing {
            self.inst.dispatch(op::PROCESS_END, 0, 0, std::ptr::null_mut(), 0.0);
            self.processing = false;
        }
        self.inst.dispatch(op::SUSPEND_RESUME, 0, 0, std::ptr::null_mut(), 0.0);
        self.rate = 0;
    }

    fn start_processing(&mut self) {
        if !self.processing {
            self.inst.dispatch(op::PROCESS_BEGIN, 0, 0, std::ptr::null_mut(), 0.0);
            self.processing = true;
        }
    }

    /// UI-thread housekeeping: a new rate, a change of channels or latency, and the editor's idle call.
    pub fn idle(&mut self) {
        let wanted = self.wanted_rate.swap(0, Ordering::AcqRel);
        if self.inst.shared.io_changed.swap(false, Ordering::AcqRel) {
            let r = if wanted != 0 { wanted } else { self.rate };
            self.deactivate();
            if r != 0 {
                self.activate(r);
            } else {
                self.setup_buffers();
            }
            let n = self.inst.num_params() as usize;
            if self.defaults.len() < n {
                let more: Vec<f32> = (self.defaults.len() as u32..n as u32).map(|p| self.inst.get(p)).collect();
                self.defaults.extend(more);
            }
        } else if wanted != 0 && wanted != self.rate {
            self.activate(wanted);
        }
        if self.editor {
            self.inst.dispatch(op::EDITOR_IDLE, 0, 0, std::ptr::null_mut(), 0.0);
        }
    }

    /// Automation (audio thread): parameter `index` to `value`, `offset` frames into the next process call.
    fn automate(&mut self, offset: u32, index: u32, value: f64) {
        if index < self.inst.num_params() && self.auto.len() < MAX_AUTO {
            self.auto.push((offset, index, value.clamp(0.0, 1.0) as f32));
        }
    }

    fn queue(&mut self, n: Note) {
        if self.queue.len() < MAX_NOTES {
            self.queue.push(n);
        }
    }

    /// Sets the automation due at frame `at` of the block and returns where the piece starting there ends: at the next
    /// value (at least `MIN_SPLIT` frames on), the block's end, or `MAX_FRAMES` frames on.
    fn apply_auto(&mut self, at: u32, n: u32) -> u32 {
        let mut kept = 0;
        let mut next = u32::MAX;
        for i in 0..self.auto.len() {
            let (t, index, v) = self.auto[i];
            if t <= at {
                self.inst.set(index, v);
            } else {
                next = next.min(t);
                self.auto[kept] = self.auto[i];
                kept += 1;
            }
        }
        self.auto.truncate(kept);
        let end = (at + MAX_FRAMES).min(n);
        if next < end { next.max(at + MIN_SPLIT).min(end) } else { end }
    }

    /// Sends the notes before frame `end` of the block to the plugin, timed from frame `at`.
    fn send_notes(&mut self, at: u32, end: u32) {
        let mut count = 0;
        let mut kept = 0;
        for i in 0..self.queue.len() {
            let e = self.queue[i];
            if e.time < end && count < MAX_EVENTS {
                self.midi[count] = MidiEvent::new(e.time.saturating_sub(at), e.bytes());
                self.list.events[count] = &mut self.midi[count];
                count += 1;
                match e.kind {
                    NoteKind::On => self.ledger.start(e.key, -1),
                    NoteKind::Off => self.ledger.release(e.key),
                    NoteKind::Control => {}
                }
            } else {
                self.queue[kept] = e;
                kept += 1;
            }
        }
        self.queue.truncate(kept);
        if count > 0 {
            self.list.count = count as i32;
            let list: *mut Events<MAX_EVENTS> = &mut *self.list;
            self.inst.dispatch(op::EVENTS, 0, 0, list.cast(), 0.0);
        }
    }

    /// Events and automation the block did not reach wait for the next one, from its start on.
    fn carry(&mut self, n: u32) {
        self.queue.iter_mut().for_each(|e| e.time = e.time.saturating_sub(n));
        self.auto.iter_mut().for_each(|a| a.0 = a.0.saturating_sub(n));
    }

    /// The transport the plugin reads during the piece starting at frame `at`: the engine's (`song`) while playing,
    /// else stopped where it was. A change of playing, or the first piece after a stop, is flagged as a transport change.
    fn set_time(&mut self, song: Option<os_engine::TransportInfo>, at: u32, rate: u32) {
        use abi::time::*;
        let mut t = self.inst.shared.time();
        let was_playing = t.flags & PLAYING != 0;
        t.sample_rate = f64::from(rate);
        t.flags &= !(PLAYING | CYCLE_ACTIVE | TRANSPORT_CHANGED);
        match song.map(|s| s.advanced(u64::from(at), rate)) {
            Some(s) => {
                t.flags |= TEMPO_VALID | TIME_SIG_VALID | PPQ_POS_VALID | BARS_VALID;
                if s.playing {
                    t.flags |= PLAYING;
                }
                t.sample_pos = s.frame as f64;
                t.ppq_pos = s.beats;
                t.tempo = s.tempo;
                t.bar_start_pos = s.bar_start;
                t.time_sig_numerator = i32::from(s.numerator);
                t.time_sig_denominator = i32::from(s.denominator);
                match s.looping {
                    Some((a, b)) => {
                        t.flags |= CYCLE_ACTIVE | CYCLE_POS_VALID;
                        t.cycle_start_pos = a;
                        t.cycle_end_pos = b;
                    }
                    None => t.flags &= !CYCLE_POS_VALID,
                }
            }
            None if t.flags & TEMPO_VALID == 0 => t.sample_pos = self.steady as f64,
            None => {}
        }
        let restarted = std::mem::take(&mut self.restarted);
        if (t.flags & PLAYING != 0) != was_playing || restarted {
            t.flags |= TRANSPORT_CHANGED;
        }
        self.inst.shared.set_time(t);
    }

    /// The transport stopped (`reset`: Stop, a new Play, the start and the end of an export): what the plugin reads says
    /// so at once, and its next piece reports the change. An export's last block is still playing, and no block follows
    /// it before the next Play, so without this that Play would not be flagged as a change.
    fn stop_time(&mut self) {
        use abi::time::*;
        let mut t = self.inst.shared.time();
        t.flags &= !(PLAYING | CYCLE_ACTIVE | TRANSPORT_CHANGED);
        self.inst.shared.set_time(t);
        self.restarted = true;
    }

    /// Processes one piece (at most `MAX_FRAMES` frames): `input` is an effect's input, else the inputs are silent.
    fn run(&mut self, left: &mut [f32], right: &mut [f32], input: Option<(&[f32], &[f32])>) {
        let n = left.len();
        let max = MAX_FRAMES as usize;
        let base = self.scratch.as_mut_ptr();
        self.in_ptrs.clear();
        self.out_ptrs.clear();
        for c in 0..self.ins {
            // SAFETY: scratch holds `MAX_FRAMES` frames for every channel.
            let s = unsafe { base.add(c * max) };
            let host = match (input, self.ins, c) {
                // A mono input takes both sides.
                (Some((l, r)), 1, _) => {
                    let mono = unsafe { std::slice::from_raw_parts_mut(s, n) };
                    mono.iter_mut().zip(l.iter().zip(r)).for_each(|(m, (a, b))| *m = (a + b) * 0.5);
                    Some(s as *const f32)
                }
                (Some((l, _)), _, 0) => Some(l.as_ptr()),
                (Some((_, r)), _, 1) => Some(r.as_ptr()),
                _ => None,
            };
            self.in_ptrs.push(host.unwrap_or_else(|| {
                unsafe { std::ptr::write_bytes(s, 0, n) };
                s
            }));
        }
        left.fill(0.0);
        right.fill(0.0);
        for c in 0..self.outs {
            let p = match c {
                0 => left.as_mut_ptr(),
                1 => right.as_mut_ptr(),
                _ => {
                    let s = unsafe { base.add((self.ins + c) * max) };
                    unsafe { std::ptr::write_bytes(s, 0, n) };
                    s
                }
            };
            self.out_ptrs.push(p);
        }
        let e = self.inst.effect;
        // SAFETY: every pointer holds `n` frames; the plugin's processing calls are made one at a time.
        unsafe {
            let replacing = if (*e).flags & flags::SUPPORTS_FLOAT != 0 { (*e).process_float } else { None };
            // Without replacing processing, the old accumulating call adds to the zeroed outputs.
            if let Some(f) = replacing.or((*e).process) {
                f(e, self.in_ptrs.as_ptr(), self.out_ptrs.as_mut_ptr(), n as i32);
            }
        }
        if self.outs == 1 {
            right.copy_from_slice(left);
        }
        let loud = left.iter().chain(right.iter()).any(|v| v.abs() > 1e-5);
        self.sounding = loud;
        self.steady += n as u64;
        if !loud {
            self.ledger.quiet(false);
        }
        self.ledger.advance(n as f64 / f64::from(self.rate.max(1)));
    }

    /// Renders a host block in pieces: notes and automation inside it, the transport, an effect's input (`effect`: the
    /// buffers hold it, and get the output).
    fn render(&mut self, rate: u32, left: &mut [f32], right: &mut [f32], effect: bool) {
        self.start_processing();
        // Stable sorts: events at the same time keep their order (a note off before the next note on).
        self.queue.sort_by_key(|e| e.time);
        self.auto.sort_by_key(|a| a.0);
        let song = self.song.take();
        let n = left.len() as u32;
        let (mut il, mut ir) = std::mem::take(&mut self.fx_in);
        let mut at = 0;
        while at < n {
            let end = self.apply_auto(at, n);
            self.set_time(song, at, rate);
            self.send_notes(at, end);
            let (a, b) = (at as usize, end as usize);
            if effect {
                il.clear();
                ir.clear();
                il.extend_from_slice(&left[a..b]);
                ir.extend_from_slice(&right[a..b]);
                self.run(&mut left[a..b], &mut right[a..b], Some((&il, &ir)));
            } else {
                self.run(&mut left[a..b], &mut right[a..b], None);
            }
            at = end;
        }
        self.fx_in = (il, ir);
        self.carry(n);
        self.debug_level(rate, left, right);
    }

    /// The editor's rectangle as the plugin reports it (width, height).
    fn rect(&self) -> Option<(i32, i32)> {
        let mut r: *mut Rect = std::ptr::null_mut();
        self.inst.dispatch(op::EDITOR_GET_RECT, 0, 0, (&mut r as *mut *mut Rect).cast(), 0.0);
        // SAFETY: a non-null answer points at the plugin's rectangle.
        let r = unsafe { r.as_ref() }?;
        let (w, h) = (i32::from(r.right) - i32::from(r.left), i32::from(r.bottom) - i32::from(r.top));
        (w > 0 && h > 0).then_some((w, h))
    }

    /// An editor size as the plugin gives it (points on macOS) in pixels.
    fn to_px(&self, w: i32, h: i32) -> (u32, u32) {
        let px = |v: i32| (v.max(0) as f32 * self.editor_scale).round() as u32;
        (px(w), px(h))
    }

    pub fn has_editor(&self) -> bool {
        self.inst.flags() & flags::EDITOR != 0
    }

    /// Embeds the editor in `parent` (an HWND on Windows, an NSView on macOS, an X11 window id on Linux); returns its
    /// size in pixels. `scale` is the parent's DPI factor: outside macOS it is offered to the plugin through the
    /// content-scale extension many plugins take (vendor op-code `'PreS'` / `'AeCs'`), 1 included: plugins keep the
    /// last scale they were told, and some guess one from the monitor when told none. On macOS editors measure in
    /// points and follow their window's backing scale themselves, so their sizes are turned into pixels at `scale`.
    pub fn open_editor(&mut self, parent: *mut c_void, scale: f32) -> Result<(u32, u32), String> {
        if !self.has_editor() {
            return Err(format!("{} has no editor", self.desc.name));
        }
        self.close_editor();
        let mac = cfg!(target_os = "macos");
        self.editor_scale = if mac && scale > 0.0 { scale } else { 1.0 };
        if !mac && scale > 0.0 {
            self.inst.dispatch(op::CUSTOM, abi::fourcc(*b"PreS"), abi::fourcc(*b"AeCs") as isize, std::ptr::null_mut(), scale);
        }
        if let Ok(mut s) = self.inst.shared.size.lock() {
            *s = None;
        }
        // The answer says little: many plugins return 0 for an editor they did open.
        self.inst.dispatch(op::EDITOR_OPEN, 0, 0, parent, 0.0);
        self.editor = true;
        let (w, h) = self.rect().unwrap_or((400, 300));
        Ok(self.to_px(w, h))
    }

    /// The editor's size in pixels (when open): what it asked for last, else what it reports.
    pub fn editor_size(&self) -> Option<(u32, u32)> {
        if !self.editor {
            return None;
        }
        let asked = self.inst.shared.size.lock().ok().and_then(|s| *s);
        asked.or_else(|| self.rect()).map(|(w, h)| self.to_px(w, h))
    }

    pub fn close_editor(&mut self) {
        if std::mem::take(&mut self.editor) {
            self.inst.dispatch(op::EDITOR_CLOSE, 0, 0, std::ptr::null_mut(), 0.0);
        }
    }

    /// The instance's state ([`state`]): the plugin's chunk, or its parameter values (the program is not kept: FL
    /// Studio's layout for it is not known).
    pub fn save_state(&self) -> Option<Vec<u8>> {
        let saved = if self.inst.flags() & flags::CHUNKS != 0 {
            let mut p: *mut c_void = std::ptr::null_mut();
            let size = self.inst.dispatch(op::GET_CHUNK, 0, 0, (&mut p as *mut *mut c_void).cast(), 0.0);
            if p.is_null() || size <= 0 {
                return None;
            }
            // SAFETY: the plugin's chunk holds `size` bytes until its next call.
            Saved::Chunk(unsafe { std::slice::from_raw_parts(p as *const u8, size as usize) }.to_vec())
        } else {
            Saved::Params { program: None, values: (0..self.inst.num_params()).map(|i| self.inst.get(i)).collect() }
        };
        Some(saved.to_bytes())
    }

    /// Restores a `save_state` blob, one FL Studio saved, or one Lemon Studio's first saves wrote ([`state`]).
    pub fn load_state(&mut self, data: &[u8]) -> bool {
        match Saved::parse(data, self.inst.flags() & flags::CHUNKS != 0) {
            Some(Saved::Chunk(mut c)) => {
                let len = c.len() as isize;
                host::host_edit(|| self.inst.dispatch(op::SET_CHUNK, 0, len, c.as_mut_ptr().cast(), 0.0));
                true
            }
            Some(Saved::Params { program, values }) => {
                if let Some(program) = program.filter(|p| (0..self.inst.num_programs()).contains(p)) {
                    self.inst.dispatch(op::SET_PROGRAM, 0, program as isize, std::ptr::null_mut(), 0.0);
                }
                for (i, v) in (0..self.inst.num_params()).zip(values) {
                    self.inst.set(i, v);
                }
                true
            }
            None => false,
        }
    }

    fn debug_level(&mut self, rate: u32, left: &[f32], right: &[f32]) {
        if debug() {
            self.dbg_peak = left.iter().chain(right.iter()).fold(self.dbg_peak, |m, v| m.max(v.abs()));
            self.dbg_frames += left.len() as u64;
            if self.dbg_frames >= u64::from(rate) {
                eprintln!("[os-vst2] {} peak {:.3}", self.desc.name, self.dbg_peak);
                (self.dbg_frames, self.dbg_peak) = (0, 0.0);
            }
        }
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
        if self.rate == 0 { 0 } else { self.latency }
    }

    fn set_offline(&mut self, offline: bool) {
        self.inst.shared.offline.store(offline, Ordering::Relaxed);
    }

    fn param_count(&self) -> usize {
        self.inst.num_params() as usize
    }

    /// VST 2 parameters are numbered and already normalized; their id is their index. Ranges, steps and groups come
    /// from the plugin's parameter properties where it gives them.
    fn param_info(&self, index: usize) -> Option<os_host::ParamInfo> {
        let i = u32::try_from(index).ok().filter(|&i| i < self.inst.num_params())?;
        let mut name = self.inst.text(op::PARAM_NAME, i as i32);
        if name.is_empty() {
            name = format!("Parameter {}", i + 1);
        }
        // SAFETY: plain data, filled by the plugin when it has properties.
        let mut props: ParameterProperties = unsafe { std::mem::zeroed() };
        let has = self.inst.dispatch(op::PARAM_PROPERTIES, i as i32, 0, (&mut props as *mut ParameterProperties).cast(), 0.0) == 1;
        let pf = if has { props.flags } else { 0 };
        let steps = if pf & abi::param_flags::SWITCH != 0 {
            1
        } else if pf & abi::param_flags::INTEGER_LIMITS != 0 && props.max_value_i32 > props.min_value_i32 {
            (props.max_value_i32 - props.min_value_i32).min(1000) as u32
        } else {
            0
        };
        Some(os_host::ParamInfo {
            id: i,
            name,
            unit: self.inst.text(op::PARAM_LABEL, i as i32),
            group: if pf & abi::param_flags::CATEGORY != 0 { text(&props.category_label) } else { String::new() },
            min: 0.0,
            max: 1.0,
            default: f64::from(self.defaults.get(index).copied().unwrap_or(0.0)),
            steps,
            automatable: true,
            read_only: false,
            hidden: false,
            bypass: false,
            enumerated: false,
        })
    }

    fn param_value(&self, id: u32) -> Option<f64> {
        (id < self.inst.num_params()).then(|| f64::from(self.inst.get(id)))
    }

    /// The plugin only tells the text of a parameter's current value: other values have none.
    fn param_text(&self, id: u32, value: f64) -> Option<String> {
        if id >= self.inst.num_params() || (f64::from(self.inst.get(id)) - value).abs() > 1e-4 {
            return None;
        }
        let display = self.inst.text(op::PARAM_DISPLAY, id as i32);
        let unit = self.inst.text(op::PARAM_LABEL, id as i32);
        let t = if unit.is_empty() { display } else { format!("{display} {unit}") };
        (!t.is_empty()).then_some(t)
    }

    fn set_param(&mut self, id: u32, value: f64) -> bool {
        if id >= self.inst.num_params() {
            return false;
        }
        self.inst.set(id, value.clamp(0.0, 1.0) as f32);
        os_host::wake();
        true
    }

    fn take_param_events(&mut self) -> Vec<os_host::ParamEvent> {
        self.inst.shared.events.lock().map(|mut v| std::mem::take(&mut *v)).unwrap_or_default()
    }

    fn attention(&self) -> Option<Arc<os_host::Attention>> {
        Some(self.inst.shared.attention.clone())
    }
}

impl os_engine::Instrument for Plugin {
    fn active(&self) -> bool {
        self.sounding
    }

    fn voices(&self) -> usize {
        self.ledger.count()
    }

    /// Stop: queued notes are dropped, held notes get their note offs, and all sound and notes are switched off with the
    /// next block, whose processing starts anew (spec: audio/voices.md §3). The transport is stopped (`stop_time`).
    fn reset(&mut self) {
        self.stop_time();
        self.queue.clear();
        self.sounding = false;
        let mut sent = [false; 128];
        let held: Vec<u8> = self.ledger.held().collect();
        for key in held {
            if !std::mem::replace(&mut sent[usize::from(key.min(127))], true) {
                self.queue(Note { time: 0, kind: NoteKind::Off, key, value: 0 });
            }
        }
        for c in [ALL_SOUND_OFF, ALL_NOTES_OFF] {
            self.queue(Note { time: 0, kind: NoteKind::Control, key: c, value: 0 });
        }
        self.ledger.clear();
        // Automation still waiting is a value the parameter should have: it is kept for the next block.
        if self.processing {
            self.inst.dispatch(op::PROCESS_END, 0, 0, std::ptr::null_mut(), 0.0);
            self.processing = false;
        }
    }

    fn note_on(&mut self, offset: u32, key: u8, velocity: f32) {
        let v = (velocity.clamp(0.0, 1.0) * 127.0).round().max(1.0) as u8;
        self.queue(Note { time: offset, kind: NoteKind::On, key: key.min(127), value: v });
    }

    fn note_off(&mut self, offset: u32, key: u8) {
        self.queue(Note { time: offset, kind: NoteKind::Off, key: key.min(127), value: 64 });
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
            self.inst.shared.attention.raise();
            os_host::wake();
            return;
        }
        self.render(rate, left, right, false);
    }
}

// spec: ui/mixer.md §7 (effect slots): until the UI thread has activated the effect at the stream's rate, the sound
// passes through unchanged.
impl os_engine::HostedEffect for Plugin {
    fn process_fx(&mut self, rate: u32, left: &mut [f32], right: &mut [f32]) {
        if self.rate != rate {
            self.wanted_rate.store(rate, Ordering::Release);
            self.inst.shared.attention.raise();
            os_host::wake();
            return;
        }
        self.render(rate, left, right, true);
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
    fn drop(&mut self) {
        self.close_editor();
        self.deactivate();
        // `inst` closes the effect next.
    }
}

#[cfg(test)]
mod mock;
#[cfg(test)]
mod tests;
