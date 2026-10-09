//! The window around a hosted plugin's editor: OpenStudio's title bar (▸, gear, name, Presets, minimize, close) and the
//! gear-toggled strip (plugin / wrench tabs, ON / PAN / VOL / PITCH / RANGE / TRACK), drawn with os-ui in a native
//! top-level window owned by the main window. The plugin parents its editor into a child area below the strip.
//!
// spec: ui/plugin-window.md, plugins/open-sampler.md (strip geometry), ui/knobs.md (drag 1/1024 per px)

use std::cell::RefCell;
use std::f32::consts::PI;
use std::ffi::c_void;
use std::rc::Rc;

use os_ui::{Canvas, Color, Font, Glyph, Rect};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, EndPaint, InvalidateRect, ScreenToClient, SetDIBitsToDevice, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, PAINTSTRUCT,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

pub const TITLE_H: f32 = 24.0;
pub const STRIP_H: f32 = 47.0;
pub const BORDER: f32 = 4.0;

const FRAME: Color = Color::rgb(0x25292b);
const STRIP: Color = Color::rgb(0x1b1d1e);
const STRIP_LINE: Color = Color::rgb(0x151718);
const BODY: Color = Color::rgb(0x202324);
const TEXT: Color = Color::rgb(0xd4dade);
const GREY: Color = Color::rgb(0x8c9296);
const ICON: Color = Color::rgb(0xd5dbdf);
const GREEN: Color = Color::rgb(0x47f49a);
const CAPTION: Color = Color::rgb(0x9fb4c8);

/// Channel values the strip shows (pushed by the app).
#[derive(Clone, Debug, PartialEq)]
pub struct ChannelInfo {
    pub name: String,
    /// "Insert 3" / "Master".
    pub target: String,
    pub enabled: bool,
    /// 0..1, -1..1, cents.
    pub volume: f64,
    pub pan: f64,
    pub pitch_cents: i32,
    /// Semitones (pitch knob range).
    pub range: i32,
    /// Insert number, 0 = master.
    pub track: u16,
}

/// What the user did in the frame, for the app to apply to the channel.
#[derive(Clone, Debug, PartialEq)]
pub enum FrameEvent {
    Close,
    ToggleOn,
    Volume(f64),
    Pan(f64),
    PitchCents(i32),
}

#[derive(Clone, Copy, PartialEq)]
enum Knob {
    Pan,
    Vol,
    Pitch,
}

struct State {
    info: ChannelInfo,
    strip: bool,
    /// 0 = plugin, 1 = miscellaneous functions.
    tab: u8,
    /// Plugin editor size (logical px).
    editor: (f32, f32),
    events: Vec<FrameEvent>,
    drag: Option<(Knob, f32, f32)>,
}

impl State {
    fn knob_value(&self, k: Knob) -> f32 {
        let i = &self.info;
        match k {
            Knob::Pan => (i.pan as f32 + 1.0) / 2.0,
            Knob::Vol => i.volume as f32,
            Knob::Pitch => 0.5 + i.pitch_cents as f32 / (i.range.max(1) as f32 * 100.0) / 2.0,
        }
    }
    fn set_knob(&mut self, k: Knob, v: f32) {
        let v = v.clamp(0.0, 1.0);
        let e = match k {
            Knob::Pan => FrameEvent::Pan(f64::from(v) * 2.0 - 1.0),
            Knob::Vol => FrameEvent::Volume(f64::from(v)),
            Knob::Pitch => FrameEvent::PitchCents(((v - 0.5) * 2.0 * self.info.range.max(1) as f32 * 100.0).round() as i32),
        };
        match e {
            FrameEvent::Pan(p) => self.info.pan = p,
            FrameEvent::Volume(x) => self.info.volume = x,
            FrameEvent::PitchCents(c) => self.info.pitch_cents = c,
            _ => {}
        }
        self.events.push(e);
    }
    fn size(&self) -> (f32, f32) {
        let strip = if self.strip { STRIP_H } else { 0.0 };
        (self.editor.0 + 2.0 * BORDER, TITLE_H + strip + self.editor.1 + BORDER)
    }
    fn area_y(&self) -> f32 {
        TITLE_H + if self.strip { STRIP_H } else { 0.0 }
    }
}

/// Hit regions of the frame (window coordinates, logical px).
#[derive(Clone, Copy, PartialEq, Debug)]
enum Part {
    Close,
    Minimize,
    Gear,
    Tab(u8),
    On,
    Knob(u8),
    Caption,
}

