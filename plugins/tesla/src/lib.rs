// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Tesla: Lemon Studio's audio editor and recorder (the counterpart of FL's Edison) as a VST3 / CLAP plugin
//! (nih-plug). Put on a mixer track it passes audio through, records what reaches it, holds one sample and edits it
//! destructively, and plays it back into the track. The sample is saved in the plugin state as a plain WAV with
//! Edison's chunks.
//!
// spec: plugins/tesla/README.md, plugins/tesla/roadmap.md, plugins/edison/, plugins/plugin-formats.md
#![allow(semicolon_in_expressions_from_non_local_macros)]

pub mod doc;
pub mod editor;
pub mod peaks;
pub mod rec;
pub mod sample;
pub mod spectrum;
pub mod state;
pub mod tools;
pub mod view;
pub mod wav;

use std::collections::BTreeMap;
use std::num::NonZeroU32;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, Weak};

use nih_plug::prelude::*;

use doc::Doc;
use rec::{Recorder, Ring};
use sample::{Format, Sample};
use state::Saved;

/// What the editor asks the audio thread to do.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Cmd {
    /// Play `from..=to` (frames of the sample), starting at `at`.
    Play { from: usize, to: usize, at: f64 },
    Pause,
    /// Stop, back to `at`.
    Stop { at: f64 },
    /// Scrub towards a position (frames), or end scrubbing.
    Scrub(Option<f64>),
}

/// The recording in progress, as the drain thread builds it.
#[derive(Debug, Default)]
pub struct Take {
    pub l: Vec<f32>,
    pub r: Vec<f32>,
    pub rate: u32,
    /// Frames arrived since arming.
    pub active: bool,
    /// Disarmed: the next drain commits the take (frames still in flight arrive first).
    stopping: bool,
}

/// What the audio thread, the editor and the drain thread share.
pub struct Shared {
    pub doc: Mutex<Doc>,
    pub host_rate: AtomicU32,
    /// The sample the audio thread plays, and the document version it is.
    published: Mutex<Arc<Sample>>,
    published_ver: AtomicU64,
    /// Samples the audio thread let go of (freed here, not on the audio thread).
    retired: Mutex<Vec<Arc<Sample>>>,
    cmds: Mutex<Vec<Cmd>>,
    /// Playback, as of the audio thread's last block: playing, and the position in frames (f64 bits).
    pub playing: AtomicBool,
    pub pos: AtomicU64,
    /// The input's peak over the last block (f32 bits).
    pub level: AtomicU32,
    /// The recording settings (the editor and the audio thread read them every block).
    pub when: AtomicU8,
    pub max: AtomicU8,
    pub append: AtomicBool,
    pub slave: AtomicBool,
    pub armed: AtomicBool,
    pub mute: AtomicBool,
    pub looping: AtomicBool,
    pub ignore_host_sel: AtomicBool,
    pub ignore_own_sel: AtomicBool,
    /// Frames are being kept right now.
    pub capturing: AtomicBool,
    pub ring: Ring,
    pub take: Mutex<Take>,
    /// Bumped when the take grows or is committed.
    pub take_ver: AtomicU64,
    /// Bumped when a state is loaded (the editor starts over on it).
    pub loaded: AtomicU64,
    /// The editor's size in logical pixels.
    pub size: (AtomicU32, AtomicU32),
    pub editor_open: AtomicBool,
}

/// The ring holds this many seconds at 192 kHz (the drain thread empties it every 10 ms).
const RING_SECONDS: usize = 8;

impl Default for Shared {
    fn default() -> Self {
        let d = Doc::default();
        let s = Shared {
            published: Mutex::new(d.sample.clone()),
            published_ver: AtomicU64::new(d.version),
            doc: Mutex::new(d),
            host_rate: AtomicU32::new(44100),
            retired: Mutex::new(Vec::with_capacity(64)),
            cmds: Mutex::new(Vec::with_capacity(64)),
            playing: AtomicBool::new(false),
            pos: AtomicU64::new(0f64.to_bits()),
            level: AtomicU32::new(0),
            when: AtomicU8::new(state::WHEN_ON_INPUT),
            max: AtomicU8::new(5),
            append: AtomicBool::new(true),
            slave: AtomicBool::new(false),
            armed: AtomicBool::new(false),
            mute: AtomicBool::new(false),
            looping: AtomicBool::new(false),
            ignore_host_sel: AtomicBool::new(false),
            ignore_own_sel: AtomicBool::new(false),
            capturing: AtomicBool::new(false),
            ring: Ring::new(192_000 * RING_SECONDS),
            take: Mutex::new(Take::default()),
            take_ver: AtomicU64::new(0),
            loaded: AtomicU64::new(0),
            size: (AtomicU32::new(view::W), AtomicU32::new(view::H)),
            editor_open: AtomicBool::new(false),
        };
        s.restore_settings(&state::Rec::default(), &state::View::default());
        s
    }
}

