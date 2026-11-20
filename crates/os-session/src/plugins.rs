// Copyright (C) 2026 ItsAlphaNeon
// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Hosted plugins (any format, through the `os-host` layer): the plugins known (built in, and found by the plugin scan,
//! `plugin_db` / `plugin_scan`), one instance per plugin channel or mixer slot, their windows, and the instrument /
//! effect lists the transport plays.
//!
// spec: plugins/plugin-formats.md (hosting, Lemon Sampler state), ui/plugin-manager.md §4 (scan results, loading for
// use), ui/plugin-window.md, plugins/host-link.md (Lemon plugins join the window strip)


use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use os_host::{Descriptor, Library, SharedPlugin};
use os_engine::{SharedEffect, SharedInstrument};


/// A plugin the host knows (offered in the plugin menus unless hidden or combined away).
#[derive(Clone)]
pub struct Available {
    pub lib: Arc<dyn Library>,
    pub desc: Descriptor,
}

pub struct Hosted {
    pub plugin: SharedPlugin,
    /// The channel or slot plugin it plays (`os_model::Id`): the control plane keeps it with them when rows or slots
    /// move under it (`App::follow_ids`). `Id::NONE` until the next tick takes the id of what is under its key.
    pub owner: os_model::Id,
    /// Processing time and voices, read without locking the instance (spec: ui/plugin-performance.md §3,
    /// audio/voices.md §5).
    pub meter: Option<Arc<os_host::Meter>>,
    /// The plugin's name (the performance monitor).
    pub name: String,
    /// The app was told that the instance stopped after a crash.
    stop_told: bool,
    /// A native instance started from a project's state that it did not give back byte for byte: a save keeps the
    /// file's bytes instead of this instance's state (spec: plugins/fruity-abi.md "Saved state").
    pub keep_file_state: bool,
    /// A copy in another format standing in for the wrapped plugin a project names (`project_plugins::switched_format`),
    /// not edited by the user yet: a save leaves the channel's or slot's wrapper bytes as the file had them, so the named
    /// plugin and its settings stay in the project. Playback and automation moving its parameters are not edits; the
    /// user's own are (`Plugins::mark_edited`). Spec: flp/events/plugins.md §4, "copy in another format".
    pub stand_in: bool,
    /// Its attention count, read without locking it (`os_host::Attention`); `None`: looked at on every tick.
    pub attention: Option<Arc<os_host::Attention>>,
    /// The count and time `idle` last looked at it (`Hosted::due`).
    seen: Look,
    /// Its spare state (window or slot menu ▸ Copy to B; `presets::PCmd::StoreSpare`): a second copy of its
    /// state that Swap A / B swaps with the one in use. Lemon Studio's choice: kept in memory with the
    /// instance only, not in the project (spec: ui/plugin-window.md "Spare state").
    pub spare: Option<Vec<u8>>,
    /// "Display preset name in preset selector" unticked: its window's Presets box shows no preset name.
    pub hide_preset: bool,
}

/// When the UI last locked an instance for one of its per-tick jobs: the attention count it acted on, and when.
#[derive(Debug, Clone, Copy)]
pub struct Look {
    pub count: u64,
    pub at: Instant,
}

impl Look {
    /// Never looked at yet: due at once.
    pub fn never() -> Look {
        Look { count: u64::MAX, at: Instant::now() }
    }
}

/// An instance whose count did not move is still looked at this often (anything a plugin did not report).
const LOOK_EVERY: Duration = Duration::from_millis(250);


/// How long a whole-state read waits for its moment (`lock_between_blocks`): two blocks of 256 frames and more.
pub const STATE_WAIT: Duration = Duration::from_millis(12);
/// How long the UI's per-tick work may wait for such moments, all instances together, per `TICK_WINDOW`: the wait is on
/// the UI thread, and a project has many instances. Past it, an instance not free at once skips the tick.
const TICK_BUDGET: Duration = Duration::from_millis(3);
const TICK_WINDOW: Duration = Duration::from_millis(16);

thread_local! {
    /// When the current `TICK_WINDOW` started and what it has waited.
    static TICK_WAITED: std::cell::Cell<Option<(Instant, Duration)>> = const { std::cell::Cell::new(None) };
}

/// Locks `m` for the UI's per-tick work on an instance (its idle, its parameters, a keyboard's screen), `None` to skip
/// the tick. One whose holds are short (`Meter::holds_long`) is taken if free, with no wait; a long one waits for the
/// moment between two blocks (`lock_between_blocks`), out of `TICK_BUDGET`.
pub fn lock_for_tick<'a, T: ?Sized>(m: &'a std::sync::Mutex<T>, meter: Option<&os_host::Meter>) -> Option<std::sync::MutexGuard<'a, T>> {
    if !meter.is_some_and(os_host::Meter::holds_long) {
        return m.try_lock().ok();
    }
    let t0 = Instant::now();
    let (start, waited) = TICK_WAITED.get().filter(|(s, _)| t0.duration_since(*s) < TICK_WINDOW).unwrap_or((t0, Duration::ZERO));
    let got = lock_between_blocks(m, meter, TICK_BUDGET.saturating_sub(waited));
    TICK_WAITED.set(Some((start, waited + t0.elapsed())));
    got
}

/// Locks `m` for a UI call that may hold it a few ms: right after the audio thread processed it (`meter`), when it will
/// not need it again for most of a block (it gives up on an instance it waits for over 2 ms: a late plugin), else at
/// once when nothing is processing it. Waits up to `wait` for that moment; `None` when it did not come.
pub fn lock_between_blocks<'a, T: ?Sized>(m: &'a std::sync::Mutex<T>, meter: Option<&os_host::Meter>, wait: Duration) -> Option<std::sync::MutexGuard<'a, T>> {
    const JUST: Duration = Duration::from_millis(1);
    const IDLE: Duration = Duration::from_millis(50);
    let t0 = Instant::now();
    loop {
        let free = match meter.and_then(|m| m.since_processed()) {
            None => true,
            Some(d) => d <= JUST || d >= IDLE,
        };
        if free {
            // The audio thread holds it only while processing: a short wait if it just started another block.
            return m.lock().ok();
        }
        if t0.elapsed() >= wait {
            return None;
        }
        std::thread::sleep(Duration::from_micros(200));
    }
}

impl Hosted {
    /// A newly hosted instance, without a window.
    pub fn new(plugin: SharedPlugin) -> Hosted {
        let (meter, attention, name) = plugin.lock().map(|p| (p.meter(), p.attention(), p.desc().name.clone())).unwrap_or_default();
        Hosted {
            plugin,
            owner: os_model::Id::NONE,
            meter,
            name,
            stop_told: false,
            keep_file_state: false,
            stand_in: false,
            attention,
            seen: Look::never(),
            spare: None,
            hide_preset: false,
        }
    }

    /// Locks the instance to read its whole state (a save, an autosave): between two of its blocks
    /// ([`lock_between_blocks`]), else once the audio thread lets go of it. Never fails for want of the moment.
    pub fn lock_for_state(&self) -> Option<std::sync::MutexGuard<'_, dyn os_host::Plugin + 'static>> {
        self.lock_between_blocks().or_else(|| self.plugin.lock().ok())
    }

    /// Locks the instance for a UI call that may hold it a few ms (reading its state), between two of its blocks
    /// ([`lock_between_blocks`]).
    pub fn lock_between_blocks(&self) -> Option<std::sync::MutexGuard<'_, dyn os_host::Plugin + 'static>> {
        lock_between_blocks(&self.plugin, self.meter.as_deref(), STATE_WAIT)
    }

    /// Whether a per-tick job last done at `last` should lock the instance now, and the look to remember if it does:
    /// its attention count moved, its editor is open, it has no count, or `LOOK_EVERY` passed. Otherwise the UI leaves
    /// it to the audio thread (spec: audio/implementing.md "Playing live").
    /// How often the instance has asked for attention (a parameter changed in its window, a state to save...); None
    /// for one that never says.
    pub fn attention_count(&self) -> Option<u64> {
        self.attention.as_ref().map(|a| a.count())
    }

    pub fn due(&self, last: Look, editor: bool) -> Option<Look> {
        let now = Instant::now();
        let Some(a) = &self.attention else {
            return Some(Look { count: 0, at: now });
        };
        let count = a.count();
        (count != last.count || editor || now.duration_since(last.at) >= LOOK_EVERY).then_some(Look { count, at: now })
    }

    /// The instance stopped after a crash.
    pub fn stopped(&self) -> bool {
        self.stop_told
    }

}

/// What happened to an instance that its editor window follows: recorded by the host, replayed in order by the
/// interface's `EditorHost` (step 7.3b: the windows are the interface's).
#[derive(Debug, Clone, PartialEq)]
pub enum EditorEvent {
    /// The user added the instance under `.0`: its window opens detached (`.1`) or docked.
    Detached(usize, bool),
    /// The instance under `.0` went: its window closes.
    Drop(usize),
    /// Show the preset name of the instance under `.0` in its open window.
    PresetShown(usize),
    /// Slot instances moved: (old key, new key), in the order they moved.
    FxMoves(Vec<(usize, usize)>),
    /// The rack rows' instances moved: each hosted row's new row, `None` for one that went.
    Remap(Vec<(usize, Option<usize>)>),
    /// Every instance went (the hosted keys there were).
    Clear(Vec<usize>),
}

/// Keys from here up are mixer slots, not rack rows: `FX_KEY + track * 16 + slot` (spec: ui/mixer.md §7).
pub const FX_KEY: usize = 1 << 24;

/// Keys from here up are the effects inside containers (`crate::container`), numbered in the order they were made
/// (`Plugins::next_member`): neither rack rows nor slots, hosted for their windows, parameters, presets and
/// housekeeping. Automation and links name them by their container's slot and their id instead (`params::target_of`).
pub const MEMBER_KEY: usize = 1 << 26;

/// Plugin editor windows open at once under "Limit the number of open plugin editors" (spec:
/// ui/settings-choices.md "Miscellaneous: naming, detached plugins and startup").
pub const MAX_EDITORS: usize = 25;

/// The `hosted` key of mixer track `track`'s slot `slot`.
pub fn fx_key(track: u16, slot: u8) -> usize {
    FX_KEY + usize::from(track) * 16 + usize::from(slot)
}

/// The mixer (track, slot) of a `hosted` key, `None` for rack rows and container members.
pub fn fx_of(key: usize) -> Option<(u16, u8)> {
    (FX_KEY..MEMBER_KEY).contains(&key).then(|| (((key - FX_KEY) / 16) as u16, ((key - FX_KEY) % 16) as u8))
}

#[derive(Default)]
pub struct Plugins {
    pub available: Vec<Available>,
    /// Effect plugins found (offered for mixer slots).
    pub effects: Vec<Available>,
    /// By rack row, or by `fx_key` for mixer slots.
    pub hosted: BTreeMap<usize, Hosted>,
    /// What happened to the instances that their editor windows follow (`crate::editor_host`), oldest first.
    pub editor_events: Vec<EditorEvent>,
    /// The keys whose editor window is open, as the interface last said (`EditorHost::open_keys`): an instance with an
    /// open editor is looked at every tick.
    pub editors_open: std::collections::BTreeSet<usize>,
    /// Main window handle (owner of plugin windows).
    pub owner: isize,
    pub scale: f32,
    /// The preset file each instance last loaded or saved (spec: plugins/presets.md), by `hosted` key.
    pub presets: BTreeMap<usize, PathBuf>,
    /// Presets dropped where a new instance is being made: they load once it is hosted (`presets::pending_tick`).
    pub preset_waits: Vec<(usize, PathBuf, std::time::Instant)>,
    /// Plugins (uids) whose file the last scan no longer found: not listed, kept so indices stay valid.
    pub missing: HashSet<String>,
    /// The file and stamp each found plugin was listed from (uid -> path, size, time): an unchanged file keeps its
    /// library (and a library loaded for use stays loaded).
    stamps: HashMap<String, (String, u64, u64)>,
    /// The rate new instances start at; 0 until the first one asks the output device. An instance handed a block at any
    /// other rate stays silent until the UI moves it, and the device's rate depends on the machine: a test that
    /// processes at a literal rate pins it first (`set_rate`).
    pub rate: u32,
    /// Verdicts of the one-time child-process check a native plugin passes before its first load here (spec:
    /// plugins/fruity-abi.md "Crash containment"). `None`: no check (tests, where this program is the test harness).
    #[cfg(feature = "fl-native")]
    native_checks: Option<os_native::check::Checks>,
    /// Instances that went with their channel or slot, by its id: (an effect, the plugin's uid, its saved state).
    /// Only those whose channel or slot went (`keep`); an undo that brings it back hosts it again from here
    /// (`App::follow_ids`); the oldest go past `STASH_MAX`.
    pub stash: Vec<(os_model::Id, bool, String, Vec<u8>)>,
    /// A plugin menu's Scaling ▸ (Lemon Studio's design, session only), by `hosted` key: the scale its window and
    /// editor are drawn at instead of the program's (`scale_for`).
    pub editor_scales: BTreeMap<usize, f32>,
    /// The containers playing mixer slots, by `fx_key` (`crate::container`).
    pub containers: BTreeMap<usize, crate::container::Live>,
    /// The next container member's key, counted from `MEMBER_KEY`.
    pub next_member: usize,
    /// The Lemon MIDI Out instruments, by rack row (`App::sync_midi_outs`; spec: plugins/midi-out.md §6).
    pub midi_outs: BTreeMap<usize, std::sync::Arc<std::sync::Mutex<crate::lemon_midi_out::MidiOutInstrument>>>,
    /// Sampler channel rows whose Lemon Sampler instance could not be created (not retried).
    pub sampler_failed: Vec<usize>,
    /// Mixer slots (`plugins::fx_key`) whose Lemon Parametric EQ could not start (not retried).
    pub fx_failed: Vec<usize>,
    /// Wrapped plugin channels (rows) and slots (`fx_key`) whose plugin was not found or did not start (not retried).
    pub wrap_failed: Vec<usize>,
    /// Plugin channels (rows) and slots (`fx_key`) naming a native plugin that is not installed or did not start
    /// (not retried; spec: plugins/fruity-abi.md "Opened projects").
    pub native_failed: Vec<usize>,
    /// While a project loads: when the plugin hosts stop for this tick (`host_plugins`), and whether one stopped with
    /// instances left to start.
    pub host_deadline: Option<std::time::Instant>,
    pub host_more: bool,
    /// Plugins the project names that are not installed or did not start (`project_plugins::sync_wrapped`,
    /// `host_native`), each once: the missing files box lists them.
    pub not_found: Vec<String>,
}

