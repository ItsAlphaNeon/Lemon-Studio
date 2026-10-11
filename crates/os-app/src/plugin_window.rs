//! The window around a hosted plugin's editor: OpenStudio's title bar (menu arrow, gear, channel name, "(mixer target)",
//! Presets box, minimize, close) and the gear-toggled strip, one row: the plugin's page tabs (a plain plugin has the plug
//! tab, an Open plugin joined through the host link brings its own pages), the Misc wrench, then
//! ON / PAN / VOL / PITCH / RANGE / TRACK.
//! Drawn with os-ui in a native window: docked (a child of the main window, clipped to the workspace and stacked with
//! the app's own windows) until the menu arrow's "Detached" makes it a top-level window owned by the main window. The
//! plugin parents its editor into a child area below the strip.
//!
// spec: ui/plugin-window.md, plugins/host-link.md, plugins/open-sampler.md (strip geometry), ui/knobs.md (drag 1/1024 per px)

use std::cell::RefCell;
use std::f32::consts::PI;
use std::ffi::c_void;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, Once};

use os_hostlink::PageIcon;
use os_ui::tabler as tb;
use os_ui::{Canvas, Color, Font, Rect};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, ClientToScreen, CombineRgn, CreateRectRgn, DeleteObject, EndPaint, GetWindowRgn, InvalidateRect, RedrawWindow, ScreenToClient, SetWindowRgn, RDW_ALLCHILDREN, RDW_INVALIDATE,
    RDW_UPDATENOW, RGN_AND, RGN_COPY, RGN_DIFF, SetDIBitsToDevice, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, PAINTSTRUCT,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetFocus, GetKeyState, ReleaseCapture, SetCapture, VK_CONTROL, VK_MENU};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

/// A key pressed or released while a plugin window has the keyboard focus (the typing keyboard keeps playing).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyMsg {
    /// Hardware scancode as winit's `PhysicalKeyExtScancode` expects it (extended keys carry 0xE000).
    pub scancode: u32,
    pub pressed: bool,
    pub ctrl: bool,
    pub alt: bool,
}

static KEYS: Mutex<Vec<KeyMsg>> = Mutex::new(Vec::new());

/// A message an Open plugin's editor sent to its frame (host link, plugin -> host).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkMsg {
    /// Hint or page changed: poll the link now.
    Changed,
    /// Preview note from the plugin's keyboard: key, velocity, on.
    Note(u8, u8, bool),
    /// "Create automation clip" on the plugin parameter at this list position.
    Automate(usize),
}

/// (editor area window, message) since the last `take_link`.
static LINK: Mutex<Vec<(isize, LinkMsg)>> = Mutex::new(Vec::new());

/// Link messages seen since the last call (UI thread).
pub fn take_link() -> Vec<(isize, LinkMsg)> {
    LINK.lock().map(|mut k| std::mem::take(&mut *k)).unwrap_or_default()
}
static KEY_HOOK: Once = Once::new();

/// A docked window moved and the main window repainted what it uncovered.
static EXPOSED: AtomicBool = AtomicBool::new(false);

/// Whether a docked plugin window uncovered part of the main window since the last call: the next present must show
/// the whole canvas, not only the app's own damage (UI thread).
pub fn take_exposed() -> bool {
    EXPOSED.swap(false, Ordering::Relaxed)
}

/// Keys seen since the last call (UI thread).
pub fn take_keys() -> Vec<KeyMsg> {
    KEYS.lock().map(|mut k| std::mem::take(&mut *k)).unwrap_or_default()
}

/// Thread-wide keyboard hook: a plugin's editor (any plugin, any child window) swallows key messages, so the key
/// events are copied here whenever the focus is inside one of our plugin frames, and the app plays them.
unsafe extern "system" fn key_hook(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        if code == 0 {
            let focus = GetFocus();
            if !focus.is_null() {
                // Inside a plugin frame: docked frames are children of the main window, so walk up to the frame.
                let mut w = focus;
                let mut in_frame = false;
                while !w.is_null() && !in_frame {
                    let mut name = [0u16; 64];
                    let n = GetClassNameW(w, name.as_mut_ptr(), 64) as usize;
                    in_frame = String::from_utf16_lossy(&name[..n]) == FRAME_CLASS;
                    w = GetAncestor(w, GA_PARENT);
                }
                let up = lp & (1 << 31) != 0;
                let repeat = !up && lp & (1 << 30) != 0;
                if in_frame && !repeat {
                    let mut sc = ((lp >> 16) & 0xff) as u32;
                    if lp & (1 << 24) != 0 {
                        sc |= 0xE000;
                    }
                    let down = |vk: u16| GetKeyState(i32::from(vk)) < 0;
                    if let Ok(mut k) = KEYS.lock() {
                        k.push(KeyMsg { scancode: sc, pressed: !up, ctrl: down(VK_CONTROL), alt: down(VK_MENU) });
                    }
                }
            }
        }
        CallNextHookEx(std::ptr::null_mut(), code, wp, lp)
    }
}

fn install_key_hook() {
    KEY_HOOK.call_once(|| unsafe {
        SetWindowsHookExW(WH_KEYBOARD, Some(key_hook), std::ptr::null_mut(), GetCurrentThreadId());
    });
}

/// Title bar height (outline row included); the strip, or the editor's outline, starts here.
pub const TITLE_H: f32 = 27.0;
/// Strip height, its outline included (y 27..72).
pub const STRIP_H: f32 = 46.0;
/// Left / right / bottom inset of the editor: 1 px outline, 5 px frame, 1 px editor outline.
pub const BORDER: f32 = 7.0;
/// Width of the Presets box in the title bar (its right edge is 49 px from the window's).
const PRESETS_W: f32 = 80.0;

fn c_frame() -> Color {
    os_theme::current().s(0x25292b)
}
fn c_outline() -> Color {
    os_theme::current().s(0x191d1f)
}
fn c_top_light() -> Color {
    os_theme::current().s(0x2b2f31)
}
fn c_editor_line() -> Color {
    os_theme::current().s(0x13191d)
}
fn c_presets() -> Color {
    os_theme::current().s(0x1d2123)
}
fn c_arrow() -> Color {
    os_theme::current().tx(0xc2c8cc)
}
fn c_gear_on() -> Color {
    os_theme::current().a(0x8eff7e)
}
fn c_strip() -> Color {
    os_theme::current().s(0x1b1d1e)
}
fn c_strip_line() -> Color {
    os_theme::current().s(0x151718)
}
fn c_body() -> Color {
    os_theme::current().s(0x202324)
}
fn c_text() -> Color {
    os_theme::current().tx(0xd4dade)
}
fn c_grey() -> Color {
    os_theme::current().tx(0x8c9296)
}
fn c_icon() -> Color {
    os_theme::current().tx(0xd4dade)
}
fn c_green() -> Color {
    os_theme::current().a(0x47f49a)
}
fn c_caption() -> Color {
    os_theme::current().tx(0x9fb4c8)
}

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
    /// The channel's Misc tab values (`None`: no Misc tab, e.g. mixer effects).
    pub misc: Option<crate::misc_page::MiscVals>,
}