impl Shared {
    /// Runs `f` on the document, then hands a changed sample to the audio thread.
    pub fn with_doc<R>(&self, f: impl FnOnce(&mut Doc) -> R) -> R {
        let mut d = self.doc.lock().unwrap_or_else(|e| e.into_inner());
        let r = f(&mut d);
        self.publish(&d);
        r
    }

    fn publish(&self, d: &Doc) {
        if self.published_ver.load(Ordering::Acquire) != d.version {
            if let Ok(mut p) = self.published.lock() {
                *p = d.sample.clone();
            }
            self.published_ver.store(d.version, Ordering::Release);
        }
        if let Ok(mut r) = self.retired.lock() {
            r.clear();
        }
    }

    pub fn send(&self, c: Cmd) {
        if let Ok(mut q) = self.cmds.lock() {
            q.push(c);
        }
    }

    pub fn position(&self) -> f64 {
        f64::from_bits(self.pos.load(Ordering::Relaxed))
    }

    fn restore_settings(&self, r: &state::Rec, v: &state::View) {
        self.when.store(r.when, Ordering::Relaxed);
        self.max.store(r.max, Ordering::Relaxed);
        self.append.store(r.append, Ordering::Relaxed);
        self.slave.store(r.slave, Ordering::Relaxed);
        self.armed.store(r.armed, Ordering::Relaxed);
        self.mute.store(r.mute, Ordering::Relaxed);
        self.ignore_host_sel.store(v.ignore_host_sel, Ordering::Relaxed);
        self.ignore_own_sel.store(v.ignore_own_sel, Ordering::Relaxed);
        if v.w >= view::MIN_W && v.h >= view::MIN_H {
            self.size.0.store(v.w, Ordering::Relaxed);
            self.size.1.store(v.h, Ordering::Relaxed);
        }
    }

    /// The recording settings as saved.
    pub fn rec(&self) -> state::Rec {
        state::Rec {
            when: self.when.load(Ordering::Relaxed),
            max: self.max.load(Ordering::Relaxed),
            append: self.append.load(Ordering::Relaxed),
            slave: self.slave.load(Ordering::Relaxed),
            armed: self.armed.load(Ordering::Relaxed),
            mute: self.mute.load(Ordering::Relaxed),
        }
    }

    /// What the host saves: the document, the settings, and a recording in progress as if it were done.
    pub fn saved(&self) -> Saved {
        let mut s = self.doc.lock().unwrap_or_else(|e| e.into_inner()).saved();
        s.rec = self.rec();
        s.view.ignore_host_sel = self.ignore_host_sel.load(Ordering::Relaxed);
        s.view.ignore_own_sel = self.ignore_own_sel.load(Ordering::Relaxed);
        s.view.w = self.size.0.load(Ordering::Relaxed);
        s.view.h = self.size.1.load(Ordering::Relaxed);
        if let Ok(t) = self.take.lock() {
            if t.active && !t.l.is_empty() {
                s.sample = joined(&s.sample, &t, s.rec.append);
            }
        }
        s
    }

    /// A saved state (the host's, a preset, a converted Edison slot) becomes the document.
    pub fn restore(&self, s: Saved) {
        self.restore_settings(&s.rec, &s.view);
        if let Ok(mut t) = self.take.lock() {
            *t = Take::default();
        }
        self.with_doc(|d| {
            let ver = d.version + 1;
            *d = Doc::from_saved(s);
            d.version = ver;
        });
        self.loaded.fetch_add(1, Ordering::AcqRel);
    }