impl Plugins {
    /// One instance's tick on the UI thread (`EditorHost::idle`, and without windows `idle_headless`): its plugin's
    /// idle, a crash told once, and what its editor shows. `menu_at` places a menu the plugin asks for (`None`: no
    /// window to show it over, closed at once); `size` takes the editor size the plugin asks for. Returns what to tell
    /// the user of a crash and the menu to show.
    pub fn tick_row(h: &mut Hosted, row: usize, open: bool, menu_at: impl FnOnce(&os_host::Menu) -> Option<(i32, i32)>, size: impl FnOnce((u32, u32))) -> (Option<String>, Option<(os_host::Menu, (i32, i32))>) {
        let look = h.due(h.seen, open);
        let mut stopped = None;
        let mut menu = None;
        if let Some(mut p) = look.and_then(|_| lock_for_tick(&h.plugin, h.meter.as_deref())) {
            // The dropout log is Linux only; elsewhere the hold is only timed.
            #[cfg(target_os = "linux")]
            let t0 = crate::dropout_log::Hold::start();
            #[cfg(not(target_os = "linux"))]
            let t0 = std::time::Instant::now();
            h.seen = look.unwrap_or(h.seen);
            // A crash stopped it (spec: plugins/plugin-formats.md, "Crash containment"): said once, window closed.
            if let (Some(code), false) = (p.stopped(), h.stop_told) {
                h.stop_told = true;
                stopped = Some(stopped_message(&p.desc().name, code, row >= FX_KEY));
            }
            p.idle();
            // A menu the plugin asked to pop up over its editor: at its point on the screen, or closed at once
            // without a window to show it over (spec: plugins/vst3-hosting.md §9).
            if let Some(m) = p.take_menu() {
                match menu_at(&m) {
                    Some(at) => menu = Some((m, at)),
                    None => p.menu_done(m.id, None),
                }
            }
            if let Some(s) = p.editor_size() {
                size(s);
            }
            if let Some(m) = &h.meter {
                m.note_hold(t0.elapsed());
            }
            #[cfg(target_os = "linux")]
            crate::dropout_log::held(&p.desc().name, "the UI's tick", t0);
        }
        (stopped, menu)
    }

    /// The UI tick's plugin housekeeping without windows (`--render`, `--serve`).
    pub fn idle_headless(&mut self) {
        sweep_retired();
        os_host::guard::watch_windows(false);
        #[cfg(target_os = "linux")]
        os_host::runloop::dispatch();
        for (&row, h) in &mut self.hosted {
            let _ = Self::tick_row(h, row, false, |_| None, |_| {});
        }
        self.editor_events.clear();
    }

    /// Adds plugin names to `not_found` (each once; "Serum (error text)" counts as its own entry).
    pub fn add_not_found(&mut self, names: &[String]) {
        for n in names {
            if !self.not_found.contains(n) {
                self.not_found.push(n.clone());
            }
        }
    }

    /// While a project loads, the hosting of this tick is over (`host_deadline`): it goes on at the next.
    pub fn past_deadline(&mut self) -> bool {
        let past = self.host_deadline.is_some_and(|d| std::time::Instant::now() >= d);
        self.host_more |= past;
        past
    }
}

/// How many gone instances `Plugins::stash` keeps.
const STASH_MAX: usize = 64;

/// A stock plugin's CLAP entry table when its feature is on (`None`: not in this build).
macro_rules! stock {
    ($feature:literal, $krate:ident) => {{
        #[cfg(feature = $feature)]
        let e = Some(&$krate::LINKED_CLAP_ENTRY as *const _ as *const std::ffi::c_void);
        #[cfg(not(feature = $feature))]
        let e = None;
        e
    }};
}

/// Lemon Sampler compiled into Lemon Studio and hosted through its own CLAP entry, so sampler channels never depend on
/// finding a `.clap` file (spec: plugins/lemon-sampler.md §7); Lemon Trio, Citrus, Smith, Lemon Slicer and Lemon Drum
/// Sampler likewise (spec: plugins/lemon-3x-osc.md §6, plugins/citrus.md, plugins/smith.md, plugins/lemon-slicer.md §6,
/// plugins/lemon-drum-sampler.md §4).
pub fn builtin() -> Vec<Available> {
    static LIB: OnceLock<Option<Arc<os_clap::Library>>> = OnceLock::new();
    static OSC3: OnceLock<Option<Arc<os_clap::Library>>> = OnceLock::new();
    static CITRUS: OnceLock<Option<Arc<os_clap::Library>>> = OnceLock::new();
    static SMITH: OnceLock<Option<Arc<os_clap::Library>>> = OnceLock::new();
    static SLICER: OnceLock<Option<Arc<os_clap::Library>>> = OnceLock::new();
    static DRUMS: OnceLock<Option<Arc<os_clap::Library>>> = OnceLock::new();
    static ZEST: OnceLock<Option<Arc<os_clap::Library>>> = OnceLock::new();
    static SOUNDFONT: OnceLock<Option<Arc<os_clap::Library>>> = OnceLock::new();
    let mut v = linked(&LIB, &lemon_sampler::LINKED_CLAP_ENTRY as *const _ as *const std::ffi::c_void, LEMON_SAMPLER);
    v.extend(stock!("stock-trio", lemon_3x_osc).map(|e| linked(&OSC3, e, crate::osc3::LEMON_TRIO)).unwrap_or_default());
    v.extend(stock!("stock-citrus", citrus).map(|e| linked(&CITRUS, e, crate::citrus::CITRUS)).unwrap_or_default());
    v.extend(stock!("stock-smith", smith).map(|e| linked(&SMITH, e, crate::smith::SMITH)).unwrap_or_default());
    v.extend(stock!("stock-slicer", lemon_slicer).map(|e| linked(&SLICER, e, crate::slicer::LEMON_SLICER)).unwrap_or_default());
    v.extend(stock!("stock-drum-sampler", lemon_drum_sampler).map(|e| linked(&DRUMS, e, crate::drums::LEMON_DRUMS)).unwrap_or_default());
    v.extend(stock!("stock-zest", zest_plugin).map(|e| linked(&ZEST, e, ZEST_NAME)).unwrap_or_default());
    v.extend(stock!("stock-soundfont", lemon_soundfont).map(|e| linked(&SOUNDFONT, e, crate::soundfont::LEMON_SOUNDFONT)).unwrap_or_default());
    v
}

/// Zest. Saved as a wrapped CLAP plugin naming `Zest.clap` in the per-user CLAP folder, as the compiled-in effects are
/// (`project_plugins::is_wrapped`): without it a project keeps no state for it, not even which plugin played the channel.
pub const ZEST_NAME: &str = "Zest";

/// Lemon Parametric EQ, Pulp Filter and Press, compiled in the same way (`FILE_MAPPED`): offered for every mixer slot,
/// and they play the slots a project saved with the original EQ, Love Philter and Maximus (spec:
/// plugins/lemon-parametric-eq-2.md §8, plugins/lemon-pulp-filter.md §6, plugins/lemon-press.md §6). Then Lemon Studio's other effects (`LINKED_EFFECTS`), compiled in too, so
/// they need no plugin file either.
pub fn builtin_effects() -> Vec<Available> {
    const N: usize = FILE_MAPPED.len() + LINKED_EFFECTS.len();
    static LIBS: [OnceLock<Option<Arc<os_clap::Library>>>; N] = [const { OnceLock::new() }; N];
    let entries: [Option<*const std::ffi::c_void>; N] = [
        stock!("stock-parametric-eq-2", lemon_parametric_eq_2),
        stock!("stock-pulp-filter", pulp_filter),
        stock!("stock-press", press),
        stock!("stock-limiter", lemon_limiter),
        stock!("stock-reverb", lemon_reverb),
        stock!("stock-delay", lemon_delay),
        stock!("stock-soft-clipper", lemon_soft_clipper),
        stock!("stock-fast-distortion", lemon_fast_distortion),
        stock!("stock-balance", lemon_balance),
        stock!("stock-stereo-enhancer", lemon_stereo_enhancer),
        stock!("stock-transient-processor", lemon_transient_processor),
        stock!("stock-chorus", lemon_chorus),
        stock!("stock-stereo-shaper", lemon_stereo_shaper),
        stock!("stock-filter", lemon_filter),
        Some(&tesla::LINKED_CLAP_ENTRY as *const _ as *const std::ffi::c_void),
        stock!("stock-flanger", lemon_flanger),
        stock!("stock-phaser", lemon_phaser),
        stock!("stock-peak-controller", lemon_peak_controller),
        stock!("stock-formula-controller", lemon_formula_controller),
        stock!("stock-vocoder", lemon_vocoder),
        stock!("stock-drop", lemon_drop),
    ];
    let names = FILE_MAPPED.into_iter().chain(LINKED_EFFECTS);
    LIBS.iter().zip(entries).zip(names).filter_map(|((lib, entry), name)| Some(linked(lib, entry?, name))).flatten().collect()
}

/// Whether `plugin` is one compiled into Lemon Studio, whose editor is drawn in the app's own window (Linux and macOS;
/// `plugin_window::EmbedFrame`). `LEMONSTUDIO_PLUGIN_WINDOWS=1` gives them windows of their own again.
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub fn embeds(plugin: &SharedPlugin) -> bool {
    static IDS: OnceLock<HashSet<String>> = OnceLock::new();
    if std::env::var_os("LEMONSTUDIO_PLUGIN_WINDOWS").is_some_and(|v| v == "1") {
        return false;
    }
    let ids = IDS.get_or_init(|| builtin().into_iter().chain(builtin_effects()).map(|a| a.desc.id).collect());
    plugin.lock().is_ok_and(|p| ids.contains(&p.desc().id))
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

/// Loads plugin file `path` of `format` under the crash guard (spec: plugins/plugin-formats.md, "Crash containment").
pub fn load_file(path: &Path, format: os_host::Format) -> Result<Arc<dyn Library>, String> {
    let load = || -> Result<Arc<dyn Library>, String> {
        match format {
            os_host::Format::Clap => os_clap::Library::load(path).map(|l| l as Arc<dyn Library>),
            #[cfg(feature = "vst3")]
            os_host::Format::Vst3 => os_vst3::Module::load(path).map(|l| l as Arc<dyn Library>),
            #[cfg(not(feature = "vst3"))]
            os_host::Format::Vst3 => Err(format!("{}: VST 3 is not in this build", path.display())),
            #[cfg(feature = "vst2")]
            os_host::Format::Vst2 => os_vst2::Module::load(path).map(|l| l as Arc<dyn Library>),
            #[cfg(not(feature = "vst2"))]
            os_host::Format::Vst2 => Err(format!("{}: VST 2 is not in this build", path.display())),
            #[cfg(feature = "fl-native")]
            os_host::Format::Native => os_native::NativeLibrary::load(path).map(|l| l as Arc<dyn Library>),
            #[cfg(not(feature = "fl-native"))]
            os_host::Format::Native => Err(format!("{}: native plugins are not in this build", path.display())),
        }
    };
    os_host::guard::catch(load).unwrap_or_else(|c| Err(format!("{}: crashed while loading ({})", path.display(), os_host::guard::describe(c))))
}

/// The installed native plugin files (generators and effects), when the experimental native host is opted into
/// (`native::enabled()`, Settings > Plugins or `OPENSTUDIO_NATIVE=1`): they are loaded through an unverified
/// interop path (spec plugins/fruity-abi.md), so they stay off by default. Each is tried in a child process before
/// its first load here (`Plugins::native_check`).
pub fn native_files() -> Vec<PathBuf> {
    #[cfg(feature = "fl-native")]
    if crate::native::enabled() {
        return os_native::dll_files();
    }
    Vec::new()
}

/// A plugin file known from the scan cache: lists the cached plugins without loading the file, and loads it the first
/// time an instance is created (spec: ui/plugin-manager.md §4, "Loading for use").
struct LazyLibrary {
    path: PathBuf,
    format: os_host::Format,
    plugins: Vec<Descriptor>,
    real: OnceLock<Result<Arc<dyn Library>, String>>,
}

impl Library for LazyLibrary {
    fn path(&self) -> &Path {
        &self.path
    }

    fn plugins(&self) -> &[Descriptor] {
        &self.plugins
    }

    fn create(self: Arc<Self>, id: &str) -> Result<SharedPlugin, String> {
        let real = self.real.get_or_init(|| load_file(&self.path, self.format)).clone()?;
        if !real.plugins().iter().any(|d| d.id == id) {
            return Err(format!("{} no longer offers this plugin: scan for plugins again", self.path.display()));
        }
        real.create(id)
    }
}

/// A format's name as the UI shows it.
pub fn format_name(f: os_host::Format) -> &'static str {
    match f {
        os_host::Format::Clap => "CLAP",
        os_host::Format::Vst3 => "VST3",
        os_host::Format::Vst2 => "VST2",
        os_host::Format::Native => "native",
    }
}

