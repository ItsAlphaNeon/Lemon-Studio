// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The startup splash: General ▸ Miscellaneous "Show startup splash screen" (`prefs::SHOW_SPLASH`). With the check on,
//! a borderless window centred on the primary monitor, above every other window, shows the full-colour logo as a
//! loading gauge from launch until startup is done: the lemon fills round with startup's progress while light rakes
//! round the record's grooves, and a status line under the disc names the step. Once the main window shows and the
//! opened project has loaded (and the splash has shown for a moment, however quick that was), the lemon tops up, the
//! light fades, the mark pops and the splash fades away. With the check off there is none. A program setting, read
//! once at start (`main`); `--screenshot`, the helper processes and tests never open it.
//!
//! The window has no background of its own: only the disc, its soft (Gaussian) shadow and the status line show, over
//! whatever is behind, and clicks go through it. It is animated on a thread of its own (on macOS in a process of its
//! own, `mac`), so it keeps moving while the UI thread loads the project and the plugins. Windows, macOS and Linux (X11,
//! XWayland under Wayland, `x11`); elsewhere `open` shows nothing.
// spec: ui/settings-choices.md "Miscellaneous: naming, detached plugins and startup"
#![cfg_attr(not(any(windows, target_os = "macos", target_os = "linux")), allow(dead_code))]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use os_ui::{Canvas, Color, Font};
use tiny_skia::{
    FillRule, FilterQuality, GradientStop, LinearGradient, Mask, Paint, Path, PathBuilder, Pixmap, PixmapPaint, Point, PremultipliedColorU8, SpreadMode, Stroke,
    Transform,
};

use crate::logo;

/// The splash window in logical px: the disc, its shadow and the status line under it.
pub const SIZE: (f32, f32) = (500.0, 500.0);
/// The disc in the window: its centre and diameter (logical px).
const DISC: (f32, f32, f32) = (250.0, 236.0, 360.0);
/// The disc's shadow: how far down it falls, its Gaussian blur (sigma) in logical px, and its opacity under the disc.
const SHADOW: (f32, f32, f32) = (10.0, 18.0, 0.45);
/// The status line: a pill this far under the disc and this high (logical px), never wider than the disc, with this
/// much room either side of its text.
const CAPTION_GAP: f32 = 22.0;
const CAPTION_H: f32 = 26.0;
const CAPTION_PAD: f32 = 14.0;
const CAPTION_FONT: Font = Font::new(15);
const CAPTION_BG: u32 = 0x262626;
const CAPTION_INK: u32 = 0xcfcfcf;

/// Where startup's steps start on the gauge (0..1). The opened project's loading fills it from `LOADING` to `LOADED`.
pub const SETTINGS: f32 = 0.06;
pub const PROJECT: f32 = 0.2;
pub const MIDI: f32 = 0.32;
pub const WINDOW: f32 = 0.4;
pub const PLUGINS: f32 = 0.5;
pub const LOADING: f32 = 0.62;
pub const LOADED: f32 = 0.96;

// The logo in its own units (`logo.rs`): the disc's centre and outer radius, fitted to its edge (the label is
// concentric).
const C: (f32, f32) = (490.4, 488.1);
const R: f32 = 476.5;
// The full-colour logo's fills (`assets/logo/logo-colored-lemon.svg`): the label and the rind, the slices from the
// lower right round to the upper left, the record's edge and its gradient (lower right to upper left).
const LEMON: u32 = 0xffe066;
const SLICE_FILLS: [u32; 4] = [0xffe066, 0xffe88a, 0xffeda3, 0xfff1b8];
const RECORD_EDGE: u32 = 0x2d2d2d;
const RECORD_FROM: (f32, f32) = (857.748, 789.317);
const RECORD_TO: (f32, f32) = (147.545, 158.696);
const RECORD_STOPS: [(f32, u32); 3] = [(0.0, 0x2d2d2d), (0.387, 0x171717), (0.8914, 0x000000)];
/// The empty part of the gauge: each fill this far toward the workspace grey.
const EMPTY: (u32, f32) = (0x262626, 0.8);
/// The pie sector the gauge fills, clockwise from the rind's lower end (degrees clockwise from +x).
const GAUGE_FROM: f32 = 36.0;
const GAUGE_SWEEP: f32 = 192.0;
/// The groove light: two opposite wedges of this half-angle and reach (degrees, logo units), turning at this speed
/// (degrees a second), lighting the grooves (rings from 190 units out, 8.4 apart, 1.7 wide) and a faint glow.
const WEDGE: f32 = 18.0;
const WEDGE_R: f32 = 520.0;
const TURN: f32 = 165.0;
const GROOVES: (f32, f32, usize) = (190.0, 8.4, 34);
const GROOVE_W: f32 = 1.7;
const GROOVE_ALPHA: f32 = 0.6;
const GLOW_ALPHA: f32 = 0.13;

/// Seconds: the intro (the light fades in), the fade in, how quickly the gauge follows startup, and the finish: the
/// gauge tops up, the light fades out, the mark pops, the window fades away.
const INTRO: f32 = 0.5;
const FADE_IN: f32 = 0.3;
const FOLLOW: f32 = 0.18;
const TOP_UP: f32 = 0.4;
const LIGHT_OUT: f32 = 0.6;
const POP: (f32, f32) = (0.4, 0.8);
const FADE_OUT: (f32, f32) = (1.0, 1.3);
/// Seconds from the splash opening: a quicker startup still sees the gauge fill until the first; one taking longer
/// than the second does not keep the splash up.
const EARLIEST: f32 = 1.5;
const LONGEST: f32 = 120.0;

/// Whether this start shows the splash.
pub fn wanted() -> bool {
    crate::prefs::check(&crate::prefs::SHOW_SPLASH)
}

/// What the UI thread tells the splash.
#[derive(Default)]
struct Feed {
    /// Startup's progress (0..=1, never going back) and the step it is on.
    progress: f32,
    step: String,
    /// Startup is done.
    done: bool,
}

/// Between the UI thread and the splash's.
#[derive(Default)]
struct Shared {
    feed: Mutex<Feed>,
    /// The splash finished and faded away (on macOS its process ends instead).
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    gone: AtomicBool,
    /// The splash was dropped: the window closes.
    closed: AtomicBool,
}

impl Shared {
    /// Startup's progress, step and whether it is done.
    fn feed(&self) -> (f32, String, bool) {
        let f = self.feed.lock().unwrap_or_else(|e| e.into_inner());
        (f.progress, f.step.clone(), f.done)
    }
}

/// The splash while it shows: dropping it closes the window.
pub struct Splash {
    shared: Arc<Shared>,
    #[cfg(any(windows, target_os = "macos", target_os = "linux"))]
    window: platform::Window,
}

impl Splash {
    /// Startup has reached `progress` (0..1) and is on the step `what`.
    pub fn step(&self, progress: f32, what: &str) {
        let mut f = self.shared.feed.lock().unwrap_or_else(|e| e.into_inner());
        let p = f.progress.max(progress.clamp(0.0, 1.0));
        if p == f.progress && f.step == what {
            return;
        }
        f.progress = p;
        if f.step != what {
            f.step = what.to_string();
        }
        #[cfg(target_os = "macos")]
        self.window.tell(&f);
    }

    /// Startup is done: the gauge tops up and the splash goes.
    pub fn finish(&self) {
        let mut f = self.shared.feed.lock().unwrap_or_else(|e| e.into_inner());
        if !f.done {
            f.done = true;
            #[cfg(target_os = "macos")]
            self.window.tell(&f);
        }
    }