    /// Moves recorded frames from the ring into the take, which keeps only its last frames up to the For limit (a
    /// rolling log, as Edison "wraps and overwrites"); once disarmed, the take joins the document (appended, or
    /// replacing the sample with Append off). Called by the drain thread every 10 ms.
    pub fn drain(&self) {
        let mut t = self.take.lock().unwrap_or_else(|e| e.into_inner());
        if !t.active && t.l.is_empty() {
            t.rate = self.host_rate.load(Ordering::Relaxed);
        }
        let mut l = std::mem::take(&mut t.l);
        let mut r = std::mem::take(&mut t.r);
        let n = self.ring.drain(&mut l, &mut r);
        let limit = rec::Recorder::limit_frames(self.max.load(Ordering::Relaxed), t.rate.max(1));
        if l.len() as u64 > limit {
            let over = l.len() - limit as usize;
            l.drain(..over);
            r.drain(..over);
        }
        t.l = l;
        t.r = r;
        if n > 0 {
            t.active = true;
            self.take_ver.fetch_add(1, Ordering::AcqRel);
        }
        if t.active && !self.armed.load(Ordering::Acquire) {
            if !t.stopping {
                t.stopping = true;
                return;
            }
            let take = std::mem::take(&mut *t);
            drop(t);
            let append = self.append.load(Ordering::Relaxed);
            self.with_doc(|d| {
                let joined = joined(&d.sample, &take, append);
                let path = if append { d.path.clone() } else { String::new() };
                d.load_as(joined, &path, "Record");
            });
            self.take_ver.fetch_add(1, Ordering::AcqRel);
        }
    }
}

/// The document's sample with a take after it (`append`), or the take alone, at the document's rate and channels
/// (a take alone keeps its own rate, stereo, 32-bit float).
pub fn joined(s: &Sample, t: &Take, append: bool) -> Sample {
    let take = Sample::from_channels(t.rate.max(1), vec![t.l.clone(), t.r.clone()], Format::Float32);
    if !append || s.is_empty() {
        return take;
    }
    let take = doc::conform(&take, s.rate, s.channels());
    let mut out = s.clone();
    for (c, src) in out.ch.iter_mut().zip(&take.ch) {
        c.extend_from_slice(src);
    }
    out.quantize();
    out
}

fn drain_loop(shared: Weak<Shared>) {
    loop {
        std::thread::sleep(std::time::Duration::from_millis(10));
        let Some(s) = shared.upgrade() else { return };
        s.drain();
    }
}

pub struct TeslaParams {
    pub play: BoolParam,
    pub record: BoolParam,
    pub shared: Arc<Shared>,
}

impl Default for TeslaParams {
    fn default() -> Self {
        TeslaParams { play: BoolParam::new("Play / pause", false), record: BoolParam::new("Record", false), shared: Arc::new(Shared::default()) }
    }
}

// SAFETY: the parameter pointers point into this struct, which the plugin keeps alive (and pinned in its `Arc`) for as
// long as the host holds them.
unsafe impl Params for TeslaParams {
    fn param_map(&self) -> Vec<(String, ParamPtr, String)> {
        vec![("play".into(), self.play.as_ptr(), String::new()), ("record".into(), self.record.as_ptr(), String::new())]
    }

    fn serialize_fields(&self) -> BTreeMap<String, String> {
        BTreeMap::from([(state::STATE_FIELD.to_string(), state::to_json(&self.shared.saved()))])
    }

    fn deserialize_fields(&self, serialized: &BTreeMap<String, String>) {
        if let Some(s) = serialized.get(state::STATE_FIELD).and_then(|t| state::from_json(t)) {
            self.shared.restore(s);
        }
    }
}

/// The audio thread's playback.
#[derive(Default)]
struct Player {
    sample: Option<Arc<Sample>>,
    ver: u64,
    playing: bool,
    pos: f64,
    from: usize,
    to: usize,
    scrub: Option<f64>,
    host_was_playing: bool,
    play_param: bool,
    rec_param: bool,
    rec: Recorder,
    was_armed: bool,
}

impl Player {
    /// Takes the newest published sample (the one it lets go of is freed off the audio thread).
    fn sync(&mut self, s: &Shared) {
        let v = s.published_ver.load(Ordering::Acquire);
        if v == self.ver {
            return;
        }
        if let Ok(p) = s.published.try_lock() {
            let old = self.sample.replace(p.clone());
            self.ver = v;
            if let (Some(old), Ok(mut r)) = (old, s.retired.try_lock()) {
                if r.len() < r.capacity() {
                    r.push(old);
                }
            }
            let n = self.frames();
            if n == 0 {
                self.playing = false;
                self.pos = 0.0;
            } else {
                self.to = self.to.min(n - 1);
                self.from = self.from.min(self.to);
                self.pos = self.pos.min(n as f64);
            }
        }
    }