fn parts(st: &State) -> Vec<(Rect, Part)> {
    let (w, _) = st.size();
    let mut v = vec![
        (Rect::new(w - 22.0, 4.0, 16.0, 16.0), Part::Close),
        (Rect::new(w - 39.0, 4.0, 16.0, 16.0), Part::Minimize),
        (Rect::new(16.0, 4.0, 16.0, 16.0), Part::Gear),
    ];
    if st.strip {
        let dx = w - 568.0;
        v.push((Rect::new(5.0, 25.0, 43.0, 46.0), Part::Tab(0)));
        v.push((Rect::new(48.0, 25.0, 43.0, 46.0), Part::Tab(1)));
        v.push((Rect::new(340.0 + dx, 35.0, 18.0, 18.0), Part::On));
        for (i, x) in [383.0, 417.0, 455.0].into_iter().enumerate() {
            v.push((Rect::new(x + dx - 14.0, 30.0, 28.0, 28.0), Part::Knob(i as u8)));
        }
    }
    v.push((Rect::new(0.0, 0.0, w, TITLE_H), Part::Caption));
    v
}

fn part_at(st: &State, x: f32, y: f32) -> Option<Part> {
    parts(st).into_iter().find(|(r, _)| r.contains(x, y)).map(|(_, p)| p)
}

const KNOBS: [Knob; 3] = [Knob::Pan, Knob::Vol, Knob::Pitch];

fn paint(c: &mut Canvas, st: &State) {
    let (w, h) = st.size();
    c.begin_frame(None);
    c.clear(FRAME);
    // Title bar.
    c.fill_polygon(&[(7.0, 8.0), (11.5, 12.0), (7.0, 16.0)], ICON);
    gear(c, 24.0, 12.0, if st.strip { GREEN } else { ICON });
    let f = Font::new(15);
    let lh = c.line_height(f);
    let ty = (12.0 - lh / 2.0).round();
    let nw = c.text(f, 36.0, ty, TEXT, &st.info.name);
    c.text_clip(f, 36.0 + nw + 4.0, ty, (w - 170.0 - nw).max(0.0), GREY, &format!("({})", st.info.target));
    let pr = Rect::new(w - 126.0, 3.0, 80.0, 18.0);
    c.fill_rrect(pr, 2.0, Color::rgb(0x1d2022));
    let plh = c.line_height(Font::new(14));
    c.text(Font::new(14), pr.x + 7.0, (pr.cy() - plh / 2.0).round(), TEXT, "Presets");
    c.fill_polygon(&[(pr.right() - 18.0, 12.0), (pr.right() - 14.0, 8.5), (pr.right() - 14.0, 15.5)], ICON);
    c.fill_polygon(&[(pr.right() - 8.0, 8.5), (pr.right() - 4.0, 12.0), (pr.right() - 8.0, 15.5)], ICON);
    c.fill_rect(Rect::new(w - 35.0, 12.0, 8.0, 1.5), ICON);
    let mut x = Glyph::new();
    x.line(w - 18.0, 8.0, w - 10.0, 16.0, 1.5).line(w - 18.0, 16.0, w - 10.0, 8.0, 1.5);
    c.glyph(ICON, &x);
    if st.strip {
        strip(c, st, w);
    }
    if st.tab == 1 {
        let a = Rect::new(BORDER, st.area_y(), w - 2.0 * BORDER, h - st.area_y() - BORDER);
        c.fill_rect(a, BODY);
        c.text_center(Font::new(16), a, GREY, "Miscellaneous functions: not available yet");
    }
}