/// What the user did in the frame, for the app to apply to the channel.
#[derive(Clone, Debug, PartialEq)]
pub enum FrameEvent {
    Close,
    /// The user picked page `n` of the plugin (a tab left of the Misc tab).
    Page(usize),
    ToggleOn,
    Volume(f64),
    Pan(f64),
    PitchCents(i32),
    /// The window was clicked: it comes to the front of the app's window stack.
    Raise,
    /// The menu arrow was clicked: the app opens the window menu at this screen point.
    Menu(i32, i32),
    /// The Presets box was clicked: the app opens the preset list at this screen point (spec: plugins/presets.md).
    Presets(i32, i32),
    /// The Presets box arrows (or the wheel over the box): next (+1) / previous (-1) preset.
    PresetStep(i32),
    /// A Misc tab value changed (spec: ui/plugin-window.md "Misc tab").
    Misc(crate::misc_page::M, f32),
    /// A Misc tab list (chord list, number box values): the app opens it at this screen point.
    MiscMenu(crate::misc_page::M, i32, i32),
    /// A Misc tab keyboard key: key, velocity, pressed.
    Note(u8, u8, bool),
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
    /// The plugin's page tabs, left to right (a plain plugin: the plug).
    pages: Vec<PageIcon>,
    /// The plugin's active page.
    page: usize,
    /// The Misc tab (right of the page tabs) is showing instead of the plugin.
    misc: bool,
    /// The plugin's hint line (parameter under the mouse), shown in the title bar.
    hint: String,
    /// The loaded preset's name, shown in the Presets box ("Presets" without one).
    preset: Option<String>,
    /// Misc tab page state (tracking page, keyboard scroll, drag, hover).
    misc_ui: crate::misc_page::MiscUi,
    /// Plugin editor size (logical px).
    editor: (f32, f32),
    events: Vec<FrameEvent>,
    drag: Option<(Knob, f32, f32)>,
    /// Title-bar move in progress: cursor and window position (screen px) at the press. Done by hand: the system's
    /// move loop (HTCAPTION) blocks the UI thread, which would freeze all drawing while a window moves.
    moving: Option<(i32, i32, i32, i32)>,
    /// A top-level window (menu arrow > Detached) instead of a child of the main window.
    detached: bool,
    /// Docked: the workspace and the rects covering it (the app's windows and popups above this one), in the main
    /// window's client area (physical px). Kept so a move re-clips at once instead of at the app's next tick.
    clip: Option<([i32; 4], Vec<[i32; 4]>)>,
    /// The clip last applied (window rect, workspace, covering rects), so unchanged clips are not reset.
    clip_key: Option<Vec<i32>>,
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
        (self.editor.0 + 2.0 * BORDER, self.area_y() + self.editor.1 + BORDER)
    }
    /// Top of the editor (below the strip, or the title bar, and the editor's 1 px outline).
    fn area_y(&self) -> f32 {
        TITLE_H + if self.strip { STRIP_H } else { 0.0 } + 1.0
    }
}

/// Hit regions of the frame (window coordinates, logical px).
#[derive(Clone, Copy, PartialEq, Debug)]
enum Part {
    Close,
    Minimize,
    Gear,
    /// The menu arrow (the window menu, an app popup).
    Menu,
    /// The Presets box and its previous / next arrows (spec: plugins/presets.md).
    Presets,
    PresetPrev,
    PresetNext,
    /// Page tab `i`; `i == pages.len()` is the Misc tab.
    Tab(usize),
    On,
    Knob(u8),
    Caption,
}

fn parts(st: &State) -> Vec<(Rect, Part)> {
    let (w, _) = st.size();
    let p = presets_rect(w);
    let mut v = vec![
        (Rect::new(w - 23.0, 7.0, 17.0, 17.0), Part::Close),
        (Rect::new(w - 42.0, 7.0, 17.0, 17.0), Part::Minimize),
        (MENU_RECT, Part::Menu),
        (GEAR_RECT, Part::Gear),
        (Rect::new(p.right() - 33.0, p.y, 17.0, p.h), Part::PresetPrev),
        (Rect::new(p.right() - 16.0, p.y, 16.0, p.h), Part::PresetNext),
        (p, Part::Presets),
    ];
    if st.strip {
        let dx = w - STRIP_RIGHT;
        for i in 0..=st.pages.len() {
            v.push((tab_rect(i), Part::Tab(i)));
        }
        v.push((Rect::new(340.0 + dx, 38.0, 18.0, 18.0), Part::On));
        for (i, x) in [383.0, 417.0, 455.0].into_iter().enumerate() {
            v.push((Rect::new(x + dx - 14.0, 33.0, 28.0, 28.0), Part::Knob(i as u8)));
        }
    }
    v.push((Rect::new(0.0, 0.0, w, TITLE_H), Part::Caption));
    v
}

fn part_at(st: &State, x: f32, y: f32) -> Option<Part> {
    parts(st).into_iter().find(|(r, _)| r.contains(x, y)).map(|(_, p)| p)
}

const KNOBS: [Knob; 3] = [Knob::Pan, Knob::Vol, Knob::Pitch];

/// Cell of strip tab `i` (45 px wide from x 7, inside the strip's outline; the Misc tab is the one after the pages).
fn tab_rect(i: usize) -> Rect {
    Rect::new(7.0 + 45.0 * i as f32, TITLE_H + 1.0, 45.0, STRIP_H - 1.0)
}

/// The strip's right-hand controls (ON ... TRACK) are placed for a window this wide and move with its right edge.
const STRIP_RIGHT: f32 = 570.0;

/// The title bar's menu arrow and gear (always in the title bar, whether the strip is shown or not).
const MENU_RECT: Rect = Rect { x: 8.0, y: 5.0, w: 14.0, h: 21.0 };
const GEAR_RECT: Rect = Rect { x: 24.0, y: 5.0, w: 21.0, h: 21.0 };

/// The Presets box: right side of the title bar, left of minimize.
fn presets_rect(w: f32) -> Rect {
    Rect::new(w - 49.0 - PRESETS_W, 6.0, PRESETS_W, 19.0)
}

fn page_tabler(p: PageIcon) -> &'static tb::TablerIcon {
    match p {
        PageIcon::Plugin => &tb::PLUG,
        PageIcon::Sample => &tb::WAVE_SINE,
        PageIcon::Envelope => &tb::TIMELINE,
    }
}

fn icon_at(c: &mut Canvas, t: &tb::TablerIcon, cx: f32, cy: f32, size: f32, col: Color) {
    c.icon(t, Rect::new(cx - size / 2.0, cy - size / 2.0, size, size), col);
}

/// The title bar's text font (FL: 8 px cap height, baseline under row 18).
fn title_font() -> Font {
    Font::new(13)
}