impl Plugins {
    /// The built-in plugins only (what the app knows before the scan cache is read).
    pub fn builtins() -> Plugins {
        Plugins {
            available: builtin(),
            effects: builtin_effects(),
            scale: 1.0,
            #[cfg(feature = "fl-native")]
            native_checks: (!cfg!(any(test, feature = "test-util"))).then(|| os_native::check::Checks::load(os_paths::user_file(NATIVE_CHECKS))),
            ..Plugins::default()
        }
    }

    /// Takes a scan's results (spec: ui/plugin-manager.md §4, "Results"): plugins new to this run are appended, so the
    /// indices the lists, channels and slots use stay valid for the whole run; a plugin whose file changed gets a new
    /// lazily loaded library; plugins no longer found are marked missing (not listed). The first file wins for a
    /// plugin id. Built-in plugins are never replaced.
    pub fn merge(&mut self, cache: &crate::plugin_db::Cache) {
        let builtin: HashSet<String> = self.available.iter().chain(&self.effects).filter(|a| crate::project_plugins::is_builtin(&a.desc)).map(|a| a.desc.uid()).collect();
        let mut seen: HashSet<String> = builtin.clone();
        for f in cache.files.iter().filter(|f| f.status == crate::plugin_db::Status::Ok) {
            let Some(format) = os_host::Format::parse(&f.format) else { continue };
            let path = PathBuf::from(&f.path);
            let descs: Vec<Descriptor> = f.plugins.iter().map(|r| r.descriptor(format, &path)).collect();
            let lib: Arc<dyn Library> = Arc::new(LazyLibrary { path: path.clone(), format, plugins: descs.clone(), real: OnceLock::new() });
            for d in descs {
                let uid = d.uid();
                let list = match d.kind {
                    os_host::Kind::Instrument => &mut self.available,
                    os_host::Kind::Effect => &mut self.effects,
                    os_host::Kind::Other => continue,
                };
                if !seen.insert(uid.clone()) {
                    continue;
                }
                let stamp = (f.path.clone(), f.size, f.modified);
                match list.iter().position(|a| a.desc.uid() == uid) {
                    Some(i) if self.stamps.get(&uid) == Some(&stamp) && list[i].desc == d => {}
                    Some(i) => list[i] = Available { lib: lib.clone(), desc: d },
                    None => list.push(Available { lib: lib.clone(), desc: d }),
                }
                self.stamps.insert(uid, stamp);
            }
        }
        self.missing = self.available.iter().chain(&self.effects).map(|a| a.desc.uid()).filter(|u| !seen.contains(u)).collect();
        if std::env::var_os("OS_DEBUG").is_some() {
            eprintln!("[plugins] {} instruments, {} effects, {} missing", self.available.len(), self.effects.len(), self.missing.len());
        }
    }

    /// Native plugins load into this process only once a child process tried them (`os_native::check`): one that
    /// crashes, hangs or stops with an error there is refused with the reason.
    fn native_check(&mut self, desc: &Descriptor) -> Result<(), String> {
        if desc.format != os_host::Format::Native {
            return Ok(());
        }
        #[cfg(not(feature = "fl-native"))]
        return Err(format!("{}: native plugins are not in this build", desc.name));
        #[cfg(feature = "fl-native")]
        {
            let (Some(checks), Ok(program)) = (&mut self.native_checks, std::env::current_exe()) else { return Ok(()) };
            match checks.verdict(&program, &desc.path) {
                os_native::check::Verdict::Works => Ok(()),
                os_native::check::Verdict::Fails(why) => Err(format!("{} can't load ({why})", desc.name)),
            }
        }
    }

    /// Whether effect `index` can be created without waiting: anything but a native effect whose check has not run
    /// yet. That check is started in the background; ask again on a later tick.
    pub fn effect_ready(&mut self, index: usize) -> bool {
        let path = self.effects.get(index).filter(|a| a.desc.format == os_host::Format::Native).map(|a| a.desc.path.clone());
        path.is_none_or(|p| self.native_ready(&p))
    }

    /// `effect_ready` for instrument `index` (of `available`).
    pub fn instrument_ready(&mut self, index: usize) -> bool {
        let path = self.available.get(index).filter(|a| a.desc.format == os_host::Format::Native).map(|a| a.desc.path.clone());
        path.is_none_or(|p| self.native_ready(&p))
    }

    fn native_ready(&mut self, dll: &std::path::Path) -> bool {
        #[cfg(not(feature = "fl-native"))]
        return {
            let _ = dll;
            true
        };
        #[cfg(feature = "fl-native")]
        {
            let (Some(checks), Ok(program)) = (&mut self.native_checks, std::env::current_exe()) else { return true };
            checks.ready(&program, dll)
        }
    }

    /// The built-in plugins plus every plugin file in the standard folders, scanned in this process at once (tests).
    #[cfg(any(test, feature = "test-util"))]
    pub fn scan() -> Plugins {
        use crate::plugin_db as db;
        let prefs = db::Prefs::default();
        let found = db::discover(&db::search_dirs(&prefs), native_files());
        let scan = |p: &Path, f: os_host::Format| crate::plugin_scan::scan_here(p, f);
        let cache = db::sweep(&found, &db::Cache::default(), db::Mode::All, false, 1, &scan, &|_, _, _| {});
        let mut pl = Plugins::builtins();
        pl.merge(&cache);
        pl
    }

    /// Creates an instance of available plugin `index` for rack row `row` and activates it at the device rate.
    pub fn create(&mut self, row: usize, index: usize) -> Result<String, String> {
        self.create_with(row, index, None)
    }

    /// `create` with a starting state (the plugin's own saved state format).
    pub fn create_with(&mut self, row: usize, index: usize, state: Option<&[u8]>) -> Result<String, String> {
        let (plugin, name) = self.instantiate(index, state)?;
        self.hosted.insert(row, Hosted::new(plugin));
        Ok(name)
    }

    /// An instance of instrument `index` (of `available`) started from `state`, activated at the device rate, not
    /// hosted under any key (`create_with` hosts it; a channel's container's member, `crate::container`).
    pub fn instantiate(&mut self, index: usize, state: Option<&[u8]>) -> Result<(SharedPlugin, String), String> {
        let a = self.available.get(index).ok_or("no such plugin")?.clone();
        self.native_check(&a.desc)?;
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
        // Trimmed audio clips start their note further into the sample (spec: plugins/lemon-sampler.md §7).
        p.use_start_param(SAMPLE_START);
        // A note's own pan and mod X / Y reach the built-in Lemon instruments only (spec: audio/voice-levels.md); audio
        // clips' fades and gain reach Lemon Sampler's voices one by one, so overlapping clips of one sample crossfade
        // (spec: audio/audio-clips.md "Fades and clip gain"); the instruments work the channel's volume and pan into
        // their voices' final levels (spec: audio/voice-levels.md "Final levels").
        // A third-party plugin gets the channel's levels as FL's plugin wrapper gives them (spec: audio/voice-levels.md
        // "A plugin's output").
        if !crate::project_plugins::is_builtin(&a.desc) {
            p.use_wrapper_levels();
        }
        if crate::project_plugins::is_builtin(&a.desc) {
            p.use_note_levels();
            if a.desc.name == LEMON_SAMPLER {
                p.use_voice_gains();
                // Each audio clip's voice ends at its trimmed end, in the sampler (spec: audio/audio-clips.md "What a
                // clip plays").
                p.use_end_param(CLIP_END);
            }
            if [LEMON_SAMPLER, crate::osc3::LEMON_TRIO, crate::citrus::CITRUS].contains(&a.desc.name.as_str()) {
                p.use_channel_levels();
            }
            if a.desc.name == crate::citrus::CITRUS {
                p.use_param_aliases(crate::citrus::SYTRUS_PARAMS);
            }
        }
        drop(p);
        Ok((plugin, a.desc.name))
    }

    /// The pitch-bend range (semitones) instrument `row` takes glides over when it has no tuning of a note (spec:
    /// audio/channel-voices.md).
    pub fn set_bend_range(&self, row: usize, semitones: u32) {
        if let Some(Ok(mut p)) = self.hosted.get(&row).map(|h| h.plugin.lock()) {
            p.set_bend_range(semitones);
        }
    }

    /// Creates effect `index` (of `effects`) in mixer track `track`'s slot `slot`, replacing what was there.
    pub fn create_effect(&mut self, track: u16, slot: u8, index: usize, state: Option<&[u8]>) -> Result<String, String> {
        self.drop_key(fx_key(track, slot));
        let (plugin, name) = self.instantiate_effect(index, state)?;
        self.hosted.insert(fx_key(track, slot), Hosted::new(plugin));
        Ok(name)
    }

    /// An instance of effect `index` (of `effects`) started from `state`, activated at the device rate, not hosted under
    /// any key (a container's member, `crate::container`).
    pub fn instantiate_effect(&mut self, index: usize, state: Option<&[u8]>) -> Result<(SharedPlugin, String), String> {
        let a = self.effects.get(index).ok_or("no such effect")?.clone();
        self.native_check(&a.desc)?;
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
        Ok((plugin, a.desc.name))
    }

    /// The user just added the instance under `key` (a new channel's plugin, a new effect in a slot): its window opens
    /// detached while "Open new plugin windows floating" is on. Instances a project brings open docked, as before (spec:
    /// ui/settings-choices.md "Miscellaneous: naming, detached plugins and startup").
    pub fn added(&mut self, st: &crate::settings_file::SettingsStore, key: usize) {
        let on = st.check(&crate::prefs::DETACH_NEW);
        if self.hosted.contains_key(&key) {
            self.editor_events.push(EditorEvent::Detached(key, on));
        }
    }

    /// Removes the instance under `key` (closing its window first).
    pub fn drop_key(&mut self, key: usize) {
        self.editor_events.push(EditorEvent::Drop(key));
        self.hosted.remove(&key);
        self.presets.remove(&key);
    }

    /// Keeps the state of the instance under `key`, about to go with its channel or slot, in `stash` by its owner's id.
    pub fn keep(&mut self, key: usize) {
        if let Some(h) = self.hosted.remove(&key) {
            self.keep_hosted(&h, key >= FX_KEY);
            self.hosted.insert(key, h);
        }
    }

    /// The state of the instance under `key` kept in `stash` for `owner` too (a cloned channel, hosted from it on the
    /// next tick as a channel back from an undo is: `control::host::follow_apply`).
    pub fn stash_copy(&mut self, key: usize, owner: os_model::Id) {
        let Some(h) = self.hosted.get(&key).filter(|h| !h.stopped()) else { return };
        let Some((uid, state)) = h.lock_for_state().and_then(|p| Some((p.desc().uid(), p.save_state()?))) else { return };
        self.stash.retain(|e| e.0 .0 != owner.0);
        self.stash.push((owner, key >= FX_KEY, uid, state));
        if self.stash.len() > STASH_MAX {
            self.stash.remove(0);
        }
    }

    /// `keep` for an instance already taken out of `hosted`.
    fn keep_hosted(&mut self, h: &Hosted, fx: bool) {
        if h.owner.is_none() || h.stopped() {
            return;
        }
        let Some((uid, state)) = h.lock_for_state().and_then(|p| Some((p.desc().uid(), p.save_state()?))) else { return };
        let owner = h.owner;
        self.stash.retain(|e| e.0 .0 != owner.0);
        self.stash.push((owner, fx, uid, state));
        if self.stash.len() > STASH_MAX {
            self.stash.remove(0);
        }
    }