    /// Whether the splash has finished and faded away: it can be dropped.
    pub fn gone(&self) -> bool {
        #[cfg(target_os = "macos")]
        return self.window.gone();
        #[cfg(not(target_os = "macos"))]
        self.shared.gone.load(Ordering::Acquire)
    }
}

/// `Splash::step`, when there is a splash.
pub fn step(splash: &Option<Splash>, progress: f32, what: &str) {
    if let Some(s) = splash {
        s.step(progress, what);
    }
}

/// Opens the splash when its check is on (`wanted`). None when it is off, or where there is no splash yet.
pub fn open() -> Option<Splash> {
    if !wanted() {
        return None;
    }
    #[cfg(any(windows, target_os = "macos", target_os = "linux"))]
    {
        let shared = Arc::new(Shared::default());
        let window = platform::open(shared.clone())?;
        Some(Splash { shared, window })
    }
    #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
    None
}

impl Drop for Splash {
    fn drop(&mut self) {
        self.shared.closed.store(true, Ordering::SeqCst);
        #[cfg(any(windows, target_os = "macos", target_os = "linux"))]
        self.window.close();
    }
}

#[cfg(windows)]
use win as platform;

#[cfg(target_os = "macos")]
use mac as platform;

#[cfg(target_os = "linux")]
use x11 as platform;

#[cfg(target_os = "macos")]
pub use mac::{child_main, ARG};

// ---------------------------------------------------------------------------------------------------------------
// The animation.

/// The splash's animation: what it shows at each moment, from what startup reports.
#[derive(Default)]
struct Anim {
    /// The progress shown (following startup's), and when the last frame was.
    shown: f32,
    last: f32,
    /// Finishing: when it began, and the progress shown then.
    done: Option<(f32, f32)>,
}

/// One frame of the splash.
#[derive(Clone, Debug, PartialEq)]
struct Look {
    /// The window's opacity: fading in at the start, away at the end.
    alpha: f32,
    /// The disc's scale (its shadow's too): growing in at the start, popping at the end.
    scale: f32,
    /// How far round the lemon is filled, 0..=1.
    progress: f32,
    /// The groove light's opacity, and where its wedges point (degrees clockwise).
    light: f32,
    angle: f32,
    /// The status line.
    caption: String,
}

impl Anim {
    /// The splash `t` seconds after it opened, startup being at `progress` on the step `what` (and `done`); `None`
    /// once it has finished and faded away.
    fn frame(&mut self, t: f32, progress: f32, what: &str, done: bool) -> Option<Look> {
        let dt = (t - self.last).max(0.0);
        self.last = t;
        if self.done.is_none() && (done || t >= LONGEST) && t >= EARLIEST {
            self.done = Some((t, self.shown));
        }
        let k = ease_out(seg(t, 0.0, FADE_IN));
        let light = ease_out(seg(t, 0.0, INTRO));
        let angle = 20.0 + TURN * t;
        let status = |p: f32| match what {
            "" => format!("{:.0}%", p * 100.0),
            _ => format!("{what} · {:.0}%", p * 100.0),
        };
        let Some((t0, from)) = self.done else {
            // The gauge eases toward startup's progress (all the way once it is done), never going back.
            let to = if done { 1.0 } else { progress.clamp(0.0, 1.0) };
            self.shown += (to - self.shown).max(0.0) * (1.0 - (-dt / FOLLOW).exp());
            return Some(Look { alpha: k, scale: lerp(0.94, 1.0, k), progress: self.shown, light, angle, caption: status(self.shown) });
        };
        let d = t - t0;
        if d >= FADE_OUT.1 {
            return None;
        }
        let progress = lerp(from, 1.0, ease_out(seg(d, 0.0, TOP_UP)));
        Some(Look {
            alpha: k * (1.0 - ease_in_out(seg(d, FADE_OUT.0, FADE_OUT.1))),
            scale: lerp(0.94, 1.0, k) * (1.0 + 0.03 * bump(seg(d, POP.0, POP.1))),
            progress,
            light: light * (1.0 - ease_in_out(seg(d, 0.0, LIGHT_OUT))),
            angle,
            caption: if d >= TOP_UP { "Ready".into() } else { status(progress) },
        })
    }
}

/// How far `t` is from `a` to `b`, 0..=1.
fn seg(t: f32, a: f32, b: f32) -> f32 {
    ((t - a) / (b - a)).clamp(0.0, 1.0)
}

/// From `a` (at 0) to `b` (at 1), exactly at either end.
fn lerp(a: f32, b: f32, k: f32) -> f32 {
    a * (1.0 - k) + b * k
}

fn ease_out(k: f32) -> f32 {
    1.0 - (1.0 - k).powi(3)
}

fn ease_in_out(k: f32) -> f32 {
    if k < 0.5 { 4.0 * k * k * k } else { 1.0 - (2.0 - 2.0 * k).powi(3) / 2.0 }
}

/// Up and back down over 0..1.
fn bump(k: f32) -> f32 {
    if k <= 0.0 || k >= 1.0 { 0.0 } else { (std::f32::consts::PI * k).sin() }
}

// ---------------------------------------------------------------------------------------------------------------
// The picture.

/// The window's size in device px at `scale` (device px per logical one).
fn device(scale: f32) -> (u32, u32) {
    ((SIZE.0 * scale).round().max(1.0) as u32, (SIZE.1 * scale).round().max(1.0) as u32)
}

/// The logo's paths in its own units, and the shapes the groove light is made of. Outlines are stroked once here, so
/// a frame only fills.
struct Parts {
    label: Path,
    shimmer: Path,
    record: Path,
    record_edge: Path,
    center: Path,
    rind: Path,
    rind_edge: Path,
    slices: [Path; 4],
}

impl Parts {
    fn new() -> Option<Parts> {
        let p = |d: &str| os_ui::path_data(d, 0.0, 0.0, 1.0);
        // Outlines `w` logo units wide, flattened finely enough for the largest splash.
        let outline = |path: &Path, w: f32| path.stroke(&Stroke { width: w, ..Stroke::default() }, 2.0);
        let [s1, s2, s3, s4] = logo::SLICES;
        let (record, rind) = (p(logo::RECORD)?, p(logo::RIND)?);
        Some(Parts {
            label: p(logo::LABEL)?,
            shimmer: p(logo::SHIMMER)?,
            record_edge: outline(&record, 1.0)?,
            record,
            center: p(logo::CENTER)?,
            rind_edge: outline(&rind, 1.0)?,
            rind,
            slices: [p(s1)?, p(s2)?, p(s3)?, p(s4)?],
        })
    }
}

/// The groove light's shapes with its wedges pointing at `angle` (degrees clockwise): the two wedges, and the
/// stretches of groove inside them (logo units).
fn light_shapes(angle: f32) -> Option<(Path, Path)> {
    let (mut fans, mut grooves) = (PathBuilder::new(), PathBuilder::new());
    for mid in [angle, angle + 180.0] {
        fan(&mut fans, mid - WEDGE, 2.0 * WEDGE, WEDGE_R);
        for i in 0..GROOVES.2 {
            let r = GROOVES.0 + GROOVES.1 * i as f32;
            band(&mut grooves, mid - WEDGE, 2.0 * WEDGE, r - GROOVE_W / 2.0, r + GROOVE_W / 2.0);
        }
    }
    Some((fans.finish()?, grooves.finish()?))
}