fn paint(c: &mut Canvas, st: &State) {
    let (w, h) = st.size();
    c.begin_frame(None);
    c.clear(c_frame());
    // 1 px outline around the window, a lighter line under its top edge.
    for r in [Rect::new(0.0, 0.0, w, 1.0), Rect::new(0.0, h - 1.0, w, 1.0), Rect::new(0.0, 0.0, 1.0, h), Rect::new(w - 1.0, 0.0, 1.0, h)] {
        c.fill_rect(r, c_outline());
    }
    c.fill_rect(Rect::new(1.0, 1.0, w - 2.0, 1.0), c_top_light());
    title_bar(c, st, w);
    if st.strip {
        strip(c, st, w);
    }
    // The editor's 1 px outline; the plugin's own window covers the inside.
    let ay = st.area_y();
    c.fill_rect(Rect::new(BORDER - 1.0, ay - 1.0, w - 2.0 * BORDER + 2.0, h - ay - BORDER + 2.0), c_editor_line());
    let a = Rect::new(BORDER, ay, w - 2.0 * BORDER, h - ay - BORDER);
    c.fill_rect(a, c_body());
    if st.misc {
        match &st.info.misc {
            Some(v) => crate::misc_page::paint(c, &os_theme::current(), a.x, a.y, v, &st.misc_ui),
            None => c.text_center(Font::new(16), a, c_grey(), "No channel settings here"),
        }
    }
}

/// Title bar, left to right: menu arrow, gear (boxed while the strip is shown), channel name, "(mixer target)", the
/// plugin's hint; right side: Presets box with previous / next arrows, minimize, close.
fn title_bar(c: &mut Canvas, st: &State, w: f32) {
    icon_at(c, &tb::CARET_RIGHT, 14.4, 15.5, 15.4, c_arrow());
    let g = GEAR_RECT;
    if st.strip {
        c.stroke_rrect(Rect::new(g.x + 1.0, g.y + 1.0, g.w - 2.0, g.h - 2.0), 3.5, 2.0, c_gear_on());
    }
    icon_at(c, &tb::SETTINGS_FILLED, g.cx(), g.cy(), 18.4, if st.strip { c_gear_on() } else { c_icon() });
    let p = presets_rect(w);
    let f = title_font();
    let ty = (19.0 - c.ascent(f)).round();
    let nx = 48.0;
    let room = (p.x - 8.0 - nx).max(0.0);
    let nw = c.text_width(f, &st.info.name).min(room);
    c.text_clip(f, nx, ty, room, c_text(), &st.info.name);
    let target = format!("({})", st.info.target);
    let tx = nx + nw + c.text_width(f, " ");
    let troom = (p.x - 8.0 - tx).max(0.0);
    c.text_clip(f, tx, ty, troom, c_grey(), &target);
    let tw = c.text_width(f, &target).min(troom);
    let hint = if st.misc { &st.misc_ui.hint } else { &st.hint };
    if !hint.is_empty() {
        let hx = tx + tw + 14.0;
        let hf = Font::new(12);
        let hy = (19.0 - c.ascent(hf)).round();
        // A tab splits the hint: the part after it sits right-aligned before the Presets box (spec: plugins/host-link.md).
        let (left, right) = hint.split_once('\t').unwrap_or((hint, ""));
        let end = p.x - 8.0;
        let rw = if right.is_empty() { 0.0 } else { c.text_width(hf, right).min((end - hx).max(0.0)) };
        if rw > 0.0 {
            c.text_clip(hf, end - rw, hy, rw, c_caption(), right);
        }
        c.text_clip(hf, hx, hy, (end - rw - 10.0 - hx).max(0.0), c_caption(), left);
    }
    // Presets box: the loaded preset's name, or "Presets" (spec: plugins/presets.md).
    c.fill_rrect(p, 3.0, c_presets());
    c.text_clip(f, p.x + 9.0, ty, p.w - 9.0 - 31.0, c_text(), st.preset.as_deref().unwrap_or("Presets"));
    icon_at(c, &tb::CARET_LEFT, p.right() - 22.2, 15.5, 18.9, c_icon());
    icon_at(c, &tb::CARET_RIGHT, p.right() - 7.8, 15.5, 18.9, c_icon());
    c.icon_stroke(&tb::MINUS, Rect::new(w - 39.5, 7.0, 12.0, 12.0), c_icon(), 2.0);
    c.icon_stroke(&tb::X, Rect::new(w - 22.25, 7.75, 15.5, 15.5), c_icon(), 1.6);
}

fn strip(c: &mut Canvas, st: &State, w: f32) {
    // Strip box with a 1 px outline (its bottom edge is the editor's outline).
    let (dx, dy) = (w - STRIP_RIGHT, TITLE_H - 24.0);
    c.fill_rect(Rect::new(BORDER - 1.0, TITLE_H, w - 2.0 * BORDER + 2.0, STRIP_H), c_strip_line());
    c.fill_rect(Rect::new(BORDER, TITLE_H + 1.0, w - 2.0 * BORDER, STRIP_H - 1.0), c_strip());
    // Page tabs, then the Misc wrench (always the last tab).
    let n = st.pages.len();
    for i in 0..=n {
        let cell = tab_rect(i);
        let active = if i == n { st.misc } else { !st.misc && st.page == i };
        if active {
            c.fill_rect(cell, c_body());
        }
        let col = if active { os_theme::current().a(0x49f1b1) } else { c_icon() };
        let t = if i == n { &tb::TOOL } else { page_tabler(st.pages[i]) };
        icon_at(c, t, cell.cx(), cell.cy(), 26.0, col);
    }
    // ON LED.
    let (lx, ly) = (349.0 + dx, 43.0 + dy);
    os_ui::kit::led(c, &os_theme::current(), lx, ly, 7.5, os_ui::kit::State::NORMAL.with_on(st.info.enabled));
    caption(c, lx, "ON");
    value_box(c, Rect::new(455.0 + dx, 32.0 + dy, 43.0, 23.0), &st.info.range.to_string());
    for (k, x, cap) in [(Knob::Pan, 383.0, "PAN"), (Knob::Vol, 417.0, "VOL"), (Knob::Pitch, 455.0, "PITCH")] {
        // Indicator dots: pan teal, volume green, pitch red.
        let dot = os_theme::current().a(match k { Knob::Pan => 0x43aebd, Knob::Vol => 0x47f49a, Knob::Pitch => 0xbb465f });
        knob(c, x + dx, 43.0 + dy, st.knob_value(k), k != Knob::Vol, dot);
        caption(c, if k == Knob::Pitch { x + dx - 3.0 } else { x + dx }, cap);
    }
    caption(c, 484.0 + dx, "RANGE");
    let track = if st.info.track == 0 { "---".to_string() } else { st.info.track.to_string() };
    value_box(c, Rect::new(517.0 + dx, 32.0 + dy, 41.0, 23.0), &track);
    caption(c, 537.0 + dx, "TRACK");
}

fn caption(c: &mut Canvas, cx: f32, s: &str) {
    let f = Font::condensed(9, 4).bold();
    let w = c.text_width(f, s);
    c.text(f, (cx - w / 2.0).round(), TITLE_H + 33.0, c_caption(), s);
}