    /// Remembers the preset the instance under `key` holds and shows its name in the window's Presets box.
    pub fn set_preset(&mut self, key: usize, path: Option<PathBuf>) {
        match path {
            Some(p) => self.presets.insert(key, p),
            None => self.presets.remove(&key),
        };
        self.editor_events.push(EditorEvent::PresetShown(key));
    }

    /// The name of the preset the instance under `key` holds.
    pub fn preset_name(&self, key: usize) -> Option<String> {
        self.presets.get(&key).map(|p| crate::presets::shown_name(p))
    }

    /// What the window's Presets box of the instance under `key` shows: its preset's name, none while "Display preset
    /// name in preset selector" is unticked.
    pub fn shown_preset(&self, key: usize) -> Option<String> {
        self.preset_name(key).filter(|_| !self.hosted.get(&key).is_some_and(|h| h.hide_preset))
    }

    /// Window or slot menu ▸ Display preset name in preset selector: ticks (`show`) or unticks it for the instance under
    /// `key`; the open window's Presets box follows.
    pub fn show_preset_name(&mut self, key: usize, show: bool) {
        let Some(h) = self.hosted.get_mut(&key) else { return };
        h.hide_preset = !show;
        self.editor_events.push(EditorEvent::PresetShown(key));
    }

    /// Moves mixer slot instances: `m(track, slot)` gives the new place, `None` drops the instance.
    pub fn remap_fx(&mut self, m: &dyn Fn(u16, u8) -> Option<(u16, u8)>) {
        let keys: Vec<usize> = self.hosted.keys().copied().filter(|&k| fx_of(k).is_some()).collect();
        let mut moved = Vec::new();
        for k in keys {
            let (t, s) = fx_of(k).unwrap();
            match m(t, s) {
                Some((nt, ns)) if (nt, ns) == (t, s) => {}
                Some((nt, ns)) => {
                    if let Some(p) = self.presets.remove(&k) {
                        self.presets.insert(fx_key(nt, ns), p);
                    }
                    moved.push((fx_key(nt, ns), self.hosted.remove(&k).unwrap(), k))
                }
                None => {
                    self.keep(k);
                    self.drop_key(k);
                }
            }
        }
        self.editor_events.push(EditorEvent::FxMoves(moved.iter().map(|&(k, _, from)| (from, k)).collect()));
        for (k, h, _) in moved {
            if let Some(old) = self.hosted.insert(k, h) {
                self.keep_hosted(&old, true);
            }
        }
    }

    /// The mixer slot instances for the transport: (track, slot, effect).
    pub fn effects(&self) -> Vec<(usize, usize, SharedEffect)> {
        let hosted = self.hosted.range(FX_KEY..MEMBER_KEY).map(|(&k, h)| (k, h.plugin.clone() as SharedEffect));
        // A container's chain plays its slot (after a hosted instance still under the key, which it replaces); a
        // channel's container is an instrument (`instruments`).
        let chains = self.containers.range(FX_KEY..).map(|(&k, c)| (k, c.fx.clone() as SharedEffect));
        hosted
            .chain(chains)
            .map(|(k, fx)| {
                let (t, s) = fx_of(k).unwrap();
                (usize::from(t), usize::from(s), fx)
            })
            .collect()
    }

    /// Index of the available plugin called `name` (the built-in plugins come first).
    pub fn index_of(&self, name: &str) -> Option<usize> {
        self.available.iter().position(|a| a.desc.name == name)
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
        self.hosted.get(&key)?.lock_for_state()?.save_state()
    }

    /// The state of the instance under `key`, read between two of its blocks (`Hosted::lock_between_blocks`): `Err`
    /// when no such moment came in time.
    pub fn state_if_free(&self, key: usize) -> Result<Option<Vec<u8>>, ()> {
        let Some(h) = self.hosted.get(&key) else { return Ok(None) };
        let Some(p) = h.lock_between_blocks() else { return Err(()) };
        // The dropout log is Linux only.
        #[cfg(target_os = "linux")]
        let t0 = crate::dropout_log::Hold::start();
        let state = p.save_state();
        #[cfg(target_os = "linux")]
        crate::dropout_log::held(&p.desc().name, "reading its state", t0);
        Ok(state)
    }


    /// Marks the instance under `key`, just created, as a copy standing in for a plugin in another format
    /// (`Hosted::stand_in`).
    pub fn mark_stand_in(&mut self, key: usize) {
        if let Some(h) = self.hosted.get_mut(&key) {
            h.stand_in = true;
        }
    }

    /// The user edited the instance under `key`: in its own window, with the parameter menu, or by loading a preset. A
    /// copy standing in for another format's plugin is saved as itself from then on (`Hosted::stand_in`).
    pub fn mark_edited(&mut self, key: usize) {
        if let Some(h) = self.hosted.get_mut(&key) {
            h.stand_in = false;
        }
    }

