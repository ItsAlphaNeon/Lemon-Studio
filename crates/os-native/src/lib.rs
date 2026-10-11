//! Hosting plugins written against the FL Studio native plugin SDK, loaded from the user's own installation.
//!
//! Implemented from `OpenStudio-specs/plugins/fruity-abi.md` (public SDK interface).
//! Windows x64 only for now.
//
// spec: plugins/fruity-abi.md
#![allow(unsafe_op_in_unsafe_fn)]

pub mod abi;
pub mod adapter;
pub mod host;
pub mod stream;
pub mod stub;

use std::ffi::{c_char, CString};
use std::path::{Path, PathBuf};

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
/// `OPENSTUDIO_NATIVE` environment variable (`1`/`true`/`on`). The app also exposes a settings
/// toggle; it sets this at startup so the whole process agrees.
pub fn enabled() -> bool {
    matches!(
        std::env::var("OPENSTUDIO_NATIVE").as_deref(),
        Ok("1") | Ok("true") | Ok("on") | Ok("yes")
    )
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
}

/// A plugin instance. Not thread-safe: call from one thread.
pub struct Plugin {
    obj: *mut PluginObject,
    // Field order matters for drop: destroy the instance, then the host, then unload the DLL.
    host: Box<HostObject>,
    _lib: libloading::Library,
    pub info: PluginInfo,
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
        trace("LoadLibrary");
        let lib = unsafe { libloading::Library::new(dll) }.map_err(|e| err(e.to_string()))?;
        let create: libloading::Symbol<abi::CreatePlugInstance> =
            unsafe { lib.get(b"CreatePlugInstance\0") }.map_err(|e| err(e.to_string()))?;
        trace("got CreatePlugInstance");
        let mut host = HostObject::new(&fl_dir.to_string_lossy(), sample_rate, max_block);
        let host_ptr = &mut *host as *mut HostObject as *mut std::ffi::c_void;
        trace("calling CreatePlugInstance");
        let obj = unsafe { create(host_ptr, 0) };
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
        let mut p = Plugin { obj, host, _lib: lib, info };
        p.dispatch(abi::dispatch::SET_SAMPLE_RATE, 0, sample_rate as IntPtr);
        p.dispatch(abi::dispatch::SET_BLOCK_SIZE, 0, max_block as IntPtr);
        p.dispatch(abi::dispatch::PROCESS_MODE, 0, 0);
        Ok(p)
    }

    fn vt(&self) -> &abi::PluginVtbl {
        unsafe { &*(*self.obj).vtable }
    }

    pub fn dispatch(&mut self, id: IntPtr, index: IntPtr, value: IntPtr) -> IntPtr {
        unsafe { (self.vt().dispatcher)(self.obj, id, index, value) }
    }

    /// Sets the tempo (BPM) and samples per tick.
    pub fn set_tempo(&mut self, bpm: f32, ppq: u16) {
        let sr = self.host.context().sample_rate as f32;
        let spt = sr * 60.0 / bpm / f32::from(ppq);
        unsafe {
            (self.vt().process_event)(self.obj, abi::event::TEMPO, bpm.to_bits() as i32, spt as i32);
        }
        self.dispatch(abi::dispatch::SET_SAMPLES_PER_TICK, 0, spt.to_bits() as IntPtr);
    }

    pub fn param_name(&mut self, index: i32) -> String {
        let mut buf = [0 as c_char; 512];
        unsafe { (self.vt().get_name)(self.obj, abi::name::PARAM, index, 0, buf.as_mut_ptr()) };
        host::cstr(buf.as_ptr())
    }

    /// Current value of a parameter (plugin-specific integer scale).
    pub fn param_value(&mut self, index: i32) -> i32 {
        unsafe { (self.vt().process_param)(self.obj, index, 0, 2) }
    }

    /// Saves the plugin state into a byte vector.
    pub fn save_state(&mut self) -> Vec<u8> {
        let mut s = MemStream::new(Vec::new());
        unsafe { (self.vt().save_restore_state)(self.obj, s.as_ptr(), 1) };
        std::mem::take(&mut s.data)
    }

    /// Restores a previously saved plugin state.
    pub fn restore_state(&mut self, data: &[u8]) {
        let mut s = MemStream::new(data.to_vec());
        unsafe { (self.vt().save_restore_state)(self.obj, s.as_ptr(), 0) };
    }

    /// Starts a voice. `key` 60 = C5; `velocity` 0..1; `pan` −1..1.
    pub fn trigger(&mut self, key: f32, velocity: f32, pan: f32, tag: IntPtr) -> IntPtr {
        let lv = abi::LevelParams { pan, vol: velocity, pitch: key * 100.0, fcut: 0.0, fres: 0.0 };
        let vp = VoiceParams { init: lv, fin: lv };
        unsafe { (self.vt().trigger_voice)(self.obj, &vp, tag) }
    }

    pub fn release(&mut self, voice: IntPtr) {
        unsafe { (self.vt().voice_release)(self.obj, voice) }
    }

    pub fn kill(&mut self, voice: IntPtr) {
        unsafe { (self.vt().voice_kill)(self.obj, voice) }
    }

    /// Full generators: renders up to `out.len()/2` frames, returns frames rendered.
    pub fn gen_render(&mut self, out: &mut [f32]) -> usize {
        let mut n = (out.len() / 2) as i32;
        unsafe { (self.vt().gen_render)(self.obj, out.as_mut_ptr(), &mut n) };
        n.max(0) as usize
    }

    /// Hybrid generators: renders one voice; returns false when it has no more data.
    pub fn voice_render(&mut self, voice: IntPtr, out: &mut [f32]) -> (usize, bool) {
        let mut n = (out.len() / 2) as i32;
        let r = unsafe { (self.vt().voice_render)(self.obj, voice, out.as_mut_ptr(), &mut n) };
        (n.max(0) as usize, r == 0)
    }

    /// Effects: processes `buf` (interleaved stereo) in place.
    pub fn eff_render(&mut self, buf: &mut [f32]) {
        let n = (buf.len() / 2) as i32;
        let p = buf.as_mut_ptr();
        unsafe { (self.vt().eff_render)(self.obj, p, p, n) };
    }

    pub fn idle(&mut self) {
        unsafe { (self.vt().idle)(self.obj) }
    }

    /// Advances the plugin's internal clock one tick. Generators that set the `WantNewTick` flag
    /// expect this between buffers; without it their envelopes and timing do not progress.
    pub fn new_tick(&mut self) {
        unsafe { (self.vt().new_tick)(self.obj) }
    }

    /// Tells the plugin whether the song is playing (dispatcher `SetPlaying`).
    pub fn set_playing(&mut self, playing: bool) {
        self.dispatch(abi::dispatch::SET_PLAYING, 0, playing as IntPtr);
    }

    /// Enables or bypasses the plugin (dispatcher `SetEnabled`).
    pub fn set_enabled(&mut self, enabled: bool) {
        self.dispatch(abi::dispatch::SET_ENABLED, 0, enabled as IntPtr);
    }

    pub fn killed_voices(&mut self) -> Vec<IntPtr> {
        std::mem::take(&mut self.host.context().killed_voices)
    }

    pub fn host_name(&self) -> CString {
        CString::default()
    }
}

impl Drop for Plugin {
    fn drop(&mut self) {
        if !self.obj.is_null() {
            unsafe { (self.vt().destroy_object)(self.obj) };
            self.obj = std::ptr::null_mut();
        }
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

/// Loads an effect plugin and wraps it as a shared engine insert effect.
pub fn load_effect(dll: &Path, fl_dir: &Path, rate: u32) -> Result<os_engine::instrument::SharedEffect, NativeError> {
    let plugin = Plugin::load(dll, fl_dir, rate, DEFAULT_BLOCK)?;
    Ok(std::sync::Arc::new(std::sync::Mutex::new(adapter::NativeEffect::new(plugin, rate))))
}

/// Loads a generator plugin and wraps it as a shared engine instrument (experimental; see adapter).
pub fn load_instrument(dll: &Path, fl_dir: &Path, rate: u32) -> Result<os_engine::instrument::SharedInstrument, NativeError> {
    let plugin = Plugin::load(dll, fl_dir, rate, DEFAULT_BLOCK)?;
    Ok(std::sync::Arc::new(std::sync::Mutex::new(adapter::NativeInstrument::new(plugin, rate))))
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
}