fn knob(c: &mut Canvas, cx: f32, cy: f32, v: f32, bipolar: bool, dot: Color) {
    let r = 11.0;
    c.fill_circle(cx, cy, r + 4.0, os_theme::current().s(0x191c1f));
    c.fill_circle(cx, cy, r + 3.0, os_theme::current().s(0x23262a));
    c.fill_circle(cx, cy, r + 1.5, os_theme::current().s(0x191c1f));
    c.fill_rect_gradient(Rect::new(cx - r, cy - r, 2.0 * r, 2.0 * r), os_theme::current().s(0x4e5154), os_theme::current().s(0x14171a), r);
    c.fill_rect_gradient(Rect::new(cx - r + 1.6, cy - r + 1.2, 2.0 * r - 3.2, 2.0 * r - 3.2), os_theme::current().s(0x2a2d30), os_theme::current().s(0x1a1d20), r - 1.6);
    let half = 175.0 * PI / 180.0;
    let ang = |v: f32| -half + 2.0 * half * v.clamp(0.0, 1.0);
    let (a, from) = (ang(v), if bipolar { 0.0 } else { ang(0.0) });
    if (a - from).abs() > 0.01 {
        let (s, e) = if a > from { (from, a) } else { (a, from) };
        c.arc(cx, cy, r + 2.5, s - PI / 2.0, e - s, 2.0, c_green());
    }
    c.fill_circle(cx + (r - 5.0) * a.sin(), cy - (r - 5.0) * a.cos(), 1.4, dot);
}