    /// The names of the instance's outputs past its main one (spec: audio/multi-output.md); empty when it has none or
    /// nothing is hosted under `key`.
    pub fn extra_output_names(&self, key: usize) -> Vec<String> {
        self.hosted.get(&key).and_then(|h| h.plugin.lock().ok()).map(|p| p.extra_output_names()).unwrap_or_default()
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

    /// The parameters of the instance under `key`, in the plugin's order.
    pub fn param_infos(&self, key: usize) -> Vec<os_host::ParamInfo> {
        let Some(p) = self.hosted.get(&key).and_then(|h| h.plugin.lock().ok()) else { return Vec::new() };
        (0..p.param_count()).filter_map(|i| p.param_info(i)).collect()
    }

    /// The normalized value of parameter `id` of the instance under `key`.
    pub fn param_value(&self, key: usize, id: u32) -> Option<f64> {
        self.hosted.get(&key)?.plugin.lock().ok()?.param_value(id)
    }

    /// A parameter-list preset (spec: plugins/presets.md "File format"): every parameter that is not read-only or the
    /// bypass (hidden ones too, which a preset may also list) goes to its default, then each listed one (by name, case ignored where no exact match) to its plain value. Returns the
    /// listed names the plugin does not have; `None` when nothing is hosted under `key`.
    pub fn apply_params(&self, key: usize, list: &[(String, f64)]) -> Option<Vec<String>> {
        let mut p = self.hosted.get(&key)?.plugin.lock().ok()?;
        let infos: Vec<os_host::ParamInfo> = (0..p.param_count()).filter_map(|i| p.param_info(i)).collect();
        for i in infos.iter().filter(|i| !i.read_only && !i.bypass) {
            p.set_param(i.id, i.default);
        }
        let mut missing = Vec::new();
        for (name, plain) in list {
            let found = infos.iter().find(|i| &i.name == name).or_else(|| infos.iter().find(|i| i.name.eq_ignore_ascii_case(name)));
            match found {
                Some(i) => {
                    let n = if i.max > i.min { (plain - i.min) / (i.max - i.min) } else { 0.0 };
                    p.set_param(i.id, n.clamp(0.0, 1.0));
                }
                None => missing.push(name.clone()),
            }
        }
        Some(missing)
    }

    /// The project-file state of the Lemon Parametric EQ in mixer slot `key` (its plugin state converted back),
    /// `None` when the slot hosts something else.
    pub fn eq_file_state(&self, key: usize) -> Option<Vec<u8>> {
        let h = self.hosted.get(&key)?;
        let p = h.lock_for_state()?;
        if p.desc().name != LEMON_EQ {
            return None;
        }
        #[cfg(not(feature = "stock-parametric-eq-2"))]
        return None;
        #[cfg(feature = "stock-parametric-eq-2")]
        lemon_parametric_eq_2::params::fl_from_plugin_state(&p.save_state()?)
    }

    /// Edison's state (the project file's form) of the Tesla hosted under `key`; `None` for any other plugin.
    pub fn edison_file_state(&self, key: usize) -> Option<Vec<u8>> {
        let h = self.hosted.get(&key)?;
        let p = h.lock_for_state()?;
        if p.desc().name != TESLA {
            return None;
        }
        tesla::edison_from_plugin_state(&p.save_state()?)
    }

    /// Love Philter's state (the project file's form) of the Pulp Filter hosted under `key`; `None` for any other
    /// plugin, and for one on its starting state that was never edited.
    pub fn love_philter_file_state(&self, key: usize) -> Option<Vec<u8>> {
        let h = self.hosted.get(&key)?;
        let p = h.lock_for_state()?;
        if p.desc().name != PULP_FILTER {
            return None;
        }
        #[cfg(not(feature = "stock-pulp-filter"))]
        return None;
        #[cfg(feature = "stock-pulp-filter")]
        pulp_filter::fl_from_plugin_state(&p.save_state()?).filter(|b| !b.is_empty())
    }

    /// Maximus's state (the project file's form) of the Press hosted under `key`; `None` for any other plugin, and
    /// for one on its starting state that was never edited.
    pub fn maximus_file_state(&self, key: usize) -> Option<Vec<u8>> {
        let h = self.hosted.get(&key)?;
        let p = h.lock_for_state()?;
        if p.desc().name != PRESS {
            return None;
        }
        #[cfg(not(feature = "stock-press"))]
        return None;
        #[cfg(feature = "stock-press")]
        press::fl_from_plugin_state(&p.save_state()?).filter(|b| !b.is_empty())
    }

    /// The FL state (the project file's form) of the Lemon effect hosted under `key`, when it is the one that plays
    /// slots stored as `file` (`FL_RECREATIONS`); `None` for any other plugin.
    pub fn recreation_file_state(&self, key: usize, file: &str) -> Option<Vec<u8>> {
        let lemon = recreation_of(file)?;
        let h = self.hosted.get(&key)?;
        let p = h.lock_for_state()?;
        if p.desc().name != lemon || !crate::project_plugins::is_builtin(p.desc()) {
            return None;
        }
        fl_from_recreation_state(lemon, &p.save_state()?)
    }

    /// Row `row`'s Lemon Sampler takes the stretch settings `sound` holds (§9b), keeping its other settings.
    pub fn set_sampler_stretch(&mut self, row: usize, sound: &os_model::ChannelSound) -> bool {
        self.set_sampler(row, None, &stretch_params(sound))
    }

    /// Loads sample `path` into row `row`'s Lemon Sampler: its current state with a new `sample-path`, so every other
    /// setting stays. False when the row hosts nothing or the plugin refused the state.
    pub fn load_sample(&mut self, row: usize, path: &str) -> bool {
        self.set_sampler(row, Some(path), &[])
    }

    /// Row `row`'s Lemon Sampler takes parameters `set` and, with `path`, that sample, in one change of its state:
    /// every other setting stays. False when the row hosts nothing or the plugin refused the state.
    pub fn set_sampler(&mut self, row: usize, path: Option<&str>, set: &[(&str, serde_json::Value)]) -> bool {
        let Some(h) = self.hosted.get(&row) else { return false };
        let Ok(mut p) = h.plugin.lock() else { return false };
        let Some(mut new) = p.save_state() else { return false };
        if let Some(path) = path {
            let Some(b) = with_sample_path(&new, path) else { return false };
            new = b;
        }
        if !set.is_empty() {
            let Some(b) = with_params(&new, set) else { return false };
            new = b;
        }
        p.load_state(&new)
    }

    /// Closes every window and drops every instance (a new or opened project).
    pub fn clear(&mut self) {
        self.editor_events.push(EditorEvent::Clear(self.hosted.keys().copied().collect()));
        self.hosted.clear();
        self.containers.clear();
        self.presets.clear();
        self.stash.clear();
        self.preset_waits.clear();
        // Scaling ▸ belongs to the project's windows: an opened project brings its own (`lemon_state`).
        self.editor_scales.clear();
        self.midi_outs.clear();
    }

    /// The sample file a Lemon Sampler instance holds (its persisted `sample-path`), `Some("")` when it has none.
    pub fn sampler_path(&self, row: usize) -> Option<String> {
        let h = self.hosted.get(&row)?;
        let blob = h.lock_for_state()?.save_state()?;
        sample_path_of(&blob)
    }

    /// The instances for the transport.
    /// Switches every hosted instance to offline rendering (an export) or back to live playback: plugins that know
    /// the difference use their best quality offline (Lemon Sampler: the resampler's exact weights).
    pub fn set_offline(&mut self, offline: bool) {
        // Lemon MIDI Out sends nothing while a render or export runs.
        crate::lemon_midi_out::set_offline(offline);
        for h in self.hosted.values() {
            if let Ok(mut p) = h.plugin.lock() {
                p.set_offline(offline);
            }
        }
    }

    /// Moves every hosted instance to sample rate `rate` (the audio settings changed); new ones start at it.
    pub fn set_rate(&mut self, rate: u32) {
        if rate == self.rate {
            return;
        }
        self.rate = rate;
        for h in self.hosted.values() {
            if let Ok(mut p) = h.plugin.lock() {
                if !p.activate(rate) {
                    eprintln!("[plugins] an instance did not restart at {rate} Hz");
                }
            }
        }
    }

    /// The voices of every hosted instrument, as their last processed blocks left them (spec: audio/voices.md §5).
    pub fn voices(&self) -> usize {
        self.hosted.range(..FX_KEY).filter_map(|(_, h)| h.meter.as_ref()).map(|m| m.voices()).sum()
    }

    pub fn instruments(&self) -> Vec<(usize, SharedInstrument)> {
        let hosted = self.hosted.range(..FX_KEY).map(|(&r, h)| (r, h.plugin.clone() as SharedInstrument));
        // A channel's container plays its channel (`crate::container`).
        let chains = self.containers.range(..FX_KEY).map(|(&r, c)| (r, c.fx.clone() as SharedInstrument));
        hosted.chain(self.midi_outs.iter().map(|(&r, m)| (r, m.clone() as SharedInstrument))).chain(chains).collect()
    }

    /// Tells instance `key` that its menu `id` closed, with the entry picked: a pick runs now, on this (the UI) thread
    /// (spec: plugins/vst3-hosting.md §9).
    pub fn menu_done(&self, key: usize, id: u64, pick: Option<usize>) {
        if let Some(Ok(mut p)) = self.hosted.get(&key).map(|h| h.plugin.lock()) {
            p.menu_done(id, pick);
        }
    }

    /// The scale row `row`'s window is drawn at where the program would use `scale`: its Scaling ▸ choice, if any.
    pub fn scale_for(&self, row: usize, scale: f32) -> f32 {
        self.editor_scales.get(&row).copied().unwrap_or(scale)
    }

    /// Rack rows were renumbered (`m(old) = new`; `None` drops the instance and its window). The presets the instances
    /// hold go with them.
    pub fn remap(&mut self, m: &dyn Fn(usize) -> Option<usize>) {
        self.midi_outs = std::mem::take(&mut self.midi_outs).into_iter().filter_map(|(r, i)| Some((m(r)?, i))).collect();
        let rows: Vec<usize> = self.presets.range(..FX_KEY).map(|(&r, _)| r).collect();
        let presets: Vec<(usize, PathBuf)> = rows.into_iter().filter_map(|r| Some((r, self.presets.remove(&r)?))).filter_map(|(r, p)| Some((m(r)?, p))).collect();
        self.presets.extend(presets);
        // The editors go with their instances (`EditorEvent::Remap`); one whose instance goes closes first.
        let mut map = Vec::new();
        for (row, h) in std::mem::take(&mut self.hosted) {
            if row >= FX_KEY {
                self.hosted.insert(row, h);
            } else if let Some(n) = m(row) {
                map.push((row, Some(n)));
                // Two instances mapped onto one row: the one there goes, kept.
                if let Some(old) = self.hosted.insert(n, h) {
                    self.keep_hosted(&old, false);
                }
            } else {
                map.push((row, None));
                self.keep_hosted(&h, false);
            }
        }
        self.editor_events.push(EditorEvent::Remap(map));
    }

}

/// What the user is told when instance `name` stopped after exception `code`.
pub fn stopped_message(name: &str, code: u32, effect: bool) -> String {
    let now = if effect { "its slot passes the sound through" } else { "its channel is silent" };
    format!("{}: {now}. Save your project and restart Lemon Studio to use it again", os_host::guard::stopped_text(name, code))
}

impl Drop for Hosted {
    /// The instance is retired, not destroyed here: see [`sweep_retired`]. Its editor was closed by whatever let it
    /// go (`Editor::close`).
    fn drop(&mut self) {
        let plugin = self.plugin.clone();
        let _ = RETIRED.try_with(|r| r.borrow_mut().push(plugin));
    }
}

thread_local! {
    /// Instances the app has let go of while the engine may still hold them (it lets go on its own thread when it
    /// swaps in a new engine). A plugin is destroyed on the UI thread, never on the engine's: plugins free their UI-side
    /// objects in their destructors, and doing it on another thread while the UI thread delivers the plugin's
    /// messages corrupted memory (spec: plugins/plugin-formats.md, "Destroying instances").
    static RETIRED: std::cell::RefCell<Vec<SharedPlugin>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Destroys the retired instances nothing else holds any more (UI thread: every tick, and at exit).
pub fn sweep_retired() {
    let gone: Vec<SharedPlugin> = RETIRED
        .try_with(|r| {
            let mut r = r.borrow_mut();
            let (gone, kept) = std::mem::take(&mut *r).into_iter().partition(|p| Arc::strong_count(p) == 1);
            *r = kept;
            gone
        })
        .unwrap_or_default();
    // Outside the borrow: a plugin's destructor may run a message loop that retires another instance.
    drop(gone);
}

/// Retired instances still held elsewhere (tests).
#[cfg(any(test, feature = "test-util"))]
pub fn retired_count() -> usize {
    RETIRED.with(|r| r.borrow().len())
}

/// Name of Lemon Studio's sampler plugin: it plays and edits every sampler channel (spec: plugins/lemon-sampler.md §7).
pub const LEMON_SAMPLER: &str = "Lemon Sampler";
/// Name of Lemon Studio's parametric EQ plugin.
pub const LEMON_EQ: &str = "Lemon Parametric EQ";
/// Lemon Studio's effects compiled in besides the EQ (`builtin_effects`, in this order). They have no file plugin of
/// their own to be stored as, so a project stores them wrapped, as it stored their plugin files
/// (`project_plugins::is_wrapped`).
pub const LINKED_EFFECTS: [&str; 18] = [
    "Lemon Limiter",
    "Lemon Reverb",
    "Lemon Delay",
    "Lemon Soft Clipper",
    "Lemon Fast Distortion",
    "Lemon Balance",
    "Lemon Stereo Enhancer",
    "Lemon Transient Processor",
    "Lemon Chorus",
    "Lemon Stereo Shaper",
    "Lemon Filter",
    TESLA,
    "Lemon Flanger",
    "Lemon Phaser",
    LEMON_PEAK_CONTROLLER,
    LEMON_FORMULA_CONTROLLER,
    LEMON_VOCODER,
    LEMON_DROP,
];
/// Lemon Studio's effects compiled in that a project stores under the FL plugin they recreate, with that plugin's
/// state (`new_slot`, `store_builtin_states`), so they are never saved wrapped.
pub const FILE_MAPPED: [&str; 3] = [LEMON_EQ, PULP_FILTER, PRESS];
/// Name of Lemon Peak Controller, Lemon Studio's peak follower and LFO controller: its outputs drive the project's links
/// (spec: plugins/lemon-peak-controller.md).
pub const LEMON_PEAK_CONTROLLER: &str = "Lemon Peak Controller";
/// Name of Lemon Formula Controller, Lemon Studio's formula controller: its output drives the project's links (spec:
/// plugins/lemon-formula-controller.md).
pub const LEMON_FORMULA_CONTROLLER: &str = "Lemon Formula Controller";
/// Name of Lemon Vocoder, Lemon Studio's vocoder (spec: plugins/lemon-vocoder.md).
pub const LEMON_VOCODER: &str = "Lemon Vocoder";
/// The plugin name a project file stores for Vocodex (file-format identifier, kept for interop): slots with it are
/// played and edited by Lemon Vocoder (spec: plugins/lemon-vocoder.md §6).
pub use crate::effect_ports::VOCODEX_FILE_NAME;
/// Name of Lemon Drop, Lemon Studio's audio visualizer (spec: plugins/lemon-drop.md).
pub const LEMON_DROP: &str = "Lemon Drop";
/// The plugin name a project file stores for Wave Candy (file-format identifier, kept for interop): slots with it are
/// shown by Lemon Drop (spec: plugins/lemon-drop.md "Projects").
pub const WAVE_CANDY_FILE_NAME: &str = "Wave Candy";
/// Name of Tesla, Lemon Studio's audio editor and recorder (spec: plugins/tesla/README.md).
pub const TESLA: &str = "Tesla";
/// Name of Pulp Filter, Lemon Studio's pattern-controlled multi-filter (spec: plugins/lemon-pulp-filter.md).
pub const PULP_FILTER: &str = "Pulp Filter";
/// The plugin name a project file stores for Love Philter (file-format identifier, kept for interop): slots with it
/// are played and edited by Pulp Filter (spec: plugins/lemon-pulp-filter.md §6).
pub const LOVE_PHILTER_FILE_NAME: &str = "Fruity Love Philter";
/// Name of Press, Lemon Studio's multiband maximiser (spec: plugins/lemon-press.md).
pub const PRESS: &str = "Press";
/// The plugin name a project file stores for Maximus (file-format identifier, kept for interop): slots with it are
/// played and edited by Press (spec: plugins/lemon-press.md §6).
pub const MAXIMUS_FILE_NAME: &str = "Maximus";
/// The plugin name a project file stores for Edison (file-format identifier, kept for interop): slots with it are
/// played by Tesla (spec: plugins/tesla/README.md "Opening Edison slots").
pub const EDISON_FILE_NAME: &str = "Edison";
/// The plugin name a project file stores for the parametric EQ (file-format identifier, kept for interop): slots
/// with it are played by Lemon Parametric EQ (spec: plugins/lemon-parametric-eq-2.md §8).
pub const EQ_FILE_NAME: &str = "Fruity Parametric EQ 2";
/// The built-in plugins' earlier names, newest first, with their names now: "Lemon Parametric EQ 2" and "Lemon 3x Osc"
/// (2026-10-13 to 2026-10-14), and the "Open" names before the line was renamed "Lemon". Plugin files and preset
/// folders made earlier still carry them (spec: plugins/plugin-formats.md, identity).
pub const OLD_NAMES: [(&str, &str); 5] = [
    ("Lemon Parametric EQ 2", LEMON_EQ),
    ("Lemon 3x Osc", crate::osc3::LEMON_TRIO),
    ("Open Sampler", LEMON_SAMPLER),
    ("Open Parametric EQ 2", LEMON_EQ),
    ("Open 3x Osc", crate::osc3::LEMON_TRIO),
];

/// The Lemon effects compiled in (`LINKED_EFFECTS`) that recreate an FL mixer effect, by the plugin name a project file
/// stores for it (file-format identifiers, kept for interop): an opened project's slot holding one is played and edited
/// by the Lemon effect, started from the slot's saved state, and saved back as the FL effect with that effect's state
/// (`App::sync_effects`, `App::store_builtin_states`; spec: plugins/lemon-small-effects.md "Opened projects' slots").
/// Not Lemon Stereo Shaper: the difference output is the mixer slot's alone (spec: plugins/lemon-stereo-shaper.md §5),
/// so the engine's own Stereo Shaper keeps playing those slots.
pub const FL_RECREATIONS: [(&str, &str); 16] = [
    ("Fruity Limiter", "Lemon Limiter"),
    ("Fruity Reeverb 2", "Lemon Reverb"),
    ("Fruity Delay 3", "Lemon Delay"),
    ("Fruity Soft Clipper", "Lemon Soft Clipper"),
    ("Fruity Fast Dist", "Lemon Fast Distortion"),
    ("Fruity Balance", "Lemon Balance"),
    ("Fruity Stereo Enhancer", "Lemon Stereo Enhancer"),
    ("Transient Processor", "Lemon Transient Processor"),
    ("Fruity Chorus", "Lemon Chorus"),
    ("Fruity Filter", "Lemon Filter"),
    ("Fruity Flanger", "Lemon Flanger"),
    ("Fruity Phaser", "Lemon Phaser"),
    ("Fruity Peak Controller", LEMON_PEAK_CONTROLLER),
    ("Fruity Formula Controller", LEMON_FORMULA_CONTROLLER),
    (VOCODEX_FILE_NAME, LEMON_VOCODER),
    (WAVE_CANDY_FILE_NAME, LEMON_DROP),
];

/// Lemon Parametric EQ's plugin state for an EQ slot state; `None` when the EQ is not in this build.
pub fn eq_state_from_fl(fl: &[u8]) -> Option<Vec<u8>> {
    #[cfg(feature = "stock-parametric-eq-2")]
    return Some(lemon_parametric_eq_2::params::plugin_state_from_fl(fl));
    #[cfg(not(feature = "stock-parametric-eq-2"))]
    {
        let _ = fl;
        None
    }
}

/// Pulp Filter's plugin state for a Love Philter slot state (bytes that are not one: its starting state); `None` when
/// Pulp Filter is not in this build.
pub fn pulp_filter_state_from_fl(fl: &[u8]) -> Option<Vec<u8>> {
    #[cfg(feature = "stock-pulp-filter")]
    return Some(pulp_filter::plugin_state_from_fl(fl));
    #[cfg(not(feature = "stock-pulp-filter"))]
    {
        let _ = fl;
        None
    }
}

/// Press's plugin state for a Maximus slot state (bytes that are not one: its starting state); `None` when Press is
/// not in this build.
pub fn press_state_from_fl(fl: &[u8]) -> Option<Vec<u8>> {
    #[cfg(feature = "stock-press")]
    return Some(press::plugin_state_from_fl(fl));
    #[cfg(not(feature = "stock-press"))]
    {
        let _ = fl;
        None
    }
}

/// The Lemon effect of `FL_RECREATIONS` that plays slots stored as `file` (any case: files may spell the id
/// differently).
pub fn recreation_of(file: &str) -> Option<&'static str> {
    FL_RECREATIONS.iter().find(|(f, _)| f.eq_ignore_ascii_case(file)).map(|&(_, lemon)| lemon)
}

/// Lemon effect `lemon`'s plugin state for an FL slot state of the effect it recreates (`FL_RECREATIONS`).
pub fn recreation_state_from_fl(lemon: &str, fl: &[u8]) -> Option<Vec<u8>> {
    use lemon_small_fx::params::plugin_state_from_fl as small;
    Some(match lemon {
        #[cfg(feature = "stock-limiter")]
        "Lemon Limiter" => lemon_limiter::params::plugin_state_from_fl(fl),
        #[cfg(feature = "stock-reverb")]
        "Lemon Reverb" => lemon_reverb::params::plugin_state_from_fl(fl),
        #[cfg(feature = "stock-delay")]
        "Lemon Delay" => lemon_delay::params::plugin_state_from_fl(fl),
        "Lemon Soft Clipper" => small(lemon_small_fx::soft_clipper::SPEC, fl),
        "Lemon Fast Distortion" => small(lemon_small_fx::fast_distortion::SPEC, fl),
        "Lemon Balance" => small(lemon_small_fx::balance::SPEC, fl),
        "Lemon Stereo Enhancer" => small(lemon_small_fx::stereo_enhancer::SPEC, fl),
        "Lemon Transient Processor" => small(lemon_small_fx::transient_processor::SPEC, fl),
        "Lemon Chorus" => small(lemon_small_fx::chorus::SPEC, fl),
        "Lemon Filter" => small(lemon_small_fx::filter::SPEC, fl),
        "Lemon Flanger" => small(lemon_small_fx::flanger::SPEC, fl),
        "Lemon Phaser" => small(lemon_small_fx::phaser::SPEC, fl),
        LEMON_PEAK_CONTROLLER => small(lemon_small_fx::peak_controller::SPEC, fl),
        LEMON_FORMULA_CONTROLLER => small(lemon_small_fx::formula_controller::SPEC, fl),
        LEMON_VOCODER => small(lemon_small_fx::vocoder::SPEC, fl),
        #[cfg(feature = "stock-drop")]
        LEMON_DROP => lemon_drop::plugin_state_from_fl(fl),
        _ => return None,
    })
}

/// The FL slot state (the project file's form) of Lemon effect `lemon`'s plugin state `blob`; `None` for another
/// plugin or a blob that is not its state.
pub fn fl_from_recreation_state(lemon: &str, blob: &[u8]) -> Option<Vec<u8>> {
    use lemon_small_fx::params::fl_from_plugin_state as small;
    match lemon {
        #[cfg(feature = "stock-limiter")]
        "Lemon Limiter" => lemon_limiter::params::fl_from_plugin_state(blob),
        #[cfg(feature = "stock-reverb")]
        "Lemon Reverb" => lemon_reverb::params::fl_from_plugin_state(blob),
        #[cfg(feature = "stock-delay")]
        "Lemon Delay" => lemon_delay::params::fl_from_plugin_state(blob),
        "Lemon Soft Clipper" => small(lemon_small_fx::soft_clipper::SPEC, blob),
        "Lemon Fast Distortion" => small(lemon_small_fx::fast_distortion::SPEC, blob),
        "Lemon Balance" => small(lemon_small_fx::balance::SPEC, blob),
        "Lemon Stereo Enhancer" => small(lemon_small_fx::stereo_enhancer::SPEC, blob),
        "Lemon Transient Processor" => small(lemon_small_fx::transient_processor::SPEC, blob),
        "Lemon Chorus" => small(lemon_small_fx::chorus::SPEC, blob),
        "Lemon Filter" => small(lemon_small_fx::filter::SPEC, blob),
        "Lemon Flanger" => small(lemon_small_fx::flanger::SPEC, blob),
        "Lemon Phaser" => small(lemon_small_fx::phaser::SPEC, blob),
        LEMON_PEAK_CONTROLLER => small(lemon_small_fx::peak_controller::SPEC, blob),
        LEMON_FORMULA_CONTROLLER => small(lemon_small_fx::formula_controller::SPEC, blob),
        LEMON_VOCODER => small(lemon_small_fx::vocoder::SPEC, blob),
        #[cfg(feature = "stock-drop")]
        LEMON_DROP => lemon_drop::fl_from_plugin_state(blob),
        _ => None,
    }
}

/// What the mixer shows for a slot's stored plugin name.
pub fn slot_label(name: &str) -> &str {
    match name {
        EQ_FILE_NAME => LEMON_EQ,
        EDISON_FILE_NAME => TESLA,
        LOVE_PHILTER_FILE_NAME => PULP_FILTER,
        MAXIMUS_FILE_NAME => PRESS,
        _ => recreation_of(name).unwrap_or(name),
    }
}

/// A new slot for effect `name`: what the project stores (Lemon Parametric EQ as the file's EQ with its default
/// state, Pulp Filter as the file's Love Philter and Press as the file's Maximus with their starting states, so the file
/// opens elsewhere; other effects by name).
pub fn new_slot(name: &str) -> os_model::SlotPlugin {
    #[cfg(feature = "stock-parametric-eq-2")]
    if name == LEMON_EQ {
        return os_model::SlotPlugin { id: os_model::Id::NONE, name: EQ_FILE_NAME.into(), state: lemon_parametric_eq_2::params::default_state() };
    }
    if name == PULP_FILTER {
        os_model::SlotPlugin { id: os_model::Id::NONE, name: LOVE_PHILTER_FILE_NAME.into(), state: os_engine::effects::love_philter::state::State::new_default().to_bytes() }
    } else if name == PRESS {
        os_model::SlotPlugin { id: os_model::Id::NONE, name: MAXIMUS_FILE_NAME.into(), state: os_engine::effects::maximus::state::State::new_default().to_bytes() }
    } else {
        os_model::SlotPlugin { id: os_model::Id::NONE, name: name.into(), state: Vec::new() }
    }
}
/// File (in the user folder) remembering which native plugins passed the child-process check.
#[cfg(feature = "fl-native")]
const NATIVE_CHECKS: &str = "native-checks.txt";
/// The sampler parameter a note's sample start modulates per note (Lemon Sampler's START OFFSET).
const SAMPLE_START: &str = "Sample start offset";
/// Lemon Sampler's hidden parameter an audio clip voice's end modulates per note.
const CLIP_END: &str = "Clip end";

/// Lemon Sampler's starting state for a sampler channel of the project: its sample, its five envelope records, its
/// stretch settings, the sample's start, length, loop points, loop options and precomputed effects, the channel
/// filter, declick mode and sampler flags, and the root note page (nih-plug JSON state with the u64 length prefix of
/// its CLAP state; spec: plugins/lemon-sampler.md §6, §7).
pub fn sampler_state(st: &crate::settings_file::SettingsStore, ch: &os_model::Channel) -> Vec<u8> {
    let mut params = Vec::new();
    // Model records: panning, volume, cutoff (Mod X), resonance (Mod Y), pitch.
    for (k, (e, pre)) in ch.envelopes.iter().zip(ENV_PREFIXES).enumerate() {
        let f = |v: f32| format!("{{\"f32\":{v}}}");
        let time = |field: i32| f(field.max(0) as f32 / 65536.0);
        let scale = amount_scale(k);
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
            ("amt", f(e.amount.clamp(-scale, scale) as f32 / scale as f32)),
            ("lfo_delay", time(e.lfo_delay)),
            ("lfo_att", time(e.lfo_attack)),
            ("lfo_speed", time(e.lfo_speed)),
            ("lfo_amt", f(e.lfo_amount.clamp(-scale, scale) as f32 / scale as f32)),
        ] {
            params.push(format!("\"{pre}_{id}\":{v}"));
        }
        params.push(format!("\"{pre}_lfo_shape\":{{\"i32\":{}}}", e.lfo_shape.clamp(0, 2)));
        for (bit, id) in ENV_FLAGS {
            params.push(format!("\"{pre}_{id}\":{{\"bool\":{}}}", e.flags & bit != 0));
        }
    }
    // Stretch settings (spec: audio/time-stretch.md "Fields"; Lemon Sampler stretches for the channel).
    if ch.sound.stretch != [0; 4] {
        for (id, v) in stretch_params(&ch.sound) {
            params.push(format!("\"{id}\":{v}"));
        }
    }
    for (id, v) in sample_params(&ch.sound, ch.kind == os_model::ChannelKind::AudioClip) {
        params.push(format!("\"{id}\":{v}"));
    }
    // Factory placeholders (`%FLStudioFactoryData%`...) become real paths: the plugin opens files directly.
    let path = resolve_sample(st, ch.sample_path.as_deref().unwrap_or(""));
    let path = path.as_str();
    for (id, v) in channel_params(ch, || sample_frames(st, path)) {
        params.push(format!("\"{id}\":{v}"));
    }
    // The persisted fields are JSON strings holding JSON. The file's own precomputed values travel whole: Lemon Sampler
    // plays them while its knobs stand where they put them (values beyond a knob's range, start and length doubles).
    let inner = json_string(path);
    let settings = json_string(&lemon_sampler::params::fx_text(&ch.sound.fx));
    let state = format!(
        "{{\"version\":\"0.0.1\",\"params\":{{{}}},\"fields\":{{\"sample-path\":{},\"file-settings\":{}}}}}",
        params.join(","),
        json_string(&inner),
        json_string(&settings)
    );
    let mut blob = (state.len() as u64).to_le_bytes().to_vec();
    blob.extend_from_slice(state.as_bytes());
    blob
}

