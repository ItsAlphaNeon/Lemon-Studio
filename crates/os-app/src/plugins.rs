//! Hosted generator plugins (CLAP): scanning, one instance per plugin channel, their windows, and the
//! instrument list the transport plays.
//!
// spec: plugins/plugin-formats.md (hosting, where to find plugins, Open Sampler state), ui/plugin-window.md,
// plugins/host-link.md (Open plugins join the window strip)

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
    /// Set while the open editor belongs to an Open plugin that joined the strip (host link).
    #[cfg(windows)]
    pub link: Option<os_hostlink::Link>,
}

/// What a plugin window asks of the app.
#[derive(Clone, Debug, PartialEq)]
pub enum Request {
    Close(usize),
    ToggleOn(usize),
    Volume(usize, f64),
    Pan(usize, f64),
    PitchCents(usize, i32),
    /// A preview note from an Open plugin's own keyboard (host link): row, key, velocity, on.
    Note(usize, u8, u8, bool),
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
        self.hosted.insert(row, Hosted { plugin: Arc::new(Mutex::new(p)), #[cfg(windows)] window: None, #[cfg(windows)] link: None });
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
        // An Open plugin answers the hello: its page tabs go into the strip row and its editor shrinks to what is below.
        h.link = os_hostlink::Link::connect(w.area() as isize);
        match &h.link {
            Some(l) => {
                w.set_pages(l.pages.clone(), l.page_now());
                let (lw, lh) = l.size().unwrap_or((ew, eh));
                w.set_editor_size((lw as f32, lh as f32));
            }
            None => w.set_editor_size((ew as f32 / scale, eh as f32 / scale)),
        }
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
            h.link = None;
        }
    }