/// Adds a band between radii `r0` and `r1` round the disc's centre: from `from` degrees clockwise, `sweep` degrees
/// round (logo units; 3-degree steps stay within a sixth of a unit of the arc).
fn band(pb: &mut PathBuilder, from: f32, sweep: f32, r0: f32, r1: f32) {
    let steps = (sweep / 3.0).ceil().max(1.0) as usize;
    let at = |i: usize, r: f32| {
        let a = (from + sweep * i as f32 / steps as f32).to_radians();
        (C.0 + r * a.cos(), C.1 + r * a.sin())
    };
    let (x, y) = at(0, r1);
    pb.move_to(x, y);
    for i in 1..=steps {
        let (x, y) = at(i, r1);
        pb.line_to(x, y);
    }
    for i in (0..=steps).rev() {
        let (x, y) = at(i, r0);
        pb.line_to(x, y);
    }
    pb.close();
}

/// Adds a pie sector from the disc's centre: from `from` degrees clockwise, `sweep` degrees round, out to `r` (logo
/// units). Its outer edge lies outside the disc, so straight steps serve for the arc.
fn fan(pb: &mut PathBuilder, from: f32, sweep: f32, r: f32) {
    let steps = (sweep / 6.0).ceil().max(1.0) as usize;
    pb.move_to(C.0, C.1);
    for i in 0..=steps {
        let a = (from + sweep * i as f32 / steps as f32).to_radians();
        pb.line_to(C.0 + r * a.cos(), C.1 + r * a.sin());
    }
    pb.close();
}

/// Draws the splash in device px, premultiplied RGBA on a transparent background.
struct Painter {
    /// Device px per logical one.
    scale: f32,
    frame: Canvas,
    /// The status line, drawn on its own when it changes and laid over each frame, and the text it shows.
    caption: Canvas,
    caption_text: Option<String>,
    shadow: Pixmap,
    mask: Mask,
    /// The record's shape as a mask, and the disc scale it was made at (it changes only as the disc grows in and pops).
    record_mask: Mask,
    record_scale: f32,
    parts: Parts,
}

impl Painter {
    fn new(scale: f32) -> Option<Painter> {
        let (w, h) = device(scale);
        let caption = Canvas::new((DISC.2 * scale).round() as u32, (CAPTION_H * scale).round() as u32, scale).ok()?;
        Some(Painter {
            scale,
            frame: Canvas::new(w, h, scale).ok()?,
            caption,
            caption_text: None,
            shadow: shadow(w, h, scale)?,
            mask: Mask::new(w, h)?,
            record_mask: Mask::new(w, h)?,
            record_scale: f32::NAN,
            parts: Parts::new()?,
        })
    }

    /// The frame for `look`.
    fn paint(&mut self, look: &Look) -> &Pixmap {
        let s = self.scale;
        let (cx, cy) = (DISC.0 * s, DISC.1 * s);
        self.paint_caption(&look.caption);
        let (px, mask, p) = (&mut self.frame.pixmap, &mut self.mask, &self.parts);
        // The shadow first, as it is unless the disc is growing in or popping.
        if look.scale == 1.0 {
            px.data_mut().copy_from_slice(self.shadow.data());
        } else {
            px.fill(tiny_skia::Color::TRANSPARENT);
            let about = Transform::from_translate(cx, cy).pre_scale(look.scale, look.scale).pre_translate(-cx, -cy);
            let smooth = PixmapPaint { quality: FilterQuality::Bilinear, ..PixmapPaint::default() };
            px.draw_pixmap(0, 0, self.shadow.as_ref(), &smooth, about, None);
        }
        // Logo units to device px, the disc's outer edge on the disc.
        let k = DISC.2 * s * look.scale / (2.0 * R);
        let t = Transform::from_translate(cx, cy).pre_scale(k, k).pre_translate(-C.0, -C.1);
        let fill = |px: &mut Pixmap, path: &Path, c: u32, mask: Option<&Mask>| px.fill_path(path, &solid(c), FillRule::Winding, t, mask);
        // The record half: the label and the shimmer seen through its cut-outs, the record, the spindle.
        fill(px, &p.label, LEMON, None);
        fill(px, &p.shimmer, 0xffffff, None);
        px.fill_path(&p.record, &record_paint(), FillRule::Winding, t, None);
        fill(px, &p.record_edge, RECORD_EDGE, None);
        fill(px, &p.center, 0x000000, None);
        // The groove light: its two wedges and the grooves inside them, on the record only, brightest along the
        // wedges' middle.
        if look.light > 0.001 {
            if self.record_scale != look.scale {
                self.record_mask.clear();
                self.record_mask.fill_path(&p.record, FillRule::Winding, true, t);
                self.record_scale = look.scale;
            }
            if let Some((fans, grooves)) = light_shapes(look.angle) {
                let (turn, on) = (Transform::from_rotate_at(look.angle, C.0, C.1), Some(&self.record_mask));
                px.fill_path(&fans, &light_paint(GLOW_ALPHA * look.light, turn), FillRule::Winding, t, on);
                px.fill_path(&grooves, &light_paint(GROOVE_ALPHA * look.light, turn), FillRule::Winding, t, on);
            }
        }
        // The lemon half: empty, then filled round as far as the gauge.
        let lemon = std::iter::once((&p.rind, LEMON)).chain(p.slices.iter().zip(SLICE_FILLS));
        if look.progress < 1.0 {
            for (path, c) in lemon.clone() {
                fill(px, path, mix(c, EMPTY.0, EMPTY.1), None);
            }
        }
        if look.progress > 0.0 {
            let part = look.progress < 0.999;
            if part {
                mask.clear();
                let mut pb = PathBuilder::new();
                fan(&mut pb, GAUGE_FROM, GAUGE_SWEEP * look.progress, WEDGE_R + 80.0);
                if let Some(sector) = pb.finish() {
                    mask.fill_path(&sector, FillRule::Winding, true, t);
                }
            }
            let mask = part.then_some(&*mask);
            for (path, c) in lemon {
                fill(px, path, c, mask);
            }
            // The rind's outline, as in the logo.
            fill(px, &p.rind_edge, LEMON, mask);
        }
        let at = (((DISC.0 - DISC.2 / 2.0) * s).round() as i32, ((DISC.1 + DISC.2 / 2.0 + CAPTION_GAP) * s).round() as i32);
        px.draw_pixmap(at.0, at.1, self.caption.pixmap.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
        &self.frame.pixmap
    }

    /// Draws the status line when it changed: `text` on a pill as wide as it needs, centred.
    fn paint_caption(&mut self, text: &str) {
        if self.caption_text.as_deref() == Some(text) {
            return;
        }
        self.caption_text = Some(text.to_string());
        let (c, s) = (&mut self.caption, self.scale);
        c.pixmap.fill(tiny_skia::Color::TRANSPARENT);
        let font = c.fit_font(CAPTION_FONT, text, DISC.2 - 2.0 * CAPTION_PAD);
        let w = c.text_width(font, text);
        let pill = (w + 2.0 * CAPTION_PAD).min(DISC.2);
        let (x0, r) = ((DISC.2 - pill) / 2.0, CAPTION_H / 2.0);
        let mut pb = PathBuilder::new();
        if let Some(mid) = tiny_skia::Rect::from_xywh((x0 + r) * s, 0.0, (pill - 2.0 * r) * s, CAPTION_H * s) {
            pb.push_rect(mid);
        }
        pb.push_circle((x0 + r) * s, r * s, r * s);
        pb.push_circle((x0 + pill - r) * s, r * s, r * s);
        if let Some(shape) = pb.finish() {
            c.pixmap.fill_path(&shape, &solid(CAPTION_BG), FillRule::Winding, Transform::identity(), None);
        }
        let y = ((CAPTION_H - c.line_height(font)) / 2.0).round();
        c.text(font, ((DISC.2 - w) / 2.0).round(), y, Color::rgb(CAPTION_INK), text);
    }
}

/// The disc's shadow, `w` x `h` device px at `scale`: a black disc under it, blurred with a Gaussian.
fn shadow(w: u32, h: u32, scale: f32) -> Option<Pixmap> {
    let mut px = Pixmap::new(w, h)?;
    let (cx, cy) = (DISC.0 * scale, (DISC.1 + SHADOW.0) * scale);
    let (r, sigma) = (DISC.2 / 2.0 * scale, SHADOW.1 * scale);
    let reach = r + 4.0 * sigma;
    let pixels = px.pixels_mut();
    for y in 0..h {
        for x in 0..w {
            let d = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt();
            if d >= reach {
                continue;
            }
            // Across its edge a disc ten sigmas in radius blurs as a straight edge does.
            let a = SHADOW.2 * 0.5 * erfc((d - r) / (sigma * std::f32::consts::SQRT_2));
            if let Some(c) = PremultipliedColorU8::from_rgba(0, 0, 0, (a * 255.0).round() as u8) {
                pixels[(y * w + x) as usize] = c;
            }
        }
    }
    Some(px)
}

/// The complementary error function (Abramowitz and Stegun 7.1.26, within 1.5e-7).
fn erfc(x: f32) -> f32 {
    let z = x.abs();
    let t = 1.0 / (1.0 + 0.327_591_1 * z);
    let y = t * (0.254_829_6 + t * (-0.284_496_74 + t * (1.421_413_7 + t * (-1.453_152 + t * 1.061_405_4)))) * (-z * z).exp();
    if x >= 0.0 { y } else { 2.0 - y }
}

/// `0xRRGGBB` as a colour with opacity `a`.
fn sk(c: u32, a: f32) -> tiny_skia::Color {
    tiny_skia::Color::from_rgba8((c >> 16) as u8, (c >> 8) as u8, c as u8, (a.clamp(0.0, 1.0) * 255.0).round() as u8)
}

/// An opaque fill of `0xRRGGBB`.
fn solid(c: u32) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(sk(c, 1.0));
    p
}

