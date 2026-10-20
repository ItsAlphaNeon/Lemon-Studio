// Copyright (C) 2026 ItsAlphaNeon
// Copyright (C) 2026 ariymarkowitz
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! `lemonstudio --plugin-window-check <dir> <plugin file>...` (macOS): opens each plugin's editor in a real plugin
//! window, as the app does, at the display's scale, then at 1x and 2x (a window moved between monitors), and checks at
//! each that the frame's editor area fits the editor's view (neither clipped nor left with a gap). A picture of the
//! window at each scale goes to `<dir>`. The format is the file's (`.vst`, `.vst3`, `.clap`).
//!
//! Plugin windows need the main thread on macOS, which tests never get, so this runs from the program itself (the
//! Windows checks are ignored tests in `plugins.rs`). The app is not activated: the windows open behind the one in use.
//! Exits 1 when an editor did not fit or a plugin did not open.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use objc2_app_kit::NSView;
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
use winit::platform::pump_events::EventLoopExtPumpEvents;
use winit::window::WindowId;

use super::{load_file, Available, Plugins};
use crate::plugin_window::ChannelInfo;

/// Nothing to handle: the frames are not winit windows.
struct Idle;

impl ApplicationHandler for Idle {
    fn resumed(&mut self, _: &ActiveEventLoop) {}
    fn window_event(&mut self, _: &ActiveEventLoop, _: WindowId, _: WindowEvent) {}
}

/// Runs the windows and the plugins' idle calls for `ms`.
fn pump(el: &mut EventLoop<()>, pl: &mut Plugins, ms: u64) {
    let end = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < end {
        el.pump_app_events(Some(Duration::from_millis(10)), &mut Idle);
        pl.idle(|_| None);
    }
}

/// A view's size in points.
fn size_of(view: &NSView) -> (f64, f64) {
    let s = view.frame().size;
    (s.width, s.height)
}

/// A view's frame in its superview, in points: x, y, width, height.
fn frame_of(view: &NSView) -> (f64, f64, f64, f64) {
    let f = view.frame();
    (f.origin.x, f.origin.y, f.size.width, f.size.height)
}

/// What one look at an open window found: the editor area and the editor's own view's frame in it, in points; the
/// window's number.
struct Look {
    area: (f64, f64),
    editor: Option<(f64, f64, f64, f64)>,
    window: Option<isize>,
}

fn look(pl: &Plugins) -> Option<Look> {
    let w = pl.hosted.get(&0)?.window.as_ref()?;
    // SAFETY: the frame's views live as long as its window, which `pl` holds.
    let (frame, area) = unsafe { (&*(w.hwnd() as *const NSView), &*(w.area() as *const NSView)) };
    let editor = area.subviews().firstObject().map(|v| frame_of(&v));
    Some(Look { area: size_of(area), editor, window: frame.window().map(|win| win.windowNumber()) })
}

/// The window's picture (no shadow), with `screencapture`.
fn shoot(window: isize, path: &Path) -> bool {
    std::process::Command::new("screencapture").arg("-x").arg("-o").arg(format!("-l{window}")).arg(path).status().is_ok_and(|s| s.success())
}

pub fn run(args: &[String]) -> i32 {
    let Some((dir, files)) = args.split_first() else {
        eprintln!("usage: lemonstudio --plugin-window-check <dir> <plugin file>...");
        return 2;
    };
    let dir = PathBuf::from(dir);
    let _ = std::fs::create_dir_all(&dir);
    let mut builder = EventLoop::<()>::with_user_event();
    builder.with_activation_policy(ActivationPolicy::Accessory).with_activate_ignoring_other_apps(false).with_default_menu(false);
    let mut el = builder.build().expect("event loop");
    let mut failed = false;
    for file in files.iter().map(PathBuf::from) {
        let format = match file.extension().and_then(|x| x.to_str()).map(str::to_ascii_lowercase).as_deref() {
            Some("vst") => os_host::Format::Vst2,
            Some("vst3") => os_host::Format::Vst3,
            Some("clap") => os_host::Format::Clap,
            _ => {
                println!("{}: not a plugin file", file.display());
                failed = true;
                continue;
            }
        };
        let lib = match load_file(&file, format) {
            Ok(l) => l,
            Err(e) => {
                println!("{}: {e}", file.display());
                failed = true;
                continue;
            }
        };
        let Some(desc) = lib.plugins().first().cloned() else { continue };
        let name = desc.name.clone();
        let mut pl = Plugins { available: vec![Available { lib, desc }], scale: 2.0, ..Plugins::default() };
        if let Err(e) = pl.create(0, 0) {
            println!("{name}: {e}");
            failed = true;
            continue;
        }
        if !pl.hosted[&0].plugin.lock().is_ok_and(|p| p.has_editor()) {
            println!("{name}: no editor");
            continue;
        }
        let info = ChannelInfo { name: name.clone(), target: "Insert 1".into(), enabled: true, volume: 0.78, pan: 0.0, pitch_cents: 0, range: 2, track: 1, max_track: 125, misc: None };
        if let Err(e) = pl.open_window(0, info) {
            println!("{name}: the window did not open: {e}");
            failed = true;
            continue;
        }
        pump(&mut el, &mut pl, 900);
        let stem = name.replace(|c: char| !c.is_ascii_alphanumeric(), "_");
        let display = pl.hosted[&0].window.as_ref().map_or(2.0, |w| w.scale());
        for (k, tag) in [(display, "display"), (1.0, "1x"), (2.0, "2x")] {
            if tag != "display" {
                pl.rescale_window(0, k);
                pump(&mut el, &mut pl, 700);
            }
            let reported = pl.hosted[&0].plugin.lock().ok().and_then(|p| p.editor_size());
            let Some(l) = look(&pl) else {
                println!("{name} at {k}: the window is gone");
                failed = true;
                break;
            };
            // The editor's view fills the area: its size, and at the area's origin (not left where a smaller area had it).
            let fits = l.editor.is_some_and(|(x, y, w, h)| x.abs() < 0.5 && y.abs() < 0.5 && (w - l.area.0).abs() < 0.5 && (h - l.area.1).abs() < 0.5);
            failed |= !fits;
            let shot = dir.join(format!("{stem}_{tag}.png"));
            let pictured = l.window.is_some_and(|n| shoot(n, &shot));
            println!(
                "{name} at {k} ({tag}): editor view (x, y, w, h) {:?} pt, editor area {:?} pt, reported {reported:?} px: {}{}",
                l.editor,
                l.area,
                if fits { "fits" } else { "DOES NOT FIT" },
                if pictured { format!(", {}", shot.display()) } else { String::new() }
            );
        }
        pl.drop_key(0);
        pump(&mut el, &mut pl, 200);
    }
    i32::from(failed)
}