    /// UI tick: plugin housekeeping, then the windows' requests (a closed window is closed here).
    pub fn idle(&mut self, info: impl Fn(usize) -> Option<ChannelInfoT>) -> Vec<Request> {
        let mut out = Vec::new();
        let mut close = Vec::new();
        // Plugin -> host link messages (a woken tick polls the link below anyway; notes become requests).
        #[cfg(windows)]
        for (area, m) in crate::plugin_window::take_link() {
            if let crate::plugin_window::LinkMsg::Note(k, v, on) = m {
                let row = self.hosted.iter().find(|(_, h)| h.window.as_ref().is_some_and(|w| w.area() as isize == area)).map(|(&r, _)| r);
                if let Some(row) = row {
                    out.push(Request::Note(row, k, v, on));
                }
            }
        }
        for (&row, h) in &mut self.hosted {
            if let Ok(mut p) = h.plugin.try_lock() {
                p.idle();
                #[cfg(windows)]
                if let (Some(w), None, Some((ew, eh))) = (&h.window, &h.link, p.editor_size()) {
                    w.set_editor_size((ew as f32 / self.scale, eh as f32 / self.scale));
                }
            }
            #[cfg(windows)]
            if let Some(w) = &h.window {
                if let Some(i) = info(row) {
                    w.set_info(i);
                }
                if let Some(l) = &mut h.link {
                    w.set_page(l.page());
                    w.set_hint(l.hint());
                    if let Some((lw, lh)) = l.size() {
                        w.set_editor_size((lw as f32, lh as f32));
                    }
                }
                for e in w.take_events() {
                    if let (FrameEvent::Page(n), Some(l)) = (&e, &mut h.link) {
                        l.set_page(*n);
                        continue;
                    }
                    out.push(match e {
                        FrameEvent::Page(_) => continue,
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

/// Opens the bundled Open Sampler in a real plugin window, clicks through the strip and captures the window
/// (`OS_SHOT_DIR=<dir> OS_CLAP=<Open Sampler.clap> cargo test -p os-app plugin_window_shots -- --ignored`).
#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::ffi::c_void;
    use std::time::{Duration, Instant};
    use windows_sys::Win32::Foundation::{HWND, RECT};
    use windows_sys::Win32::Graphics::Gdi::*;
    use windows_sys::Win32::Storage::Xps::PrintWindow;
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    fn pump(plugins: &mut Plugins, ms: u64) {
        let end = Instant::now() + Duration::from_millis(ms);
        while Instant::now() < end {
            unsafe {
                let mut m: MSG = std::mem::zeroed();
                while PeekMessageW(&mut m, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                    TranslateMessage(&m);
                    DispatchMessageW(&m);
                }
            }
            plugins.idle(|_| None);
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn click(hwnd: isize, x: i32, y: i32) {
        unsafe { SendMessageW(hwnd as HWND, WM_LBUTTONDOWN, 1, (x | (y << 16)) as isize) };
        unsafe { SendMessageW(hwnd as HWND, WM_LBUTTONUP, 0, (x | (y << 16)) as isize) };
    }

    /// PrintWindow of `hwnd` into a top-down 32-bit BMP.
    fn capture(hwnd: isize, path: &std::path::Path) {
        unsafe {
            let mut r: RECT = std::mem::zeroed();
            GetWindowRect(hwnd as HWND, &mut r);
            let (w, h) = (r.right - r.left, r.bottom - r.top);
            let dc = GetDC(std::ptr::null_mut());
            let mem = CreateCompatibleDC(dc);
            let mut bmi: BITMAPINFO = std::mem::zeroed();
            bmi.bmiHeader = BITMAPINFOHEADER { biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32, biWidth: w, biHeight: -h, biPlanes: 1, biBitCount: 32, biCompression: BI_RGB, ..std::mem::zeroed() };
            let mut bits: *mut c_void = std::ptr::null_mut();
            let bmp = CreateDIBSection(dc, &bmi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
            let old = SelectObject(mem, bmp);
            PrintWindow(hwnd as HWND, mem, 2);
            let px = std::slice::from_raw_parts(bits as *const u8, (w * h * 4) as usize);
            let mut f = Vec::new();
            f.extend_from_slice(b"BM");
            f.extend_from_slice(&((54 + px.len()) as u32).to_le_bytes());
            f.extend_from_slice(&[0, 0, 0, 0]);
            f.extend_from_slice(&54u32.to_le_bytes());
            f.extend_from_slice(&40u32.to_le_bytes());
            f.extend_from_slice(&w.to_le_bytes());
            f.extend_from_slice(&(-h).to_le_bytes());
            f.extend_from_slice(&1u16.to_le_bytes());
            f.extend_from_slice(&32u16.to_le_bytes());
            f.extend_from_slice(&[0; 24]);
            f.extend_from_slice(px);
            std::fs::write(path, f).unwrap();
            SelectObject(mem, old);
            DeleteObject(bmp);
            DeleteDC(mem);
            ReleaseDC(std::ptr::null_mut(), dc);
        }
    }

    #[test]
    #[ignore]
    fn plugin_window_shots() {
        let (Some(dir), Some(clap)) = (std::env::var_os("OS_SHOT_DIR"), std::env::var_os("OS_CLAP")) else { return };
        let dir = std::path::PathBuf::from(dir);
        let lib = Library::load(std::path::Path::new(&clap)).expect("load clap");
        let desc = lib.plugins[0].clone();
        let mut pl = Plugins { available: vec![Available { lib, desc }], scale: 1.0, ..Plugins::default() };
        pl.create(0, 0).unwrap();
        let info = ChannelInfo { name: "Open Sampler".into(), target: "Insert 1".into(), enabled: true, volume: 0.78, pan: 0.0, pitch_cents: 0, range: 2, track: 1 };
        pl.open_window(0, info).unwrap();
        pump(&mut pl, 400);
        let hwnd = pl.hosted[&0].window.as_ref().unwrap().hwnd();
        assert!(pl.hosted[&0].link.is_some(), "Open Sampler should join the strip");
        capture(hwnd, &dir.join("win_sample.bmp"));
        click(hwnd, 68, 47); // second page tab
        pump(&mut pl, 300);
        capture(hwnd, &dir.join("win_envelope.bmp"));
        click(hwnd, 111, 47); // Misc wrench
        pump(&mut pl, 300);
        capture(hwnd, &dir.join("win_misc.bmp"));
        click(hwnd, 24, 12); // gear: hide the strip
        pump(&mut pl, 300);
        capture(hwnd, &dir.join("win_nostrip.bmp"));
    }

    /// Random clicks, drags and wheel turns on the editor; a probe toggle must keep changing the plugin state.
    /// `OS_SHOT_DIR=.. OS_CLAP=.. cargo test -p os-app plugin_editor_stress -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn plugin_editor_stress() {
        let Some(clap) = std::env::var_os("OS_CLAP") else { return };
        // Right clicks and list buttons open modal menus; the plugin skips them under this switch.
        unsafe { std::env::set_var("OPENSAMPLER_NO_MODAL", "1") };
        let lib = Library::load(std::path::Path::new(&clap)).expect("load clap");
        let desc = lib.plugins[0].clone();
        let mut pl = Plugins { available: vec![Available { lib, desc }], scale: 1.0, ..Plugins::default() };
        pl.create(0, 0).unwrap();
        let info = ChannelInfo { name: "Open Sampler".into(), target: "Insert 1".into(), enabled: true, volume: 0.78, pan: 0.0, pitch_cents: 0, range: 2, track: 1 };
        pl.open_window(0, info).unwrap();
        pump(&mut pl, 300);
        let frame = pl.hosted[&0].window.as_ref().unwrap().hwnd();
        let editor = unsafe { GetWindow(GetWindow(frame as HWND, GW_CHILD), GW_CHILD) } as isize;
        assert!(editor != 0);
        let mut seed = 0x2545F4914F6CDD1Du64;
        let mut rnd = move |n: i32| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % n as u64) as i32
        };
        let state = |pl: &Plugins| pl.hosted[&0].plugin.lock().unwrap().save_state().unwrap();
        let send = |msg: u32, wp: usize, x: i32, y: i32| unsafe { SendMessageW(editor as HWND, msg, wp, (x | (y << 16)) as isize) };
        // Probe on the sample page: the "Normalize" radio must change the plugin state, twice (on, off).
        let probe = |pl: &mut Plugins| -> bool {
            pl.hosted.get_mut(&0).unwrap().link.as_mut().unwrap().set_page(0);
            pump(pl, 40);
            let before = state(pl);
            send(WM_LBUTTONDOWN, 1, 298, 149);
            send(WM_LBUTTONUP, 0, 298, 149);
            pump(pl, 40);
            let changed = state(pl) != before;
            send(WM_LBUTTONDOWN, 1, 298, 149);
            send(WM_LBUTTONUP, 0, 298, 149);
            pump(pl, 40);
            changed
        };
        assert!(probe(&mut pl), "the probe does not work on a fresh editor");
        let mut failed = None;
        for i in 0..4000 {
            // Not the File row (y < 45): its open button pops a modal file dialog.
            let (x, y) = (rnd(560), 45 + rnd(345));
            match rnd(8) {
                0 | 1 => {
                    send(WM_LBUTTONDOWN, 1, x, y);
                    send(WM_LBUTTONUP, 0, x, y);
                }
                2 => {
                    send(WM_LBUTTONDOWN, 1, x, y);
                    for _ in 0..rnd(30) {
                        send(WM_MOUSEMOVE, 1, x + rnd(5), y - rnd(80));
                    }
                    send(WM_LBUTTONUP, 0, x, y);
                }
                3 => {
                    send(WM_LBUTTONDBLCLK, 1, x, y);
                    send(WM_LBUTTONUP, 0, x, y);
                }
                4 => {
                    send(WM_RBUTTONDOWN, 2, x, y);
                    send(WM_RBUTTONUP, 0, x, y);
                }
                5 => {
                    send(WM_MBUTTONDOWN, 16, x, y);
                    send(WM_MBUTTONUP, 0, x, y);
                }
                6 => {
                    // Switch page through the host link.
                    if let Some(l) = &mut pl.hosted.get_mut(&0).unwrap().link {
                        l.set_page(rnd(2) as usize);
                    }
                }
                _ => {
                    send(WM_MOUSEMOVE, 0, x, y);
                }
            }
            if i % 20 == 0 {
                pump(&mut pl, 15);
            }
            if i % 100 == 99 && !probe(&mut pl) && failed.is_none() {
                failed = Some(i);
            }
        }
        assert!(failed.is_none(), "the editor stopped responding after {:?} events", failed);
    }

    /// Real mouse input (moves the desktop cursor): dragging the START OFFSET knob hides the cursor and holds it at the
    /// press point, the value follows the movement, and releasing shows the cursor at the same place.
    /// `OS_CLAP=.. cargo test -p os-app plugin_knob_drag_holds_the_cursor -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn plugin_knob_drag_holds_the_cursor() {
        use windows_sys::Win32::Foundation::POINT;
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::{SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MOVE, MOUSEINPUT};
        let Some(clap) = std::env::var_os("OS_CLAP") else { return };
        let lib = Library::load(std::path::Path::new(&clap)).expect("load clap");
        let desc = lib.plugins[0].clone();
        let mut pl = Plugins { available: vec![Available { lib, desc }], scale: 1.0, ..Plugins::default() };
        pl.create(0, 0).unwrap();
        let info = ChannelInfo { name: "Open Sampler".into(), target: "Insert 1".into(), enabled: true, volume: 0.78, pan: 0.0, pitch_cents: 0, range: 2, track: 1 };
        pl.open_window(0, info).unwrap();
        pump(&mut pl, 300);
        let frame = pl.hosted[&0].window.as_ref().unwrap().hwnd();
        let editor = unsafe { GetWindow(GetWindow(frame as HWND, GW_CHILD), GW_CHILD) };
        unsafe { SetForegroundWindow(frame as HWND) };
        pump(&mut pl, 200);
        let state = |pl: &Plugins| String::from_utf8_lossy(&pl.hosted[&0].plugin.lock().unwrap().save_state().unwrap()).to_string();
        let offset = |s: &str| -> f32 {
            let i = s.find("\"start_offset\"").expect("start_offset in state");
            let rest = &s[i + 14..];
            let rest = &rest[rest.find(':').map_or(0, |j| j + 1)..];
            let rest = &rest[rest.find(':').map_or(0, |j| j + 1)..];
            let num: String = rest.chars().skip_while(|c| !(c.is_ascii_digit() || *c == '-')).take_while(|c| c.is_ascii_digit() || matches!(c, '.' | '-' | 'e')).collect();
            num.parse().unwrap_or(-1.0)
        };
        let before = offset(&state(&pl));
        // START OFFSET knob: FL (45, 286) -> editor client (41, 214) with the host's strip row.
        let mut p = POINT { x: 41, y: 214 };
        unsafe { ClientToScreen(editor, &mut p) };
        let input = |flags: u32, dx: i32, dy: i32| unsafe {
            let i = INPUT { r#type: INPUT_MOUSE, Anonymous: INPUT_0 { mi: MOUSEINPUT { dx, dy, mouseData: 0, dwFlags: flags, time: 0, dwExtraInfo: 0 } } };
            SendInput(1, &i, std::mem::size_of::<INPUT>() as i32);
        };
        let cursor = || unsafe {
            let mut c = POINT { x: 0, y: 0 };
            GetCursorPos(&mut c);
            (c.x, c.y)
        };
        let shown = || unsafe {
            let mut ci: CURSORINFO = std::mem::zeroed();
            ci.cbSize = std::mem::size_of::<CURSORINFO>() as u32;
            GetCursorInfo(&mut ci);
            ci.flags & CURSOR_SHOWING != 0
        };
        unsafe { SetCursorPos(p.x, p.y) };
        pump(&mut pl, 50);
        input(MOUSEEVENTF_LEFTDOWN, 0, 0);
        pump(&mut pl, 50);
        let mut worst = 0;
        let t0 = Instant::now();
        for _ in 0..40 {
            input(MOUSEEVENTF_MOVE, 0, -3);
            pump(&mut pl, 12);
            let (x, y) = cursor();
            worst = worst.max((x - p.x).abs() + (y - p.y).abs());
        }
        let took = t0.elapsed();
        let hidden = !shown();
        let during = offset(&state(&pl));
        input(MOUSEEVENTF_LEFTUP, 0, 0);
        pump(&mut pl, 80);
        let after = cursor();
        println!("offset {before} -> {during}, worst cursor drift {worst} px, cursor hidden while dragging: {hidden}, 40 moves in {took:?}");
        assert!(during > before + 0.01, "the knob did not follow the drag ({before} -> {during})");
        assert!(worst <= 3, "the cursor was not held in place (drift {worst} px)");
        assert!(hidden, "the cursor should be hidden while dragging a knob");
        assert!(shown(), "the cursor should show again after the drag");
        assert_eq!(after, (p.x, p.y), "the cursor should be back at the press point");
    }

    /// Keys typed while the plugin window has the focus must reach the app (the typing keyboard keeps playing).
    #[test]
    #[ignore]
    fn keys_in_a_plugin_window_reach_the_app() {
        let Some(clap) = std::env::var_os("OS_CLAP") else { return };
        let lib = Library::load(std::path::Path::new(&clap)).expect("load clap");
        let desc = lib.plugins[0].clone();
        let mut pl = Plugins { available: vec![Available { lib, desc }], scale: 1.0, ..Plugins::default() };
        pl.create(0, 0).unwrap();
        let info = ChannelInfo { name: "Open Sampler".into(), target: "Insert 1".into(), enabled: true, volume: 0.78, pan: 0.0, pitch_cents: 0, range: 2, track: 1 };
        pl.open_window(0, info).unwrap();
        pump(&mut pl, 300);
        let frame = pl.hosted[&0].window.as_ref().unwrap().hwnd();
        let editor = unsafe { GetWindow(GetWindow(frame as HWND, GW_CHILD), GW_CHILD) };
        unsafe {
            SetForegroundWindow(frame as HWND);
            // A click inside the editor gives it the focus, as for a user.
            SendMessageW(editor, WM_LBUTTONDOWN, 1, (300 | (60 << 16)) as isize);
            SendMessageW(editor, WM_LBUTTONUP, 0, (300 | (60 << 16)) as isize);
        }
        eprintln!("focus {:?} editor {:?} frame {:?} foreground {:?}", unsafe { windows_sys::Win32::UI::Input::KeyboardAndMouse::GetFocus() }, editor, frame, unsafe { GetForegroundWindow() });
        crate::plugin_window::take_keys();
        let q = 0x10isize; // scancode of Q
        // Real key input (the hook sees what the system queue delivers; posted messages bypass it), only while our
        // frame is in front so the key cannot land in another program.
        assert_eq!(unsafe { GetForegroundWindow() }, frame as HWND, "the plugin window must be in front");
        unsafe {
            use windows_sys::Win32::UI::Input::KeyboardAndMouse::{keybd_event, KEYEVENTF_KEYUP};
            keybd_event(0x51, q as u8, 0, 0);
            keybd_event(0x51, q as u8, KEYEVENTF_KEYUP, 0);
        }
        pump(&mut pl, 100);
        let keys = crate::plugin_window::take_keys();
        assert_eq!(keys.len(), 2, "{keys:?}");
        assert!(keys[0].pressed && !keys[1].pressed && keys[0].scancode == 0x10);
    }
}
