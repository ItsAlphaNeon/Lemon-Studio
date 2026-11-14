// Copyright (C) 2026 ItsAlphaNeon
// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Plugin editors resized through real X11 frames (moved from `os_app::plugins` with the frames, control-plane step 10).

use crate::frames::NativeFrames;
use os_app::plugins::*;
use os_host::SharedPlugin;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use os_app::editor_host::EditorHost;
use os_app::plugin_window::{ChannelInfo, FrameEvent};
use std::ffi::c_void;
use std::sync::Mutex;
use x11rb::connection::Connection as _;
use x11rb::protocol::xproto::ConnectionExt as _;

/// An editor that takes sizes in steps of 10 px, from 100 x 100 to 4000 x 4000, and counts the resizes it is asked for.
struct Stretchy {
    desc: os_host::Descriptor,
    size: (u32, u32),
    resizable: bool,
    asked: Arc<Mutex<Vec<(u32, u32)>>>,
}
impl os_engine::Instrument for Stretchy {
    fn note_on(&mut self, _: u32, _: u8, _: f32) {}
    fn note_off(&mut self, _: u32, _: u8) {}
    fn process(&mut self, _: u32, _: &mut [f32], _: &mut [f32]) {}
}
impl os_engine::HostedEffect for Stretchy {
    fn process_fx(&mut self, _: u32, _: &mut [f32], _: &mut [f32]) {}
}
impl os_host::Plugin for Stretchy {
    fn desc(&self) -> &os_host::Descriptor {
        &self.desc
    }
    fn activate(&mut self, _: u32) -> bool {
        true
    }
    fn idle(&mut self) {}
    fn has_editor(&self) -> bool {
        true
    }
    fn editor_open(&self) -> bool {
        true
    }
    fn open_editor(&mut self, _: *mut c_void, _: f32) -> Result<(u32, u32), String> {
        Ok(self.size)
    }
    fn editor_size(&self) -> Option<(u32, u32)> {
        Some(self.size)
    }
    fn close_editor(&mut self) {}
    fn can_resize_editor(&self) -> bool {
        self.resizable
    }
    fn resize_editor(&mut self, w: u32, h: u32) -> Option<(u32, u32)> {
        self.asked.lock().unwrap().push((w, h));
        if !self.resizable {
            return None;
        }
        let step = |v: u32| (v / 10 * 10).clamp(100, 4000);
        self.size = (step(w), step(h));
        Some(self.size)
    }
    fn save_state(&self) -> Option<Vec<u8>> {
        None
    }
    fn load_state(&mut self, _: &[u8]) -> bool {
        false
    }
}

fn open(size: (u32, u32), resizable: bool) -> (Plugins, EditorHost, Arc<Mutex<Vec<(u32, u32)>>>) {
    let asked = Arc::new(Mutex::new(Vec::new()));
    let desc = os_host::Descriptor { format: os_host::Format::Clap, id: "stretchy".into(), name: "Stretchy".into(), vendor: String::new(), version: String::new(), features: Vec::new(), kind: os_host::Kind::Instrument, path: PathBuf::new() };
    let p: SharedPlugin = Arc::new(Mutex::new(Stretchy { desc, size, resizable, asked: asked.clone() }));
    let mut pl = Plugins::default();
    (pl.scale, pl.rate) = (1.0, 48000);
    pl.hosted.insert(0, Hosted::new(p));
    let info = ChannelInfo { icon: None, name: "Stretchy".into(), target: "Bus 1".into(), enabled: true, volume: 0.78, pan: 0.0, pitch_cents: 0, range: 2, track: 1, max_track: 125, misc: None };
    let mut host = EditorHost::default();
    host.frames = Box::new(NativeFrames);
    host.open_window(&mut pl, 0, info).unwrap();
    (pl, host, asked)
}

