//! Hosting FL Studio native plugins ("Fruity plugins") from the user's own FL installation.
//!
//! Implemented from `OpenStudio-specs/plugins/fruity-abi.md` (public SDK interface).
//! Windows x64 only for now.
//
// spec: plugins/fruity-abi.md
#![allow(unsafe_op_in_unsafe_fn)]

pub mod abi;
pub mod host;
pub mod stream;

use std::ffi::{c_char, CString};
use std::path::{Path, PathBuf};

use abi::{flags, IntPtr, PluginObject, VoiceParams};
use host::HostObject;
use stream::MemStream;

#[derive(Debug, thiserror::Error)]
pub enum FruityError {
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
    pub fn load(dll: &Path, fl_dir: &Path, sample_rate: u32, max_block: usize) -> Result<Plugin, FruityError> {
        let err = |e: String| FruityError::Load(dll.to_owned(), e);
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
            return Err(FruityError::Create(dll.to_owned()));
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