fn strip(c: &mut Canvas, st: &State, w: f32) {
    let dx = w - 568.0;
    c.fill_rect(Rect::new(BORDER, 24.0, w - 2.0 * BORDER, 47.0), STRIP_LINE);
    c.fill_rect(Rect::new(BORDER, 25.0, w - 2.0 * BORDER, 45.0), STRIP);
    for i in 0..2u8 {
        let cell = Rect::new(5.0 + 43.0 * f32::from(i), 25.0, 44.0, 46.0);
        if st.tab == i {
            c.fill_rect(cell, BODY);
        }
        let col = if st.tab == i { Color::rgb(0x49f1b1) } else { ICON };
        let cx = 27.0 + 43.0 * f32::from(i);
        if i == 0 { plug(c, cx, 47.0, col) } else { wrench(c, cx, 47.0, col) }
    }
    // ON LED.
    let (lx, ly) = (349.0 + dx, 43.0);
    c.fill_circle(lx, ly, 7.5, Color::rgb(0x5b5c5d));
    c.fill_circle(lx, ly, 6.5, Color::rgb(0x2c422d));
    if st.info.enabled {
        c.fill_circle(lx, ly, 5.5, Color::rgb(0x417e02));
        c.fill_circle(lx, ly, 4.6, Color::rgb(0xa5e436));
        c.fill_circle(lx - 0.6, ly - 1.0, 2.0, Color::rgb(0xd8f5a0));
    } else {
        c.fill_circle(lx, ly, 5.0, Color::rgb(0x1f2a14));
    }
    caption(c, lx, "ON");
    value_box(c, Rect::new(455.0 + dx, 32.0, 43.0, 23.0), &st.info.range.to_string());
    for (k, x, cap) in [(Knob::Pan, 383.0, "PAN"), (Knob::Vol, 417.0, "VOL"), (Knob::Pitch, 455.0, "PITCH")] {
        knob(c, x + dx, 43.0, st.knob_value(k), k != Knob::Vol);
        caption(c, if k == Knob::Pitch { x + dx - 3.0 } else { x + dx }, cap);
    }
    caption(c, 484.0 + dx, "RANGE");
    let track = if st.info.track == 0 { "---".to_string() } else { st.info.track.to_string() };
    value_box(c, Rect::new(517.0 + dx, 32.0, 41.0, 23.0), &track);
    caption(c, 537.0 + dx, "TRACK");
}

fn caption(c: &mut Canvas, cx: f32, s: &str) {
    let f = Font::condensed(9, 4).bold();
    let w = c.text_width(f, s);
    c.text(f, (cx - w / 2.0).round(), 57.0, CAPTION, s);
}

fn knob(c: &mut Canvas, cx: f32, cy: f32, v: f32, bipolar: bool) {
    let r = 11.0;
    c.fill_circle(cx, cy, r + 4.0, Color::rgb(0x191c1f));
    c.fill_circle(cx, cy, r + 3.0, Color::rgb(0x23262a));
    c.fill_circle(cx, cy, r + 1.5, Color::rgb(0x191c1f));
    c.fill_rect_gradient(Rect::new(cx - r, cy - r, 2.0 * r, 2.0 * r), Color::rgb(0x4e5154), Color::rgb(0x14171a), r);
    c.fill_rect_gradient(Rect::new(cx - r + 1.6, cy - r + 1.2, 2.0 * r - 3.2, 2.0 * r - 3.2), Color::rgb(0x2a2d30), Color::rgb(0x1a1d20), r - 1.6);
    let half = 175.0 * PI / 180.0;
    let ang = |v: f32| -half + 2.0 * half * v.clamp(0.0, 1.0);
    let (a, from) = (ang(v), if bipolar { 0.0 } else { ang(0.0) });
    if (a - from).abs() > 0.01 {
        let (s, e) = if a > from { (from, a) } else { (a, from) };
        c.arc(cx, cy, r + 2.5, s - PI / 2.0, e - s, 2.0, GREEN);
    }
    c.fill_circle(cx + (r - 5.0) * a.sin(), cy - (r - 5.0) * a.cos(), 1.4, GREEN);
}

fn value_box(c: &mut Canvas, r: Rect, s: &str) {
    c.fill_rrect(r, 4.0, Color::rgb(0x631d25));
    c.fill_rect_gradient(r.inset(1.0), Color::rgb(0x8d434b), Color::rgb(0x833941), 3.0);
    let f = Font::light(22, 0);
    let w = c.text_width(f, s);
    let lh = c.line_height(f);
    c.text(f, r.right() - 11.0 - w, (r.cy() - lh / 2.0).round(), Color::rgb(0xe8dfe2), s);
}

fn gear(c: &mut Canvas, cx: f32, cy: f32, col: Color) {
    let mut g = Glyph::new();
    for i in 0..8 {
        let a = i as f32 * PI / 4.0;
        g.line(cx + 3.5 * a.cos(), cy + 3.5 * a.sin(), cx + 6.5 * a.cos(), cy + 6.5 * a.sin(), 2.4);
    }
    g.ring(cx, cy, 4.2, 2.2);
    c.glyph(col, &g);
}