    fn frames(&self) -> usize {
        self.sample.as_ref().map_or(0, |s| s.frames())
    }

    fn start(&mut self, from: usize, to: usize, at: f64) {
        let n = self.frames();
        if n == 0 {
            return;
        }
        self.to = to.min(n - 1);
        self.from = from.min(self.to);
        self.pos = if at >= self.from as f64 && at <= self.to as f64 { at } else { self.from as f64 };
        self.playing = true;
    }

    fn commands(&mut self, s: &Shared) {
        let Ok(mut q) = s.cmds.try_lock() else { return };
        for c in q.drain(..) {
            match c {
                Cmd::Play { from, to, at } => self.start(from, to, at),
                Cmd::Pause => self.playing = false,
                Cmd::Stop { at } => {
                    self.playing = false;
                    self.pos = at;
                }
                Cmd::Scrub(p) => self.scrub = p,
            }
        }
    }

    /// The sample at fractional frame `x` in channel `c`.
    fn read(s: &Sample, c: usize, x: f64) -> f32 {
        let i = x.floor();
        let t = (x - i) as f32;
        let i = i as usize;
        let a = s.at(c, i);
        let b = s.at(c, i + 1);
        a + (b - a) * t
    }
}

pub struct Tesla {
    params: Arc<TeslaParams>,
    player: Player,
    drain_started: bool,
}

impl Default for Tesla {
    fn default() -> Self {
        Tesla { params: Arc::new(TeslaParams::default()), player: Player::default(), drain_started: false }
    }
}

impl Tesla {
    pub fn shared(&self) -> &Arc<Shared> {
        &self.params.shared
    }
}

