// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Studio's format-neutral plugin hosting layer.
//!
//! Every plugin format Lemon Studio hosts (CLAP in `os-clap`, VST3 in `os-vst3`, VST2 in `os-vst2`, native plugins in
//! `os-native`) offers the same two things: a [`Library`] (a loaded plugin file that lists [`Descriptor`]s and creates
//! instances) and a [`Plugin`] instance (activation, editor, state, parameters, and the engine-side [`Instrument`] /
//! [`HostedEffect`] processing). The app only talks to these traits.
//!
//! Threading (all formats): an instance is shared as [`SharedPlugin`] (`Arc<Mutex<dyn Plugin>>`). The UI thread creates
//! and activates it, opens its editor, loads / saves state, reads / sets parameters and calls [`Plugin::idle`] every
//! tick; the audio thread only calls the `Instrument` / `HostedEffect` methods through the engine (with `try_lock`).
//! A `SharedPlugin` coerces to `os_engine::SharedInstrument` / `SharedEffect` (`plugin.clone() as SharedInstrument`).
//!
//! Crashes (all formats): every instance is created as a [`Guarded`] one ([`share`]), whose calls run under the crash
//! guard ([`guard`]). A plugin whose code crashes is stopped and Lemon Studio keeps running.
// spec: plugins/plugin-formats.md (hosting inside Lemon Studio), plugins/vst3-hosting.md

#[cfg(target_os = "macos")]
pub mod bundle;
pub mod guard;
mod guarded;
mod meter;
#[cfg(unix)]
pub mod runloop;

pub use guarded::{share, Guarded};
pub use meter::{Attention, Meter};

use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

pub use os_engine::{HostedEffect, Instrument};

/// The plugin format of a library.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Format {
    Clap,
    Vst3,
    Vst2,
    /// Plugins written against the native SDK of the DAW whose projects Lemon Studio opens (`os-native`).
    Native,
}

impl Format {
    /// Short lowercase name, used in plugin identities (`Descriptor::uid`): "clap", "vst3", "vst2", "native".
    pub fn as_str(self) -> &'static str {
        match self {
            Format::Clap => "clap",
            Format::Vst3 => "vst3",
            Format::Vst2 => "vst2",
            Format::Native => "native",
        }
    }

    /// The format named by `as_str`.
    pub fn parse(s: &str) -> Option<Format> {
        [Format::Clap, Format::Vst3, Format::Vst2, Format::Native].into_iter().find(|f| f.as_str() == s)
    }
}

/// What a plugin is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Kind {
    /// A note-driven generator (channel rack).
    Instrument,
    /// An insert effect (mixer slots).
    Effect,
    /// Anything else (analyzers without audio out, note effects...): not offered yet.
    #[default]
    Other,
}

/// A plugin a library offers.
#[derive(Debug, Clone, PartialEq)]
pub struct Descriptor {
    pub format: Format,
    /// Format-specific id, stable across versions and machines: the CLAP plugin id, the VST3 class id as 32
    /// uppercase hex digits, the VST2 unique id as 8 uppercase hex digits, a native plugin's file stem.
    pub id: String,
    pub name: String,
    pub vendor: String,
    pub version: String,
    /// CLAP features, or the VST3 class's subcategories (split at `|`); for VST2, "Instrument" / "Fx" and its category.
    pub features: Vec<String>,
    pub kind: Kind,
    /// The file it was loaded from (a `.clap` file, a `.vst3` bundle folder or module file, a `.vst` bundle, a DLL or
    /// shared object; for a plugin compiled into Lemon Studio, the program itself).
    pub path: PathBuf,
}

impl Descriptor {
    pub fn is_instrument(&self) -> bool {
        self.kind == Kind::Instrument
    }

    /// An insert effect: offered for mixer slots.
    pub fn is_effect(&self) -> bool {
        self.kind == Kind::Effect
    }

    /// The plugin's identity for project files and plugin lists: `format:id` (e.g. `clap:org.openstudio.open-sampler`,
    /// `vst3:` + 32 hex digits). The path is not part of it (plugins move between folders and machines).
    pub fn uid(&self) -> String {
        format!("{}:{}", self.format.as_str(), self.id)
    }
}