fn tick(pl: &mut Plugins, host: &mut EditorHost, n: usize) {
    for _ in 0..n {
        host.idle(pl, |_| None);
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn geometry(c: &x11rb::rust_connection::RustConnection, win: u32) -> (u32, u32) {
    let g = c.get_geometry(win).unwrap().reply().unwrap();
    (u32::from(g.width), u32::from(g.height))
}

/// A real plugin's editor (`OS_RESIZE_PLUGIN=<a .clap or .vst3>`) follows its frame's edges, if it can be
/// resized; prints what happened. Opens windows: a private X server only, as above.
#[test]
#[ignore]
fn real_editor_follows_frame_resizes() {
    let Some(path) = std::env::var_os("OS_RESIZE_PLUGIN") else { return };
    let path = PathBuf::from(path);
    let format = if path.extension().is_some_and(|e| e == "vst3") { os_host::Format::Vst3 } else { os_host::Format::Clap };
    let lib = load_file(&path, format).expect("load the plugin");
    let desc = lib.plugins()[0].clone();
    let (c, _) = x11rb::connect(None).expect("an X server (DISPLAY)");
    let mut pl = Plugins::default();
    (pl.available, pl.scale, pl.rate) = (vec![Available { lib, desc }], 1.0, 48000);
    let mut host = EditorHost::default();
    host.frames = Box::new(NativeFrames);
    pl.create(0, 0).unwrap();
    let info = ChannelInfo { icon: None, name: "Resize".into(), target: "Bus 1".into(), enabled: true, volume: 0.78, pan: 0.0, pitch_cents: 0, range: 2, track: 1, max_track: 125, misc: None };
    host.open_window(&mut pl, 0, info).unwrap();
    tick(&mut pl, &mut host, 30);
    let w = host.editors[&0].window.as_ref().unwrap();
    let (win, area, k) = (w.hwnd() as u32, w.area() as usize as u32, w.scale());
    let chrome = w.chrome();
    let chrome = ((chrome.0 * k).round() as u32, (chrome.1 * k).round() as u32);
    let editor_window = || c.query_tree(area).unwrap().reply().unwrap().children.first().map(|&e| geometry(&c, e));
    let can = pl.hosted[&0].plugin.lock().unwrap().can_resize_editor();
    let before = pl.hosted[&0].plugin.lock().unwrap().editor_size().unwrap();
    println!("{}: can resize {can}; editor {before:?}, frame {:?}, editor window {:?}", path.display(), geometry(&c, win), editor_window());
    for target in [(before.0 + 200, before.1 + 150), (before.0 * 3 / 4, before.1 * 3 / 4)] {
        let logical = |v: u32| (v as f32 / k).round() as u32;
        host.editors[&0].window.as_ref().unwrap().push_event(FrameEvent::Resize(logical(target.0), logical(target.1)));
        tick(&mut pl, &mut host, 40);
        let now = pl.hosted[&0].plugin.lock().unwrap().editor_size().unwrap();
        let frame = geometry(&c, win);
        println!("asked {target:?}: editor {now:?}, frame {frame:?}, area {:?}, editor window {:?}", geometry(&c, area), editor_window());
        if can {
            assert_ne!(now, before, "the editor follows the window");
        } else {
            assert_eq!(now, before, "an editor that cannot be resized keeps its size");
        }
        assert_eq!(frame, (now.0 + chrome.0, now.1 + chrome.1), "the frame matches the editor");
    }
    drop(pl);
}

#[test]
#[ignore]
fn editors_follow_frame_resizes() {
    let (c, screen) = x11rb::connect(None).expect("an X server (DISPLAY)");
    let root = c.setup().roots[screen].clone();
    let (mut pl, mut host, asked) = open((600, 400), true);
    tick(&mut pl, &mut host, 5);
    let w = host.editors[&0].window.as_ref().unwrap();
    let (win, area, k) = (w.hwnd() as u32, w.area() as usize as u32, w.scale());
    let chrome = w.chrome();
    let chrome = ((chrome.0 * k).round() as u32, (chrome.1 * k).round() as u32);
    assert_eq!(geometry(&c, win), (600 + chrome.0, 400 + chrome.1), "the frame fits the editor");
    assert!(asked.lock().unwrap().is_empty(), "opening the window asks no resize");

    // A drag of the frame's edges: the editor is asked, and the frame takes the size it took.
    w.push_event(FrameEvent::Resize(1005, 703));
    tick(&mut pl, &mut host, 5);
    let asked_now = asked.lock().unwrap().clone();
    println!("asked {asked_now:?}; frame {:?}, area {:?}", geometry(&c, win), geometry(&c, area));
    let px = |v: f32| (v * k).round() as u32;
    assert_eq!(asked_now, vec![(px(1005.0), px(703.0))]);
    let took = pl.hosted[&0].plugin.lock().unwrap().editor_size().unwrap();
    assert_eq!(geometry(&c, area), took, "the editor area takes the size the editor took");
    assert_eq!(geometry(&c, win), (took.0 + chrome.0, took.1 + chrome.1), "the frame follows the editor");
    drop(pl);

    // An editor that cannot be resized: nothing is asked of it.
    let (mut pl, mut host, asked) = open((600, 400), false);
    tick(&mut pl, &mut host, 5);
    host.editors[&0].window.as_ref().unwrap().push_event(FrameEvent::Resize(900, 900));
    tick(&mut pl, &mut host, 5);
    assert!(asked.lock().unwrap().is_empty(), "nothing asked of an editor that cannot be resized");
    drop(pl);

    // Larger than the screen: a resizable editor opens fitted to it, another one as it is.
    let (sw, sh) = (u32::from(root.width_in_pixels), u32::from(root.height_in_pixels));
    let (mut pl, mut host, asked) = open((sw + 500, sh + 300), true);
    tick(&mut pl, &mut host, 5);
    let win = host.editors[&0].window.as_ref().unwrap().hwnd() as u32;
    let (fw, fh) = geometry(&c, win);
    println!("screen {sw} x {sh}: a {} x {} editor opens in a {fw} x {fh} window; asked {:?}", sw + 500, sh + 300, asked.lock().unwrap());
    assert!(fw <= sw && fh <= sh, "the window fits the screen");
    assert!(fw + 10 > sw && fh + 10 > sh, "and uses it");
    drop(pl);
    let (mut pl, mut host, _) = open((sw + 500, sh + 300), false);
    tick(&mut pl, &mut host, 5);
    let win = host.editors[&0].window.as_ref().unwrap().hwnd() as u32;
    assert!(geometry(&c, win).0 > sw, "an editor that cannot be resized is left as it is");
}