fn value_box(c: &mut Canvas, r: Rect, s: &str) {
    c.fill_rrect(r, 4.0, Color::rgb(0x631d25));
    c.fill_rect_gradient(r.inset(1.0), Color::rgb(0x8d434b), Color::rgb(0x833941), 3.0);
    let f = Font::light(22, 0);
    let w = c.text_width(f, s);
    let lh = c.line_height(f);
    c.text(f, r.right() - 11.0 - w, (r.cy() - lh / 2.0).round(), Color::rgb(0xe8dfe2), s);
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
    /// A hidden frame for an editor of `editor` logical px at screen position `pos` (physical px): a child of `owner`
    /// (docked) or a top-level window owned by it (`detached`).
    pub fn new(owner: isize, info: ChannelInfo, editor: (f32, f32), scale: f32, pos: (i32, i32), detached: bool) -> Result<FrameWindow, String> {
        install_key_hook();
        // Without a main window (tests) there is nothing to dock into.
        let detached = detached || owner == 0;
        let state = Rc::new(RefCell::new(State {
            info,
            strip: true,
            pages: vec![PageIcon::Plugin],
            page: 0,
            misc: false,
            hint: String::new(),
            preset: None,
            misc_ui: Default::default(),
            editor,
            events: Vec::new(),
            drag: None,
            moving: None,
            detached,
            clip: None,
            clip_key: None,
        }));
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
            let mut p = POINT { x: pos.0, y: pos.1 };
            if !detached {
                ScreenToClient(owner as HWND, &mut p);
            }
            let hwnd = CreateWindowExW(
                0,
                fc.as_ptr(),
                title.as_ptr(),
                frame_style(detached),
                p.x,
                p.y,
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

    pub fn detached(&self) -> bool {
        self.state.borrow().detached
    }

    /// Docks the window into `owner` (a child, same place on screen) or detaches it (a top-level window owned by
    /// `owner`). The plugin's editor stays embedded.
    pub fn set_detached(&self, owner: isize, on: bool) {
        if self.state.borrow().detached == on {
            return;
        }
        {
            let mut st = self.state.borrow_mut();
            st.detached = on;
            st.clip = None;
            st.clip_key = None;
        }
        unsafe {
            let mut r: RECT = std::mem::zeroed();
            GetWindowRect(self.hwnd, &mut r);
            SetWindowRgn(self.hwnd, std::ptr::null_mut(), 0);
            ShowWindow(self.hwnd, SW_HIDE);
            let mut p = POINT { x: r.left, y: r.top };
            if on {
                SetParent(self.hwnd, std::ptr::null_mut());
                SetWindowLongPtrW(self.hwnd, GWL_STYLE, frame_style(true) as isize);
                SetWindowLongPtrW(self.hwnd, GWLP_HWNDPARENT, owner);
            } else {
                SetWindowLongPtrW(self.hwnd, GWLP_HWNDPARENT, 0);
                SetWindowLongPtrW(self.hwnd, GWL_STYLE, frame_style(false) as isize);
                SetParent(self.hwnd, owner as HWND);
                ScreenToClient(owner as HWND, &mut p);
            }
            SetWindowPos(self.hwnd, HWND_TOP, p.x, p.y, 0, 0, SWP_NOSIZE | SWP_FRAMECHANGED);
        }
        self.raise();
    }

    /// Clips a docked window to `work` minus the `covers` (the app's windows and popups above it), all in the parent's
    /// client area (physical px). Moving the window by its title bar re-clips with the same rects.
    pub fn clip_to(&self, work: [i32; 4], covers: &[[i32; 4]]) {
        if self.detached() {
            return;
        }
        self.state.borrow_mut().clip = Some((work, covers.to_vec()));
        unsafe { apply_clip(self.hwnd, &self.state) };
    }

    /// The native frame window (drops find their plugin by it; tests drive and capture it).
    pub fn hwnd(&self) -> isize {
        self.hwnd as isize
    }

    /// The child window the plugin's editor goes into.
    pub fn area(&self) -> *mut c_void {
        self.area
    }

    pub fn raise(&self) {
        unsafe {
            ShowWindow(self.hwnd, SW_SHOWNORMAL);
            SetWindowPos(self.hwnd, HWND_TOP, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE);
            if self.detached() {
                SetForegroundWindow(self.hwnd);
            }
        }
    }

    pub fn set_info(&self, info: ChannelInfo) {
        let mut st = self.state.borrow_mut();
        if st.info != info && st.drag.is_none() && st.misc_ui.drag.is_none() {
            st.info = info;
            drop(st);
            unsafe { InvalidateRect(self.hwnd, std::ptr::null(), 0) };
        }
    }

    /// The plugin's own page tabs (an Open plugin joined through the host link), replacing the plug tab.
    pub fn set_pages(&self, pages: Vec<PageIcon>, active: usize) {
        let mut st = self.state.borrow_mut();
        if !pages.is_empty() && (st.pages != pages || st.page != active) {
            st.page = active.min(pages.len() - 1);
            st.pages = pages;
            drop(st);
            unsafe { InvalidateRect(self.hwnd, std::ptr::null(), 0) };
        }
    }

    /// The plugin changed its page itself.
    pub fn set_page(&self, page: usize) {
        let mut st = self.state.borrow_mut();
        if st.page != page && page < st.pages.len() {
            st.page = page;
            drop(st);
            unsafe { InvalidateRect(self.hwnd, std::ptr::null(), 0) };
        }
    }

    /// The plugin's hint line (parameter under the mouse).
    pub fn set_hint(&self, hint: String) {
        let mut st = self.state.borrow_mut();
        if st.hint != hint {
            st.hint = hint;
            drop(st);
            unsafe { InvalidateRect(self.hwnd, std::ptr::null(), 0) };
        }
    }

    /// The loaded preset's name for the Presets box.
    pub fn set_preset(&self, name: Option<String>) {
        let mut st = self.state.borrow_mut();
        if st.preset != name {
            st.preset = name;
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

    /// Shows or hides the gear strip (mixer effects open without it).
    pub fn set_strip(&self, on: bool) {
        if self.state.borrow().strip != on {
            self.state.borrow_mut().strip = on;
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

fn frame_style(detached: bool) -> u32 {
    if detached { WS_POPUP | WS_CLIPCHILDREN } else { WS_CHILD | WS_CLIPSIBLINGS | WS_CLIPCHILDREN }
}


/// Puts `parent`'s docked windows `hwnds` (bottom to top) in that native order when they are not already.
pub fn stack(parent: isize, hwnds: &[isize]) {
    if hwnds.len() < 2 {
        return;
    }
    unsafe {
        // The children's current order, bottom to top.
        let mut now = Vec::new();
        let mut c = GetWindow(parent as HWND, GW_CHILD);
        while !c.is_null() {
            if hwnds.contains(&(c as isize)) {
                now.push(c as isize);
            }
            c = GetWindow(c, GW_HWNDNEXT);
        }
        now.reverse();
        if now == hwnds {
            return;
        }
        for &h in hwnds {
            SetWindowPos(h as HWND, HWND_TOP, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        }
    }
}

/// A child window's rect in its parent's client area (physical px).
unsafe fn parent_rect(hwnd: HWND) -> [i32; 4] {
    unsafe {
        let mut r: RECT = std::mem::zeroed();
        GetWindowRect(hwnd, &mut r);
        let parent = GetParent(hwnd);
        let (mut a, mut b) = (POINT { x: r.left, y: r.top }, POINT { x: r.right, y: r.bottom });
        ScreenToClient(parent, &mut a);
        ScreenToClient(parent, &mut b);
        [a.x, a.y, b.x, b.y]
    }
}

/// Sets a docked window's region for its current position from `State::clip`. The parts that come into view are
/// repainted, the plugin's editor included: the system redraws only the frame itself, and an editor left alone keeps
/// showing whatever was seen through the old region.
unsafe fn apply_clip(hwnd: HWND, state: &RefCell<State>) {
    let (me, work, covers) = {
        let mut st = state.borrow_mut();
        let Some((work, covers)) = st.clip.as_ref() else { return };
        let me = unsafe { parent_rect(hwnd) };
        // Only what overlaps the window matters.
        let hit = |c: &[i32; 4]| c[0] < me[2] && c[2] > me[0] && c[1] < me[3] && c[3] > me[1];
        let covers: Vec<[i32; 4]> = covers.iter().copied().filter(hit).collect();
        let work = *work;
        let key: Vec<i32> = me.into_iter().chain(work).chain(covers.iter().flatten().copied()).collect();
        if st.clip_key.as_ref() == Some(&key) {
            return;
        }
        st.clip_key = Some(key);
        (me, work, covers)
    };
    let inside = work[0] <= me[0] && work[1] <= me[1] && work[2] >= me[2] && work[3] >= me[3];
    let (ox, oy, w, h) = (me[0], me[1], me[2] - me[0], me[3] - me[1]);
    unsafe {
        let whole = CreateRectRgn(0, 0, w, h);
        let rgn = if inside && covers.is_empty() {
            std::ptr::null_mut()
        } else {
            let rgn = CreateRectRgn(0, 0, w, h);
            let r = CreateRectRgn(work[0] - ox, work[1] - oy, work[2] - ox, work[3] - oy);
            CombineRgn(rgn, rgn, r, RGN_AND);
            DeleteObject(r);
            for c in &covers {
                let r = CreateRectRgn(c[0] - ox, c[1] - oy, c[2] - ox, c[3] - oy);
                CombineRgn(rgn, rgn, r, RGN_DIFF);
                DeleteObject(r);
            }
            rgn
        };
        // What the new region shows that the old one hid (no region = the whole window).
        let old = CreateRectRgn(0, 0, 0, 0);
        if GetWindowRgn(hwnd, old) == 0 {
            CombineRgn(old, whole, std::ptr::null_mut(), RGN_COPY);
        }
        let gained = CreateRectRgn(0, 0, 0, 0);
        CombineRgn(gained, if rgn.is_null() { whole } else { rgn }, old, RGN_DIFF);
        SetWindowRgn(hwnd, rgn, 1); // the system owns `rgn` now
        RedrawWindow(hwnd, std::ptr::null(), gained, RDW_INVALIDATE | RDW_ALLCHILDREN);
        for r in [whole, old, gained] {
            DeleteObject(r);
        }
    }
}

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
    let tab_plugin = !st.misc;
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
        if msg == os_hostlink::message() {
            // Plugin -> host (spec: plugins/host-link.md): queue it and wake the app so it is handled at once.
            let m = match os_hostlink::Cmd::from_usize(wp) {
                Some(os_hostlink::Cmd::Changed) => LinkMsg::Changed,
                Some(os_hostlink::Cmd::Note) => {
                    let (k, v, on) = os_hostlink::unpack_note(lp);
                    LinkMsg::Note(k, v, on)
                }
                Some(os_hostlink::Cmd::Automate) => LinkMsg::Automate(lp.max(0) as usize),
                _ => return 0,
            };
            if let Ok(mut q) = LINK.lock() {
                q.push((hwnd as isize, m));
            }
            os_clap::wake();
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
                // The whole frame is client area; the title bar is moved by hand (WM_LBUTTONDOWN).
                HTCLIENT as LRESULT
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
            WM_MOUSEACTIVATE => {
                // Any click brings the window to the front of the app's stack (spec: ui/plugin-window.md "Docking").
                SetWindowPos(hwnd, HWND_TOP, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
                w.state.borrow_mut().events.push(FrameEvent::Raise);
                os_clap::wake();
                DefWindowProcW(hwnd, msg, wp, lp)
            }
            WM_PARENTNOTIFY if matches!((wp & 0xffff) as u32, WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN) => {
                // A click inside the plugin's editor.
                SetWindowPos(hwnd, HWND_TOP, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
                w.state.borrow_mut().events.push(FrameEvent::Raise);
                os_clap::wake();
                0
            }
            WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
                let (x, y) = client_point(hwnd, lp, w.scale, false);
                let part = part_at(&w.state.borrow(), x, y);
                if part == Some(Part::Menu) {
                    // The app draws the menu (its own popup, like every other OpenStudio menu), below the arrow.
                    let m = MENU_RECT;
                    let mut p = POINT { x: (m.x * w.scale) as i32, y: (m.bottom() * w.scale) as i32 };
                    ClientToScreen(hwnd, &mut p);
                    w.state.borrow_mut().events.push(FrameEvent::Menu(p.x, p.y));
                    os_clap::wake();
                    return 0;
                }
                if part == Some(Part::Presets) {
                    // The preset list is an app popup too, under the box's left edge.
                    let b = presets_rect(w.state.borrow().size().0);
                    let mut p = POINT { x: (b.x * w.scale) as i32, y: (b.bottom() * w.scale) as i32 };
                    ClientToScreen(hwnd, &mut p);
                    w.state.borrow_mut().events.push(FrameEvent::Presets(p.x, p.y));
                    os_clap::wake();
                    return 0;
                }
                if let Some(d) = match part { Some(Part::PresetPrev) => Some(-1), Some(Part::PresetNext) => Some(1), _ => None } {
                    w.state.borrow_mut().events.push(FrameEvent::PresetStep(d));
                    os_clap::wake();
                    return 0;
                }
                if part.is_none() && misc_button(hwnd, w, x, y, Btn::Left) {
                    return 0;
                }
                let mut relayout = false;
                {
                    let mut st = w.state.borrow_mut();
                    match part {
                        Some(Part::Close) => st.events.push(FrameEvent::Close),
                        Some(Part::Minimize) => {
                            // A docked window hides (its channel's name button shows it again).
                            let cmd = if st.detached { SW_MINIMIZE } else { SW_HIDE };
                            drop(st);
                            ShowWindow(hwnd, cmd);
                            return 0;
                        }
                        Some(Part::Gear) => {
                            st.strip = !st.strip;
                            if !st.strip {
                                st.misc = false;
                            }
                            relayout = true;
                        }
                        Some(Part::Tab(t)) if t == st.pages.len() => {
                            st.misc = true;
                            relayout = true;
                        }
                        Some(Part::Tab(t)) => {
                            st.misc = false;
                            st.page = t;
                            st.events.push(FrameEvent::Page(t));
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
                        Some(Part::Caption) if msg == WM_LBUTTONDOWN => {
                            // Drag the window by the title bar.
                            let mut c: POINT = std::mem::zeroed();
                            let mut r: RECT = std::mem::zeroed();
                            GetCursorPos(&mut c);
                            GetWindowRect(hwnd, &mut r);
                            st.moving = Some((c.x, c.y, r.left, r.top));
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
            WM_RBUTTONDOWN => {
                let (x, y) = client_point(hwnd, lp, w.scale, false);
                misc_button(hwnd, w, x, y, Btn::Right);
                0
            }
            WM_MBUTTONDOWN => {
                // Middle click resets a knob (spec: ui/knobs.md, owner decision).
                let (x, y) = client_point(hwnd, lp, w.scale, false);
                if misc_button(hwnd, w, x, y, Btn::Middle) {
                    return 0;
                }
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
                let (x, y) = client_point(hwnd, lp, w.scale, false);
                if misc_move(hwnd, w, x, y) {
                    return 0;
                }
                let mut st = w.state.borrow_mut();
                if let Some((cx, cy, wx, wy)) = st.moving {
                    drop(st);
                    let mut c: POINT = std::mem::zeroed();
                    GetCursorPos(&mut c);
                    // Screen position; a docked window is placed in its parent's client area.
                    let mut p = POINT { x: wx + c.x - cx, y: wy + c.y - cy };
                    let docked = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32 & WS_CHILD != 0;
                    if docked {
                        ScreenToClient(GetParent(hwnd), &mut p);
                    }
                    SetWindowPos(hwnd, std::ptr::null_mut(), p.x, p.y, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
                    if docked {
                        // Paint what the move uncovered (the main window, other docked plugin windows and their
                        // editors) before the next move. Left to ordinary WM_PAINTs, which wait behind the mouse
                        // moves, the windows below lag behind and show trails of this one while it is dragged.
                        apply_clip(hwnd, &w.state);
                        EXPOSED.store(true, Ordering::Relaxed);
                        RedrawWindow(GetParent(hwnd), std::ptr::null(), std::ptr::null_mut(), RDW_UPDATENOW | RDW_ALLCHILDREN);
                    }
                    return 0;
                }
                if let Some((k, y0, v0)) = st.drag {
                    st.set_knob(k, v0 + (y0 - y) / 1024.0);
                    drop(st);
                    InvalidateRect(hwnd, std::ptr::null(), 0);
                }
                0
            }
            WM_LBUTTONUP => {
                ReleaseCapture();
                let mut st = w.state.borrow_mut();
                st.drag = None;
                st.moving = None;
                misc_release(&mut st);
                0
            }
            WM_CAPTURECHANGED => {
                let mut st = w.state.borrow_mut();
                st.drag = None;
                st.moving = None;
                misc_release(&mut st);
                0
            }
            WM_MOUSEWHEEL => {
                let (x, y) = client_point(hwnd, lp, w.scale, true);
                let notches = ((wp >> 16) & 0xffff) as u16 as i16 as f32 / 120.0;
                let part = part_at(&w.state.borrow(), x, y);
                if part.is_none() && misc_wheel(hwnd, w, x, y, notches) {
                    return 0;
                }
                if matches!(part, Some(Part::Presets | Part::PresetPrev | Part::PresetNext)) && notches != 0.0 {
                    // Wheel down = next preset.
                    w.state.borrow_mut().events.push(FrameEvent::PresetStep(if notches < 0.0 { 1 } else { -1 }));
                    os_clap::wake();
                    return 0;
                }
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

// ---------------------------------------------------------------------------------------------------------------
// Misc tab input (spec: ui/plugin-window.md "Misc tab"; the page itself is `misc_page`).

#[derive(Clone, Copy, PartialEq)]
enum Btn {
    Left,
    Right,
    Middle,
}

/// The Misc page hit under frame point (x, y), when the Misc tab shows.
fn misc_hit(st: &State, x: f32, y: f32) -> Option<crate::misc_page::Hit> {
    if !st.misc || st.info.misc.is_none() {
        return None;
    }
    let (ax, ay) = (BORDER, st.area_y());
    crate::misc_page::hit(&st.misc_ui, x - ax, y - ay)
}

/// Sets a Misc value on the page at once and tells the app.
fn misc_set(st: &mut State, m: crate::misc_page::M, v: f32) {
    if let Some(vals) = st.info.misc.as_mut() {
        vals.set(m, v);
    }
    st.events.push(FrameEvent::Misc(m, v));
    misc_hint(st, Some(crate::misc_page::Hit::Value(m)));
}

fn misc_hint(st: &mut State, h: Option<crate::misc_page::Hit>) {
    use crate::misc_page::{note_name, value_text, Hit, M};
    st.misc_ui.hint = match (h, &st.info.misc) {
        (Some(Hit::Value(m) | Hit::Step(m, _)), Some(v)) => format!("{}: {}", m.label(), value_text(m, v.get(m))),
        (Some(Hit::ArpDir(i)), _) => format!("Arpeggio direction: {}", value_text(M::ArpDir, f32::from(i))),
        (Some(Hit::Key(k)), _) => format!("{} (right click sets the root note)", note_name(i32::from(k))),
        (Some(Hit::CutSelf), _) => "Cut itself: a new note cuts the channel's playing notes".into(),
        (Some(Hit::Reset), _) => "Reset the root note and fine tuning".into(),
        (Some(Hit::TrackTab(k)), _) => (if k { "Key tracking" } else { "Velocity tracking" }).into(),
        _ => String::new(),
    };
}

/// A button press over the Misc page; returns whether the page took it.
unsafe fn misc_button(hwnd: HWND, w: &mut Win, x: f32, y: f32, b: Btn) -> bool {
    use crate::misc_page::{self as mp, Hit, M};
    let Some(h) = misc_hit(&w.state.borrow(), x, y) else { return false };
    let scale = w.scale;
    let mut st = w.state.borrow_mut();
    let Some(vals) = st.info.misc else { return false };
    let screen = || {
        let mut p = POINT { x: (x * scale) as i32, y: (y * scale) as i32 };
        unsafe { ClientToScreen(hwnd, &mut p) };
        (p.x, p.y)
    };
    match (b, h) {
        (Btn::Middle, Hit::Value(m)) => misc_set(&mut st, m, mp::default_of(m)),
        (Btn::Right, Hit::Value(m) | Hit::Step(m, _)) if mp::list_on_right(m) => {
            let (sx, sy) = screen();
            st.events.push(FrameEvent::MiscMenu(m, sx, sy));
        }
        (Btn::Right, Hit::Key(k)) => misc_set(&mut st, M::Root, f32::from(k)),
        (Btn::Left, Hit::Value(m)) if mp::list_on_click(m) => {
            let (sx, sy) = screen();
            st.events.push(FrameEvent::MiscMenu(m, sx, sy));
        }
        (Btn::Left, Hit::Value(m)) if mp::draggable(m) => {
            st.misc_ui.drag = Some((m, mp::drag_start(m, x - BORDER, y - st.area_y()), vals.get(m)));
            unsafe { SetCapture(hwnd) };
        }
        (Btn::Left, Hit::Value(m)) => {
            if let Some(v) = mp::click_value(m, vals.get(m)) {
                misc_set(&mut st, m, v);
            }
        }
        (Btn::Left, Hit::Step(m, d)) => {
            let (lo, hi) = mp::range_of(m);
            misc_set(&mut st, m, (vals.get(m) + d as f32).clamp(lo, hi));
        }
        (Btn::Left, Hit::ArpDir(i)) => misc_set(&mut st, M::ArpDir, f32::from(i)),
        (Btn::Left, Hit::TrackTab(k)) => st.misc_ui.track_key = k,
        (Btn::Left, Hit::CutSelf) => {
            // The channel cuts itself: cut and cut-by the same group (spec: ui/plugin-window.md "Misc tab").
            let g = if vals.get(M::Cut) >= 1.0 { vals.get(M::Cut) } else { 1.0 };
            misc_set(&mut st, M::Cut, g);
            misc_set(&mut st, M::CutBy, g);
        }
        (Btn::Left, Hit::Reset) => {
            misc_set(&mut st, M::Root, 60.0);
            misc_set(&mut st, M::Fine, 0.0);
        }
        (Btn::Left, Hit::Key(k)) => {
            st.misc_ui.held = Some(k);
            st.events.push(FrameEvent::Note(k, 100, true));
            unsafe { SetCapture(hwnd) };
        }
        _ => {}
    }
    drop(st);
    os_clap::wake();
    unsafe { InvalidateRect(hwnd, std::ptr::null(), 0) };
    true
}

/// Mouse move over / dragging on the Misc page; returns whether the page took it (a drag).
unsafe fn misc_move(hwnd: HWND, w: &mut Win, x: f32, y: f32) -> bool {
    let mut st = w.state.borrow_mut();
    if !st.misc {
        return false;
    }
    if st.misc_ui.drag.is_some() {
        let fine = unsafe { GetKeyState(VK_CONTROL as i32) } < 0;
        let ay = st.area_y();
        if let Some((m, v)) = crate::misc_page::drag_value(&st.misc_ui, x - BORDER, y - ay, fine) {
            if st.info.misc.is_some_and(|vals| vals.get(m) != v) {
                misc_set(&mut st, m, v);
                drop(st);
                os_clap::wake();
                unsafe { InvalidateRect(hwnd, std::ptr::null(), 0) };
            }
        }
        return true;
    }
    if st.drag.is_some() || st.moving.is_some() {
        return false;
    }
    let h = misc_hit(&st, x, y);
    if h != st.misc_ui.hover {
        st.misc_ui.hover = h;
        misc_hint(&mut st, h);
        drop(st);
        unsafe { InvalidateRect(hwnd, std::ptr::null(), 0) };
    }
    false
}

fn misc_release(st: &mut State) {
    st.misc_ui.drag = None;
    if let Some(k) = st.misc_ui.held.take() {
        st.events.push(FrameEvent::Note(k, 0, false));
        os_clap::wake();
    }
}

/// The wheel over the Misc page; returns whether the page took it.
unsafe fn misc_wheel(hwnd: HWND, w: &mut Win, x: f32, y: f32, notches: f32) -> bool {
    use crate::misc_page::{self as mp, Hit};
    let Some(h) = misc_hit(&w.state.borrow(), x, y) else { return false };
    let mut st = w.state.borrow_mut();
    match h {
        Hit::Value(m) | Hit::Step(m, _) => {
            let Some(v) = st.info.misc.map(|vals| vals.get(m)) else { return false };
            if let Some(nv) = mp::wheel_value(m, v, notches) {
                misc_set(&mut st, m, nv);
                os_clap::wake();
            }
        }
        Hit::Key(_) | Hit::Keyboard => mp::scroll_keyboard(&mut st.misc_ui, notches),
        _ => {}
    }
    drop(st);
    unsafe { InvalidateRect(hwnd, std::ptr::null(), 0) };
    true
}

/// Screen position for a new plugin window: right of the owner's left edge, below its toolbar (physical px).
pub fn default_position(owner: isize) -> (i32, i32) {
    let mut r: RECT = unsafe { std::mem::zeroed() };
    unsafe { GetWindowRect(owner as HWND, &mut r) };
    (r.left + 420, r.top + 160)
}

// ---------------------------------------------------------------------------------------------------------------
// Lifted overlay: an app popup opened from a detached plugin window would be painted in the main window, under the
// detached window (owned windows stay above their owner). While one is open, its pixels are shown in this small
// window above the plugin window, and its mouse input is passed to the main window as if it happened there.

struct Lift {
    hwnd: HWND,
    main: HWND,
    /// Main-window client position of the lifted rect's top-left.
    origin: (i32, i32),
    size: (i32, i32),
    buf: Vec<u32>,
}

static LIFT: Mutex<Option<LiftPtr>> = Mutex::new(None);
struct LiftPtr(Box<Lift>);
// SAFETY: only touched on the UI thread; the Mutex is just a global slot.
unsafe impl Send for LiftPtr {}

const LIFT_CLASS: &str = "OpenStudioLiftedPopup";

unsafe extern "system" fn lift_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        let Ok(g) = LIFT.lock() else { return DefWindowProcW(hwnd, msg, wp, lp) };
        let Some(l) = g.as_ref().map(|p| &p.0).filter(|l| l.hwnd == hwnd) else {
            drop(g);
            return DefWindowProcW(hwnd, msg, wp, lp);
        };
        match msg {
            WM_MOUSEACTIVATE => MA_NOACTIVATE as LRESULT,
            WM_ERASEBKGND => 1,
            WM_PAINT => {
                let mut ps: PAINTSTRUCT = std::mem::zeroed();
                let hdc = BeginPaint(hwnd, &mut ps);
                let (w, h) = l.size;
                if l.buf.len() == (w * h) as usize {
                    let mut bmi: BITMAPINFO = std::mem::zeroed();
                    bmi.bmiHeader = BITMAPINFOHEADER {
                        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                        biWidth: w,
                        biHeight: -h,
                        biPlanes: 1,
                        biBitCount: 32,
                        biCompression: BI_RGB,
                        ..std::mem::zeroed()
                    };
                    SetDIBitsToDevice(hdc, 0, 0, w as u32, h as u32, 0, 0, 0, h as u32, l.buf.as_ptr() as *const c_void, &bmi, DIB_RGB_COLORS);
                }
                EndPaint(hwnd, &ps);
                0
            }
            WM_MOUSEMOVE | WM_LBUTTONDOWN | WM_LBUTTONUP | WM_RBUTTONDOWN | WM_RBUTTONUP | WM_MBUTTONDOWN | WM_MBUTTONUP => {
                let x = (lp & 0xffff) as u16 as i16 as i32 + l.origin.0;
                let y = ((lp >> 16) & 0xffff) as u16 as i16 as i32 + l.origin.1;
                let main = l.main;
                drop(g);
                PostMessageW(main, msg, wp, ((y as u32 & 0xffff) << 16 | (x as u32 & 0xffff)) as LPARAM);
                0
            }
            _ => {
                drop(g);
                DefWindowProcW(hwnd, msg, wp, lp)
            }
        }
    }
}

/// Shows `pixels` (`size`, XRGB) above plugin window `frame`, at main-window client position `origin`.
pub fn lift_show(main: isize, frame: isize, origin: (i32, i32), size: (i32, i32), pixels: Vec<u32>) {
    unsafe {
        let mut p = POINT { x: origin.0, y: origin.1 };
        ClientToScreen(main as HWND, &mut p);
        let mut g = match LIFT.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        if g.is_none() {
            let inst = GetModuleHandleW(std::ptr::null());
            let class = wide(LIFT_CLASS);
            let wc = WNDCLASSW {
                style: 0,
                lpfnWndProc: Some(lift_proc),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: inst,
                hIcon: std::ptr::null_mut(),
                hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW),
                hbrBackground: std::ptr::null_mut(),
                lpszMenuName: std::ptr::null(),
                lpszClassName: class.as_ptr(),
            };
            RegisterClassW(&wc);
            *g = Some(LiftPtr(Box::new(Lift { hwnd: std::ptr::null_mut(), main: main as HWND, origin, size, buf: Vec::new() })));
            drop(g);
            let hwnd = CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                class.as_ptr(),
                std::ptr::null(),
                WS_POPUP,
                p.x,
                p.y,
                size.0,
                size.1,
                frame as HWND,
                std::ptr::null_mut(),
                inst,
                std::ptr::null(),
            );
            g = match LIFT.lock() {
                Ok(g) => g,
                Err(_) => return,
            };
            if let Some(l) = g.as_mut() {
                l.0.hwnd = hwnd;
            }
        }
        let Some(l) = g.as_mut().map(|p| &mut p.0) else { return };
        l.main = main as HWND;
        l.origin = origin;
        l.size = size;
        l.buf = pixels;
        let hwnd = l.hwnd;
        drop(g);
        SetWindowPos(hwnd, HWND_TOP, p.x, p.y, size.0, size.1, SWP_NOACTIVATE | SWP_SHOWWINDOW);
        InvalidateRect(hwnd, std::ptr::null(), 0);
    }
}

/// Hides the lifted popup (none open).
pub fn lift_hide() {
    let l = LIFT.lock().ok().and_then(|mut g| g.take());
    if let Some(l) = l {
        unsafe { DestroyWindow(l.0.hwnd) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(strip: bool, target: &str) -> State {
        let info = ChannelInfo { name: "Keys".into(), target: target.into(), enabled: true, volume: 0.78, pan: 0.0, pitch_cents: 0, range: 2, track: 0, misc: Some(Default::default()) };
        State { info, strip, pages: vec![PageIcon::Plugin], page: 0, misc: false, hint: String::new(), preset: None, misc_ui: Default::default(), editor: (480.0, 270.0), events: Vec::new(), drag: None, moving: None, detached: false, clip: None, clip_key: None }
    }

    #[test]
    fn frame_geometry() {
        // spec: ui/plugin-window.md "Frame": 7 px around the editor, 27 px title bar, 46 px strip.
        let st = state(true, "Master");
        assert_eq!(st.size(), (494.0, 74.0 + 270.0 + 7.0));
        assert_eq!(state(false, "Master").size().1, 28.0 + 270.0 + 7.0);
        let at = |x, y| part_at(&st, x, y);
        assert_eq!(at(15.0, 15.0), Some(Part::Menu));
        assert_eq!(at(34.0, 15.0), Some(Part::Gear));
        assert_eq!(at(390.0, 15.0), Some(Part::Presets));
        assert_eq!(at(422.0, 15.0), Some(Part::PresetPrev));
        assert_eq!(at(438.0, 15.0), Some(Part::PresetNext));
        assert_eq!(at(460.0, 15.0), Some(Part::Minimize));
        assert_eq!(at(479.0, 15.0), Some(Part::Close));
        assert_eq!(at(200.0, 15.0), Some(Part::Caption));
        assert_eq!(at(29.0, 50.0), Some(Part::Tab(0)));
        assert_eq!(at(74.0, 50.0), Some(Part::Tab(1)));
    }

    /// Renders the frame offscreen (`OS_SHOT_DIR=<dir> cargo test -p os-app frame_shots -- --ignored`).
    #[test]
    #[ignore]
    fn frame_shots() {
        let Some(dir) = std::env::var_os("OS_SHOT_DIR") else { return };
        let dir = std::path::PathBuf::from(dir);
        for (name, strip) in [("frame_strip.png", true), ("frame_bare.png", false)] {
            let st = state(strip, "Master");
            let (w, h) = st.size();
            let mut c = Canvas::new(w as u32, h as u32, 1.0).unwrap();
            paint(&mut c, &st);
            c.pixmap.save_png(dir.join(name)).unwrap();
        }
    }
}