/// One parameter of a plugin. Values crossing this API are normalized to 0..=1 (VST3's own unit; CLAP's plain value
/// maps linearly: plain = min + normalized * (max - min)), which is also what automation stores. The list order
/// (`Plugin::param_info(index)`) is the parameter's index.
#[derive(Debug, Clone, PartialEq)]
pub struct ParamInfo {
    /// Stable id (CLAP `clap_id`, VST3 `ParamID`).
    pub id: u32,
    pub name: String,
    /// Unit label ("dB", "%"), may be empty.
    pub unit: String,
    /// Group / module path ("Filter/Envelope"), may be empty.
    pub group: String,
    /// The plugin's own (plain) range, for display: CLAP min / max, VST3 the plain values of 0 and 1.
    pub min: f64,
    pub max: f64,
    /// Default value, normalized.
    pub default: f64,
    /// Number of steps for stepped parameters (a switch has 1, a 4-way list 3), 0 = continuous.
    pub steps: u32,
    pub automatable: bool,
    pub read_only: bool,
    pub hidden: bool,
    /// The plugin's bypass switch.
    pub bypass: bool,
    /// A list of named values (CLAP enum, VST3 list).
    pub enumerated: bool,
}

/// A parameter change the plugin reports by itself (its editor moved a control): the source of "last tweaked" and of
/// automation recording. `Begin` / `End` bracket a gesture (mouse down / up on a knob).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ParamEvent {
    Begin(u32),
    /// Parameter id, new normalized value.
    Value(u32, f64),
    End(u32),
}

/// A menu a plugin asked the host to pop up over its editor (VST 3 `IContextMenu::popup`), shown as one of the host's
/// own popups. A newer one replaces it.
// spec: plugins/vst3-hosting.md §9
#[derive(Debug, Clone, PartialEq)]
pub struct Menu {
    /// Which menu this is, for [`Plugin::menu_done`] (unique across instances).
    pub id: u64,
    /// Where, in the editor's pixels from its top-left (what [`Plugin::editor_size`] measures in).
    pub at: (i32, i32),
    pub items: Vec<MenuItem>,
}

/// One row of a plugin [`Menu`].
#[derive(Debug, Clone, PartialEq)]
pub enum MenuItem {
    /// `pick` is what [`Plugin::menu_done`] takes when the row is picked.
    Entry { label: String, pick: usize, enabled: bool, checked: bool },
    Separator,
    /// A submenu (its rows may hold submenus of their own).
    Sub { label: String, items: Vec<MenuItem> },
}

/// A loaded plugin file of any format.
pub trait Library: Send + Sync {
    /// The file it was loaded from.
    fn path(&self) -> &Path;
    /// The plugins it offers.
    fn plugins(&self) -> &[Descriptor];
    /// Creates and initialises plugin `id` (UI thread). Not active yet: call [`Plugin::activate`].
    fn create(self: Arc<Self>, id: &str) -> Result<SharedPlugin, String>;
}

/// The parameter of a built-in Lemon instrument that takes a note's mod X as per-note modulation (`use_note_levels`;
/// spec: plugins/lemon-sampler.md, plugins/3x-osc.md parameter names).
pub const NOTE_MOD_X: &str = "Filter frequency (modulation X)";
/// The same for a note's mod Y.
pub const NOTE_MOD_Y: &str = "Filter bandwidth (modulation Y)";

/// A plugin instance, whatever its format.
///
/// Main-thread calls (everything except the `Instrument` / `HostedEffect` methods) come from the UI thread only.
pub trait Plugin: Instrument + HostedEffect + Send {
    fn desc(&self) -> &Descriptor;

    /// Activates at `rate` (UI thread); reactivates when the rate changes. The audio thread renders silence (an
    /// instrument) or passes the sound through (an effect) until the instance is active at the stream's rate, and asks
    /// for that rate; the next `idle` reactivates.
    fn activate(&mut self, rate: u32) -> bool;

    /// UI-thread housekeeping, every tick: callbacks the plugin asked for, restarts, rate changes.
    fn idle(&mut self);

    /// The plugin has an editor that can be embedded in a host window.
    fn has_editor(&self) -> bool;
    fn editor_open(&self) -> bool;
    /// Embeds the plugin's editor in `parent` (a host window handle: HWND on Windows); returns its size in pixels.
    /// The host draws the window frame around it (spec: ui/plugin-window.md). `scale` is the parent window's DPI
    /// factor (1.0 = 96 dpi): the editor is told to draw at it (spec: ui/ui-scaling.md §3).
    fn open_editor(&mut self, parent: *mut c_void, scale: f32) -> Result<(u32, u32), String>;
    /// The editor's current size in pixels (when open). The host polls it: a plugin that resizes itself is followed.
    fn editor_size(&self) -> Option<(u32, u32)>;
    fn close_editor(&mut self);