/// `a` taken `k` of the way to `b` (`0xRRGGBB`).
fn mix(a: u32, b: u32, k: f32) -> u32 {
    let ch = |c: u32, sh: u32| ((c >> sh) & 0xff) as f32;
    [16, 8, 0].iter().fold(0, |acc, &sh| acc | ((lerp(ch(a, sh), ch(b, sh), k).round() as u32) << sh))
}

/// The record's gradient fill.
fn record_paint() -> Paint<'static> {
    let stops = RECORD_STOPS.iter().map(|&(at, c)| GradientStop::new(at, sk(c, 1.0))).collect();
    let mut p = Paint::default();
    if let Some(shader) = LinearGradient::new(Point::from_xy(RECORD_FROM.0, RECORD_FROM.1), Point::from_xy(RECORD_TO.0, RECORD_TO.1), stops, SpreadMode::Pad, Transform::identity()) {
        p.shader = shader;
    }
    p
}

/// The groove light's white: `alpha` along the middle of its wedges, nothing at their sides, turned by `turn`.
fn light_paint(alpha: f32, turn: Transform) -> Paint<'static> {
    let h = WEDGE_R * WEDGE.to_radians().sin();
    let stops = vec![GradientStop::new(0.0, sk(0xffffff, 0.0)), GradientStop::new(0.5, sk(0xffffff, alpha)), GradientStop::new(1.0, sk(0xffffff, 0.0))];
    let mut p = Paint::default();
    if let Some(shader) = LinearGradient::new(Point::from_xy(C.0, C.1 - h), Point::from_xy(C.0, C.1 + h), stops, SpreadMode::Pad, turn) {
        p.shader = shader;
    }
    p
}

// ---------------------------------------------------------------------------------------------------------------
// Windows: a layered window (per-pixel alpha), updated from the splash thread.

