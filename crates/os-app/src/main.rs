// Copyright (C) 2026 ItsAlphaNeon
// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Studio: the application window.

// Plugin windows exist on Windows, Linux and macOS (`plugin_window`): elsewhere what only they reach (the Misc tab, the
// window and preset menus, their requests) is built but unused. Those builds still report real dead code.
#![cfg_attr(not(plugin_windows), allow(dead_code))]

mod app;
mod audio_cfg;
mod auto_close;
mod autoscroll;
mod backup;
#[cfg(test)]
mod audio_bench;
#[cfg(test)]
mod open_bench;
mod browser;
mod browser_index;
mod cable;
mod capture;
mod one_click;
mod track_rec;
mod audio_log;
mod score_log;
mod clip_colors;
mod clock;
mod control;
mod cpu;
mod data_folder;
mod device;
mod dialog;
mod filedlg;
mod enable_groups;
mod export;
mod export_form;
mod factory_folder;
mod high_visibility;
mod history;
mod hold;
mod keep_on_disk;
mod icons;
mod init;
mod knobs;
mod layout;
mod loading;
mod missing;
mod mix_threads;
mod zipped;
mod logo;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
mod menu_bar;
mod messages;
mod meters;
mod metronome;
mod midi_cfg;
mod midi_file;
mod midi_in;
mod midi_out;
mod midi_play;
mod mixer;
mod osc3;
mod citrus;
mod monitor;
mod params;
mod piano;
mod piano_snap;
mod piano_split;
mod piano_tools;
mod rack;
mod remote;
mod render;
mod pattern_render;
mod patterns;
mod perf;
mod playlist;
mod plugin_db;
mod plugin_manager;
mod plugin_menu;
mod plugin_popup;
mod plugin_rebuild;
mod plugin_scan;
#[cfg(plugin_windows)]
mod plugin_window;
mod plugins;
mod ppi;
mod prefs;
mod preview_track;
mod project_files;
mod project_forms;
mod project_info;
mod project_picker;
mod project_plugins;
mod presets;
mod misc;
mod misc_page;
mod note_names;
mod save_form;
mod scroll_speed;
mod search_folders;
mod settings_file;
mod shortcuts;
mod smooth;
mod snap;
mod song_pos;
mod toolbar;
mod splash;
mod stall;
mod step_edit;
mod strum;
mod flam;
mod flip;
mod levels;
mod chords;
mod claw;
mod limit;
mod lfo;
mod quantize;
mod randomize;
mod chop;
mod piano_dlg;
mod arpeggiate;
mod tapper;
mod touch;
mod events;
mod record;
mod recent;
mod theme_adjust;
mod theme_library;
mod templates;
mod timesig;
mod transport;
mod typein;
mod views;
mod wallpaper;
mod waves;
mod win_clamp;
#[cfg(windows)]
mod file_assoc;

use std::num::NonZeroU32;
use std::rc::Rc;
use std::time::{Duration, Instant};

use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{DeviceEvent, DeviceId, ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::ModifiersState;
use winit::window::{CursorGrabMode, CursorIcon, ResizeDirection, Window, WindowId};

use app::App;
use os_ui::Canvas;

struct Shell {
    app: App,
    window: Option<Rc<Window>>,
    surface: Option<softbuffer::Surface<Rc<Window>, Rc<Window>>>,
    canvas: Option<Canvas>,
    mods: ModifiersState,
    last_click: Instant,
    /// While a scroll-bar right-drag runs: the physical cursor position it is held at, and the
    /// virtual (logical) mouse position fed to the app.
    lock: Option<((f64, f64), (f32, f32))>,
    /// The held cursor is locked by the window system (Wayland's pointer lock): it stays put by itself and cannot be
    /// warped. Otherwise (Windows, X11) it is warped back to the anchor after each move.
    pointer_locked: bool,
    cursor: app::Cursor,
    /// Main-window border in device pixels (left, top, right, bottom); zero when maximized.
    inset: (u32, u32, u32, u32),
    /// The border edge under the mouse (drags resize the OS window).
    edge: Option<ResizeDirection>,
    /// A title-bar move or edge resize done by hand: cursor and window rect (screen px) at the press, and the edge
    /// (None = move). The system's own move / size loops block the event loop, which froze all drawing.
    grab: Option<Grab>,
    /// A press on the maximized window's title bar: the cursor (screen px) and its height in the window (physical px).
    /// The window is restored under the cursor and moved once the cursor travels past the drag distance; a click
    /// alone leaves it maximized.
    unmax: Option<((i32, i32), i32)>,
    /// A tick or mouse input left damage that is not painted yet: `about_to_wait` asks for the frame (see `FRAME`).
    pending: bool,
    /// When the last frame was painted (finished presenting), and when its paint started.
    painted: Instant,
    frame_start: Instant,
    /// Shortest time from one frame's start to the next, from the refresh rate of `frame_monitor` (see `frame_interval`).
    frame: Duration,
    frame_monitor: Option<winit::monitor::MonitorHandle>,
    /// The startup splash, until startup is done and it has faded away (`splash`).
    splash: Option<splash::Splash>,
    /// The main window has shown its first frame.
    shown: bool,
    /// `OS_FRAMES=<file>`: one line per frame (see `FrameLog`).
    frames: Option<FrameLog>,
    /// Wakes the app while a modal loop holds up winit's (see `stall`).
    watchdog: std::sync::Arc<stall::Watchdog>,
    /// macOS: the left button went down with Control held, which is a right click there; its release is one too.
    ctrl_click: bool,
    /// The main window's restored frame and whether it is maximized, kept for the next start as the app closes
    /// (`layout`): the last run's at first, then followed as the window moves and resizes (`follow_frame`).
    main_win: Option<layout::MainWin>,
}

/// Frame timing for tuning (`OS_FRAMES=<file>`): per frame the time, what was repainted (`full` or the number of
/// damage rects and their device pixels), paint and present milliseconds, and how long the oldest input it answers
/// waited; slow ticks too.
struct FrameLog {
    out: std::io::BufWriter<std::fs::File>,
    start: Instant,
    /// The first input event since the last frame.
    input: Option<Instant>,
}

impl FrameLog {
    fn open() -> Option<FrameLog> {
        let path = std::env::var_os("OS_FRAMES")?;
        let file = std::fs::File::create(path).ok()?;
        let mut log = FrameLog { out: std::io::BufWriter::new(file), start: Instant::now(), input: None };
        // Wall-clock start (Unix seconds), to line the log up with a script driving the app.
        let unix = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64());
        log.line(format_args!("start {unix:.3}"));
        Some(log)
    }

    fn line(&mut self, text: std::fmt::Arguments) {
        use std::io::Write;
        let _ = writeln!(self.out, "{:9.1} {text}", self.start.elapsed().as_secs_f64() * 1000.0);
        // Flushed at once: the app may be killed rather than closed.
        let _ = self.out.flush();
    }
}

/// Longest frame interval: frames asked for by ticks and mouse input (a drag moves the mouse hundreds of times a second)
/// come at least about 60 times a second, and at the monitor's rate on a faster one (`frame_interval`).
const FRAME: Duration = Duration::from_millis(16);

/// Shortest time from one frame's start to the next on a monitor refreshing at `mhz` millihertz: a little under one
/// refresh, as the timer wakes a little late. A window dragged at 60 frames a second trails the cursor on a faster
/// screen (the cursor moves at the screen's rate), and paints during a drag are short (about a millisecond).
fn frame_interval(mhz: Option<u32>) -> Duration {
    match mhz.filter(|&m| m > 0) {
        Some(m) => Duration::from_secs_f64(960.0 / f64::from(m)).clamp(IDLE, FRAME),
        None => FRAME,
    }
}

/// The time from one frame's start to the next: `frame` (a little under the display's refresh), and while something
/// animates no less than General ▸ Display ▸ Animation refresh rate allows (`smooth::frame_cap`; spec:
/// ui/settings-choices.md "Display: Smoothing, Animation refresh rate, Transparent menus and windows (2026-10-16)").
fn paced(frame: Duration, animating: bool) -> Duration {
    if animating { frame.max(smooth::frame_cap()) } else { frame }
}

/// Shortest idle time after a frame. Windows hands out a window's WM_PAINT ahead of its child windows' and ahead of
/// every WM_TIMER, so a main window that always has a frame pending starves the plugin windows (docked children and
/// detached ones alike) and the timers their editors animate with. The gap lets the loop go idle and serve them.
/// (Counting `FRAME` from the end of a frame instead held everything to 16 ms plus the paint: 30-45 fps.)
const IDLE: Duration = Duration::from_millis(4);

/// The app's Ctrl: the Control key, and on macOS the Command key (Command + S saves there).
fn ctrl(m: ModifiersState) -> bool {
    m.control_key() || (cfg!(target_os = "macos") && m.super_key())
}

#[derive(Clone, Copy)]
struct Grab {
    cursor: (i32, i32),
    rect: (i32, i32, i32, i32),
    edge: Option<ResizeDirection>,
}

/// Cursor position and outer window rect (left, top, right, bottom) in screen pixels.
#[cfg(windows)]
fn screen_state(window: &Window) -> Option<((i32, i32), (i32, i32, i32, i32))> {
    use windows_sys::Win32::Foundation::{POINT, RECT};
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetCursorPos, GetWindowRect};
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let RawWindowHandle::Win32(h) = window.window_handle().ok()?.as_raw() else { return None };
    let (mut p, mut r): (POINT, RECT) = unsafe { std::mem::zeroed() };
    unsafe {
        GetCursorPos(&mut p);
        GetWindowRect(h.hwnd.get() as _, &mut r);
    }
    Some(((p.x, p.y), (r.left, r.top, r.right, r.bottom)))
}

#[cfg(target_os = "macos")]
use macos::{is_maximized, screen_state, set_maximized, set_rect};

/// Whether the window fills the screen's work area. Windows: asked of the system, since winit hears of a maximize the
/// system makes (the taskbar, Win+Up, coming back from the taskbar) only after the move that comes with it, which would
/// be taken for the restored frame (`Shell::follow_frame`).
#[cfg(windows)]
fn is_maximized(window: &Window) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::IsZoomed;
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match window.window_handle().map(|h| h.as_raw()) {
        // SAFETY: our own live window.
        Ok(RawWindowHandle::Win32(h)) => unsafe { IsZoomed(h.hwnd.get() as _) != 0 },
        _ => window.is_maximized(),
    }
}

/// Whether the window fills the screen's work area.
#[cfg(not(any(windows, target_os = "macos")))]
fn is_maximized(window: &Window) -> bool {
    window.is_maximized()
}

