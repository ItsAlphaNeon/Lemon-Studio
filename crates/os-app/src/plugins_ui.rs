// Copyright (C) 2026 ItsAlphaNeon
// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The interface's side of `crate::plugins` (step 9: the session's side stays there).

#![allow(unused_imports)]
use crate::plugins::*;

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::ffi::c_void;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    use windows_sys::Win32::Foundation::{HWND, RECT};
    use windows_sys::Win32::Graphics::Gdi::*;
    use windows_sys::Win32::Storage::Xps::PrintWindow;
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    fn pump(host: &mut crate::editor_host::EditorHost, plugins: &mut Plugins, ms: u64) {
        let end = Instant::now() + Duration::from_millis(ms);
        while Instant::now() < end {
            unsafe {
                let mut m: MSG = std::mem::zeroed();
                while PeekMessageW(&mut m, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                    TranslateMessage(&m);
                    DispatchMessageW(&m);
                }
            }
            host.idle(plugins, |_| None);
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

    /// Plugin editors draw at the window's DPI scale and follow a scale change (spec: ui/ui-scaling.md §3). Opens real
    /// windows: `cargo test -p os-app --lib plugin_editors_follow_the_scale -- --ignored`.
    #[test]
    #[ignore]
    fn plugin_editors_follow_the_scale() {
        let size = |h: isize| unsafe {
            let mut r: RECT = std::mem::zeroed();
            GetWindowRect(h as HWND, &mut r);
            (r.right - r.left, r.bottom - r.top)
        };
        let mut pl = Plugins { available: builtin(), scale: 1.5, rate: 48000, ..Plugins::default() };
        let mut host = crate::editor_host::EditorHost::default();
        let index = pl.index_of(LEMON_SAMPLER).expect("built-in Lemon Sampler");
        pl.create_with(0, index, None).unwrap();
        let info = ChannelInfo { name: LEMON_SAMPLER.into(), target: "Bus 1".into(), enabled: true, volume: 0.78, pan: 0.0, pitch_cents: 0, range: 2, track: 1, max_track: 125, misc: None, icon: None };
        host.open_window(&mut pl, 0, info).unwrap();
        pump(&mut host, &mut pl, 200);
        let w = host.editors[&0].window.as_ref().unwrap();
        let (frame, area) = (w.hwnd(), w.area() as isize);
        let editor = || unsafe { GetWindow(area as HWND, GW_CHILD) } as isize;
        // Joined the strip: 560 x 390 logical (spec: plugins/host-link.md), drawn at 150 %.
        assert_eq!(size(editor()), (840, 585), "the editor draws at the window's scale");
        assert_eq!(size(area), (840, 585), "the frame's editor area matches it");
        let page = 1;
        host.editors.get_mut(&0).unwrap().link.as_mut().unwrap().set_page(page);
        for (k, want) in [(1.0, (560, 390)), (2.0, (1120, 780))] {
            host.rescale_window(&mut pl, 0, k);
            pump(&mut host, &mut pl, 100);
            assert_eq!(size(editor()), want, "editor at scale {k}");
            assert_eq!(size(area), want, "editor area at scale {k}");
            let (fw, _) = size(frame);
            assert_eq!(fw, want.0 + (2.0 * 7.0 * k) as i32, "frame width at scale {k}");
            assert!(host.editors[&0].link.is_some(), "the reopened editor joins the strip again");
            assert_eq!(host.editors[&0].link.as_ref().unwrap().page_now(), page, "the plugin's page is kept");
        }
    }

    #[test]
    #[ignore]
    fn plugin_window_shots() {
        let (Some(dir), Some(clap)) = (std::env::var_os("OS_SHOT_DIR"), std::env::var_os("OS_CLAP")) else { return };
        let dir = std::path::PathBuf::from(dir);
        let lib = os_clap::Library::load(std::path::Path::new(&clap)).expect("load clap");
        let desc = lib.plugins[0].clone();
        let mut pl = Plugins { available: vec![Available { lib, desc }], scale: 1.0, ..Plugins::default() };
        let mut host = crate::editor_host::EditorHost::default();
        pl.create(0, 0).unwrap();
        let info = ChannelInfo { name: "Lemon Sampler".into(), target: "Bus 1".into(), enabled: true, volume: 0.78, pan: 0.0, pitch_cents: 0, range: 2, track: 1, max_track: 125, misc: None, icon: None };
        host.open_window(&mut pl, 0, info).unwrap();
        pump(&mut host, &mut pl, 400);
        let hwnd = host.editors[&0].window.as_ref().unwrap().hwnd();
        assert!(host.editors[&0].link.is_some(), "Lemon Sampler should join the strip");
        capture(hwnd, &dir.join("win_sample.bmp"));
        click(hwnd, 68, 47); // second page tab
        pump(&mut host, &mut pl, 300);
        capture(hwnd, &dir.join("win_envelope.bmp"));
        click(hwnd, 111, 47); // Misc wrench
        pump(&mut host, &mut pl, 300);
        capture(hwnd, &dir.join("win_misc.bmp"));
        click(hwnd, 24, 12); // gear: hide the strip
        pump(&mut host, &mut pl, 300);
        capture(hwnd, &dir.join("win_nostrip.bmp"));
    }

    /// The built-in Lemon Parametric EQ in a mixer slot window: the frame without a strip, the editor inside it, and a
    /// click on a band's switch reaching the plugin state.
    /// `OS_SHOT_DIR=.. cargo test -p os-app --lib eq_window_shots -- --ignored`
    #[test]
    #[ignore]
    fn eq_window_shots() {
        let Some(dir) = std::env::var_os("OS_SHOT_DIR") else { return };
        let dir = std::path::PathBuf::from(dir);
        let mut pl = Plugins { effects: builtin_effects(), scale: 1.0, rate: 48000, ..Plugins::default() };
        let mut host = crate::editor_host::EditorHost::default();
        let index = pl.effect_index(LEMON_EQ).expect("built-in EQ");
        pl.create_effect(1, 0, index, None).unwrap();
        let key = fx_key(1, 0);
        let info = ChannelInfo { name: LEMON_EQ.into(), target: "Bus 1".into(), enabled: true, volume: 0.8, pan: 0.0, pitch_cents: 0, range: 2, track: 1, max_track: 125, misc: None, icon: None };
        host.open_window(&mut pl, key, info).unwrap();
        pump(&mut host, &mut pl, 400);
        let frame = host.editors[&key].window.as_ref().unwrap().hwnd();
        capture(frame, &dir.join("eq_window.bmp"));
        let editor = unsafe { GetWindow(GetWindow(frame as HWND, GW_CHILD), GW_CHILD) } as isize;
        assert!(editor != 0, "the editor is a child of the frame");
        let mask = |pl: &Plugins| pl.eq_file_state(key).map(|b| b[318]);
        assert_eq!(mask(&pl), Some(0x7f));
        // Band 3's switch (editor coordinates, spec: plugins/lemon-parametric-eq-2.md §3): mutes band 3.
        click(editor, 549 + 21 * 2 + 6, 296);
        pump(&mut host, &mut pl, 200);
        capture(frame, &dir.join("eq_window_muted.bmp"));
        assert_eq!(mask(&pl), Some(0x7f & !4));
    }

    /// Lemon Trio in its window: a wheel turn on a knob changes the plugin's value AND the picture (spec:
    /// plugins/lemon-3x-osc.md §5). `OS_SHOT_DIR=.. cargo test -p os-app --lib osc3_window_redraws -- --ignored`
    #[test]
    #[ignore]
    fn osc3_window_redraws() {
        let Some(dir) = std::env::var_os("OS_SHOT_DIR") else { return };
        let dir = std::path::PathBuf::from(dir);
        let mut pl = Plugins { available: builtin(), scale: 1.0, rate: 48000, ..Plugins::default() };
        let mut host = crate::editor_host::EditorHost::default();
        let index = pl.index_of(crate::osc3::LEMON_TRIO).expect("built-in Lemon Trio");
        pl.create_with(0, index, None).unwrap();
        let info = ChannelInfo { name: crate::osc3::LEMON_TRIO.into(), target: "Bus 1".into(), enabled: true, volume: 0.78, pan: 0.0, pitch_cents: 0, range: 2, track: 1, max_track: 125, misc: None, icon: None };
        host.open_window(&mut pl, 0, info).unwrap();
        pump(&mut host, &mut pl, 400);
        let frame = host.editors[&0].window.as_ref().unwrap().hwnd();
        let editor = unsafe { GetWindow(GetWindow(frame as HWND, GW_CHILD), GW_CHILD) } as isize;
        assert!(editor != 0, "the editor is a child of the frame");
        // Where osc 2's coarse knob is drawn (painted-frame coordinates -> editor client, hosted crop).
        let mut c = os_ui::Canvas::new(lemon_3x_osc::view::W, lemon_3x_osc::view::H, 1.0).unwrap();
        struct Mid;
        impl lemon_3x_osc::view::Model for Mid {
            fn norm(&self, _: &str) -> f32 {
                0.5
            }
            fn plain(&self, _: &str) -> f32 {
                0.0
            }
            fn text(&self, _: &str) -> String {
                String::new()
            }
        }
        let hits = lemon_3x_osc::view::paint(&mut c, &os_theme::design::palette(os_theme::Design::Lemon, None), &Mid, &Default::default(), "", true);
        let t = lemon_3x_osc::view::Target::Knob("o2_coarse".into());
        let r = hits.iter().rev().find(|h| h.t == t).expect("coarse knob").r;
        let (cx, cy) = ((r.cx() - lemon_3x_osc::view::CROP_X as f32) as i32, (r.cy() - lemon_3x_osc::view::HOSTED_CROP_Y as f32) as i32);
        let mut er: RECT = unsafe { std::mem::zeroed() };
        unsafe { GetWindowRect(editor as HWND, &mut er) };
        let state = |pl: &Plugins| pl.hosted[&0].plugin.lock().unwrap().save_state().unwrap();
        let (a, b) = (dir.join("osc3_before.bmp"), dir.join("osc3_after.bmp"));
        capture(frame, &a);
        let before = state(&pl);
        for _ in 0..6 {
            let (sx, sy) = (er.left + cx, er.top + cy);
            unsafe { SendMessageW(editor as HWND, WM_MOUSEWHEEL, (120usize) << 16, ((sx & 0xffff) | (sy << 16)) as isize) };
            pump(&mut host, &mut pl, 60);
        }
        pump(&mut host, &mut pl, 300);
        capture(frame, &b);
        assert_ne!(state(&pl), before, "the knob moved the plugin value");
        assert_ne!(std::fs::read(&a).unwrap(), std::fs::read(&b).unwrap(), "the editor redrew");
    }

    /// The built-in Citrus's editor in its window: the wheel on SOFTEN and a right click on a matrix cell edit the
    /// Sytrus state the channel saves (spec: plugins/citrus.md §6).
    /// `OS_SHOT_DIR=.. cargo test -p os-app --lib citrus_window_edits -- --ignored`
    #[test]
    #[ignore]
    fn citrus_window_edits() {
        let Some(dir) = std::env::var_os("OS_SHOT_DIR") else { return };
        let dir = std::path::PathBuf::from(dir);
        let mut pl = Plugins { available: builtin(), scale: 1.0, rate: 48000, ..Plugins::default() };
        let mut host = crate::editor_host::EditorHost::default();
        let index = pl.index_of(crate::citrus::CITRUS).expect("built-in Citrus");
        pl.create_with(0, index, None).unwrap();
        let info = ChannelInfo { name: crate::citrus::CITRUS.into(), target: "Bus 1".into(), enabled: true, volume: 0.78, pan: 0.0, pitch_cents: 0, range: 2, track: 1, max_track: 125, misc: None, icon: None };
        host.open_window(&mut pl, 0, info).unwrap();
        pump(&mut host, &mut pl, 400);
        let frame = host.editors[&0].window.as_ref().unwrap().hwnd();
        let editor = unsafe { GetWindow(GetWindow(frame as HWND, GW_CHILD), GW_CHILD) } as isize;
        assert!(editor != 0, "the editor is a child of the frame");
        let mut er: RECT = unsafe { std::mem::zeroed() };
        unsafe { GetWindowRect(editor as HWND, &mut er) };
        assert_eq!((er.right - er.left, er.bottom - er.top), (citrus::view::W as i32, citrus::view::H as i32));
        let (a, b) = (dir.join("citrus_before.bmp"), dir.join("citrus_after.bmp"));
        capture(frame, &a);
        // The default patch saves no state until it is edited.
        assert_eq!(pl.sytrus_file_state(0), Some(Vec::new()));
        for _ in 0..3 {
            let (sx, sy) = (er.left + 524, er.top + 261);
            unsafe { SendMessageW(editor as HWND, WM_MOUSEWHEEL, (120usize) << 16, ((sx & 0xffff) | (sy << 16)) as isize) };
            pump(&mut host, &mut pl, 60);
        }
        let (cx, cy) = citrus::view::cell_centre(0, citrus::state::COL_OUT);
        unsafe { SendMessageW(editor as HWND, WM_RBUTTONDOWN, 0, ((cx as isize) | ((cy as isize) << 16)) as isize) };
        unsafe { SendMessageW(editor as HWND, WM_RBUTTONUP, 0, ((cx as isize) | ((cy as isize) << 16)) as isize) };
        pump(&mut host, &mut pl, 300);
        capture(frame, &b);
        let st = citrus::state::State::from_bytes(&pl.sytrus_file_state(0).expect("a Sytrus state")).expect("an edited state");
        assert_eq!(st.soften, 12, "three wheel notches of 4");
        assert_eq!(st.cell_off[citrus::state::COL_OUT], 1, "the output cell turned off");
        assert_ne!(std::fs::read(&a).unwrap(), std::fs::read(&b).unwrap(), "the editor redrew");
    }

    /// The built-in Smith's editor in its window: the wheel on LEVEL and a click on the filter's HP edit the Ogun state
    /// the channel saves (spec: plugins/smith.md).
    /// `OS_SHOT_DIR=.. cargo test -p os-app --lib smith_window_edits -- --ignored`
    #[test]
    #[ignore]
    fn smith_window_edits() {
        let Some(dir) = std::env::var_os("OS_SHOT_DIR") else { return };
        let dir = std::path::PathBuf::from(dir);
        let mut pl = Plugins { available: builtin(), scale: 1.0, rate: 48000, ..Plugins::default() };
        let mut host = crate::editor_host::EditorHost::default();
        let index = pl.index_of(crate::smith::SMITH).expect("built-in Smith");
        pl.create_with(0, index, None).unwrap();
        let info = ChannelInfo { name: crate::smith::SMITH.into(), target: "Bus 1".into(), enabled: true, volume: 0.78, pan: 0.0, pitch_cents: 0, range: 2, track: 1, max_track: 125, misc: None, icon: None };
        host.open_window(&mut pl, 0, info).unwrap();
        pump(&mut host, &mut pl, 400);
        let frame = host.editors[&0].window.as_ref().unwrap().hwnd();
        let editor = unsafe { GetWindow(GetWindow(frame as HWND, GW_CHILD), GW_CHILD) } as isize;
        assert!(editor != 0, "the editor is a child of the frame");
        let mut er: RECT = unsafe { std::mem::zeroed() };
        unsafe { GetWindowRect(editor as HWND, &mut er) };
        assert_eq!((er.right - er.left, er.bottom - er.top), (smith::view::W as i32, smith::view::H as i32));
        let (a, b) = (dir.join("smith_before.bmp"), dir.join("smith_after.bmp"));
        capture(frame, &a);
        // The default patch saves no state until it is edited.
        assert_eq!(pl.ogun_file_state(0), Some(Vec::new()));
        for _ in 0..3 {
            let (sx, sy) = (er.left + 21, er.top + 38);
            unsafe { SendMessageW(editor as HWND, WM_MOUSEWHEEL, (120usize) << 16, ((sx & 0xffff) | (sy << 16)) as isize) };
            pump(&mut host, &mut pl, 60);
        }
        let (cx, cy) = (541isize, 109isize);
        unsafe { SendMessageW(editor as HWND, WM_LBUTTONDOWN, 0, cx | (cy << 16)) };
        unsafe { SendMessageW(editor as HWND, WM_LBUTTONUP, 0, cx | (cy << 16)) };
        pump(&mut host, &mut pl, 300);
        capture(frame, &b);
        let st = smith::state::State::from_bytes(&pl.ogun_file_state(0).expect("an Ogun state")).expect("an edited state");
        assert_eq!(st.ints[smith::state::LEVEL], 160 + 3 * 8, "three wheel notches of 8");
        assert_eq!(st.ints[smith::state::F_TYPE], 3, "the high pass");
        assert_ne!(std::fs::read(&a).unwrap(), std::fs::read(&b).unwrap(), "the editor redrew");
    }

    /// Random clicks, drags and wheel turns on the editor; a probe toggle must keep changing the plugin state.
    /// `OS_SHOT_DIR=.. OS_CLAP=.. cargo test -p os-app plugin_editor_stress -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn plugin_editor_stress() {
        let Some(clap) = std::env::var_os("OS_CLAP") else { return };
        // Right clicks and list buttons open modal menus; the plugin skips them under this switch.
        unsafe { std::env::set_var("LEMONSAMPLER_NO_MODAL", "1") };
        let lib = os_clap::Library::load(std::path::Path::new(&clap)).expect("load clap");
        let desc = lib.plugins[0].clone();
        let mut pl = Plugins { available: vec![Available { lib, desc }], scale: 1.0, ..Plugins::default() };
        let mut host = crate::editor_host::EditorHost::default();
        pl.create(0, 0).unwrap();
        let info = ChannelInfo { name: "Lemon Sampler".into(), target: "Bus 1".into(), enabled: true, volume: 0.78, pan: 0.0, pitch_cents: 0, range: 2, track: 1, max_track: 125, misc: None, icon: None };
        host.open_window(&mut pl, 0, info).unwrap();
        pump(&mut host, &mut pl, 300);
        let frame = host.editors[&0].window.as_ref().unwrap().hwnd();
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
            host.editors.get_mut(&0).unwrap().link.as_mut().unwrap().set_page(0);
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
                    if let Some(l) = &mut host.editors.get_mut(&0).unwrap().link {
                        l.set_page(rnd(2) as usize);
                    }
                }
                _ => {
                    send(WM_MOUSEMOVE, 0, x, y);
                }
            }
            if i % 20 == 0 {
                pump(&mut host, &mut pl, 15);
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
        let lib = os_clap::Library::load(std::path::Path::new(&clap)).expect("load clap");
        let desc = lib.plugins[0].clone();
        let mut pl = Plugins { available: vec![Available { lib, desc }], scale: 1.0, ..Plugins::default() };
        let mut host = crate::editor_host::EditorHost::default();
        pl.create(0, 0).unwrap();
        let info = ChannelInfo { name: "Lemon Sampler".into(), target: "Bus 1".into(), enabled: true, volume: 0.78, pan: 0.0, pitch_cents: 0, range: 2, track: 1, max_track: 125, misc: None, icon: None };
        host.open_window(&mut pl, 0, info).unwrap();
        pump(&mut host, &mut pl, 300);
        let frame = host.editors[&0].window.as_ref().unwrap().hwnd();
        let editor = unsafe { GetWindow(GetWindow(frame as HWND, GW_CHILD), GW_CHILD) };
        unsafe { SetForegroundWindow(frame as HWND) };
        pump(&mut host, &mut pl, 200);
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
        pump(&mut host, &mut pl, 50);
        input(MOUSEEVENTF_LEFTDOWN, 0, 0);
        pump(&mut host, &mut pl, 50);
        let mut worst = 0;
        let t0 = Instant::now();
        for _ in 0..40 {
            input(MOUSEEVENTF_MOVE, 0, -3);
            pump(&mut host, &mut pl, 12);
            let (x, y) = cursor();
            worst = worst.max((x - p.x).abs() + (y - p.y).abs());
        }
        let took = t0.elapsed();
        let hidden = !shown();
        let during = offset(&state(&pl));
        input(MOUSEEVENTF_LEFTUP, 0, 0);
        pump(&mut host, &mut pl, 80);
        let after = cursor();
        println!("offset {before} -> {during}, worst cursor drift {worst} px, cursor hidden while dragging: {hidden}, 40 moves in {took:?}");
        assert!(during > before + 0.01, "the knob did not follow the drag ({before} -> {during})");
        assert!(worst <= 3, "the cursor was not held in place (drift {worst} px)");
        assert!(hidden, "the cursor should be hidden while dragging a knob");
        assert!(shown(), "the cursor should show again after the drag");
        assert_eq!(after, (p.x, p.y), "the cursor should be back at the press point");
    }

    /// Real mouse input on the hosted editor must change what the plugin plays: drag the filter's MOD X knob down with
    /// the real cursor, then render a note through the same instance the engine uses.
    /// `OS_CLAP=.. cargo test -p os-app plugin_knob_edit_changes_the_sound -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn plugin_knob_edit_changes_the_sound() {
        use windows_sys::Win32::Foundation::POINT;
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::{SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MOVE, MOUSEINPUT};
        let Some(clap) = std::env::var_os("OS_CLAP") else { return };
        let lib = os_clap::Library::load(std::path::Path::new(&clap)).expect("load clap");
        let desc = lib.plugins[0].clone();
        let mut pl = Plugins { available: vec![Available { lib, desc }], scale: 1.0, ..Plugins::default() };
        let mut host = crate::editor_host::EditorHost::default();
        pl.create(0, 0).unwrap();
        let info = ChannelInfo { name: "Lemon Sampler".into(), target: "Bus 1".into(), enabled: true, volume: 0.78, pan: 0.0, pitch_cents: 0, range: 2, track: 1, max_track: 125, misc: None, icon: None };
        host.open_window(&mut pl, 0, info).unwrap();
        pump(&mut host, &mut pl, 300);
        // A bright 2 s saw so a low-pass is audible.
        let wav = std::env::temp_dir().join(format!("os-app-knob-sound-{}.wav", std::process::id()));
        let tone: Vec<f32> = (0..88200).flat_map(|i| { let v = ((i as f32 / 44100.0 * 220.0).fract() * 2.0 - 1.0) * 0.4; [v, v] }).collect();
        os_engine::wav::write_f32_stereo(&mut std::fs::File::create(&wav).unwrap(), 44100, &tone).unwrap();
        // The persisted field is a JSON string holding JSON: forward slashes keep it free of escapes.
        let path = format!("\"\\\"{}\\\"\"", wav.to_string_lossy().replace('\\', "/"));
        let st = format!("{{\"version\":\"0.0.1\",\"params\":{{}},\"fields\":{{\"sample-path\":{path}}}}}");
        let mut blob = (st.len() as u64).to_le_bytes().to_vec();
        blob.extend_from_slice(st.as_bytes());
        assert!(pl.hosted[&0].plugin.lock().unwrap().load_state(&blob));
        pump(&mut host, &mut pl, 300);
        let rate = pl.rate;
        let render = |pl: &mut Plugins| -> Vec<f32> {
            let plugin = pl.hosted[&0].plugin.clone();
            let mut p = plugin.lock().unwrap();
            let mut out = Vec::new();
            for b in 0..60 {
                if b == 2 {
                    p.note_on(0, 60, 0.78);
                }
                if b == 40 {
                    p.note_off(0, 60);
                }
                let (mut l, mut r) = (vec![0.0f32; 480], vec![0.0f32; 480]);
                p.process(rate, &mut l, &mut r);
                out.extend(l);
            }
            p.reset();
            out
        };
        // Let the plugin pick up its processed sample first.
        render(&mut pl);
        let before = render(&mut pl);
        assert!(before.iter().any(|v| v.abs() > 0.05), "the sample plays");
        host.editors.get_mut(&0).unwrap().link.as_mut().unwrap().set_page(1);
        pump(&mut host, &mut pl, 100);
        let frame = host.editors[&0].window.as_ref().unwrap().hwnd();
        let editor = unsafe { GetWindow(GetWindow(frame as HWND, GW_CHILD), GW_CHILD) };
        unsafe { SetForegroundWindow(frame as HWND) };
        pump(&mut host, &mut pl, 200);
        // Filter MOD X: FL (492.5, 168.5) -> editor client (488, 96) with the host's strip row.
        let mut p = POINT { x: 488, y: 96 };
        unsafe { ClientToScreen(editor, &mut p) };
        let input = |flags: u32, dx: i32, dy: i32| unsafe {
            let i = INPUT { r#type: INPUT_MOUSE, Anonymous: INPUT_0 { mi: MOUSEINPUT { dx, dy, mouseData: 0, dwFlags: flags, time: 0, dwExtraInfo: 0 } } };
            SendInput(1, &i, std::mem::size_of::<INPUT>() as i32);
        };
        unsafe { SetCursorPos(p.x, p.y) };
        pump(&mut host, &mut pl, 50);
        input(MOUSEEVENTF_LEFTDOWN, 0, 0);
        pump(&mut host, &mut pl, 50);
        for _ in 0..60 {
            input(MOUSEEVENTF_MOVE, 0, 4);
            pump(&mut host, &mut pl, 10);
        }
        input(MOUSEEVENTF_LEFTUP, 0, 0);
        pump(&mut host, &mut pl, 200);
        let state = String::from_utf8_lossy(&pl.hosted[&0].plugin.lock().unwrap().save_state().unwrap()).to_string();
        let i = state.find("\"f_cut\"").unwrap_or(0);
        println!("state: {}", &state[i..(i + 40).min(state.len())]);
        let after = render(&mut pl);
        let diff = before.iter().zip(&after).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
        let rms = |v: &[f32]| (v.iter().map(|x| x * x).sum::<f32>() / v.len() as f32).sqrt();
        println!("max diff {diff}, rms {} -> {}", rms(&before), rms(&after));
        assert!(diff > 0.05, "the MOD X drag did not change the sound");
    }

    /// The app's own path (needs an audio device): a plugin channel on the transport's live stream, a held note, a real
    /// mouse drag of MOD X in the editor; the output level must follow.
    /// `OS_CLAP=.. cargo test -p os-app app_hears_plugin_knob_edits -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn app_hears_plugin_knob_edits() {
        use windows_sys::Win32::Foundation::POINT;
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::{SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MOVE, MOUSEINPUT};
        let Some(clap) = std::env::var_os("OS_CLAP") else { return };
        let lib = os_clap::Library::load(std::path::Path::new(&clap)).expect("load clap");
        let desc = lib.plugins[0].clone();
        let mut pl = Plugins { available: vec![Available { lib, desc }], scale: 1.0, ..Plugins::default() };
        let mut host = crate::editor_host::EditorHost::default();
        pl.create(0, 0).unwrap();
        let info = ChannelInfo { name: "Lemon Sampler".into(), target: "Bus 1".into(), enabled: true, volume: 0.78, pan: 0.0, pitch_cents: 0, range: 2, track: 1, max_track: 125, misc: None, icon: None };
        host.open_window(&mut pl, 0, info).unwrap();
        pump(&mut host, &mut pl, 300);
        // A bright 2 s saw so a low-pass is audible.
        let wav = std::env::temp_dir().join(format!("os-app-app-knob-sound-{}.wav", std::process::id()));
        let tone: Vec<f32> = (0..88200).flat_map(|i| { let v = ((i as f32 / 44100.0 * 220.0).fract() * 2.0 - 1.0) * 0.4; [v, v] }).collect();
        os_engine::wav::write_f32_stereo(&mut std::fs::File::create(&wav).unwrap(), 44100, &tone).unwrap();
        // The persisted field is a JSON string holding JSON: forward slashes keep it free of escapes.
        let path = format!("\"\\\"{}\\\"\"", wav.to_string_lossy().replace('\\', "/"));
        let st = format!("{{\"version\":\"0.0.1\",\"params\":{{}},\"fields\":{{\"sample-path\":{path}}}}}");
        let mut blob = (st.len() as u64).to_le_bytes().to_vec();
        blob.extend_from_slice(st.as_bytes());
        assert!(pl.hosted[&0].plugin.lock().unwrap().load_state(&blob));
        pump(&mut host, &mut pl, 300);
        let mut project = crate::projects::test_project();
        project.channels.clear();
        let row = crate::projects::add_plugin_channel(&mut project, "Lemon Sampler");
        assert_eq!(row, 0);
        let transport = crate::transport::Transport::new(None);
        transport.update(&project);
        transport.set_instruments(pl.instruments());
        // Hold a note 400 ms, keep the UI side ticking, return the loudest output peak.
        let level = |pl: &mut Plugins| -> f32 {
            transport.take_peaks();
            transport.note_on(0, 60, 100);
            let mut peak = 0.0f32;
            let t0 = Instant::now();
            while t0.elapsed() < Duration::from_millis(400) {
                pump(pl, 20);
                let (l, r) = transport.take_peaks();
                peak = peak.max(l.max(r));
            }
            transport.note_off(0, 60);
            pump(pl, 300);
            peak
        };
        level(&mut pl);
        let before = level(&mut pl);
        println!("level before {before}");
        assert!(before > 0.05, "the note is heard");
        host.editors.get_mut(&0).unwrap().link.as_mut().unwrap().set_page(1);
        pump(&mut host, &mut pl, 100);
        let frame = host.editors[&0].window.as_ref().unwrap().hwnd();
        let editor = unsafe { GetWindow(GetWindow(frame as HWND, GW_CHILD), GW_CHILD) };
        unsafe { SetForegroundWindow(frame as HWND) };
        pump(&mut host, &mut pl, 200);
        // Filter MOD X: FL (492.5, 168.5) -> editor client (488, 96) with the host's strip row.
        let mut p = POINT { x: 488, y: 96 };
        unsafe { ClientToScreen(editor, &mut p) };
        let input = |flags: u32, dx: i32, dy: i32| unsafe {
            let i = INPUT { r#type: INPUT_MOUSE, Anonymous: INPUT_0 { mi: MOUSEINPUT { dx, dy, mouseData: 0, dwFlags: flags, time: 0, dwExtraInfo: 0 } } };
            SendInput(1, &i, std::mem::size_of::<INPUT>() as i32);
        };
        unsafe { SetCursorPos(p.x, p.y) };
        pump(&mut host, &mut pl, 50);
        input(MOUSEEVENTF_LEFTDOWN, 0, 0);
        pump(&mut host, &mut pl, 50);
        for _ in 0..120 {
            input(MOUSEEVENTF_MOVE, 0, 4);
            pump(&mut host, &mut pl, 10);
        }
        input(MOUSEEVENTF_LEFTUP, 0, 0);
        pump(&mut host, &mut pl, 200);
        let state = String::from_utf8_lossy(&pl.hosted[&0].plugin.lock().unwrap().save_state().unwrap()).to_string();
        let i = state.find("\"f_cut\"").unwrap_or(0);
        println!("state: {}", &state[i..(i + 40).min(state.len())]);
        let after = level(&mut pl);
        println!("level after {after}");
        assert!(after < before * 0.5, "MOD X down did not change what the app plays ({before} -> {after})");
    }

    /// Keys typed while the plugin window has the focus must reach the app (the typing keyboard keeps playing): a CLAP
    /// plugin (`OS_CLAP`) or a VST2 one (`OS_VST2`), whose editor first gets a click at (300, 60).
    #[test]
    #[ignore]
    fn keys_in_a_plugin_window_reach_the_app() {
        let (file, format) = match (std::env::var_os("OS_CLAP"), std::env::var_os("OS_VST2")) {
            (Some(f), _) => (f, os_host::Format::Clap),
            (None, Some(f)) => (f, os_host::Format::Vst2),
            (None, None) => return,
        };
        let lib = load_file(std::path::Path::new(&file), format).expect("load");
        let desc = lib.plugins()[0].clone();
        let name = desc.name.clone();
        let mut pl = Plugins { available: vec![Available { lib, desc }], scale: 1.0, ..Plugins::default() };
        let mut host = crate::editor_host::EditorHost::default();
        pl.create(0, 0).unwrap();
        let info = ChannelInfo { name, target: "Bus 1".into(), enabled: true, volume: 0.78, pan: 0.0, pitch_cents: 0, range: 2, track: 1, max_track: 125, misc: None, icon: None };
        host.open_window(&mut pl, 0, info).unwrap();
        pump(&mut host, &mut pl, 300);
        let frame = host.editors[&0].window.as_ref().unwrap().hwnd();
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
        pump(&mut host, &mut pl, 100);
        let keys = crate::plugin_window::take_keys();
        assert_eq!(keys.len(), 2, "{keys:?}");
        assert!(keys[0].pressed && !keys[1].pressed && keys[0].scancode == 0x10);
    }

    /// A VST2 plugin's editor in a real plugin window at 100, 150 and 200 % (spec: ui/ui-scaling.md §3): the frame's
    /// editor area must fit the editor's own window at each scale (neither clipped nor left with a gap), and a real
    /// mouse drag in it (`OS_VST2_DRAG=x,y` in editor pixels as it first opens, default the middle) must reach the app
    /// as an edit. The window first opens at the monitor's scale, as in the app (whose DPI awareness the test takes).
    /// Moves the desktop cursor. Screenshots go to `OS_SHOT_DIR` when it is set.
    /// `OS_VST2=<plugin dll> cargo test -p os-app --lib vst2_editor_scales -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn vst2_editor_scales() {
        use windows_sys::Win32::Foundation::POINT;
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::{SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MOVE, MOUSEINPUT};
        let Some(file) = std::env::var_os("OS_VST2") else { return };
        let dir = std::env::var_os("OS_SHOT_DIR").map(std::path::PathBuf::from);
        let drag = std::env::var("OS_VST2_DRAG").ok().and_then(|s| s.split_once(',').and_then(|(x, y)| Some((x.trim().parse::<i32>().ok()?, y.trim().parse::<i32>().ok()?))));
        // Per-monitor DPI awareness, as winit gives the app: plugins see the monitor's own DPI, as they do there.
        #[link(name = "user32")]
        unsafe extern "system" {
            fn SetProcessDpiAwarenessContext(value: isize) -> i32;
            fn GetDpiForSystem() -> u32;
        }
        const PER_MONITOR_AWARE_V2: isize = -4;
        unsafe { SetProcessDpiAwarenessContext(PER_MONITOR_AWARE_V2) };
        let monitor = unsafe { GetDpiForSystem() } as f32 / 96.0;
        let lib = load_file(std::path::Path::new(&file), os_host::Format::Vst2).expect("load");
        let desc = lib.plugins()[0].clone();
        let name = desc.name.clone();
        let mut pl = Plugins { available: vec![Available { lib, desc }], scale: monitor, rate: 48000, ..Plugins::default() };
        let mut host = crate::editor_host::EditorHost::default();
        pl.create(0, 0).unwrap();
        let info = ChannelInfo { name: name.clone(), target: "Bus 1".into(), enabled: true, volume: 0.78, pan: 0.0, pitch_cents: 0, range: 2, track: 1, max_track: 125, misc: None, icon: None };
        host.open_window(&mut pl, 0, info).unwrap();
        pump(&mut host, &mut pl, 500);
        let size = |h: HWND| unsafe {
            let mut r: RECT = std::mem::zeroed();
            GetWindowRect(h, &mut r);
            (r.right - r.left, r.bottom - r.top)
        };
        let handles = |pl: &Plugins| {
            let w = host.editors[&0].window.as_ref().unwrap();
            let area = w.area() as HWND;
            (w.hwnd() as HWND, area, unsafe { GetWindow(area, GW_CHILD) })
        };
        let shot = |pl: &Plugins, tag: &str| {
            if let Some(d) = &dir {
                let stem = name.replace(|c: char| !c.is_ascii_alphanumeric(), "_");
                capture(handles(pl).0 as isize, &d.join(format!("vst2_{stem}_{tag}.bmp")));
            }
        };
        let (frame, _, editor) = handles(&pl);
        assert!(!editor.is_null(), "the editor is a child of the frame's editor area");
        // A drag at the monitor's scale: what the editor reports is what "last tweaked" and automation recording hear.
        let (ew, eh) = size(editor);
        let (x, y) = drag.unwrap_or((ew / 2, eh / 2));
        let mut p = POINT { x, y };
        unsafe {
            ClientToScreen(editor, &mut p);
            SetForegroundWindow(frame);
        }
        pump(&mut host, &mut pl, 200);
        let input = |flags: u32, dx: i32, dy: i32| unsafe {
            let i = INPUT { r#type: INPUT_MOUSE, Anonymous: INPUT_0 { mi: MOUSEINPUT { dx, dy, mouseData: 0, dwFlags: flags, time: 0, dwExtraInfo: 0 } } };
            SendInput(1, &i, std::mem::size_of::<INPUT>() as i32);
        };
        let _ = pl.hosted[&0].plugin.lock().unwrap().take_param_events();
        unsafe { SetCursorPos(p.x, p.y) };
        pump(&mut host, &mut pl, 50);
        input(MOUSEEVENTF_LEFTDOWN, 0, 0);
        pump(&mut host, &mut pl, 50);
        for _ in 0..40 {
            input(MOUSEEVENTF_MOVE, 0, -3);
            pump(&mut host, &mut pl, 12);
        }
        input(MOUSEEVENTF_LEFTUP, 0, 0);
        pump(&mut host, &mut pl, 200);
        let edits = pl.hosted[&0].plugin.lock().unwrap().take_param_events();
        println!("{name}: opened at the monitor's {monitor} as {ew} x {eh}; a drag at ({x}, {y}) gave {} edits, starting {:?}", edits.len(), &edits[..edits.len().min(3)]);
        shot(&pl, "monitor");
        if drag.is_some() {
            assert!(!edits.is_empty(), "the drag reached no parameter");
        }
        let mut base = None;
        for k in [1.0f32, 1.5, 2.0] {
            host.rescale_window(&mut pl, 0, k);
            pump(&mut host, &mut pl, 600);
            let (_, area, editor) = handles(&pl);
            let (e, a) = (size(editor), size(area));
            let reported = pl.hosted[&0].plugin.lock().unwrap().editor_size();
            let b = *base.get_or_insert(e);
            println!("{name} at {k}: editor window {e:?} ({:.2} x its size at 1), editor area {a:?}, reported {reported:?}", e.0 as f32 / b.0 as f32);
            shot(&pl, &format!("{}", (k * 100.0) as u32));
            assert_eq!(a, e, "at {k} the editor area fits the editor's window");
        }
    }

    fn fixture_mp3() -> String {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../os-engine/tests/data/click_sine.mp3");
        p.canonicalize().unwrap().to_string_lossy().trim_start_matches(r"\\?\").to_string()
    }

    /// Main swing delays an off-beat step played by a hosted Lemon Sampler (spec: flp/events/project-info.md "Swing timing").
    #[test]
    fn swing_reaches_hosted_sampler() {
        let wav = std::env::temp_dir().join(format!("os-swing-click-{}.wav", std::process::id()));
        let click: Vec<f32> = (0..48000).flat_map(|i| { let v = if i < 200 { 0.8 } else { 0.0 }; [v, v] }).collect();
        os_engine::wav::write_f32_stereo(&mut std::fs::File::create(&wav).unwrap(), 48000, &click).unwrap();
        let onsets = |swing: u8| -> Vec<usize> {
            let mut pl = Plugins { available: builtin(), scale: 1.0, rate: 48000, ..Plugins::default() };
            let mut host = crate::editor_host::EditorHost::default();
            let index = pl.index_of(LEMON_SAMPLER).unwrap();
            let mut p = crate::projects::test_project();
            p.channels.truncate(1);
            p.patterns = vec![os_model::Pattern { number: 1, notes: vec![os_model::Note::step(0, 0), os_model::Note::step(0, 24)], ..Default::default() }];
            p.swing = Some(swing);
            pl.create_with(0, index, Some(&sampler_state(&p.channels[0]))).unwrap();
            assert!(pl.load_sample(0, &wav.to_string_lossy()));
            let inst = pl.hosted[&0].plugin.clone() as SharedInstrument;
            let mut lib = os_engine::SampleLibrary::default();
            let mut e = os_engine::Engine::new(&p, &mut lib, 48000, os_engine::PlayMode::Pattern(1));
            e.set_instrument(0, inst);
            let mut out = Vec::new();
            let mut buf = vec![0.0f32; 480 * 2];
            for _ in 0..40 {
                e.process(&mut buf);
                out.extend(buf.iter().step_by(2).copied());
            }
            let mut on = Vec::new();
            let mut quiet = 1000;
            for (i, v) in out.iter().enumerate() {
                if v.abs() > 0.05 {
                    if quiet > 500 {
                        on.push(i);
                    }
                    quiet = 0;
                } else {
                    quiet += 1;
                }
            }
            on
        };
        let straight = onsets(0);
        let swung = onsets(128);
        let _ = std::fs::remove_file(&wav);
        eprintln!("straight {straight:?} swung {swung:?}");
        assert_eq!(straight.len(), 2);
        assert_eq!(swung.len(), 2);
        // Half a step at 130 BPM, 48 kHz: 24 ticks = 5538 frames, so about 2769 frames later.
        let shift = swung[1] as i64 - straight[1] as i64;
        assert!((shift - 2769).abs() < 20, "{shift}");
    }

    /// Lemon Sampler is compiled in: no `.clap` file is needed, and a sample loads into a running instance without
    /// touching its other settings.
    #[test]
    fn builtin_lemon_sampler_takes_a_sample() {
        let mut pl = Plugins { available: builtin(), scale: 1.0, rate: 48000, ..Plugins::default() };
        let mut host = crate::editor_host::EditorHost::default();
        let index = pl.index_of(LEMON_SAMPLER).expect("built-in Lemon Sampler");
        let mut ch = crate::projects::test_project().channels.remove(0);
        ch.envelopes[1].enabled = true;
        ch.envelopes[1].attack = 65536 / 2;
        pl.create_with(0, index, Some(&sampler_state(&ch))).unwrap();
        assert_eq!(pl.sampler_path(0).as_deref(), Some(""));
        let mp3 = fixture_mp3();
        assert!(pl.load_sample(0, &mp3));
        assert_eq!(pl.sampler_path(0).as_deref(), Some(mp3.as_str()));
        let st = pl.hosted[&0].plugin.lock().unwrap().save_state().unwrap();
        let v: serde_json::Value = serde_json::from_slice(&st[8..]).unwrap();
        assert_eq!(v["params"]["envvol_on"], serde_json::json!({"bool": true}), "other settings kept");
        // And it sounds.
        let inst = pl.hosted[&0].plugin.clone() as SharedInstrument;
        let mut i = inst.lock().unwrap();
        i.note_on(0, 60, 0.8);
        let mut peak = 0.0f32;
        for _ in 0..50 {
            let (mut l, mut r) = (vec![0.0f32; 480], vec![0.0f32; 480]);
            i.process(48000, &mut l, &mut r);
            peak = l.iter().fold(peak, |m, v| m.max(v.abs()));
        }
        // The 0.5 s volume attack kept from the channel still fades the click (0.9 at 0.1 s) in.
        assert!(peak > 0.01 && peak < 0.3, "the mp3 plays through the kept envelope (peak {peak})");
    }

    /// Portamento and slide notes reach the built-in synths through the CLAP host as a key's tuning (spec:
    /// audio/channel-voices.md): a note tuned an octave up sounds twice as high, and back at 0 as it started.
    #[test]
    fn builtin_synths_follow_the_hosts_tuning() {
        for name in [crate::osc3::LEMON_TRIO, crate::citrus::CITRUS] {
            let mut pl = Plugins { available: builtin(), scale: 1.0, rate: 48000, ..Plugins::default() };
            let mut host = crate::editor_host::EditorHost::default();
            let index = pl.index_of(name).unwrap();
            pl.create_with(0, index, None).unwrap();
            let inst = pl.hosted[&0].plugin.clone() as SharedInstrument;
            let mut i = inst.lock().unwrap();
            // Rising zero crossings per second over 0.2 s.
            let rate = |i: &mut dyn os_engine::Instrument| {
                let mut all = Vec::new();
                for _ in 0..20 {
                    let (mut l, mut r) = (vec![0.0f32; 480], vec![0.0f32; 480]);
                    i.process(48000, &mut l, &mut r);
                    all.extend(l);
                }
                all.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count() as f64 / 0.2
            };
            i.note_on(0, 60, 0.8);
            rate(&mut *i);
            let plain = rate(&mut *i);
            i.tune(0, 60, 12.0);
            rate(&mut *i);
            let up = rate(&mut *i);
            i.tune(0, 60, 0.0);
            rate(&mut *i);
            let back = rate(&mut *i);
            assert!(plain > 50.0, "{name} sounds ({plain} Hz)");
            assert!((up / plain - 2.0).abs() < 0.1, "{name}: an octave up ({plain} -> {up})");
            assert!((back / plain - 1.0).abs() < 0.05, "{name}: back ({plain} -> {back})");
        }
    }

    /// A mono + portamento channel played by Lemon Trio (Phanty's leads): the engine's glide steps reach the synth, so
    /// an overlapping note an octave up moves the sounding voice there over the slide time.
    // spec: audio/note-rules.md "Portamento", audio/channel-voices.md
    #[test]
    fn lemon_trio_glides_on_a_mono_portamento_channel() {
        let mut pl = Plugins { available: builtin(), scale: 1.0, rate: 48000, ..Plugins::default() };
        let mut host = crate::editor_host::EditorHost::default();
        pl.create_with(0, pl.index_of(crate::osc3::LEMON_TRIO).unwrap(), None).unwrap();
        let inst = pl.hosted[&0].plugin.clone() as SharedInstrument;
        let misc = os_model::ChannelMisc { mono: true, porta: true, slide: 1.0, ..Default::default() };
        let mut ch = crate::projects::test_project().channels.remove(0);
        (ch.kind, ch.plugin, ch.misc, ch.sample_path) = (os_model::ChannelKind::Plugin, Some(crate::osc3::FILE_NAME.into()), misc, None);
        let project = os_model::Project { ppq: 96, tempo: Some(130.0), channels: vec![ch], ..Default::default() };
        let mut slib = os_engine::SampleLibrary::default();
        let mut e = os_engine::Engine::new(&project, &mut slib, 48000, os_engine::PlayMode::Pattern(1));
        e.clear_schedule();
        e.set_instrument(0, inst);
        let block = 480;
        let mut out = Vec::new();
        for b in 0..(48000 * 3 / block) {
            match b {
                10 => e.note_on(0, 48, 100),
                100 => e.note_on(0, 60, 100),
                _ => {}
            }
            let mut buf = vec![0.0f32; block * 2];
            e.process(&mut buf);
            out.extend(buf.chunks_exact(2).map(|f| f[0]));
        }
        let rate = |from: f32, to: f32| {
            let w = &out[(from * 48000.0) as usize..(to * 48000.0) as usize];
            w.windows(2).filter(|p| p[0] <= 0.0 && p[1] > 0.0).count() as f32 / (to - from)
        };
        let glide_s = (os_engine::laws::glide_ticks(1.0, 96) + 1) as f32 * 60.0 / (130.0 * 96.0);
        let (before, mid, after) = (rate(0.5, 0.95), rate(1.0 + glide_s * 0.4, 1.0 + glide_s * 0.6), rate(1.0 + glide_s + 0.05, 2.0));
        assert!(before > 50.0, "sounds: {before}");
        assert!((after / before - 2.0).abs() < 0.05, "glides to the octave: {before} -> {after}");
        assert!(mid / before > 1.15 && mid / before < 1.85, "between the keys mid-glide: {before} -> {mid}");
    }

    /// Hosted instruments' voices (spec: audio/voices.md §2-§5): Lemon Sampler counts a voice until its sample ends (the
    /// plugin reports the end), Lemon Trio until its release is over; a reset (Stop) ends every voice at once. The
    /// counts reach `Plugins::voices` through the instances' meters, without locking them.
    #[test]
    fn hosted_instrument_voices_follow_the_plugins() {
        let mut pl = Plugins { available: builtin(), scale: 1.0, rate: 48000, ..Plugins::default() };
        let mut host = crate::editor_host::EditorHost::default();
        let sampler = pl.index_of(LEMON_SAMPLER).unwrap();
        pl.create_with(0, sampler, None).unwrap();
        // A tenth of a second of sound.
        let wav = std::env::temp_dir().join(format!("os-app-voices-{}.wav", std::process::id()));
        let frames: Vec<f32> = (0..4800).flat_map(|_| [0.25f32; 2]).collect();
        os_engine::wav::write_f32_stereo(&mut std::fs::File::create(&wav).unwrap(), 48000, &frames).unwrap();
        assert!(pl.load_sample(0, &wav.to_string_lossy()));
        let osc = pl.index_of(crate::osc3::LEMON_TRIO).unwrap();
        pl.create_with(1, osc, None).unwrap();
        let run = |pl: &Plugins, key: usize, blocks: usize| {
            let inst = pl.hosted[&key].plugin.clone() as SharedInstrument;
            let mut i = inst.lock().unwrap();
            for _ in 0..blocks {
                let (mut l, mut r) = (vec![0.0f32; 480], vec![0.0f32; 480]);
                i.process(48000, &mut l, &mut r);
            }
        };
        let with = |pl: &Plugins, key: usize, f: &dyn Fn(&mut dyn os_engine::Instrument)| {
            let inst = pl.hosted[&key].plugin.clone() as SharedInstrument;
            f(&mut *inst.lock().unwrap());
        };
        assert_eq!(pl.voices(), 0);
        // Three sampler notes and a chord on the synth.
        with(&pl, 0, &|i| (60..63).for_each(|k| i.note_on(0, k, 0.8)));
        with(&pl, 1, &|i| [48, 52, 55].into_iter().for_each(|k| i.note_on(0, k, 0.8)));
        run(&pl, 0, 1);
        run(&pl, 1, 1);
        assert_eq!(pl.voices(), 6, "3 sampler voices + 3 synth voices");
        // The sample ends while the keys are still held: its voices end with it.
        run(&pl, 0, 20);
        assert_eq!(pl.hosted[&0].meter.as_ref().unwrap().voices(), 0, "the sampler's voices ended with the sample");
        assert_eq!(pl.voices(), 3);
        // Released: the synth's voices ring out, then end.
        with(&pl, 1, &|i| i.note_off(0, 48));
        run(&pl, 1, 200);
        assert_eq!(pl.voices(), 2, "the released voice ended after its release");
        // Stop resets the instruments: nothing sounds.
        with(&pl, 0, &|i| i.note_on(0, 60, 0.8));
        run(&pl, 0, 1);
        assert_eq!(pl.voices(), 3);
        for key in [0, 1] {
            with(&pl, key, &|i| i.reset());
        }
        assert_eq!(pl.voices(), 0, "Stop ends every voice");
        let _ = std::fs::remove_file(&wav);
    }

    /// A channel's stretch settings reach its Lemon Sampler (spec: audio/time-stretch.md "Fields"), which plays them:
    /// in Stretch, twice as long with the pitch moved by PITCH alone.
    #[test]
    fn builtin_lemon_sampler_takes_the_stretch_settings() {
        let mut pl = Plugins { available: builtin(), scale: 1.0, rate: 48000, ..Plugins::default() };
        let mut host = crate::editor_host::EditorHost::default();
        let index = pl.index_of(LEMON_SAMPLER).expect("built-in Lemon Sampler");
        let wav = std::env::temp_dir().join(format!("os-app-sampler-stretch-{}.wav", std::process::id()));
        let tone: Vec<f32> = (0..48000)
            .flat_map(|i| {
                let v = ((f64::from(i) * 440.0 * std::f64::consts::TAU / 48000.0).sin() * 0.5) as f32;
                [v, v]
            })
            .collect();
        os_engine::wav::write_f32_stereo(&mut std::fs::File::create(&wav).unwrap(), 48000, &tone).unwrap();
        let mut ch = crate::projects::test_project().channels.remove(0);
        // Stretch (a realtime method), twice as long, 300 cents up.
        ch.sound.stretch = [0, 300, 10000, (-1i32) as u32];
        pl.create_with(0, index, Some(&sampler_state(&ch))).unwrap();
        assert!(pl.load_sample(0, &wav.to_string_lossy()));
        let params = |pl: &Plugins| {
            let st = pl.hosted[&0].plugin.lock().unwrap().save_state().unwrap();
            serde_json::from_slice::<serde_json::Value>(&st[8..]).unwrap()["params"].clone()
        };
        let f = |v: &serde_json::Value| v["f32"].as_f64().unwrap();
        let p = params(&pl);
        assert_eq!(p["ts_mode"], serde_json::json!({"i32": 1}), "Stretch");
        assert!((f(&p["ts_mul"]) - 0.75).abs() < 1e-6, "x2");
        assert!((f(&p["ts_pitch"]) - 0.625).abs() < 1e-6, "+300 cents");
        let mut out = Vec::new();
        {
            let inst = pl.hosted[&0].plugin.clone() as SharedInstrument;
            let mut i = inst.lock().unwrap();
            i.note_on(0, 60, 0.8);
            for _ in 0..250 {
                let (mut l, mut r) = (vec![0.0f32; 480], vec![0.0f32; 480]);
                i.process(48000, &mut l, &mut r);
                out.extend_from_slice(&l);
            }
        }
        let mid = &out[24000..72000];
        let ups: Vec<usize> = mid.windows(2).enumerate().filter(|(_, w)| w[0] < 0.0 && w[1] >= 0.0).map(|(i, _)| i).collect();
        let hz = (ups.len() - 1) as f64 * 48000.0 / (ups[ups.len() - 1] - ups[0]) as f64;
        assert!((hz - 440.0 * 2f64.powf(0.25)).abs() < 4.0, "{hz} Hz");
        let late = &out[80000..92000];
        assert!(late.iter().map(|v| v * v).sum::<f32>() / late.len() as f32 > 0.0025, "still playing after the sample's own length");
        // A stretch time set from the playlist: 4 beats = 16 steps, the multiplier back at 1.
        let mut sound = ch.sound;
        sound.set_stretch_time(4.0);
        assert!(pl.set_sampler_stretch(0, &sound));
        let p = params(&pl);
        assert!((lemon_sampler::params::time_steps(f(&p["ts_time"]) as f32) - 16.0).abs() < 1e-3);
        assert!((f(&p["ts_mul"]) - 0.5).abs() < 1e-6);
        assert_eq!(p["ts_mode"], serde_json::json!({"i32": 1}), "the method kept");
        let _ = std::fs::remove_file(&wav);
    }

    /// A channel's sample settings reach its Lemon Sampler (spec: audio/precomputed-effects.md "Settings",
    /// audio/channel-filter.md, audio/declick.md): the knobs stand where the file's values put them, the file's own
    /// values travel whole, and the sample is played as they build it.
    #[test]
    fn builtin_lemon_sampler_takes_the_sample_settings() {
        use lemon_sampler::params::fx_from_text;
        use os_model::SampleFx;
        let mut pl = Plugins { available: builtin(), scale: 1.0, rate: 48000, ..Plugins::default() };
        let mut host = crate::editor_host::EditorHost::default();
        let index = pl.index_of(LEMON_SAMPLER).expect("built-in Lemon Sampler");
        let saved = |pl: &Plugins, row| {
            let st = pl.hosted[&row].plugin.lock().unwrap().save_state().unwrap();
            serde_json::from_slice::<serde_json::Value>(&st[8..]).unwrap()
        };
        let f = |v: &serde_json::Value| v["f32"].as_f64().unwrap();
        let mut ch = crate::projects::test_project().channels.remove(0);
        let fx = SampleFx {
            flags: SampleFx::REVERSE | SampleFx::SWAP,
            boost: 128,
            stereo_delay: 1024,
            trim: 32768,
            normalize: true,
            start_bits: 0.25f64.to_bits(),
            loop_follows_reverse: true,
            ..SampleFx::default()
        };
        ch.sound.fx = fx;
        (ch.sound.cutoff, ch.sound.resonance, ch.sound.filter_type, ch.sound.declick, ch.sound.sampler_flags) = (128, 64, 3, 2, Some(2));
        pl.create_with(0, index, Some(&sampler_state(&ch))).unwrap();
        let v = saved(&pl, 0);
        let p = &v["params"];
        assert_eq!((&p["fx_reverse"], &p["fx_swap"], &p["fx_normalize"]), (&serde_json::json!({"bool": true}), &serde_json::json!({"bool": true}), &serde_json::json!({"bool": true})));
        assert_eq!((&p["fx_clip"], &p["fx_fade_stereo"]), (&serde_json::json!({"bool": false}), &serde_json::json!({"bool": false})));
        assert!((f(&p["fx_boost"]) - 0.5).abs() < 1e-6 && (f(&p["fx_sdelay"]) + 0.5).abs() < 1e-6 && f(&p["fx_trim"]) == 1.0);
        assert!((f(&p["fx_start"]) - 0.25).abs() < 1e-9 && f(&p["fx_length"]) == 1.0);
        assert!((f(&p["f_cut"]) - 0.5).abs() < 1e-9 && (f(&p["f_res"]) - 0.25).abs() < 1e-9);
        assert_eq!((&p["f_type"], &p["declick"]), (&serde_json::json!({"i32": 3}), &serde_json::json!({"i32": 2})));
        assert_eq!(p["loop_use"], serde_json::json!({"bool": false}), "sampler flags without 8");
        // The file's values whole: the trim threshold beyond the knob's range is kept.
        let field: String = serde_json::from_str(v["fields"]["file-settings"].as_str().unwrap()).unwrap();
        assert_eq!(fx_from_text(&field), Some(fx));

        // The sound: reversed and swapped, nothing else.
        let wav = std::env::temp_dir().join(format!("os-app-sampler-fx-{}.wav", std::process::id()));
        let frames: Vec<f32> = (0..1000).flat_map(|i| [i as f32 / 1000.0, -(i as f32) / 2000.0]).collect();
        os_engine::wav::write_f32_stereo(&mut std::fs::File::create(&wav).unwrap(), 48000, &frames).unwrap();
        let mut ch = crate::projects::test_project().channels.remove(0);
        ch.sound.fx = SampleFx { flags: SampleFx::REVERSE | SampleFx::SWAP, ..SampleFx::default() };
        pl.create_with(1, index, Some(&sampler_state(&ch))).unwrap();
        assert!(pl.load_sample(1, &wav.to_string_lossy()));
        let inst = pl.hosted[&1].plugin.clone() as SharedInstrument;
        let mut i = inst.lock().unwrap();
        let (mut l, mut r) = (vec![0.0f32; 480], vec![0.0f32; 480]);
        for _ in 0..20 {
            // The processed sample is built in the background: wait for it.
            i.process(48000, &mut l, &mut r);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        i.note_on(0, 60, 100.0 / 128.0);
        let (mut l, mut r) = (vec![0.0f32; 480], vec![0.0f32; 480]);
        i.process(48000, &mut l, &mut r);
        assert!((l[0] + 999.0 / 2000.0).abs() < 1e-6 && (r[0] - 0.999).abs() < 1e-6, "{} {}", l[0], r[0]);
        assert!((l[10] + 989.0 / 2000.0).abs() < 1e-6 && (r[10] - 0.989).abs() < 1e-6, "{} {}", l[10], r[10]);
        drop(i);
        let _ = std::fs::remove_file(&wav);
    }

    /// Lemon Sampler's root note page edits reach the channel as they happen (`misc_param`): the engine plays the main
    /// pitch switch; the key zone's top key is FL's "no zone".
    #[test]
    fn lemon_sampler_root_page_edits_give_the_channel_settings() {
        use lemon_sampler::params::KEY_MAX;
        let mut m = os_model::ChannelMisc::default();
        assert!(misc_param("Affected by the main pitch", 0.0, &mut m) && !m.main_pitch);
        assert!(misc_param("Add root note to key", 1.0, &mut m) && m.add_key);
        assert!(misc_param("Root note", 48.0 / f64::from(KEY_MAX), &mut m) && m.root == 48);
        assert!(misc_param("Fine tuning", 0.4, &mut m) && m.fine == -20);
        assert!(misc_param("Zone low key", 37.0 / f64::from(KEY_MAX), &mut m) && misc_param("Zone high key", 1.0, &mut m));
        assert_eq!(m.key_range, [37, 256]);
        assert!(!misc_param("Pitch shift", 0.5, &mut m));
    }

    /// A Lemon Sampler state that names no sample (an undone load into an empty channel, `history`) leaves it with
    /// none: a note plays nothing.
    #[test]
    fn lemon_sampler_state_without_a_sample_plays_nothing() {
        let mut pl = Plugins { available: builtin(), scale: 1.0, rate: 48000, ..Plugins::default() };
        let mut host = crate::editor_host::EditorHost::default();
        let index = pl.index_of(LEMON_SAMPLER).expect("built-in Lemon Sampler");
        let wav = std::env::temp_dir().join(format!("os-app-sampler-cleared-{}.wav", std::process::id()));
        let tone: Vec<f32> = (0..9600).flat_map(|i| [((i as f32) * 0.0576).sin() * 0.5; 2]).collect();
        os_engine::wav::write_f32_stereo(&mut std::fs::File::create(&wav).unwrap(), 48000, &tone).unwrap();
        pl.create_with(0, index, None).unwrap();
        assert!(pl.load_sample(0, &wav.to_string_lossy()));
        let peak = |pl: &Plugins| {
            let inst = pl.hosted[&0].plugin.clone() as SharedInstrument;
            let mut i = inst.lock().unwrap();
            i.reset();
            i.note_on(0, 60, 0.8);
            let mut m = 0f32;
            for _ in 0..10 {
                let (mut l, mut r) = (vec![0.0f32; 480], vec![0.0f32; 480]);
                i.process(48000, &mut l, &mut r);
                m = l.iter().chain(&r).fold(m, |a, v| a.max(v.abs()));
            }
            m
        };
        assert!(peak(&pl) > 0.05, "the sample plays");
        assert!(pl.set_sampler(0, Some(""), &[]));
        assert_eq!(pl.sampler_path(0).as_deref(), Some(""));
        assert_eq!(peak(&pl), 0.0, "nothing plays");
        let _ = std::fs::remove_file(&wav);
    }

    /// Lemon Sampler's time-stretch parameters read back into the channel's stretch settings (`stretch_param`, the
    /// inverse of `stretch_params`): what the app does with the editor's edits (spec: ui/playlist.md §9c).
    #[test]
    fn lemon_sampler_stretch_parameters_give_the_channel_settings() {
        let mut pl = Plugins { available: builtin(), scale: 1.0, rate: 48000, ..Plugins::default() };
        let mut host = crate::editor_host::EditorHost::default();
        let index = pl.index_of(LEMON_SAMPLER).expect("built-in Lemon Sampler");
        let mut ch = crate::projects::test_project().channels.remove(0);
        let read = |pl: &Plugins| {
            let p = pl.hosted[&0].plugin.lock().unwrap();
            let mut st = [0u32; 4];
            let n = (0..p.param_count()).filter_map(|i| p.param_info(i)).filter(|i| stretch_param(&i.name, p.param_value(i.id).unwrap(), &mut st)).count();
            assert_eq!(n, 4, "the four stretch parameters by name");
            st
        };
        // Stretch, x2, +300 cents.
        ch.sound.stretch = [0, 300, 10000, (-1i32) as u32];
        pl.create_with(0, index, Some(&sampler_state(&ch))).unwrap();
        assert_eq!(read(&pl), ch.sound.stretch);
        // A time of 4 beats (768 / 192), the multiplier back at 1; e2 speech, -1200 cents.
        ch.sound.stretch = [768f32.to_bits(), (-1200i32) as u32, 0, 9];
        assert!(pl.set_sampler_stretch(0, &ch.sound));
        assert_eq!(read(&pl), ch.sound.stretch);
        // Resample, x0.25.
        ch.sound.stretch = [0, 0, (-20000i32) as u32, 0];
        assert!(pl.set_sampler_stretch(0, &ch.sound));
        assert_eq!(read(&pl), ch.sound.stretch);
    }

    /// The built-in Lemon Parametric EQ starts from a project file's EQ state, plays it, and gives the state back
    /// (spec: plugins/lemon-parametric-eq-2.md §8).
    #[test]
    fn builtin_parametric_eq_takes_and_gives_the_file_state() {
        use lemon_parametric_eq_2::params::{default_state, field, plugin_state_from_fl, set_field};
        let mut pl = Plugins { effects: builtin_effects(), scale: 1.0, rate: 48000, ..Plugins::default() };
        let mut host = crate::editor_host::EditorHost::default();
        let index = pl.effect_index(LEMON_EQ).expect("built-in EQ");
        // Band 4 (default frequency: the middle of the scale) at +12 dB.
        let mut fl = default_state();
        set_field(&mut fl, 4, 1200);
        pl.create_effect(3, 2, index, Some(&plugin_state_from_fl(&fl))).unwrap();
        let key = fx_key(3, 2);
        let back = pl.eq_file_state(key).expect("EQ state");
        assert_eq!(field(&back, 4), 1200);
        assert_eq!(back, fl, "the whole file state comes back");
        // A sine at the band's frequency comes out about 4 times louder.
        let hz = os_engine::effects::eq::freq_hz(os_engine::effects::eq::default_freq(3));
        let fx = pl.hosted[&key].plugin.clone() as SharedEffect;
        let mut f = fx.lock().unwrap();
        let mut peak = 0.0f32;
        for block in 0..40 {
            let mut l: Vec<f32> = (0..480).map(|i| (0.1 * (std::f64::consts::TAU * hz * f64::from(block * 480 + i) / 48000.0).sin()) as f32).collect();
            let mut r = l.clone();
            f.process_fx(48000, &mut l, &mut r);
            if block > 20 {
                peak = l.iter().fold(peak, |m, v| m.max(v.abs()));
            }
        }
        assert!((peak - 0.398).abs() < 0.03, "+12 dB at the band (peak {peak})");
        assert_eq!(slot_label(EQ_FILE_NAME), LEMON_EQ);
        assert_eq!(new_slot(LEMON_EQ).name, EQ_FILE_NAME);
        assert_eq!(slot_label(LOVE_PHILTER_FILE_NAME), PULP_FILTER);
        assert_eq!(new_slot(PULP_FILTER).name, LOVE_PHILTER_FILE_NAME);
        assert_eq!(slot_label(MAXIMUS_FILE_NAME), PRESS);
        assert_eq!(new_slot(PRESS).name, MAXIMUS_FILE_NAME);
    }

    /// Playlist clips through Lemon Sampler (spec: plugins/lemon-sampler.md §7): a note can start partway into the sample
    /// (per-note modulation of the start offset) and a choke ends it at once.
    #[test]
    fn lemon_sampler_starts_late_and_chokes() {
        let mut pl = Plugins { available: builtin(), scale: 1.0, rate: 48000, ..Plugins::default() };
        let mut host = crate::editor_host::EditorHost::default();
        let index = pl.index_of(LEMON_SAMPLER).unwrap();
        pl.create_with(0, index, None).unwrap();
        // One second: silent first half, 0.5 in the second.
        let wav = std::env::temp_dir().join(format!("os-app-start-late-{}.wav", std::process::id()));
        let frames: Vec<f32> = (0..48000).flat_map(|i| if i < 24000 { [0.0; 2] } else { [0.5; 2] }).collect();
        os_engine::wav::write_f32_stereo(&mut std::fs::File::create(&wav).unwrap(), 48000, &frames).unwrap();
        assert!(pl.load_sample(0, &wav.to_string_lossy()));
        let inst = pl.hosted[&0].plugin.clone() as SharedInstrument;
        let mut i = inst.lock().unwrap();
        let mut block = |i: &mut dyn os_engine::Instrument| {
            let (mut l, mut r) = (vec![0.0f32; 480], vec![0.0f32; 480]);
            i.process(48000, &mut l, &mut r);
            l.iter().fold(0.0f32, |m, v| m.max(v.abs()))
        };
        // From the top: the first 100 ms are silent.
        i.note_on(0, 60, 0.78);
        assert!((0..10).all(|_| block(&mut *i) < 1e-4), "plays from the top");
        i.choke(0, 60);
        for _ in 0..10 {
            block(&mut *i);
        }
        // Started halfway: sound at once (after the declick fade-in).
        i.note_on_from(0, 60, 0.78, 0.5);
        let first: Vec<f32> = (0..5).map(|_| block(&mut *i)).collect();
        assert!(first.iter().any(|&p| p > 0.1), "starts in the loud half: {first:?}");
        // A choke silences it within the declick out time.
        i.choke(0, 60);
        let after: Vec<f32> = (0..10).map(|_| block(&mut *i)).collect();
        assert!(after[3..].iter().all(|&p| p < 1e-4), "choked: {after:?}");
    }

    /// A file dropped on an open Lemon Sampler window (what a browser drag released over it does) reaches its editor
    /// the way an Explorer drop does: `OS_CLAP` not needed (`cargo test -p os-app drop_on -- --ignored`).
    #[test]
    #[ignore]
    fn drop_on_plugin_window_loads_the_sample() {
        use windows_sys::Win32::Foundation::RECT;
        let mut pl = Plugins { available: builtin(), scale: 1.0, rate: 48000, ..Plugins::default() };
        let mut host = crate::editor_host::EditorHost::default();
        pl.create(0, pl.index_of(LEMON_SAMPLER).unwrap()).unwrap();
        let info = ChannelInfo { name: "Lemon Sampler".into(), target: "Bus 1".into(), enabled: true, volume: 0.78, pan: 0.0, pitch_cents: 0, range: 2, track: 1, max_track: 125, misc: None, icon: None };
        host.open_window(&mut pl, 0, info).unwrap();
        pump(&mut host, &mut pl, 300);
        let frame = host.editors[&0].window.as_ref().unwrap().hwnd();
        let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        unsafe {
            SetForegroundWindow(frame as HWND);
            GetWindowRect(frame as HWND, &mut r);
        }
        pump(&mut host, &mut pl, 100);
        let (x, y) = ((r.left + r.right) / 2, (r.top + r.bottom) / 2);
        assert_eq!(host.row_at_screen(x, y), Some(0), "the window is found under the point");
        let mp3 = fixture_mp3();
        assert!(crate::editor_host::post_drop(x, y, &mp3, frame), "the editor accepts files");
        pump(&mut host, &mut pl, 300);
        assert_eq!(pl.sampler_path(0).as_deref(), Some(mp3.as_str()));
        // The strip (no editor under the point): loaded through the state instead.
        let wav = std::env::temp_dir().join(format!("os-app-drop-{}.wav", std::process::id()));
        os_engine::wav::write_f32_stereo(&mut std::fs::File::create(&wav).unwrap(), 44100, &[0.5; 2000]).unwrap();
        let wav = wav.to_string_lossy().into_owned();
        assert!(host.drop_file(&mut pl, 0, r.left + 12, r.top + 40, &wav, true));
        pump(&mut host, &mut pl, 200);
        assert_eq!(pl.sampler_path(0).as_deref(), Some(wav.as_str()));
    }

    /// `<target>/bundled/<file>` of this test's build folder (`cargo run -p bundle-plugins`), if built.
    fn bundled(file: &str) -> Option<PathBuf> {
        let target = std::env::current_exe().ok()?.parent()?.parent()?.parent()?.to_path_buf();
        Some(target.join("bundled").join(file)).filter(|p| p.exists())
    }

    /// A VST3 instrument hosts like a CLAP one (spec: plugins/vst3-hosting.md): created, given state bytes, played;
    /// the same plugin in both formats gets format labels in the picker and each label finds its own entry.
    #[test]
    fn vst3_plugins_host_like_clap_ones() {
        let (Some(clap), Some(vst3)) = (bundled("Lemon Sampler.clap"), bundled("Lemon Sampler.vst3")) else { return };
        let available = |p: &std::path::Path, load: fn(&std::path::Path) -> Result<Arc<dyn Library>, String>| {
            let lib = load(p).unwrap();
            Available { desc: lib.plugins()[0].clone(), lib }
        };
        let mut pl = Plugins {
            available: vec![
                available(&clap, |p| os_clap::Library::load(p).map(|l| l as Arc<dyn Library>)),
                available(&vst3, |p| os_vst3::Module::load(p).map(|l| l as Arc<dyn Library>)),
            ],
            scale: 1.0,
            rate: 48000,
            ..Plugins::default()
        };
        assert_eq!(pl.index_of("Lemon Sampler"), Some(0), "a name finds the first");
        pl.create(0, 1).unwrap();
        let d = pl.descriptor(0).unwrap();
        assert_eq!(d.format, os_host::Format::Vst3);
        assert!(d.uid().starts_with("vst3:") && d.uid().len() == 5 + 32);
        let wav = std::env::temp_dir().join(format!("os-app-vst3-{}.wav", std::process::id()));
        os_engine::wav::write_f32_stereo(&mut std::fs::File::create(&wav).unwrap(), 48000, &[0.5; 9600]).unwrap();
        // A VST3 instance takes a bare component state too (here Lemon Sampler's JSON with the sample).
        let json = format!("{{\"version\":\"0.0.1\",\"params\":{{}},\"fields\":{{\"sample-path\":{}}}}}", json_string(&json_string(&wav.to_string_lossy())));
        assert!(pl.set_state(0, json.as_bytes()), "the sample reaches the VST3 instance");
        let state = pl.state(0).unwrap();
        assert!(os_vst3::fl_state::FlState::parse(&state).is_some(), "state bytes are in FL Studio's wrapper layout");
        assert!(pl.set_state(0, &state));
        let (_, inst) = pl.instruments().pop().unwrap();
        let mut i = inst.lock().unwrap();
        let (mut l, mut r) = (vec![0.0f32; 512], vec![0.0f32; 512]);
        i.note_on(0, 60, 0.8);
        i.process(48000, &mut l, &mut r);
        assert!(l.iter().any(|v| v.abs() > 0.1), "plays");
    }

    /// A VST3 instrument channel and a VST3 effect in a mixer slot are saved as wrapped plugins naming the class id, and
    /// the reopened project hosts the same classes with the saved state (spec: plugins/vst3-hosting.md §8,
    /// flp/events/plugins.md §4).
    #[test]
    #[allow(deprecated)]
    fn vst3_plugins_save_and_reopen() {
        let (Some(smp), Some(eq)) = (bundled("Lemon Sampler.vst3"), bundled("Lemon Parametric EQ.vst3")) else { return };
        let load = |p: &std::path::Path| {
            let lib = os_vst3::Module::load(p).map(|l| l as Arc<dyn Library>).unwrap();
            Available { desc: lib.plugins()[0].clone(), lib }
        };
        let path = std::env::temp_dir().join(format!("openstudio-vst3-{}.flp", std::process::id()));
        let wav = std::env::temp_dir().join(format!("os-app-vst3-save-{}.wav", std::process::id()));
        os_engine::wav::write_f32_stereo(&mut std::fs::File::create(&wav).unwrap(), 48000, &[0.25; 4800]).unwrap();
        let mut a = crate::app::App::new(None);
        a.state.session.project = crate::locked::Locked::new(crate::projects::test_project());
        a.state.session.file = None;
        a.state.session.path = None;
        a.state.session.plugins.available = vec![load(&smp)];
        a.state.session.plugins.effects = vec![load(&eq)];
        let row = crate::projects::add_plugin_channel(a.state.session.project.get_mut(), "Lemon Sampler");
        a.state.session.plugins.create(row, 0).unwrap();
        a.state.session.hosted_rows.push(row);
        let json = format!("{{\"version\":\"0.0.1\",\"params\":{{}},\"fields\":{{\"sample-path\":{}}}}}", json_string(&json_string(&wav.to_string_lossy())));
        assert!(a.state.session.plugins.set_state(row, json.as_bytes()));
        a.state.session.project.get_mut().mixer[3].slots[2] = Some(os_model::SlotPlugin { id: os_model::Id::NONE, name: "Lemon Parametric EQ".into(), state: Vec::new() });
        a.state.session.plugins.create_effect(3, 2, 0, None).unwrap();
        // An edit of the effect, through the hosting layer, flushed into its processor.
        {
            let mut fx = a.state.session.plugins.hosted[&fx_key(3, 2)].plugin.lock().unwrap();
            let id = (0..fx.param_count()).filter_map(|i| fx.param_info(i)).find(|p| !p.read_only && !p.hidden && p.automatable).unwrap().id;
            assert!(fx.set_param(id, 0.3));
            assert!(fx.activate(48000));
            fx.idle();
            std::thread::sleep(Duration::from_millis(60));
            fx.idle();
        }
        let (ch_state, fx_state) = (a.state.session.plugins.state(row).unwrap(), a.state.session.plugins.state(fx_key(3, 2)).unwrap());
        let fl = |b: &[u8]| os_vst3::fl_state::FlState::parse(b).is_some();
        assert!(fl(&ch_state) && fl(&fx_state), "saved in FL Studio's wrapper layout");
        a.save_to(path.clone());
        assert!(a.state.message.as_deref().is_some_and(|m| m.starts_with("Saved")), "{:?}", a.state.message);

        // The file names each class by its id.
        let saved = os_flp::project::load(&os_flp::FlpFile::parse(&std::fs::read(&path).unwrap()).unwrap()).unwrap();
        let r = crate::project_plugins::wrapped_channel(&saved.channels[row]).unwrap();
        assert_eq!((r.format.as_str(), r.id.as_str(), r.name.as_str()), ("vst3", a.state.session.plugins.available[0].desc.id.as_str(), "Lemon Sampler"));
        assert_eq!(r.state, ch_state);
        let s = crate::project_plugins::wrapped_slot(saved.mixer[3].slots[2].as_ref().unwrap()).unwrap();
        assert_eq!((s.format.as_str(), s.id.as_str()), ("vst3", a.state.session.plugins.effects[0].desc.id.as_str()));
        assert_eq!(s.state, fx_state);

        // Reopened: found by class id even when the plugin file is named differently now.
        let mut b = crate::app::App::new(None);
        let (mut i, mut e) = (load(&smp), load(&eq));
        i.desc.path = "D:/moved/Renamed.vst3".into();
        e.desc.path = "D:/moved/Other.vst3".into();
        b.state.session.plugins.available = vec![i];
        b.state.session.plugins.effects = vec![e];
        b.open(path.clone());
        b.sync_wrapped();
        let _ = std::fs::remove_file(&path);
        assert!(b.state.session.hosted_rows.contains(&row), "{:?}", b.state.message);
        assert_eq!(b.state.session.plugins.descriptor(row).unwrap().format, os_host::Format::Vst3);
        assert_eq!(b.state.session.plugins.state(row).as_deref(), Some(&ch_state[..]));
        assert_eq!(b.state.session.plugins.state(fx_key(3, 2)).as_deref(), Some(&fx_state[..]));
        assert_eq!(crate::edit::mixer::slot_name(&b.state, 3, 2).as_deref(), Some("Lemon Parametric EQ"));
    }

    /// VST3 editors inside Lemon Studio's plugin frame, moved off every screen and captured with PrintWindow:
    /// `OS_SHOT_DIR=<dir> cargo test -p os-app --lib vst3_window_shots -- --ignored`.
    #[test]
    #[ignore]
    fn vst3_window_shots() {
        let Some(dir) = std::env::var_os("OS_SHOT_DIR") else { return };
        let dir = std::path::PathBuf::from(dir);
        for (file, effect) in [("Lemon Sampler.vst3", false), ("Lemon Parametric EQ.vst3", true)] {
            let Some(path) = bundled(file) else { continue };
            let lib = os_vst3::Module::load(&path).unwrap();
            let a = Available { desc: lib.plugins()[0].clone(), lib };
            let name = a.desc.name.clone();
            let mut pl = Plugins { scale: 1.0, rate: 48000, ..Plugins::default() };
            let mut host = crate::editor_host::EditorHost::default();
            let key = if effect {
                pl.effects = vec![a];
                pl.create_effect(1, 0, 0, None).unwrap();
                fx_key(1, 0)
            } else {
                pl.available = vec![a];
                pl.create(0, 0).unwrap();
                0
            };
            let info = ChannelInfo { name: name.clone(), target: "Bus 1".into(), enabled: true, volume: 0.78, pan: 0.0, pitch_cents: 0, range: 2, track: 1, max_track: 125, misc: None, icon: None };
            host.open_window(&mut pl, key, info).unwrap();
            let frame = host.editors[&key].window.as_ref().unwrap().hwnd();
            unsafe { SetWindowPos(frame as HWND, std::ptr::null_mut(), -20000, -20000, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE) };
            pump(&mut host, &mut pl, 400);
            let editor = unsafe { GetWindow(GetWindow(frame as HWND, GW_CHILD), GW_CHILD) };
            assert!(!editor.is_null(), "{name}: the editor is a child of the frame");
            let out = dir.join(format!("vst3_{}.bmp", name.replace(' ', "_")));
            // The first capture of a window that never reached the screen makes it paint.
            capture(frame, &out);
            pump(&mut host, &mut pl, 200);
            capture(frame, &out);
        }
    }

    /// The user's installed FL native effects show their own editors inside Lemon Studio's plugin frame (spec
    /// plugins/fruity-abi.md, "Editors"): frame moved off every screen, captured with PrintWindow, closed and opened
    /// again. Loads FL DLLs in-process, so ignored: `OS_SHOT_DIR=<dir> cargo test -p os-app --lib
    /// native_window_shots -- --ignored` (shots only when OS_SHOT_DIR is set). Skips without FL.
    #[test]
    #[ignore]
    fn native_window_shots() {
        let fl = os_paths::default_fl_dir();
        let dir = std::env::var_os("OS_SHOT_DIR").map(std::path::PathBuf::from);
        for name in ["Fruity Balance", "Fruity Parametric EQ 2", "Fruity Delay 3", "Fruity Limiter"] {
            let Some(dll) = os_native::find_plugin(&fl, name) else { continue };
            let lib = os_native::NativeLibrary::load(&dll).unwrap() as Arc<dyn Library>;
            let a = Available { desc: lib.plugins()[0].clone(), lib };
            let mut pl = Plugins { scale: 1.0, rate: 48000, effects: vec![a], ..Plugins::default() };
            let mut host = crate::editor_host::EditorHost::default();
            pl.create_effect(1, 0, 0, None).unwrap();
            let key = fx_key(1, 0);
            let info = ChannelInfo { name: name.into(), target: "Bus 1".into(), enabled: true, volume: 0.78, pan: 0.0, pitch_cents: 0, range: 2, track: 1, max_track: 125, misc: None, icon: None };
            for round in 0..2 {
                host.open_window(&mut pl, key, info.clone()).unwrap();
                let frame = host.editors[&key].window.as_ref().unwrap().hwnd();
                unsafe { SetWindowPos(frame as HWND, std::ptr::null_mut(), -20000, -20000, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE) };
                pump(&mut host, &mut pl, 300);
                let area = host.editors[&key].window.as_ref().unwrap().area() as HWND;
                let editor = unsafe { GetWindow(area, GW_CHILD) };
                assert!(!editor.is_null(), "{name}: the editor is a child of the frame's editor area");
                let (mut ra, mut re): (RECT, RECT) = unsafe { (std::mem::zeroed(), std::mem::zeroed()) };
                unsafe { GetWindowRect(area, &mut ra) };
                unsafe { GetWindowRect(editor, &mut re) };
                assert_eq!((ra.right - ra.left, ra.bottom - ra.top), (re.right - re.left, re.bottom - re.top), "{name}: the frame fits the editor");
                assert_eq!((ra.left, ra.top), (re.left, re.top), "{name}: the editor sits at the area's corner");
                if let Some(dir) = &dir {
                    let out = dir.join(format!("native_{}_{round}.bmp", name.replace(' ', "_")));
                    capture(frame, &out);
                    pump(&mut host, &mut pl, 200);
                    capture(frame, &out);
                }
                assert!(host.close_if_open(&mut pl, key));
                pump(&mut host, &mut pl, 100);
            }
        }
    }

    /// One installed native effect (`OS_NATIVE_ONE=<folder name>`) through the app's whole path: created and
    /// activated at the device rate, rendered on an audio thread while its editor is open, state saved and restored,
    /// closed and dropped. Loads FL DLLs in-process, so ignored; run once per plugin in its own process:
    /// `OS_NATIVE_ONE="Fruity Balance" cargo test -p os-app --lib native_effect_lifecycle -- --ignored`.
    #[test]
    #[ignore]
    fn native_effect_lifecycle() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let Some(name) = std::env::var_os("OS_NATIVE_ONE").map(|s| s.to_string_lossy().into_owned()) else { return };
        let fl = os_paths::default_fl_dir();
        let dll = os_native::find_plugin(&fl, &name).expect("plugin installed");
        let lib = os_native::NativeLibrary::load(&dll).unwrap() as Arc<dyn Library>;
        let a = Available { desc: lib.plugins()[0].clone(), lib };
        let mut pl = Plugins { scale: 1.0, rate: 48000, effects: vec![a], ..Plugins::default() };
        let mut host = crate::editor_host::EditorHost::default();
        let key = fx_key(1, 0);
        eprintln!("STEP create");
        if let Err(e) = pl.create_effect(1, 0, 0, None) {
            eprintln!("RESULT refused: {e}");
            return;
        }
        let fx = pl.effects()[0].2.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let audio = {
            let stop = stop.clone();
            std::thread::spawn(move || {
                let (mut l, mut r) = (vec![0.0f32; 512], vec![0.0f32; 512]);
                let mut n = 0u64;
                while !stop.load(Ordering::Relaxed) {
                    for (i, (a, b)) in l.iter_mut().zip(r.iter_mut()).enumerate() {
                        *a = ((n + i as u64) as f32 * 0.05).sin() * 0.5;
                        *b = *a;
                    }
                    fx.lock().unwrap().process_fx(48000, &mut l, &mut r);
                    n += 512;
                    std::thread::sleep(Duration::from_millis(5));
                }
                l.iter().chain(&r).all(|s| s.is_finite())
            })
        };
        eprintln!("STEP window");
        let info = ChannelInfo { name: name.clone(), target: "Bus 1".into(), enabled: true, volume: 0.78, pan: 0.0, pitch_cents: 0, range: 2, track: 1, max_track: 125, misc: None, icon: None };
        match host.open_window(&mut pl, key, info) {
            Ok(()) => {
                let frame = host.editors[&key].window.as_ref().unwrap().hwnd();
                unsafe { SetWindowPos(frame as HWND, std::ptr::null_mut(), -20000, -20000, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE) };
                pump(&mut host, &mut pl, 700);
            }
            Err(e) => eprintln!("STEP no window: {e}"),
        }
        eprintln!("STEP state");
        let st = pl.state(key).unwrap_or_default();
        pl.set_state(key, &st);
        eprintln!("STEP close");
        host.close_if_open(&mut pl, key);
        pump(&mut host, &mut pl, 100);
        stop.store(true, Ordering::Relaxed);
        let finite = audio.join().unwrap();
        eprintln!("STEP drop");
        pl.drop_key(key);
        eprintln!("RESULT ok (state {} bytes, finite output {finite})", st.len());
    }

    /// A project file's native plugins (`OS_NATIVE_FLP=<path>`) hosted the way an opened project's are with the native
    /// host on (`App::host_native`, spec plugins/fruity-abi.md "Opened projects"): prints each plugin channel and filled
    /// slot with what became of it (hosted, hosted but keeping the file's state, not installed, did not start), then
    /// renders a few blocks through every instance. Loads FL DLLs in-process: ignored, run alone on a hidden desktop:
    /// `OS_NATIVE_FLP="x.flp" cargo test -p os-app --lib native_plugins_of_an_opened_project -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn native_plugins_of_an_opened_project() {
        let Some(flp) = std::env::var_os("OS_NATIVE_FLP").map(PathBuf::from) else { return };
        os_native::set_enabled(true);
        let mut app = crate::app::App::new(None);
        app.state.session.plugins = Plugins::scan();
        eprintln!("STEP {} native plugins listed", app.state.session.plugins.available.iter().chain(&app.state.session.plugins.effects).filter(|a| a.desc.format == os_host::Format::Native).count());
        app.open(flp);
        eprintln!("STEP opened: {:?}", app.state.message);
        // The app's order: the built-in Lemon plugins take their channels and slots first.
        app.sync_effects();
        app.sync_osc3();
        for _ in 0..3 {
            app.host_native();
        }
        eprintln!("STEP {:?}", app.state.message);
        let proj = app.state.session.project.clone();
        let mut keys: Vec<(usize, String)> = proj.channels.iter().enumerate().filter_map(|(r, c)| Some((r, format!("channel {r} {}", c.plugin.clone()?)))).collect();
        keys.extend(proj.mixer.iter().flat_map(|m| m.slots.iter().enumerate().filter_map(move |(k, s)| Some((fx_key(m.index, k as u8), format!("track {} slot {k} {}", m.index, s.as_ref()?.name))))));
        for (key, label) in &keys {
            let what = match app.state.session.plugins.hosted.get(key) {
                Some(h) if h.keep_file_state => "hosted, keeps the file's state",
                Some(_) => "hosted",
                None if app.state.session.plugins.native_failed.contains(key) => "not installed or did not start",
                None => "not hosted (built-in or wrapped)",
            };
            eprintln!("RESULT {label:50} {what}");
        }
        let (mut l, mut r) = (vec![0.0f32; 480], vec![0.0f32; 480]);
        for (row, inst) in app.state.session.plugins.instruments() {
            let mut p = inst.lock().unwrap();
            p.note_on(0, 60, 0.8);
            for _ in 0..20 {
                p.process(48000, &mut l, &mut r);
            }
            p.note_off(0, 60);
            eprintln!("RESULT channel {row} renders, peak {:.3}", l.iter().chain(&r).fold(0.0f32, |m, s| m.max(s.abs())));
        }
        for (t, k, fx) in app.state.session.plugins.effects() {
            let mut p = fx.lock().unwrap();
            for _ in 0..20 {
                p.process_fx(48000, &mut l, &mut r);
            }
            eprintln!("RESULT track {t} slot {k} renders, finite {}", l.iter().chain(&r).all(|s| s.is_finite()));
        }
        app.reset_hosted();
        eprintln!("RESULT done");
    }

    /// One installed native generator (`OS_NATIVE_ONE=<folder name>`) added from the channel rack's Add one ▸ menu with
    /// the native host opted in (a mixer slot's effect list offers the native effects the same way): the next ticks
    /// create it on the new channel, and an export of a pattern with one C5 on that channel has the note at its pitch,
    /// centred (spec plugins/fruity-abi.md, "Voices"). Loads FL DLLs in-process and sets `OPENSTUDIO_NATIVE`, so
    /// ignored; run in its own process:
    /// `OS_NATIVE_ONE="Harmor" cargo test -p os-app --lib native_generator_from_the_rack_menu -- --ignored`.
    /// The test process may then crash while it exits: some plugins fail in their own shutdown once the thread that
    /// created them has ended (here the test's thread; the app creates them on its main thread, spec "Threads").
    #[test]
    #[ignore]
    #[allow(deprecated)]
    fn native_generator_from_the_rack_menu_plays_in_an_export() {
        let Some(name) = std::env::var_os("OS_NATIVE_ONE").map(|s| s.to_string_lossy().into_owned()) else { return };
        // SAFETY: test-only, run alone; `OPENSTUDIO_NATIVE` is read only by the scan.
        unsafe { std::env::set_var("OPENSTUDIO_NATIVE", "1") };
        let mut a = crate::app::App::new(None);
        a.state.session.project = crate::locked::Locked::new(crate::projects::test_project());
        a.state.session.plugins = Plugins::scan();
        a.state.plugdb.rebuild(&a.state.session.plugins);
        use crate::plugin_menu::PlugCmd;
        let fx = a.state.session.plugins.effects.iter().position(|e| e.desc.format == os_host::Format::Native).expect("a native effect");
        let fx_menu = crate::plugin_menu::items(&a.state, crate::plugin_menu::Target::Fx(Some(0)), true);
        let fx_cmd = crate::playlist::Cmd::Plugin(PlugCmd::Effect { index: fx, slot: Some(0) });
        assert!(fx_menu.iter().any(|it| it.cmd == fx_cmd), "the slot's list has it");
        let pick = a.state.session.plugins.available.iter().position(|x| x.desc.name == name).expect("the plugin is found");
        let add = crate::rack::add_one_items(&a.state).into_iter().find(|it| it.label == name).expect("Add one lists it");
        assert!(add.cmd == crate::playlist::Cmd::Plugin(PlugCmd::Generator { index: pick, after: None }));
        let row = a.state.session.project.channels.len();
        crate::playlist::run(&mut a, add.cmd);
        assert_eq!(a.state.session.pending_plugin, Some((row, pick)));
        for _ in 0..100 {
            a.tick();
            if a.state.session.pending_plugin.is_none() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(a.state.session.hosted_rows.contains(&row), "hosted: {:?}", a.state.message);
        assert_eq!(a.state.session.plugins.descriptor(row).map(|d| d.format), Some(os_host::Format::Native));
        eprintln!("MESSAGE {:?}", a.state.message);
        // One C5 at velocity 100, a beat long, on the new channel.
        let channel = a.state.session.project.channels[row].index;
        a.state.session.project.get_mut().patterns[0].notes = vec![os_model::Note { length: 96, key: 60, ..os_model::Note::step(channel, 0) }];
        let path = std::env::temp_dir().join(format!("openstudio-native-gen-{}.wav", std::process::id()));
        a.state.session.transport.export(&a.state.session.project, os_engine::PlayMode::Pattern(1), 48000, crate::export::Job::wav(&path));
        let t0 = std::time::Instant::now();
        let secs = loop {
            match a.state.session.transport.export_status() {
                crate::transport::Export::Done(r) => break r.expect("export").0,
                _ if t0.elapsed() > Duration::from_secs(60) => panic!("export did not finish"),
                _ => std::thread::sleep(Duration::from_millis(10)),
            }
        };
        let bytes = std::fs::read(&path).expect("file written");
        let _ = std::fs::remove_file(&path);
        let samples: Vec<f32> = bytes[44..].chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect();
        let (left, right): (Vec<f32>, Vec<f32>) = samples.chunks_exact(2).map(|f| (f[0], f[1])).unzip();
        let peak = |v: &[f32]| v.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        // Pitch from the note's middle: rising zero crossings over 0.1..0.4 s.
        let mid = &left[4800..19200];
        let rises = mid.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count();
        let hz = rises as f64 / 0.3;
        eprintln!("RESULT {secs:.2} s, peak L {:.3} R {:.3}, about {hz:.0} Hz", peak(&left), peak(&right));
        assert!(peak(&left) > 0.01 && peak(&left) < 2.0, "audible, not too loud");
        assert!((peak(&left) - peak(&right)).abs() < 0.2 * peak(&left), "centred");
        // Key 60 = C5 = 261.6 Hz, within a semitone; 3x Osc sounds an octave up (spec plugins/3x-osc.md). Default
        // patches with a plain fundamental: Harmor, Kepler, 3x Osc.
        let near = |f: f64| (hz / f).log2().abs() < 1.0 / 12.0;
        assert!(near(261.63) || (name == "3x Osc" && near(523.25)), "pitch {hz:.0} Hz");
    }

    /// One installed native generator (`OS_NATIVE_ONE=<folder name>`) through the app's whole path: created on a rack
    /// row, a note played on an audio thread while its editor is open (spec plugins/fruity-abi.md, "Voices"), state
    /// saved and restored, closed and dropped. Prints the note's peak and whether it ended after its note off. Loads FL
    /// DLLs in-process, so ignored; run once per plugin in its own process:
    /// `OS_NATIVE_ONE="3x Osc" cargo test -p os-app --lib native_generator_lifecycle -- --ignored`.
    #[test]
    #[ignore]
    fn native_generator_lifecycle() {
        let Some(name) = std::env::var_os("OS_NATIVE_ONE").map(|s| s.to_string_lossy().into_owned()) else { return };
        let fl = os_paths::default_fl_dir();
        let dll = os_native::find_plugin(&fl, &name).expect("plugin installed");
        let lib = os_native::NativeLibrary::load(&dll).unwrap() as Arc<dyn Library>;
        let a = Available { desc: lib.plugins()[0].clone(), lib };
        let mut pl = Plugins { scale: 1.0, rate: 48000, available: vec![a], ..Plugins::default() };
        let mut host = crate::editor_host::EditorHost::default();
        eprintln!("STEP create");
        if let Err(e) = pl.create(0, 0) {
            eprintln!("RESULT refused: {e}");
            return;
        }
        let inst = pl.instruments()[0].1.clone();
        // Held for half a second, then two seconds to end: (peak while held, peak in the last 100 ms, all finite).
        let audio = std::thread::spawn(move || {
            let (mut l, mut r) = (vec![0.0f32; 480], vec![0.0f32; 480]);
            let (mut held, mut tail, mut finite) = (0.0f32, 0.0f32, true);
            for block in 0..250 {
                let mut p = inst.lock().unwrap();
                if block == 0 {
                    p.note_on(0, 60, 100.0 / 128.0);
                }
                if block == 50 {
                    p.note_off(0, 60);
                }
                p.process(48000, &mut l, &mut r);
                drop(p);
                let peak = l.iter().chain(&r).fold(0.0f32, |m, s| m.max(s.abs()));
                finite &= l.iter().chain(&r).all(|s| s.is_finite());
                if block < 50 {
                    held = held.max(peak);
                }
                if block >= 240 {
                    tail = tail.max(peak);
                }
                std::thread::sleep(Duration::from_millis(2));
            }
            (held, tail, finite)
        });
        eprintln!("STEP window");
        let info = ChannelInfo { name: name.clone(), target: "Bus 1".into(), enabled: true, volume: 0.78, pan: 0.0, pitch_cents: 0, range: 2, track: 1, max_track: 125, misc: None, icon: None };
        match host.open_window(&mut pl, 0, info) {
            Ok(()) => {
                let frame = host.editors[&0].window.as_ref().unwrap().hwnd();
                unsafe { SetWindowPos(frame as HWND, std::ptr::null_mut(), -20000, -20000, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE) };
                pump(&mut host, &mut pl, 700);
            }
            Err(e) => eprintln!("STEP no window: {e}"),
        }
        let (held, tail, finite) = audio.join().unwrap();
        eprintln!("STEP state");
        let st = pl.state(0).unwrap_or_default();
        pl.set_state(0, &st);
        eprintln!("STEP close");
        host.close_if_open(&mut pl, 0);
        pump(&mut host, &mut pl, 100);
        eprintln!("STEP drop");
        pl.drop_key(0);
        eprintln!("RESULT ok (peak {held:.3}, after its end {tail:.4}, state {} bytes, finite output {finite})", st.len());
    }

    /// With the experimental native host opted in, the scan offers the user's installed FL native
    /// generators and effects in the pickers (spec plugins/fruity-abi.md). Skips when FL is not installed.
    /// Ignored in the default run: it mutates the process `OPENSTUDIO_NATIVE` env var, which is
    /// unsound to do while other parallel tests read the environment. Run with
    /// `cargo test -p os-app native_plugins_listed_when_opted_in -- --ignored`.
    #[test]
    #[ignore]
    fn native_plugins_listed_when_opted_in() {
        let fl = os_paths::default_fl_dir();
        if !fl.join("Plugins").join("Fruity").join("Effects").is_dir() {
            return;
        }
        // SAFETY: test-only; `OPENSTUDIO_NATIVE` is read only by this scan path.
        unsafe { std::env::set_var("OPENSTUDIO_NATIVE", "1") };
        let opted = Plugins::scan();
        unsafe { std::env::remove_var("OPENSTUDIO_NATIVE") };
        let baseline = Plugins::scan();

        let native: Vec<_> = opted.effects.iter().filter(|a| a.desc.format == os_host::Format::Native).collect();
        assert!(!native.is_empty(), "native effects should be listed when opted in");
        assert!(native.iter().any(|a| a.desc.name == "Fruity Balance"));
        assert!(
            baseline.effects.iter().all(|a| a.desc.format != os_host::Format::Native),
            "native effects must not appear without the opt-in"
        );
        let native = |l: &[Available]| l.iter().filter(|a| a.desc.format == os_host::Format::Native).map(|a| a.desc.name.clone()).collect::<Vec<_>>();
        assert!(native(&opted.available).iter().any(|n| n == "3x Osc"), "native generators should be listed when opted in");
        assert!(native(&baseline.available).is_empty(), "native generators must not appear without the opt-in");
    }
    /// An instance that records the thread it is destroyed on.
    struct Tracked(os_host::Descriptor, Arc<Mutex<Option<std::thread::ThreadId>>>);
    impl Drop for Tracked {
        fn drop(&mut self) {
            *self.1.lock().unwrap() = Some(std::thread::current().id());
        }
    }
    impl os_engine::Instrument for Tracked {
        fn note_on(&mut self, _: u32, _: u8, _: f32) {}
        fn note_off(&mut self, _: u32, _: u8) {}
        fn process(&mut self, _: u32, _: &mut [f32], _: &mut [f32]) {}
    }
    impl os_engine::HostedEffect for Tracked {
        fn process_fx(&mut self, _: u32, _: &mut [f32], _: &mut [f32]) {}
    }
    impl os_host::Plugin for Tracked {
        fn desc(&self) -> &os_host::Descriptor {
            &self.0
        }
        fn activate(&mut self, _: u32) -> bool {
            true
        }
        fn idle(&mut self) {}
        fn has_editor(&self) -> bool {
            false
        }
        fn editor_open(&self) -> bool {
            false
        }
        fn open_editor(&mut self, _: *mut c_void, _: f32) -> Result<(u32, u32), String> {
            Err(String::new())
        }
        fn editor_size(&self) -> Option<(u32, u32)> {
            None
        }
        fn close_editor(&mut self) {}
        fn save_state(&self) -> Option<Vec<u8>> {
            None
        }
        fn load_state(&mut self, _: &[u8]) -> bool {
            false
        }
    }

    /// The UI locks an instance for its per-tick jobs only when it has work: its attention count moved, its editor is
    /// open, or `LOOK_EVERY` passed; an instance without a count is looked at on every tick (spec:
    /// audio/implementing.md "Playing live").
    #[test]
    fn instances_are_looked_at_when_they_ask() {
        let fx = &builtin_effects()[0];
        let h = Hosted::new(fx.lib.clone().create(&fx.desc.id).unwrap());
        let a = h.attention.clone().expect("a built-in plugin has a count");
        let first = h.due(Look::never()).expect("a new instance is looked at");
        assert!(h.due(first).is_none(), "nothing new: left alone");
        a.raise();
        let next = h.due(first).expect("it asked");
        assert!(next.count > first.count);
        assert!(h.due(next).is_none());
        assert!(h.due(Look { at: next.at - LOOK_EVERY, ..next }).is_some(), "looked at after a while anyway");
        let untracked = Hosted::new(Arc::new(Mutex::new(Tracked(fx.desc.clone(), Arc::new(Mutex::new(None))))));
        assert!(untracked.attention.is_none());
        let seen = untracked.due(Look::never()).unwrap();
        assert!(untracked.due(seen).is_some(), "no count: every tick");
    }

    /// A removed instance the engine still plays is destroyed on the UI thread once the engine lets go of it on its own
    /// thread, never on the engine's (spec: plugins/plugin-formats.md, "Destroying instances").
    #[test]
    fn removed_plugins_are_destroyed_on_the_ui_thread() {
        let gone = Arc::new(Mutex::new(None));
        let desc = builtin()[0].desc.clone();
        let plugin: SharedPlugin = Arc::new(Mutex::new(Tracked(desc, gone.clone())));
        let mut pl = Plugins { scale: 1.0, rate: 48000, ..Plugins::default() };
        let mut host = crate::editor_host::EditorHost::default();
        pl.hosted.insert(3, Hosted::new(plugin));
        let engine = pl.instruments();
        pl.drop_key(3);
        assert_eq!(retired_count(), 1);
        sweep_retired();
        assert_eq!(retired_count(), 1, "kept while the engine holds it");
        std::thread::spawn(move || drop(engine)).join().unwrap();
        assert_eq!(*gone.lock().unwrap(), None, "not destroyed by the engine's release");
        host.idle(&mut pl, |_| None);
        assert_eq!(*gone.lock().unwrap(), Some(std::thread::current().id()));
        assert_eq!(retired_count(), 0);
    }
    /// An effect whose processing faults once `FAULT` is set, as one with a bad pointer does.
    struct Faulty(os_host::Descriptor, Arc<std::sync::atomic::AtomicBool>);
    impl os_engine::Instrument for Faulty {
        fn note_on(&mut self, _: u32, _: u8, _: f32) {}
        fn note_off(&mut self, _: u32, _: u8) {}
        fn process(&mut self, _: u32, _: &mut [f32], _: &mut [f32]) {}
    }
    impl os_engine::HostedEffect for Faulty {
        fn process_fx(&mut self, _: u32, l: &mut [f32], _: &mut [f32]) {
            if self.1.load(std::sync::atomic::Ordering::Relaxed) {
                // SAFETY: deliberately not: the read faults and the crash guard catches it.
                l[0] = unsafe { std::ptr::read_volatile(8 as *const f32) };
            }
        }
    }
    impl os_host::Plugin for Faulty {
        fn desc(&self) -> &os_host::Descriptor {
            &self.0
        }
        fn activate(&mut self, _: u32) -> bool {
            true
        }
        fn idle(&mut self) {}
        fn has_editor(&self) -> bool {
            false
        }
        fn editor_open(&self) -> bool {
            false
        }
        fn open_editor(&mut self, _: *mut c_void, _: f32) -> Result<(u32, u32), String> {
            Err(String::new())
        }
        fn editor_size(&self) -> Option<(u32, u32)> {
            None
        }
        fn close_editor(&mut self) {}
        fn save_state(&self) -> Option<Vec<u8>> {
            Some(vec![4, 5, 6])
        }
        fn load_state(&mut self, _: &[u8]) -> bool {
            true
        }
    }

    /// A hosted effect that crashes while the engine plays it is stopped, not the app: the user is told once, the slot
    /// passes the sound through and a save keeps the state it last gave (spec: plugins/plugin-formats.md, "Crash
    /// containment").
    #[test]
    fn a_crashed_plugin_is_stopped_and_reported_once() {
        if !os_host::guard::contained() {
            return;
        }
        let fault = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut desc = builtin_effects()[0].desc.clone();
        desc.name = "Faulty".into();
        let plugin = os_host::share(Faulty(desc, fault.clone()), Arc::new(os_host::guard::Module::default()));
        let mut pl = Plugins { scale: 1.0, rate: 48000, ..Plugins::default() };
        let mut host = crate::editor_host::EditorHost::default();
        let key = fx_key(1, 0);
        pl.hosted.insert(key, Hosted::new(plugin));
        assert_eq!(pl.state(key), Some(vec![4, 5, 6]));
        let (_, _, fx) = pl.effects().pop().unwrap();
        fault.store(true, std::sync::atomic::Ordering::Relaxed);
        // The engine's thread runs into the crash.
        let engine = fx.clone();
        std::thread::spawn(move || engine.lock().unwrap().process_fx(48000, &mut [0.5; 8], &mut [0.5; 8])).join().unwrap();
        let told: Vec<String> = host.idle(&mut pl, |_| None).into_iter().filter_map(|r| if let Request::Stopped(k, t) = r { Some(format!("{k}: {t}")) } else { None }).collect();
        assert_eq!(told, [format!("{key}: Faulty stopped after a memory error: its slot passes the sound through. Save your project and restart Lemon Studio to use it again")]);
        assert!(!host.idle(&mut pl, |_| None).iter().any(|r| matches!(r, Request::Stopped(..))), "told once");
        let (mut l, mut r) = ([0.5f32; 8], [0.5f32; 8]);
        fx.lock().unwrap().process_fx(48000, &mut l, &mut r);
        assert_eq!((l, r), ([0.5; 8], [0.5; 8]), "passed through");
        assert_eq!(pl.state(key), Some(vec![4, 5, 6]), "the state it last gave");
    }

    /// Third-party effects added to mixer slots with their windows open while a pattern plays, then removed one by
    /// one, round after round. `OS_VST3_FX=<bundle>;<bundle>` (two effects whose editors run their own message
    /// handling), `OS_ROUNDS` (default 6): `cargo test -p os-app --lib effect_churn -- --ignored`.
    #[test]
    #[ignore]
    fn effect_churn() {
        let Some(paths) = std::env::var("OS_VST3_FX").ok() else { return };
        let rounds: usize = std::env::var("OS_ROUNDS").ok().and_then(|s| s.parse().ok()).unwrap_or(6);
        let effects: Vec<Available> = paths
            .split(';')
            .map(|p| {
                let lib = os_vst3::Module::load(std::path::Path::new(p)).map(|l| l as Arc<dyn Library>).expect("load");
                let desc = lib.plugins()[0].clone();
                Available { lib, desc }
            })
            .collect();
        let mut pl = Plugins { effects, scale: 1.0, rate: 48000, ..Plugins::default() };
        let mut host = crate::editor_host::EditorHost::default();
        let project = crate::projects::test_project();
        let transport = crate::transport::Transport::new(None);
        transport.update(&project);
        transport.play(&project, os_engine::PlayMode::Pattern(1), 0.0);
        for round in 0..rounds {
            for i in 0..pl.effects.len() {
                pl.create_effect(1, i as u8, i, None).unwrap();
                let name = pl.effects[i].desc.name.clone();
                let info = ChannelInfo { name, target: "Bus 1".into(), enabled: true, volume: 0.8, pan: 0.0, pitch_cents: 0, range: 2, track: 1, max_track: 125, misc: None, icon: None };
                host.open_window(&mut pl, fx_key(1, i as u8), info).unwrap();
                transport.set_effects(pl.effects());
                pump(&mut host, &mut pl, 400);
            }
            for i in 0..pl.effects.len() {
                pl.drop_key(fx_key(1, i as u8));
                transport.set_effects(pl.effects());
                pump(&mut host, &mut pl, 150);
            }
            pump(&mut host, &mut pl, 300);
            assert_eq!(retired_count(), 0, "round {round}: the engine let go and the UI destroyed them");
            println!("round {round} ok");
        }
        transport.stop();
    }

    /// The four plugin window checks as found in FL; "Open new plugin windows floating": an instance the user adds takes the
    /// check's state as its window mode, one that exists keeps its own (spec: ui/settings-choices.md "Miscellaneous:
    /// naming, detached plugins and startup").
    #[test]
    fn new_plugins_take_the_detach_check() {
        use crate::prefs::{check, DETACHED_WITH_MAIN, DETACH_NEW, LIMIT_EDITORS, TYPING_PRIORITY_OFF};
        assert_eq!((check(&DETACH_NEW), check(&DETACHED_WITH_MAIN), check(&TYPING_PRIORITY_OFF), check(&LIMIT_EDITORS)), (false, false, false, true));
        let mut pl = Plugins { effects: builtin_effects(), scale: 1.0, rate: 48000, ..Plugins::default() };
        let mut host = crate::editor_host::EditorHost::default();
        let index = pl.effect_index(LEMON_EQ).expect("built-in EQ");
        pl.create_effect(1, 0, index, None).unwrap();
        assert!(!host.editors.get(&fx_key(1, 0)).is_some_and(|e| e.detached), "a project's instance opens docked");
        pl.added(fx_key(1, 0));
        assert!(!host.editors.get(&fx_key(1, 0)).is_some_and(|e| e.detached), "check off: docked");
        crate::prefs::set_check(&DETACH_NEW, true);
        pl.create_effect(1, 1, index, None).unwrap();
        pl.added(fx_key(1, 1));
        assert!(host.editors.get(&fx_key(1, 1)).is_some_and(|e| e.detached), "check on: detached");
        assert!(!host.editors.get(&fx_key(1, 0)).is_some_and(|e| e.detached), "an existing plugin keeps its mode");
    }

    /// "Limit the number of open plugin editors": with 25 open, the one opened earliest closes before another opens
    /// (spec: as above).
    #[test]
    fn the_earliest_editor_closes_past_the_limit() {
        assert_eq!(MAX_EDITORS, 25);
        // Key 24 opened earliest.
        let open: Vec<(usize, u64)> = (0..25).map(|i| (i, 50 - i as u64)).collect();
        assert_eq!(crate::editor_host::earliest_over(open[..24].iter().copied(), MAX_EDITORS), None, "24 open: the 25th opens beside them");
        assert_eq!(crate::editor_host::earliest_over(open.iter().copied(), MAX_EDITORS), Some(24));
        assert_eq!(crate::editor_host::earliest_over(std::iter::empty(), MAX_EDITORS), None);
    }

    /// The limit and the reopening place with real windows (spec: as above): a detached window moved, closed and
    /// reopened comes back where it was; of 26 Lemon Parametric EQ windows the first opened closes, its plugin still
    /// hosted; without the check there is no limit. Opens real windows:
    /// `cargo test -p os-app --lib editor_limit_and_reopen_place -- --ignored`
    #[test]
    #[ignore]
    fn editor_limit_and_reopen_place() {
        let mut pl = Plugins { effects: builtin_effects(), scale: 1.0, rate: 48000, ..Plugins::default() };
        let mut host = crate::editor_host::EditorHost::default();
        let index = pl.effect_index(LEMON_EQ).expect("built-in EQ");
        let info = |t: u16| ChannelInfo { name: LEMON_EQ.into(), target: format!("Bus {t}"), enabled: true, volume: 0.8, pan: 0.0, pitch_cents: 0, range: 2, track: t, max_track: 125, misc: None, icon: None };
        let keys: Vec<usize> = (0..26u16).map(|i| fx_key(1 + i / 8, (i % 8) as u8)).collect();
        for &k in &keys {
            let (t, s) = fx_of(k).unwrap();
            pl.create_effect(t, s, index, None).unwrap();
        }
        let open = |pl: &Plugins| host.editors.values().filter(|e| e.window.is_some()).count();
        // Without a main window every frame is detached.
        host.open_window(&mut pl, keys[0], info(1)).unwrap();
        let hwnd = host.window_of(keys[0]).unwrap().0;
        unsafe { SetWindowPos(hwnd as HWND, std::ptr::null_mut(), 300, 200, 0, 0, SWP_NOSIZE | SWP_NOZORDER) };
        pump(&mut host, &mut pl, 50);
        assert!(host.close_if_open(&mut pl, keys[0]));
        assert_eq!((host.editors[&keys[0]].detached, host.editors[&keys[0]].placed), (true, Some((300, 200))));
        host.open_window(&mut pl, keys[0], info(1)).unwrap();
        assert_eq!(host.editors[&keys[0]].window.as_ref().unwrap().position(), Some((300, 200)), "reopened where it was");
        for &k in &keys[1..25] {
            host.open_window(&mut pl, k, info(fx_of(k).unwrap().0)).unwrap();
            pump(&mut host, &mut pl, 10);
        }
        assert_eq!(open(&pl), 25);
        host.open_window(&mut pl, keys[25], info(4)).unwrap();
        assert_eq!(open(&pl), 25, "the 26th closed the earliest first");
        assert!(host.editors[&keys[0]].window.is_none() && pl.hosted.contains_key(&keys[0]), "closed, its plugin kept");
        crate::prefs::set_check(&crate::prefs::LIMIT_EDITORS, false);
        host.open_window(&mut pl, keys[0], info(1)).unwrap();
        assert_eq!(open(&pl), 26, "no limit");
        for &k in &keys {
            pl.drop_key(k);
        }
        pump(&mut host, &mut pl, 100);
    }
}