/// Writes Lemon Sampler's settings from its saved `state` into its channel, the inverse of [`sampler_state`]: the five
/// envelope records with their LFOs, the channel filter, the declick mode, the sampler flags, the sample's start,
/// length, loop points, start offset, loop options and precomputed effects, and the root note page (spec:
/// plugins/lemon-sampler.md §7). A value whose knob still stands where the channel's value puts it keeps the channel's
/// value, so an untouched setting is saved as the file had it. The stretch settings follow the editor's edits as they
/// happen (`params_tick`).
pub fn store_sampler(st: &crate::settings_file::SettingsStore, ch: &mut os_model::Channel, state: &[u8]) {
    use lemon_sampler::params as sp;
    let (_, json) = state_json(state);
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(json) else { return };
    let Some(params) = v.get("params").and_then(|p| p.as_object()) else { return };
    let f = |id: &str| params.get(id)?.get("f32")?.as_f64().map(|x| x as f32);
    let i = |id: &str| params.get(id)?.get("i32")?.as_i64().map(|x| x as i32);
    let b = |id: &str| params.get(id)?.get("bool")?.as_bool();
    // The channel's value while the knob stands where it puts it, else the knob's.
    let keep = |cur: i32, knob: Option<f32>, to_knob: &dyn Fn(i32) -> f32, from_knob: &dyn Fn(f32) -> i32| match knob {
        Some(k) if to_knob(cur) != k => from_knob(k),
        _ => cur,
    };
    let time_knob = |c: i32| c.max(0) as f32 / 65536.0;
    let field = |k: f32| sp::time_field(k);
    let unit_knob = |c: i32| c.clamp(-128, 128) as f32 / 128.0;
    let units = |k: f32| (k * 128.0).round() as i32;
    let sus_knob = |c: i32| c.clamp(0, 128) as f32 / 128.0;
    for (k, (e, pre)) in ch.envelopes.iter_mut().zip(ENV_PREFIXES).enumerate() {
        let id = |s: &str| format!("{pre}_{s}");
        // Amounts: -128..128, the pitch record's in cents.
        let scale = amount_scale(k);
        let amt_knob = |c: i32| c.clamp(-scale, scale) as f32 / scale as f32;
        let amt_units = |k: f32| (k * scale as f32).round() as i32;
        e.enabled = b(&id("on")).unwrap_or(e.enabled);
        for (val, s) in [(&mut e.delay, "delay"), (&mut e.attack, "att"), (&mut e.hold, "hold"), (&mut e.decay, "dec"), (&mut e.release, "rel")] {
            *val = keep(*val, f(&id(s)), &time_knob, &field);
        }
        for (val, s) in [(&mut e.lfo_delay, "lfo_delay"), (&mut e.lfo_attack, "lfo_att"), (&mut e.lfo_speed, "lfo_speed")] {
            *val = keep(*val, f(&id(s)), &time_knob, &field);
        }
        e.sustain = keep(e.sustain, f(&id("sus")), &sus_knob, &units);
        for (val, s) in [(&mut e.attack_tension, "att_t"), (&mut e.decay_tension, "dec_t"), (&mut e.release_tension, "rel_t")] {
            *val = keep(*val, f(&id(s)), &unit_knob, &units);
        }
        for (val, s) in [(&mut e.amount, "amt"), (&mut e.lfo_amount, "lfo_amt")] {
            *val = keep(*val, f(&id(s)), &amt_knob, &amt_units);
        }
        if let Some(shape) = i(&id("lfo_shape")).filter(|&s| s != e.lfo_shape.clamp(0, 2)) {
            e.lfo_shape = shape;
        }
        for (bit, s) in ENV_FLAGS {
            if let Some(on) = b(&id(s)) {
                e.flags = if on { e.flags | bit } else { e.flags & !bit };
            }
        }
    }
    let s = &mut ch.sound;
    let filter_knob = |c: i32| c.clamp(0, 256) as f32 / 256.0;
    let filter_units = |k: f32| (k * 256.0).round() as i32;
    s.cutoff = keep(s.cutoff, f("f_cut"), &filter_knob, &filter_units);
    s.resonance = keep(s.resonance, f("f_res"), &filter_knob, &filter_units);
    if let Some(t) = i("f_type").filter(|&t| t != s.filter_type.clamp(0, 7)) {
        s.filter_type = t;
    }
    // Mode 6 plays (and is shown) as mode 0: it stays 6 until another mode is picked.
    if let Some(d) = i("declick").filter(|&d| d != if s.declick <= 5 { i32::from(s.declick) } else { 0 }) {
        s.declick = d.clamp(0, 5) as u8;
    }
    // A channel without sampler flags has those of a new sampler channel (Load regions, Use loop points; spec:
    // flp/events/channels.md "Sampler flags (event 143)"); it gets the event when one of them changes.
    let flags = s.sampler_flags.unwrap_or(os_model::ChannelSound::DEFAULT_FLAGS);
    let mut new = flags;
    for (bit, id) in SAMPLER_FLAGS {
        if let Some(on) = b(id) {
            new = if on { new | bit } else { new & !bit };
        }
    }
    if new != flags || s.sampler_flags.is_some() {
        s.sampler_flags = Some(new);
    }
    if let Some(k) = f("start_offset").filter(|&k| k != s.start_offset().clamp(0.0, 1.0) as f32) {
        s.start_offset_bits = f64::from(k).to_bits();
    }
    // Loop points: frames of the file, the knobs fractions of it (the file is read only when they matter).
    let (ls, le) = (f("loop_start").unwrap_or(0.0), f("loop_end").unwrap_or(1.0));
    if s.loop_points.is_some() || ls > 0.0 || le < 1.0 {
        let path = v.get("fields").and_then(|x| x.get("sample-path")).and_then(|x| x.as_str());
        let path = path.and_then(|p| serde_json::from_str::<String>(p).ok()).unwrap_or_default();
        if let Some(n) = sample_frames(st, &path).filter(|&n| (ls, le) != loop_knobs(s.loop_points, n)) {
            s.loop_points = Some([loop_frame(ls, n), loop_frame(le, n)]);
        }
    }
    let mut k = sp::SamplerParams::default().fx_knobs();
    let knob = |id: &str, v: &mut f32| *v = f(id).unwrap_or(*v);
    let switch = |id: &str, v: &mut bool| *v = b(id).unwrap_or(*v);
    knob("fx_start", &mut k.start);
    knob("fx_length", &mut k.length);
    knob("fx_in", &mut k.fade_in);
    knob("fx_out", &mut k.fade_out);
    knob("fx_crossfade", &mut k.crossfade);
    knob("fx_trim", &mut k.trim);
    knob("fx_boost", &mut k.boost);
    knob("fx_eq", &mut k.eq);
    knob("fx_rm_mix", &mut k.ring_mix);
    knob("fx_rm_freq", &mut k.ring_freq);
    knob("fx_cut", &mut k.cutoff);
    knob("fx_res", &mut k.resonance);
    knob("fx_reverb", &mut k.reverb);
    knob("fx_sdelay", &mut k.stereo_delay);
    knob("fx_pogo", &mut k.pogo);
    switch("fx_dc", &mut k.remove_dc);
    switch("fx_normalize", &mut k.normalize);
    switch("fx_polarity", &mut k.polarity);
    switch("fx_reverse", &mut k.reverse);
    switch("fx_fade_stereo", &mut k.fade_stereo);
    switch("fx_swap", &mut k.swap);
    switch("fx_clip", &mut k.clip);
    switch("loop_pingpong", &mut k.ping_pong);
    switch("fx_loop_follows", &mut k.loop_follows);
    k.reverb_type = i("fx_reverb_type").unwrap_or(k.reverb_type);
    s.fx = sp::fx_from_knobs(&k, Some(&s.fx));
    // The root note page.
    let m = &mut ch.misc;
    if let Some(r) = i("root").filter(|&r| r != i32::from(m.root).clamp(0, sp::KEY_MAX)) {
        m.root = r.clamp(0, 127) as u8;
    }
    if let Some(x) = i("fine").filter(|&x| x != m.fine.clamp(-100, 100)) {
        m.fine = x;
    }
    m.main_pitch = b("main_pitch").unwrap_or(m.main_pitch);
    m.add_key = b("add_key").unwrap_or(m.add_key);
    for (k, id) in ["zone_lo", "zone_hi"].into_iter().enumerate() {
        if let Some(z) = i(id).filter(|&z| z != m.key_range[k].clamp(0, sp::KEY_MAX)) {
            m.key_range[k] = zone_units(z);
        }
    }
}