#[cfg(not(target_os = "macos"))]
fn set_maximized(window: &Window, on: bool) {
    window.set_maximized(on);
}

#[cfg(not(any(windows, target_os = "macos")))]
fn screen_state(_: &Window) -> Option<((i32, i32), (i32, i32, i32, i32))> {
    None
}

/// Move / resize the window to `rect` (left, top, right, bottom) in one call.
#[cfg(windows)]
fn set_rect(window: &Window, r: (i32, i32, i32, i32)) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{SetWindowPos, SWP_NOACTIVATE, SWP_NOZORDER};
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    if let Ok(h) = window.window_handle() {
        if let RawWindowHandle::Win32(h) = h.as_raw() {
            unsafe { SetWindowPos(h.hwnd.get() as _, std::ptr::null_mut(), r.0, r.1, r.2 - r.0, r.3 - r.1, SWP_NOZORDER | SWP_NOACTIVATE) };
        }
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
fn set_rect(_: &Window, _: (i32, i32, i32, i32)) {}

/// Whether the cursor went from `from` to `to` (screen px) farther than the system lets a click wander before it
/// counts as a drag.
#[cfg(windows)]
fn past_drag_distance(from: (i32, i32), to: (i32, i32)) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXDRAG, SM_CYDRAG};
    let (dx, dy) = unsafe { (GetSystemMetrics(SM_CXDRAG), GetSystemMetrics(SM_CYDRAG)) };
    (to.0 - from.0).abs() > dx.max(1) || (to.1 - from.1).abs() > dy.max(1)
}

#[cfg(not(windows))]
fn past_drag_distance(from: (i32, i32), to: (i32, i32)) -> bool {
    (to.0 - from.0).abs() > 4 || (to.1 - from.1).abs() > 4
}

impl Grab {
    /// The window rect for the cursor at `cursor`, never below `min` (physical px).
    fn rect_at(&self, cursor: (i32, i32), min: (i32, i32)) -> (i32, i32, i32, i32) {
        use ResizeDirection::*;
        let (dx, dy) = (cursor.0 - self.cursor.0, cursor.1 - self.cursor.1);
        let (mut l, mut t, mut r, mut b) = self.rect;
        let Some(e) = self.edge else { return (l + dx, t + dy, r + dx, b + dy) };
        if matches!(e, West | NorthWest | SouthWest) {
            l = (l + dx).min(r - min.0);
        }
        if matches!(e, East | NorthEast | SouthEast) {
            r = (r + dx).max(l + min.0);
        }
        if matches!(e, North | NorthWest | NorthEast) {
            t = (t + dy).min(b - min.1);
        }
        if matches!(e, South | SouthWest | SouthEast) {
            b = (b + dy).max(t + min.1);
        }
        (l, t, r, b)
    }
}

/// Border of the restored main window (spec ui/main-window.md §12), logical px: left, top, right, bottom.
const BORDER: (f32, f32, f32, f32) = (6.0, 2.0, 6.0, 6.0);
/// Depth of the resize zone along the top edge and the size of the corner squares (logical px).
const TOP_GRIP: f32 = 4.0;
const CORNER: f32 = 14.0;

/// The UI's scale (device pixels per logical one) on a monitor whose own is `system`: Settings > General > Display >
/// Main GUI scaling keeps it (System) or replaces it with a percentage (spec: ui/ui-scaling.md §1).
pub(crate) fn gui_scale(system: f64) -> f64 {
    let pct = prefs::choice(&prefs::MAIN_GUI_SCALING).strip_suffix('%').and_then(|p| p.parse::<f64>().ok());
    pct.map_or(system, |p| p / 100.0)
}

/// The main window's UI scale: its monitor's, or the one chosen (`gui_scale`). Read at every event and paint, so a new
/// choice applies at once, as a change of the monitor's scale does.
fn ui_scale(window: &Window) -> f64 {
    gui_scale(window.scale_factor())
}

/// Device pixels per unit of the window system's own window places: points on macOS, pixels elsewhere (`layout`).
fn place_k(scale: f64) -> f64 {
    if cfg!(target_os = "macos") { scale } else { 1.0 }
}

/// The main window's outer top-left and inner size in the window system's own units (`place_k`); `None` where a window
/// has no position (Wayland).
fn frame_of(window: &Window) -> Option<((i32, i32), (u32, u32))> {
    let (p, s, k) = (window.outer_position().ok()?, window.inner_size(), place_k(window.scale_factor()));
    let at = |v: i32| (f64::from(v) / k).round() as i32;
    let len = |v: u32| (f64::from(v) / k).round() as u32;
    Some(((at(p.x), at(p.y)), (len(s.width), len(s.height))))
}

/// The screens ([left, top, right, bottom]) in the window system's own units (`place_k`).
fn screens(el: &ActiveEventLoop) -> Vec<[i32; 4]> {
    el.available_monitors()
        .map(|m| {
            let (p, s, k) = (m.position(), m.size(), place_k(m.scale_factor()));
            let at = |v: f64| (v / k).round() as i32;
            let (x, y) = (at(f64::from(p.x)), at(f64::from(p.y)));
            [x, y, x + at(f64::from(s.width)), y + at(f64::from(s.height))]
        })
        .collect()
}

fn insets(window: &Window) -> (u32, u32, u32, u32) {
    if is_maximized(window) || window.fullscreen().is_some() {
        return (0, 0, 0, 0);
    }
    let k = ui_scale(window) as f32;
    let d = |v: f32| (v * k).round() as u32;
    (d(BORDER.0), d(BORDER.1), d(BORDER.2), d(BORDER.3))
}

/// Draw the border into the frame buffer around the canvas (§12): 1 px outline, a 1 px highlight
/// along the top, the frame, and a 1 px inner line at the left, right and bottom of the canvas.
fn draw_border(buf: &mut [u32], w: usize, h: usize, (l, t, r, b): (u32, u32, u32, u32)) {
    if l == 0 || t == 0 || w < 2 * (l + r) as usize || h < 2 * (t + b) as usize {
        return;
    }
    let (l, t, r, b) = (l as usize, t as usize, r as usize, b as usize);
    let mut rect = |x0: usize, y0: usize, x1: usize, y1: usize, c: u32| {
        for y in y0.min(h)..y1.min(h) {
            buf[y * w + x0.min(w)..y * w + x1.min(w)].fill(c);
        }
    };
    let (frame, outline, light, inner) = (0x2e3234, 0x202426, 0x393d3f, 0x2a2d2f);
    rect(0, 0, w, t, frame);
    rect(0, h - b, w, h, frame);
    rect(0, 0, l, h, frame);
    rect(w - r, 0, w, h, frame);
    if t > 1 {
        rect(1, 1, w - 1, 2, light);
    }
    rect(l - 1, t, l, h - b + 1, inner);
    rect(w - r, t, w - r + 1, h - b + 1, inner);
    rect(l - 1, h - b, w - r + 1, h - b + 1, inner);
    rect(0, 0, w, 1, outline);
    rect(0, h - 1, w, h, outline);
    rect(0, 0, 1, h, outline);
    rect(w - 1, 0, w, h, outline);
}

impl Shell {
    /// Holds the cursor for a drag that moves a value rather than the cursor (knobs, scroll-bar right-drags): hidden,
    /// with raw motion driving the virtual position `virt` (`device_event`).
    fn lock_cursor(&mut self, window: &Window, anchor: (f64, f64), virt: (f32, f32)) {
        self.lock = Some((anchor, virt));
        self.pointer_locked = window.set_cursor_grab(CursorGrabMode::Locked).is_ok();
        window.set_cursor_visible(false);
    }

    /// Ends a held drag: the cursor shows again where the drag started, as if it never moved.
    fn unlock_cursor(&mut self, window: &Window) {
        let Some((anchor, _)) = self.lock.take() else { return };
        // Under a pointer lock this is where the cursor reappears once unlocked.
        let _ = window.set_cursor_position(winit::dpi::PhysicalPosition::new(anchor.0, anchor.1));
        if std::mem::take(&mut self.pointer_locked) {
            let _ = window.set_cursor_grab(CursorGrabMode::None);
        }
        window.set_cursor_visible(true);
    }

    /// Show the cursor the app wants (resize arrows over edges and note ends).
    fn sync_cursor(&mut self, window: &Window) {
        let c = match self.edge {
            Some(ResizeDirection::East | ResizeDirection::West) => app::Cursor::EwResize,
            Some(ResizeDirection::North | ResizeDirection::South) => app::Cursor::NsResize,
            Some(ResizeDirection::NorthWest | ResizeDirection::SouthEast) => app::Cursor::NwseResize,
            Some(_) => app::Cursor::NeswResize,
            None => self.app.cursor(),
        };
        if c != self.cursor {
            self.cursor = c;
            window.set_cursor(match c {
                app::Cursor::Default => CursorIcon::Default,
                app::Cursor::EwResize => CursorIcon::EwResize,
                app::Cursor::NsResize => CursorIcon::NsResize,
                app::Cursor::NwseResize => CursorIcon::NwseResize,
                app::Cursor::NeswResize => CursorIcon::NeswResize,
            });
        }
    }

    /// The resize edge at physical position (x, y) of the restored window, if any.
    fn edge_at(&self, window: &Window, x: f64, y: f64) -> Option<ResizeDirection> {
        if self.inset == (0, 0, 0, 0) {
            return None;
        }
        let size = window.inner_size();
        let (w, h) = (f64::from(size.width), f64::from(size.height));
        let k = ui_scale(window);
        let (l, r, b) = (f64::from(self.inset.0), f64::from(self.inset.2), f64::from(self.inset.3));
        let t = f64::from(TOP_GRIP) * k;
        let c = f64::from(CORNER) * k;
        let (left, right, top, bottom) = (x < l, x >= w - r, y < t, y >= h - b);
        let (near_l, near_r, near_t, near_b) = (x < c, x >= w - c, y < c, y >= h - c);
        use ResizeDirection::*;
        Some(if (top && near_l) || (left && near_t) {
            NorthWest
        } else if (top && near_r) || (right && near_t) {
            NorthEast
        } else if (bottom && near_l) || (left && near_b) {
            SouthWest
        } else if (bottom && near_r) || (right && near_b) {
            SouthEast
        } else if left {
            West
        } else if right {
            East
        } else if top {
            North
        } else if bottom {
            South
        } else {
            return None;
        })
    }

