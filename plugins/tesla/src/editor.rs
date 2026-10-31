// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The plugin editor: a child of the host's window (Win32, X11 on Linux, an NSView on macOS) that paints `view` with
//! the os-ui canvas, and the editor logic (platform independent): selecting with the mouse (snapped), dragging flags,
//! zooming and scrolling, the transport and recording controls, the tool grid, the menus with Edison's shortcuts, the
//! Amp panel, files in and out (dialogs, the clipboard, dropping a file on the editor, dragging the sample out) and
//! resizing.
//!
// spec: plugins/tesla/README.md, plugins/edison/window.md, plugins/edison/menus.md, plugins/edison/tools.md,
// spec: plugins/host-link.md

use std::any::Any;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicIsize, AtomicU32, Ordering};
use std::sync::Arc;

use nih_plug::prelude::*;
use os_plugkit::MenuDef;
use os_ui::kit::menu::{Mark, MenuItem, MenuStyle};
use os_ui::{Canvas, Rect};

use crate::doc::{self, Doc, Paste};
use crate::env;
use crate::peaks::Peaks;
use crate::sample::{Format, Sample};
use crate::state;
use crate::spectral::{self, Blur, Clean, Curve, Equalize, Profile};
use crate::tools::{self, Amp, Law, Reverb, Stretch};
use crate::view::{self, AmpCtl, AmpUi, Bt, Hit, Layout, MenuId, Part, Shown, Target, Tr, ViewState};
use crate::wav::{self, Depth};
use crate::{Cmd as Play, Shared, TeslaParams};

pub struct TeslaEditor {
    pub params: Arc<TeslaParams>,
    /// DPI scale as f32 bits.
    pub scale: AtomicU32,
    /// The open editor window (0 = closed).
    #[cfg_attr(not(windows), allow(dead_code))]
    hwnd: Arc<AtomicIsize>,
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    slot: os_plugkit::gui::Slot,
}

impl TeslaEditor {
    pub fn new(params: Arc<TeslaParams>) -> Self {
        TeslaEditor {
            params,
            scale: AtomicU32::new(1f32.to_bits()),
            hwnd: Arc::new(AtomicIsize::new(0)),
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            slot: Default::default(),
        }
    }
}

impl Editor for TeslaEditor {
    fn spawn(&self, parent: ParentWindowHandle, context: Arc<dyn GuiContext>) -> Box<dyn Any + Send> {
        let scale = f32::from_bits(self.scale.load(Ordering::Relaxed));
        match parent {
            #[cfg(windows)]
            ParentWindowHandle::Win32Hwnd(hwnd) => Box::new(win32::open(hwnd, scale, self.params.clone(), context, self.hwnd.clone())),
            #[cfg(target_os = "linux")]
            ParentWindowHandle::X11Window(window) => Box::new(gui::open(window, scale, self.params.clone(), context, &self.slot)),
            #[cfg(target_os = "macos")]
            ParentWindowHandle::AppKitNsView(view) => {
                let scale = os_plugkit::gui::backing_scale(view as usize);
                Box::new(gui::open(view as usize, scale, self.params.clone(), context, &self.slot))
            }
            _ => {
                let _ = (context, scale);
                Box::new(())
            }
        }
    }

    fn size(&self) -> (u32, u32) {
        let s = &self.params.shared.size;
        (s.0.load(Ordering::Relaxed), s.1.load(Ordering::Relaxed))
    }

    fn set_scale_factor(&self, factor: f32) -> bool {
        self.scale.store(factor.to_bits(), Ordering::Relaxed);
        true
    }

    fn param_value_changed(&self, _id: &str, _normalized_value: f32) {}
    fn param_modulation_changed(&self, _id: &str, _modulation_offset: f32) {}
    fn param_values_changed(&self) {}
}

// ---------------------------------------------------------------------------------------------------------------
// Files

/// A sample file: WAV with its chunks (Edison's and Tesla's own), else whatever the host's decoders read (AIFF, FLAC,
/// OGG...); a decoded file whose channels are equal comes in mono.
pub fn load_file(path: &str) -> Result<Sample, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    if wav::is_wav(&bytes) {
        if let Ok(s) = wav::read(&bytes) {
            return Ok(s);
        }
    }
    let s = os_engine::sample::load(Path::new(path)).map_err(|e| e.to_string())?;
    let ch = if s.left == s.right { vec![s.left] } else { vec![s.left, s.right] };
    Ok(Sample::from_channels(s.rate, ch, Format::Float32))
}

/// A file name made of `title` (characters a file system refuses are replaced).
fn file_stem(title: &str) -> String {
    let t: String = title.chars().map(|c| if "<>:\"/\\|?*".contains(c) || c.is_control() { '_' } else { c }).collect();
    let t = t.trim().trim_end_matches(".wav").trim_end_matches(".WAV").to_string();
    if t.is_empty() { "Tesla".into() } else { t }
}

/// Where a sample handed out of the plugin (dragged out, sent to the host) is written.
fn out_path(title: &str) -> PathBuf {
    let dir = std::env::temp_dir().join("Tesla");
    let _ = std::fs::create_dir_all(&dir);
    let stem = file_stem(title);
    let mut p = dir.join(format!("{stem}.wav"));
    let mut k = 2;
    // A file handed out before may be used by the host (a clip, a channel's sample): never overwritten, a new name.
    while p.exists() {
        p = dir.join(format!("{stem} {k}.wav"));
        k += 1;
    }
    p
}