fn plug(c: &mut Canvas, cx: f32, cy: f32, col: Color) {
    let mut g = Glyph::new();
    g.polygon(&[(cx - 9.0, cy - 4.0), (cx - 4.0, cy - 4.0), (cx + 3.0, cy - 9.0), (cx + 3.0, cy + 9.0), (cx - 4.0, cy + 4.0), (cx - 9.0, cy + 4.0)]);
    g.arc(cx + 4.0, cy, 6.0, -0.3 * PI, 0.6 * PI, 1.6);
    g.arc(cx + 4.0, cy, 10.0, -0.3 * PI, 0.6 * PI, 1.6);
    c.glyph(col, &g);
}

fn wrench(c: &mut Canvas, cx: f32, cy: f32, col: Color) {
    let mut g = Glyph::new();
    g.line(cx - 7.0, cy + 7.0, cx + 1.0, cy - 1.0, 3.2);
    g.arc(cx + 4.0, cy - 4.0, 4.2, 0.9 * PI, 1.55 * PI, 2.6);
    c.glyph(col, &g);
}

struct Win {
    hwnd: HWND,
    area: HWND,
    canvas: Canvas,
    buf: Vec<u32>,
    scale: f32,
    state: Rc<RefCell<State>>,
}

/// A plugin window. Dropping it destroys the native window (close the plugin's editor first).
pub struct FrameWindow {
    hwnd: HWND,
    area: HWND,
    state: Rc<RefCell<State>>,
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

const FRAME_CLASS: &str = "OpenStudioPluginFrame";
const AREA_CLASS: &str = "OpenStudioPluginArea";

impl FrameWindow {
    /// A hidden frame owned by `owner` for an editor of `editor` logical px, at screen position `pos` (physical px).
    pub fn new(owner: isize, info: ChannelInfo, editor: (f32, f32), scale: f32, pos: (i32, i32)) -> Result<FrameWindow, String> {
        let state = Rc::new(RefCell::new(State { info, strip: true, tab: 0, editor, events: Vec::new(), drag: None }));
        let (w, h) = state.borrow().size();
        let px = |v: f32| (v * scale).round() as i32;
        unsafe {
            let inst = GetModuleHandleW(std::ptr::null());
            let (fc, ac) = (wide(FRAME_CLASS), wide(AREA_CLASS));
            let cursor = LoadCursorW(std::ptr::null_mut(), IDC_ARROW);
            for (class, proc_) in [(&fc, frame_proc as WndProc), (&ac, area_proc as WndProc)] {
                let wc = WNDCLASSW {
                    style: CS_DBLCLKS,
                    lpfnWndProc: Some(proc_),
                    cbClsExtra: 0,
                    cbWndExtra: 0,
                    hInstance: inst,
                    hIcon: std::ptr::null_mut(),
                    hCursor: cursor,
                    hbrBackground: std::ptr::null_mut(),
                    lpszMenuName: std::ptr::null(),
                    lpszClassName: class.as_ptr(),
                };
                RegisterClassW(&wc); // fails harmlessly once registered
            }
            let canvas = Canvas::new(px(w) as u32, px(h) as u32, scale)?;
            let win = Box::new(Win { hwnd: std::ptr::null_mut(), area: std::ptr::null_mut(), canvas, buf: Vec::new(), scale, state: state.clone() });
            let title = wide(&state.borrow().info.name);
            let hwnd = CreateWindowExW(
                WS_EX_TOOLWINDOW,
                fc.as_ptr(),
                title.as_ptr(),
                WS_POPUP | WS_CLIPCHILDREN,
                pos.0,
                pos.1,
                px(w),
                px(h),
                owner as HWND,
                std::ptr::null_mut(),
                inst,
                Box::into_raw(win) as *const c_void,
            );
            if hwnd.is_null() {
                return Err("could not create the plugin window".into());
            }
            let ay = state.borrow().area_y();
            let area = CreateWindowExW(
                0,
                ac.as_ptr(),
                std::ptr::null(),
                WS_CHILD | WS_VISIBLE | WS_CLIPCHILDREN,
                px(BORDER),
                px(ay),
                px(editor.0),
                px(editor.1),
                hwnd,
                std::ptr::null_mut(),
                inst,
                std::ptr::null(),
            );
            if let Some(w) = win_of(hwnd) {
                w.area = area;
            }
            Ok(FrameWindow { hwnd, area, state })
        }
    }