    /// The plugin's whole state as bytes, in the format's own layout (UI thread). Round-trips through `load_state`
    /// of the same plugin; what a project file stores for the instance.
    fn save_state(&self) -> Option<Vec<u8>>;
    /// Restores a `save_state` blob (UI thread).
    fn load_state(&mut self, data: &[u8]) -> bool;

    /// Processing latency the plugin reports, in frames (0 when it reports none).
    fn latency(&self) -> u32 {
        0
    }

    /// Sends each note's sample start (`Instrument::note_on_from`) as per-note modulation of the parameter named
    /// `name` (spec: plugins/lemon-sampler.md §7). False when the plugin or format cannot (notes play from the top).
    fn use_start_param(&mut self, name: &str) -> bool {
        let _ = name;
        false
    }

    /// Sends each note's own pan and mod X / Y (`Instrument::note_start`) to the instance: the pan as a per-note
    /// expression, the mods as per-note modulation of the parameters named [`NOTE_MOD_X`] / [`NOTE_MOD_Y`] where it has
    /// them (UI thread). For the built-in Lemon plugins only (an OpenStudio choice: other plugins have no agreed way to
    /// take FL-style note levels). False when the format cannot.
    fn use_note_levels(&mut self) -> bool {
        false
    }

    /// Sends the gains the engine gives audio clip voices (`Instrument::voice_gain`) to the instance, which then puts
    /// each clip's fades and gain on that clip's voice (UI thread). For Lemon Sampler only (it moves to them as the
    /// engine expects). False when the format cannot.
    fn use_voice_gains(&mut self) -> bool {
        false
    }

    /// Tells the instance whether it renders offline (an export: no deadline, the plugin's best quality) or for live
    /// playback (UI thread, with no stream processing the instance). Formats without such a switch ignore it.
    fn set_offline(&mut self, offline: bool) {
        let _ = offline;
    }

    /// Number of parameters.
    fn param_count(&self) -> usize {
        0
    }
    /// Parameter `index` (0..param_count).
    fn param_info(&self, index: usize) -> Option<ParamInfo> {
        let _ = index;
        None
    }
    /// Current value of parameter `id`, normalized.
    fn param_value(&self, id: u32) -> Option<f64> {
        let _ = id;
        None
    }
    /// The plugin's text for normalized `value` of parameter `id` ("-6.0 dB").
    fn param_text(&self, id: u32, value: f64) -> Option<String> {
        let _ = (id, value);
        None
    }
    /// Sets parameter `id` to a normalized value (UI thread). The change reaches the sound on the next processed
    /// block (or at once when nothing is processing the instance). False for an unknown id.
    fn set_param(&mut self, id: u32, value: f64) -> bool {
        let _ = (id, value);
        false
    }
    /// Parameter changes the plugin made by itself since the last call (its editor), oldest first.
    fn take_param_events(&mut self) -> Vec<ParamEvent> {
        Vec::new()
    }
    /// Parameters the plugin's own menus asked the host to automate since the last call ("Create automation clip",
    /// CLAP context-menu host entries, VST 3 context-menu host entries), by parameter id. Plugins without such menus
    /// return nothing.
    fn take_automation_requests(&mut self) -> Vec<u32> {
        Vec::new()
    }
    /// A menu the plugin asked the host to pop up over its editor since the last call (UI thread). The host shows it
    /// and reports how it ended with [`Plugin::menu_done`].
    fn take_menu(&mut self) -> Option<Menu> {
        None
    }
    /// Menu `id` ([`Plugin::take_menu`]) closed: with `pick`, the user picked that entry and the plugin runs it now (UI
    /// thread); without, it closed with no pick. A menu a newer one replaced is ignored.
    fn menu_done(&mut self, id: u64, pick: Option<usize>) {
        let _ = (id, pick);
    }

    /// The exception code of the crash that stopped the instance ([`Guarded`]), or of the error it stopped itself after,
    /// if one did. A stopped instance is never called again: its instrument plays silence, its effect passes the sound
    /// through.
    fn stopped(&self) -> Option<u32> {
        None
    }

    /// The instance's meter: processing time and voices, readable without locking the instance ([`Guarded`] keeps it).
    fn meter(&self) -> Option<Arc<Meter>> {
        None
    }

    /// Its attention count ([`Attention`]); `None`: the UI looks at it on every tick.
    fn attention(&self) -> Option<Arc<Attention>> {
        None
    }
}

pub type SharedPlugin = Arc<Mutex<dyn Plugin>>;

