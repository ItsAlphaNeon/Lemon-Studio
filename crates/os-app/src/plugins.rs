//! Hosted generator plugins (CLAP): scanning, one instance per plugin channel, their windows, and the
//! instrument list the transport plays.
//!
// spec: plugins/plugin-formats.md (hosting, where to find plugins, Open Sampler state), ui/plugin-window.md

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use os_clap::{Descriptor, Library, Plugin, SharedPlugin};
use os_engine::SharedInstrument;

#[cfg(windows)]
use crate::plugin_window::{ChannelInfo, FrameEvent, FrameWindow};

/// A plugin offered in the picker.
#[derive(Clone)]
pub struct Available {
    pub lib: Arc<Library>,
    pub desc: Descriptor,
}

pub struct Hosted {
    pub plugin: SharedPlugin,
    #[cfg(windows)]
    pub window: Option<FrameWindow>,
}

/// What a plugin window asks of the app.
#[derive(Clone, Debug, PartialEq)]
pub enum Request {
    Close(usize),
    ToggleOn(usize),
    Volume(usize, f64),
    Pan(usize, f64),
    PitchCents(usize, i32),
}

#[derive(Default)]
pub struct Plugins {
    pub available: Vec<Available>,
    /// By rack row.
    pub hosted: BTreeMap<usize, Hosted>,
    /// Main window handle (owner of plugin windows).
    pub owner: isize,
    pub scale: f32,
    rate: u32,
}

/// Folders searched for `.clap` files: the standard CLAP folders, then `bundled/` next to the build folders of this
/// workspace (where `cargo run -p bundle-plugins` puts OpenStudio's own plugins).
fn search_dirs() -> Vec<PathBuf> {
    let mut dirs = os_clap::standard_dirs();
    if let Ok(exe) = std::env::current_exe() {
        // target*/release/openstudio.exe -> target*/bundled, and the sibling build folders.
        if let Some(target) = exe.parent().and_then(|p| p.parent()) {
            dirs.push(target.join("bundled"));
            if let Some(ws) = target.parent() {
                for t in ["target", "target-dev"] {
                    dirs.push(ws.join(t).join("bundled"));
                }
            }
        }
    }
    dirs.dedup();
    dirs
}

impl Plugins {
    /// Scans the plugin folders for instruments (first file wins for a plugin id).
    pub fn scan() -> Plugins {
        let mut available: Vec<Available> = Vec::new();
        for f in os_clap::find_files(&search_dirs()) {
            match Library::load(&f) {
                Ok(lib) => {
                    for d in lib.plugins.iter().filter(|d| d.is_instrument()) {
                        if !available.iter().any(|a| a.desc.id == d.id) {
                            available.push(Available { lib: lib.clone(), desc: d.clone() });
                        }
                    }
                }
                Err(e) => eprintln!("plugin scan: {e}"),
            }
        }
        Plugins { available, scale: 1.0, ..Plugins::default() }
    }

    pub fn names(&self) -> Vec<String> {
        self.available.iter().map(|a| a.desc.name.clone()).collect()
    }

    /// Creates an instance of available plugin `index` for rack row `row` and activates it at the device rate.
    pub fn create(&mut self, row: usize, index: usize) -> Result<String, String> {
        let a = self.available.get(index).ok_or("no such plugin")?.clone();
        let mut p = Plugin::new(&a.lib, &a.desc.id)?;
        if self.rate == 0 {
            self.rate = crate::transport::Transport::device_rate().unwrap_or(48000);
        }
        if std::env::var_os("OS_DEBUG").is_some() {
            eprintln!("[plugins] {} activated at {} Hz", a.desc.name, self.rate);
        }
        if !p.activate(self.rate) {
            return Err(format!("{} could not start", a.desc.name));
        }
        self.hosted.insert(row, Hosted { plugin: Arc::new(Mutex::new(p)), #[cfg(windows)] window: None });
        Ok(a.desc.name)
    }

    /// The instances for the transport.
    pub fn instruments(&self) -> Vec<(usize, SharedInstrument)> {
        self.hosted.iter().map(|(&r, h)| (r, h.plugin.clone() as SharedInstrument)).collect()
    }

    /// Opens (or raises) row `row`'s plugin window.
    #[cfg(windows)]
    pub fn open_window(&mut self, row: usize, info: ChannelInfo) -> Result<(), String> {
        let (owner, scale) = (self.owner, self.scale);
        let h = self.hosted.get_mut(&row).ok_or("not a plugin channel")?;
        if let Some(w) = &h.window {
            w.raise();
            return Ok(());
        }
        let mut p = h.plugin.lock().map_err(|_| "plugin busy")?;
        let pos = crate::plugin_window::default_position(owner);
        // Create the frame at a provisional size, embed the editor, then fit the frame to the editor's real size.
        let w = FrameWindow::new(owner, info, (560.0, 437.0), scale, pos)?;
        let (ew, eh) = p.open_editor(w.area())?;
        w.set_editor_size((ew as f32 / scale, eh as f32 / scale));
        w.show();
        drop(p);
        h.window = Some(w);
        Ok(())
    }

    #[cfg(windows)]
    fn close_window(&mut self, row: usize) {
        if let Some(h) = self.hosted.get_mut(&row) {
            if let Ok(mut p) = h.plugin.lock() {
                p.close_editor();
            }
            h.window = None;
        }
    }

    /// UI tick: plugin housekeeping, then the windows' requests (a closed window is closed here).
    pub fn idle(&mut self, info: impl Fn(usize) -> Option<ChannelInfoT>) -> Vec<Request> {
        let mut out = Vec::new();
        let mut close = Vec::new();
        for (&row, h) in &mut self.hosted {
            if let Ok(mut p) = h.plugin.try_lock() {
                p.idle();
                #[cfg(windows)]
                if let (Some(w), Some((ew, eh))) = (&h.window, p.editor_size()) {
                    w.set_editor_size((ew as f32 / self.scale, eh as f32 / self.scale));
                }
            }
            #[cfg(windows)]
            if let Some(w) = &h.window {
                if let Some(i) = info(row) {
                    w.set_info(i);
                }
                for e in w.take_events() {
                    out.push(match e {
                        FrameEvent::Close => {
                            close.push(row);
                            Request::Close(row)
                        }
                        FrameEvent::ToggleOn => Request::ToggleOn(row),
                        FrameEvent::Volume(v) => Request::Volume(row, v),
                        FrameEvent::Pan(v) => Request::Pan(row, v),
                        FrameEvent::PitchCents(c) => Request::PitchCents(row, c),
                    });
                }
            }
        }
        #[cfg(windows)]
        for row in close {
            self.close_window(row);
        }
        out
    }
}

#[cfg(windows)]
pub type ChannelInfoT = ChannelInfo;
#[cfg(not(windows))]
pub type ChannelInfoT = ();