    /// The child window the plugin's editor goes into.
    pub fn area(&self) -> *mut c_void {
        self.area
    }

    pub fn show(&self) {
        unsafe {
            ShowWindow(self.hwnd, SW_SHOWNORMAL);
        }
    }

    pub fn raise(&self) {
        unsafe {
            ShowWindow(self.hwnd, SW_SHOWNORMAL);
            SetForegroundWindow(self.hwnd);
        }
    }

    pub fn set_info(&self, info: ChannelInfo) {
        let mut st = self.state.borrow_mut();
        if st.info != info && st.drag.is_none() {
            st.info = info;
            drop(st);
            unsafe { InvalidateRect(self.hwnd, std::ptr::null(), 0) };
        }
    }

    /// Follows a new editor size (plugins may resize).
    pub fn set_editor_size(&self, editor: (f32, f32)) {
        if self.state.borrow().editor != editor {
            self.state.borrow_mut().editor = editor;
            unsafe { layout(self.hwnd) };
        }
    }

    pub fn take_events(&self) -> Vec<FrameEvent> {
        std::mem::take(&mut self.state.borrow_mut().events)
    }
}

impl Drop for FrameWindow {
    fn drop(&mut self) {
        unsafe { DestroyWindow(self.hwnd) };
    }
}

type WndProc = unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT;

unsafe fn win_of(hwnd: HWND) -> Option<&'static mut Win> {
    let p = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut Win;
    if p.is_null() { None } else { Some(unsafe { &mut *p }) }
}

/// Resizes the frame to its state (strip shown or not, editor size) and moves the editor area.
unsafe fn layout(hwnd: HWND) {
    let Some(w) = (unsafe { win_of(hwnd) }) else { return };
    let st = w.state.borrow();
    let (fw, fh) = st.size();
    let px = |v: f32| (v * w.scale).round() as i32;
    let tab_plugin = st.tab == 0;
    let (ay, ed) = (st.area_y(), st.editor);
    drop(st);
    unsafe {
        SetWindowPos(hwnd, std::ptr::null_mut(), 0, 0, px(fw), px(fh), SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
        SetWindowPos(w.area, std::ptr::null_mut(), px(BORDER), px(ay), px(ed.0), px(ed.1), SWP_NOZORDER | SWP_NOACTIVATE);
        ShowWindow(w.area, if tab_plugin { SW_SHOW } else { SW_HIDE });
    }
    w.canvas.resize(px(fw) as u32, px(fh) as u32, w.scale);
    unsafe { InvalidateRect(hwnd, std::ptr::null(), 0) };
}

unsafe extern "system" fn area_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        if msg == WM_ERASEBKGND {
            return 1;
        }
        DefWindowProcW(hwnd, msg, wp, lp)
    }
}

fn client_point(hwnd: HWND, lp: LPARAM, scale: f32, screen: bool) -> (f32, f32) {
    let mut p = POINT { x: (lp & 0xffff) as u16 as i16 as i32, y: ((lp >> 16) & 0xffff) as u16 as i16 as i32 };
    if screen {
        unsafe { ScreenToClient(hwnd, &mut p) };
    }
    (p.x as f32 / scale, p.y as f32 / scale)
}