impl Plugin for Tesla {
    const NAME: &'static str = "Tesla";
    const VENDOR: &'static str = "Lemon Studio";
    const URL: &'static str = "";
    const EMAIL: &'static str = "";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] =
        &[AudioIOLayout { main_input_channels: NonZeroU32::new(2), main_output_channels: NonZeroU32::new(2), ..AudioIOLayout::const_default() }];
    const SAMPLE_ACCURATE_AUTOMATION: bool = false;

    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        Some(Box::new(editor::TeslaEditor::new(self.params.clone())))
    }

    fn initialize(&mut self, _layout: &AudioIOLayout, config: &BufferConfig, _ctx: &mut impl InitContext<Self>) -> bool {
        self.shared().host_rate.store(config.sample_rate.round() as u32, Ordering::Relaxed);
        if !self.drain_started {
            let weak = Arc::downgrade(self.shared());
            self.drain_started = std::thread::Builder::new().name("tesla-take".into()).spawn(move || drain_loop(weak)).is_ok();
        }
        true
    }

    fn reset(&mut self) {
        self.player.scrub = None;
    }

    fn process(&mut self, buffer: &mut Buffer, _aux: &mut AuxiliaryBuffers, ctx: &mut impl ProcessContext<Self>) -> ProcessStatus {
        let s = self.params.shared.clone();
        let host_rate = s.host_rate.load(Ordering::Relaxed).max(1);
        let p = &mut self.player;
        p.sync(&s);
        p.commands(&s);

        // The two parameters toggle on change (host automation): play / pause, arm / disarm.
        let play = self.params.play.value();
        if play != p.play_param {
            p.play_param = play;
            if p.playing {
                p.playing = false;
            } else {
                let n = p.frames();
                if n > 0 {
                    let at = p.pos;
                    p.start(0, n - 1, at);
                }
            }
        }
        let rec = self.params.record.value();
        if rec != p.rec_param {
            p.rec_param = rec;
            s.armed.store(!s.armed.load(Ordering::Relaxed), Ordering::Release);
        }

        // Slave playback to the host's transport (Tesla's rule: the host's position from its start, or from its loop
        // start, lands on Tesla's selection start, or on the sample's start).
        let t = ctx.transport();
        let host_playing = t.playing;
        if s.slave.load(Ordering::Relaxed) {
            if host_playing != p.host_was_playing {
                if host_playing {
                    let n = p.frames();
                    if n > 0 {
                        let sel = s.doc.try_lock().ok().and_then(|d| d.sel);
                        let (from, to) = if s.ignore_own_sel.load(Ordering::Relaxed) { None } else { sel }.unwrap_or((0, n - 1));
                        let mut host = t.pos_samples().unwrap_or(0).max(0);
                        if !s.ignore_host_sel.load(Ordering::Relaxed) {
                            if let Some((a, _)) = t.loop_range_samples() {
                                host = (host - a).max(0);
                            }
                        }
                        let ratio = p.sample.as_ref().map_or(1.0, |x| x.rate as f64 / host_rate as f64);
                        let at = from as f64 + host as f64 * ratio;
                        if at <= to as f64 {
                            p.start(from, to, at);
                            p.pos = at;
                        }
                    }
                } else {
                    p.playing = false;
                }
            }
        }
        p.host_was_playing = host_playing;

        let out = buffer.as_slice();
        if out.len() < 2 {
            return ProcessStatus::Normal;
        }
        let (left, right) = out.split_at_mut(1);
        let (l, r) = (&mut *left[0], &mut *right[0]);
        let frames = l.len();

        // Recording: the input before playback is added.
        let armed = s.armed.load(Ordering::Acquire);
        if armed && !p.was_armed {
            p.rec.reset();
        }
        p.was_armed = armed;
        let mut peak = 0f32;
        for i in 0..frames {
            peak = peak.max(l[i].abs()).max(r[i].abs());
        }
        s.level.store(peak.to_bits(), Ordering::Relaxed);
        if armed {
            let when = s.when.load(Ordering::Relaxed);
            let ring = &s.ring;
            let (li, ri) = (&*l, &*r);
            let b = p.rec.block(when, host_playing, host_rate, li, ri, |i| {
                ring.push([li[i], ri[i]]);
            });
            s.capturing.store(b.kept > 0, Ordering::Relaxed);
        } else {
            s.capturing.store(false, Ordering::Relaxed);
        }

        if s.mute.load(Ordering::Relaxed) {
            l.fill(0.0);
            r.fill(0.0);
        }

        // Playback (or scrubbing) into the track.
        if let Some(smp) = p.sample.clone() {
            let n = smp.frames();
            let step = smp.rate as f64 / host_rate as f64;
            if let (Some(target), true) = (p.scrub, n > 0) {
                // Scrub: glide to the target over the block, at most 4× speed either way.
                let target = target.clamp(0.0, (n - 1) as f64);
                let v = ((target - p.pos) / frames.max(1) as f64).clamp(-4.0 * step, 4.0 * step);
                for i in 0..frames {
                    l[i] += Player::read(&smp, 0, p.pos);
                    r[i] += Player::read(&smp, 1, p.pos);
                    p.pos = (p.pos + v).clamp(0.0, (n - 1) as f64);
                }
            } else if p.playing && n > 0 {
                let looping = s.looping.load(Ordering::Relaxed);
                for i in 0..frames {
                    if p.pos > p.to as f64 + 0.999 {
                        if looping {
                            p.pos -= (p.to + 1 - p.from) as f64;
                            p.pos = p.pos.max(p.from as f64);
                        } else {
                            p.playing = false;
                            p.pos = p.from as f64;
                            break;
                        }
                    }
                    l[i] += Player::read(&smp, 0, p.pos);
                    r[i] += Player::read(&smp, 1, p.pos);
                    p.pos += step;
                }
            }
        }
        s.playing.store(p.playing || p.scrub.is_some(), Ordering::Relaxed);
        s.pos.store(p.pos.to_bits(), Ordering::Relaxed);
        ProcessStatus::Normal
    }
}

impl ClapPlugin for Tesla {
    const CLAP_ID: &'static str = "org.openstudio.tesla";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("Lemon Studio's audio editor and recorder");
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[ClapFeature::AudioEffect, ClapFeature::Utility, ClapFeature::Stereo];
}

impl Vst3Plugin for Tesla {
    const VST3_CLASS_ID: [u8; 16] = *b"LemonStudioTesla";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Fx, Vst3SubCategory::Tools];
}

// Built as a plugin module: the C entry points hosts look for. Linked into Lemon Studio (`linked` feature): only the
// symbol-free table, so several Lemon plugins fit in one program (spec: plugins/plugin-formats.md).
#[cfg(not(feature = "linked"))]
nih_export_clap!(Tesla);
#[cfg(not(feature = "linked"))]
nih_export_vst3!(Tesla);
os_plugkit::linked_clap_entry!(Tesla);