// ---------------------------------------------------------------------------------------------------------------
// Menus

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolCmd {
    Amp,
    Stretch,
    Reverb,
    /// The spectral panels, and Acquire noise profile (Clean up's).
    Equalize,
    Blur,
    CleanUp,
    AcquireNoise,
    ReversePolarity,
    Normalize,
    FadeIn,
    FadeOut,
    DeclickIn,
    DeclickOut,
    Center,
    Reverse,
    Swap,
    LeftToMono,
    RightToMono,
    MonoSignal,
    Noise,
    NormalizeRegions,
    DeclickInRegions,
    DeclickOutRegions,
    /// Tools > Mixdown amp envelopes and the Envelope section.
    Mixdown,
    CancelEnvelopes,
    PointsAtSelection,
    PointsAtRegions,
    FlipEnvelope,
    NormalizeEnvelope,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Cmd {
    None,
    New,
    Open,
    Save(Depth),
    About,
    Properties,
    Format(Format),
    Channels(usize),
    Undo,
    Redo,
    UndoTo(usize),
    UndoLarge,
    Cut,
    Copy,
    Paste(Paste),
    Clear,
    Delete,
    Trim,
    DeleteAfterLoop,
    InsertSilence,
    ClickFree,
    Tool(ToolCmd),
    Send(bool),
    AddRegion,
    AddMarker,
    SetLoop,
    ClearLoop,
    DeleteCue(u32),
    DeleteCueHere,
    RenameCue(u32),
    AutoRename,
    Freeze,
    Spectrum,
    Regions,
    LoopView,
    Time(u8),
    Bands(u8),
    Gradient(u8),
    Snap(u8),
    Deselect,
    SelectBefore,
    SelectAfter,
    SelectZoomed,
    SelectAll,
    SelectRegion(i32),
    ZoomIn,
    ZoomOut,
    ZoomFull,
    ZoomSel,
    ZoomLeft,
    ZoomRight,
    When(u8),
    Max(u8),
    IgnoreHostSel,
    IgnoreOwnSel,
}

/// Which menu is open.
#[derive(Clone, Debug, PartialEq)]
pub enum MenuReq {
    Main(MenuId),
    When,
    Max,
    Undo,
    Flag(u32),
    LoopFlag,
    Slave,
    /// Right-click in the wave area.
    Wave,
}

/// A name or value the user types.
#[derive(Clone, Debug, PartialEq)]
pub enum Prompt {
    Marker,
    Region,
    Rename(u32),
    Tempo,
}

// ---------------------------------------------------------------------------------------------------------------
// Editor logic

/// Something the window has to do after an event.
#[derive(Clone, Debug, PartialEq)]
pub enum Act {
    Repaint,
    Menu(MenuReq, f32, f32),
    TypeIn(Prompt, f32, f32),
    /// The open dialog, then `Logic::open`.
    Open,
    /// The save dialog, then `Logic::save`.
    Save(Depth),
    /// Put WAV bytes on the system clipboard.
    Clipboard(Vec<u8>),
    /// Paste the system clipboard's audio (`Logic::paste`).
    Paste(Paste),
    /// Drag this file out of the editor.
    DragOut(PathBuf),
    /// Hand this file to the host: to the playlist as an audio clip, or (true) to the selected channel.
    Send(PathBuf, bool),
    /// The editor's new size in logical px.
    Resize(u32, u32),
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Mods {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Btn {
    Left,
    Right,
    Middle,
}

/// The keys the editor answers (from Win32 virtual keys or X11 key symbols).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    /// A letter (upper case) or digit.
    Char(char),
    Space,
    Delete,
    Insert,
    Left,
    Right,
    Up,
    Down,
    PageUp,
    PageDown,
    Home,
    End,
    F2,
    Escape,
}

impl Key {
    /// A Win32 virtual key.
    pub fn from_vk(vk: u32) -> Option<Key> {
        Some(match vk {
            0x20 => Key::Space,
            0x2e => Key::Delete,
            0x2d => Key::Insert,
            0x25 => Key::Left,
            0x27 => Key::Right,
            0x26 => Key::Up,
            0x28 => Key::Down,
            0x21 => Key::PageUp,
            0x22 => Key::PageDown,
            0x24 => Key::Home,
            0x23 => Key::End,
            0x71 => Key::F2,
            0x1b => Key::Escape,
            0x30..=0x39 | 0x41..=0x5a => Key::Char(char::from_u32(vk)?),
            _ => return None,
        })
    }

    /// An X11 key symbol.
    pub fn from_sym(sym: u32) -> Option<Key> {
        Some(match sym {
            0x20 => Key::Space,
            0xffff => Key::Delete,
            0xff63 => Key::Insert,
            0xff51 => Key::Left,
            0xff53 => Key::Right,
            0xff52 => Key::Up,
            0xff54 => Key::Down,
            0xff55 => Key::PageUp,
            0xff56 => Key::PageDown,
            0xff50 => Key::Home,
            0xff57 => Key::End,
            0xffbf => Key::F2,
            0xff1b => Key::Escape,
            0x30..=0x39 => Key::Char(char::from_u32(sym)?),
            0x41..=0x5a | 0x61..=0x7a => Key::Char(char::from_u32(sym)?.to_ascii_uppercase()),
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Drag {
    /// Selecting from `anchor` (frames), pressed at `x`.
    Select { anchor: usize, x: f32 },
    /// A flag: its id, the frame under the press, the flag's position then, whether it has moved.
    Flag { id: u32, grab: f64, pos: usize, moved: bool },
    /// The scroll bar's thumb, grabbed this many frames from its start.
    Scroll { grab: f64 },
    Scrub,
    Resize { x: f32, y: f32, w: u32, h: u32 },
    Knob { ctl: AmpCtl, y: f32, v: f32 },
    Fader(AmpCtl),
    /// Envelope `k`'s point `i`; `moved` once its undo step is recorded.
    Point { k: usize, i: usize, moved: bool },
    /// A spectral panel's curve point.
    CurvePoint(usize),
}

pub struct Logic {
    pub shared: Arc<Shared>,
    pub view: ViewState,
    pub hits: Vec<Hit>,
    pub hint: String,
    /// The host draws the hint (host link).
    pub hosted: bool,
    drag: Option<Drag>,
    /// The editor's own clipboard (the system's is used where there is one).
    clip: Option<Sample>,
    /// Clean up's noise profile (Acquire noise profile), kept until another is acquired.
    noise: Option<Profile>,
    peaks: Peaks,
    peaks_ver: u64,
    take_peaks: Peaks,
    take_ver: u64,
    spectrum: Vec<u32>,
    spectrum_key: Option<(u64, u64, u64, u64, usize, usize, u8, u8)>,
    loaded: u64,
}

impl Logic {
    pub fn new(params: Arc<TeslaParams>) -> Self {
        let shared = params.shared.clone();
        let loaded = shared.loaded.load(Ordering::Acquire);
        Logic {
            shared,
            view: ViewState::default(),
            hits: Vec::new(),
            hint: String::new(),
            hosted: false,
            drag: None,
            clip: None,
            noise: None,
            peaks: Peaks::default(),
            peaks_ver: u64::MAX,
            take_peaks: Peaks::default(),
            take_ver: u64::MAX,
            spectrum: Vec::new(),
            spectrum_key: None,
            loaded,
        }
    }

    pub fn size(&self) -> (u32, u32) {
        (self.shared.size.0.load(Ordering::Relaxed), self.shared.size.1.load(Ordering::Relaxed))
    }

    fn layout(&self) -> Layout {
        let (w, h) = self.size();
        Layout::new(w, h)
    }

    /// The host resized the editor's frame: the editor takes `w` × `h` (logical px), at least its smallest size.
    pub fn resize_to(&mut self, w: u32, h: u32) -> Vec<Act> {
        let (w, h) = (w.max(view::MIN_W), h.max(view::MIN_H));
        if (w, h) == self.size() {
            return vec![];
        }
        self.shared.size.0.store(w, Ordering::Relaxed);
        self.shared.size.1.store(h, Ordering::Relaxed);
        vec![Act::Resize(w, h), Act::Repaint]
    }

    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    fn at(&self, x: f32, y: f32) -> Option<Target> {
        self.hits.iter().rev().find(|h| h.r.contains(x, y)).map(|h| h.t)
    }

    fn hit_rect(&self, t: Target) -> Option<Rect> {
        self.hits.iter().rev().find(|h| h.t == t).map(|h| h.r)
    }

    fn doc<R>(&self, f: impl FnOnce(&mut Doc) -> R) -> R {
        self.shared.with_doc(f)
    }

    /// The frame under `x` in the wave area (unclamped, fractional).
    fn frame_at(&self, d: &Doc, x: f32) -> f64 {
        let lane = self.layout().lane();
        let (start, per_px) = view::span(d, d.frames(), lane.w);
        start + (x - lane.x) as f64 * per_px
    }

    /// `f` snapped as View > Snap says (when snapping is on), clamped to the sample.
    fn snap(&self, d: &Doc, f: f64, m: Mods) -> usize {
        let n = d.frames();
        if n == 0 {
            return 0;
        }
        let f = f.clamp(0.0, n as f64);
        let lane = self.layout().lane();
        let (_, per_px) = view::span(d, n, lane.w);
        // Alt holds snapping off for the drag, as in Lemon's other editors.
        if !d.view.snap_on || m.alt {
            return f.round() as usize;
        }
        let near = |cands: &mut dyn Iterator<Item = usize>| -> Option<usize> {
            let best = cands.min_by(|a, b| (*a as f64 - f).abs().total_cmp(&(*b as f64 - f).abs()))?;
            ((best as f64 - f).abs() <= per_px * 10.0).then_some(best)
        };
        let s = &d.sample;
        let r = match d.view.snap {
            state::SNAP_GRID => {
                let step = view::grid_step(d.view.time, s.rate, s.tempo, per_px, 90.0) / 4.0;
                Some(((f / step).round() * step).round() as usize)
            }
            state::SNAP_REGIONS => {
                let mut c = vec![0, n];
                for q in &s.cues {
                    c.push(q.pos);
                    if let Some(l) = q.len {
                        c.push(q.pos + l);
                    }
                }
                if let Some((a, b)) = s.looped {
                    c.extend([a, b + 1]);
                }
                near(&mut c.into_iter())
            }
            state::SNAP_ZERO => {
                let (lo, hi) = ((f - per_px * 8.0).max(1.0) as usize, ((f + per_px * 8.0) as usize).min(n - 1));
                let ch = &s.ch[0];
                near(&mut (lo..=hi).filter(|&i| i < n && (ch[i - 1] <= 0.0) != (ch[i] <= 0.0)))
            }
            _ => None,
        };
        r.unwrap_or(f.round() as usize).min(n)
    }

    // ------------------------------------------------------------------------------------------- view

    /// Shows `start .. start + len` (frames); the whole sample when it fits.
    fn set_view(d: &mut Doc, lane_w: f32, start: f64, len: f64) {
        let n = d.frames() as f64;
        let min = (lane_w as f64 * 0.05).max(4.0).min(n);
        let len = len.clamp(min, n.max(min));
        if len >= n {
            d.view.start = 0.0;
            d.view.len = 0.0;
        } else {
            d.view.len = len;
            d.view.start = start.clamp(0.0, n - len);
        }
    }

    /// Zooms by `factor` (less than 1 = in) around frame `around` (the view's centre by default).
    pub fn zoom(&mut self, factor: f64, around: Option<f64>) {
        let w = self.layout().lane().w;
        self.doc(|d| {
            if d.frames() == 0 {
                return;
            }
            let (start, per_px) = view::span(d, d.frames(), w);
            let len = per_px * w as f64;
            let at = around.unwrap_or(start + len / 2.0);
            let nl = len * factor;
            Self::set_view(d, w, at - (at - start) * factor, nl);
        });
    }

    fn zoom_to(&mut self, a: f64, b: f64) {
        let w = self.layout().lane().w;
        self.doc(|d| Self::set_view(d, w, a, (b - a).max(1.0)));
    }

    fn scroll_by(&mut self, px: f32) {
        let w = self.layout().lane().w;
        self.doc(|d| {
            let (start, per_px) = view::span(d, d.frames(), w);
            Self::set_view(d, w, start + px as f64 * per_px, per_px * w as f64);
        });
    }

    // ------------------------------------------------------------------------------------------- transport

    fn play_pause(&mut self) {
        if self.shared.playing.load(Ordering::Relaxed) {
            self.shared.send(Play::Pause);
            self.shared.playing.store(false, Ordering::Relaxed);
            return;
        }
        let at = self.shared.position();
        let r = self.doc(|d| d.range());
        if let Some((a, b)) = r {
            self.shared.send(Play::Play { from: a, to: b, at });
            self.shared.playing.store(true, Ordering::Relaxed);
        }
    }

    fn stop(&mut self) {
        let at = self.doc(|d| d.sel.map_or(0, |(a, _)| a)) as f64;
        self.shared.send(Play::Stop { at });
        self.shared.playing.store(false, Ordering::Relaxed);
        self.shared.pos.store(at.to_bits(), Ordering::Relaxed);
    }

    fn toggle(a: &std::sync::atomic::AtomicBool) {
        a.store(!a.load(Ordering::Relaxed), Ordering::Release);
    }

    // ------------------------------------------------------------------------------------------- mouse

    pub fn down(&mut self, x: f32, y: f32, btn: Btn, m: Mods, dbl: bool) -> Vec<Act> {
        self.view.mouse = Some((x, y));
        let Some(t) = self.at(x, y) else { return vec![] };
        self.view.pressed = Some(t);
        if self.view.amp.is_some() && !matches!(t, Target::Amp(_) | Target::AmpPanel) {
            // The Amp panel is modal over the editor.
            return vec![Act::Repaint];
        }
        if btn == Btn::Right {
            return self.right(t, x, y);
        }
        if btn == Btn::Middle {
            // A middle click resets an Amp control, as a right click does.
            if let Target::Amp(c) = t {
                self.reset_amp(c);
                return vec![Act::Repaint];
            }
            return vec![];
        }
        let mut acts = vec![Act::Repaint];
        match t {
            Target::Transport(Tr::Slave) => Self::toggle(&self.shared.slave),
            Target::Transport(Tr::Loop) => Self::toggle(&self.shared.looping),
            Target::Transport(Tr::Play) => self.play_pause(),
            Target::Transport(Tr::Stop) => self.stop(),
            Target::Transport(Tr::Scrub) => self.drag = Some(Drag::Scrub),
            Target::Transport(Tr::Record) => Self::toggle(&self.shared.armed),
            Target::When => acts.push(Act::Menu(MenuReq::When, x, y)),
            Target::Max => acts.push(Act::Menu(MenuReq::Max, x, y)),
            Target::Append => Self::toggle(&self.shared.append),
            Target::Mute => Self::toggle(&self.shared.mute),
            Target::Menu(id) => {
                let r = self.hit_rect(t).unwrap_or(Rect::new(x, y, 0.0, 0.0));
                acts.push(Act::Menu(MenuReq::Main(id), r.x, r.bottom()));
            }
            Target::Tool(i) => acts.extend(self.tool(i, false, x, y)),
            Target::Wordmark => self.hint = about(),
            Target::Format => {
                let n = self.doc(|d| d.sample.channels());
                self.doc(|d| d.set_channels(if n == 1 { 2 } else { 1 }));
            }
            Target::Wave => acts.extend(self.wave_down(x, y, m, dbl)),
            Target::Flag(id) => {
                let (grab, pos) = self.doc(|d| (self.frame_at(d, x), d.sample.cues.iter().find(|c| c.id == id).map_or(0, |c| c.pos)));
                if dbl {
                    acts.push(Act::TypeIn(Prompt::Rename(id), x, y));
                } else {
                    // A click selects a region (a marker puts the cursor on it); a drag moves the flag.
                    self.doc(|d| {
                        if let Some(c) = d.sample.cues.iter().find(|c| c.id == id).cloned() {
                            match c.len {
                                Some(l) => d.select(c.pos, c.pos + l - 1),
                                None => {
                                    d.sel = None;
                                    d.cursor = c.pos;
                                }
                            }
                        }
                    });
                    self.drag = Some(Drag::Flag { id, grab, pos, moved: false });
                }
            }
            Target::LoopFlag => {
                self.doc(|d| {
                    if let Some((a, b)) = d.sample.looped {
                        d.select(a, b);
                    }
                });
            }
            Target::Scroll => {
                let lay = self.layout();
                let inner = lay.overview();
                let (start, len, n) = self.doc(|d| {
                    let (s, p) = view::span(d, d.frames(), lay.lane().w);
                    (s, p * lay.lane().w as f64, d.frames() as f64)
                });
                if n > 0.0 {
                    let f = (x - inner.x) as f64 / inner.w as f64 * n;
                    let grab = if f >= start && f <= start + len { f - start } else { len / 2.0 };
                    self.drag = Some(Drag::Scroll { grab });
                    acts.extend(self.moved(x, y, m));
                }
            }
            Target::ScrollStep(right) => self.scroll_by(if right { 40.0 } else { -40.0 }),
            Target::Bottom(b) => acts.extend(self.bottom(b, x, y)),
            Target::Grip => {
                let (w, h) = self.size();
                self.drag = Some(Drag::Resize { x, y, w, h });
            }
            Target::Amp(c) => acts.extend(self.amp_down(c, y, dbl)),
            Target::AmpPanel => {}
        }
        acts
    }

    fn right(&mut self, t: Target, x: f32, y: f32) -> Vec<Act> {
        let mut acts = vec![Act::Repaint];
        match t {
            Target::Tool(i) => acts.extend(self.tool(i, true, x, y)),
            Target::Flag(id) => acts.push(Act::Menu(MenuReq::Flag(id), x, y)),
            Target::LoopFlag => acts.push(Act::Menu(MenuReq::LoopFlag, x, y)),
            Target::Transport(Tr::Slave) => acts.push(Act::Menu(MenuReq::Slave, x, y)),
            Target::Bottom(Bt::Undo) => acts.push(Act::Menu(MenuReq::Undo, x, y)),
            Target::Format => acts.push(Act::Menu(MenuReq::Main(MenuId::Format), x, y)),
            Target::When => acts.push(Act::Menu(MenuReq::When, x, y)),
            Target::Max => acts.push(Act::Menu(MenuReq::Max, x, y)),
            Target::Wave => {
                // In an envelope mode a right click on a point deletes it.
                let k = self.doc(|d| if d.frames() > 0 { d.env_shown() } else { None });
                let hit = k.and_then(|k| self.doc(|d| self.env_point_at(d, k, x, y)).map(|i| (k, i)));
                match hit {
                    Some((k, i)) => {
                        let removed = self.doc(|d| {
                            let n = d.frames();
                            let mut e = d.env.clone();
                            if !e.remove(k, n, i) {
                                return false;
                            }
                            *d.edit_env("Delete envelope point") = e;
                            true
                        });
                        if !removed {
                            self.hint = "The first and last points stay (drag them, or double-click to reset)".into();
                        }
                    }
                    None => acts.push(Act::Menu(MenuReq::Wave, x, y)),
                }
            }
            Target::Amp(AmpCtl::Curve) => {
                let i = self.curve_point_at(x, y);
                let removed = match (i, self.view.amp.as_mut()) {
                    (Some(i), Some(ui)) => ui.curve.remove(i),
                    _ => false,
                };
                if removed {
                    self.amp_changed();
                } else if i.is_some() {
                    self.hint = "The first and last points stay (drag them up or down)".into();
                }
            }
            // Right-click resets a control.
            Target::Amp(c) => self.reset_amp(c),
            _ => {}
        }
        acts
    }

    /// Amp panel control `c` back to its default (its knobs and faders; the panel's buttons have none).
    fn reset_amp(&mut self, c: AmpCtl) {
        let d = Amp::default();
        let defaults = AmpUi { eq: Some(Equalize::default()), blur: Some(Blur::default()), clean: Some(Clean::default()), ..AmpUi::default() };
        let v = match c {
            AmpCtl::Mix | AmpCtl::Amount | AmpCtl::Offset | AmpCtl::Threshold | AmpCtl::Reduction | AmpCtl::ClipLevel => {
                Self::spectral_value(&defaults, c).unwrap_or(0.0)
            }
            AmpCtl::Pan => d.pan,
            AmpCtl::Sep => d.sep,
            AmpCtl::Left => d.left,
            AmpCtl::Volume => d.volume,
            AmpCtl::Right => d.right,
            AmpCtl::Pitch | AmpCtl::Time | AmpCtl::Formant | AmpCtl::Dry | AmpCtl::Decay => 0.0,
            AmpCtl::Wet => tools::WET_DEFAULT,
            _ => return,
        };
        self.set_amp_value(c, v);
        self.amp_changed();
    }

    /// The point of envelope `k` under (`x`, `y`), the nearest within grabbing distance.
    fn env_point_at(&self, d: &Doc, k: usize, x: f32, y: f32) -> Option<usize> {
        let lane = self.layout().lane();
        let n = d.frames();
        let (start, per_px) = view::span(d, n, lane.w);
        let p = d.env.points(k, n);
        p.iter()
            .enumerate()
            .map(|(i, q)| (i, (view::frame_x(lane, start, per_px, q.pos) - x).hypot(view::env_y(lane, q.v) - y)))
            .filter(|&(_, dist)| dist <= view::GRAB)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i)
    }

    /// A press in the wave area in an envelope mode: on a point drags it (a double-click puts it back at mid height),
    /// on the line adds a point there and drags it. `None` elsewhere: the press selects, as in waveform mode.
    fn env_down(&mut self, k: usize, x: f32, y: f32, dbl: bool) -> Option<Vec<Act>> {
        let lane = self.layout().lane();
        if let Some(i) = self.doc(|d| self.env_point_at(d, k, x, y)) {
            if dbl {
                self.doc(|d| {
                    let n = d.frames();
                    let p = d.env.points(k, n)[i];
                    if p.v != env::MID {
                        d.edit_env("Reset envelope point").move_point(k, n, i, p.pos, env::MID);
                    }
                });
                self.drag = None;
            } else {
                self.drag = Some(Drag::Point { k, i, moved: false });
            }
            return Some(vec![Act::Repaint]);
        }
        let added = self.doc(|d| {
            let n = d.frames();
            let f = self.frame_at(d, x).round();
            if f < 0.0 || f > (n - 1) as f64 {
                return None;
            }
            let v = env::value(&d.env.points(k, n), f);
            if (view::env_y(lane, v) - y).abs() > view::GRAB {
                return None;
            }
            Some(d.edit_env("Add envelope point").insert(k, n, f, view::env_v(lane, y)))
        })?;
        self.drag = Some(Drag::Point { k, i: added, moved: true });
        self.hint = format!("{}: {}", env::NAMES[k], env::text(k, view::env_v(lane, y)));
        Some(vec![Act::Repaint])
    }

    fn wave_down(&mut self, x: f32, y: f32, m: Mods, dbl: bool) -> Vec<Act> {
        if self.doc(|d| d.frames()) == 0 {
            return vec![Act::Open];
        }
        if let Some(k) = self.doc(|d| d.env_shown()) {
            if let Some(acts) = self.env_down(k, x, y, dbl) {
                return acts;
            }
        }
        if dbl {
            // Double-click: the region under the pointer, or the whole sample.
            self.doc(|d| {
                let f = self.frame_at(d, x).max(0.0) as usize;
                let r = d.sample.cues.iter().rev().find(|c| c.is_region() && f >= c.pos && f <= c.end()).map(|c| (c.pos, c.end()));
                let (a, b) = r.unwrap_or((0, d.frames() - 1));
                d.select(a, b);
            });
            self.drag = None;
            return vec![];
        }
        let (anchor, keep) = self.doc(|d| {
            let f = self.snap(d, self.frame_at(d, x), m);
            // Shift extends the selection from its far end.
            match (m.shift, d.sel) {
                (true, Some((a, b))) => {
                    let far = if (f as isize - a as isize).abs() > (f as isize - b as isize).abs() { a } else { b + 1 };
                    (far, true)
                }
                _ => (f, false),
            }
        });
        self.drag = Some(Drag::Select { anchor, x: if keep { f32::NAN } else { x } });
        if keep {
            return self.moved(x, 0.0, m);
        }
        self.doc(|d| {
            d.sel = None;
            d.cursor = anchor.min(d.frames());
        });
        vec![]
    }

    pub fn moved(&mut self, x: f32, y: f32, m: Mods) -> Vec<Act> {
        if !self.dragging() {
            self.view.mouse = Some((x, y));
            self.hint = self.hover_hint(x, y);
            return vec![Act::Repaint];
        }
        let lay = self.layout();
        match self.drag.clone() {
            Some(Drag::Select { anchor, x: x0 }) => {
                self.doc(|d| {
                    let f = self.snap(d, self.frame_at(d, x), m);
                    if !x0.is_nan() && (x - x0).abs() < 3.0 {
                        d.sel = None;
                        d.cursor = anchor;
                    } else if f != anchor {
                        d.select(anchor.min(f), anchor.max(f) - 1);
                    }
                });
                let len = self.doc(|d| d.sel.map(|(a, b)| (b + 1 - a, d.sample.rate)));
                if let Some((n, rate)) = len {
                    self.hint = format!("Selection: {}", view::time_text(state::TIME_MSM, n as f64, rate, 0.0, 1.0));
                }
            }
            Some(Drag::Flag { id, grab, pos, moved }) => {
                if self.doc(|d| d.view.freeze) {
                    self.hint = "Regions are frozen (Regions > Freeze all)".into();
                    return vec![Act::Repaint];
                }
                self.doc(|d| {
                    let f = self.frame_at(d, x);
                    let to = self.snap(d, pos as f64 + f - grab, m);
                    if to != pos || moved {
                        d.move_cue(id, to, !moved);
                    }
                });
                if let Some(Drag::Flag { moved, .. }) = &mut self.drag {
                    *moved = true;
                }
            }
            Some(Drag::Scroll { grab }) => {
                let inner = lay.overview();
                let w = lay.lane().w;
                self.doc(|d| {
                    let n = d.frames() as f64;
                    let (_, per_px) = view::span(d, d.frames(), w);
                    let f = (x - inner.x) as f64 / inner.w as f64 * n;
                    Self::set_view(d, w, f - grab, per_px * w as f64);
                });
            }
            Some(Drag::Scrub) => {
                let f = self.doc(|d| self.frame_at(d, x));
                self.shared.send(Play::Scrub(Some(f.max(0.0))));
            }
            Some(Drag::Resize { x: x0, y: y0, w, h }) => {
                let nw = ((w as f32 + x - x0).round() as u32).max(view::MIN_W);
                let nh = ((h as f32 + y - y0).round() as u32).max(view::MIN_H);
                if (nw, nh) != self.size() {
                    self.shared.size.0.store(nw, Ordering::Relaxed);
                    self.shared.size.1.store(nh, Ordering::Relaxed);
                    return vec![Act::Resize(nw, nh), Act::Repaint];
                }
            }
            Some(Drag::Knob { ctl, y: y0, v }) => {
                let nv = (v + (y0 - y) / 100.0).clamp(-1.0, 1.0);
                self.set_amp_value(ctl, nv);
                self.hint = self.amp_hint(ctl);
            }
            Some(Drag::Point { k, i, moved }) => {
                let lane = lay.lane();
                let v = view::env_v(lane, y);
                self.doc(|d| {
                    let n = d.frames();
                    if n == 0 {
                        return;
                    }
                    let pos = self.frame_at(d, x).round().clamp(0.0, (n - 1) as f64);
                    let e = if moved { d.env_mut() } else { d.edit_env("Move envelope point") };
                    e.move_point(k, n, i, pos, v);
                });
                if let Some(Drag::Point { moved, .. }) = &mut self.drag {
                    *moved = true;
                }
                self.hint = format!("{}: {}", env::NAMES[k], env::text(k, v));
            }
            Some(Drag::CurvePoint(i)) => {
                if let Some(ui) = &mut self.view.amp {
                    let (_, bx, _) = view::spectral_parts(&lay, ui);
                    let (px, v) = (((x - bx.x) / bx.w).clamp(0.0, 1.0) as f64, ((bx.bottom() - y) / bx.h).clamp(0.0, 1.0));
                    ui.curve.move_point(i, px, v);
                }
                self.hint = self.curve_hint();
            }
            Some(Drag::Fader(ctl)) => {
                let (_, parts) = view::amp_parts(&lay);
                if let Some((_, r)) = parts.iter().find(|(c, _)| *c == ctl) {
                    let (top, bot) = view::fader_travel(*r);
                    let v = ((bot - y) / (bot - top) * 2.0 - 1.0).clamp(-1.0, 1.0);
                    // The middle (×1) catches the fader.
                    let v = if (v - 0.5).abs() < 0.02 { 0.5 } else { v };
                    self.set_amp_value(ctl, v);
                    self.hint = self.amp_hint(ctl);
                }
            }
            None => {}
        }
        vec![Act::Repaint]
    }

    pub fn up(&mut self, _btn: Btn) -> Vec<Act> {
        let d = self.drag.take();
        self.view.pressed = None;
        match d {
            Some(Drag::Scrub) => self.shared.send(Play::Scrub(None)),
            Some(Drag::Knob { .. } | Drag::Fader(_) | Drag::CurvePoint(_)) => self.amp_changed(),
            _ => {}
        }
        vec![Act::Repaint]
    }

    pub fn leave(&mut self) -> Vec<Act> {
        self.view.mouse = None;
        vec![Act::Repaint]
    }

    pub fn wheel(&mut self, x: f32, y: f32, notches: f32, m: Mods) -> Vec<Act> {
        match self.at(x, y) {
            Some(Target::Amp(c @ (AmpCtl::Pan | AmpCtl::Sep | AmpCtl::Left | AmpCtl::Volume | AmpCtl::Right | AmpCtl::Pitch | AmpCtl::Time | AmpCtl::Formant | AmpCtl::Dry | AmpCtl::Wet | AmpCtl::Decay | AmpCtl::Mix | AmpCtl::Amount | AmpCtl::Offset | AmpCtl::Threshold | AmpCtl::Reduction | AmpCtl::ClipLevel))) => {
                let v = self.amp_value(c) + notches * 0.02;
                self.set_amp_value(c, v.clamp(-1.0, 1.0));
                self.hint = self.amp_hint(c);
                self.amp_changed();
            }
            _ if self.view.amp.is_some() => {}
            Some(Target::Wave | Target::Flag(_) | Target::LoopFlag | Target::Scroll) => {
                if m.shift {
                    self.scroll_by(-notches * 60.0);
                } else {
                    let at = self.doc(|d| self.frame_at(d, x));
                    let at = if self.at(x, y) == Some(Target::Scroll) { None } else { Some(at) };
                    self.zoom(0.8f64.powf(notches as f64), at);
                }
            }
            _ => return vec![],
        }
        vec![Act::Repaint]
    }

    fn hover_hint(&self, x: f32, y: f32) -> String {
        let Some(t) = self.at(x, y) else { return String::new() };
        if self.view.amp.is_some() {
            return match t {
                Target::Amp(c) => self.amp_hint(c),
                _ => String::new(),
            };
        }
        if t == Target::Wave {
            return self.doc(|d| {
                let n = d.frames();
                if n == 0 {
                    return "Click to load a sample".to_string();
                }
                let f = self.frame_at(d, x);
                if f < 0.0 || f >= n as f64 {
                    return String::new();
                }
                if let Some(k) = d.env_shown() {
                    // The envelope's reading at the pointer (at the point under it, if any).
                    let p = d.env.points(k, n);
                    let v = self.env_point_at(d, k, x, y).map_or_else(|| env::value(&p, f), |i| p[i].v);
                    return format!("{} envelope: {} \u{00b7} click the line to add a point, right-click one to delete it", env::NAMES[k], env::text(k, v));
                }
                let i = f as usize;
                let v = (0..d.sample.channels()).map(|c| d.sample.ch[c][i]).fold(0f32, |m, v| if v.abs() > m.abs() { v } else { m });
                let fmt = if d.view.time == state::TIME_AUTO { state::TIME_MSM } else { d.view.time };
                format!("{} | {} {:.2}", view::time_text(fmt, f, d.sample.rate, d.sample.tempo, 1.0), view::db_text(v), v)
            });
        }
        let rec = self.shared.rec();
        view::hint(&t, &view::HintCtx { when: rec.when, max: rec.max })
    }

    // ------------------------------------------------------------------------------------------- tools

    /// A tool button: its left (`right` false) or right action.
    fn tool(&mut self, i: usize, right: bool, x: f32, y: f32) -> Vec<Act> {
        let def = &view::TOOLS[i];
        if !(if right { def.right } else { def.left }) {
            self.hint = format!("{} is not in this version", def.hint.split(" (").next().unwrap_or(def.hint));
            return vec![Act::Repaint];
        }
        let cmd = match (i, right) {
            (0, false) => Cmd::Undo,
            (0, true) => return vec![Act::Menu(MenuReq::Undo, x, y)],
            (1, false) => Cmd::Tool(ToolCmd::Normalize),
            (1, true) => Cmd::Tool(ToolCmd::NormalizeRegions),
            (2, false) => Cmd::Tool(ToolCmd::FadeIn),
            (2, true) => Cmd::Tool(ToolCmd::DeclickIn),
            (3, false) => Cmd::Tool(ToolCmd::Amp),
            (3, true) => Cmd::Tool(ToolCmd::Center),
            (4, false) => Cmd::Tool(ToolCmd::Reverb),
            (5, false) => Cmd::Tool(ToolCmd::Equalize),
            (13, false) => Cmd::Tool(ToolCmd::Blur),
            (14, false) => Cmd::Tool(ToolCmd::CleanUp),
            (14, true) => Cmd::Tool(ToolCmd::AcquireNoise),
            (6, false) => {
                let region = self.doc(|d| d.sel.is_some());
                if region { Cmd::AddRegion } else { Cmd::AddMarker }
            }
            (6, true) => Cmd::DeleteCueHere,
            (7, false) => Cmd::SetLoop,
            (7, true) => Cmd::ClearLoop,
            (8, false) => return self.drag_out(),
            (8, true) => Cmd::Copy,
            (9, false) => Cmd::Tool(ToolCmd::Reverse),
            (9, true) => Cmd::Tool(ToolCmd::Swap),
            (11, false) => Cmd::Tool(ToolCmd::FadeOut),
            (11, true) => Cmd::Tool(ToolCmd::DeclickOut),
            (12, false) => Cmd::Tool(ToolCmd::Stretch),
            (16, false) => Cmd::Save(Depth::Own),
            (16, true) => Cmd::Save(Depth::Int24),
            (17, b) => Cmd::Send(b),
            _ => Cmd::None,
        };
        self.pick(&MenuReq::Main(MenuId::Tools), cmd, x, y)
    }

    fn run_tool(&mut self, t: ToolCmd) -> Vec<Act> {
        let ok = self.doc(|d| match t {
            ToolCmd::Amp | ToolCmd::Stretch | ToolCmd::Reverb | ToolCmd::Equalize | ToolCmd::Blur | ToolCmd::CleanUp | ToolCmd::AcquireNoise => false,
            ToolCmd::ReversePolarity => d.tool("Reverse polarity", tools::reverse_polarity),
            ToolCmd::Normalize => {
                let mut changed = false;
                let r = d.range();
                if let Some((a, b)) = r {
                    if d.sample.peak(a, b) > 0.0 {
                        changed = d.tool("Normalize", |s, a, b| {
                            tools::normalize(s, a, b);
                        });
                    }
                }
                changed
            }
            ToolCmd::FadeIn => d.tool("Fade in", tools::fade_in),
            ToolCmd::FadeOut => d.tool("Fade out", tools::fade_out),
            ToolCmd::DeclickIn => d.tool("Declick in", tools::declick_in),
            ToolCmd::DeclickOut => d.tool("Declick out", tools::declick_out),
            ToolCmd::Center => d.tool("Center", tools::center),
            ToolCmd::Reverse => d.tool("Reverse", tools::reverse),
            ToolCmd::Swap => d.sample.channels() == 2 && d.tool("Swap channels", tools::swap_channels),
            ToolCmd::LeftToMono => d.sample.channels() == 2 && d.whole("Convert left channel to mono", |s| {
                tools::channel_to_mono(s, 0);
                true
            }),
            ToolCmd::RightToMono => d.sample.channels() == 2 && d.whole("Convert right channel to mono", |s| {
                tools::channel_to_mono(s, 1);
                true
            }),
            ToolCmd::MonoSignal => d.whole("Convert mono signal to mono format", tools::mono_signal_to_mono),
            ToolCmd::Noise => {
                let seed = d.version.wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ 0x7e51a;
                d.tool("Generate noise", |s, a, b| tools::generate_noise(s, a, b, seed))
            }
            ToolCmd::NormalizeRegions => d.tool_regions("Normalize all regions", |s, a, b| {
                tools::normalize(s, a, b);
            }),
            ToolCmd::DeclickInRegions => d.tool_regions("Declick in all regions", tools::declick_in),
            ToolCmd::DeclickOutRegions => d.tool_regions("Declick out all regions", tools::declick_out),
            ToolCmd::Mixdown => d.mixdown(),
            ToolCmd::CancelEnvelopes => d.env_shown().is_some() && d.cancel_envelopes(),
            ToolCmd::PointsAtSelection => d.env_points_at(false),
            ToolCmd::PointsAtRegions => d.env_points_at(true),
            ToolCmd::FlipEnvelope => d.env_tool("Flip envelope vertically", |e, k, n| {
                e.flip(k, n);
                true
            }),
            ToolCmd::NormalizeEnvelope => d.env_tool("Normalize envelope levels", |e, k, n| e.normalize(k, n)),
        });
        if t == ToolCmd::AcquireNoise {
            self.acquire_noise();
        } else if matches!(t, ToolCmd::Amp | ToolCmd::Stretch | ToolCmd::Reverb | ToolCmd::Equalize | ToolCmd::Blur | ToolCmd::CleanUp) {
            if self.doc(|d| d.frames()) > 0 {
                let stretch = (t == ToolCmd::Stretch).then(Stretch::default);
                let reverb = (t == ToolCmd::Reverb).then(Reverb::default);
                let eq = (t == ToolCmd::Equalize).then(Equalize::default);
                let blur = (t == ToolCmd::Blur).then(Blur::default);
                let clean = (t == ToolCmd::CleanUp).then(Clean::default);
                let curve = if blur.is_some() { Curve::bell() } else { Curve::flat() };
                self.view.amp = Some(AmpUi { stretch, reverb, eq, blur, clean, curve, ..AmpUi::default() });
                self.backdrop();
                self.hint = match t {
                    ToolCmd::Stretch => "Time stretch / pitch shift of the selection",
                    ToolCmd::Reverb => "Convolution reverb of the selection",
                    ToolCmd::Equalize => "Equalize the selection: draw the curve over its spectrum",
                    ToolCmd::Blur => "Blur the selection: each frequency smeared over time by the impulse",
                    ToolCmd::CleanUp if self.noise.is_none() => "Clean up: select noise only and acquire a noise profile first (Alt+U)",
                    ToolCmd::CleanUp => "Clean up (denoise) the selection against the noise profile",
                    _ => "Amp: pan, volume and stereo separation of the selection",
                }
                .into();
            }
        } else if ok {
            self.hint = self.doc(|d| d.history().first().cloned().unwrap_or_default());
        }
        vec![Act::Repaint]
    }

    /// Acquire noise profile: the range's mean spectrum, per channel, for Clean up.
    fn acquire_noise(&mut self) {
        let got = self.doc(|d| {
            let (a, b) = d.range()?;
            let ch: Vec<&[f32]> = d.sample.ch.iter().map(|c| &c[a..=b]).collect();
            Some((spectral::acquire(&ch, d.sample.rate), view::time_text(state::TIME_MSM, (b + 1 - a) as f64, d.sample.rate, 0.0, 1.0)))
        });
        let Some((p, len)) = got else { return };
        self.noise = Some(p);
        self.view.profile = true;
        self.hint = format!("Noise profile acquired ({len})");
        self.backdrop();
    }

    /// What the open spectral panel draws behind its curve: Equalize, the spectrum of (the first 10 s of) the range;
    /// Clean up, the noise profile.
    fn backdrop(&mut self) {
        const COLS: usize = 198;
        let Some(ui) = self.view.amp else { return };
        let mean = |p: &Profile| {
            let k = p.mag.len().max(1) as f32;
            let m: Vec<f32> = (0..p.mag.first().map_or(0, |c| c.len())).map(|i| p.mag.iter().map(|c| c[i]).sum::<f32>() / k).collect();
            spectral::columns(&m, p.n, p.rate, COLS)
        };
        self.view.backdrop = if ui.eq.is_some() {
            self.doc(|d| {
                let Some((a, b)) = d.range() else { return Vec::new() };
                let b = b.min(a + 10 * d.sample.rate as usize);
                let ch: Vec<&[f32]> = d.sample.ch.iter().map(|c| &c[a..=b]).collect();
                mean(&spectral::acquire(&ch, d.sample.rate))
            })
        } else if ui.clean.is_some() {
            self.noise.as_ref().map(mean).unwrap_or_default()
        } else {
            Vec::new()
        };
    }

    /// The open panel's curve point under (x, y).
    fn curve_point_at(&self, x: f32, y: f32) -> Option<usize> {
        let ui = self.view.amp.as_ref()?;
        if ui.clean.is_some() {
            return None;
        }
        let (_, bx, _) = view::spectral_parts(&self.layout(), ui);
        ui.curve
            .points()
            .iter()
            .enumerate()
            .map(|(i, q)| (i, (bx.x + q.pos as f32 * bx.w - x).hypot(bx.bottom() - q.v * bx.h - y)))
            .filter(|&(_, dist)| dist <= view::GRAB)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i)
    }

    /// A press in the curve box: on a point drags it (a double-click puts it back at mid height), elsewhere adds a
    /// point there and drags it.
    fn curve_down(&mut self, x: f32, y: f32, dbl: bool) -> Vec<Act> {
        let lay = self.layout();
        let hit = self.curve_point_at(x, y);
        let Some(ui) = &mut self.view.amp else { return vec![] };
        if ui.clean.is_some() {
            return vec![Act::Repaint];
        }
        let (_, bx, _) = view::spectral_parts(&lay, ui);
        let (px, v) = (((x - bx.x) / bx.w).clamp(0.0, 1.0) as f64, ((bx.bottom() - y) / bx.h).clamp(0.0, 1.0));
        match hit {
            Some(i) if dbl => {
                let q = ui.curve.points()[i];
                ui.curve.move_point(i, q.pos, 0.5);
                self.amp_changed();
            }
            Some(i) => self.drag = Some(Drag::CurvePoint(i)),
            None => match ui.curve.insert(px, v) {
                Some(i) => self.drag = Some(Drag::CurvePoint(i)),
                None => self.hint = format!("A curve has {} points at most", Curve::MAX),
            },
        }
        self.hint = self.curve_hint();
        vec![Act::Repaint]
    }

    /// The curve's reading under the pointer (at the point under it, if any).
    fn curve_hint(&self) -> String {
        let Some(ui) = self.view.amp else { return String::new() };
        if ui.clean.is_some() {
            return if self.noise.is_some() { "The noise profile, and the threshold over it".into() } else { "No noise profile yet".into() };
        }
        let (_, bx, _) = view::spectral_parts(&self.layout(), &ui);
        let (mx, my) = self.view.mouse.unwrap_or((bx.cx(), bx.cy()));
        let smooth = ui.eq.is_none_or(|e| e.smooth);
        let (x, v) = match self.curve_point_at(mx, my).or(match self.drag {
            Some(Drag::CurvePoint(i)) => Some(i),
            _ => None,
        }) {
            Some(i) => (ui.curve.points()[i].pos, ui.curve.points()[i].v),
            None => {
                let x = ((mx - bx.x) / bx.w).clamp(0.0, 1.0) as f64;
                (x, spectral::curve_at(ui.curve.points(), x, smooth))
            }
        };
        if ui.eq.is_some() {
            let hz = spectral::eq_hz(x);
            let f = if hz >= 1000.0 { format!("{:.2} kHz", hz / 1000.0) } else { format!("{hz:.0} Hz") };
            format!("{f}: {:+.1} dB \u{00b7} click to add a point, right-click one to delete it", spectral::eq_db(v))
        } else {
            format!("Impulse: {:.0}% \u{00b7} click to add a point, right-click one to delete it", v * 100.0)
        }
    }

    fn drag_out(&mut self) -> Vec<Act> {
        let Some((s, title)) = self.doc(|d| d.copy().map(|s| (s, d.title()))) else { return vec![] };
        let p = out_path(&title);
        if std::fs::write(&p, wav::write(&s, Depth::Own)).is_err() {
            self.hint = "Could not write the sample".into();
            return vec![Act::Repaint];
        }
        vec![Act::DragOut(p), Act::Repaint]
    }

    // ------------------------------------------------------------------------------------------- the Amp panel

    /// A spectral panel knob's setting in the knobs' −1 to 1 (the 0 to 1 settings stretched over it).
    fn spectral_value(ui: &AmpUi, c: AmpCtl) -> Option<f32> {
        let (eq, bl, cl) = (ui.eq.unwrap_or_default(), ui.blur.unwrap_or_default(), ui.clean.unwrap_or_default());
        let u = |v: f32| v * 2.0 - 1.0;
        Some(match c {
            AmpCtl::Mix if ui.eq.is_some() => u(eq.mix),
            AmpCtl::Mix => u(bl.mix),
            AmpCtl::Amount => u(bl.amount),
            AmpCtl::Offset => bl.offset,
            AmpCtl::Threshold => u(cl.denoise.threshold),
            AmpCtl::Reduction => u(cl.denoise.amount),
            AmpCtl::ClipLevel => u(cl.level),
            _ => return None,
        })
    }

    fn amp_value(&self, c: AmpCtl) -> f32 {
        if let Some(v) = self.view.amp.as_ref().and_then(|u| Self::spectral_value(u, c)) {
            return v;
        }
        let a = self.view.amp.map(|a| a.amp).unwrap_or_default();
        let st = self.view.amp.and_then(|a| a.stretch).unwrap_or_default();
        let rv = self.view.amp.and_then(|a| a.reverb).unwrap_or_default();
        match c {
            AmpCtl::Dry => rv.dry,
            AmpCtl::Wet => rv.wet,
            AmpCtl::Decay => rv.decay,
            AmpCtl::Pitch => st.pitch,
            AmpCtl::Time => st.time,
            AmpCtl::Formant => st.formant,
            AmpCtl::Pan => a.pan,
            AmpCtl::Sep => a.sep,
            AmpCtl::Left => a.left,
            AmpCtl::Volume => a.volume,
            AmpCtl::Right => a.right,
            _ => 0.0,
        }
    }

    fn set_amp_value(&mut self, c: AmpCtl, v: f32) {
        if let Some(ui) = &mut self.view.amp {
            let h = (v + 1.0) / 2.0;
            if let Some(e) = &mut ui.eq {
                if c == AmpCtl::Mix {
                    e.mix = h;
                }
            }
            if let Some(b) = &mut ui.blur {
                match c {
                    AmpCtl::Mix => b.mix = h,
                    AmpCtl::Amount => b.amount = h,
                    AmpCtl::Offset => b.offset = v,
                    _ => {}
                }
            }
            if let Some(cl) = &mut ui.clean {
                match c {
                    AmpCtl::Threshold => cl.denoise.threshold = h,
                    AmpCtl::Reduction => cl.denoise.amount = h,
                    AmpCtl::ClipLevel => cl.level = h,
                    _ => {}
                }
            }
            if let Some(r) = &mut ui.reverb {
                match c {
                    AmpCtl::Dry => r.dry = v,
                    AmpCtl::Wet => r.wet = v,
                    AmpCtl::Decay => r.decay = v,
                    _ => {}
                }
            }
            if let Some(st) = &mut ui.stretch {
                match c {
                    AmpCtl::Pitch => st.pitch = v,
                    AmpCtl::Time => st.time = v,
                    AmpCtl::Formant => st.formant = v,
                    _ => {}
                }
            }
            let a = &mut ui.amp;
            match c {
                AmpCtl::Pan => a.pan = v,
                AmpCtl::Sep => a.sep = v,
                AmpCtl::Left => a.left = v,
                AmpCtl::Volume => a.volume = v,
                AmpCtl::Right => a.right = v,
                _ => {}
            }
        }
    }

    fn amp_hint(&self, c: AmpCtl) -> String {
        let v = self.amp_value(c);
        match c {
            AmpCtl::Pan => {
                let p = (v * 100.0).round() as i32;
                if p == 0 { "Panning: centered".into() } else { format!("Panning: {}% {}", p.abs(), if p < 0 { "left" } else { "right" }) }
            }
            AmpCtl::Sep => {
                let p = (v * 100.0).round() as i32;
                if p == 0 { "Stereo separation: none".into() } else { format!("Stereo separation: {}% {}", p.abs(), if p < 0 { "merged" } else { "separated" }) }
            }
            AmpCtl::Left | AmpCtl::Volume | AmpCtl::Right => {
                let g = tools::fader_gain(v);
                let name = match c {
                    AmpCtl::Left => "Left channel volume",
                    AmpCtl::Right => "Right channel volume",
                    _ => "Volume",
                };
                format!("{name}: {:.0}%{}", g.abs() * 100.0, if g < 0.0 { " (inverted)" } else { "" })
            }
            AmpCtl::Pitch | AmpCtl::Time | AmpCtl::Formant => {
                let st = self.view.amp.and_then(|a| a.stretch).unwrap_or_default();
                match c {
                    AmpCtl::Pitch => format!("Pitch: {:+.2} semitones", st.semitones()),
                    AmpCtl::Time => format!("Length: x{:.3}", st.ratio()),
                    _ if st.preserve => format!("Formant: {:+.2} semitones", st.formant_semitones()),
                    _ => "Formant: moves with the pitch (formant preservation off)".into(),
                }
            }
            AmpCtl::Mix => format!("Mix: {:.0}% processed", (v + 1.0) * 50.0),
            AmpCtl::Amount => format!("Amount: the blur spreads over {:.0} ms", self.view.amp.and_then(|a| a.blur).unwrap_or_default().seconds() * 1000.0),
            AmpCtl::Offset => {
                let p = (v * 100.0).round() as i32;
                if p == 0 { "Offset: centred on the sound".into() } else { format!("Offset: {}% {}", p.abs(), if p < 0 { "before the sound" } else { "after the sound" }) }
            }
            AmpCtl::Threshold => format!("Threshold: +{:.1} dB over the noise profile", self.view.amp.and_then(|a| a.clean).unwrap_or_default().denoise.threshold_db()),
            AmpCtl::Reduction => format!("Amount: noise down by {:.0} dB", self.view.amp.and_then(|a| a.clean).unwrap_or_default().denoise.reduction_db()),
            AmpCtl::ClipLevel => format!("Declipper: samples over {:.0}% of the peak count as clipped", self.view.amp.and_then(|a| a.clean).unwrap_or_default().clip_ratio() * 100.0),
            AmpCtl::Curve => self.curve_hint(),
            AmpCtl::Dry => format!("Dry: x{:.2}", Reverb::gain(v)),
            AmpCtl::Wet => format!("Wet: x{:.2}", Reverb::gain(v)),
            AmpCtl::Decay => {
                let r = self.view.amp.and_then(|a| a.reverb).unwrap_or_default();
                format!("Decay of the default impulse: {:.2} s (to -60 dB){}", r.seconds(), if r.clip { ", unused with the clipboard's" } else { "" })
            }
            _ => view::hint(&Target::Amp(c), &view::HintCtx { when: 0, max: 0 }),
        }
    }

    /// Applies the open panel's tool to the range: Amp, or Time stretch / pitch shift.
    fn apply_panel(d: &mut Doc, ui: &AmpUi, clip: Option<&Sample>, noise: Option<&Profile>) -> bool {
        if let Some(e) = ui.eq {
            return d.tool("Equalize", |s, a, b| spectral::equalize_range(s, a, b, &ui.curve, &e));
        }
        if let Some(bl) = ui.blur {
            return d.tool("Blur", |s, a, b| spectral::blur_range(s, a, b, &ui.curve, &bl));
        }
        if let Some(c) = ui.clean {
            if !ui.ready(noise.is_some()) {
                return false;
            }
            return d.tool("Clean up", |s, a, b| spectral::clean_range(s, a, b, noise, &c));
        }
        if let Some(r) = ui.reverb {
            let impulse = match (r.clip, clip) {
                (true, Some(c)) if c.frames() > 0 => doc::conform(c, d.sample.rate, d.sample.channels()).ch,
                _ => tools::default_impulse(d.sample.rate, d.sample.channels(), r.seconds()),
            };
            return d.replace_range("Convolution reverb", |s, a, b| tools::reverb(s, a, b, &r, &impulse));
        }
        match ui.stretch {
            Some(st) => d.replace_range("Time stretch / pitch shift", |s, a, b| tools::stretch(s, a, b, &st)),
            None => d.tool("Amp", |s, a, b| tools::amp(s, a, b, &ui.amp)),
        }
    }

    /// The panel's settings changed: a running preview is redone with them.
    fn amp_changed(&mut self) {
        let Some(ui) = self.view.amp else { return };
        if ui.previewing {
            self.doc(|d| {
                d.undo();
                Self::apply_panel(d, &ui, self.clip.as_ref(), self.noise.as_ref());
            });
        }
    }

    fn close_amp(&mut self, accept: bool) {
        let Some(ui) = self.view.amp.take() else { return };
        match (ui.previewing, accept) {
            (true, false) => {
                self.doc(|d| d.undo());
            }
            (false, true) => {
                self.doc(|d| Self::apply_panel(d, &ui, self.clip.as_ref(), self.noise.as_ref()));
            }
            _ => {}
        }
        // The redo step a cancelled preview leaves is of no use.
        if ui.previewing && !accept {
            self.doc(|d| d.discard_redo());
        }
        if ui.previewing {
            self.shared.send(Play::Pause);
        }
    }

    fn amp_down(&mut self, c: AmpCtl, y: f32, dbl: bool) -> Vec<Act> {
        let ready = self.view.amp.is_some_and(|u| u.ready(self.noise.is_some()));
        match c {
            AmpCtl::Curve => {
                let (x, _) = self.view.mouse.unwrap_or((0.0, y));
                return self.curve_down(x, y, dbl);
            }
            AmpCtl::Smooth | AmpCtl::NoiseOnly | AmpCtl::Declip => {
                if let Some(ui) = &mut self.view.amp {
                    if let Some(e) = &mut ui.eq {
                        e.smooth = !e.smooth;
                    }
                    if let Some(cl) = &mut ui.clean {
                        if c == AmpCtl::NoiseOnly {
                            cl.denoise.noise_only = !cl.denoise.noise_only;
                        } else {
                            cl.declip = !cl.declip;
                        }
                    }
                }
                // The declipper alone can start a preview: a running one stops when nothing is left to do.
                if self.view.amp.is_some_and(|u| u.previewing && !u.ready(self.noise.is_some())) {
                    self.doc(|d| {
                        d.undo();
                        d.discard_redo();
                    });
                    self.shared.send(Play::Pause);
                    if let Some(u) = &mut self.view.amp {
                        u.previewing = false;
                    }
                } else {
                    self.amp_changed();
                }
            }
            AmpCtl::Acquire => {
                if self.view.amp.is_some_and(|u| u.previewing) {
                    self.hint = "Stop the preview before acquiring a noise profile".into();
                } else {
                    self.acquire_noise();
                }
            }
            AmpCtl::Mix | AmpCtl::Amount | AmpCtl::Offset | AmpCtl::Threshold | AmpCtl::Reduction | AmpCtl::ClipLevel => {
                if dbl {
                    self.reset_amp(c);
                } else {
                    self.drag = Some(Drag::Knob { ctl: c, y, v: self.amp_value(c) });
                }
            }
            AmpCtl::Preview | AmpCtl::Accept if !ready => {
                self.hint = "Clean up needs a noise profile: select noise only and press ACQUIRE (Alt+U), or turn on the declipper".into();
            }
            AmpCtl::Preserve => {
                if let Some(st) = self.view.amp.as_mut().and_then(|u| u.stretch.as_mut()) {
                    st.preserve = !st.preserve;
                }
                self.amp_changed();
            }
            AmpCtl::Tail | AmpCtl::ClipImpulse => {
                let has_clip = self.clip.as_ref().is_some_and(|c| c.frames() > 0);
                if let Some(r) = self.view.amp.as_mut().and_then(|u| u.reverb.as_mut()) {
                    if c == AmpCtl::Tail {
                        r.tail = !r.tail;
                    } else if r.clip || has_clip {
                        r.clip = !r.clip;
                    } else {
                        self.hint = "Copy an impulse response to Tesla's clipboard first (Ctrl+C)".into();
                        return vec![Act::Repaint];
                    }
                }
                self.amp_changed();
            }
            AmpCtl::Dry | AmpCtl::Wet | AmpCtl::Decay => {
                if dbl {
                    self.reset_amp(c);
                } else {
                    self.drag = Some(Drag::Knob { ctl: c, y, v: self.amp_value(c) });
                }
            }
            AmpCtl::Pan | AmpCtl::Sep | AmpCtl::Pitch | AmpCtl::Time | AmpCtl::Formant => {
                if dbl {
                    self.set_amp_value(c, 0.0);
                    self.amp_changed();
                } else {
                    self.drag = Some(Drag::Knob { ctl: c, y, v: self.amp_value(c) });
                }
            }
            AmpCtl::Left | AmpCtl::Volume | AmpCtl::Right => {
                self.drag = Some(Drag::Fader(c));
                let (x, _) = self.view.mouse.unwrap_or((0.0, y));
                return self.moved(x, y, Mods::default());
            }
            AmpCtl::Law => {
                if let Some(ui) = &mut self.view.amp {
                    ui.amp.law = if ui.amp.law == Law::Circular { Law::Triangular } else { Law::Circular };
                }
                self.amp_changed();
            }
            AmpCtl::Reset => {
                if let Some(ui) = &mut self.view.amp {
                    ui.amp = Amp::default();
                    ui.stretch = ui.stretch.map(|_| Stretch::default());
                    ui.reverb = ui.reverb.map(|_| Reverb::default());
                    ui.eq = ui.eq.map(|_| Equalize::default());
                    ui.blur = ui.blur.map(|_| Blur::default());
                    ui.clean = ui.clean.map(|_| Clean::default());
                    ui.curve = if ui.blur.is_some() { Curve::bell() } else { Curve::flat() };
                }
                self.amp_changed();
            }
            AmpCtl::Preview => {
                let Some(ui) = self.view.amp else { return vec![] };
                if ui.previewing {
                    self.doc(|d| d.undo());
                    self.shared.send(Play::Pause);
                } else {
                    let r = self.doc(|d| {
                        Self::apply_panel(d, &ui, self.clip.as_ref(), self.noise.as_ref());
                        d.range()
                    });
                    if let Some((a, b)) = r {
                        self.shared.send(Play::Play { from: a, to: b, at: a as f64 });
                    }
                }
                if ui.previewing {
                    self.doc(|d| d.discard_redo());
                }
                if let Some(u) = &mut self.view.amp {
                    u.previewing = !ui.previewing;
                }
            }
            AmpCtl::Accept => self.close_amp(true),
            AmpCtl::Close => self.close_amp(false),
        }
        vec![Act::Repaint]
    }

    // ------------------------------------------------------------------------------------------- the bottom bar

    fn bottom(&mut self, b: Bt, x: f32, y: f32) -> Vec<Act> {
        match b {
            Bt::Undo => return self.pick(&MenuReq::Undo, Cmd::Undo, x, y),
            Bt::Mode(i) => self.doc(|d| d.view.mode = i.min(4)),
            Bt::Threshold => self.hint = "Not in this version".into(),
            Bt::Follow => self.doc(|d| d.view.follow = !d.view.follow),
            Bt::Spectrum => self.doc(|d| d.view.spectrum = !d.view.spectrum),
            Bt::Regions => self.doc(|d| {
                d.view.regions = !d.view.regions;
                d.view.looped = d.view.regions;
            }),
            Bt::Freeze => self.doc(|d| d.view.freeze = !d.view.freeze),
            Bt::ClickFree => self.doc(|d| d.view.clickfree = !d.view.clickfree),
            Bt::Snap => self.doc(|d| d.view.snap_on = !d.view.snap_on),
            Bt::Slide => self.doc(|d| d.view.slide = !d.view.slide),
        }
        vec![Act::Repaint]
    }

    // ------------------------------------------------------------------------------------------- keyboard

    /// A key press; empty when the editor doesn't use it (the host gets it).
    pub fn key(&mut self, k: Key, m: Mods) -> Vec<Act> {
        if self.view.amp.is_some() {
            return match k {
                Key::Escape => {
                    self.close_amp(false);
                    vec![Act::Repaint]
                }
                _ => vec![],
            };
        }
        let (c, s, a) = (m.ctrl, m.shift, m.alt);
        let cmd = match k {
            Key::Space => {
                self.play_pause();
                return vec![Act::Repaint];
            }
            Key::Char('O') if c => Cmd::Open,
            Key::Char('S') if c => Cmd::Save(Depth::Own),
            Key::F2 if c || a => Cmd::AutoRename,
            Key::F2 => Cmd::Properties,
            Key::Char('Z') if c => Cmd::Undo,
            Key::Char('Y') if c => Cmd::Redo,
            Key::Char('X') if c => Cmd::Cut,
            Key::Char('C') if c => Cmd::Copy,
            Key::Char('V') if c && !s && !a => Cmd::Paste(Paste::Insert),
            Key::Char('V') if a && !c => Cmd::Paste(Paste::Replace),
            Key::Char('V') if s && !c => Cmd::Paste(Paste::Mix),
            Key::Delete if s && c => Cmd::DeleteCueHere,
            Key::Delete if s => Cmd::Clear,
            Key::Delete if c => Cmd::Trim,
            Key::Delete if a => Cmd::DeleteAfterLoop,
            Key::Delete => Cmd::Delete,
            Key::Insert => Cmd::InsertSilence,
            Key::Char('C') if s => Cmd::Send(false),
            Key::Char('C') if !a => Cmd::ClickFree,
            Key::Char('E') if c && !a => Cmd::Tool(ToolCmd::Mixdown),
            Key::Char('E') if a && !c => Cmd::Tool(ToolCmd::CancelEnvelopes),
            Key::Char('P') if !c && !a && !s => Cmd::Tool(ToolCmd::PointsAtSelection),
            Key::Char('R') if c && !a => Cmd::Tool(ToolCmd::Reverb),
            Key::Char('Q') if c && !a => Cmd::Tool(ToolCmd::Equalize),
            Key::Char('B') if c && !a => Cmd::Tool(ToolCmd::Blur),
            Key::Char('U') if c && !a => Cmd::Tool(ToolCmd::CleanUp),
            Key::Char('U') if a && !c => Cmd::Tool(ToolCmd::AcquireNoise),
            Key::Char('A') if a => Cmd::Tool(ToolCmd::Amp),
            Key::Char('T') if a => Cmd::Tool(ToolCmd::Stretch),
            Key::Char('A') if c => Cmd::SelectZoomed,
            Key::Char('N') if c => Cmd::Tool(ToolCmd::Normalize),
            Key::Char('F') if c => Cmd::Tool(ToolCmd::FadeIn),
            Key::Char('F') if a => Cmd::Tool(ToolCmd::FadeOut),
            Key::Left if a => Cmd::Tool(ToolCmd::Reverse),
            Key::Char('M') if a => Cmd::AddRegion,
            Key::Char('M') if !c => Cmd::AddMarker,
            Key::Char('L') if a => Cmd::SetLoop,
            Key::Char('S') if !a => Cmd::Spectrum,
            Key::Char(d @ '1'..='4') if !c && !a => Cmd::Snap(d as u8 - b'1'),
            Key::Up => Cmd::Deselect,
            Key::Left if c => Cmd::ZoomLeft,
            Key::Right if c => Cmd::ZoomRight,
            Key::Left => Cmd::SelectRegion(-1),
            Key::Right => Cmd::SelectRegion(1),
            Key::PageUp => Cmd::ZoomIn,
            Key::PageDown => Cmd::ZoomOut,
            Key::Home => Cmd::ZoomFull,
            Key::End => Cmd::ZoomSel,
            _ => return vec![],
        };
        let (x, y) = self.view.mouse.unwrap_or((40.0, 140.0));
        self.pick(&MenuReq::Main(MenuId::Edit), cmd, x, y)
    }

    // ------------------------------------------------------------------------------------------- menus

    pub fn menu(&self, req: &MenuReq) -> MenuDef<Cmd> {
        let (d, rec) = (self.shared.doc.lock().unwrap_or_else(|e| e.into_inner()), self.shared.rec());
        let mut m = MenuDef::new(MenuStyle::Context);
        let item = |l: &str| MenuItem::item(l);
        let check = |l: &str, on: bool| MenuItem::item(l).mark(Mark::Check(on));
        let radio = |l: &str, on: bool| MenuItem::item(l).mark(Mark::Radio(on));
        let has = d.frames() > 0;
        let sel = d.sel.is_some();
        match req {
            MenuReq::Main(MenuId::Options) => {
                for (i, (id, name)) in view::MENUS.iter().enumerate() {
                    if i == 2 || i == 5 {
                        m.sep(Cmd::None);
                    }
                    m.submenu(name, self.main_menu(*id, &d), Cmd::None);
                }
            }
            MenuReq::Main(id) => return self.main_menu(*id, &d),
            MenuReq::When => {
                m.head("When / what to record", Cmd::None);
                for (i, n) in state::WHEN_NAMES.iter().enumerate() {
                    m.add(radio(n, rec.when == i as u8), Cmd::When(i as u8));
                }
            }
            MenuReq::Max => {
                m.head("Maximum recording time", Cmd::None);
                for (i, n) in state::MAX_NAMES.iter().enumerate() {
                    if i == 7 {
                        m.sep(Cmd::None);
                    }
                    m.add(radio(n, rec.max == i as u8), Cmd::Max(i as u8));
                }
            }
            MenuReq::Undo => {
                m.head("Undo history", Cmd::None);
                let h = d.history();
                if h.is_empty() {
                    m.add(item("(nothing to undo)").enabled(false), Cmd::None);
                }
                for (i, n) in h.iter().enumerate().take(32) {
                    m.add(item(n), Cmd::UndoTo(i + 1));
                }
                m.sep(Cmd::None);
                m.add(item("Redo").shortcut("Ctrl+Y").enabled(d.can_redo()), Cmd::Redo);
            }
            MenuReq::Flag(id) => {
                let name = d.sample.cues.iter().find(|c| c.id == *id).map_or(String::new(), |c| c.name.clone());
                m.head(&name, Cmd::None);
                m.add(item("Rename..."), Cmd::RenameCue(*id));
                m.add(item("Delete"), Cmd::DeleteCue(*id));
            }
            MenuReq::LoopFlag => {
                m.head("Loop", Cmd::None);
                m.add(item("Remove loop"), Cmd::ClearLoop);
            }
            MenuReq::Slave => {
                m.head("Slave playback to host", Cmd::None);
                m.add(check("Ignore host selection", self.shared.ignore_host_sel.load(Ordering::Relaxed)), Cmd::IgnoreHostSel);
                m.add(check("Ignore own selection", self.shared.ignore_own_sel.load(Ordering::Relaxed)), Cmd::IgnoreOwnSel);
            }
            MenuReq::Wave => {
                m.add(item("Cut").shortcut("Ctrl+X").enabled(has), Cmd::Cut);
                m.add(item("Copy").shortcut("Ctrl+C").enabled(has), Cmd::Copy);
                m.add(item("Paste insert").shortcut("Ctrl+V"), Cmd::Paste(Paste::Insert));
                m.add(item("Paste replace").shortcut("Alt+V"), Cmd::Paste(Paste::Replace));
                m.add(item("Paste mix").shortcut("Shift+V"), Cmd::Paste(Paste::Mix));
                m.sep(Cmd::None);
                m.add(item("Delete").shortcut("Del").enabled(has), Cmd::Delete);
                m.add(item("Trim").shortcut("Ctrl+Del").enabled(sel), Cmd::Trim);
                m.sep(Cmd::None);
                m.add(item("Add region...").shortcut("Alt+M").enabled(has), Cmd::AddRegion);
                m.add(item("Add marker...").shortcut("M").enabled(has), Cmd::AddMarker);
                m.add(item("Set loop").shortcut("Alt+L").enabled(has), Cmd::SetLoop);
            }
        }
        m
    }

    fn main_menu(&self, id: MenuId, d: &Doc) -> MenuDef<Cmd> {
        let mut m = MenuDef::new(MenuStyle::Context);
        let item = |l: &str| MenuItem::item(l);
        let check = |l: &str, on: bool| MenuItem::item(l).mark(Mark::Check(on));
        let radio = |l: &str, on: bool| MenuItem::item(l).mark(Mark::Radio(on));
        let has = d.frames() > 0;
        let sel = d.sel.is_some();
        let stereo = d.sample.channels() == 2;
        let regions = d.sample.cues.iter().any(|c| c.is_region());
        let v = &d.view;
        match id {
            MenuId::File => {
                m.add(item("New"), Cmd::New);
                m.add(item("Load sample...").shortcut("Ctrl+O"), Cmd::Open);
                m.add(item("Save sample as...").shortcut("Ctrl+S").enabled(has), Cmd::Save(Depth::Own));
                m.add(item("Save sample as 24Bit .wav file...").enabled(has), Cmd::Save(Depth::Int24));
                m.sep(Cmd::None);
                m.add(item("About Tesla"), Cmd::About);
            }
            MenuId::Format => {
                m.add(item("Edit properties...").shortcut("F2"), Cmd::Properties);
                m.head("Precision", Cmd::None);
                m.add(radio("16Bit int", d.sample.format == Format::Int16), Cmd::Format(Format::Int16));
                m.add(radio("32Bit float", d.sample.format == Format::Float32), Cmd::Format(Format::Float32));
                m.head("Channels", Cmd::None);
                m.add(radio("Mono", !stereo), Cmd::Channels(1));
                m.add(radio("Stereo", stereo), Cmd::Channels(2));
            }
            MenuId::Edit => {
                let last = d.history().first().cloned();
                let undo = last.map_or("Undo".to_string(), |n| format!("Undo {}", n.to_lowercase()));
                m.add(item(&undo).shortcut("Ctrl+Z").enabled(d.can_undo()), Cmd::Undo);
                m.add(item("Redo").shortcut("Ctrl+Y").enabled(d.can_redo()), Cmd::Redo);
                m.submenu("Undo history", self.menu_undo(d), Cmd::None);
                m.add(check("Enable undo for large samples", v.undo_large), Cmd::UndoLarge);
                m.sep(Cmd::None);
                m.add(item("Cut").shortcut("Ctrl+X").enabled(has), Cmd::Cut);
                m.add(item("Copy").shortcut("Ctrl+C").enabled(has), Cmd::Copy);
                m.add(item("Paste insert").shortcut("Ctrl+V"), Cmd::Paste(Paste::Insert));
                m.add(item("Paste replace").shortcut("Alt+V"), Cmd::Paste(Paste::Replace));
                m.add(item("Paste mix").shortcut("Shift+V"), Cmd::Paste(Paste::Mix));
                m.sep(Cmd::None);
                m.add(item("Clear").shortcut("Shift+Del").enabled(has), Cmd::Clear);
                m.add(item("Delete").shortcut("Del").enabled(has), Cmd::Delete);
                m.add(item("Trim").shortcut("Ctrl+Del").enabled(sel), Cmd::Trim);
                m.add(item("Delete part after loop").shortcut("Alt+Del").enabled(d.sample.looped.is_some_and(|(_, b)| b + 1 < d.frames())), Cmd::DeleteAfterLoop);
                m.add(item("Insert silence").shortcut("Ins"), Cmd::InsertSilence);
                m.sep(Cmd::None);
                m.add(check("Click-free (smooth) editing", v.clickfree).shortcut("C"), Cmd::ClickFree);
            }
            MenuId::Tools => {
                let t = |m: &mut MenuDef<Cmd>, l: &str, k: &str, c: ToolCmd, on: bool| {
                    m.add(item(l).shortcut(k).enabled(on), Cmd::Tool(c));
                };
                // Edison greys the Envelope section in waveform mode; the mixdown is Tesla's to use whenever an amp
                // envelope is drawn.
                let envm = has && d.env_shown().is_some();
                m.head("Envelope", Cmd::None);
                t(&mut m, "Cancel all envelopes", "Alt+E", ToolCmd::CancelEnvelopes, envm && d.env != env::Envs::default());
                t(&mut m, "Add points at selection", "P", ToolCmd::PointsAtSelection, envm && sel);
                t(&mut m, "Add points at regions", "", ToolCmd::PointsAtRegions, envm && regions);
                t(&mut m, "Flip vertically", "", ToolCmd::FlipEnvelope, envm);
                t(&mut m, "Normalize levels", "", ToolCmd::NormalizeEnvelope, envm && d.env_shown().is_some_and(|k| !d.env.is_flat(k)));
                m.head("Amp", Cmd::None);
                t(&mut m, "Mixdown amp envelopes", "Ctrl+E", ToolCmd::Mixdown, has && !d.env.amp_flat());
                t(&mut m, "Amp...", "Alt+A", ToolCmd::Amp, has);
                t(&mut m, "Reverse polarity", "", ToolCmd::ReversePolarity, has);
                t(&mut m, "Normalize", "Ctrl+N", ToolCmd::Normalize, has);
                t(&mut m, "Fade in", "Ctrl+F", ToolCmd::FadeIn, has);
                t(&mut m, "Fade out", "Alt+F", ToolCmd::FadeOut, has);
                t(&mut m, "Declick in", "", ToolCmd::DeclickIn, has);
                t(&mut m, "Declick out", "", ToolCmd::DeclickOut, has);
                m.head("Waveform", Cmd::None);
                t(&mut m, "Center", "", ToolCmd::Center, has);
                m.head("Time", Cmd::None);
                t(&mut m, "Reverse", "Alt+Left", ToolCmd::Reverse, has);
                t(&mut m, "Time stretch / pitch shift...", "Alt+T", ToolCmd::Stretch, has);
                m.head("Channels", Cmd::None);
                t(&mut m, "Swap channels", "", ToolCmd::Swap, has && stereo);
                t(&mut m, "Convert left channel to mono", "", ToolCmd::LeftToMono, has && stereo);
                t(&mut m, "Convert right channel to mono", "", ToolCmd::RightToMono, has && stereo);
                t(&mut m, "Convert mono signal to mono format", "", ToolCmd::MonoSignal, has && stereo);
                m.add(MenuItem::Column, Cmd::None);
                m.head("Synthesis", Cmd::None);
                t(&mut m, "Generate noise", "", ToolCmd::Noise, has);
                m.head("Spectral", Cmd::None);
                t(&mut m, "Convolution reverb...", "Ctrl+R", ToolCmd::Reverb, has);
                t(&mut m, "Blur...", "Ctrl+B", ToolCmd::Blur, has);
                t(&mut m, "Equalize...", "Ctrl+Q", ToolCmd::Equalize, has);
                t(&mut m, "Acquire noise profile", "Alt+U", ToolCmd::AcquireNoise, has);
                t(&mut m, "Clean up (denoise)...", "Ctrl+U", ToolCmd::CleanUp, has);
                m.head("Regions", Cmd::None);
                t(&mut m, "Normalize all regions", "", ToolCmd::NormalizeRegions, regions);
                t(&mut m, "Declick in all regions", "", ToolCmd::DeclickInRegions, regions);
                t(&mut m, "Declick out all regions", "", ToolCmd::DeclickOutRegions, regions);
                m.head("Sequencing", Cmd::None);
                m.add(item("Send to arranger as audio clip").shortcut("Shift+C").enabled(has), Cmd::Send(false));
                m.add(item("Send to selected channel").enabled(has), Cmd::Send(true));
            }
            MenuId::Regions => {
                m.add(item("Add region...").shortcut("Alt+M").enabled(has), Cmd::AddRegion);
                m.add(item("Add marker...").shortcut("M").enabled(has), Cmd::AddMarker);
                m.add(item("Set loop").shortcut("Alt+L").enabled(has), Cmd::SetLoop);
                m.add(item("Remove loop").enabled(d.sample.looped.is_some()), Cmd::ClearLoop);
                m.add(item("Delete").shortcut("Shift+Ctrl+Del").enabled(d.cue_at(d.insert_at()).is_some()), Cmd::DeleteCueHere);
                m.add(item("Auto rename all").shortcut("Ctrl+F2").enabled(!d.sample.cues.is_empty()), Cmd::AutoRename);
                m.sep(Cmd::None);
                m.add(check("Freeze all", v.freeze), Cmd::Freeze);
            }
            MenuId::View => {
                m.head("Audio", Cmd::None);
                m.add(check("Spectrum", v.spectrum).shortcut("S"), Cmd::Spectrum);
                let mut ds = MenuDef::new(MenuStyle::Context);
                ds.head("Spectrum precision", Cmd::None);
                for (i, b) in state::BANDS.iter().enumerate() {
                    ds.add(radio(&format!("{b} bands"), v.bands == i as u8), Cmd::Bands(i as u8));
                }
                ds.add(MenuItem::Column, Cmd::None);
                ds.head("Spectrum gradient", Cmd::None);
                for (i, (n, _)) in crate::spectrum::GRADIENTS.iter().enumerate() {
                    ds.add(radio(n, v.gradient == i as u8), Cmd::Gradient(i as u8));
                }
                m.submenu("Display settings", ds, Cmd::None);
                m.head("Regions", Cmd::None);
                m.add(check("Regions", v.regions), Cmd::Regions);
                m.add(check("Loop", v.looped), Cmd::LoopView);
                m.head("Time", Cmd::None);
                let mut tf = MenuDef::new(MenuStyle::Context);
                for (i, n) in ["Samples", "Minute : Second : Millisecond", "Bar : Beat : Tick", "Auto"].iter().enumerate() {
                    tf.add(radio(n, v.time == i as u8), Cmd::Time(i as u8));
                }
                m.submenu("Time format", tf, Cmd::None);
            }
            MenuId::Snap => {
                for (i, n) in ["Snap to grid", "Snap to regions", "Snap to samples", "Snap to zero-crossing"].iter().enumerate() {
                    m.add(radio(n, v.snap == i as u8).shortcut(&format!("{}", i + 1)), Cmd::Snap(i as u8));
                }
            }
            MenuId::Select => {
                m.add(item("Deselect").shortcut("Up").enabled(sel), Cmd::Deselect);
                m.add(item("Select before current selection").enabled(d.sel.is_some_and(|(a, _)| a > 0)), Cmd::SelectBefore);
                m.add(item("Select after current selection").enabled(d.sel.is_some_and(|(_, b)| b + 1 < d.frames())), Cmd::SelectAfter);
                m.add(item("Select zoomed part").shortcut("Ctrl+A").enabled(has), Cmd::SelectZoomed);
                m.add(item("Select all").enabled(has), Cmd::SelectAll);
                m.add(item("Select previous region").shortcut("Left").enabled(regions), Cmd::SelectRegion(-1));
                m.add(item("Select next region").shortcut("Right").enabled(regions), Cmd::SelectRegion(1));
            }
            MenuId::Zoom => {
                m.add(item("Zoom in").shortcut("PgUp").enabled(has), Cmd::ZoomIn);
                m.add(item("Zoom out").shortcut("PgDn").enabled(has), Cmd::ZoomOut);
                m.add(item("Zoom out full").shortcut("Home").enabled(has), Cmd::ZoomFull);
                m.add(item("Zoom on selection").shortcut("End").enabled(sel), Cmd::ZoomSel);
                m.add(item("Zoom on left of selection").shortcut("Ctrl+Left").enabled(sel), Cmd::ZoomLeft);
                m.add(item("Zoom on right of selection").shortcut("Ctrl+Right").enabled(sel), Cmd::ZoomRight);
            }
            MenuId::Options => {}
        }
        m
    }

    fn menu_undo(&self, d: &Doc) -> MenuDef<Cmd> {
        let mut m = MenuDef::new(MenuStyle::Context);
        let h = d.history();
        if h.is_empty() {
            m.add(MenuItem::item("(nothing to undo)").enabled(false), Cmd::None);
        }
        for (i, n) in h.iter().enumerate().take(32) {
            m.add(MenuItem::item(n), Cmd::UndoTo(i + 1));
        }
        m
    }

    pub fn pick(&mut self, _req: &MenuReq, cmd: Cmd, x: f32, y: f32) -> Vec<Act> {
        let w = self.layout().lane().w;
        let mut acts = vec![Act::Repaint];
        match cmd {
            Cmd::None => {}
            Cmd::New => {
                self.doc(|d| d.new_sample());
            }
            Cmd::Open => acts.push(Act::Open),
            Cmd::Save(depth) => {
                if self.doc(|d| d.frames()) > 0 {
                    acts.push(Act::Save(depth));
                }
            }
            Cmd::About => self.hint = about(),
            Cmd::Properties => acts.push(Act::TypeIn(Prompt::Tempo, x, y)),
            Cmd::Format(f) => {
                self.doc(|d| d.set_format(f));
            }
            Cmd::Channels(n) => {
                self.doc(|d| d.set_channels(n));
            }
            Cmd::Undo => {
                let n = self.doc(|d| d.undo());
                self.hint = n.map_or("Nothing to undo".into(), |n| format!("Undone: {n}"));
            }
            Cmd::Redo => {
                let n = self.doc(|d| d.redo());
                self.hint = n.map_or("Nothing to redo".into(), |n| format!("Redone: {n}"));
            }
            Cmd::UndoTo(n) => self.doc(|d| d.undo_to(n)),
            Cmd::UndoLarge => self.doc(|d| d.view.undo_large = !d.view.undo_large),
            Cmd::Cut | Cmd::Copy => {
                let c = self.doc(|d| if cmd == Cmd::Cut { d.cut() } else { d.copy() });
                if let Some(c) = c {
                    acts.push(Act::Clipboard(wav::write(&c, Depth::Own)));
                    self.clip = Some(c);
                    self.hint = if cmd == Cmd::Cut { "Cut to the clipboard".into() } else { "Copied to the clipboard".into() };
                }
            }
            Cmd::Paste(mode) => acts.push(Act::Paste(mode)),
            Cmd::Clear => {
                self.doc(|d| d.clear());
            }
            Cmd::Delete => {
                self.doc(|d| d.delete());
            }
            Cmd::Trim => {
                self.doc(|d| d.trim());
            }
            Cmd::DeleteAfterLoop => {
                self.doc(|d| d.delete_after_loop());
            }
            Cmd::InsertSilence => {
                self.doc(|d| {
                    // Tesla's rule: the selection's length, else one second.
                    let n = d.sel.map_or(d.sample.rate as usize, |(a, b)| b + 1 - a);
                    d.insert_silence(n)
                });
            }
            Cmd::ClickFree => self.doc(|d| d.view.clickfree = !d.view.clickfree),
            Cmd::Tool(t) => acts.extend(self.run_tool(t)),
            Cmd::Send(channel) => {
                if let Some((s, title)) = self.doc(|d| d.copy().map(|s| (s, d.title()))) {
                    let p = out_path(&title);
                    if std::fs::write(&p, wav::write(&s, Depth::Own)).is_ok() {
                        acts.push(Act::Send(p, channel));
                    }
                }
            }
            Cmd::AddRegion => {
                if self.doc(|d| d.frames()) > 0 {
                    acts.push(Act::TypeIn(Prompt::Region, x, y));
                }
            }
            Cmd::AddMarker => {
                if self.doc(|d| d.frames()) > 0 {
                    acts.push(Act::TypeIn(Prompt::Marker, x, y));
                }
            }
            Cmd::SetLoop => {
                self.doc(|d| d.set_loop());
            }
            Cmd::ClearLoop => {
                self.doc(|d| d.clear_loop());
            }
            Cmd::DeleteCue(id) => {
                self.doc(|d| d.delete_cue(id));
            }
            Cmd::DeleteCueHere => {
                self.doc(|d| {
                    let id = d.cue_at(d.insert_at());
                    id.is_some_and(|id| d.delete_cue(id))
                });
            }
            Cmd::RenameCue(id) => acts.push(Act::TypeIn(Prompt::Rename(id), x, y)),
            Cmd::AutoRename => {
                self.doc(|d| d.auto_rename());
            }
            Cmd::Freeze => self.doc(|d| d.view.freeze = !d.view.freeze),
            Cmd::Spectrum => self.doc(|d| d.view.spectrum = !d.view.spectrum),
            Cmd::Regions => self.doc(|d| d.view.regions = !d.view.regions),
            Cmd::LoopView => self.doc(|d| d.view.looped = !d.view.looped),
            Cmd::Time(t) => self.doc(|d| d.view.time = t),
            Cmd::Bands(b) => self.doc(|d| d.view.bands = b),
            Cmd::Gradient(g) => self.doc(|d| d.view.gradient = g),
            Cmd::Snap(s) => self.doc(|d| {
                d.view.snap = s;
                d.view.snap_on = true;
            }),
            Cmd::Deselect => self.doc(|d| d.sel = None),
            Cmd::SelectBefore => {
                self.doc(|d| d.select_before());
            }
            Cmd::SelectAfter => {
                self.doc(|d| d.select_after());
            }
            Cmd::SelectZoomed => self.doc(|d| {
                let (s, p) = view::span(d, d.frames(), w);
                let n = d.frames();
                if n > 0 {
                    d.select(s as usize, ((s + p * w as f64) as usize).min(n).saturating_sub(1));
                }
            }),
            Cmd::SelectAll => self.doc(|d| {
                if d.frames() > 0 {
                    d.select(0, d.frames() - 1);
                }
            }),
            Cmd::SelectRegion(dir) => {
                self.doc(|d| d.select_region(dir));
            }
            Cmd::ZoomIn => self.zoom(0.5, None),
            Cmd::ZoomOut => self.zoom(2.0, None),
            Cmd::ZoomFull => self.doc(|d| {
                d.view.start = 0.0;
                d.view.len = 0.0;
            }),
            Cmd::ZoomSel | Cmd::ZoomLeft | Cmd::ZoomRight => {
                if let Some((a, b)) = self.doc(|d| d.sel) {
                    let (a, b) = (a as f64, b as f64 + 1.0);
                    let edge = (b - a).clamp(64.0, 8192.0);
                    match cmd {
                        Cmd::ZoomSel => self.zoom_to(a, b),
                        Cmd::ZoomLeft => self.zoom_to(a - edge / 2.0, a + edge / 2.0),
                        _ => self.zoom_to(b - edge / 2.0, b + edge / 2.0),
                    }
                }
            }
            Cmd::When(i) => self.shared.when.store(i, Ordering::Relaxed),
            Cmd::Max(i) => self.shared.max.store(i, Ordering::Relaxed),
            Cmd::IgnoreHostSel => Self::toggle(&self.shared.ignore_host_sel),
            Cmd::IgnoreOwnSel => Self::toggle(&self.shared.ignore_own_sel),
        }
        acts
    }

    // ------------------------------------------------------------------------------------------- prompts and files

    pub fn prompt_text(&self, p: &Prompt) -> String {
        self.doc(|d| match p {
            Prompt::Marker => format!("Marker {}", d.sample.cues.iter().filter(|c| !c.is_region()).count() + 1),
            Prompt::Region => format!("Region {}", d.sample.cues.iter().filter(|c| c.is_region()).count() + 1),
            Prompt::Rename(id) => d.sample.cues.iter().find(|c| c.id == *id).map_or(String::new(), |c| c.name.clone()),
            Prompt::Tempo => if d.sample.tempo > 0.0 { format!("{}", d.sample.tempo) } else { String::new() },
        })
    }

    pub fn typed(&mut self, p: &Prompt, text: &str) {
        let t = text.trim();
        match p {
            Prompt::Marker => {
                self.doc(|d| d.add_cue(t, false));
            }
            Prompt::Region => {
                self.doc(|d| d.add_cue(t, true));
            }
            Prompt::Rename(id) => {
                self.doc(|d| d.rename_cue(*id, t));
            }
            Prompt::Tempo => {
                let v = if t.is_empty() { Some(0.0) } else { t.trim_end_matches("bpm").trim().replace(',', ".").parse::<f32>().ok() };
                match v {
                    Some(v) if (0.0..=999.0).contains(&v) => {
                        self.doc(|d| {
                            if d.frames() > 0 || d.sample.tempo != v {
                                d.whole("Edit properties", |s| {
                                    s.tempo = v;
                                    true
                                });
                            }
                        });
                    }
                    _ => self.hint = "Tempo: a number of beats per minute".into(),
                }
            }
        }
    }

    /// A file chosen in the open dialog or dropped on the editor.
    pub fn open(&mut self, path: &str) {
        match load_file(path) {
            Ok(s) => {
                self.doc(|d| d.load(s, path));
                self.hint = format!("Loaded {}", Path::new(path).file_name().map_or(path.into(), |f| f.to_string_lossy()));
            }
            Err(e) => self.hint = format!("Could not load the file: {e}"),
        }
    }

    pub fn save(&mut self, path: &str, depth: Depth) {
        let s = self.doc(|d| d.sample.clone());
        let mut path = path.to_string();
        if !path.to_ascii_lowercase().ends_with(".wav") {
            path.push_str(".wav");
        }
        match std::fs::write(&path, wav::write(&s, depth)) {
            Ok(()) => {
                self.doc(|d| d.path = path.clone());
                self.hint = format!("Saved {}", Path::new(&path).file_name().map_or(path.clone().into(), |f| f.to_string_lossy()));
            }
            Err(e) => self.hint = format!("Could not save: {e}"),
        }
    }

    /// The suggested file name for Save.
    pub fn save_name(&self) -> String {
        self.doc(|d| format!("{}.wav", file_stem(&d.title())))
    }

    /// Pastes WAV bytes from the system clipboard, or the editor's own clipboard without them.
    pub fn paste(&mut self, bytes: Option<Vec<u8>>, mode: Paste) {
        let clip = bytes.and_then(|b| wav::read(&b).ok()).or_else(|| self.clip.clone());
        match clip {
            Some(c) => {
                let ok = self.doc(|d| {
                    if d.frames() == 0 {
                        d.load(c.clone(), "");
                        true
                    } else {
                        d.paste(&c, mode)
                    }
                });
                if !ok {
                    self.hint = "Nothing to paste".into();
                }
            }
            None => self.hint = "The clipboard holds no audio".into(),
        }
    }

    /// The host answered (true) or didn't take a sample sent to it.
    pub fn sent(&mut self, ok: bool, channel: bool) {
        self.hint = match (ok, channel) {
            (true, false) => "Sent to the playlist".into(),
            (true, true) => "Sent to the selected channel".into(),
            (false, _) => "Sending needs Lemon Studio".into(),
        };
    }

    // ------------------------------------------------------------------------------------------- polling and paint

    /// A timer tick: follows playback, picks up a loaded state. True when something to paint changed.
    pub fn poll(&mut self) -> bool {
        let loaded = self.shared.loaded.load(Ordering::Acquire);
        if loaded != self.loaded {
            self.loaded = loaded;
            self.view = ViewState::default();
            self.drag = None;
        }
        let playing = self.shared.playing.load(Ordering::Relaxed);
        if playing && self.drag.is_none() {
            let pos = self.shared.position();
            let w = self.layout().lane().w;
            self.doc(|d| {
                if d.view.follow && d.view.len > 0.0 {
                    let (start, per_px) = view::span(d, d.frames(), w);
                    let len = per_px * w as f64;
                    if pos < start || pos >= start + len {
                        Self::set_view(d, w, pos, len);
                    }
                }
            });
        }
        true
    }

    pub fn paint(&mut self, c: &mut Canvas) {
        let (w, h) = self.size();
        let lay = Layout::new(w, h);
        let lane = lay.lane();
        let sh = self.shared.clone();
        let d = sh.doc.lock().unwrap_or_else(|e| e.into_inner());
        let take = sh.take.lock().unwrap_or_else(|e| e.into_inner());
        if self.peaks_ver != d.version {
            let ch: Vec<&[f32]> = d.sample.ch.iter().map(|c| c.as_slice()).collect();
            self.peaks = Peaks::new(&ch);
            self.peaks_ver = d.version;
        }
        let live = take.active && !take.l.is_empty();
        let tv = sh.take_ver.load(Ordering::Acquire);
        if live {
            if tv != self.take_ver {
                self.take_peaks.extend(&[&take.l, &take.r]);
                self.take_ver = tv;
            }
        } else if self.take_peaks.frames() > 0 {
            self.take_peaks = Peaks::default();
            self.take_ver = u64::MAX;
        }
        let rec = sh.rec();
        let doc_part = || Part { ch: d.sample.ch.iter().map(|c| c.as_slice()).collect(), peaks: &self.peaks, at: 0 };
        let take_part = |at| Part { ch: vec![take.l.as_slice(), take.r.as_slice()], peaks: &self.take_peaks, at };
        let shown = if live && (!rec.append || d.sample.is_empty()) {
            Shown { parts: vec![take_part(0)], rate: take.rate.max(1), channels: 2 }
        } else if live {
            Shown { parts: vec![doc_part(), take_part(d.frames())], rate: d.sample.rate, channels: d.sample.channels() }
        } else {
            Shown { parts: vec![doc_part()], rate: d.sample.rate, channels: d.sample.channels() }
        };
        let spectrum = if d.view.spectrum && shown.frames() > 0 {
            let n = shown.frames();
            let (start, per_px) = view::span(&d, n, lane.w);
            let (sw, sh_) = (lane.w.max(1.0) as usize, lane.h.max(1.0) as usize);
            let key = (d.version, if live { tv } else { 0 }, start.to_bits(), per_px.to_bits(), sw, sh_, d.view.bands, d.view.gradient);
            if self.spectrum_key != Some(key) {
                let chans = shown.channels;
                let at = |i: isize| -> f32 {
                    if i < 0 {
                        return 0.0;
                    }
                    (0..chans).map(|c| shown.at(c, i as usize)).sum::<f32>() / chans as f32
                };
                let bands = state::BANDS[(d.view.bands as usize).min(state::BANDS.len() - 1)];
                self.spectrum = crate::spectrum::render(&at, n, start, per_px, sw, sh_, bands, d.view.gradient as usize);
                self.spectrum_key = Some(key);
            }
            Some((sw, sh_, self.spectrum.as_slice()))
        } else {
            None
        };
        // FREE: what the For limit leaves of the recording, while armed.
        let free = match crate::rec::Recorder::limit_frames(rec.max, take.rate.max(1)) {
            u64::MAX => "-".to_string(),
            _ if !rec.armed => "-".to_string(),
            limit => {
                let s = limit.saturating_sub(take.l.len() as u64) / u64::from(take.rate.max(1));
                format!("{}:{:02}", s / 60, s % 60)
            }
        };
        let look = view::Look {
            doc: &d,
            shown: &shown,
            st: &self.view,
            hint: &self.hint,
            hosted: self.hosted,
            free,
            playing: sh.playing.load(Ordering::Relaxed),
            pos: sh.position(),
            armed: rec.armed,
            capturing: sh.capturing.load(Ordering::Relaxed),
            level: f32::from_bits(sh.level.load(Ordering::Relaxed)),
            rec: &rec,
            looping: sh.looping.load(Ordering::Relaxed),
            can_undo: d.can_undo(),
            spectrum,
        };
        let hits = view::paint(c, &look, w, h);
        drop(look);
        drop(shown);
        drop(take);
        drop(d);
        self.hits = hits;
    }
}

fn about() -> String {
    format!("Tesla {}: Lemon Studio's audio editor and recorder", env!("CARGO_PKG_VERSION"))
}

// ---------------------------------------------------------------------------------------------------------------
// The window

#[cfg(windows)]
mod win32 {
    use std::ffi::c_void;
    use std::path::Path;
    use std::sync::atomic::{AtomicIsize, Ordering};
    use std::sync::Arc;

    use nih_plug::prelude::GuiContext;
    use os_plugkit::win as pw;
    use os_theme::Palette;
    use os_ui::Canvas;
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
    use windows_sys::Win32::Graphics::Gdi::InvalidateRect;
    use windows_sys::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard, SetClipboardData};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE};
    use windows_sys::Win32::System::Ole::CF_WAVE;
    use windows_sys::Win32::UI::Controls::Dialogs::{GetOpenFileNameW, GetSaveFileNameW, OFN_FILEMUSTEXIST, OFN_NOCHANGEDIR, OFN_OVERWRITEPROMPT, OPENFILENAMEW};
    use windows_sys::Win32::UI::Controls::WM_MOUSELEAVE;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture, SetFocus, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT};
    use windows_sys::Win32::UI::Shell::{DragAcceptFiles, DragFinish, DragQueryFileW, HDROP};
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    use super::{Act, Btn, Key, Logic, Mods, Prompt};
    use crate::TeslaParams;

    const TIMER: usize = 1;

    struct Win {
        hwnd: HWND,
        canvas: Canvas,
        buf: Vec<u32>,
        scale: f32,
        /// Painted size in device px.
        size: (u32, u32),
        logic: Logic,
        palette: Palette,
        ctx: Arc<dyn GuiContext>,
        last_hint: String,
        tracking: bool,
        slot: Arc<AtomicIsize>,
    }

    /// Owns the window; dropping it closes the editor.
    pub struct Handle(isize, Arc<AtomicIsize>, Arc<TeslaParams>);

    // SAFETY: the handle is only an HWND value; the wrapper drops it on the GUI thread.
    unsafe impl Send for Handle {}

    impl Drop for Handle {
        fn drop(&mut self) {
            self.1.store(0, Ordering::Release);
            self.2.shared.editor_open.store(false, Ordering::Relaxed);
            unsafe { DestroyWindow(self.0 as HWND) };
        }
    }

    fn mods() -> Mods {
        let (ctrl, shift, alt) = pw::mods();
        Mods { ctrl, shift, alt }
    }

    fn device(v: u32, scale: f32) -> u32 {
        (v as f32 * scale).round() as u32
    }

    pub fn open(parent: *mut c_void, scale: f32, params: Arc<TeslaParams>, ctx: Arc<dyn GuiContext>, slot: Arc<AtomicIsize>) -> Handle {
        let scale = if scale > 0.0 { scale } else { 1.0 };
        let logic = Logic::new(params.clone());
        let (w, h) = logic.size();
        let size = (device(w, scale), device(h, scale));
        params.shared.editor_open.store(true, Ordering::Relaxed);
        unsafe {
            let class = pw::register("LemonTeslaView", Some(wndproc));
            let win = Box::new(Win {
                hwnd: std::ptr::null_mut(),
                canvas: Canvas::new(size.0, size.1, scale).expect("canvas"),
                buf: vec![0; (size.0 * size.1) as usize],
                scale,
                size,
                logic,
                palette: os_plugkit::palette(),
                ctx,
                last_hint: String::new(),
                tracking: false,
                slot: slot.clone(),
            });
            let title = pw::wide("Tesla");
            let hwnd = CreateWindowExW(
                0,
                class.as_ptr(),
                title.as_ptr(),
                WS_CHILD | WS_VISIBLE | WS_CLIPSIBLINGS,
                0,
                0,
                size.0 as i32,
                size.1 as i32,
                parent as HWND,
                std::ptr::null_mut(),
                GetModuleHandleW(std::ptr::null()),
                Box::into_raw(win) as *const c_void,
            );
            slot.store(hwnd as isize, Ordering::Release);
            DragAcceptFiles(hwnd, 1);
            // About 33 frames a second (playhead, recording, level).
            SetTimer(hwnd, TIMER, 30, None);
            Handle(hwnd as isize, slot, params)
        }
    }

    unsafe fn state(hwnd: HWND) -> Option<&'static mut Win> {
        let p = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut Win;
        if p.is_null() { None } else { Some(unsafe { &mut *p }) }
    }

    unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
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
                    (*p).slot.store(0, Ordering::Release);
                    drop(Box::from_raw(p));
                }
                return DefWindowProcW(hwnd, msg, wp, lp);
            }
            let Some(w) = state(hwnd) else { return DefWindowProcW(hwnd, msg, wp, lp) };
            if msg == os_hostlink::message() {
                return w.host_link(wp, lp);
            }
            match msg {
                WM_PAINT => {
                    w.paint();
                    0
                }
                WM_ERASEBKGND => 1,
                WM_TIMER => {
                    w.poll();
                    0
                }
                // Arrow keys, Delete and the rest come here rather than to the host's dialog handling.
                WM_GETDLGCODE => DLGC_WANTALLKEYS as LRESULT,
                WM_LBUTTONDOWN | WM_LBUTTONDBLCLK | WM_RBUTTONDOWN | WM_RBUTTONDBLCLK | WM_MBUTTONDOWN => {
                    SetFocus(hwnd);
                    let (x, y) = w.mouse(lp);
                    let btn = match msg {
                        WM_RBUTTONDOWN | WM_RBUTTONDBLCLK => Btn::Right,
                        WM_MBUTTONDOWN => Btn::Middle,
                        _ => Btn::Left,
                    };
                    let acts = w.logic.down(x, y, btn, mods(), msg == WM_LBUTTONDBLCLK);
                    if w.logic.dragging() {
                        SetCapture(hwnd);
                    }
                    w.run(acts);
                    0
                }
                WM_MOUSEMOVE => {
                    if !w.tracking {
                        let mut tme = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
                        TrackMouseEvent(&mut tme);
                        w.tracking = true;
                    }
                    let (x, y) = w.mouse(lp);
                    let acts = w.logic.moved(x, y, mods());
                    w.run(acts);
                    0
                }
                WM_MOUSELEAVE => {
                    w.tracking = false;
                    if !w.logic.dragging() {
                        let acts = w.logic.leave();
                        w.run(acts);
                    }
                    0
                }
                WM_LBUTTONUP | WM_RBUTTONUP | WM_MBUTTONUP => {
                    ReleaseCapture();
                    let acts = w.logic.up(Btn::Left);
                    w.run(acts);
                    0
                }
                WM_CAPTURECHANGED => {
                    if w.logic.dragging() && lp as HWND != hwnd {
                        let acts = w.logic.up(Btn::Left);
                        w.run(acts);
                    }
                    0
                }
                WM_MOUSEWHEEL => {
                    let mut r: RECT = std::mem::zeroed();
                    GetWindowRect(hwnd, &mut r);
                    let (sx, sy) = pw::xy(lp);
                    let (x, y) = ((sx - r.left) as f32 / w.scale, (sy - r.top) as f32 / w.scale);
                    let notches = ((wp >> 16) & 0xffff) as u16 as i16 as f32 / 120.0;
                    let acts = w.logic.wheel(x, y, notches, mods());
                    w.run(acts);
                    0
                }
                WM_KEYDOWN | WM_SYSKEYDOWN => {
                    let acts = match Key::from_vk(wp as u32) {
                        Some(k) => w.logic.key(k, mods()),
                        None => vec![],
                    };
                    if acts.is_empty() {
                        return DefWindowProcW(hwnd, msg, wp, lp);
                    }
                    w.run(acts);
                    0
                }
                // Alt+letter shortcuts would otherwise beep (no menu bar to find them in).
                WM_SYSCHAR => 0,
                WM_DROPFILES => {
                    let drop = wp as HDROP;
                    let mut buf = vec![0u16; 1024];
                    let n = DragQueryFileW(drop, 0, buf.as_mut_ptr(), buf.len() as u32) as usize;
                    DragFinish(drop);
                    if n > 0 {
                        w.logic.open(&String::from_utf16_lossy(&buf[..n]));
                        w.invalidate();
                    }
                    0
                }
                WM_DESTROY => {
                    KillTimer(hwnd, TIMER);
                    0
                }
                _ => DefWindowProcW(hwnd, msg, wp, lp),
            }
        }
    }

    impl Win {
        fn mouse(&self, l: LPARAM) -> (f32, f32) {
            let (x, y) = pw::xy(l);
            (x as f32 / self.scale, y as f32 / self.scale)
        }

        fn screen(&self, x: f32, y: f32) -> (i32, i32) {
            pw::to_screen(self.hwnd, (x * self.scale) as i32, (y * self.scale) as i32)
        }

        fn invalidate(&self) {
            unsafe { InvalidateRect(self.hwnd, std::ptr::null(), 0) };
        }

        fn poll(&mut self) {
            if self.logic.poll() {
                self.invalidate();
            }
            if self.logic.hint != self.last_hint {
                self.last_hint = self.logic.hint.clone();
                if self.logic.hosted {
                    os_hostlink::to_host(self.hwnd as isize, os_hostlink::Cmd::Changed, 0);
                }
            }
        }

        fn run(&mut self, acts: Vec<Act>) {
            let mut repaint = false;
            for a in acts {
                match a {
                    Act::Repaint => repaint = true,
                    Act::Menu(req, x, y) => {
                        let def = self.logic.menu(&req);
                        let (sx, sy) = self.screen(x, y);
                        if let Some(cmd) = pw::popup(self.hwnd, sx, sy, &def, &self.palette, self.scale) {
                            let acts = self.logic.pick(&req, cmd, x, y);
                            self.run(acts);
                        }
                        repaint = true;
                    }
                    Act::TypeIn(p, x, y) => {
                        let (sx, sy) = self.screen(x, y);
                        let cur = self.logic.prompt_text(&p);
                        if let Some(t) = pw::type_in(self.hwnd, sx, sy, &cur, self.scale) {
                            self.logic.typed(&p, &t);
                        } else if matches!(p, Prompt::Marker | Prompt::Region) && os_plugkit::no_modal() {
                            // No prompt can show (tests, offscreen): the suggested name.
                            self.logic.typed(&p, &cur);
                        }
                        repaint = true;
                    }
                    Act::Open => {
                        if let Some(path) = file_dialog(self.hwnd, None) {
                            self.logic.open(&path);
                        }
                        repaint = true;
                    }
                    Act::Save(depth) => {
                        let name = self.logic.save_name();
                        if let Some(path) = file_dialog(self.hwnd, Some(&name)) {
                            self.logic.save(&path, depth);
                        }
                        repaint = true;
                    }
                    Act::Clipboard(bytes) => set_clipboard_wave(self.hwnd, &bytes),
                    Act::Paste(mode) => {
                        let bytes = clipboard_wave(self.hwnd);
                        self.logic.paste(bytes, mode);
                        repaint = true;
                    }
                    Act::DragOut(path) => {
                        // The drag runs its own loop until the drop; the button is up by then.
                        unsafe { ReleaseCapture() };
                        if !drag_out(self.hwnd, &path) {
                            self.logic.hint = "Could not start the drag".into();
                        }
                        let acts = self.logic.up(Btn::Left);
                        self.run(acts);
                    }
                    Act::Send(path, channel) => {
                        os_hostlink::post_audio(&path.to_string_lossy(), channel);
                        let ok = os_hostlink::to_host(self.hwnd as isize, os_hostlink::Cmd::Audio, channel as isize) == 1;
                        if !ok {
                            os_hostlink::take_audio();
                        }
                        self.logic.sent(ok, channel);
                        repaint = true;
                    }
                    Act::Resize(w, h) => {
                        self.size = (device(w, self.scale), device(h, self.scale));
                        self.canvas.resize(self.size.0, self.size.1, self.scale);
                        unsafe { SetWindowPos(self.hwnd, std::ptr::null_mut(), 0, 0, self.size.0 as i32, self.size.1 as i32, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE) };
                        if self.logic.hosted {
                            os_hostlink::to_host(self.hwnd as isize, os_hostlink::Cmd::Changed, 0);
                        } else {
                            self.ctx.request_resize();
                        }
                        repaint = true;
                    }
                }
            }
            if repaint {
                self.invalidate();
            }
        }

        /// Answers the host (spec `plugins/host-link.md`): one page, the hint line, the editor's size.
        unsafe fn host_link(&mut self, wp: WPARAM, lp: LPARAM) -> LRESULT {
            use os_hostlink::{Cmd, PageIcon};
            let Some(cmd) = Cmd::from_usize(wp) else { return 0 };
            match cmd {
                Cmd::Hello => {
                    self.logic.hosted = true;
                    self.invalidate();
                    os_hostlink::VERSION as LRESULT
                }
                Cmd::PageCount => 1,
                Cmd::PageIcon => PageIcon::Sample as LRESULT,
                Cmd::GetPage | Cmd::SetPage => 0,
                Cmd::Metrics => {
                    let (w, h) = self.logic.size();
                    os_hostlink::pack_size(w, h)
                }
                Cmd::Hint => {
                    if lp != 0 {
                        // SAFETY: the host passes a `HintBuf` of its own process (spec: host-link.md).
                        unsafe { &mut *(lp as *mut os_hostlink::HintBuf) }.set(&self.logic.hint);
                    }
                    1
                }
                // The host's frame may resize the editor (spec: plugins/tesla/README.md "The window").
                Cmd::Resizable => os_hostlink::pack_size(crate::view::MIN_W, crate::view::MIN_H),
                Cmd::SetSize => {
                    let (w, h) = os_hostlink::unpack_size(lp);
                    let acts = self.logic.resize_to(w, h);
                    self.run(acts);
                    1
                }
                Cmd::Changed | Cmd::Note | Cmd::Automate | Cmd::Audio | Cmd::Notes => 0,
            }
        }

        fn paint(&mut self) {
            let (hdc, ps) = pw::begin_paint(self.hwnd);
            self.logic.paint(&mut self.canvas);
            self.buf.resize((self.size.0 * self.size.1) as usize, 0);
            self.canvas.copy_to_xrgb(&mut self.buf);
            pw::blit(hdc, &self.buf, self.size.0, self.size.1);
            pw::end_paint(self.hwnd, &ps);
        }
    }

    fn file_dialog(owner: HWND, save: Option<&str>) -> Option<String> {
        if os_plugkit::no_modal() {
            return None;
        }
        let mut file = vec![0u16; 1024];
        if let Some(name) = save {
            for (o, c) in file.iter_mut().zip(name.encode_utf16().take(1000)) {
                *o = c;
            }
        }
        let filter: Vec<u16> = if save.is_some() { "WAV files\0*.wav\0\0" } else { "Audio files\0*.wav;*.flac;*.ogg;*.wv;*.aif;*.aiff;*.mp3\0All files\0*.*\0\0" }
            .encode_utf16()
            .collect();
        let title = pw::wide(if save.is_some() { "Save sample as" } else { "Load sample" });
        let ext = pw::wide("wav");
        unsafe {
            let mut ofn: OPENFILENAMEW = std::mem::zeroed();
            ofn.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
            ofn.hwndOwner = owner;
            ofn.lpstrFilter = filter.as_ptr();
            ofn.lpstrFile = file.as_mut_ptr();
            ofn.nMaxFile = file.len() as u32;
            ofn.lpstrTitle = title.as_ptr();
            ofn.lpstrDefExt = ext.as_ptr();
            ofn.Flags = OFN_NOCHANGEDIR | if save.is_some() { OFN_OVERWRITEPROMPT } else { OFN_FILEMUSTEXIST };
            let ok = if save.is_some() { GetSaveFileNameW(&mut ofn) } else { GetOpenFileNameW(&mut ofn) };
            if ok == 0 {
                return None;
            }
        }
        let end = file.iter().position(|&c| c == 0).unwrap_or(file.len());
        Some(String::from_utf16_lossy(&file[..end]))
    }

    fn set_clipboard_wave(owner: HWND, bytes: &[u8]) {
        unsafe {
            if OpenClipboard(owner) == 0 {
                return;
            }
            EmptyClipboard();
            let h = GlobalAlloc(GMEM_MOVEABLE, bytes.len());
            if !h.is_null() {
                let p = GlobalLock(h) as *mut u8;
                if !p.is_null() {
                    std::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
                    GlobalUnlock(h);
                    SetClipboardData(u32::from(CF_WAVE), h);
                }
            }
            CloseClipboard();
        }
    }

    fn clipboard_wave(owner: HWND) -> Option<Vec<u8>> {
        unsafe {
            if OpenClipboard(owner) == 0 {
                return None;
            }
            let h = GetClipboardData(u32::from(CF_WAVE));
            let out = if h.is_null() {
                None
            } else {
                let size = GlobalSize(h);
                let p = GlobalLock(h) as *const u8;
                let v = (!p.is_null()).then(|| std::slice::from_raw_parts(p, size).to_vec());
                GlobalUnlock(h);
                v
            };
            CloseClipboard();
            out
        }
    }

    /// Drags the file out as a shell item (Explorer, the host's playlist, other programs take it as a dropped file).
    fn drag_out(hwnd: HWND, path: &Path) -> bool {
        use windows_sys::core::GUID;
        use windows_sys::Win32::System::Ole::{OleInitialize, DROPEFFECT_COPY};
        use windows_sys::Win32::UI::Shell::Common::ITEMIDLIST;
        use windows_sys::Win32::UI::Shell::{ILClone, ILFindLastID, ILFree, ILRemoveLastID, SHCreateDataObject, SHDoDragDrop, SHParseDisplayName};

        #[repr(C)]
        struct Unknown {
            query_interface: usize,
            add_ref: usize,
            release: unsafe extern "system" fn(*mut c_void) -> u32,
        }
        const IID_IDATAOBJECT: GUID = GUID::from_u128(0x0000010e_0000_0000_c000_000000000046);

        let wide = pw::wide(&path.to_string_lossy());
        unsafe {
            OleInitialize(std::ptr::null_mut());
            let mut full: *mut ITEMIDLIST = std::ptr::null_mut();
            if SHParseDisplayName(wide.as_ptr(), std::ptr::null_mut(), &mut full, 0, std::ptr::null_mut()) < 0 || full.is_null() {
                return false;
            }
            let parent = ILClone(full);
            ILRemoveLastID(parent);
            let child = ILFindLastID(full) as *const ITEMIDLIST;
            let mut obj: *mut c_void = std::ptr::null_mut();
            let made = SHCreateDataObject(parent, 1, &child, std::ptr::null_mut(), &IID_IDATAOBJECT, &mut obj) >= 0 && !obj.is_null();
            if made {
                let mut effect = 0;
                SHDoDragDrop(hwnd, obj as _, std::ptr::null_mut(), DROPEFFECT_COPY, &mut effect);
                let vt = *(obj as *const *const Unknown);
                ((*vt).release)(obj);
            }
            ILFree(parent);
            ILFree(full);
            made
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
mod gui {
    use std::sync::atomic::Ordering;
    use std::sync::Arc;

    use nih_plug::prelude::GuiContext;
    use os_hostlink::{Cmd, PageIcon, Peer};
    use os_plugkit::gui::{self as px, Button, Input, Window};
    use os_theme::Palette;
    use os_ui::Canvas;

    use super::{Act, Btn, Key, Logic, Mods};
    use crate::TeslaParams;

    struct Ed {
        win: Window,
        canvas: Canvas,
        buf: Vec<u32>,
        scale: f32,
        size: (u32, u32),
        logic: Logic,
        palette: Palette,
        ctx: Arc<dyn GuiContext>,
        last_hint: String,
        dirty: bool,
        link: Arc<Peer>,
    }

    impl Drop for Ed {
        fn drop(&mut self) {
            self.link.unregister();
        }
    }

    pub struct Handle(Option<px::EditorThread>, Arc<TeslaParams>);

    impl Drop for Handle {
        fn drop(&mut self) {
            self.0.take();
            self.1.shared.editor_open.store(false, Ordering::Relaxed);
        }
    }

    fn device(v: u32, scale: f32) -> u32 {
        (v as f32 * scale).round() as u32
    }

    pub fn open(parent: px::Parent, scale: f32, params: Arc<TeslaParams>, ctx: Arc<dyn GuiContext>, slot: &px::Slot) -> Handle {
        let scale = if scale > 0.0 { scale } else { 1.0 };
        let (w, h) = (params.shared.size.0.load(Ordering::Relaxed), params.shared.size.1.load(Ordering::Relaxed));
        let size = (device(w, scale), device(h, scale));
        let wake = slot.clone();
        let link = Peer::register(parent as u64, vec![PageIcon::Sample], (w, h), move || px::notify(&wake));
        // The host's frame may resize the editor (spec: plugins/tesla/README.md "The window").
        link.set_resizable((crate::view::MIN_W, crate::view::MIN_H));
        let peer = link.clone();
        let p = params.clone();
        let thread = px::spawn(parent, size, slot, move |win| {
            let Ok(canvas) = Canvas::new(size.0, size.1, scale) else { return };
            let logic = Logic::new(p);
            let ed = Ed { win, canvas, buf: Vec::new(), scale, size, logic, palette: os_plugkit::palette(), ctx, last_hint: String::new(), dirty: true, link: peer };
            #[cfg(target_os = "macos")]
            ed.win.accept_files();
            ed.run();
        });
        if thread.is_none() {
            link.unregister();
        }
        params.shared.editor_open.store(thread.is_some(), Ordering::Relaxed);
        Handle(thread, params)
    }

    fn btn(b: Button) -> Btn {
        match b {
            Button::Left => Btn::Left,
            Button::Right => Btn::Right,
            Button::Middle => Btn::Middle,
        }
    }

    impl Ed {
        fn run(mut self) {
            while let Some(input) = self.win.next() {
                match input {
                    Input::Expose => self.dirty = true,
                    Input::Tick => self.poll(),
                    Input::Down { x, y, btn: b, dbl } => {
                        let acts = self.logic.down(x / self.scale, y / self.scale, btn(b), self.mods(), dbl && b == Button::Left);
                        self.act(acts);
                    }
                    Input::Move { x, y } => {
                        let acts = self.logic.moved(x / self.scale, y / self.scale, self.mods());
                        self.act(acts);
                    }
                    Input::Delta { .. } => {}
                    Input::Up { .. } => {
                        let acts = self.logic.up(Btn::Left);
                        self.act(acts);
                    }
                    Input::Leave => {
                        if !self.logic.dragging() {
                            let acts = self.logic.leave();
                            self.act(acts);
                        }
                    }
                    Input::Wheel { x, y, notches } => {
                        let acts = self.logic.wheel(x / self.scale, y / self.scale, notches, self.mods());
                        self.act(acts);
                    }
                    Input::Key { sym, .. } => {
                        if let Some(k) = Key::from_sym(sym) {
                            let acts = self.logic.key(k, self.mods());
                            self.act(acts);
                        }
                    }
                    Input::Drop { path } => {
                        self.logic.open(&path);
                        self.dirty = true;
                    }
                }
                if self.dirty && self.win.idle() {
                    self.paint();
                }
            }
        }

        fn mods(&self) -> Mods {
            let (ctrl, shift, alt) = self.win.mods();
            Mods { ctrl, shift, alt }
        }

        fn screen(&self, x: f32, y: f32) -> (i32, i32) {
            self.win.to_root((x * self.scale) as i32, (y * self.scale) as i32)
        }

        fn poll(&mut self) {
            if self.logic.poll() {
                self.dirty = true;
            }
            if self.logic.hint != self.last_hint {
                self.last_hint.clone_from(&self.logic.hint);
                self.link.set_hint(&self.logic.hint);
                self.link.to_host(Cmd::Changed, 0);
            }
            self.logic.hosted = self.link.hosted();
            // The host's frame was dragged to a new size.
            if let Some((w, h)) = self.link.take_size_request() {
                let acts = self.logic.resize_to(w, h);
                self.act(acts);
            }
        }

        fn act(&mut self, acts: Vec<Act>) {
            for a in acts {
                match a {
                    Act::Repaint => self.dirty = true,
                    Act::Menu(req, x, y) => {
                        let def = self.logic.menu(&req);
                        let (sx, sy) = self.screen(x, y);
                        if let Some(cmd) = self.win.popup(sx, sy, &def, &self.palette, self.scale) {
                            let acts = self.logic.pick(&req, cmd, x, y);
                            self.act(acts);
                        }
                        self.dirty = true;
                    }
                    Act::TypeIn(p, x, y) => {
                        let (sx, sy) = self.screen(x, y);
                        let cur = self.logic.prompt_text(&p);
                        if let Some(t) = self.win.type_in(sx, sy, &cur, self.scale) {
                            self.logic.typed(&p, &t);
                        }
                        self.dirty = true;
                    }
                    Act::Open => {
                        if let Some(path) = file_dialog(&self.win, None) {
                            self.logic.open(&path);
                        }
                        self.dirty = true;
                    }
                    Act::Save(depth) => {
                        let name = self.logic.save_name();
                        if let Some(path) = file_dialog(&self.win, Some(name)) {
                            self.logic.save(&path, depth);
                        }
                        self.dirty = true;
                    }
                    Act::Clipboard(bytes) => {
                        #[cfg(target_os = "macos")]
                        self.win.copy_audio(bytes);
                        #[cfg(not(target_os = "macos"))]
                        let _ = bytes;
                    }
                    Act::Paste(mode) => {
                        #[cfg(target_os = "macos")]
                        let bytes = self.win.paste_audio();
                        #[cfg(not(target_os = "macos"))]
                        let bytes = None;
                        self.logic.paste(bytes, mode);
                        self.dirty = true;
                    }
                    // No drag source here yet: the sample goes to the clipboard instead.
                    Act::DragOut(path) => {
                        if let Ok(bytes) = std::fs::read(&path) {
                            self.act(vec![Act::Clipboard(bytes)]);
                        }
                        self.logic.hint = "Copied to the clipboard (dragging out is Windows only so far)".into();
                        let acts = self.logic.up(Btn::Left);
                        self.act(acts);
                    }
                    Act::Send(path, channel) => {
                        os_hostlink::post_audio(&path.to_string_lossy(), channel);
                        let ok = self.link.hosted() && self.link.to_host(Cmd::Audio, channel as isize) == 1;
                        if !ok {
                            os_hostlink::take_audio();
                        }
                        self.logic.sent(ok, channel);
                        self.dirty = true;
                    }
                    Act::Resize(w, h) => {
                        self.size = (device(w, self.scale), device(h, self.scale));
                        self.canvas.resize(self.size.0, self.size.1, self.scale);
                        self.win.resize(self.size.0, self.size.1);
                        // Lemon Studio's frame follows the link; other hosts the request.
                        self.link.set_size(w, h);
                        self.ctx.request_resize();
                        self.dirty = true;
                    }
                }
            }
        }

        fn paint(&mut self) {
            self.dirty = false;
            self.logic.paint(&mut self.canvas);
            self.buf.resize((self.size.0 * self.size.1) as usize, 0);
            self.canvas.copy_to_xrgb(&mut self.buf);
            self.win.blit(&self.buf, self.size.0, self.size.1);
        }
    }

    fn file_dialog(win: &Window, save: Option<String>) -> Option<String> {
        if os_plugkit::no_modal() {
            return None;
        }
        let path = win.dialog(move || {
            let d = rfd::FileDialog::new();
            match save {
                Some(name) => d.set_title("Save sample as").set_file_name(name).add_filter("WAV files", &["wav"]).save_file(),
                None => d.set_title("Load sample").add_filter("Audio files", &["wav", "flac", "ogg", "wv", "aif", "aiff", "mp3"]).pick_file(),
            }
        });
        path.flatten().map(|p| p.to_string_lossy().into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn logic() -> Logic {
        let mut l = Logic::new(Arc::new(TeslaParams::default()));
        paint(&mut l);
        l
    }

    fn paint(l: &mut Logic) {
        let (w, h) = l.size();
        let mut c = Canvas::new(w, h, 1.0).unwrap();
        l.paint(&mut c);
    }

    fn centre(l: &Logic, t: Target) -> (f32, f32) {
        let h = l.hits.iter().rev().find(|h| h.t == t).unwrap_or_else(|| panic!("no hit {t:?}"));
        (h.r.cx(), h.r.cy())
    }

    fn sine(frames: usize) -> Sample {
        let ch: Vec<f32> = (0..frames).map(|i| (i as f32 * 0.05).sin() * 0.5).collect();
        Sample::from_channels(44100, vec![ch.clone(), ch.iter().map(|v| v * 0.5).collect()], Format::Float32)
    }

    fn load(l: &mut Logic, s: Sample) {
        l.doc(|d| d.load(s, "test.wav"));
        paint(l);
    }

    #[test]
    fn controls_stay_inside_and_apart() {
        let l = logic();
        let (w, h) = l.size();
        for (i, a) in l.hits.iter().enumerate() {
            assert!(a.r.x >= 0.0 && a.r.y >= 0.0 && a.r.right() <= w as f32 + 0.01 && a.r.bottom() <= h as f32 + 0.01, "{:?}", a.t);
            for b in &l.hits[i + 1..] {
                // The wave area holds its flags; the scroll bar and the grip may touch the edges.
                if matches!((a.t, b.t), (Target::Wave, Target::Flag(_) | Target::LoopFlag) | (Target::Flag(_) | Target::LoopFlag, Target::Wave)) {
                    continue;
                }
                let apart = a.r.right() <= b.r.x || b.r.right() <= a.r.x || a.r.bottom() <= b.r.y || b.r.bottom() <= a.r.y;
                assert!(apart, "{:?} overlaps {:?}", a.t, b.t);
            }
        }
        for t in [Target::Transport(Tr::Play), Target::Transport(Tr::Record), Target::When, Target::Max, Target::Tool(17), Target::Menu(MenuId::Zoom), Target::Grip] {
            centre(&l, t);
        }
    }

    #[test]
    fn an_empty_wave_opens_the_load_dialog() {
        let mut l = logic();
        let (x, y) = centre(&l, Target::Wave);
        assert!(l.down(x, y, Btn::Left, Mods::default(), false).contains(&Act::Open));
        l.up(Btn::Left);
        assert_eq!(l.hover_hint(x, y), "Click to load a sample");
    }

    #[test]
    fn dragging_selects_and_snaps() {
        let mut l = logic();
        load(&mut l, sine(44100));
        let lane = l.layout().lane();
        // Snapping off: frames per pixel = 44100 / lane width.
        l.doc(|d| d.view.snap_on = false);
        let per = 44100.0 / lane.w as f64;
        l.down(lane.x + 100.0, lane.cy(), Btn::Left, Mods::default(), false);
        l.moved(lane.x + 300.0, lane.cy(), Mods::default());
        l.up(Btn::Left);
        let (a, b) = l.doc(|d| d.sel).unwrap();
        assert_eq!(a, (100.0 * per).round() as usize);
        assert_eq!(b + 1, (300.0 * per).round() as usize);
        assert!(l.hint.starts_with("Selection: "));
        // A click without a drag clears the selection and sets the cursor.
        l.down(lane.x + 50.0, lane.cy(), Btn::Left, Mods::default(), false);
        l.up(Btn::Left);
        assert_eq!(l.doc(|d| (d.sel, d.cursor)), (None, (50.0 * per).round() as usize));
        // Snap to grid (min:sec:ms at this zoom: lines every 0.1 s, snapping to quarters of it).
        l.doc(|d| {
            d.view.snap_on = true;
            d.view.time = state::TIME_MSM;
        });
        l.down(lane.x + 100.0, lane.cy(), Btn::Left, Mods::default(), false);
        l.moved(lane.x + 300.0, lane.cy(), Mods::default());
        l.up(Btn::Left);
        let (a, _) = l.doc(|d| d.sel).unwrap();
        let step = view::grid_step(state::TIME_MSM, 44100, 0.0, per, 90.0) / 4.0;
        assert!((a as f64 / step - (a as f64 / step).round()).abs() < 1e-6, "{a} on {step}");
    }

    #[test]
    fn zoom_and_scroll() {
        let mut l = logic();
        load(&mut l, sine(100_000));
        l.pick(&MenuReq::Main(MenuId::Zoom), Cmd::ZoomIn, 0.0, 0.0);
        assert_eq!(l.doc(|d| (d.view.start, d.view.len)), (25_000.0, 50_000.0));
        let lane = l.layout().lane();
        l.wheel(lane.x, lane.cy(), 1.0, Mods::default());
        let (s, len) = l.doc(|d| (d.view.start, d.view.len));
        assert!((len - 40_000.0).abs() < 1.0 && (s - 25_000.0).abs() < 1.0, "{s} {len}");
        l.wheel(lane.x, lane.cy(), 1.0, Mods { shift: true, ..Mods::default() });
        assert!(l.doc(|d| d.view.start) < 25_000.0);
        l.key(Key::Home, Mods::default());
        assert_eq!(l.doc(|d| d.view.len), 0.0);
        l.doc(|d| d.select(1000, 1999));
        l.key(Key::End, Mods::default());
        assert_eq!(l.doc(|d| (d.view.start, d.view.len)), (1000.0, 1000.0));
    }

    #[test]
    fn keys_and_menus_edit_the_sample() {
        let mut l = logic();
        load(&mut l, sine(1000));
        l.doc(|d| {
            d.view.clickfree = false;
            d.select(100, 199);
        });
        l.key(Key::Char('C'), Mods { ctrl: true, ..Mods::default() });
        l.key(Key::Delete, Mods::default());
        assert_eq!(l.doc(|d| d.frames()), 900);
        // Paste from the editor's own clipboard (no system one in tests).
        assert_eq!(l.key(Key::Char('V'), Mods { ctrl: true, ..Mods::default() }).last(), Some(&Act::Paste(Paste::Insert)));
        l.paste(None, Paste::Insert);
        assert_eq!(l.doc(|d| d.frames()), 1000);
        l.key(Key::Char('Z'), Mods { ctrl: true, ..Mods::default() });
        assert_eq!(l.doc(|d| d.frames()), 900);
        assert_eq!(l.hint, "Undone: Paste insert");
        // Insert silence without a selection: one second.
        l.doc(|d| d.sel = None);
        l.key(Key::Insert, Mods::default());
        assert_eq!(l.doc(|d| d.frames()), 900 + 44100);
        let m = l.menu(&MenuReq::Main(MenuId::Tools));
        assert_eq!(m.find("Normalize"), Some(Cmd::Tool(ToolCmd::Normalize)));
        assert_eq!(m.find("Send to selected channel"), Some(Cmd::Send(true)));
        let m = l.menu(&MenuReq::Main(MenuId::Options));
        assert_eq!(m.find("Zoom out full"), Some(Cmd::ZoomFull));
        assert_eq!(l.key(Key::Char('M'), Mods::default()).last(), Some(&Act::TypeIn(Prompt::Marker, 40.0, 140.0)));
        l.typed(&Prompt::Marker, "kick");
        assert_eq!(l.doc(|d| d.sample.cues[0].name.clone()), "kick");
        l.typed(&Prompt::Tempo, "128");
        assert_eq!(l.doc(|d| d.sample.tempo), 128.0);
    }

    #[test]
    fn tools_left_and_right() {
        let mut l = logic();
        load(&mut l, sine(4410));
        let (x, y) = centre(&l, Target::Tool(1));
        l.down(x, y, Btn::Left, Mods::default(), false);
        l.up(Btn::Left);
        assert!((l.doc(|d| d.sample.peak(0, 4409)) - 1.0).abs() < 1e-6);
        assert_eq!(l.hint, "Normalize");
        let (x, y) = centre(&l, Target::Tool(9));
        l.down(x, y, Btn::Right, Mods::default(), false);
        assert!(l.doc(|d| d.sample.ch[0][10].abs() < d.sample.ch[1][10].abs()));
        let (x, y) = centre(&l, Target::Tool(15));
        l.down(x, y, Btn::Left, Mods::default(), false);
        assert_eq!(l.hint, "Auto-slicing is not in this version");
        let (x, y) = centre(&l, Target::Tool(0));
        assert!(matches!(l.down(x, y, Btn::Right, Mods::default(), false).last(), Some(Act::Menu(MenuReq::Undo, _, _))));
        assert_eq!(l.menu(&MenuReq::Undo).find("Swap channels"), Some(Cmd::UndoTo(1)));
    }

    #[test]
    fn the_amp_panel_previews_and_cancels() {
        let mut l = logic();
        load(&mut l, Sample::from_channels(44100, vec![vec![0.5; 100], vec![0.5; 100]], Format::Float32));
        l.pick(&MenuReq::Main(MenuId::Tools), Cmd::Tool(ToolCmd::Amp), 0.0, 0.0);
        paint(&mut l);
        let lay = l.layout();
        let (_, parts) = view::amp_parts(&lay);
        let vol = parts.iter().find(|(c, _)| *c == AmpCtl::Volume).unwrap().1;
        let (top, _) = view::fader_travel(vol);
        // The top of the volume fader: ×4.
        l.down(vol.cx(), top, Btn::Left, Mods::default(), false);
        l.up(Btn::Left);
        assert_eq!(l.view.amp.unwrap().amp.volume, 1.0);
        let (x, y) = centre(&l, Target::Amp(AmpCtl::Preview));
        l.down(x, y, Btn::Left, Mods::default(), false);
        assert!((l.doc(|d| d.sample.ch[0][0]) - 2.0).abs() < 1e-5);
        // Cancel: the sample is as it was, with no undo or redo step left over.
        l.key(Key::Escape, Mods::default());
        assert!(l.view.amp.is_none());
        assert_eq!(l.doc(|d| (d.sample.ch[0][0], d.can_redo())), (0.5, false));
        // Accept without a preview applies once.
        l.pick(&MenuReq::Main(MenuId::Tools), Cmd::Tool(ToolCmd::Amp), 0.0, 0.0);
        paint(&mut l);
        l.set_amp_value(AmpCtl::Volume, 0.0);
        let (x, y) = centre(&l, Target::Amp(AmpCtl::Accept));
        l.down(x, y, Btn::Left, Mods::default(), false);
        assert_eq!(l.doc(|d| d.sample.ch[0][0]), 0.0);
        assert_eq!(l.doc(|d| d.history()[0].clone()), "Amp");
    }

    /// Time stretch / pitch shift (the tool grid's stretch button, Alt+T): the panel's Time knob sets the new length
    /// of the selection, the marker after it moves with the audio, Accept is one undo step, and the pitch knob shifts
    /// a tone by its semitones.
    #[test]
    fn time_stretch_and_pitch_shift() {
        let mut l = logic();
        let mut s = sine(44100);
        s.add_cue("after", 30000, None);
        load(&mut l, s);
        l.doc(|d| d.select(10000, 19999));
        let (x, y) = centre(&l, Target::Tool(12));
        l.down(x, y, Btn::Left, Mods::default(), false);
        l.up(Btn::Left);
        assert!(l.view.amp.is_some_and(|a| a.stretch.is_some()), "the stretch panel is open");
        paint(&mut l);
        // Time +0.5: twice as long.
        l.set_amp_value(AmpCtl::Time, 0.5);
        assert_eq!(l.amp_hint(AmpCtl::Time), "Length: x2.000");
        let (x, y) = centre(&l, Target::Amp(AmpCtl::Accept));
        l.down(x, y, Btn::Left, Mods::default(), false);
        assert!(l.view.amp.is_none());
        assert_eq!(l.doc(|d| (d.frames(), d.sel, d.sample.cues[0].pos)), (54100, Some((10000, 29999)), 40000));
        assert_eq!(l.doc(|d| d.history()[0].clone()), "Time stretch / pitch shift");
        l.doc(|d| d.undo());
        assert_eq!(l.doc(|d| d.frames()), 44100);
        // Pitch +12 over the whole sample: the tone's zero crossings double, the length stays.
        l.doc(|d| d.sel = None);
        let crossings = |l: &mut Logic| l.doc(|d| d.sample.ch[0][8000..36000].windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count());
        let before = crossings(&mut l);
        l.key(Key::Char('T'), Mods { alt: true, ..Mods::default() });
        paint(&mut l);
        l.set_amp_value(AmpCtl::Pitch, 0.5);
        assert_eq!(l.amp_hint(AmpCtl::Pitch), "Pitch: +12.00 semitones");
        let (x, y) = centre(&l, Target::Amp(AmpCtl::Accept));
        l.down(x, y, Btn::Left, Mods::default(), false);
        let after = crossings(&mut l);
        assert_eq!(l.doc(|d| d.frames()), 44100);
        assert!((after as f32 / before as f32 - 2.0).abs() < 0.05, "{before} -> {after}");
    }

    /// Formant preservation (the panel's switch) changes how a pitch shift sounds; the Formant knob only says
    /// "with pitch" while it is off.
    #[test]
    fn formant_preservation() {
        // A buzz: a pulse train has formant-like harmonics for the stretcher to keep.
        let ch: Vec<f32> = (0..44100).map(|i| if i % 200 < 4 { 0.8 } else { 0.0 }).collect();
        let shifted = |preserve: bool| {
            let mut l = logic();
            load(&mut l, Sample::from_channels(44100, vec![ch.clone()], Format::Float32));
            l.pick(&MenuReq::Main(MenuId::Tools), Cmd::Tool(ToolCmd::Stretch), 0.0, 0.0);
            paint(&mut l);
            l.set_amp_value(AmpCtl::Pitch, 0.25);
            if preserve {
                assert_eq!(l.amp_hint(AmpCtl::Formant), "Formant: moves with the pitch (formant preservation off)");
                let (x, y) = centre(&l, Target::Amp(AmpCtl::Preserve));
                l.down(x, y, Btn::Left, Mods::default(), false);
                assert_eq!(l.amp_hint(AmpCtl::Formant), "Formant: +0.00 semitones");
            }
            let (x, y) = centre(&l, Target::Amp(AmpCtl::Accept));
            l.down(x, y, Btn::Left, Mods::default(), false);
            l.doc(|d| (d.sample.channels(), d.history()[0].clone(), d.sample.ch[0].clone()))
        };
        let (plain, kept) = (shifted(false), shifted(true));
        assert_eq!((plain.0, kept.0), (1, 1), "a mono sample stays mono");
        assert_eq!((plain.1.as_str(), kept.1.as_str()), ("Time stretch / pitch shift", "Time stretch / pitch shift"));
        let diff: f32 = plain.2.iter().zip(&kept.2).map(|(a, b)| (a - b).abs()).sum::<f32>() / plain.2.len() as f32;
        assert!(diff > 1e-3, "preservation changes the sound ({diff})");
    }

    #[test]
    fn flags_move_and_resizing_keeps_a_minimum() {
        let mut l = logic();
        load(&mut l, sine(44100));
        l.doc(|d| {
            d.cursor = 1000;
            d.view.snap_on = false;
            d.view.freeze = false;
        });
        l.typed(&Prompt::Marker, "m");
        paint(&mut l);
        let id = l.doc(|d| d.sample.cues[0].id);
        let (x, y) = centre(&l, Target::Flag(id));
        l.down(x, y, Btn::Left, Mods::default(), false);
        l.moved(x + 100.0, y, Mods::default());
        l.moved(x + 200.0, y, Mods::default());
        l.up(Btn::Left);
        let per = 44100.0 / l.layout().lane().w as f64;
        let pos = l.doc(|d| d.sample.cues[0].pos);
        assert!((pos as f64 - (1000.0 + 200.0 * per)).abs() < 2.0, "{pos}");
        // One undo step for the whole drag.
        l.key(Key::Char('Z'), Mods { ctrl: true, ..Mods::default() });
        assert_eq!(l.doc(|d| d.sample.cues[0].pos), 1000);
        let (x, y) = centre(&l, Target::Grip);
        l.down(x, y, Btn::Left, Mods::default(), false);
        let acts = l.moved(x - 500.0, y + 40.0, Mods::default());
        assert_eq!(acts[0], Act::Resize(view::MIN_W, view::H + 40));
        l.up(Btn::Left);
        paint(&mut l);
        assert_eq!(l.size(), (view::MIN_W, view::H + 40));
    }

    /// Lemon Studio's frame resizes the editor through the host link: it takes the size, never below its smallest.
    #[test]
    fn the_host_resizes_the_editor() {
        let mut l = logic();
        assert_eq!(l.resize_to(900, 600), vec![Act::Resize(900, 600), Act::Repaint]);
        assert_eq!(l.size(), (900, 600));
        assert!(l.resize_to(900, 600).is_empty(), "unchanged");
        assert_eq!(l.resize_to(10, 10), vec![Act::Resize(view::MIN_W, view::MIN_H), Act::Repaint]);
    }

    #[test]
    fn transport_and_recording_controls() {
        let mut l = logic();
        load(&mut l, sine(1000));
        let (x, y) = centre(&l, Target::Transport(Tr::Record));
        l.down(x, y, Btn::Left, Mods::default(), false);
        assert!(l.shared.armed.load(Ordering::Relaxed));
        l.pick(&MenuReq::When, Cmd::When(state::WHEN_NOW), 0.0, 0.0);
        assert_eq!(l.shared.rec().when, state::WHEN_NOW);
        let (x, y) = centre(&l, Target::Append);
        l.down(x, y, Btn::Left, Mods::default(), false);
        assert!(!l.shared.append.load(Ordering::Relaxed));
        let (x, y) = centre(&l, Target::Transport(Tr::Play));
        l.down(x, y, Btn::Left, Mods::default(), false);
        assert!(l.shared.playing.load(Ordering::Relaxed));
        assert_eq!(l.shared.cmds.lock().unwrap().last(), Some(&Play::Play { from: 0, to: 999, at: 0.0 }));
        assert_eq!(l.menu(&MenuReq::Max).find("1GB"), Some(Cmd::Max(11)));
    }

    #[test]
    fn files_round_trip() {
        let dir = std::env::temp_dir().join(format!("tesla-editor-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut l = logic();
        load(&mut l, sine(500));
        l.typed(&Prompt::Marker, "m");
        let p = dir.join("a").to_string_lossy().into_owned();
        l.save(&p, Depth::Int24);
        assert!(l.hint.starts_with("Saved a.wav"));
        l.pick(&MenuReq::Main(MenuId::File), Cmd::New, 0.0, 0.0);
        assert_eq!(l.doc(|d| d.frames()), 0);
        l.open(&format!("{p}.wav"));
        assert_eq!(l.doc(|d| (d.frames(), d.sample.cues.len())), (500, 1));
        assert_eq!(l.save_name(), "a.wav");
        l.open(&dir.join("missing.wav").to_string_lossy());
        assert!(l.hint.starts_with("Could not load the file"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn rms(x: &[f32]) -> f64 {
        (x.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / x.len().max(1) as f64).sqrt()
    }

    fn curve_box(l: &Logic) -> Rect {
        l.hits.iter().rev().find(|h| h.t == Target::Amp(AmpCtl::Curve)).expect("the curve box").r
    }

    /// Equalize (the tool grid's EQ button, Ctrl+Q): a point added over 1 kHz near the top boosts a 1 kHz tone; a
    /// right click deletes a point.
    #[test]
    fn equalize_draws_a_curve_and_boosts() {
        let mut l = logic();
        let rate = 44100;
        let tone: Vec<f32> = (0..rate as usize).map(|i| (std::f64::consts::TAU * 1000.0 * i as f64 / rate as f64).sin() as f32 * 0.05).collect();
        load(&mut l, Sample::from_channels(rate, vec![tone.clone()], Format::Float32));
        let (x, y) = centre(&l, Target::Tool(5));
        l.down(x, y, Btn::Left, Mods::default(), false);
        l.up(Btn::Left);
        assert!(l.view.amp.is_some_and(|a| a.eq.is_some() && a.curve.points().len() == 3), "the Equalize panel is open, flat");
        // The tone shows in the backdrop at 1 kHz.
        let at = (spectral::eq_x(1000.0) * l.view.backdrop.len() as f64) as usize;
        assert!(l.view.backdrop[at] > l.view.backdrop[at / 2] + 0.3);
        paint(&mut l);
        let bx = curve_box(&l);
        let px = bx.x + spectral::eq_x(1000.0) as f32 * bx.w;
        l.down(px, bx.y + 40.0, Btn::Left, Mods::default(), false);
        l.up(Btn::Left);
        assert_eq!(l.view.amp.map(|a| a.curve.points().len()), Some(4));
        l.view.mouse = Some((px, bx.y + 40.0));
        assert!(l.curve_hint().starts_with("1.00 kHz: +"), "{}", l.curve_hint());
        l.down(px, bx.y + 40.0, Btn::Right, Mods::default(), false);
        assert_eq!(l.view.amp.map(|a| a.curve.points().len()), Some(3), "a right click deletes the point");
        // Again, dragged to the top: +24 dB there.
        l.down(px, bx.y + 40.0, Btn::Left, Mods::default(), false);
        l.moved(px, bx.y - 30.0, Mods::default());
        l.up(Btn::Left);
        assert!(l.view.amp.is_some_and(|a| a.curve.points()[2].v == 1.0), "the new point is after the middle one");
        let (x, y) = centre(&l, Target::Amp(AmpCtl::Accept));
        l.down(x, y, Btn::Left, Mods::default(), false);
        assert!(l.view.amp.is_none());
        assert_eq!(l.doc(|d| d.history()[0].clone()), "Equalize");
        let gain = l.doc(|d| 20.0 * (rms(&d.sample.ch[0][4000..40000]) / rms(&tone[4000..40000])).log10());
        assert!((gain - 24.0).abs() < 1.0, "{gain} dB");
    }

    /// Blur (Ctrl+B): a click smeared after itself with the Offset at +100%.
    #[test]
    fn blur_smears_after_the_sound() {
        let mut l = logic();
        let mut x = vec![0f32; 44100];
        x[10000] = 1.0;
        load(&mut l, Sample::from_channels(44100, vec![x], Format::Float32));
        l.key(Key::Char('B'), Mods { ctrl: true, ..Mods::default() });
        assert!(l.view.amp.is_some_and(|a| a.blur.is_some() && a.curve.points().len() == 5), "the Blur panel is open with its bell");
        paint(&mut l);
        l.set_amp_value(AmpCtl::Offset, 1.0);
        assert_eq!(l.amp_hint(AmpCtl::Offset), "Offset: 100% after the sound");
        assert_eq!(l.amp_hint(AmpCtl::Amount), "Amount: the blur spreads over 141 ms");
        let (x, y) = centre(&l, Target::Amp(AmpCtl::Accept));
        l.down(x, y, Btn::Left, Mods::default(), false);
        assert_eq!(l.doc(|d| d.history()[0].clone()), "Blur");
        l.doc(|d| {
            let e = |a: usize, b: usize| d.sample.ch[0][a..b].iter().map(|v| v * v).sum::<f32>();
            assert!(e(12205, 16615) > 10.0 * e(3385, 7795));
        });
    }

    /// Clean up (Ctrl+U): greyed Preview and Accept until a noise profile is acquired (Alt+U), then the noise goes and
    /// the tone stays.
    #[test]
    fn clean_up_needs_a_profile_then_denoises() {
        let mut l = logic();
        let rate = 44100;
        let mut r = tools::Rng::new(5);
        let hiss: Vec<f32> = (0..2 * rate as usize).map(|_| r.gauss() as f32 * 0.05).collect();
        let mut x = hiss.clone();
        for (i, v) in x[rate as usize..].iter_mut().enumerate() {
            *v += (std::f64::consts::TAU * 440.0 * i as f64 / rate as f64).sin() as f32 * 0.4;
        }
        load(&mut l, Sample::from_channels(rate, vec![x.clone(), x.clone()], Format::Float32));
        let (bx, by) = centre(&l, Target::Tool(14));
        l.down(bx, by, Btn::Left, Mods::default(), false);
        l.up(Btn::Left);
        assert!(l.view.amp.is_some_and(|a| a.clean.is_some() && !a.ready(false)));
        paint(&mut l);
        let (x0, y0) = centre(&l, Target::Amp(AmpCtl::Accept));
        l.down(x0, y0, Btn::Left, Mods::default(), false);
        assert!(l.view.amp.is_some(), "Accept does nothing without a profile");
        assert!(l.hint.starts_with("Clean up needs a noise profile"));
        // ACQUIRE in the panel takes the selection's noise.
        l.doc(|d| d.select(0, rate as usize - 1));
        let (ax, ay) = centre(&l, Target::Amp(AmpCtl::Acquire));
        l.down(ax, ay, Btn::Left, Mods::default(), false);
        assert!(l.hint.starts_with("Noise profile acquired"), "{}", l.hint);
        assert!(l.view.profile && !l.view.backdrop.is_empty());
        paint(&mut l);
        l.doc(|d| d.sel = None);
        l.down(x0, y0, Btn::Left, Mods::default(), false);
        assert!(l.view.amp.is_none());
        assert_eq!(l.doc(|d| d.history()[0].clone()), "Clean up");
        l.doc(|d| {
            let n = rate as usize;
            for c in 0..2 {
                let y = &d.sample.ch[c];
                let down = 20.0 * (rms(&y[4000..n - 4000]) / rms(&hiss[4000..n - 4000])).log10();
                assert!(down < -18.0, "the noise alone: {down} dB");
                let tone: Vec<f32> = (n + 4000..2 * n - 4000).map(|i| x[i] - hiss[i]).collect();
                let kept = 20.0 * (rms(&y[n + 4000..2 * n - 4000]) / rms(&tone)).log10();
                assert!(kept.abs() < 1.0, "the tone: {kept} dB");
            }
        });
        // The right-click of the tool button acquires too; the declipper alone makes the panel ready.
        l.noise = None;
        l.view.profile = false;
        l.doc(|d| d.select(0, 999));
        l.down(bx, by, Btn::Right, Mods::default(), false);
        assert!(l.noise.is_some());
        l.noise = None;
        l.view.profile = false;
        l.key(Key::Char('U'), Mods { ctrl: true, ..Mods::default() });
        paint(&mut l);
        let (dx, dy) = centre(&l, Target::Amp(AmpCtl::Declip));
        l.down(dx, dy, Btn::Left, Mods::default(), false);
        assert!(l.view.amp.is_some_and(|a| a.ready(false)));
    }

    /// Convolution reverb (the tool grid's reverb button, Ctrl+R): the default impulse or the clipboard, with the tail.
    #[test]
    fn convolution_reverb() {
        let mut l = logic();
        let rate = 44100;
        let mut s = Sample::from_channels(rate, vec![vec![0.0; 44100], vec![0.0; 44100]], Format::Float32);
        s.ch[0][1000] = 1.0;
        s.add_cue("after", 30000, None);
        load(&mut l, s);
        l.doc(|d| d.select(0, 9999));
        let (x, y) = centre(&l, Target::Tool(4));
        l.down(x, y, Btn::Left, Mods::default(), false);
        l.up(Btn::Left);
        assert!(l.view.amp.is_some_and(|a| a.reverb.is_some()), "the reverb panel is open");
        paint(&mut l);
        assert_eq!(l.amp_hint(AmpCtl::Wet), "Wet: x0.50");
        // Decay at its lowest: a 0.2 s impulse; Add tail grows the selection by its length less one frame.
        l.set_amp_value(AmpCtl::Decay, -1.0);
        assert!(l.amp_hint(AmpCtl::Decay).starts_with("Decay of the default impulse: 0.20 s"));
        // No clipboard yet: the Clipboard switch stays off.
        let (x, y) = centre(&l, Target::Amp(AmpCtl::ClipImpulse));
        l.down(x, y, Btn::Left, Mods::default(), false);
        assert!(l.view.amp.is_some_and(|a| a.reverb.is_some_and(|r| !r.clip)));
        let (x, y) = centre(&l, Target::Amp(AmpCtl::Accept));
        l.down(x, y, Btn::Left, Mods::default(), false);
        assert!(l.view.amp.is_none());
        let tail = (0.2 * rate as f64).round() as usize - 1;
        assert_eq!(l.doc(|d| (d.frames(), d.sel, d.sample.cues[0].pos)), (44100 + tail, Some((0, 9999 + tail)), 30000 + tail));
        assert_eq!(l.doc(|d| d.history()[0].clone()), "Convolution reverb");
        let imp = tools::default_impulse(rate, 2, 0.2);
        l.doc(|d| {
            assert!((d.sample.ch[0][1000] - (1.0 + 0.5 * imp[0][0])).abs() < 1e-5);
            assert!((d.sample.ch[0][5000] - 0.5 * imp[0][4000]).abs() < 1e-5);
            assert!(d.sample.ch[1].iter().all(|v| *v == 0.0));
        });
        l.doc(|d| d.undo());
        // The clipboard as the impulse: a copied two-frame echo, without the tail.
        l.doc(|d| d.sel = None);
        l.clip = Some(Sample::from_channels(rate, vec![vec![0.0, 0.0, 1.0]], Format::Float32));
        l.key(Key::Char('R'), Mods { ctrl: true, ..Mods::default() });
        paint(&mut l);
        for c in [AmpCtl::ClipImpulse, AmpCtl::Tail] {
            let (x, y) = centre(&l, Target::Amp(c));
            l.down(x, y, Btn::Left, Mods::default(), false);
        }
        l.set_amp_value(AmpCtl::Dry, -1.0);
        l.set_amp_value(AmpCtl::Wet, 0.0);
        let (x, y) = centre(&l, Target::Amp(AmpCtl::Accept));
        l.down(x, y, Btn::Left, Mods::default(), false);
        l.doc(|d| {
            assert_eq!(d.frames(), 44100);
            assert_eq!((d.sample.ch[0][1000], d.sample.ch[0][1002]), (0.0, 1.0));
        });
    }

    #[test]
    fn envelope_modes_edit_points_and_mix_down() {
        let mut l = logic();
        let n = 44100;
        load(&mut l, Sample::from_channels(44100, vec![vec![0.5; n], vec![0.5; n]], Format::Float32));
        // The bottom bar's ramp: the volume envelope.
        let (x, y) = centre(&l, Target::Bottom(Bt::Mode(2)));
        l.down(x, y, Btn::Left, Mods::default(), false);
        l.up(Btn::Left);
        assert_eq!(l.doc(|d| d.env_shown()), Some(env::VOLUME));
        paint(&mut l);
        let lane = l.layout().lane();
        let (start, per) = l.doc(|d| view::span(d, n, lane.w));
        let fx = |f: f64| view::frame_x(lane, start, per, f);
        // Drag the middle point to the bottom (past it, then back to it): one undo step.
        let (mx, my) = (fx(22050.0), view::env_y(lane, 0.5));
        l.down(mx, my, Btn::Left, Mods::default(), false);
        l.moved(mx, lane.bottom() + 20.0, Mods::default());
        l.moved(mx, lane.bottom(), Mods::default());
        assert_eq!(l.hint, "Volume: -inf dB");
        l.up(Btn::Left);
        let p = l.doc(|d| d.env.points(env::VOLUME, n));
        assert_eq!((p[1].pos, p[1].v), (22050.0, 0.0));
        assert_eq!(l.doc(|d| d.history()), vec!["Move envelope point", "Load"]);
        // A click on the line adds a point; a right click on it deletes it; the ends can't be deleted.
        let (ax, ay) = (fx(11025.0), view::env_y(lane, 0.25));
        l.down(ax, ay, Btn::Left, Mods::default(), false);
        l.up(Btn::Left);
        assert_eq!(l.doc(|d| d.env.points(env::VOLUME, n).len()), 4);
        l.down(ax, ay, Btn::Right, Mods::default(), false);
        assert_eq!(l.doc(|d| d.env.points(env::VOLUME, n).len()), 3);
        l.down(fx(0.0), view::env_y(lane, 0.5), Btn::Right, Mods::default(), false);
        assert_eq!(l.doc(|d| d.env.points(env::VOLUME, n).len()), 3);
        assert!(l.hint.starts_with("The first and last points stay"));
        // Away from the line a drag still selects.
        l.down(lane.x + 10.0, view::env_y(lane, 0.95), Btn::Left, Mods::default(), false);
        l.moved(lane.x + 100.0, view::env_y(lane, 0.95), Mods::default());
        l.up(Btn::Left);
        assert!(l.doc(|d| d.sel.is_some()));
        assert!(l.hover_hint(fx(22050.0), my).starts_with("Volume envelope: -inf dB"));
        // Ctrl+E mixes the envelopes down and flattens them; Ctrl+Z brings both back.
        l.key(Key::Char('E'), Mods { ctrl: true, ..Mods::default() });
        assert_eq!(l.doc(|d| d.sample.ch[0][22050]), 0.0);
        assert_eq!(l.doc(|d| d.sample.ch[1][0]), 0.5);
        assert!(l.doc(|d| d.env.amp_flat()));
        assert_eq!(l.doc(|d| d.history().first().cloned()), Some("Mixdown amp envelopes".into()));
        l.key(Key::Char('Z'), Mods { ctrl: true, ..Mods::default() });
        assert_eq!(l.doc(|d| d.sample.ch[0][22050]), 0.5);
        assert!(!l.doc(|d| d.env.amp_flat()));
        // Alt+E cancels them; the waveform button goes back to editing the waveform.
        l.key(Key::Char('E'), Mods { alt: true, ..Mods::default() });
        assert_eq!(l.doc(|d| d.env.clone()), env::Envs::default());
        let (x, y) = centre(&l, Target::Bottom(Bt::Mode(0)));
        l.down(x, y, Btn::Left, Mods::default(), false);
        l.up(Btn::Left);
        assert_eq!(l.doc(|d| d.env_shown()), None);
    }

    /// `TESLA_PNG=<dir> cargo test -p tesla -- --ignored`: the editor in a few states, as PNGs.
    #[test]
    #[ignore]
    fn render_editor() {
        let Some(dir) = std::env::var_os("TESLA_PNG") else { return };
        let save = |l: &mut Logic, name: &str, scale: f32| {
            let (w, h) = l.size();
            let (w, h) = ((w as f32 * scale) as u32, (h as f32 * scale) as u32);
            let mut c = Canvas::new(w, h, scale).unwrap();
            l.paint(&mut c);
            let mut px = vec![0u32; (w * h) as usize];
            c.copy_to_xrgb(&mut px);
            let mut pm = tiny_skia::Pixmap::new(w, h).unwrap();
            for (o, p) in pm.data_mut().chunks_mut(4).zip(&px) {
                o.copy_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, *p as u8, 255]);
            }
            pm.save_png(std::path::Path::new(&dir).join(format!("{name}.png"))).unwrap();
        };
        let mut l = logic();
        save(&mut l, "tesla_empty", 1.0);
        // TESLA_WAV=<file>: that file as loaded, at 2x (to set beside a capture of Edison with the same file).
        if let Some(wav) = std::env::var_os("TESLA_WAV") {
            let mut r = logic();
            r.open(&wav.to_string_lossy());
            r.hosted = true;
            save(&mut r, "tesla_file_x2", 2.0);
        }
        let n = 88200;
        let l_ch: Vec<f32> = (0..n).map(|i| (i as f32 * 0.03).sin() * 0.8 * (-(i as f32) / 30000.0).exp()).collect();
        let r_ch: Vec<f32> = (0..n).map(|i| (i as f32 * 0.071).sin() * 0.4 * (1.0 - i as f32 / n as f32)).collect();
        let mut s = Sample::from_channels(44100, vec![l_ch, r_ch], Format::Float32);
        s.add_cue("kick", 0, None);
        s.add_cue("verse", 20000, Some(30000));
        s.looped = Some((0, n - 1));
        s.tempo = 120.0;
        load(&mut l, s);
        l.doc(|d| d.select(30000, 52000));
        l.hint = "0:680 | -3.10dB 0.70".into();
        save(&mut l, "tesla_sample", 1.0);
        save(&mut l, "tesla_sample_x2", 2.0);
        l.doc(|d| d.view.spectrum = true);
        save(&mut l, "tesla_spectrum", 1.0);
        l.doc(|d| d.view.spectrum = false);
        l.doc(|d| {
            d.view.mode = 2;
            let e = d.edit_env("Draw");
            e.move_point(crate::env::VOLUME, n, 1, 30000.0, 0.85);
            e.insert(crate::env::VOLUME, n, 60000.0, 0.2);
        });
        save(&mut l, "tesla_envelope", 1.0);
        l.doc(|d| d.view.mode = 0);
        l.pick(&MenuReq::Main(MenuId::Tools), Cmd::Tool(ToolCmd::Reverb), 0.0, 0.0);
        save(&mut l, "tesla_reverb", 1.0);
        l.view.amp = None;
        l.pick(&MenuReq::Main(MenuId::Tools), Cmd::Tool(ToolCmd::Equalize), 0.0, 0.0);
        if let Some(a) = l.view.amp.as_mut() {
            a.curve.insert(0.2, 0.75);
            a.curve.insert(0.8, 0.3);
        }
        save(&mut l, "tesla_equalize", 1.0);
        l.view.amp = None;
        l.pick(&MenuReq::Main(MenuId::Tools), Cmd::Tool(ToolCmd::Blur), 0.0, 0.0);
        save(&mut l, "tesla_blur", 1.0);
        l.view.amp = None;
        l.doc(|d| d.select(0, 20000));
        l.pick(&MenuReq::Main(MenuId::Tools), Cmd::Tool(ToolCmd::AcquireNoise), 0.0, 0.0);
        l.doc(|d| d.sel = None);
        l.pick(&MenuReq::Main(MenuId::Tools), Cmd::Tool(ToolCmd::CleanUp), 0.0, 0.0);
        save(&mut l, "tesla_cleanup", 1.0);
        l.view.amp = None;
        l.pick(&MenuReq::Main(MenuId::Tools), Cmd::Tool(ToolCmd::Amp), 0.0, 0.0);
        save(&mut l, "tesla_amp", 1.0);
        l.view.amp = None;
        l.pick(&MenuReq::Main(MenuId::Tools), Cmd::Tool(ToolCmd::Stretch), 0.0, 0.0);
        l.set_amp_value(AmpCtl::Pitch, 0.125);
        l.set_amp_value(AmpCtl::Time, 0.25);
        l.view.amp.as_mut().unwrap().stretch.as_mut().unwrap().preserve = true;
        save(&mut l, "tesla_stretch", 1.0);
        save(&mut l, "tesla_stretch_x2", 2.0);
        l.view.amp = None;
        l.shared.armed.store(true, Ordering::Relaxed);
        l.shared.size.0.store(1000, Ordering::Relaxed);
        l.shared.size.1.store(600, Ordering::Relaxed);
        save(&mut l, "tesla_big_armed", 1.0);
    }
}