unsafe extern "system" fn frame_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        if msg == WM_NCCREATE {
            let cs = lp as *const CREATESTRUCTW;
            let w = (*cs).lpCreateParams as *mut Win;
            (*w).hwnd = hwnd;
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, w as isize);
            return DefWindowProcW(hwnd, msg, wp, lp);
        }
        if msg == WM_NCDESTROY {
            let p = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Win;
            if !p.is_null() {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                drop(Box::from_raw(p));
            }
            return DefWindowProcW(hwnd, msg, wp, lp);
        }
        let Some(w) = win_of(hwnd) else { return DefWindowProcW(hwnd, msg, wp, lp) };
        match msg {
            WM_NCHITTEST => {
                // The title bar drags the window, except over its buttons.
                let (x, y) = client_point(hwnd, lp, w.scale, true);
                let st = w.state.borrow();
                match part_at(&st, x, y) {
                    Some(Part::Caption) => HTCAPTION as LRESULT,
                    _ => HTCLIENT as LRESULT,
                }
            }
            WM_ERASEBKGND => 1,
            WM_PAINT => {
                let mut ps: PAINTSTRUCT = std::mem::zeroed();
                let hdc = BeginPaint(hwnd, &mut ps);
                paint(&mut w.canvas, &w.state.borrow());
                let (cw, ch) = (w.canvas.size().0 * w.scale, w.canvas.size().1 * w.scale);
                let (cw, ch) = (cw.round() as u32, ch.round() as u32);
                w.buf.resize((cw * ch) as usize, 0);
                w.canvas.copy_to_xrgb(&mut w.buf);
                let mut bmi: BITMAPINFO = std::mem::zeroed();
                bmi.bmiHeader = BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: cw as i32,
                    biHeight: -(ch as i32),
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB,
                    ..std::mem::zeroed()
                };
                SetDIBitsToDevice(hdc, 0, 0, cw, ch, 0, 0, 0, ch, w.buf.as_ptr() as *const c_void, &bmi, DIB_RGB_COLORS);
                EndPaint(hwnd, &ps);
                0
            }
            WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
                let (x, y) = client_point(hwnd, lp, w.scale, false);
                let part = part_at(&w.state.borrow(), x, y);
                let mut relayout = false;
                {
                    let mut st = w.state.borrow_mut();
                    match part {
                        Some(Part::Close) => st.events.push(FrameEvent::Close),
                        Some(Part::Minimize) => {
                            drop(st);
                            ShowWindow(hwnd, SW_MINIMIZE);
                            return 0;
                        }
                        Some(Part::Gear) => {
                            st.strip = !st.strip;
                            if !st.strip {
                                st.tab = 0;
                            }
                            relayout = true;
                        }
                        Some(Part::Tab(t)) => {
                            st.tab = t;
                            relayout = true;
                        }
                        Some(Part::On) => {
                            st.info.enabled = !st.info.enabled;
                            st.events.push(FrameEvent::ToggleOn);
                        }
                        Some(Part::Knob(i)) => {
                            let k = KNOBS[usize::from(i)];
                            st.drag = Some((k, y, st.knob_value(k)));
                            SetCapture(hwnd);
                        }
                        _ => {}
                    }
                }
                if relayout {
                    layout(hwnd);
                }
                InvalidateRect(hwnd, std::ptr::null(), 0);
                0
            }
            WM_MBUTTONDOWN => {
                // Middle click resets a knob (spec: ui/knobs.md, owner decision).
                let (x, y) = client_point(hwnd, lp, w.scale, false);
                let part = part_at(&w.state.borrow(), x, y);
                if let Some(Part::Knob(i)) = part {
                    let k = KNOBS[usize::from(i)];
                    let d = if k == Knob::Vol { 0.78125 } else { 0.5 };
                    w.state.borrow_mut().set_knob(k, d);
                    InvalidateRect(hwnd, std::ptr::null(), 0);
                }
                0
            }
            WM_MOUSEMOVE => {
                let (_, y) = client_point(hwnd, lp, w.scale, false);
                let mut st = w.state.borrow_mut();
                if let Some((k, y0, v0)) = st.drag {
                    st.set_knob(k, v0 + (y0 - y) / 1024.0);
                    drop(st);
                    InvalidateRect(hwnd, std::ptr::null(), 0);
                }
                0
            }
            WM_LBUTTONUP => {
                ReleaseCapture();
                w.state.borrow_mut().drag = None;
                0
            }
            WM_CAPTURECHANGED => {
                w.state.borrow_mut().drag = None;
                0
            }
            WM_MOUSEWHEEL => {
                let (x, y) = client_point(hwnd, lp, w.scale, true);
                let notches = ((wp >> 16) & 0xffff) as u16 as i16 as f32 / 120.0;
                let part = part_at(&w.state.borrow(), x, y);
                if let Some(Part::Knob(i)) = part {
                    let k = KNOBS[usize::from(i)];
                    let mut st = w.state.borrow_mut();
                    let v = st.knob_value(k) + notches / 64.0;
                    st.set_knob(k, v);
                    drop(st);
                    InvalidateRect(hwnd, std::ptr::null(), 0);
                }
                0
            }
            WM_CLOSE => {
                w.state.borrow_mut().events.push(FrameEvent::Close);
                0
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

/// Screen position for a new plugin window: right of the owner's left edge, below its toolbar (physical px).
pub fn default_position(owner: isize) -> (i32, i32) {
    let mut r: RECT = unsafe { std::mem::zeroed() };
    unsafe { GetWindowRect(owner as HWND, &mut r) };
    (r.left + 420, r.top + 160)
}