    /// The app's modifier flags from the keyboard's.
    fn sync_mods(&mut self) {
        self.app.ctrl = ctrl(self.mods) && !(self.ctrl_click && !self.mods.super_key());
        self.app.shift = self.mods.shift_key();
        self.app.alt = self.mods.alt_key();
        self.app.rshift &= self.app.shift;
    }

    /// Logical app coordinates of a physical window position (the canvas sits inside the border).
    fn to_app(&self, window: &Window, x: f64, y: f64) -> (f32, f32) {
        let k = ui_scale(window);
        (((x - f64::from(self.inset.0)) / k) as f32, ((y - f64::from(self.inset.1)) / k) as f32)
    }

    /// Follows the main window for `layout`: its frame while it is neither maximized, minimized nor full screen, and
    /// whether it is maximized (as it was while minimized).
    fn follow_frame(&mut self, window: &Window) {
        if window.is_minimized() == Some(true) || window.fullscreen().is_some() {
            return;
        }
        let max = is_maximized(window);
        match frame_of(window) {
            Some((pos, size)) if !max => self.main_win = Some(layout::MainWin { pos, size, max }),
            _ => {
                if let Some(m) = &mut self.main_win {
                    m.max = max;
                }
            }
        }
    }

    /// Ticks the app, asks for a frame when one is due and sets the next wake-up: from `about_to_wait`, or from a
    /// watchdog wake while a modal loop holds up winit's (`held`, see `stall`).
    fn step(&mut self, el: &ActiveEventLoop, held: bool) {
        // Animate while playing; otherwise repaint a few times a second (peaks, clock).
        let ms = if self.app.animating() { 16 } else { 100 };
        let now = Instant::now();
        let mut wake = now + Duration::from_millis(ms);
        // Unsaved changes were answered (or there were none): the window closes.
        if self.app.want_exit {
            el.exit();
        }
        // macOS: Quit from the menu bar or the Dock asks about unsaved changes as closing the window does; AppKit waits
        // for the answer. A Quit while a dialog is open is turned down (the dialog is answered first).
        #[cfg(target_os = "macos")]
        {
            if macos::take_quit() {
                if self.app.modal_open() {
                    macos::answer_quit(false);
                } else {
                    self.app.ask_discard("closing", crate::dialog::Next::Quit);
                    self.pending = true;
                }
            }
            if let Some(quit) = self.app.quit_answer.take() {
                macos::answer_quit(quit);
            }
            // A project double-clicked in the Finder opens as a dropped one would.
            for path in macos::take_opened() {
                self.app.drop_file(path);
                self.pending = true;
            }
            // The menu bar: what was chosen in it, then its entries as they read now.
            for (menu, i) in menu_bar::take_picks() {
                // As a click on the window's own menu does (`App::mouse_down`): the whole canvas is painted again.
                self.app.invalidate();
                self.app.act(views::menus::pick(menu, i));
                self.pending = true;
            }
            menu_bar::update(views::menus::bar_model(&self.app.state));
        }
        self.pending |= self.app.tick();
        // The startup splash: the opened project's loading moves its gauge on; once the main window has shown and
        // nothing is loading, it finishes. It is dropped when it has faded away.
        if let Some(s) = &self.splash {
            match self.app.project_loading() {
                Some((p, what)) => s.step(splash::LOADING + (splash::LOADED - splash::LOADING) * p, &what),
                None if self.shown => s.finish(),
                None => {}
            }
            if s.gone() {
                self.splash = None;
            }
        }
        // Linux: a docked plugin window uncovered part of the main window, which needs presenting again.
        #[cfg(target_os = "linux")]
        {
            self.pending |= crate::plugin_window::exposed();
        }
        if let Some(log) = &mut self.frames {
            let took = now.elapsed().as_secs_f64() * 1000.0;
            if took > 2.0 {
                log.line(format_args!("tick {took:.2}"));
            }
            if held {
                log.line(format_args!("held"));
            }
        }
        // winit stops dispatching after each frame and comes back here: asking for the next frame at once would keep
        // our WM_PAINT pending forever while playing, and the plugin windows would never paint (see `FRAME`).
        if self.pending {
            let next = (self.frame_start + paced(self.frame, self.app.animating())).max(self.painted + IDLE);
            if now >= next {
                self.pending = false;
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            } else {
                wake = wake.min(next);
            }
        }
        el.set_control_flow(ControlFlow::WaitUntil(wake));
        self.watchdog.next(wake, held);
    }
}