#[cfg(windows)]
mod win {
    use std::cell::RefCell;
    use std::sync::atomic::{AtomicIsize, Ordering};
    use std::sync::Arc;
    use std::time::Instant;

    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, SIZE, WPARAM};
    use windows_sys::Win32::Graphics::Gdi::{
        CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetMonitorInfoW, MonitorFromPoint, SelectObject, AC_SRC_ALPHA, AC_SRC_OVER, BITMAPINFO,
        BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION, DIB_RGB_COLORS, HBITMAP, HDC, MONITORINFO, MONITOR_DEFAULTTOPRIMARY,
    };
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, KillTimer, PostMessageW, PostQuitMessage, RegisterClassW, SetTimer,
        ShowWindow, TranslateMessage, UpdateLayeredWindow, MA_NOACTIVATE, MSG, SW_SHOWNOACTIVATE, ULW_ALPHA, WM_CLOSE, WM_DESTROY, WM_MOUSEACTIVATE, WM_TIMER,
        WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
    };

    use super::{device, Anim, Painter, Shared};

    const CLASS: &str = "LemonStudioSplash";
    /// The frame timer and its period (ms).
    const TIMER: usize = 1;
    const FRAME_MS: u32 = 15;

    /// The splash's window, as the splash thread made it (0 until then).
    pub struct Window {
        hwnd: Arc<AtomicIsize>,
    }

    impl Window {
        /// Closes the window. Not made yet: the thread sees `Shared::closed` and does not make it.
        pub fn close(&self) {
            let hwnd = self.hwnd.load(Ordering::SeqCst);
            if hwnd != 0 {
                // SAFETY: a window of the splash thread, or one already gone (the post then fails harmlessly).
                unsafe { PostMessageW(hwnd as HWND, WM_CLOSE, 0, 0) };
            }
        }
    }

    /// The splash thread's drawing: the animation, its painter, and the bitmap the window is updated from.
    struct State {
        shared: Arc<Shared>,
        anim: Anim,
        painter: Painter,
        start: Instant,
        dc: HDC,
        bitmap: HBITMAP,
        bits: *mut u32,
        size: (i32, i32),
    }

    impl Drop for State {
        fn drop(&mut self) {
            // SAFETY: the memory DC and bitmap made for this state, no longer used.
            unsafe {
                DeleteDC(self.dc);
                DeleteObject(self.bitmap);
            }
        }
    }

    thread_local! {
        static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    pub fn open(shared: Arc<Shared>) -> Option<Window> {
        let hwnd = Arc::new(AtomicIsize::new(0));
        let h = hwnd.clone();
        std::thread::Builder::new().name("splash".into()).spawn(move || run(shared, &h)).ok()?;
        Some(Window { hwnd })
    }

    /// The splash thread: makes the window, then animates it until it is closed or has gone.
    fn run(shared: Arc<Shared>, hwnd_out: &AtomicIsize) {
        // SAFETY: plain Win32 calls on this thread's own window and GDI objects; the structs are zeroed C structs.
        unsafe {
            // The primary monitor, in physical px: winit made the process per-monitor DPI aware when the event loop
            // was built, before this.
            let mon = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
            let mut mi: MONITORINFO = std::mem::zeroed();
            mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
            if GetMonitorInfoW(mon, &mut mi) == 0 {
                return;
            }
            let (mut dx, mut dy) = (96, 96);
            if GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy) != 0 {
                dx = 96;
            }
            // The UI scale there: the monitor's, or Main GUI scaling's (spec: ui/ui-scaling.md §1).
            let scale = crate::gui_scale(f64::from(dx) / 96.0) as f32;
            let Some(painter) = Painter::new(scale) else { return };
            let (w, h) = device(scale);
            let (w, h) = (w as i32, h as i32);
            // The bitmap the window is updated from: 32-bit premultiplied BGRA, top row first.
            let dc = CreateCompatibleDC(std::ptr::null_mut());
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
            let mut bits = std::ptr::null_mut();
            let bitmap = CreateDIBSection(dc, &bmi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
            let state = State { shared: shared.clone(), anim: Anim::default(), painter, start: Instant::now(), dc, bitmap, bits: bits.cast(), size: (w, h) };
            if dc.is_null() || bitmap.is_null() || bits.is_null() {
                return;
            }
            SelectObject(dc, bitmap);
            let r = mi.rcMonitor;
            let (x, y) = (r.left + (r.right - r.left - w) / 2, r.top + (r.bottom - r.top - h) / 2);
            let inst = GetModuleHandleW(std::ptr::null());
            let class = wide(CLASS);
            let mut wc: WNDCLASSW = std::mem::zeroed();
            wc.lpfnWndProc = Some(splash_proc);
            wc.hInstance = inst;
            wc.lpszClassName = class.as_ptr();
            RegisterClassW(&wc);
            let title = wide("Lemon Studio");
            // Layered (per-pixel alpha) and transparent to the mouse; above every window that is not topmost too; a
            // tool window takes no taskbar button; never activated, so the main window gets the focus as before.
            let hwnd = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                class.as_ptr(),
                title.as_ptr(),
                WS_POPUP,
                x,
                y,
                w,
                h,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                inst,
                std::ptr::null(),
            );
            if hwnd.is_null() {
                return;
            }
            hwnd_out.store(hwnd as isize, Ordering::SeqCst);
            if shared.closed.load(Ordering::SeqCst) {
                DestroyWindow(hwnd);
                return;
            }
            STATE.with(|s| *s.borrow_mut() = Some(state));
            // The first frame before the window shows.
            frame(hwnd);
            ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            SetTimer(hwnd, TIMER, FRAME_MS, None);
            let mut msg: MSG = std::mem::zeroed();
            while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            STATE.with(|s| s.borrow_mut().take());
        }
    }

    /// Draws the next frame into the window; once the splash has gone, destroys the window.
    ///
    /// # Safety
    /// On the splash thread, for its window.
    unsafe fn frame(hwnd: HWND) {
        let gone = STATE.with(|s| {
            let mut s = s.borrow_mut();
            let Some(st) = s.as_mut() else { return false };
            let (progress, what, done) = st.shared.feed();
            let Some(look) = st.anim.frame(st.start.elapsed().as_secs_f32(), progress, &what, done) else {
                st.shared.gone.store(true, Ordering::Release);
                return true;
            };
            let px = st.painter.paint(&look);
            // SAFETY: the DIB section's pixels, `size` of them, live as long as the state.
            let out = unsafe { std::slice::from_raw_parts_mut(st.bits, (st.size.0 * st.size.1) as usize) };
            for (o, p) in out.iter_mut().zip(px.data().chunks_exact(4)) {
                *o = (u32::from(p[3]) << 24) | (u32::from(p[0]) << 16) | (u32::from(p[1]) << 8) | u32::from(p[2]);
            }
            let blend = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: (look.alpha.clamp(0.0, 1.0) * 255.0).round() as u8,
                AlphaFormat: AC_SRC_ALPHA as u8,
            };
            let size = SIZE { cx: st.size.0, cy: st.size.1 };
            let from = POINT { x: 0, y: 0 };
            // SAFETY: our own layered window and memory DC.
            unsafe { UpdateLayeredWindow(hwnd, std::ptr::null_mut(), std::ptr::null(), &size, st.dc, &from, 0, &blend, ULW_ALPHA) };
            false
        });
        if gone {
            // SAFETY: the splash thread's own window.
            unsafe { DestroyWindow(hwnd) };
        }
    }

    unsafe extern "system" fn splash_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        // SAFETY: called by the system on the splash thread for its own window.
        unsafe {
            match msg {
                WM_TIMER => {
                    frame(hwnd);
                    0
                }
                WM_MOUSEACTIVATE => MA_NOACTIVATE as LRESULT,
                WM_DESTROY => {
                    KillTimer(hwnd, TIMER);
                    PostQuitMessage(0);
                    0
                }
                _ => DefWindowProcW(hwnd, msg, wp, lp),
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Linux: the splash in an X11 window of its own connection (XWayland under Wayland), animated on a thread of its own. A
// 32-bit (ARGB) window, so the compositor blends it over whatever is behind; override-redirect, so the window manager
// neither frames nor places it and keeps it above the managed windows; an empty input shape, so clicks go through.

#[cfg(target_os = "linux")]
mod x11 {
    use std::sync::atomic::Ordering;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use x11rb::connection::{Connection, RequestConnection as _};
    use x11rb::protocol::randr::ConnectionExt as _;
    use x11rb::protocol::shape::{self, ConnectionExt as _};
    use x11rb::protocol::xproto::{
        AtomEnum, ClipOrdering, ColormapAlloc, ConnectionExt as _, CreateGCAux, CreateWindowAux, ImageFormat, ImageOrder, PropMode, VisualClass, WindowClass,
    };
    use x11rb::rust_connection::RustConnection;
    use x11rb::wrapper::ConnectionExt as _;

    use super::{device, Anim, Painter, Shared};

    /// Time between frames.
    const FRAME: Duration = Duration::from_millis(16);

    /// The splash thread (it closes the window once `Shared::closed` is set).
    pub struct Window;

    impl Window {
        /// Nothing to do: the thread sees `Shared::closed` at its next frame and closes the window.
        pub fn close(&self) {}
    }

    pub fn open(shared: Arc<Shared>) -> Option<Window> {
        if !std::env::var_os("DISPLAY").is_some_and(|d| !d.is_empty()) {
            return None;
        }
        std::thread::Builder::new()
            .name("splash".into())
            .spawn(move || {
                if run(&shared).is_none() {
                    // No splash after all (no X server, no 32-bit visual): startup goes on without it.
                    shared.gone.store(true, Ordering::Release);
                }
            })
            .ok()?;
        Some(Window)
    }

    /// Makes the window, then animates it until it is closed or has gone.
    fn run(shared: &Shared) -> Option<()> {
        let (c, screen_num) = RustConnection::connect(None).ok()?;
        let screen = &c.setup().roots[screen_num];
        let root = screen.root;
        // A 32-bit TrueColor visual: per-pixel alpha.
        let visual = screen
            .allowed_depths
            .iter()
            .filter(|d| d.depth == 32)
            .flat_map(|d| d.visuals.iter())
            .find(|v| v.class == VisualClass::TRUE_COLOR)?
            .visual_id;
        // The UI scale: the X resources' `Xft.dpi` / 96, or Main GUI scaling's (spec: ui/ui-scaling.md §1).
        let dpi = x11rb::resource_manager::new_from_default(&c).ok().and_then(|db| db.get_value::<u32>("Xft.dpi", "").ok().flatten());
        let scale = crate::gui_scale(dpi.map_or(1.0, |d| f64::from(d) / 96.0)) as f32;
        let mut painter = Painter::new(scale)?;
        let (w, h) = device(scale);
        // The primary monitor (RandR), else the first, else the whole screen.
        let (mx, my, mw, mh) = c
            .randr_get_monitors(root, true)
            .ok()
            .and_then(|r| r.reply().ok())
            .and_then(|r| r.monitors.iter().find(|m| m.primary).or(r.monitors.first()).map(|m| (m.x, m.y, m.width, m.height)))
            .unwrap_or((0, 0, screen.width_in_pixels, screen.height_in_pixels));
        let x = i32::from(mx) + (i32::from(mw) - w as i32) / 2;
        let y = i32::from(my) + (i32::from(mh) - h as i32) / 2;

        let colormap = c.generate_id().ok()?;
        c.create_colormap(ColormapAlloc::NONE, colormap, root, visual).ok()?;
        let win = c.generate_id().ok()?;
        c.create_window(
            32,
            win,
            root,
            x as i16,
            y as i16,
            w as u16,
            h as u16,
            0,
            WindowClass::INPUT_OUTPUT,
            visual,
            &CreateWindowAux::new().background_pixel(0).border_pixel(0).colormap(colormap).override_redirect(1),
        )
        .ok()?;
        let atom = |name: &[u8]| c.intern_atom(false, name).ok()?.reply().ok().map(|r| r.atom);
        if let (Some(ty), Some(splash)) = (atom(b"_NET_WM_WINDOW_TYPE"), atom(b"_NET_WM_WINDOW_TYPE_SPLASH")) {
            let _ = c.change_property32(PropMode::REPLACE, win, ty, AtomEnum::ATOM, &[splash]);
        }
        let _ = c.change_property8(PropMode::REPLACE, win, AtomEnum::WM_NAME, AtomEnum::STRING, b"Lemon Studio");
        let _ = c.change_property8(PropMode::REPLACE, win, AtomEnum::WM_CLASS, AtomEnum::STRING, b"lemonstudio\0LemonStudio\0");
        // No input region: clicks go through to what is behind.
        let _ = c.shape_rectangles(shape::SO::SET, shape::SK::INPUT, ClipOrdering::UNSORTED, win, 0, 0, &[]);
        let gc = c.generate_id().ok()?;
        c.create_gc(gc, win, &CreateGCAux::new()).ok()?;

        // Rows per PutImage, within the server's request size.
        let stride = w as usize * 4;
        let max = c.maximum_request_bytes().saturating_sub(64);
        let band = (max / stride).clamp(1, h as usize);
        let lsb = c.setup().image_byte_order == ImageOrder::LSB_FIRST;
        let mut out = vec![0u8; stride * h as usize];

        let mut anim = Anim::default();
        let start = Instant::now();
        let mut mapped = false;
        while !shared.closed.load(Ordering::SeqCst) {
            let next = Instant::now() + FRAME;
            let (progress, what, done) = shared.feed();
            let Some(look) = anim.frame(start.elapsed().as_secs_f32(), progress, &what, done) else {
                shared.gone.store(true, Ordering::Release);
                break;
            };
            // Premultiplied RGBA to the server's 32-bit ARGB, the window's opacity multiplied in.
            let k = (look.alpha.clamp(0.0, 1.0) * 256.0) as u32;
            for (o, p) in out.chunks_exact_mut(4).zip(painter.paint(&look).data().chunks_exact(4)) {
                let [r, g, b, a] = [p[0], p[1], p[2], p[3]].map(|v| ((u32::from(v) * k) >> 8) as u8);
                o.copy_from_slice(&if lsb { [b, g, r, a] } else { [a, r, g, b] });
            }
            for (i, rows) in out.chunks(band * stride).enumerate() {
                let _ = c.put_image(ImageFormat::Z_PIXMAP, win, gc, w as u16, (rows.len() / stride) as u16, 0, (i * band) as i16, 0, 32, rows);
            }
            if !mapped {
                // The first frame before the window shows.
                let _ = c.map_window(win);
                mapped = true;
            }
            let _ = c.flush();
            // Errors (the server's) are of no consequence to the splash.
            while let Ok(Some(_)) = c.poll_for_event() {}
            std::thread::sleep(next.saturating_duration_since(Instant::now()));
        }
        let _ = c.destroy_window(win);
        let _ = c.free_colormap(colormap);
        let _ = c.flush();
        Some(())
    }
}

// ---------------------------------------------------------------------------------------------------------------
// macOS: the splash in a process of its own (`lemonstudio --splash`). AppKit puts no window of a process on the
// screen before its main thread has launched it, and the app's main thread is busy starting up until just before the
// main window shows; the splash process launches at once and animates on its own main thread, from what the app writes
// down its standard input. A borderless, clear window at the status level, whose layer it fills.

#[cfg(target_os = "macos")]
mod mac {
    use std::ffi::c_void;
    use std::io::{BufRead, Write};
    use std::process::{Child, ChildStdin, Command, Stdio};
    use std::ptr::NonNull;
    use std::sync::atomic::Ordering;
    use std::sync::{Arc, Mutex, MutexGuard};
    use std::time::{Duration, Instant};

    use objc2::rc::Retained;
    use objc2::{MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{
        NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSColor, NSScreen, NSWindow, NSWindowAnimationBehavior, NSWindowCollectionBehavior,
        NSWindowStyleMask,
    };
    use objc2_core_foundation::{kCFRunLoopDefaultMode, CFRetained, CFRunLoop};
    use objc2_core_graphics::{
        CGBitmapInfo, CGColorRenderingIntent, CGColorSpace, CGDataProvider, CGImage, CGImageAlphaInfo, CGImageByteOrderInfo, CGImageComponentInfo, CGImagePixelFormatInfo,
    };
    use objc2_foundation::{NSPoint, NSRect, NSSize};
    use objc2_quartz_core::{CALayer, CATransaction};

    use super::{device, Anim, Feed, Painter, Shared};

    /// The argument that starts the splash process.
    pub const ARG: &str = "--splash";
    /// Above every other window, the Dock and floating panels included (`NSStatusWindowLevel`).
    const LEVEL: isize = 25;
    /// Time between frames.
    const FRAME: Duration = Duration::from_millis(16);

    fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
        m.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The splash process, and the pipe startup's progress goes down.
    pub struct Window {
        child: Mutex<Child>,
        pipe: Mutex<Option<ChildStdin>>,
    }

    pub fn open(_shared: Arc<Shared>) -> Option<Window> {
        let mut child = Command::new(std::env::current_exe().ok()?).arg(ARG).stdin(Stdio::piped()).stdout(Stdio::null()).spawn().ok()?;
        let pipe = child.stdin.take();
        Some(Window { child: Mutex::new(child), pipe: Mutex::new(pipe) })
    }

    impl Window {
        /// Tells the splash process where startup is: a line "<progress> <step>", or "done".
        pub fn tell(&self, feed: &Feed) {
            let mut pipe = lock(&self.pipe);
            let Some(p) = pipe.as_mut() else { return };
            let line = if feed.done { "done\n".to_string() } else { format!("{} {}\n", feed.progress, feed.step) };
            if p.write_all(line.as_bytes()).is_err() {
                *pipe = None;
            }
        }

        /// Whether the splash process has ended: the splash finished and faded away.
        pub fn gone(&self) -> bool {
            matches!(lock(&self.child).try_wait(), Ok(Some(_)))
        }

        /// Ends the splash process: its input closing ends it, and it is ended in any case.
        pub fn close(&self) {
            lock(&self.pipe).take();
            let mut child = lock(&self.child);
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    /// The splash process (`ARG`): shows the splash until it has gone, or the app has closed the pipe (dropped the
    /// splash, or ended). Never returns.
    pub fn child_main() -> ! {
        let shared = Arc::new(Shared::default());
        let feed = shared.clone();
        std::thread::spawn(move || {
            for line in std::io::stdin().lock().lines() {
                let Ok(line) = line else { break };
                let mut f = lock(&feed.feed);
                if line == "done" {
                    f.done = true;
                } else if let Some((p, what)) = line.split_once(' ') {
                    if let Ok(p) = p.parse::<f32>() {
                        f.progress = f.progress.max(p);
                        f.step = what.to_string();
                    }
                }
            }
            feed.closed.store(true, Ordering::SeqCst);
        });
        run(&shared);
        std::process::exit(0)
    }

    /// Shows the splash and animates it on this (the main) thread.
    fn run(shared: &Shared) -> Option<()> {
        let mtm = MainThreadMarker::new()?;
        // No Dock icon or menu bar of its own; launched now, so its window shows.
        let app = NSApplication::sharedApplication(mtm);
        app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
        app.finishLaunching();
        // The primary screen (the one with the menu bar), in points from its bottom left.
        let screen = NSScreen::screens(mtm).firstObject()?;
        let (frame, backing) = (screen.frame(), screen.backingScaleFactor());
        let scale = crate::gui_scale(backing) as f32;
        let (w, h) = device(scale);
        let size = NSSize::new(f64::from(w) / backing, f64::from(h) / backing);
        let at = NSPoint::new(frame.origin.x + (frame.size.width - size.width) / 2.0, frame.origin.y + (frame.size.height - size.height) / 2.0);
        // SAFETY: a new borderless window, kept until the process ends (not released when closed).
        let window: Retained<NSWindow> = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(NSWindow::alloc(mtm), NSRect::new(at, size), NSWindowStyleMask::Borderless, NSBackingStoreType::Buffered, false)
        };
        unsafe { window.setReleasedWhenClosed(false) };
        window.setOpaque(false);
        window.setBackgroundColor(Some(&NSColor::clearColor()));
        window.setHasShadow(false);
        window.setIgnoresMouseEvents(true);
        window.setLevel(LEVEL);
        window.setAnimationBehavior(NSWindowAnimationBehavior::None);
        window.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::Stationary
                | NSWindowCollectionBehavior::FullScreenAuxiliary
                | NSWindowCollectionBehavior::IgnoresCycle,
        );
        // A layer-hosting view: the frames go into its layer.
        let layer = CALayer::new();
        layer.setContentsScale(backing);
        let view = window.contentView()?;
        view.setLayer(Some(&layer));
        view.setWantsLayer(true);
        window.orderFrontRegardless();
        let mut painter = Painter::new(scale)?;
        let mut anim = Anim::default();
        let start = Instant::now();
        while !shared.closed.load(Ordering::SeqCst) {
            let next = Instant::now() + FRAME;
            let (progress, what, done) = shared.feed();
            let Some(look) = anim.frame(start.elapsed().as_secs_f32(), progress, &what, done) else { break };
            if let Some(img) = image(painter.paint(&look)) {
                CATransaction::begin();
                CATransaction::setDisableActions(true);
                // SAFETY: a CGImage is what a layer's contents may be.
                unsafe { layer.setContents(Some(img.as_ref())) };
                layer.setOpacity(look.alpha);
                CATransaction::commit();
            }
            // AppKit puts the frame on the screen as the run loop turns, until the next one is due.
            // SAFETY: CoreFoundation's own constant.
            CFRunLoop::run_in_mode(unsafe { kCFRunLoopDefaultMode }, next.saturating_duration_since(Instant::now()).as_secs_f64(), false);
            std::thread::sleep(next.saturating_duration_since(Instant::now()));
        }
        Some(())
    }

    /// `px` (premultiplied RGBA) as an image for the layer.
    fn image(px: &tiny_skia::Pixmap) -> Option<CFRetained<CGImage>> {
        unsafe extern "C-unwind" fn release(_info: *mut c_void, data: NonNull<c_void>, size: usize) {
            let slice = std::ptr::slice_from_raw_parts_mut(data.cast::<u8>().as_ptr(), size);
            // SAFETY: the boxed slice handed out below.
            drop(unsafe { Box::from_raw(slice) });
        }
        let (w, h) = (px.width() as usize, px.height() as usize);
        let bytes = px.data().to_vec().into_boxed_slice();
        let len = bytes.len();
        let data = Box::into_raw(bytes).cast::<c_void>();
        // SAFETY: `data` holds `len` bytes until `release` frees it.
        let provider = unsafe { CGDataProvider::with_data(std::ptr::null_mut(), data, len, Some(release)) }?;
        let space = CGColorSpace::new_device_rgb()?;
        let info = CGBitmapInfo(CGImageAlphaInfo::PremultipliedLast.0 | CGImageComponentInfo::Integer.0 | CGImageByteOrderInfo::OrderDefault.0 | CGImagePixelFormatInfo::Packed.0);
        unsafe { CGImage::new(w, h, 8, 32, w * 4, Some(&space), info, Some(&provider), std::ptr::null(), false, CGColorRenderingIntent::RenderingIntentDefault) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prefs::{check, find_check, set_check, SHOW_SPLASH, SILENT_STARTUP};

    /// "Show startup splash screen" (spec: ui/settings-choices.md "Miscellaneous: naming, detached plugins and
    /// startup"): on as found, a program setting; with it off the start opens no splash.
    #[test]
    fn the_splash_follows_its_check() {
        assert_eq!(find_check("Show startup splash screen").map(|c| c.key), Some(SHOW_SPLASH.key));
        assert!(wanted(), "on by default");
        set_check(&SHOW_SPLASH, false);
        assert!(!wanted());
        assert!(open().is_none(), "off: no splash");
        set_check(&SHOW_SPLASH, true);
        assert!(wanted());
    }

    /// The gauge follows startup's progress, never going back, under a status line that names the step; it fades and
    /// grows in, a quick startup still sees it fill for a moment, and then it tops up, the light fades, it says "Ready"
    /// and fades away.
    #[test]
    fn the_gauge_follows_startup_then_goes() {
        let mut a = Anim::default();
        let first = a.frame(0.0, 0.0, "Reading settings", false).unwrap();
        assert_eq!((first.alpha, first.progress, first.light), (0.0, 0.0, 0.0), "it fades in from nothing");
        assert!(first.scale < 1.0);
        let mut t = 0.0;
        let mut last = 0.0;
        // Startup steps on; the gauge eases after it, never past it.
        for (to, p) in [(0.4, 0.2), (1.5, 0.5)] {
            while t < to {
                t += 1.0 / 60.0;
                let l = a.frame(t, p, "Opening project", false).unwrap();
                assert!(l.progress >= last && l.progress <= p, "at {t}: {}", l.progress);
                last = l.progress;
            }
        }
        let l = a.frame(t, 0.3, "Opening project", false).unwrap();
        assert!((l.progress - 0.5).abs() < 0.01, "caught up, and not back: {}", l.progress);
        assert_eq!((l.alpha, l.scale, l.light), (1.0, 1.0, 1.0));
        assert_eq!(l.caption, "Opening project · 50%");
        // Done: it tops up while the light fades, then fades away.
        let t0 = t;
        let mut seen_ready = false;
        let gone_at = loop {
            t += 1.0 / 60.0;
            match a.frame(t, 0.5, "Opening project", true) {
                Some(l) => {
                    if t - t0 >= TOP_UP + 0.02 {
                        assert!((l.progress - 1.0).abs() < 1e-6);
                        assert_eq!(l.caption, "Ready");
                        seen_ready = true;
                    }
                    if t - t0 >= LIGHT_OUT + 0.02 {
                        assert_eq!(l.light, 0.0);
                    }
                    assert!(t - t0 < FADE_OUT.1 + 0.02);
                }
                None => break t - t0,
            }
        };
        assert!(seen_ready);
        assert!((gone_at - FADE_OUT.1).abs() < 0.05, "gone after {gone_at} s");
        // A quick startup: done at once, the gauge fills all the way, and it finishes at the earliest.
        let mut b = Anim::default();
        let (mut t, mut l) = (0.0, b.frame(0.0, 0.2, "Reading settings", true).unwrap());
        while t + 1.0 / 60.0 < EARLIEST {
            t += 1.0 / 60.0;
            l = b.frame(t, 0.2, "Reading settings", true).unwrap();
        }
        assert!(b.done.is_none() && l.progress > 0.99, "filled, not finishing yet: {}", l.progress);
        b.frame(EARLIEST, 0.2, "Reading settings", true).unwrap();
        assert_eq!(b.done.map(|d| d.0), Some(EARLIEST));
        // A start that never says it is done does not keep the splash up for ever.
        let mut c = Anim::default();
        c.frame(LONGEST, 0.3, "Stuck", false).unwrap();
        assert!(c.done.is_some());
    }

    /// Device px of logo point `(x, y)` (logo units) in the frame at `scale`, the disc unscaled.
    fn at(scale: f32, x: f32, y: f32) -> (usize, usize) {
        let k = DISC.2 * scale / (2.0 * R);
        ((DISC.0 * scale + (x - C.0) * k) as usize, (DISC.1 * scale + (y - C.1) * k) as usize)
    }

    /// The splash's picture (as above): the full-colour logo centred on a transparent window, a soft shadow falling
    /// under the disc, the lemon filled round as far as the gauge, the groove light on the record and the status line
    /// under the disc.
    #[test]
    fn the_splash_draws_the_gauge_with_its_shadow() {
        for scale in [1.0, 2.0] {
            let mut p = Painter::new(scale).expect("painter");
            let look = |progress: f32, light: f32| Look { alpha: 1.0, scale: 1.0, progress, light, angle: -30.0, caption: "Opening project · 25%".into() };
            let (w, h) = device(scale);
            let (w, h) = (w as usize, h as usize);
            let px = p.paint(&look(0.25, 0.0)).clone();
            let rgba = |x: usize, y: usize| {
                let c = px.pixel(x as u32, y as u32).unwrap().demultiply();
                (c.red(), c.green(), c.blue(), c.alpha())
            };
            for (x, y) in [(0, 0), (w - 1, 0), (0, h - 1), (w - 1, h - 1)] {
                assert_eq!(rgba(x, y).3, 0, "a clear corner, scale {scale}");
            }
            // The shadow: under the disc's bottom edge more than over its top, nothing well clear of it.
            let (cx, cy, r) = ((DISC.0 * scale) as usize, (DISC.1 * scale) as usize, (DISC.2 / 2.0 * scale) as usize);
            let gap = (8.0 * scale) as usize;
            let (below, above) = (rgba(cx, cy + r + gap).3, rgba(cx, cy - r - gap).3);
            assert!(below > above && above > 0, "a soft shadow falling down, scale {scale}: {below} vs {above}");
            assert_eq!(rgba(w - 1, cy).3, 0, "it fades out before the window's edge, scale {scale}");
            // The gauge at a quarter: the lower right slice filled, the upper left one still empty.
            let (x1, y1) = at(scale, 602.0, 691.0);
            let (x4, y4) = at(scale, 272.0, 410.0);
            assert_eq!(rgba(x1, y1), (0xff, 0xe0, 0x66, 0xff), "slice 1 filled, scale {scale}");
            let empty = mix(SLICE_FILLS[3], EMPTY.0, EMPTY.1);
            let e = rgba(x4, y4);
            assert_eq!((u32::from(e.0) << 16) | (u32::from(e.1) << 8) | u32::from(e.2), empty, "slice 4 empty, scale {scale}");
            let full = p.paint(&look(1.0, 0.0)).clone();
            let c = full.pixel(x4 as u32, y4 as u32).unwrap().demultiply();
            assert_eq!((c.red(), c.green(), c.blue()), (0xff, 0xf1, 0xb8), "slice 4 filled at the end, scale {scale}");
            // The groove light: the record brighter along the wedge (pointing up and right, clear of the record's
            // windows), and as it was 30 degrees off it.
            let lit = p.paint(&look(0.25, 1.0)).clone();
            let a = (-30f32).to_radians();
            let (lx, ly) = at(scale, C.0 + 300.0 * a.cos(), C.1 + 300.0 * a.sin());
            let sum = |pm: &Pixmap, x: usize, y: usize| {
                let c = pm.pixel(x as u32, y as u32).unwrap();
                u32::from(c.red()) + u32::from(c.green()) + u32::from(c.blue())
            };
            assert!(sum(&lit, lx, ly) > sum(&px, lx, ly) + 20, "the light on the record, scale {scale}");
            let (ox, oy) = at(scale, C.0 + 300.0, C.1);
            assert_eq!(sum(&lit, ox, oy), sum(&px, ox, oy), "away from the wedge, scale {scale}");
            // The status line: an opaque pill centred under the disc, no wider than its text needs.
            let ty = cy + r + ((CAPTION_GAP + CAPTION_H / 2.0) * scale) as usize;
            assert_eq!(rgba(cx, ty).3, 0xff, "the status line, scale {scale}");
            assert_eq!(rgba(cx - r, ty).3, 0, "clear beside it, scale {scale}");
        }
    }

    /// The shadow's blur is a Gaussian's: half strength on the disc's edge, the error function either side.
    #[test]
    fn the_shadow_is_gaussian() {
        assert!((erfc(0.0) - 1.0).abs() < 1e-6);
        assert!((erfc(1.0) - 0.157_299_2).abs() < 1e-6);
        assert!((erfc(-1.0) - 1.842_700_8).abs() < 1e-6);
        assert!(erfc(4.0) < 1e-7);
    }

    /// "Silent startup" (spec, as above): stored and shown, off as found. Lemon Studio has no startup sound, so it
    /// silences nothing yet.
    #[test]
    fn silent_startup_is_stored() {
        assert_eq!(find_check("Silent startup").map(|c| c.key), Some(SILENT_STARTUP.key));
        assert!(!check(&SILENT_STARTUP), "off by default");
        set_check(&SILENT_STARTUP, true);
        assert!(check(&SILENT_STARTUP));
        set_check(&SILENT_STARTUP, false);
    }
}
