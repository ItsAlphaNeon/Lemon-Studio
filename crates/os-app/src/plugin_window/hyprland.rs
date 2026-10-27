// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Hyprland as the window manager of the detached frames (XWayland windows).
//! - It lists `_NET_WM_MOVERESIZE` as supported but ignores it, so a title-bar drag would not move the frame: the frame
//!   moves itself instead (Hyprland applies a floating XWayland window's own moves), see `x11`.
//! - It draws its border and shadow around every window, the frame's own title bar and edge included: once a frame is
//!   on screen, its IPC socket takes them off that window (`plain`).

use std::collections::HashSet;
use std::io::{Read as _, Write as _};
use std::os::unix::net::UnixStream;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Hyprland runs this session.
pub fn running() -> bool {
    std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some()
}

/// The windows already made plain (Hyprland addresses).
static PLAIN: Mutex<Option<HashSet<String>>> = Mutex::new(None);

/// A frame was just mapped: within the next second and a half, takes the border and shadow off every XWayland window of
/// this process that Hyprland shows (all of them are frames; the main window is a Wayland one, or no frame is apart
/// from it). In the background: nothing waits for the compositor.
pub fn plain() {
    if !running() {
        return;
    }
    std::thread::spawn(|| {
        let start = Instant::now();
        while start.elapsed() < Duration::from_millis(1500) {
            std::thread::sleep(Duration::from_millis(40));
            let Some(fresh) = ours() else { return };
            if fresh.is_empty() {
                continue;
            }
            for a in &fresh {
                set(a, "border_size", "0");
                set(a, "no_shadow", "1");
            }
            PLAIN.lock().unwrap().get_or_insert_with(HashSet::new).extend(fresh);
            return;
        }
    });
}

/// This process's XWayland windows not made plain yet (`None`: no answer from Hyprland).
fn ours() -> Option<Vec<String>> {
    let list: serde_json::Value = serde_json::from_str(&ask("j/clients")?).ok()?;
    let pid = std::process::id() as u64;
    let done = PLAIN.lock().unwrap();
    Some(
        list.as_array()?
            .iter()
            .filter(|c| c["pid"].as_u64() == Some(pid) && c["xwayland"].as_bool() == Some(true))
            .filter_map(|c| c["address"].as_str())
            .filter(|a| !done.as_ref().is_some_and(|d| d.contains(*a)))
            .map(str::to_owned)
            .collect(),
    )
}

/// Sets a window property: a Lua dispatch (Hyprland's Lua configuration), else the older syntax.
fn set(address: &str, prop: &str, value: &str) {
    let lua = format!("dispatch hl.dsp.window.set_prop({{ prop = '{prop}', value = '{value}', window = 'address:{address}' }})");
    if ask(&lua).as_deref() != Some("ok") {
        ask(&format!("dispatch setprop address:{address} {prop} {value}"));
    }
}

/// One request on Hyprland's socket, its answer.
fn ask(request: &str) -> Option<String> {
    let dir = std::env::var_os("XDG_RUNTIME_DIR")?;
    let sig = std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE")?;
    let path = std::path::Path::new(&dir).join("hypr").join(sig).join(".socket.sock");
    let mut s = UnixStream::connect(path).ok()?;
    s.set_read_timeout(Some(Duration::from_millis(500))).ok()?;
    s.write_all(request.as_bytes()).ok()?;
    let mut out = String::new();
    s.read_to_string(&mut out).ok()?;
    Some(out.trim().to_owned())
}