impl ApplicationHandler for Shell {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        splash::step(&self.splash, splash::WINDOW, "Opening the main window");
        let attrs = Window::default_attributes()
            .with_title("Lemon Studio")
            .with_decorations(false)
            .with_inner_size(LogicalSize::new(1920.0, 1080.0))
            // Layouts are measured at 1920x1080 and are not responsive yet; below this they break.
            .with_min_inner_size(LogicalSize::new(1024.0, 600.0))
            .with_maximized(true);
        // macOS: the restored size (the window's size once un-maximized) fits the screen; a Mac's screen is often
        // smaller than 1920 x 1080 points, and AppKit would leave the window hanging off it.
        #[cfg(target_os = "macos")]
        let attrs = match el.primary_monitor() {
            Some(m) => {
                let screen = m.size().to_logical::<f64>(m.scale_factor());
                attrs.with_inner_size(LogicalSize::new((screen.width * 0.9).clamp(1024.0, 1920.0), (screen.height * 0.85).clamp(600.0, 1080.0)))
            }
            None => attrs,
        };
        // Where the last run left it (`layout`), its top on the screens there are now. macOS places it at its restored
        // frame and maximizes it once made (`set_maximized` keeps that frame to restore to).
        let frame = self.main_win.map(|m| layout::on_screens(m, &screens(el)));
        self.main_win = frame;
        let attrs = match frame {
            #[cfg(target_os = "macos")]
            Some(m) => attrs
                .with_position(winit::dpi::LogicalPosition::new(m.pos.0, m.pos.1))
                .with_inner_size(LogicalSize::new(m.size.0, m.size.1))
                .with_maximized(false),
            #[cfg(not(target_os = "macos"))]
            Some(m) => attrs
                .with_position(winit::dpi::PhysicalPosition::new(m.pos.0, m.pos.1))
                .with_inner_size(winit::dpi::PhysicalSize::new(m.size.0, m.size.1))
                .with_maximized(m.max),
            None => attrs,
        };
        // App icon: resource 1, embedded by build.rs from assets/lemonstudio.ico.
        #[cfg(windows)]
        let attrs = {
            use winit::dpi::PhysicalSize;
            use winit::platform::windows::{IconExtWindows, WindowAttributesExtWindows};
            use winit::window::Icon;
            let icon = |s: u32| Icon::from_resource(1, Some(PhysicalSize::new(s, s))).ok();
            attrs.with_window_icon(icon(16)).with_taskbar_icon(icon(32))
        };
        // Linux: the app id names the desktop entry (`lemonstudio.desktop`), where Wayland compositors find the icon;
        // X11 takes the icon from the window.
        #[cfg(all(unix, not(target_os = "macos")))]
        let attrs = {
            use winit::platform::wayland::WindowAttributesExtWayland;
            use winit::platform::x11::WindowAttributesExtX11;
            let attrs = WindowAttributesExtWayland::with_name(attrs, APP_ID, APP_ID);
            let attrs = WindowAttributesExtX11::with_name(attrs, APP_ID, APP_ID);
            attrs.with_window_icon(linux_icon())
        };
        let window = Rc::new(el.create_window(attrs).expect("create window"));
        #[cfg(target_os = "macos")]
        if frame.is_some_and(|m| m.max) {
            set_maximized(&window, true);
        }
        let context = softbuffer::Context::new(window.clone()).expect("graphics context");
        self.surface = Some(softbuffer::Surface::new(&context, window.clone()).expect("surface"));
        // Hosted plugins: windows are owned by the main window; scanning loads the .clap files found.
        #[cfg(windows)]
        {
            use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
            if let Ok(h) = window.window_handle() {
                if let RawWindowHandle::Win32(w) = h.as_raw() {
                    self.app.plugins.owner = w.hwnd.get();
                    // Docked plugin windows are children: our own painting must not draw over them.
                    use windows_sys::Win32::UI::WindowsAndMessaging::{GetWindowLongPtrW, SetWindowLongPtrW, GWL_STYLE, WS_CLIPCHILDREN};
                    let hwnd = w.hwnd.get() as windows_sys::Win32::Foundation::HWND;
                    // SAFETY: our own live window.
                    unsafe { SetWindowLongPtrW(hwnd, GWL_STYLE, GetWindowLongPtrW(hwnd, GWL_STYLE) | WS_CLIPCHILDREN as isize) };
                }
            }
        }
        // Under X11 plugin windows dock into the main window (under Wayland they cannot: they stay top-level).
        #[cfg(target_os = "linux")]
        {
            use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
            match window.window_handle().map(|h| h.as_raw()) {
                Ok(RawWindowHandle::Xlib(h)) => self.app.plugins.owner = h.window as isize,
                Ok(RawWindowHandle::Xcb(h)) => self.app.plugins.owner = h.window.get() as isize,
                _ => {}
            }
        }
        // macOS: plugin windows dock into the main window's view (winit's).
        #[cfg(target_os = "macos")]
        {
            use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
            if let Ok(RawWindowHandle::AppKit(h)) = window.window_handle().map(|h| h.as_raw()) {
                self.app.plugins.owner = h.ns_view.as_ptr() as isize;
            }
        }
        #[cfg(target_os = "macos")]
        menu_bar::install();
        self.app.plugins.scale = ui_scale(&window) as f32;
        // Built-in and cached plugins at once, then a background scan (spec: ui/plugin-manager.md §4).
        splash::step(&self.splash, splash::PLUGINS, "Loading plugins");
        self.app.start_plugins();
        splash::step(&self.splash, splash::LOADING, "Loading project");
        self.window = Some(window);
        el.set_control_flow(ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(16)));
    }

    fn user_event(&mut self, el: &ActiveEventLoop, _e: ()) {
        // A wake (see `wake_ui`): `about_to_wait` follows and ticks.
        WAKE_PENDING.store(false, std::sync::atomic::Ordering::Release);
        // Unless a modal loop (a plugin's menu, a file dialog) holds up winit's: then tick from here (see `stall`).
        if self.watchdog.take() {
            self.step(el, true);
        }
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        self.step(el, false);
    }

    /// The app closes: where the windows are is kept for the next start (`layout`). macOS: a quit the user agreed to
    /// (`macos::answer_quit`) ends the process from AppKit, after this: the plugin windows close and the instances go as
    /// they do when `run_app` returns.
    fn exiting(&mut self, _el: &ActiveEventLoop) {
        if let Some(window) = self.window.clone() {
            self.follow_frame(&window);
        }
        if let Some(m) = self.main_win {
            layout::save_main(m);
        }
        layout::save(&self.app.state);
        #[cfg(target_os = "macos")]
        {
            if os_host::guard::tripped() {
                return;
            }
            self.app.plugins.clear();
            plugins::sweep_retired();
        }
    }

    /// Raw mouse motion drives a locked scroll-bar drag; the hidden cursor is held at the anchor.
    fn device_event(&mut self, _el: &ActiveEventLoop, _id: DeviceId, event: DeviceEvent) {
        let (DeviceEvent::MouseMotion { delta }, Some((anchor, virt)), Some(window)) = (event, self.lock, self.window.clone()) else { return };
        let scale = ui_scale(&window);
        let v = (virt.0 + (delta.0 / scale) as f32, virt.1 + (delta.1 / scale) as f32);
        self.lock = Some((anchor, v));
        self.app.mouse_move(v.0, v.1);
        if !self.pointer_locked {
            let _ = window.set_cursor_position(winit::dpi::PhysicalPosition::new(anchor.0, anchor.1));
        }
        self.pending = true;
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(window) = self.window.clone() else { return };
        let scale = ui_scale(&window) as f32;
        // The monitor's own scale, for General ▸ Display's PPI ruler (`crate::ppi`).
        self.app.state.display_scale = window.scale_factor() as f32;
        // A project is loading (spec: ui/project-loading.md §2): presses, wheel turns and drops wait until it is done;
        // releases still go through so nothing stays held.
        let press = match &event {
            WindowEvent::MouseInput { state, .. } => *state == ElementState::Pressed,
            WindowEvent::KeyboardInput { event, .. } => event.state == ElementState::Pressed,
            WindowEvent::MouseWheel { .. } | WindowEvent::DroppedFile(_) => true,
            _ => false,
        };
        if press && self.app.loading() {
            return;
        }
        // Linux: a wheel turn over a docked plugin window is the plugin window's alone; nothing behind it scrolls
        // (`plugin_window::pointer_in_docked`).
        #[cfg(target_os = "linux")]
        if matches!(event, WindowEvent::MouseWheel { .. }) && crate::plugin_window::pointer_in_docked() {
            return;
        }
        // Linux and Windows: a click in the main window takes the keyboard back from a plugin editor that holds it (X11
        // leaves it there, and so does Windows for a docked editor, a child window of this one), so the shortcuts and
        // the typing keyboard work again (spec: ui/piano-keys.md §4 rule 2).
        #[cfg(any(target_os = "linux", windows))]
        if matches!(event, WindowEvent::MouseInput { state: ElementState::Pressed, .. }) {
            crate::plugin_window::focus_main();
        }
        if let Some(log) = &mut self.frames {
            let input = matches!(
                event,
                WindowEvent::CursorMoved { .. } | WindowEvent::MouseInput { .. } | WindowEvent::MouseWheel { .. } | WindowEvent::KeyboardInput { .. }
            );
            if input && log.input.is_none() {
                log.input = Some(Instant::now());
            }
        }
        // macOS: Control + click is a right click (the app's Ctrl is Command there, see `ctrl`).
        #[cfg(target_os = "macos")]
        let event = match event {
            WindowEvent::MouseInput { device_id, state, button: MouseButton::Left }
                if if state == ElementState::Pressed { self.mods.control_key() } else { self.ctrl_click } =>
            {
                self.ctrl_click = state == ElementState::Pressed;
                self.sync_mods();
                WindowEvent::MouseInput { device_id, state, button: MouseButton::Right }
            }
            e => e,
        };
        match event {
            WindowEvent::CloseRequested => {
                self.app.close_requested();
                if self.app.want_exit {
                    el.exit();
                }
                window.request_redraw();
            }
            WindowEvent::ModifiersChanged(m) => {
                self.mods = m.state();
                self.sync_mods();
            }
            WindowEvent::CursorMoved { position, .. } => {
                // Locked: turn the offset from the anchor into virtual movement, then warp back.
                // Locked: movement comes from raw motion (`device_event`); positions are ignored,
                // since hiding the cursor makes Windows report a jump to the window centre.
                if self.lock.is_some() {
                    return;
                }
                if let (Some((at, grab_y)), Some((cursor, (l, _, r, _)))) = (self.unmax, screen_state(&window)) {
                    if !past_drag_distance(at, cursor) {
                        return;
                    }
                    // Dragging a maximized window restores it under the cursor, then moves it.
                    self.unmax = None;
                    let frac = f64::from(at.0 - l) / f64::from((r - l).max(1));
                    set_maximized(&window, false);
                    if let Some((_, (nl, nt, nr, nb))) = screen_state(&window) {
                        let w = nr - nl;
                        let left = at.0 - (frac * f64::from(w)) as i32;
                        let rect = (left, at.1 - grab_y, left + w, at.1 - grab_y + (nb - nt));
                        set_rect(&window, rect);
                        self.grab = Some(Grab { cursor: at, rect, edge: None });
                    }
                }
                if let (Some(g), Some((cursor, _))) = (self.grab, screen_state(&window)) {
                    let k = window.scale_factor();
                    let min = ((1024.0 * k) as i32, (600.0 * k) as i32);
                    set_rect(&window, g.rect_at(cursor, min));
                    return;
                }
                self.edge = if self.app.drag.is_none() { self.edge_at(&window, position.x, position.y) } else { None };
                let (mx, my) = self.to_app(&window, position.x, position.y);
                self.app.mouse_move(mx, my);
                self.sync_cursor(&window);
                self.pending |= self.app.has_damage();
            }
            WindowEvent::MouseInput { state, button: MouseButton::Left, .. } => {
                if state == ElementState::Pressed {
                    if let Some(dir) = self.edge {
                        match screen_state(&window) {
                            Some((cursor, rect)) => self.grab = Some(Grab { cursor, rect, edge: Some(dir) }),
                            None => {
                                let _ = window.drag_resize_window(dir);
                            }
                        }
                        return;
                    }
                    let double = self.last_click.elapsed() < Duration::from_millis(350);
                    self.last_click = Instant::now();
                    self.app.mouse_down();
                    if self.app.cursor_locked() {
                        let (mx, my) = self.app.mouse;
                        let anchor = (f64::from(mx * scale) + f64::from(self.inset.0), f64::from(my * scale) + f64::from(self.inset.1));
                        self.lock_cursor(&window, anchor, (mx, my));
                    }
                    if self.app.want_drag_window {
                        self.app.want_drag_window = false;
                        if double {
                            set_maximized(&window, !is_maximized(&window));
                        } else if is_maximized(&window) {
                            // Dragging a maximized window restores it under the cursor, then moves it, once the cursor
                            // has travelled (`unmax`). Without screen coordinates (Linux) the window system's own move
                            // does both.
                            match screen_state(&window) {
                                Some((cursor, _)) => {
                                    let grab_y = (self.app.mouse.1 * scale) as i32 + self.inset.1 as i32;
                                    self.unmax = Some((cursor, grab_y));
                                }
                                None => {
                                    let _ = window.drag_window();
                                }
                            }
                        } else {
                            match screen_state(&window) {
                                Some((cursor, rect)) => self.grab = Some(Grab { cursor, rect, edge: None }),
                                None => {
                                    let _ = window.drag_window();
                                }
                            }
                        }
                    }
                } else {
                    self.grab = None;
                    self.unmax = None;
                    self.unlock_cursor(&window);
                    self.app.mouse_up();
                    if self.app.resize_cursor && self.app.drag.is_none() {
                        self.app.resize_cursor = false;
                    }
                    self.sync_cursor(&window);
                }
                self.pending = true;
            }
            WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Right, .. } => {
                self.app.mouse_down_right();
                if self.app.cursor_locked() {
                    let (mx, my) = self.app.mouse;
                    let anchor = (f64::from(mx * scale) + f64::from(self.inset.0), f64::from(my * scale) + f64::from(self.inset.1));
                    self.lock_cursor(&window, anchor, (mx, my));
                }
                self.pending = true;
            }
            WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Middle, .. } => {
                self.app.mouse_down_middle();
                self.pending = true;
            }
            WindowEvent::MouseInput { state: ElementState::Released, button: MouseButton::Right | MouseButton::Middle, .. } => {
                self.unlock_cursor(&window);
                self.app.mouse_up();
                self.pending = true;
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let (dx, dy) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => (x, y),
                    MouseScrollDelta::PixelDelta(p) => (p.x as f32 / 40.0, p.y as f32 / 40.0),
                };
                self.app.wheel(dx, dy, self.mods.shift_key(), ctrl(self.mods));
                // A wheel step in a menu puts the cursor on the row it lit.
                if let Some((x, y)) = self.app.warp.take() {
                    let k = ui_scale(&window);
                    let at = (f64::from(x) * k + f64::from(self.inset.0), f64::from(y) * k + f64::from(self.inset.1));
                    let _ = window.set_cursor_position(winit::dpi::PhysicalPosition::new(at.0, at.1));
                }
                self.pending = true;
            }
            WindowEvent::KeyboardInput { event, .. } => {
                let pressed = event.state == ElementState::Pressed;
                // Right Shift on its own: the playlist's quick slice (spec: ui/playlist.md §3).
                if event.physical_key == winit::keyboard::PhysicalKey::Code(winit::keyboard::KeyCode::ShiftRight) {
                    self.app.rshift = pressed;
                }
                // The typing keyboard plays notes by physical key (spec: ui/typing-keyboard.md).
                if let winit::keyboard::PhysicalKey::Code(code) = event.physical_key {
                    if self.app.typing_key(code, pressed, ctrl(self.mods), self.mods.alt_key()) {
                        if self.app.has_damage() {
                            window.request_redraw();
                        }
                        return;
                    }
                }
                // Repeats go on too: a dialog, an Info box or a browser folder cell takes them (typing, moving through a
                // list), and `App::key` keeps those of the piano roll's moving and copying keys (spec: ui/piano-keys.md
                // §4 rule 1).
                if pressed {
                    // macOS: Option and Command change the character a key types (Option+S is "ß"); shortcuts go by the
                    // key itself.
                    #[cfg(target_os = "macos")]
                    let event = {
                        use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
                        let mut event = event;
                        if self.mods.alt_key() || self.mods.super_key() {
                            event.logical_key = match event.key_without_modifiers() {
                                winit::keyboard::Key::Character(c) if self.mods.shift_key() => winit::keyboard::Key::Character(c.to_uppercase().into()),
                                k => k,
                            };
                        }
                        event
                    };
                    // Windows: Ctrl+Alt is AltGr there, so a key that types nothing with it (Ctrl+Alt+Z, the redo) comes
                    // with no character; shortcuts go by the key itself. A character AltGr does type is kept. Text sent
                    // as characters rather than keys (an on-screen keyboard, a remote desktop, the emoji panel) comes as
                    // an unidentified key carrying its text: it types that text.
                    #[cfg(windows)]
                    let event = {
                        use winit::keyboard::Key;
                        use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
                        let mut event = event;
                        if matches!(event.logical_key, Key::Unidentified(_)) {
                            event.logical_key = match (event.key_without_modifiers(), &event.text) {
                                (Key::Character(c), _) if self.mods.shift_key() => Key::Character(c.to_uppercase().into()),
                                (Key::Character(c), _) => Key::Character(c),
                                (_, Some(t)) if !t.chars().any(char::is_control) => Key::Character(t.clone()),
                                (k, _) => k,
                            };
                        }
                        event
                    };
                    self.app.repeat = event.repeat;
                    self.app.key(&event.logical_key, ctrl(self.mods), self.mods.alt_key());
                    self.app.repeat = false;
                    window.request_redraw();
                }
            }
            // Audio files dropped on the playlist become audio clips (spec: ui/playlist.md §9); a dropped project opens.
            WindowEvent::DroppedFile(path) => {
                // No mouse moves arrive during a drag and drop: take the cursor position now.
                let at = screen_state(&window).map(|s| s.0);
                // X11 main window (Linux): the X server knows where the pointer is.
                #[cfg(target_os = "linux")]
                let at = at.or_else(|| (self.app.plugins.owner != 0).then(crate::plugin_window::pointer).flatten());
                if let (Some((cx, cy)), Ok(p)) = (at, window.inner_position()) {
                    let (mx, my) = self.to_app(&window, f64::from(cx - p.x), f64::from(cy - p.y));
                    self.app.mouse_move(mx, my);
                }
                self.app.drop_file(path);
                window.request_redraw();
            }
            WindowEvent::HoveredFile(_) => {}
            WindowEvent::Focused(focused) => {
                // Linux: plugin windows stay above the main window while Lemon Studio is the active application.
                #[cfg(target_os = "linux")]
                crate::plugin_window::main_focused(focused);
                // The project's working time counts while the main window is in front (`project_info::WorkClock`).
                self.app.main_focused(focused);
                // Audio ▸ Auto close, where the system can't tell which program is in front (`auto_close`).
                self.app.auto_close.main_focused = focused;
                if !focused {
                    self.grab = None;
                    self.unmax = None;
                    self.app.typing_release_all();
                    window.request_redraw();
                }
            }
            // Windows display scale changed, or the window moved to a monitor with another DPI: the canvas follows at
            // the next paint; docked plugin windows are redrawn at the new scale (spec: ui/ui-scaling.md §3).
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                self.app.plugins.set_scale(gui_scale(scale_factor) as f32);
                self.inset = insets(&window);
                self.app.invalidate();
                window.request_redraw();
            }
            WindowEvent::Resized(_) => {
                self.follow_frame(&window);
                self.inset = insets(&window);
                self.app.invalidate();
                window.request_redraw();
            }
            WindowEvent::Moved(_) => self.follow_frame(&window),
            WindowEvent::RedrawRequested => {
                let size = window.inner_size();
                let (Some(w), Some(h)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) else { return };
                let inset = insets(&window);
                if inset != self.inset {
                    self.inset = inset;
                    self.app.invalidate();
                }
                // The canvas fills the window inside the border.
                let cw = size.width.saturating_sub(inset.0 + inset.2).max(1);
                let ch = size.height.saturating_sub(inset.1 + inset.3).max(1);
                let off = (inset.0 as usize, inset.1 as usize);
                let stride = size.width as usize;
                if self.canvas.is_none() {
                    match Canvas::new(cw, ch, scale) {
                        Ok(c) => self.canvas = Some(c),
                        Err(e) => {
                            eprintln!("lemonstudio: {e}");
                            el.exit();
                            return;
                        }
                    }
                }
                let canvas = self.canvas.as_mut().expect("canvas");
                if canvas.pixmap.width() != cw || canvas.pixmap.height() != ch || canvas.scale != scale {
                    canvas.resize(cw, ch, scale);
                    self.app.invalidate();
                    // Main GUI scaling changed: docked plugin windows follow (spec: ui/ui-scaling.md §3).
                    self.app.plugins.set_scale(scale);
                }
                // The window may have reached a monitor with another refresh rate.
                let monitor = window.current_monitor();
                if monitor != self.frame_monitor {
                    self.frame = frame_interval(monitor.as_ref().and_then(|m| m.refresh_rate_millihertz()));
                    self.frame_monitor = monitor;
                }
                // Paint only what changed, then present only that region.
                let damage = self.app.take_damage();
                let t_paint = Instant::now();
                if damage.is_some() {
                    self.frame_start = t_paint;
                }
                if let Some(d) = &damage {
                    self.app.canvas_off = (self.inset.0 as i32, self.inset.1 as i32);
                    self.app.paint_region(canvas, d.as_deref());
                    #[cfg(plugin_windows)]
                    self.app.sync_lift(canvas);
                }
                // A scrolled playlist moved its pixels instead of repainting them: the buffer moves them too.
                let moved = self.app.take_moved();
                let t_present = Instant::now();
                if let Some(surface) = self.surface.as_mut() {
                    if surface.resize(w, h).is_ok() {
                        if let Ok(mut buf) = surface.buffer_mut() {
                            // The buffer holds the last frame only when its age is 1. Wayland hands out two or
                            // more buffers in turn (age 2 and up): such a buffer lacks what was painted since
                            // it was last shown, so it gets the whole canvas.
                            let age = buf.age();
                            // A docked plugin window moved: what it uncovered needs the whole canvas.
                            #[cfg(plugin_windows)]
                            let exposed = crate::plugin_window::take_exposed();
                            #[cfg(not(plugin_windows))]
                            let exposed = false;
                            match &damage {
                                Some(Some(rects)) if age == 1 && !exposed => {
                                    let mut present = Vec::with_capacity(rects.len() + moved.len());
                                    let mut show = |(x, y, w, h): (usize, usize, usize, usize)| {
                                        if let (Some(width), Some(height)) = (NonZeroU32::new(w as u32), NonZeroU32::new(h as u32)) {
                                            present.push(softbuffer::Rect { x: (x + off.0) as u32, y: (y + off.1) as u32, width, height });
                                        }
                                    };
                                    // The buffer holds the canvas as it was before this paint: moving the same pixels
                                    // keeps them equal, then the damage is copied over.
                                    let mut areas = Vec::with_capacity(moved.len());
                                    let (ox, oy) = (off.0 as i32, off.1 as i32);
                                    let at = |(x0, y0, x1, y1): (i32, i32, i32, i32)| (x0 + ox, y0 + oy, x1 + ox, y1 + oy);
                                    for m in &moved {
                                        let (x0, y0, x1, y1) = os_ui::shift_area(m.area, canvas.scale, (cw, ch));
                                        let keep: Vec<_> = m.keep.iter().map(|k| at(os_ui::keep_area(*k, canvas.scale, (cw, ch)))).collect();
                                        os_ui::shift_pixels(&mut buf[..], stride, at((x0, y0, x1, y1)), m.dx, m.dy, &keep);
                                        let a = (x0 as usize, y0 as usize, (x1 - x0).max(0) as usize, (y1 - y0).max(0) as usize);
                                        areas.push(a);
                                        show(a);
                                    }
                                    let inside_moved = |p: (usize, usize, usize, usize)| {
                                        areas.iter().any(|a| a.0 <= p.0 && a.1 <= p.1 && p.0 + p.2 <= a.0 + a.2 && p.1 + p.3 <= a.1 + a.3)
                                    };
                                    for r in rects {
                                        let px = canvas.device_rect(*r);
                                        canvas.copy_region_to_xrgb_at(&mut buf, stride, px, off);
                                        if !inside_moved(px) {
                                            show(px);
                                        }
                                    }
                                    let _ = if present.is_empty() { buf.present() } else { buf.present_with_damage(&present) };
                                }
                                // Nothing painted (the OS asked for a redraw): the buffer already
                                // holds the last frame unless it was discarded.
                                // Linux: an uncovered part of the main window shows nothing until presented again.
                                None => {
                                    if age != 1 || (cfg!(target_os = "linux") && exposed) {
                                        canvas.copy_region_to_xrgb_at(&mut buf, stride, (0, 0, cw as usize, ch as usize), off);
                                        draw_border(&mut buf, stride, size.height as usize, inset);
                                    }
                                    let _ = buf.present();
                                }
                                _ => {
                                    canvas.copy_region_to_xrgb_at(&mut buf, stride, (0, 0, cw as usize, ch as usize), off);
                                    draw_border(&mut buf, stride, size.height as usize, inset);
                                    let _ = buf.present();
                                }
                            }
                        }
                    }
                }
                self.painted = Instant::now();
                // The main window shows its first frame: the splash finishes once the project has loaded (`step`).
                self.shown = true;
                if let Some(log) = &mut self.frames {
                    let what = match &damage {
                        None => "none".to_string(),
                        Some(None) => format!("full {}", u64::from(cw) * u64::from(ch)),
                        Some(Some(rs)) => {
                            let px: usize = rs.iter().map(|r| canvas.device_rect(*r)).map(|(_, _, w, h)| w * h).sum();
                            let blit = if moved.is_empty() { String::new() } else { format!(" blit{}", moved.len()) };
                            format!("rects{} {px}{blit}", rs.len())
                        }
                    };
                    let ms = |a: Instant, b: Instant| b.duration_since(a).as_secs_f64() * 1000.0;
                    let wait = log.input.take().map_or(-1.0, |i| ms(i, self.painted));
                    log.line(format_args!("frame {what} paint {:.2} present {:.2} input {wait:.1}", ms(t_paint, t_present), ms(t_present, self.painted)));
                }
                if self.app.want_minimize {
                    self.app.want_minimize = false;
                    window.set_minimized(true);
                }
                if self.app.want_maximize {
                    self.app.want_maximize = false;
                    set_maximized(&window, !is_maximized(&window));
                }
                if self.app.want_close {
                    self.app.want_close = false;
                    if !self.app.modal_open() {
                        self.app.ask_discard("closing", crate::dialog::Next::Close);
                    }
                    if self.app.want_exit {
                        el.exit();
                    } else {
                        window.request_redraw();
                    }
                }
            }
            _ => {}
        }
    }
}