/// The five envelope records' parameter prefixes in Lemon Sampler, in the model's order.
const ENV_PREFIXES: [&str; 5] = ["envpan", "envvol", "envmodx", "envmody", "envpitch"];
/// Envelope record flags and their switches: 1 = envelope times follow the tempo, 2 = LFO times follow the tempo,
/// 0x20 = global LFO (spec: audio/envelopes-and-lfos.md).
const ENV_FLAGS: [(i32, &str); 3] = [(1, "tempo"), (2, "lfo_tempo"), (0x20, "lfo_global")];
/// Sampler flags (event 143) and their switches (spec: flp/events/channels.md "Sampler flags").
const SAMPLER_FLAGS: [(u32, &str); 5] = [
    (os_model::ChannelSound::RESAMPLE, "resample"),
    (os_model::ChannelSound::LOAD_REGIONS, "load_regions"),
    (os_model::ChannelSound::LOAD_SLICES, "load_slices"),
    (os_model::ChannelSound::USE_LOOP, "loop_use"),
    (os_model::ChannelSound::KEEP_ON_DISK, "keep_disk"),
];

/// Units of envelope record `k`'s amounts at full scale: 128, the pitch record's 2400 cents (spec:
/// audio/envelopes-and-lfos.md).
fn amount_scale(k: usize) -> i32 {
    if k == os_model::Envelope::PITCH { 2400 } else { 128 }
}

/// The stored key zone end for Lemon Sampler's key: its highest key is the "no zone" value 256.
fn zone_units(key: i32) -> i32 {
    if key >= lemon_sampler::params::KEY_MAX { 256 } else { key.max(0) }
}

/// Frames of the sample file at `path` (placeholders resolved), when it can be read.
fn sample_frames(st: &crate::settings_file::SettingsStore, path: &str) -> Option<u64> {
    let path = resolve_sample(st, path);
    if path.is_empty() {
        return None;
    }
    let s = os_engine::sample::load_shared(std::path::Path::new(&path)).ok()?;
    Some(s.frames() as u64).filter(|&n| n > 0)
}

/// Lemon Sampler's loop start / end knobs for stored loop points (frames) of a sample of `n` frames: fractions of its
/// last frame's index, where the Sampler window's END knob stops; -1, `i64::MAX` and points past it are the sample's
/// ends (spec: flp/events/channels.md "Loop points (event 251)").
fn loop_knobs(points: Option<[i64; 2]>, n: u64) -> (f32, f32) {
    let last = n.saturating_sub(1);
    let Some([s, e]) = points.filter(|_| last > 0) else { return (0.0, 1.0) };
    let share = |v: i64| (v as f64 / last as f64).clamp(0.0, 1.0) as f32;
    (if s < 0 { 0.0 } else { share(s) }, if e < 0 || e as u64 >= last { 1.0 } else { share(e) })
}

/// The stored frame of loop knob position `k` for a sample of `n` frames (the inverse of [`loop_knobs`]).
fn loop_frame(k: f32, n: u64) -> i64 {
    (f64::from(k.clamp(0.0, 1.0)) * n.saturating_sub(1) as f64).round() as i64
}

/// Lemon Sampler's parameters for what a channel stores outside its sound settings: the root note page (root note,
/// fine tune, main pitch, add to key, key zone), the start offset, the sampler flags' switches and the loop points
/// (`frames` gives the sample's length; it is read only for loop points).
fn channel_params(ch: &os_model::Channel, frames: impl FnOnce() -> Option<u64>) -> Vec<(&'static str, serde_json::Value)> {
    use lemon_sampler::params::KEY_MAX;
    use serde_json::json;
    let (m, s) = (&ch.misc, &ch.sound);
    let mut out = vec![
        ("root", json!({ "i32": i32::from(m.root).clamp(0, KEY_MAX) })),
        ("fine", json!({ "i32": m.fine.clamp(-100, 100) })),
        ("main_pitch", json!({ "bool": m.main_pitch })),
        ("add_key", json!({ "bool": m.add_key })),
        ("zone_lo", json!({ "i32": m.key_range[0].clamp(0, KEY_MAX) })),
        ("zone_hi", json!({ "i32": m.key_range[1].clamp(0, KEY_MAX) })),
        ("start_offset", json!({ "f32": s.start_offset().clamp(0.0, 1.0) as f32 })),
    ];
    let flags = s.sampler_flags.unwrap_or(os_model::ChannelSound::DEFAULT_FLAGS);
    // "Use loop points" only when the file has the flags (`sample_params`).
    out.extend(SAMPLER_FLAGS.iter().filter(|(_, id)| *id != "loop_use").map(|&(bit, id)| (id, json!({ "bool": flags & bit != 0 }))));
    if s.loop_points.is_some() {
        if let Some(n) = frames() {
            let (ls, le) = loop_knobs(s.loop_points, n);
            out.push(("loop_start", json!({ "f32": ls })));
            out.push(("loop_end", json!({ "f32": le })));
        }
    }
    out
}

/// Lemon Sampler's parameters for what loading a sample may read into its channel (`rack::read_tags`): the root note,
/// the stretch settings and Keep on disk (`keep_on_disk::keep_long`).
pub fn tag_params(ch: &os_model::Channel) -> Vec<(&'static str, serde_json::Value)> {
    let root = i32::from(ch.misc.root).clamp(0, lemon_sampler::params::KEY_MAX);
    let mut set = vec![("root", serde_json::json!({ "i32": root }))];
    set.extend(stretch_params(&ch.sound));
    let flags = ch.sound.sampler_flags.unwrap_or(os_model::ChannelSound::DEFAULT_FLAGS);
    set.push(("keep_disk", serde_json::json!({ "bool": flags & os_model::ChannelSound::KEEP_ON_DISK != 0 })));
    set
}

/// Writes an edit of one of Lemon Sampler's root note page parameters (`name`, at normalized value `norm`) into the
/// channel's settings, so playback follows it (the engine applies the main pitch). False for other parameters.
pub fn misc_param(name: &str, norm: f64, misc: &mut os_model::ChannelMisc) -> bool {
    use lemon_sampler::params::KEY_MAX;
    let key = || (norm.clamp(0.0, 1.0) * f64::from(KEY_MAX)).round() as i32;
    match name {
        "Root note" => misc.root = key().min(127) as u8,
        "Fine tuning" => misc.fine = (norm.clamp(0.0, 1.0) * 200.0 - 100.0).round() as i32,
        "Affected by the main pitch" => misc.main_pitch = norm >= 0.5,
        "Add root note to key" => misc.add_key = norm >= 0.5,
        "Zone low key" => misc.key_range[0] = zone_units(key()),
        "Zone high key" => misc.key_range[1] = zone_units(key()),
        _ => return false,
    }
    true
}

