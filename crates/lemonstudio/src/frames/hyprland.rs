// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Hyprland as the window manager of the detached frames (XWayland windows).
//! - It lists `_NET_WM_MOVERESIZE` as supported but ignores it, so a title-bar drag would not move the frame. Moving it
//!   ourselves through X11 shakes: XWayland reports the pointer against where the window is drawn, which lags behind
//!   where it was asked to go, and every step overshoots. So the drag asks Hyprland where the pointer is and has it put
//!   the window there, both in its own coordinates (`Drag`).
//! - It draws its border and shadow around every window, the frame's own title bar and edge included, and animates
//!   moves (a dragged window would trail the pointer): once a frame is on screen, its IPC socket takes all three off
//!   that window (`plain`).

use std::collections::HashSet;
use std::io::{Read as _, Write as _};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, AtomicI8, AtomicU8, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Hyprland runs this session.
pub fn running() -> bool {
    std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some()
}

/// The windows already made plain (Hyprland addresses).
static PLAIN: Mutex<Option<HashSet<String>>> = Mutex::new(None);
/// Cached probe of whether Hyprland Lua dispatch (`hl.dsp.window.*`) is supported: 0 unknown, 1 yes, 2 no.
static LUA_DISPATCH: AtomicU8 = AtomicU8::new(0);
/// Cached `main_floats()` result: -1 unknown, 0 false, 1 true.
static MAIN_FLOATS: AtomicI8 = AtomicI8::new(-1);
/// Coalesces concurrent background refreshes of `main_floats()`.
static REFRESHING_FLOATS: AtomicBool = AtomicBool::new(false);

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
                set(a, "no_anim", "1");
            }
            PLAIN.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashSet::new).extend(fresh);
            return;
        }
    });
}

/// A title-bar drag: the window under the pointer (one of ours) and where the pointer holds it.
pub struct Drag {
    address: String,
    /// The pointer from the window's top-left (Hyprland's logical px).
    hold: (f64, f64),
    last: Instant,
}

impl Drag {
    /// Starts dragging the frame under the pointer; `None` when Hyprland does not answer or none of ours is there (the
    /// most recently focused one, where they overlap).
    pub fn start() -> Option<Drag> {
        let (cx, cy) = cursor()?;
        let list: serde_json::Value = serde_json::from_str(&ask("j/clients")?).ok()?;
        let pid = std::process::id() as u64;
        let num = |v: &serde_json::Value, i: usize| v[i].as_f64().unwrap_or(0.0);
        let (c, at) = list
            .as_array()?
            .iter()
            .filter(|c| c["pid"].as_u64() == Some(pid) && c["xwayland"].as_bool() == Some(true))
            .map(|c| (c, (num(&c["at"], 0), num(&c["at"], 1)), (num(&c["size"], 0), num(&c["size"], 1))))
            .filter(|(_, at, size)| cx >= at.0 && cy >= at.1 && cx < at.0 + size.0 && cy < at.1 + size.1)
            .min_by_key(|(c, ..)| c["focusHistoryID"].as_i64().unwrap_or(i64::MAX))
            .map(|(c, at, _)| (c, at))?;
        let address = c["address"].as_str()?.to_owned();
        Some(Drag { address, hold: (cx - at.0, cy - at.1), last: Instant::now() - Duration::from_secs(1) })
    }

    /// The pointer moved: the window goes with it, at most every 8 ms (`now`: the release, always).
    pub fn follow(&mut self, now: bool) {
        if !now && self.last.elapsed() < Duration::from_millis(8) {
            return;
        }
        self.last = Instant::now();
        let Some((cx, cy)) = cursor() else { return };
        let (x, y) = ((cx - self.hold.0).round(), (cy - self.hold.1).round());
        let a = &self.address;
        dispatch_with_lua(
            &format!("dispatch hl.dsp.window.move({{ x = {x}, y = {y}, window = 'address:{a}' }})"),
            &format!("dispatch movewindowpixel exact {x} {y},address:{a}"),
            Duration::from_millis(50),
        );
    }
}

/// The pointer (Hyprland's logical px).
fn cursor() -> Option<(f64, f64)> {
    let v: serde_json::Value = serde_json::from_str(&ask_timeout("j/cursorpos", Duration::from_millis(50))?).ok()?;
    Some((v["x"].as_f64()?, v["y"].as_f64()?))
}