/// Called when a plugin wants the UI thread soon (parameter flush, callback, restart, resize): the app wakes its event
/// loop so `idle` runs at once instead of on the next timer tick.
static WAKER: OnceLock<fn()> = OnceLock::new();

/// Sets the function that wakes the host's UI loop (once per process).
pub fn set_waker(f: fn()) {
    let _ = WAKER.set(f);
}

/// Wakes the host's UI loop (any thread).
pub fn wake() {
    if let Some(f) = WAKER.get() {
        f();
    }
}

/// Loads a plugin module (DLL / shared object) for a backend.
///
/// On Linux and macOS the module stays mapped after it is closed (`RTLD_NODELETE`), as other hosts there do: plugins
/// leave threads and thread-local destructors behind, and once the code is unmapped those crash the host when they run.
///
/// # Safety
/// Loading runs the module's initialisers (see `libloading::Library::new`).
pub unsafe fn load_module(path: &Path) -> Result<libloading::Library, String> {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    let lib = {
        use libloading::os::unix::{Library, RTLD_LOCAL, RTLD_NOW};
        /// `<dlfcn.h>`: glibc and musl, Apple's.
        const RTLD_NODELETE: std::ffi::c_int = if cfg!(target_os = "macos") { 0x80 } else { 0x1000 };
        unsafe { Library::open(Some(path), RTLD_NOW | RTLD_LOCAL | RTLD_NODELETE) }.map(libloading::Library::from)
    };
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    let lib = unsafe { libloading::Library::new(path) };
    lib.map_err(|e| format!("{}: {e}", path.display()))
}

/// `OS_DEBUG` is set: backends log activation and levels to stderr.
pub fn debug() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("OS_DEBUG").is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Dummy(Descriptor, f32);
    impl Instrument for Dummy {
        fn note_on(&mut self, _: u32, _: u8, v: f32) {
            self.1 = v;
        }
        fn note_off(&mut self, _: u32, _: u8) {}
        fn process(&mut self, _: u32, l: &mut [f32], r: &mut [f32]) {
            l.fill(self.1);
            r.fill(self.1);
        }
    }
    impl HostedEffect for Dummy {
        fn process_fx(&mut self, _: u32, l: &mut [f32], _: &mut [f32]) {
            l.iter_mut().for_each(|v| *v *= 2.0);
        }
    }
    impl Plugin for Dummy {
        fn desc(&self) -> &Descriptor {
            &self.0
        }
        fn activate(&mut self, _: u32) -> bool {
            true
        }
        fn idle(&mut self) {}
        fn has_editor(&self) -> bool {
            false
        }
        fn editor_open(&self) -> bool {
            false
        }
        fn open_editor(&mut self, _: *mut c_void, _: f32) -> Result<(u32, u32), String> {
            Err("none".into())
        }
        fn editor_size(&self) -> Option<(u32, u32)> {
            None
        }
        fn close_editor(&mut self) {}
        fn save_state(&self) -> Option<Vec<u8>> {
            Some(self.1.to_le_bytes().to_vec())
        }
        fn load_state(&mut self, d: &[u8]) -> bool {
            d.try_into().map(|b| self.1 = f32::from_le_bytes(b)).is_ok()
        }
    }

    /// A shared instance is the engine's instrument and effect at once (trait upcasting through the mutex).
    #[test]
    fn shared_plugins_are_engine_instruments_and_effects() {
        let desc = Descriptor {
            format: Format::Vst3,
            id: "0123".into(),
            name: "Dummy".into(),
            vendor: String::new(),
            version: String::new(),
            features: vec![],
            kind: Kind::Instrument,
            path: PathBuf::new(),
        };
        assert_eq!(desc.uid(), "vst3:0123");
        assert_eq!(Format::parse("vst3"), Some(Format::Vst3));
        let p: SharedPlugin = Arc::new(Mutex::new(Dummy(desc, 0.0)));
        let inst = p.clone() as os_engine::SharedInstrument;
        let fx = p.clone() as os_engine::SharedEffect;
        inst.lock().unwrap().note_on(0, 60, 0.5);
        let (mut l, mut r) = ([0.0f32; 4], [0.0f32; 4]);
        inst.lock().unwrap().process(48000, &mut l, &mut r);
        fx.lock().unwrap().process_fx(48000, &mut l, &mut r);
        assert_eq!(l, [1.0; 4]);
        assert_eq!(p.lock().unwrap().save_state(), Some(0.5f32.to_le_bytes().to_vec()));
        assert!(p.lock().unwrap().desc().is_instrument());
    }
}