/// Lemon Studio's state for an Edison slot opened as Tesla (see `state::from_edison`); `None` when the bytes are not
/// an Edison state.
pub fn plugin_state_from_edison(fl: &[u8]) -> Option<Vec<u8>> {
    state::from_edison(fl).map(|s| state::plugin_state(&s))
}

/// The Edison state to write back for FL from a Tesla plugin state; `None` when the blob is not a Tesla state.
pub fn edison_from_plugin_state(blob: &[u8]) -> Option<Vec<u8>> {
    state::from_plugin_state(blob).map(|s| state::to_edison(&s))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_take_joins_the_document_when_disarmed() {
        let sh = Shared::default();
        sh.host_rate.store(1000, Ordering::Relaxed);
        sh.with_doc(|d| d.load(Sample::from_channels(1000, vec![vec![0.5; 10], vec![0.5; 10]], Format::Float32), "a.wav"));
        sh.armed.store(true, Ordering::Relaxed);
        for _ in 0..5 {
            sh.ring.push([0.25, -0.25]);
        }
        sh.drain();
        assert!(sh.take.lock().unwrap().active);
        // A save while recording holds the take as if done.
        assert_eq!(sh.saved().sample.frames(), 15);
        sh.armed.store(false, Ordering::Relaxed);
        sh.drain();
        sh.drain();
        let d = sh.doc.lock().unwrap();
        assert_eq!(d.frames(), 15);
        assert_eq!(d.sample.ch[1][14], -0.25);
        assert_eq!(d.path, "a.wav");
        drop(d);
        // Append off: the take replaces the sample.
        sh.append.store(false, Ordering::Relaxed);
        sh.armed.store(true, Ordering::Relaxed);
        sh.ring.push([0.1, 0.1]);
        sh.drain();
        sh.armed.store(false, Ordering::Relaxed);
        sh.drain();
        sh.drain();
        assert_eq!(sh.doc.lock().unwrap().frames(), 1);
    }

    /// At its For limit a take goes on and keeps its last frames (Edison: a rolling log).
    #[test]
    fn a_take_keeps_its_last_minute() {
        let sh = Shared::default();
        sh.host_rate.store(1000, Ordering::Relaxed);
        sh.max.store(1, Ordering::Relaxed);
        sh.append.store(false, Ordering::Relaxed);
        sh.armed.store(true, Ordering::Relaxed);
        // 70 s at 1 kHz, in drains of 1 s (frame i is i / 100,000).
        for s in 0..70 {
            for i in 0..1000 {
                let v = (s * 1000 + i) as f32 / 100_000.0;
                assert!(sh.ring.push([v, -v]));
            }
            sh.drain();
        }
        assert!(sh.armed.load(Ordering::Relaxed), "still recording");
        assert_eq!(sh.take.lock().unwrap().l.len(), 60_000);
        sh.armed.store(false, Ordering::Relaxed);
        sh.drain();
        sh.drain();
        let d = sh.doc.lock().unwrap();
        assert_eq!(d.frames(), 60_000);
        assert_eq!(d.sample.ch[0][0], 10_000.0 / 100_000.0, "the first 10 s are gone");
    }

    #[test]
    fn state_survives_the_params_round_trip() {
        let p = TeslaParams::default();
        p.shared.with_doc(|d| {
            d.load(Sample::from_channels(48000, vec![vec![0.0, 0.5, -0.5]], Format::Int16), "x.wav");
        });
        p.shared.when.store(state::WHEN_ON_PLAY, Ordering::Relaxed);
        let fields = p.serialize_fields();
        let q = TeslaParams::default();
        q.deserialize_fields(&fields);
        let d = q.shared.doc.lock().unwrap();
        assert_eq!(d.sample.ch[0], vec![0.0, 0.5, -0.5]);
        assert_eq!(d.sample.format, Format::Int16);
        assert_eq!(d.path, "x.wav");
        assert_eq!(q.shared.when.load(Ordering::Relaxed), state::WHEN_ON_PLAY);
        let sample = (*d.sample).clone();
        drop(d);
        // Edison's state converts both ways.
        let blob = plugin_state_from_edison(&state::to_edison(&q.shared.saved())).unwrap();
        assert_eq!(state::from_plugin_state(&blob).unwrap().sample, sample);
        assert!(edison_from_plugin_state(&blob).is_some());
    }
}