/// A project sample path as a file path (placeholders resolved against the factory content folder, `crate::factory_folder`;
/// parts matched ignoring case where the path as written is not found; else unchanged).
pub fn resolve_sample(st: &crate::settings_file::SettingsStore, path: &str) -> String {
    if path.is_empty() {
        return String::new();
    }
    os_engine::SampleLibrary::new(crate::factory_folder::dir(st))
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

/// `blob` (a Lemon Sampler state) with its `sample-path` field replaced by `path`.
/// Lemon Sampler's MUL knob position of a stretch multiplier (25 %..400 %, doubling every quarter).
fn stretch_mul_norm(m: f64) -> f32 {
    ((m.log2() + 2.0) / 4.0).clamp(0.0, 1.0) as f32
}

/// Lemon Sampler's time-stretch parameters for a channel's stretch settings: method, MUL, PITCH and TIME (spec:
/// audio/time-stretch.md "Fields", plugins/lemon-sampler.md §5.6).
fn stretch_params(sound: &os_model::ChannelSound) -> [(&'static str, serde_json::Value); 4] {
    use lemon_sampler::params as sp;
    let steps = sound.stretch_time().map_or(0.0, |beats| beats * 4.0);
    [
        ("ts_mode", serde_json::json!({ "i32": sp::stretch_index(sound.stretch_mode()) })),
        ("ts_mul", serde_json::json!({ "f32": stretch_mul_norm(sound.stretch_multiplier()) })),
        ("ts_pitch", serde_json::json!({ "f32": ((sound.stretch_cents() + 1200.0) / 2400.0).clamp(0.0, 1.0) as f32 })),
        ("ts_time", serde_json::json!({ "f32": sp::time_norm(steps) })),
    ]
}

/// Lemon Sampler's parameters for a channel's sample settings: start and length, the precomputed effects and the loop
/// options (spec: audio/precomputed-effects.md "Settings"), the channel filter (audio/channel-filter.md: knob
/// positions = stored / 256) and the declick mode (audio/declick.md). "Use loop points" only when the file has the
/// sampler flags (value 8), or off on an audio clip channel (`clip`) without them: audio clips never loop.
fn sample_params(sound: &os_model::ChannelSound, clip: bool) -> Vec<(&'static str, serde_json::Value)> {
    use lemon_sampler::params::{unit, Unit};
    use os_model::SampleFx;
    use serde_json::json;
    let fx = &sound.fx;
    let f = |v: f32| json!({ "f32": v });
    let b = |v: bool| json!({ "bool": v });
    let knob = |u: &Unit, v: u32| f(u.knob(v));
    let flag = |bit: u16| b(fx.flags & bit != 0);
    let mut out = vec![
        ("fx_start", f(fx.start().clamp(0.0, 1.0) as f32)),
        ("fx_length", f(fx.length().clamp(0.0, 1.0) as f32)),
        ("fx_in", knob(&unit::FADE, fx.fade_in.into())),
        ("fx_out", knob(&unit::FADE, fx.fade_out.into())),
        ("fx_crossfade", knob(&unit::CROSSFADE, fx.crossfade)),
        ("fx_trim", knob(&unit::TRIM, fx.trim)),
        ("fx_boost", knob(&unit::BOOST, fx.boost.into())),
        ("fx_eq", knob(&unit::FEEDBACK, fx.feedback.into())),
        ("fx_rm_mix", knob(&unit::RING_MIX, fx.ring_mix.into())),
        ("fx_rm_freq", knob(&unit::RING_FREQ, fx.ring_freq.into())),
        ("fx_cut", knob(&unit::CUTOFF, fx.cutoff.into())),
        ("fx_res", knob(&unit::RESONANCE, fx.resonance.into())),
        ("fx_reverb", knob(&unit::REVERB, fx.reverb.into())),
        ("fx_reverb_type", json!({ "i32": i32::from(fx.reverb_type.min(1)) })),
        ("fx_sdelay", knob(&unit::STEREO_DELAY, fx.stereo_delay.into())),
        ("fx_pogo", knob(&unit::POGO, fx.pogo.into())),
        ("fx_dc", b(fx.remove_dc)),
        ("fx_normalize", b(fx.normalize)),
        ("fx_polarity", b(fx.polarity)),
        ("fx_reverse", flag(SampleFx::REVERSE)),
        ("fx_fade_stereo", flag(SampleFx::FADE_STEREO)),
        ("fx_swap", flag(SampleFx::SWAP)),
        ("fx_clip", flag(SampleFx::CLIP)),
        ("loop_pingpong", b(fx.ping_pong)),
        ("fx_loop_follows", b(fx.loop_follows_reverse)),
        ("f_cut", f(sound.cutoff.clamp(0, 256) as f32 / 256.0)),
        ("f_res", f(sound.resonance.clamp(0, 256) as f32 / 256.0)),
        ("f_type", json!({ "i32": sound.filter_type.clamp(0, 7) })),
        // Lemon Sampler's list holds modes 0 to 5: mode 6 (valid only as stored data) plays as mode 0 (Lemon Studio).
        ("declick", json!({ "i32": if sound.declick <= 5 { i32::from(sound.declick) } else { 0 } })),
    ];
    match sound.sampler_flags {
        Some(flags) => out.push(("loop_use", b(flags & os_model::ChannelSound::USE_LOOP != 0))),
        None if clip => out.push(("loop_use", b(false))),
        None => {}
    }
    out
}

/// Writes an edit of one of Lemon Sampler's time-stretch parameters (`name`, at normalized value `norm`) into a
/// channel's stretch settings (event 215 ints 24..27, the inverse of `stretch_params`). False for other parameters.
pub fn stretch_param(name: &str, norm: f64, stretch: &mut [u32; 4]) -> bool {
    use lemon_sampler::params as sp;
    let v = norm.clamp(0.0, 1.0) as f32;
    match name {
        "Time stretch (time)" => {
            let units = sp::time_file(sp::time_steps(v));
            stretch[0] = if units >= 1.0 { units.to_bits() } else { 0 };
        }
        "Pitch shift" => stretch[1] = sp::ts_cents(v) as i32 as u32,
        "Time stretch (multiplicator)" => stretch[2] = (sp::mul_of(v).log2() * 10000.0).round() as i32 as u32,
        "Stretching method" => {
            let last = sp::STRETCH_FILE.len() - 1;
            let i = ((f64::from(v) * last as f64).round() as usize).min(last);
            stretch[3] = sp::STRETCH_FILE[i] as u32;
        }
        _ => return false,
    }
    true
}

/// `blob` (a Lemon Sampler state) with parameters set to the given JSON values.
pub fn with_params(blob: &[u8], set: &[(&str, serde_json::Value)]) -> Option<Vec<u8>> {
    let (prefixed, json) = state_json(blob);
    let mut v: serde_json::Value = serde_json::from_slice(json).ok()?;
    let params = v.get_mut("params")?.as_object_mut()?;
    for (k, val) in set {
        params.insert((*k).to_string(), val.clone());
    }
    let out = serde_json::to_vec(&v).ok()?;
    let mut b = if prefixed { (out.len() as u64).to_le_bytes().to_vec() } else { Vec::new() };
    b.extend_from_slice(&out);
    Some(b)
}

pub fn with_sample_path(blob: &[u8], path: &str) -> Option<Vec<u8>> {
    let (prefixed, json) = state_json(blob);
    let mut v: serde_json::Value = serde_json::from_slice(json).ok()?;
    let inner = serde_json::to_string(path).ok()?;
    v.get_mut("fields")?.as_object_mut()?.insert("sample-path".into(), serde_json::Value::String(inner));
    let out = serde_json::to_vec(&v).ok()?;
    let mut b = if prefixed { (out.len() as u64).to_le_bytes().to_vec() } else { Vec::new() };
    b.extend_from_slice(&out);
    Some(b)
}

/// The `sample-path` field of a Lemon Sampler (or Lemon Trio) state blob.
pub use os_hostlink::state::json as state_json;

pub fn sample_path_of(blob: &[u8]) -> Option<String> {
    let v: serde_json::Value = serde_json::from_slice(state_json(blob).1).ok()?;
    let field = v.get("fields")?.get("sample-path")?.as_str()?;
    serde_json::from_str::<String>(field).ok()
}

/// Opens the bundled Lemon Sampler in a real plugin window, clicks through the strip and captures the window
/// (`OS_SHOT_DIR=<dir> OS_CLAP=<Lemon Sampler.clap> cargo test -p os-session plugin_window_shots -- --ignored`).

/// Host-driven resizing of plugin editors on X11: a window whose editor can be resized follows a resize by the window
/// manager (simulated: a bare X server has none, so the test resizes the frame itself), and an editor larger than the
/// screen opens fitted to it. Opens windows: run it on a private X server only (`DISPLAY=:78 cargo test -p os-app
/// --bin lemonstudio editors_follow_window_resizes -- --ignored`).

#[cfg(test)]
mod slot_tests {
    use super::*;

    /// A state's length prefix is told by its value: one whose low byte is `{` (a 0x7B-byte JSON) still counts, and a
    /// bare JSON state is read whole.
    #[test]
    fn a_state_prefix_is_told_by_its_length() {
        let with = |path: &str| format!(r#"{{"fields":{{"sample-path":"\"{path}\""}}}}"#);
        let path = "a".repeat(0x7B - with("").len());
        let json = with(&path);
        assert_eq!(json.len(), 0x7B);
        let mut blob = (json.len() as u64).to_le_bytes().to_vec();
        blob.extend_from_slice(json.as_bytes());
        assert_eq!(blob[0], b'{');
        assert_eq!(state_json(&blob), (true, json.as_bytes()));
        assert_eq!(sample_path_of(&blob), Some(path.clone()));
        assert_eq!(state_json(json.as_bytes()), (false, json.as_bytes()));
        assert_eq!(sample_path_of(json.as_bytes()), Some(path));
    }

    /// An instance whose holds are short is taken at once, or skipped; those with long holds wait, all of them together
    /// no longer than the budget: a project with many instances keeps its frame rate during playback.
    #[test]
    fn the_ui_waits_little_for_many_instances() {
        let meter = os_host::Meter::default();
        // Mid-block: just processed a while ago, not yet idle.
        meter.add(Duration::ZERO);
        std::thread::sleep(Duration::from_millis(2));
        let fast = std::sync::Mutex::new(());
        let t0 = Instant::now();
        assert!(lock_for_tick(&fast, Some(&meter)).is_some(), "short holds: taken at once");
        assert!(t0.elapsed() < Duration::from_millis(2));
        meter.note_hold(Duration::from_millis(3));
        assert!(meter.holds_long());
        let t0 = Instant::now();
        for _ in 0..30 {
            assert!(lock_for_tick(&fast, Some(&meter)).is_none(), "not between blocks");
        }
        assert!(t0.elapsed() < Duration::from_millis(16), "{:?}: the budget, not 30 waits", t0.elapsed());
    }

    /// A long hold is remembered for a while, then fades.
    #[test]
    fn a_long_hold_fades() {
        let m = os_host::Meter::default();
        assert!(!m.holds_long());
        m.note_hold(Duration::from_millis(2));
        for _ in 0..100 {
            m.note_hold(Duration::from_micros(50));
        }
        assert!(m.holds_long(), "still slow");
        for _ in 0..400 {
            m.note_hold(Duration::from_micros(50));
        }
        assert!(!m.holds_long(), "faded");
    }

    /// A UI call holding a plugin 3 ms every 40 ms beside an audio thread that processes it every 5.33 ms (256 frames at
    /// 48 kHz) and gives up after waiting 2 ms, as the engine does: taken at any moment it makes the plugin late now and
    /// then; taken between two blocks (`lock_between_blocks`), never. Timing on the real clock for 3 s per way:
    /// `cargo test -p os-session --lib long_ui_calls_between_blocks -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn long_ui_calls_between_blocks_make_no_plugin_late() {
        use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
        use std::sync::{Arc, Mutex};
        fn run(between: bool) -> u32 {
            let plugin = Arc::new(Mutex::new(()));
            let meter = Arc::new(os_host::Meter::default());
            let (late, stop) = (Arc::new(AtomicU32::new(0)), Arc::new(AtomicBool::new(false)));
            let audio = {
                let (plugin, meter, late, stop) = (plugin.clone(), meter.clone(), late.clone(), stop.clone());
                std::thread::spawn(move || {
                    let (period, start) = (Duration::from_micros(5333), Instant::now());
                    for n in 1u32.. {
                        if stop.load(Ordering::Relaxed) {
                            break;
                        }
                        let waited = Instant::now();
                        let got = loop {
                            match plugin.try_lock() {
                                Ok(g) => break Some(g),
                                Err(_) if waited.elapsed() >= Duration::from_millis(2) => break None,
                                Err(_) => std::thread::yield_now(),
                            }
                        };
                        match got {
                            Some(_g) => {
                                let t0 = Instant::now();
                                while t0.elapsed() < Duration::from_micros(800) {
                                    std::hint::spin_loop();
                                }
                                meter.add(t0.elapsed());
                            }
                            None => {
                                late.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                        let next = start + period * n;
                        if let Some(d) = next.checked_duration_since(Instant::now()) {
                            std::thread::sleep(d);
                        }
                    }
                })
            };
            // Playing already: before its first block nothing processes the instance (`since_processed` is `None`).
            while meter.since_processed().is_none() {
                std::thread::sleep(Duration::from_millis(1));
            }
            let t0 = Instant::now();
            while t0.elapsed() < Duration::from_secs(3) {
                let g = if between { lock_between_blocks(&plugin, Some(&meter), STATE_WAIT) } else { plugin.lock().ok() };
                if let Some(_g) = g {
                    std::thread::sleep(Duration::from_millis(3));
                }
                std::thread::sleep(Duration::from_millis(40));
            }
            stop.store(true, Ordering::Relaxed);
            audio.join().unwrap();
            late.load(Ordering::Relaxed)
        }
        let (any, between) = (run(false), run(true));
        println!("late blocks in 3 s: {any} with the lock taken at any moment, {between} between blocks");
        assert!(any > 0, "the plain lock makes it late (else the test shows nothing)");
        assert_eq!(between, 0);
    }
}