/// The application id on Linux: Wayland's app id and X11's WM_CLASS, matching `assets/lemonstudio.desktop`.
#[cfg(all(unix, not(target_os = "macos")))]
const APP_ID: &str = "lemonstudio";

/// The app icon as RGBA bytes, width and height: `assets/lemonstudio.png`, 256 px (X11 windows show it; Wayland takes the
/// desktop entry's).
#[cfg(all(unix, not(target_os = "macos")))]
pub(crate) fn icon_rgba() -> Option<(Vec<u8>, u32, u32)> {
    let png = tiny_skia::Pixmap::decode_png(include_bytes!("../assets/lemonstudio.png")).ok()?;
    let rgba = png.pixels().iter().flat_map(|p| {
        let c = p.demultiply();
        [c.red(), c.green(), c.blue(), c.alpha()]
    });
    Some((rgba.collect(), png.width(), png.height()))
}

/// The main window's icon.
#[cfg(all(unix, not(target_os = "macos")))]
fn linux_icon() -> Option<winit::window::Icon> {
    let (rgba, w, h) = icon_rgba()?;
    winit::window::Icon::from_rgba(rgba, w, h).ok()
}

/// `lemonstudio --screenshot out.png [--view playlist|mixer|piano|none] [--pattern N] [--channel I] [--no-rack] [--browser-tab N] [--browser-width PX] [--browser-menu] [--browser-search TEXT] [--browser-folder DIR[|SUB]] [project.flp]`:
/// paint one 1920×1080 frame offscreen (used to compare with reference measurements, and for the README's screenshots
/// with `--float`, `--rack-pos`, `--rack-all`, `--rack-scroll` and `--insert`).
fn screenshot(args: &[String]) {
    let mut out = None;
    let mut view = "playlist".to_string();
    let mut rack = true;
    let mut browser = true;
    let mut rack_w: Option<f32> = None;
    let mut rack_roll = false;
    let mut browser_tab: Option<usize> = None;
    let mut browser_w: Option<f32> = None;
    let mut browser_menu = false;
    let mut browser_search: Option<String> = None;
    let mut browser_folder: Option<String> = None;
    let mut menu: Option<usize> = None;
    let mut plugin_menu: Option<plugin_menu::Target> = None;
    let mut import_midi: Option<(String, bool)> = None;
    let mut settings: Option<usize> = None;
    let mut info_edit: Option<usize> = None;
    let mut clicks: Vec<(f32, f32, bool)> = Vec::new();
    let mut wheels: Vec<(f32, f32, f32)> = Vec::new();
    let mut lane: Option<usize> = None;
    let mut lane_menu = false;
    let mut pans: Option<Vec<f32>> = None;
    let mut path = None;
    let mut mouse: Option<(f32, f32)> = None;
    let mut pattern = None;
    let mut channel = None;
    let mut win_rect: Option<os_ui::Rect> = None;
    let mut theme: Option<std::path::PathBuf> = None;
    let mut design: Option<os_theme::Design> = None;
    let mut scale = 1.0f32;
    let mut size: Option<(u32, u32)> = None;
    let mut bar_w: Option<f32> = None;
    let mut wait = false;
    let mut perf = false;
    let mut cpu_view: Option<String> = None;
    let mut floats: Vec<(app::Win, os_ui::Rect)> = Vec::new();
    let mut rack_pos: Option<(f32, f32)> = None;
    let mut insert: Option<u16> = None;
    let mut rack_h: Option<f32> = None;
    let mut rack_scroll: Option<f32> = None;
    let mut rack_all = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--screenshot" => out = it.next().cloned(),
            "--theme" => theme = it.next().map(std::path::PathBuf::from),
            // "classic" / "lemon" (spec: ui/lemon-design.md); default: the saved design.
            "--design" => design = it.next().and_then(|v| os_theme::Design::parse(v)),
            // Device pixels per logical one (the frame stays 1920x1080 logical): 1.5 = Windows at 150 %.
            "--scale" => scale = it.next().and_then(|v| v.parse().ok()).filter(|s: &f32| *s >= 0.5 && *s <= 4.0).unwrap_or(1.0),
            "--view" => view = it.next().cloned().unwrap_or_default(),
            "--no-rack" => rack = false,
            "--no-browser" => browser = false,
            "--rack-roll" => rack_roll = true,
            "--browser-menu" => browser_menu = true,
            "--browser-search" => browser_search = it.next().cloned(),
            "--browser-folder" => browser_folder = it.next().cloned(),
            "--browser-tab" => browser_tab = it.next().and_then(|v| v.parse().ok()),
            // The browser's width incl. its splitter, as dragged (spec: ui/browser.md §9).
            "--browser-width" => browser_w = it.next().and_then(|v| v.parse().ok()),
            // "all" / "gen" / "fx": that plugin menu at the window's centre (spec: ui/plugin-menu.md §5).
            "--plugin-menu" => {
                plugin_menu = Some(match it.next().map(String::as_str) {
                    Some("gen") => plugin_menu::Target::Gen(None),
                    Some("fx") => plugin_menu::Target::Fx(None),
                    _ => plugin_menu::Target::All,
                })
            }
            // A MIDI file through FILE ▸ Import ▸ MIDI file (`--import-midi-piano`: the piano roll's import): the Import
            // MIDI data form, shown every time (spec: ui/midi-files.md §2).
            a @ ("--import-midi" | "--import-midi-piano") => import_midi = it.next().map(|p| (p.clone(), a == "--import-midi-piano")),
            // Hover at x,y before the frame is captured.
            "--mouse" => {
                let v: Vec<f32> = it.next().map(|v| v.split(',').filter_map(|n| n.parse().ok()).collect()).unwrap_or_default();
                if let [x, y] = v[..] {
                    mouse = Some((x, y));
                }
            }
            // Click at x,y (left button; `--rclick`: the right one) before the frame is captured; may be given more than
            // once, done in order.
            a @ ("--click" | "--rclick") => {
                let right = a == "--rclick";
                let v: Vec<f32> = it.next().map(|v| v.split(',').filter_map(|n| n.parse().ok()).collect()).unwrap_or_default();
                if let [x, y] = v[..] {
                    clicks.push((x, y, right));
                }
            }
            // The wheel turned `dy` notches at x,y after the first paint, before the clicks (scrolls a page, e.g.
            // `--settings 5 --wheel 960,500,-40` shows the Theme page's buttons); may be given more than once.
            "--wheel" => {
                let v: Vec<f32> = it.next().map(|v| v.split(',').filter_map(|n| n.parse().ok()).collect()).unwrap_or_default();
                if let [x, y, dy] = v[..] {
                    wheels.push((x, y, dy));
                }
            }
            "--lane" => lane = it.next().and_then(|v| v.parse().ok()),
            "--pans" => pans = it.next().map(|v| v.split(',').filter_map(|p| p.parse().ok()).collect()),
            "--settings" => settings = it.next().and_then(|v| v.parse().ok()),
            // With --settings 7: that Info box (0 title .. 4 web link) being edited.
            "--info-edit" => info_edit = it.next().and_then(|v| v.parse().ok()),
            "--lane-menu" => lane_menu = true,
            "--menu" => menu = it.next().and_then(|v| v.parse().ok()),
            "--rack-width" => rack_w = it.next().and_then(|v| v.parse().ok()),
            "--pattern" => pattern = it.next().and_then(|v| v.parse().ok()),
            "--channel" => channel = it.next().and_then(|v| v.parse().ok()),
            // Physical canvas size "WxH" (default 1920x1080 times the scale), e.g. FL's window on a 150 % display:
            // `--size 2560x1540 --scale 1.5`.
            "--size" => {
                if let Some((w, h)) = it.next().and_then(|v| v.split_once('x')) {
                    size = w.parse().ok().zip(h.parse().ok());
                }
            }
            // Playlist zoom: logical pixels per bar.
            "--bar-w" => bar_w = it.next().and_then(|v| v.parse().ok()),
            // Let the project finish loading (samples decoded, samplers started) before the frame is captured.
            "--wait" => wait = true,
            // The Plugin performance monitor open; the CPU panel's view ("fps" or "misc").
            "--perf" => perf = true,
            "--cpu" => cpu_view = it.next().cloned(),
            // Float the shown window at x,y,w,h instead of maximizing it.
            "--win" => {
                let v: Vec<f32> = it.next().map(|v| v.split(',').filter_map(|n| n.parse().ok()).collect()).unwrap_or_default();
                if let [x, y, w, h] = v[..] {
                    win_rect = Some(os_ui::Rect::new(x, y, w, h));
                }
            }
            // "--float piano:x,y,w,h": another window floated over the main view (playlist, mixer or piano).
            "--float" => {
                if let Some((name, r)) = it.next().and_then(|v| v.split_once(':')) {
                    let win = match name {
                        "playlist" => Some(app::Win::Playlist),
                        "mixer" => Some(app::Win::Mixer),
                        "piano" => Some(app::Win::PianoRoll),
                        _ => None,
                    };
                    let v: Vec<f32> = r.split(',').filter_map(|n| n.parse().ok()).collect();
                    if let (Some(win), [x, y, w, h]) = (win, &v[..]) {
                        floats.push((win, os_ui::Rect::new(*x, *y, *w, *h)));
                    }
                }
            }
            // "--rack-pos x,y[,w,h]": the channel rack moved there, and sized.
            "--rack-pos" => {
                let v: Vec<f32> = it.next().map(|v| v.split(',').filter_map(|n| n.parse().ok()).collect()).unwrap_or_default();
                if let [x, y, ..] = v[..] {
                    rack_pos = Some((x, y));
                }
                if let [_, _, w, h] = v[..] {
                    rack_w = Some(w);
                    rack_h = Some(h);
                }
            }
            // The channel rack shows every channel (not the group the project saved).
            "--rack-all" => rack_all = true,
            // Channel rack scrolled down by this many rows.
            "--rack-scroll" => rack_scroll = it.next().and_then(|v| v.parse().ok()),
            // The selected mixer insert (its effect slots fill the side panel).
            "--insert" => insert = it.next().and_then(|v| v.parse().ok()),
            other => path = Some(std::path::PathBuf::from(other)),
        }
    }
    let mut app = App::new(path);
    if let Some(i) = insert {
        app.state.selected_mixer = i;
        // Scrolled so the selected insert sits in the right half of the strips.
        app.state.mixer_scroll = (f32::from(i.saturating_sub(18)) * views::mixer::STRIP_W).max(0.0);
    }
    if let Some(d) = design {
        app.theme = app::design_theme(d, true);
        os_theme::set_current(&app.theme.colors);
    }
    match theme {
        Some(t) if t.as_os_str() == "default" => {
            app.theme = os_theme::Theme::default();
            os_theme::set_current(&app.theme.colors);
        }
        Some(t) => app.load_theme_file_ex(&t, false),
        None => {}
    }
    if let Some(p) = pattern {
        app.state.pattern = p;
    }
    if let Some(c) = channel {
        app.state.selected_channel = c;
    }
    app.state.show_browser = browser;
    if let Some(t) = browser_tab {
        app.state.browser_tab = t.min(3);
    }
    if let Some(w) = browser_w {
        app.state.browser_w = w;
    }
    // A dropped folder (spec: ui/browser.md §6), opened; not remembered. `--browser-folder DIR[|sub/path]` opens a subfolder and selects it.
    if let Some(f) = browser_folder {
        let (dir, sub) = f.split_once('|').unwrap_or((&f, ""));
        let dir = std::path::PathBuf::from(dir);
        let mut key = views::browser::disk_key(&dir);
        app.state.browser_open.insert(key.clone());
        for part in sub.split('/').filter(|p| !p.is_empty()) {
            key = format!("{key}/{part}");
            app.state.browser_open.insert(key.clone());
        }
        app.state.browser_sel = Some(key);
        app.state.browser_folders.push(dir);
    }
    if let Some(q) = browser_search {
        app.state.browser_search = q;
        app.state.browser_search_focus = true;
    }
    if browser_menu {
        app.act(app::Action::BrowserMenu);
    }
    app.state.z = match view.as_str() {
        "mixer" => vec![app::Win::Mixer],
        "piano" => vec![app::Win::PianoRoll],
        "none" => vec![],
        _ => vec![app::Win::Playlist],
    };
    for (w, _) in &floats {
        if !app.state.z.contains(w) {
            app.state.z.push(*w);
        }
    }
    if let Some(p) = rack_pos {
        app.state.rack_pos = p;
    }
    if rack {
        app.state.z.push(app::Win::ChannelRack);
    }
    app.state.rack_roll_view = rack_roll;
    app.state.menu_open = menu;
    // The built-in and cached plugins (no scan).
    app.load_plugins();
    if let Some(t) = settings {
        app.open_settings(t);
    }
    if let Some(i) = info_edit {
        use views::settings::InfoField as F;
        app.info_act([F::Title, F::Author, F::Genre, F::Comments, F::Url][i.min(4)]);
    }
    app.state.lane_menu = lane_menu;
    // "--pans 0.2,0.8": mixer pan knob values for inserts 1.. (normalised), to check the value arcs.
    for (i, v) in pans.iter().flatten().enumerate() {
        app.state.set_knob(knobs::KnobId::MixPan(i as u16 + 1), *v);
    }
    if let Some(i) = lane {
        app.state.piano.lane = piano::Lane::ALL[i.min(piano::Lane::ALL.len() - 1)];
    }
    if let Some(target) = plugin_menu {
        let (w, h) = app.state.screen;
        app.open_plugin_menu(target, ((w / 2.0).round(), (h / 3.0).round()));
    }
    if let Some((p, piano)) = import_midi {
        // Alt: the MIDI import window's "Show every time" row as it comes.
        app.alt = true;
        app.midi_file_chosen(p.into(), piano);
        app.alt = false;
    }
    if let Some(w) = rack_w {
        app.state.rack_size.0 = w;
    }
    if let Some(h) = rack_h {
        app.state.rack_size.1 = h;
    }
    if rack_all {
        app.state.rack.filter = rack::Filter::All;
    }
    if let Some(r) = rack_scroll {
        app.state.rack_scroll = r * views::rack::ROW_H;
    }
    // The reference views fill the workspace unless `--win` floats the window.
    if let Some(w) = app.state.z.first().copied().filter(|w| w.slot().is_some()) {
        let ws = &mut app.state.wins[w.slot().unwrap_or(0)];
        match win_rect {
            Some(r) => {
                ws.rect = r;
                ws.maxed = false;
            }
            None => ws.maxed = true,
        }
    }
    let mut keys_h = 744.0;
    for (w, r) in &floats {
        if let Some(slot) = w.slot() {
            let ws = &mut app.state.wins[slot];
            ws.rect = *r;
            ws.maxed = false;
            ws.mini = false;
        }
        if *w == app::Win::PianoRoll {
            // The note grid: the window less its title, toolbar, ruler and the control lane.
            keys_h = (r.h - 280.0).max(100.0);
        }
    }
    if app.state.shown(app::Win::PianoRoll) {
        piano::center_keys(&mut app.state, keys_h);
    }
    if let Some(w) = bar_w {
        app.state.playlist_bar_w = w;
    }
    let started = Instant::now();
    while wait && app.loading() && started.elapsed().as_secs() < 120 {
        app.tick();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    if perf {
        app.act(app::Action::ToggleWin(app::Win::Perf));
    }
    if let Some(v) = cpu_view {
        // Set without keeping them (`cpu_panel.txt` stays as the user left it).
        app.state.cpu.fps_view = v == "fps";
        app.state.cpu.misc = v == "misc";
        app.state.cpu.update(Instant::now(), None, 0, 0);
    }
    // No "Opened <path>" in the title: the frame shows the project, not where it lives on disk.
    app.state.message = None;
    let (w, h) = size.unwrap_or(((1920.0 * scale).round() as u32, (1080.0 * scale).round() as u32));
    let mut canvas = Canvas::new(w, h, scale).expect("canvas");
    app.paint(&mut canvas, None);
    for (x, y, dy) in wheels {
        app.mouse_move(x, y);
        app.wheel(0.0, dy, false, false);
        app.paint(&mut canvas, None);
    }
    for (x, y, right) in clicks {
        app.mouse_move(x, y);
        if right {
            app.mouse_down_right();
        } else {
            app.mouse_down();
        }
        app.mouse_up();
        app.paint(&mut canvas, None);
    }
    if let Some((x, y)) = mouse {
        app.mouse_move(x, y);
        app.paint(&mut canvas, None);
    }
    if std::env::var_os("OS_BENCH").is_some() {
        let t0 = Instant::now();
        for _ in 0..20 {
            app.paint(&mut canvas, None);
        }
        eprintln!("paint: {:.2} ms/frame", t0.elapsed().as_secs_f64() * 50.0);
        // Copying the whole frame into the window's buffer (a full present).
        let (cw, ch) = (canvas.pixmap.width() as usize, canvas.pixmap.height() as usize);
        let mut buf = vec![0u32; cw * ch];
        let tc = Instant::now();
        for _ in 0..20 {
            canvas.copy_region_to_xrgb(&mut buf, (0, 0, cw, ch));
        }
        eprintln!("  full present copy: {:.2} ms/frame", tc.elapsed().as_secs_f64() * 50.0);
        for (name, r) in [
            ("rack-sized damage", os_ui::Rect::new(317.0, 195.0, 510.0, 260.0)),
            ("fader-column damage", os_ui::Rect::new(317.0, 500.0, 50.0, 500.0)),
            ("hover-button damage", os_ui::Rect::new(975.0, 45.0, 36.0, 36.0)),
        ] {
            let t1 = Instant::now();
            for _ in 0..20 {
                app.paint(&mut canvas, Some(r));
            }
            eprintln!("  {name}: {:.2} ms/frame", t1.elapsed().as_secs_f64() * 50.0);
        }
        // Playback: what each tick repaints (spec: audio/transport.md; no device, the shown state is set directly).
        // About 4 ticks per frame = 60 frames a second at 140 BPM, 96 PPQ.
        for song in [false, true] {
            let shown = |tick: f64, sounding: Vec<(u16, u16)>| app::Shown { playing: true, paused: false, tick, song, sounding, tempo: None };
            app.show_playback(shown(0.0, Vec::new()));
            let d = app.take_damage();
            app.paint_region(&mut canvas, d.flatten().as_deref());
            let n = 120;
            let t2 = Instant::now();
            for i in 1..=n {
                let sounding = if (i / 6) % 2 == 0 { vec![(0, 60)] } else { Vec::new() };
                app.show_playback(shown(f64::from(i) * 4.0, sounding));
                if let Some(d) = app.take_damage() {
                    app.paint_region(&mut canvas, d.as_deref());
                }
            }
            eprintln!("  playing ({}): {:.2} ms/frame", if song { "song" } else { "pattern" }, t2.elapsed().as_secs_f64() * 1000.0 / f64::from(n));
        }
        app.show_playback(app::Shown::default());
        app.paint(&mut canvas, None);
        // Scrolling the playlist by wheel steps (60 px sideways, a track down, then back): its window repainted, or its
        // drawn pixels moved (`App::playlist_scrolled`). Paint, then the window's buffer: moved like the canvas, damage copied.
        if app.state.shown(app::Win::Playlist) {
            let row = app.state.playlist.row_h;
            for (name, step) in [("sideways", (60.0, 0.0)), ("down", (0.0, row))] {
                for moved in [false, true] {
                    let (mut paint, mut copy) = (0.0, 0.0);
                    for i in 0..20 {
                        let before = app.state.playlist_scroll;
                        let k = if i < 10 { 1.0 } else { -1.0 };
                        app.state.playlist_scroll.0 += k * step.0;
                        app.state.playlist_scroll.1 += k * step.1;
                        if moved {
                            app.playlist_scrolled(before);
                        } else {
                            app.damage_win(app::Win::Playlist);
                        }
                        let t3 = Instant::now();
                        let Some(d) = app.take_damage() else { continue };
                        app.paint_region(&mut canvas, d.as_deref());
                        let t4 = Instant::now();
                        for m in app.take_moved() {
                            let size = (cw as u32, ch as u32);
                            let keep: Vec<_> = m.keep.iter().map(|k| os_ui::keep_area(*k, canvas.scale, size)).collect();
                            os_ui::shift_pixels(&mut buf[..], cw, os_ui::shift_area(m.area, canvas.scale, size), m.dx, m.dy, &keep);
                        }
                        for r in d.into_iter().flatten() {
                            canvas.copy_region_to_xrgb(&mut buf, canvas.device_rect(r));
                        }
                        paint += t4.duration_since(t3).as_secs_f64();
                        copy += t4.elapsed().as_secs_f64();
                    }
                    let how = if moved { "moved" } else { "repainted" };
                    eprintln!("  playlist scroll {name} ({how}): paint {:.2} + copy {:.2} ms/frame", paint * 50.0, copy * 50.0);
                }
            }
            app.state.playlist_scroll = (0.0, 0.0);
            app.paint(&mut canvas, None);
        }
    }
    canvas.pixmap.save_png(out.expect("--screenshot needs a path")).expect("write png");
}