/// Whether this process's main window (its Wayland one) floats (`None`: no answer, or no such window).
pub fn main_floats() -> Option<bool> {
    let list: serde_json::Value = serde_json::from_str(&ask("j/clients")?).ok()?;
    let pid = std::process::id() as u64;
    let f = list
        .as_array()?
        .iter()
        .find(|c| c["pid"].as_u64() == Some(pid) && c["xwayland"].as_bool() == Some(false))
        .and_then(|c| c["floating"].as_bool())?;
    MAIN_FLOATS.store(if f { 1 } else { 0 }, Ordering::Release);
    Some(f)
}

/// Last known `main_floats()` value, without touching Hyprland's socket on the UI thread.
pub fn cached_main_floats() -> Option<bool> {
    match MAIN_FLOATS.load(Ordering::Acquire) {
        0 => Some(false),
        1 => Some(true),
        _ => None,
    }
}

/// Refreshes `main_floats()` off the UI thread (coalesced so live resizes never stack threads) and invokes `on_change`
/// when the value changes.
pub fn refresh_main_floats(on_change: impl FnOnce(bool) + Send + 'static) {
    if !running() || REFRESHING_FLOATS.swap(true, Ordering::AcqRel) {
        return;
    }
    let _ = std::thread::Builder::new().name("lemonstudio-hypr".into()).spawn(move || {
        let prev = MAIN_FLOATS.load(Ordering::Acquire);
        let next = main_floats();
        REFRESHING_FLOATS.store(false, Ordering::Release);
        if let Some(f) = next {
            let code = if f { 1 } else { 0 };
            if code != prev {
                on_change(f);
            }
        }
    });
}

/// This process's XWayland windows not made plain yet (`None`: no answer from Hyprland).
fn ours() -> Option<Vec<String>> {
    let list: serde_json::Value = serde_json::from_str(&ask("j/clients")?).ok()?;
    let pid = std::process::id() as u64;
    let live: Vec<&str> = list
        .as_array()?
        .iter()
        .filter(|c| c["pid"].as_u64() == Some(pid) && c["xwayland"].as_bool() == Some(true))
        .filter_map(|c| c["address"].as_str())
        .collect();
    let mut done = PLAIN.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(set) = done.as_mut() {
        set.retain(|a| live.iter().any(|l| l == a));
    }
    Some(live.into_iter().filter(|a| !done.as_ref().is_some_and(|d| d.contains(*a))).map(str::to_owned).collect())
}

/// Sets a window property: a Lua dispatch (Hyprland's Lua configuration), else the older syntax.
fn set(address: &str, prop: &str, value: &str) {
    dispatch_with_lua(
        &format!("dispatch hl.dsp.window.set_prop({{ prop = '{prop}', value = '{value}', window = 'address:{address}' }})"),
        &format!("dispatch setprop address:{address} {prop} {value}"),
        Duration::from_millis(500),
    );
}

/// Runs `lua` only when Lua dispatch is unknown or confirmed working; caches failure so non-Lua Hyprland doesn't pay a
/// wasted socket round-trip on every drag step.
fn dispatch_with_lua(lua: &str, fallback: &str, timeout: Duration) {
    match LUA_DISPATCH.load(Ordering::Relaxed) {
        1 => {
            if ask_timeout(lua, timeout).as_deref() != Some("ok") {
                ask_timeout(fallback, timeout);
            }
        }
        2 => {
            ask_timeout(fallback, timeout);
        }
        _ => {
            if ask_timeout(lua, timeout).as_deref() == Some("ok") {
                LUA_DISPATCH.store(1, Ordering::Relaxed);
            } else {
                LUA_DISPATCH.store(2, Ordering::Relaxed);
                ask_timeout(fallback, timeout);
            }
        }
    }
}

/// One request on Hyprland's socket, its answer.
fn ask(request: &str) -> Option<String> {
    ask_timeout(request, Duration::from_millis(500))
}

fn ask_timeout(request: &str, timeout: Duration) -> Option<String> {
    let dir = std::env::var_os("XDG_RUNTIME_DIR")?;
    let sig = std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE")?;
    let path = std::path::Path::new(&dir).join("hypr").join(sig).join(".socket.sock");
    let mut s = UnixStream::connect(path).ok()?;
    s.set_read_timeout(Some(timeout)).ok()?;
    s.set_write_timeout(Some(timeout)).ok()?;
    s.write_all(request.as_bytes()).ok()?;
    let mut out = String::new();
    s.read_to_string(&mut out).ok()?;
    Some(out.trim().to_owned())
}