/// Wakes the event loop from any thread (hosted plugins asking for the UI thread: a parameter flush after an editor
/// edit, a callback). One pending wake at a time; `about_to_wait` runs after the user event and ticks the app.
static PROXY: std::sync::Mutex<Option<EventLoopProxy<()>>> = std::sync::Mutex::new(None);
static WAKE_PENDING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Linux: the main window goes on X11 (XWayland under Wayland) so plugin windows can dock in it: plugin editors on Linux
/// are X11 windows, and an X11 window cannot be a child of a Wayland one. Native Wayland when there is no X server, or
/// when `OPENSTUDIO_WAYLAND=1` asks for it; plugin windows are then always detached.
#[cfg(target_os = "linux")]
fn use_x11() -> bool {
    if std::env::var_os("OPENSTUDIO_WAYLAND").is_some_and(|v| v == "1") {
        return false;
    }
    std::env::var_os("DISPLAY").is_some_and(|d| !d.is_empty()) && x11rb::rust_connection::RustConnection::connect(None).is_ok()
}

fn wake_ui() {
    if !WAKE_PENDING.swap(true, std::sync::atomic::Ordering::AcqRel) {
        if let Ok(p) = PROXY.lock() {
            if let Some(p) = p.as_ref() {
                let _ = p.send_event(());
            }
        }
    }
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    // A plugin scanner process (spec: ui/plugin-manager.md §4): loads one file, prints its plugins, ends.
    if args.first().map(String::as_str) == Some(plugin_scan::ARG) {
        plugin_scan::child_main(&args[1..]);
    }
    // A child the app starts to try a native plugin before its first load (spec: plugins/fruity-abi.md).
    if args.first().map(String::as_str) == Some(os_native::check::CHECK_ARG) {
        os_host::guard::exit(os_native::check::run(&args[1..]));
    }
    // macOS: the startup splash, in a process of its own (`splash`).
    #[cfg(target_os = "macos")]
    if args.first().map(String::as_str) == Some(splash::ARG) {
        splash::child_main();
    }
    // Plugin editors in real plugin windows, checked from the main thread (`plugins::window_check`).
    #[cfg(target_os = "macos")]
    if args.first().map(String::as_str) == Some("--plugin-window-check") {
        std::process::exit(plugins::window_check::run(&args[1..]));
    }
    app::move_old_settings();
    // The mixing threads every engine renders on, from here on (spec: audio/multicore.md).
    mix_threads::start();
    // A render without a window (FILE > Export audio from the command line, `render`).
    if args.first().map(String::as_str) == Some("--render") {
        std::process::exit(render::cli(&args[1..]));
    }
    if args.iter().any(|a| a == "--screenshot") {
        screenshot(&args);
        return;
    }
    // The control plane's socket (spec: roadmap/control-plane.md): beside the window, or without one.
    let control = control::server::Flags::take(&mut args).unwrap_or_else(|e| {
        eprintln!("Lemon Studio: {e}");
        std::process::exit(2);
    });
    let level = control.level.unwrap_or(control::Level::Edit);
    if let Some(at) = &control.serve {
        control::server::serve_headless(at, level, control.token.clone(), args.first().map(std::path::PathBuf::from));
    }
    // Run from source: Lemon Studio's own plugins are rebuilt and installed before the plugin scan looks for them.
    plugin_rebuild::run();
    let path = args.first().map(std::path::PathBuf::from);
    let mut builder = EventLoop::<()>::with_user_event();
    #[cfg(target_os = "linux")]
    if use_x11() {
        use winit::platform::x11::EventLoopBuilderExtX11;
        builder.with_x11();
    }
    let event_loop = builder.build().expect("event loop");
    // The startup splash, from here until startup is done (spec: ui/settings-choices.md "Miscellaneous: naming,
    // detached plugins and startup"); each step below moves its gauge on. After the event loop: on Windows building it
    // makes the process per-monitor DPI aware, which the splash's size follows.
    let splash = splash::open();
    // Before the launch: the files the app is launched to open come ahead of `resumed`.
    #[cfg(target_os = "macos")]
    macos::hook_app();
    if let Ok(mut p) = PROXY.lock() {
        *p = Some(event_loop.create_proxy());
    }
    os_clap::set_waker(wake_ui);
    // CLAP plugins asking which thread they are on (`thread-check`): this one is the main thread.
    os_clap::set_main_thread();
    // Lemon plugins' editors run on threads of their own off Windows and wake the app when they send something.
    #[cfg(not(windows))]
    os_hostlink::set_host_waker(wake_ui);
    // Linux: the cards the sound server gave up on while a run before this one had them through ALSA (one that
    // crashed, or ended with a plugin) are given back (`device::heal_sound_server`).
    #[cfg(target_os = "linux")]
    std::thread::spawn(device::heal_sound_server);
    let start = path.is_none();
    splash::step(&splash, splash::SETTINGS, if start { "Reading settings" } else { "Opening project" });
    let mut app = App::new(path);
    // Without a project file: the Startup project (spec: ui/settings-choices.md "Miscellaneous: seven more checks,
    // Default template and Startup project").
    if start {
        splash::step(&splash, splash::PROJECT, "Opening project");
        app.startup_project();
        // Ask for data folder "On startup": the New project form over it (spec: ui/data-folder.md §7).
        app.startup_form();
    }
    if let Some(at) = &control.control {
        app.state.message = Some(match control::server::start(at, level, wake_ui) {
            Ok(mut server) => {
                if control.token.is_some() {
                    server.token = control.token.clone();
                }
                let m = format!("Control plane: {}", server.describe());
                eprintln!("Lemon Studio: {m}");
                app.listen(server);
                m
            }
            Err(e) => format!("Control plane: could not listen at {at}: {e}"),
        });
    }
    // The windows where the last run left them.
    layout::restore(&mut app.state);
    // MIDI devices (spec: ui/midi-settings.md §2): the enabled inputs play the selected channel; master sync goes out.
    splash::step(&splash, splash::MIDI, "Opening MIDI devices");
    app.midi = midi_in::MidiIn::open(wake_ui);
    app.midi_out = midi_out::MidiOut::start(app.transport.watch());
    let message = app.state.message.clone();
    app.midi_refresh(false);
    if message.is_some() {
        app.state.message = message;
    }
    // Plugin menus and dialogs run modal loops on this thread on Windows (elsewhere Lemon plugins' editors have threads
    // of their own).
    let watchdog = stall::Watchdog::new(stall::LATE);
    #[cfg(windows)]
    watchdog.start(wake_ui);
    let mut shell = Shell {
        app,
        window: None,
        surface: None,
        canvas: None,
        mods: ModifiersState::default(),
        last_click: Instant::now() - Duration::from_secs(10),
        lock: None,
        pointer_locked: false,
        cursor: app::Cursor::Default,
        inset: (0, 0, 0, 0),
        edge: None,
        grab: None,
        unmax: None,
        pending: false,
        painted: Instant::now(),
        frame_start: Instant::now(),
        frame: FRAME,
        frame_monitor: None,
        splash,
        shown: false,
        frames: FrameLog::open(),
        watchdog,
        ctrl_click: false,
        main_win: layout::main_win(),
    };
    event_loop.run_app(&mut shell).expect("run");
    // After a contained plugin crash the process ends here, before plugin files are unloaded (a stopped plugin's code
    // must not run again).
    if os_host::guard::tripped() {
        os_host::guard::exit(0);
    }
    // The app's plugin instances are destroyed here, on this (UI) thread, as far as the engine has let go of them.
    drop(shell);
    plugins::sweep_retired();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Frames come a little under one refresh apart: never further apart than `FRAME`, never closer than `IDLE`.
    #[test]
    fn frame_interval_follows_the_refresh_rate() {
        let ms = |mhz| frame_interval(mhz).as_secs_f64() * 1000.0;
        assert!((ms(Some(60_000)) - 16.0).abs() < 1e-6);
        assert!((ms(Some(180_000)) - 5.333).abs() < 1e-3, "180 Hz: {}", ms(Some(180_000)));
        assert_eq!(frame_interval(Some(30_000)), FRAME);
        assert_eq!(frame_interval(Some(1_000_000)), IDLE);
        assert_eq!(frame_interval(None), FRAME);
        assert_eq!(frame_interval(Some(0)), FRAME);
    }

    /// Animation refresh rate: while something animates, frames come no closer than the display's refresh
    /// (Ultrasmooth, the default), 60 Hz (Smooth) or 40 Hz (Less smooth); other frames keep the display's rate.
    // spec: ui/settings-choices.md "Display: Smoothing, Animation refresh rate, Transparent menus and windows
    // (2026-10-16)"
    #[test]
    fn animation_refresh_rate_caps_frames() {
        let rate = &prefs::ANIMATION_REFRESH_RATE;
        let display = frame_interval(Some(180_000));
        assert_eq!(prefs::choice(rate), "Ultrasmooth");
        assert_eq!(paced(display, true), display);
        prefs::set_choice(rate, 1);
        assert_eq!(paced(display, true), Duration::from_secs_f64(1.0 / 60.0));
        assert_eq!(paced(display, false), display, "nothing animates");
        prefs::set_choice(rate, 0);
        assert_eq!(paced(display, true), Duration::from_millis(25));
        assert_eq!(paced(FRAME, true), Duration::from_millis(25), "a 60 Hz display too");
        prefs::set_choice(rate, 2);
        assert_eq!(paced(FRAME, true), FRAME);
    }

    /// Main GUI scaling (spec: ui/ui-scaling.md §1): System keeps the monitor's scale, a percentage replaces it.
    #[test]
    fn main_gui_scaling_replaces_the_monitor_scale() {
        let choices = prefs::MAIN_GUI_SCALING.choices;
        let mut got = Vec::new();
        for i in 0..choices.len() {
            prefs::set_choice(&prefs::MAIN_GUI_SCALING, i);
            got.push(gui_scale(1.5));
        }
        assert_eq!(got, [1.5, 1.0, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0, 3.5, 4.0]);
        prefs::set_choice(&prefs::MAIN_GUI_SCALING, 0);
        assert_eq!(gui_scale(1.25), 1.25);
    }
}
