// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The document Tesla edits: the sample, the selection, the undo history, the settings that are saved with it. Every
//! edit goes through here: Edit menu operations (with click-free joints), the tools, markers, regions and the loop.
//!
//! The sample is shared with the audio thread as an `Arc`: an edit while it plays makes a copy (`Arc::make_mut`), and
//! the undo history holds the earlier `Arc`s.
//!
// spec: plugins/edison/menus.md (Edit, Regions, Select, Format), plugins/edison/tools.md,
// plugins/tesla/README.md "Version 1"

use std::sync::Arc;

use crate::env::{self, Envs};
use crate::sample::{q16, Format, Sample};
use crate::state::{Rec, Saved, View};
use crate::tools;
use os_dsp::transients;

/// The auto-slicing and grid-slicing entries' names (their undo steps).
pub const AUTO_SLICING: [&str; 3] = ["Dull auto-slicing", "Medium auto-slicing", "Sharp auto-slicing"];
pub const GRID_SLICING: [&str; 3] = ["Small grid-slicing", "Medium grid-slicing", "Large grid-slicing"];

/// The click-free crossfade at an edit's joints (Tesla's own: 4 ms, raised cosine).
pub const CLICK_FREE_MS: f64 = 4.0;
/// The undo history keeps at most this many steps and this many bytes of earlier samples.
const UNDO_STEPS: usize = 32;
const UNDO_BYTES: usize = 1 << 30;
/// "Large" for Edit > Enable undo for large samples: over this many bytes, no undo unless enabled.
const LARGE: usize = 64 << 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Paste {
    Insert,
    Replace,
    Mix,
}

#[derive(Clone, Debug)]
struct Step {
    name: String,
    sample: Arc<Sample>,
    sel: Option<(usize, usize)>,
    env: Envs,
}

#[derive(Clone, Debug)]
pub struct Doc {
    pub sample: Arc<Sample>,
    pub path: String,
    pub rec: Rec,
    pub view: View,
    pub edison_env: Vec<u8>,
    /// The four envelopes (the bottom bar's envelope modes).
    pub env: Envs,
    /// The selection, first and last frame.
    pub sel: Option<(usize, usize)>,
    /// Where a click without a drag put the cursor (insert and paste go here with no selection).
    pub cursor: usize,
    undo: Vec<Step>,
    redo: Vec<Step>,
    /// Bumped by every change of the sample (the audio thread and the caches follow it).
    pub version: u64,
}

impl Default for Doc {
    fn default() -> Self {
        Doc::from_saved(Saved::default())
    }
}

fn bytes(s: &Sample) -> usize {
    s.frames() * s.channels() * 4
}

/// Raised-cosine gain rising from 0 to 1 over `k` steps, at step `j`.
fn rise(j: usize, k: usize) -> f32 {
    let x = (j as f64 + 0.5) / k as f64;
    (0.5 - 0.5 * (std::f64::consts::PI * x).cos()) as f32
}

impl Doc {
    pub fn from_saved(s: Saved) -> Doc {
        Doc { sample: Arc::new(s.sample), path: s.path, rec: s.rec, view: s.view, edison_env: s.edison_env, env: s.env, sel: None, cursor: 0, undo: Vec::new(), redo: Vec::new(), version: 1 }
    }

    pub fn saved(&self) -> Saved {
        Saved { sample: (*self.sample).clone(), path: self.path.clone(), rec: self.rec.clone(), view: self.view.clone(), edison_env: self.edison_env.clone(), env: self.env.clone() }
    }

    /// The title: the file's name without extension.
    pub fn title(&self) -> String {
        let name = self.path.rsplit(['/', '\\']).next().unwrap_or("");
        name.rsplit_once('.').map_or(name, |(a, _)| a).to_string()
    }

    pub fn frames(&self) -> usize {
        self.sample.frames()
    }

    /// The range a tool acts on: the selection, or the whole sample. `None` when the sample is empty.
    pub fn range(&self) -> Option<(usize, usize)> {
        let n = self.frames();
        if n == 0 {
            return None;
        }
        Some(match self.sel {
            Some((a, b)) => (a.min(n - 1), b.min(n - 1)),
            None => (0, n - 1),
        })
    }

    /// Where an insert goes: the selection's start, else the cursor.
    pub fn insert_at(&self) -> usize {
        self.sel.map_or(self.cursor, |(a, _)| a).min(self.frames())
    }

    pub fn select(&mut self, a: usize, b: usize) {
        let n = self.frames();
        if n == 0 {
            self.sel = None;
            return;
        }
        let (a, b) = (a.min(b).min(n - 1), a.max(b).min(n - 1));
        self.sel = Some((a, b));
    }

    fn click_frames(&self) -> usize {
        if self.view.clickfree { (CLICK_FREE_MS * self.sample.rate as f64 / 1000.0).round() as usize } else { 0 }
    }

    // ------------------------------------------------------------------------------------------- undo

    /// Records the current sample as an undo step named `name` and returns it for editing (copied when the audio
    /// thread or the history still holds it).
    fn edit(&mut self, name: &str) -> &mut Sample {
        self.push_step(name);
        self.version += 1;
        Arc::make_mut(&mut self.sample)
    }

    /// Records the current sample and envelopes as an undo step named `name` (forgetting the redo steps).
    fn push_step(&mut self, name: &str) {
        let large = bytes(&self.sample) > LARGE;
        if !large || self.view.undo_large {
            self.undo.push(Step { name: name.to_string(), sample: self.sample.clone(), sel: self.sel, env: self.env.clone() });
            while self.undo.len() > UNDO_STEPS || (self.undo.len() > 1 && self.undo.iter().map(|s| bytes(&s.sample)).sum::<usize>() > UNDO_BYTES) {
                self.undo.remove(0);
            }
        } else {
            self.undo.clear();
        }
        self.redo.clear();
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// The undo history's step names, newest first.
    pub fn history(&self) -> Vec<String> {
        self.undo.iter().rev().map(|s| s.name.clone()).collect()
    }

    /// Undoes the newest step; returns its name.
    pub fn undo(&mut self) -> Option<String> {
        let st = self.undo.pop()?;
        self.redo.push(Step { name: st.name.clone(), sample: self.sample.clone(), sel: self.sel, env: std::mem::take(&mut self.env) });
        self.sample = st.sample;
        self.sel = st.sel;
        self.env = st.env;
        self.version += 1;
        Some(st.name)
    }

    pub fn redo(&mut self) -> Option<String> {
        let st = self.redo.pop()?;
        self.undo.push(Step { name: st.name.clone(), sample: self.sample.clone(), sel: self.sel, env: std::mem::take(&mut self.env) });
        self.sample = st.sample;
        self.sel = st.sel;
        self.env = st.env;
        self.version += 1;
        Some(st.name)
    }

    /// Forgets the redo steps (a cancelled preview leaves one that would only confuse).
    pub fn discard_redo(&mut self) {
        self.redo.clear();
    }

    /// Undoes the newest `n` steps (picking a step in the undo history goes back to before it).
    pub fn undo_to(&mut self, n: usize) {
        for _ in 0..n {
            if self.undo().is_none() {
                break;
            }
        }
    }

    // ------------------------------------------------------------------------------------------- files

    /// A new sample replaces the old (File > Load, a recording that doesn't append, File > New).
    pub fn load(&mut self, s: Sample, path: &str) {
        self.load_as(s, path, "Load");
    }

    /// [`Doc::load`] with the undo step named `name` (a recording joining the sample).
    pub fn load_as(&mut self, s: Sample, path: &str, name: &str) {
        self.edit(name);
        self.sample = Arc::new(s);
        self.env = Envs::default();
        self.path = path.to_string();
        self.sel = None;
        self.cursor = 0;
        self.view.start = 0.0;
        self.view.len = 0.0;
    }

    pub fn new_sample(&mut self) {
        let s = Sample::new(self.sample.rate, self.sample.channels(), self.sample.format);
        self.load(s, "");
    }

    // ------------------------------------------------------------------------------------------- tools

    /// Runs a tool on the range (selection or whole sample). False with an empty sample.
    pub fn tool(&mut self, name: &str, f: impl FnOnce(&mut Sample, usize, usize)) -> bool {
        let Some((a, b)) = self.range() else { return false };
        let s = self.edit(name);
        f(s, a, b);
        s.quantize();
        true
    }

    /// Replaces the range with what `f` makes of it (its channels, of any length): the markers after it move with the
    /// audio, a selection becomes the new audio.
    pub fn replace_range(&mut self, name: &str, f: impl FnOnce(&Sample, usize, usize) -> Vec<Vec<f32>>) -> bool {
        let Some((a, b)) = self.range() else { return false };
        let new = f(&self.sample, a, b);
        let m = new.first().map_or(0, |c| c.len());
        if m == 0 {
            return false;
        }
        let selected = self.sel.is_some();
        let s = self.edit(name);
        for (c, v) in s.ch.iter_mut().zip(new) {
            c.splice(a..=b, v);
        }
        s.shift_after(b + 1, m as isize - (b + 1 - a) as isize);
        s.quantize();
        if selected {
            self.select(a, a + m - 1);
        }
        true
    }

    /// Runs a tool on every region's range.
    pub fn tool_regions(&mut self, name: &str, f: impl Fn(&mut Sample, usize, usize)) -> bool {
        let regions: Vec<(usize, usize)> = self.sample.cues.iter().filter(|c| c.is_region()).map(|c| (c.pos, c.end())).collect();
        if regions.is_empty() || self.frames() == 0 {
            return false;
        }
        let n = self.frames();
        let s = self.edit(name);
        for (a, b) in regions {
            if a < n {
                f(s, a, b.min(n - 1));
            }
        }
        s.quantize();
        true
    }

    /// A change of the whole sample's layout (channels, format).
    pub fn whole(&mut self, name: &str, f: impl FnOnce(&mut Sample) -> bool) -> bool {
        let mut copy = (*self.sample).clone();
        if !f(&mut copy) {
            return false;
        }
        let s = self.edit(name);
        *s = copy;
        s.quantize();
        true
    }

    pub fn set_format(&mut self, f: Format) -> bool {
        if self.sample.format == f {
            return false;
        }
        self.whole(if f == Format::Int16 { "16Bit int" } else { "32Bit float" }, |s| {
            s.format = f;
            true
        })
    }

    pub fn set_channels(&mut self, n: usize) -> bool {
        if self.sample.channels() == n {
            return false;
        }
        self.whole(if n == 1 { "Mono" } else { "Stereo" }, |s| {
            tools::set_channels(s, n);
            true
        })
    }

    // ------------------------------------------------------------------------------------------- edit

    /// The range as a sample of its own, with the markers, regions and loop inside it.
    pub fn copy(&self) -> Option<Sample> {
        let (a, b) = self.range()?;
        let s = &self.sample;
        let mut c = Sample::from_channels(s.rate, s.ch.iter().map(|ch| ch[a..=b].to_vec()).collect(), s.format);
        c.note = s.note;
        c.tempo = s.tempo;
        for cue in &s.cues {
            if cue.pos >= a && cue.pos <= b {
                c.add_cue(&cue.name, cue.pos - a, cue.len.map(|l| l.min(b + 1 - cue.pos)));
            }
        }
        if let Some((la, lb)) = s.looped {
            if la <= b && lb >= a {
                c.looped = Some((la.max(a) - a, lb.min(b) - a));
            }
        }
        Some(c)
    }

    pub fn cut(&mut self) -> Option<Sample> {
        let c = self.copy()?;
        self.delete_named("Cut");
        Some(c)
    }

    /// Removes the range; with click-free on, the audio before the joint fades into the audio after it.
    pub fn delete(&mut self) -> bool {
        self.delete_named("Delete")
    }

    fn delete_named(&mut self, name: &str) -> bool {
        let Some((a, b)) = self.range() else { return false };
        let k0 = self.click_frames();
        let s = self.edit(name);
        let n = s.frames();
        let k = k0.min(b + 1 - a).min(n - (b + 1));
        for c in &mut s.ch {
            for j in 0..k {
                let w = rise(j, k);
                c[b + 1 + j] = c[a + j] * (1.0 - w) + c[b + 1 + j] * w;
            }
            c.drain(a..=b);
        }
        s.shift_after(a, -((b + 1 - a) as isize));
        s.quantize();
        let left = s.frames();
        self.sel = None;
        self.cursor = a.min(left);
        true
    }

    /// Silences the range (Shift+Del); click-free fades the edges inside it.
    pub fn clear(&mut self) -> bool {
        let Some((a, b)) = self.range() else { return false };
        let k = self.click_frames().min((b + 1 - a) / 2);
        let s = self.edit("Clear");
        for c in &mut s.ch {
            let (head, tail) = (c[a..a + k].to_vec(), c[b + 1 - k..=b].to_vec());
            c[a..=b].fill(0.0);
            for j in 0..k {
                c[a + j] = head[j] * (1.0 - rise(j, k));
                c[b + 1 - k + j] = tail[j] * rise(j, k);
            }
        }
        true
    }

    /// Keeps only the selection (Ctrl+Del); click-free fades its ends.
    pub fn trim(&mut self) -> bool {
        let Some((a, b)) = self.sel else { return false };
        let n = self.frames();
        if n == 0 {
            return false;
        }
        let (a, b) = (a.min(n - 1), b.min(n - 1));
        let k = self.click_frames().min((b + 1 - a) / 2);
        self.sel = Some((a, b));
        // What stays is the selection with the markers, regions and loop inside it.
        let Some(mut kept) = self.copy() else { return false };
        for c in &mut kept.ch {
            let len = c.len();
            for j in 0..k {
                c[j] *= rise(j, k);
                c[len - 1 - j] *= rise(j, k);
            }
        }
        kept.quantize();
        let s = self.edit("Trim");
        *s = kept;
        self.sel = None;
        self.cursor = 0;
        true
    }

    /// Delete part after loop (Alt+Del).
    pub fn delete_after_loop(&mut self) -> bool {
        let Some((_, lb)) = self.sample.looped else { return false };
        let n = self.frames();
        if lb + 1 >= n {
            return false;
        }
        let s = self.edit("Delete part after loop");
        for c in &mut s.ch {
            c.truncate(lb + 1);
        }
        s.clip_marks();
        self.sel = None;
        true
    }

    /// Inserts `frames` of silence at the selection start (or the cursor).
    pub fn insert_silence(&mut self, frames: usize) -> bool {
        if frames == 0 {
            return false;
        }
        let at = self.insert_at();
        let s = self.edit("Insert silence");
        for c in &mut s.ch {
            c.splice(at..at, std::iter::repeat_n(0.0, frames));
        }
        s.shift_after(at, frames as isize);
        self.sel = Some((at, at + frames - 1));
        true
    }

    /// Pastes `clip` (converted to this sample's rate and channels).
    pub fn paste(&mut self, clip: &Sample, mode: Paste) -> bool {
        let clip = conform(clip, self.sample.rate, self.sample.channels());
        let m = clip.frames();
        if m == 0 {
            return false;
        }
        let k0 = self.click_frames();
        match mode {
            Paste::Insert => {
                let at = self.insert_at();
                self.insert_clip(&clip, at, k0, "Paste insert");
            }
            Paste::Replace => {
                let at = self.insert_at();
                if let (Some(_), Some((a, b))) = (self.sel, self.range()) {
                    // The selection goes; the clip comes in its place (one undo step).
                    let s = self.edit("Paste replace");
                    for c in &mut s.ch {
                        c.drain(a..=b);
                    }
                    s.shift_after(a, -((b + 1 - a) as isize));
                    self.insert_clip_raw_after_edit(&clip, a, k0);
                } else {
                    // Overwrite from the cursor, growing the sample when the clip runs past its end.
                    let s = self.edit("Paste replace");
                    let n = s.frames();
                    for (c, src) in s.ch.iter_mut().zip(&clip.ch) {
                        if at + m > n {
                            c.resize(at + m, 0.0);
                        }
                        c[at..at + m].copy_from_slice(src);
                    }
                    s.quantize();
                    self.sel = Some((at, at + m - 1));
                }
            }
            Paste::Mix => {
                let at = self.insert_at();
                let k = k0.min(m / 2);
                let s = self.edit("Paste mix");
                let n = s.frames();
                for (c, src) in s.ch.iter_mut().zip(&clip.ch) {
                    if at + m > n {
                        c.resize(at + m, 0.0);
                    }
                    for (j, v) in src.iter().enumerate() {
                        let g = if j < k { rise(j, k) } else if j >= m - k { rise(m - 1 - j, k) } else { 1.0 };
                        c[at + j] += v * g;
                    }
                }
                s.quantize();
                self.sel = Some((at, at + m - 1));
            }
        }
        true
    }

    fn insert_clip(&mut self, clip: &Sample, at: usize, k: usize, name: &str) {
        self.edit(name);
        self.insert_clip_raw_after_edit(clip, at, k);
    }

    /// The insert itself: the clip goes in at `at`; click-free crossfades the old audio that followed `at` out under
    /// the clip's start, and the clip's end into the old audio that led up to `at`.
    fn insert_clip_raw_after_edit(&mut self, clip: &Sample, at: usize, k0: usize) {
        let s = Arc::make_mut(&mut self.sample);
        let n = s.frames();
        let at = at.min(n);
        let m = clip.frames();
        let k = k0.min(m / 2);
        for (c, src) in s.ch.iter_mut().zip(&clip.ch) {
            let mut ins = src.clone();
            let k1 = k.min(n - at);
            for (j, v) in ins.iter_mut().enumerate().take(k1) {
                let w = rise(j, k1);
                *v = c[at + j] * (1.0 - w) + *v * w;
            }
            let k2 = k.min(at);
            for j in 0..k2 {
                let w = rise(j, k2);
                let i = m - k2 + j;
                ins[i] = ins[i] * (1.0 - w) + c[at - k2 + j] * w;
            }
            c.splice(at..at, ins);
        }
        s.shift_after(at, m as isize);
        let pasted: Vec<_> = clip.cues.iter().map(|q| (q.name.clone(), q.pos + at, q.len)).collect();
        for (name, pos, len) in pasted {
            s.add_cue(&name, pos, len);
        }
        if s.looped.is_none() {
            s.looped = clip.looped.map(|(a, b)| (a + at, b + at));
        }
        s.quantize();
        self.sel = Some((at, at + m - 1));
        self.version += 1;
    }

    // ------------------------------------------------------------------------------------------- regions

    /// Add region (the selection) or, with `len` false, add marker (at the selection start / cursor).
    pub fn add_cue(&mut self, name: &str, region: bool) -> bool {
        if self.frames() == 0 {
            return false;
        }
        let (pos, len) = match (region, self.range()) {
            (true, Some((a, b))) => (a, Some(b + 1 - a)),
            _ => (self.insert_at().min(self.frames() - 1), None),
        };
        let s = self.edit(if region { "Add region" } else { "Add marker" });
        s.add_cue(name, pos, len);
        true
    }

    /// The loop becomes the selection, or the whole sample with no selection.
    pub fn set_loop(&mut self) -> bool {
        let Some((a, b)) = self.range() else { return false };
        let s = self.edit("Set loop");
        s.looped = Some((a, b));
        true
    }

    pub fn clear_loop(&mut self) -> bool {
        if self.sample.looped.is_none() {
            return false;
        }
        self.edit("Remove loop").looped = None;
        true
    }

    /// The marker or region at frame `pos` (its flag or its range), the latest made first.
    pub fn cue_at(&self, pos: usize) -> Option<u32> {
        self.sample.cues.iter().rev().find(|c| if c.is_region() { pos >= c.pos && pos <= c.end() } else { c.pos == pos }).map(|c| c.id)
    }

    pub fn delete_cue(&mut self, id: u32) -> bool {
        if !self.sample.cues.iter().any(|c| c.id == id) {
            return false;
        }
        self.edit("Delete region").cues.retain(|c| c.id != id);
        true
    }

    pub fn rename_cue(&mut self, id: u32, name: &str) -> bool {
        if !self.sample.cues.iter().any(|c| c.id == id) {
            return false;
        }
        let s = self.edit("Rename region");
        if let Some(c) = s.cues.iter_mut().find(|c| c.id == id) {
            c.name = name.to_string();
        }
        true
    }

    /// Replaces the regions (markers stay) with one from each start to the next (the last to the sample's end), named
    /// `{prefix} 1`, `{prefix} 2`..., and sets the tempo when given: one undo step. The regions view turns on.
    pub fn slice_regions(&mut self, name: &str, starts: &[usize], prefix: &str, tempo: Option<f32>) -> usize {
        let n = self.frames();
        let mut st: Vec<usize> = starts.iter().copied().filter(|&f| f < n).collect();
        st.sort_unstable();
        st.dedup();
        if st.is_empty() {
            return 0;
        }
        let s = self.edit(name);
        s.cues.retain(|c| !c.is_region());
        for (i, &a) in st.iter().enumerate() {
            let b = st.get(i + 1).copied().unwrap_or(n);
            s.add_cue(&format!("{prefix} {}", i + 1), a, Some(b - a));
        }
        if let Some(t) = tempo {
            s.tempo = t;
        }
        self.view.regions = true;
        st.len()
    }

    /// The sample's two channels for the detectors (a mono sample's one, twice).
    fn pair(s: &Sample) -> (&[f32], &[f32]) {
        (&s.ch[0], &s.ch[s.ch.len().min(2) - 1])
    }

    /// Regions > Auto-slice > Dull / Medium / Sharp auto-slicing (`k` 0 to 2): Lemon's transient detector over the
    /// whole sample, each start moved to a zero crossing; the slices replace the regions. The number of slices.
    pub fn auto_slice(&mut self, k: usize) -> usize {
        if self.frames() == 0 {
            return 0;
        }
        let s = self.sample.clone();
        let (l, r) = Self::pair(&s);
        let mut st = transients::detect(l, r, s.rate, transients::AUTO_DB[k.min(2)]);
        transients::zero_cross(&mut st, l, r, s.rate);
        st[0] = 0;
        self.slice_regions(AUTO_SLICING[k.min(2)], &st, "Slice", None)
    }

    /// The tempo the grid slices by: the sample's, else the one Detect beats would find, else a whole number of bars.
    pub fn grid_tempo(&self) -> f64 {
        let s = &self.sample;
        if s.tempo > 0.0 {
            return s.tempo as f64;
        }
        let (l, r) = Self::pair(s);
        transients::find_beats(l, r, s.rate).map_or_else(|| transients::guess_tempo(self.frames(), s.rate).1 as f64, |b| b.bpm)
    }

    /// Regions > Auto-slice > Small / Medium / Large grid-slicing (`k` 0 to 2): slices of a quarter, a half or a whole
    /// beat at [`grid_tempo`](Self::grid_tempo), from the sample's start. The number of slices.
    pub fn grid_slice(&mut self, k: usize) -> usize {
        let n = self.frames();
        if n == 0 {
            return 0;
        }
        let step = 60.0 / self.grid_tempo() * self.sample.rate as f64 / [4.0, 2.0, 1.0][k.min(2)];
        let st: Vec<usize> = (0..).map(|i| (i as f64 * step).round() as usize).take_while(|&f| f < n).collect();
        self.slice_regions(GRID_SLICING[k.min(2)], &st, "Slice", None)
    }

    /// Regions > Detect beats: the sample's tempo and beats (`os_dsp::transients::find_beats`); the tempo is set and a
    /// region per beat replaces the regions. The tempo and the number of beats, `None` when no beats were found.
    pub fn detect_beats(&mut self) -> Option<(f64, usize)> {
        let n = self.frames();
        let s = self.sample.clone();
        let (l, r) = Self::pair(&s);
        let b = transients::find_beats(l, r, s.rate)?;
        let bpm = (b.bpm * 1000.0).round() / 1000.0;
        let k = self.slice_regions("Detect beats", &b.frames(n), "Beat", Some(bpm as f32));
        (k > 0).then_some((bpm, k))
    }

    /// Regions > Zero-cross check all regions: each region's start and end move to the nearest zero crossing within
    /// 2 ms (the end as the frame after it); a start at the sample's start and an end at its end stay. Whether anything
    /// moved.
    pub fn zero_cross_regions(&mut self) -> bool {
        let n = self.frames();
        let s = self.sample.clone();
        let (l, r) = Self::pair(&s);
        let mut moved = Vec::new();
        for c in s.cues.iter().filter(|c| c.is_region()) {
            let mut ends = [c.pos, c.end() + 1];
            transients::zero_cross(&mut ends, l, r, s.rate);
            // The sample's own start and end stay.
            let a = if c.pos == 0 { 0 } else { ends[0].min(n - 1) };
            let b = if c.end() + 1 >= n { n } else { ends[1].min(n) };
            if (a, b) != (c.pos, c.end() + 1) && b > a {
                moved.push((c.id, a, b - a));
            }
        }
        if moved.is_empty() {
            return false;
        }
        let s = self.edit("Zero-cross check all regions");
        for (id, pos, len) in moved {
            if let Some(c) = s.cues.iter_mut().find(|c| c.id == id) {
                c.pos = pos;
                c.len = Some(len);
            }
        }
        true
    }

    /// Auto rename all: regions "Region 1", "Region 2", markers "Marker 1"... by position.
    pub fn auto_rename(&mut self) -> bool {
        if self.sample.cues.is_empty() {
            return false;
        }
        let order: Vec<u32> = self.sample.cues_by_pos().iter().map(|c| c.id).collect();
        let s = self.edit("Auto rename all");
        let (mut r, mut m) = (0, 0);
        for id in order {
            if let Some(c) = s.cues.iter_mut().find(|c| c.id == id) {
                if c.is_region() {
                    r += 1;
                    c.name = format!("Region {r}");
                } else {
                    m += 1;
                    c.name = format!("Marker {m}");
                }
            }
        }
        true
    }

    /// Moves a marker (or a region's start, keeping its length) to `pos`: dragging a flag.
    pub fn move_cue(&mut self, id: u32, pos: usize, record: bool) {
        let n = self.frames();
        if n == 0 {
            return;
        }
        let s = if record { self.edit("Move region") } else {
            self.version += 1;
            Arc::make_mut(&mut self.sample)
        };
        if let Some(c) = s.cues.iter_mut().find(|c| c.id == id) {
            let len = c.len.unwrap_or(1);
            c.pos = pos.min(n - len.min(n));
        }
    }

    // ------------------------------------------------------------------------------------------- envelopes

    /// The envelope the edit mode shows (`None` in waveform mode).
    pub fn env_shown(&self) -> Option<usize> {
        (1..=4).contains(&self.view.mode).then(|| self.view.mode as usize - 1)
    }

    /// Records an undo step named `name` and returns the envelopes for editing (the sample isn't copied).
    pub fn edit_env(&mut self, name: &str) -> &mut Envs {
        self.push_step(name);
        &mut self.env
    }

    /// Changes the envelopes without an undo step (a point being dragged after its step was recorded).
    pub fn env_mut(&mut self) -> &mut Envs {
        &mut self.env
    }

    /// Tools > Cancel all envelopes: the four back to the flat line.
    pub fn cancel_envelopes(&mut self) -> bool {
        if self.env == Envs::default() {
            return false;
        }
        *self.edit_env("Cancel all envelopes") = Envs::default();
        true
    }

    /// Runs an envelope tool on the shown envelope. False in waveform mode or with an empty sample.
    pub fn env_tool(&mut self, name: &str, f: impl FnOnce(&mut Envs, usize, usize) -> bool) -> bool {
        let (Some(k), n) = (self.env_shown(), self.frames()) else { return false };
        if n == 0 {
            return false;
        }
        let mut e = self.env.clone();
        if !f(&mut e, k, n) || e == self.env {
            return false;
        }
        *self.edit_env(name) = e;
        true
    }

    /// Tools > Add points at selection (its two ends) or at regions (each region's two ends).
    pub fn env_points_at(&mut self, regions: bool) -> bool {
        let at: Vec<f64> = if regions {
            self.regions().iter().flat_map(|&(a, b)| [a as f64, b as f64]).collect()
        } else {
            self.sel.map(|(a, b)| vec![a as f64, b as f64]).unwrap_or_default()
        };
        if at.is_empty() {
            return false;
        }
        let name = if regions { "Add points at regions" } else { "Add points at selection" };
        self.env_tool(name, |e, k, n| {
            e.add_points(k, n, &at);
            true
        })
    }

    /// Tools > Mixdown amp envelopes: the panning, volume and stereo separation envelopes applied to the whole sample
    /// (one undo step), then back to the flat line. False while they are flat.
    pub fn mixdown(&mut self) -> bool {
        if self.frames() == 0 || self.env.amp_flat() {
            return false;
        }
        let envs = self.env.clone();
        let s = self.edit("Mixdown amp envelopes");
        env::mixdown(s, &envs);
        s.quantize();
        for k in [env::PAN, env::VOLUME, env::SEP] {
            self.env.e[k].clear();
        }
        true
    }

    // ------------------------------------------------------------------------------------------- select

    /// The regions' ranges by position.
    fn regions(&self) -> Vec<(usize, usize)> {
        let mut v: Vec<(usize, usize)> = self.sample.cues.iter().filter(|c| c.is_region()).map(|c| (c.pos, c.end())).collect();
        v.sort();
        v
    }

    pub fn select_before(&mut self) -> bool {
        match self.sel {
            Some((a, _)) if a > 0 => {
                self.sel = Some((0, a - 1));
                true
            }
            _ => false,
        }
    }

    pub fn select_after(&mut self) -> bool {
        let n = self.frames();
        match self.sel {
            Some((_, b)) if b + 1 < n => {
                self.sel = Some((b + 1, n - 1));
                true
            }
            _ => false,
        }
    }

    /// Select previous (`dir` < 0) or next region from the selection (or the cursor).
    pub fn select_region(&mut self, dir: i32) -> bool {
        let here = self.sel.map_or(self.cursor, |(a, _)| a);
        let r = self.regions();
        let pick = if dir < 0 { r.iter().rev().find(|(a, _)| *a < here) } else { r.iter().find(|(a, _)| *a > here || (self.sel.is_none() && *a == here)) };
        match pick {
            Some(&(a, b)) => {
                self.sel = Some((a, b.min(self.frames().saturating_sub(1))));
                true
            }
            None => false,
        }
    }
}

/// `clip` at `rate` with `channels` channels: resampled linearly when its rate differs, mono doubled or stereo
/// averaged when the channel count differs.
pub fn conform(clip: &Sample, rate: u32, channels: usize) -> Sample {
    let mut c = clip.clone();
    if c.rate != rate && c.frames() > 1 {
        let ratio = c.rate as f64 / rate as f64;
        let n = ((c.frames() as f64) / ratio).round().max(1.0) as usize;
        for ch in &mut c.ch {
            let src = std::mem::take(ch);
            *ch = (0..n)
                .map(|i| {
                    let x = i as f64 * ratio;
                    let k = (x as usize).min(src.len() - 1);
                    let t = (x - k as f64) as f32;
                    let a = src[k];
                    let b = *src.get(k + 1).unwrap_or(&a);
                    a + (b - a) * t
                })
                .collect();
        }
        for q in &mut c.cues {
            q.pos = (q.pos as f64 / ratio) as usize;
            q.len = q.len.map(|l| ((l as f64 / ratio) as usize).max(1));
        }
        c.looped = c.looped.map(|(a, b)| ((a as f64 / ratio) as usize, (b as f64 / ratio) as usize));
        c.rate = rate;
        c.clip_marks();
    }
    tools::set_channels(&mut c, channels);
    c
}

/// A value as the sample's format keeps it.
pub fn keep(f: Format, v: f32) -> f32 {
    if f == Format::Int16 { q16(v) } else { v }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(n: usize) -> Doc {
        let mut d = Doc::default();
        let l: Vec<f32> = (0..n).map(|i| i as f32).collect();
        d.sample = Arc::new(Sample::from_channels(44100, vec![l.clone(), l], Format::Float32));
        d.view.clickfree = false;
        d
    }

    #[test]
    fn cut_paste_and_undo() {
        let mut d = doc(10);
        d.select(2, 4);
        let c = d.cut().unwrap();
        assert_eq!(c.ch[0], vec![2.0, 3.0, 4.0]);
        assert_eq!(d.sample.ch[0], vec![0.0, 1.0, 5.0, 6.0, 7.0, 8.0, 9.0]);
        d.cursor = 0;
        d.paste(&c, Paste::Insert);
        assert_eq!(d.sample.ch[0][..4], [2.0, 3.0, 4.0, 0.0]);
        assert_eq!(d.sel, Some((0, 2)));
        assert_eq!(d.history(), vec!["Paste insert", "Cut"]);
        d.undo();
        d.undo();
        assert_eq!(d.sample.ch[0], (0..10).map(|i| i as f32).collect::<Vec<_>>());
        d.redo();
        assert_eq!(d.frames(), 7);
    }

    #[test]
    fn replace_mix_clear_trim_silence() {
        let mut d = doc(10);
        let clip = Sample::from_channels(44100, vec![vec![100.0, 100.0]], Format::Float32);
        d.select(1, 3);
        d.paste(&clip, Paste::Replace);
        assert_eq!(d.sample.ch[1], vec![0.0, 100.0, 100.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0]);
        assert_eq!(d.history(), vec!["Paste replace"]);
        d.sel = None;
        d.cursor = 8;
        d.paste(&clip, Paste::Mix);
        assert_eq!(d.sample.ch[0][8..], [109.0, 100.0]);
        d.select(0, 1);
        d.clear();
        assert_eq!(d.sample.ch[0][..3], [0.0, 0.0, 100.0]);
        d.select(2, 4);
        d.trim();
        assert_eq!(d.sample.ch[0], vec![100.0, 4.0, 5.0]);
        d.cursor = 1;
        d.sel = None;
        d.insert_silence(2);
        assert_eq!(d.sample.ch[0], vec![100.0, 0.0, 0.0, 4.0, 5.0]);
    }

    #[test]
    fn click_free_joints_have_no_step() {
        let mut d = doc(0);
        let n = 44100;
        let l: Vec<f32> = (0..n).map(|i| if i < n / 2 { 1.0 } else { -1.0 }).collect();
        d.sample = Arc::new(Sample::from_channels(44100, vec![l], Format::Float32));
        d.view.clickfree = true;
        // Deleting a short range across the step leaves a crossfade, not a jump.
        d.select(n / 2 - 1000, n / 2 - 1);
        d.delete();
        let c = &d.sample.ch[0];
        assert!(c.windows(2).all(|w| (w[1] - w[0]).abs() < 0.02));
    }

    #[test]
    fn zero_cross_check_moves_region_ends_to_crossings() {
        let mut d = Doc::default();
        // A 441 Hz sine: crossings every 50 frames.
        let l: Vec<f32> = (0..4410).map(|i| (std::f64::consts::TAU * i as f64 / 100.0).sin() as f32).collect();
        d.sample = Arc::new(Sample::from_channels(44100, vec![l], Format::Float32));
        d.select(1010, 2029);
        d.add_cue("rg", true);
        d.select(0, 4409);
        d.add_cue("all", true);
        assert!(d.zero_cross_regions());
        let r: Vec<(usize, usize)> = d.sample.cues.iter().map(|c| (c.pos, c.end())).collect();
        assert_eq!(r, vec![(1001, 2050), (0, 4409)], "to where the sine turns positive; the whole-sample region stays");
        assert_eq!(d.history()[0], "Zero-cross check all regions");
        assert!(!d.zero_cross_regions(), "nothing left to move");
    }

    #[test]
    fn regions_markers_loop_and_select() {
        let mut d = doc(1000);
        d.select(100, 199);
        d.add_cue("rg1", true);
        d.sel = None;
        d.cursor = 500;
        d.add_cue("mk1", false);
        d.select(600, 699);
        d.add_cue("rg2", true);
        assert_eq!(d.sample.cues.iter().map(|c| (c.id, c.pos, c.len)).collect::<Vec<_>>(), vec![(1, 100, Some(100)), (2, 500, None), (3, 600, Some(100))]);
        d.sel = None;
        d.set_loop();
        assert_eq!(d.sample.looped, Some((0, 999)));
        d.cursor = 0;
        assert!(d.select_region(1));
        assert_eq!(d.sel, Some((100, 199)));
        assert!(d.select_region(1));
        assert_eq!(d.sel, Some((600, 699)));
        assert!(d.select_region(-1));
        assert_eq!(d.sel, Some((100, 199)));
        assert!(d.select_after());
        assert_eq!(d.sel, Some((200, 999)));
        assert_eq!(d.cue_at(150), Some(1));
        d.rename_cue(1, "x");
        d.auto_rename();
        assert_eq!(d.sample.cues[0].name, "Region 1");
        assert_eq!(d.sample.cues[1].name, "Marker 1");
        d.delete_cue(2);
        assert_eq!(d.sample.cues.len(), 2);
        // Normalize all regions touches only them.
        d.tool_regions("Normalize all regions", |s, a, b| {
            tools::normalize(s, a, b);
        });
        assert_eq!(d.sample.ch[0][199], 1.0);
        assert_eq!(d.sample.ch[0][300], 300.0);
    }

    #[test]
    fn envelopes_undo_and_mix_down() {
        let mut d = doc(0);
        d.sample = Arc::new(Sample::from_channels(44100, vec![vec![0.5; 101], vec![0.5; 101]], Format::Float32));
        // Waveform mode: no envelope tools.
        assert!(!d.env_tool("Flip vertically", |e, k, n| {
            e.flip(k, n);
            true
        }));
        assert!(!d.mixdown());
        d.view.mode = 2;
        assert_eq!(d.env_shown(), Some(env::VOLUME));
        d.select(20, 80);
        assert!(d.env_points_at(false));
        let n = d.frames();
        d.edit_env("Move point").move_point(env::VOLUME, n, 1, 20.0, 0.0);
        d.env_mut().move_point(env::VOLUME, n, 2, 50.0, 0.0);
        d.env_mut().move_point(env::VOLUME, n, 3, 80.0, 0.0);
        assert_eq!(d.history(), vec!["Move point", "Add points at selection"]);
        // Undo and redo carry the envelopes, and leave the sample's Arc alone.
        let before = d.sample.clone();
        d.undo();
        assert_eq!(d.env.points(env::VOLUME, n).len(), 5);
        assert!(d.env.is_flat(env::VOLUME));
        assert!(Arc::ptr_eq(&before, &d.sample));
        d.redo();
        assert!(!d.env.is_flat(env::VOLUME));
        // The mixdown silences 20..=80 (straight lines down into it), then the amp envelopes go flat.
        assert!(d.mixdown());
        assert_eq!(d.sample.ch[0][50], 0.0);
        assert_eq!(d.sample.ch[1][0], 0.5);
        assert!(d.sample.ch[0][10] > 0.0 && d.sample.ch[0][10] < 0.5);
        assert!(d.env.amp_flat());
        assert!(!d.mixdown());
        d.undo();
        assert_eq!(d.sample.ch[0][50], 0.5);
        assert!(!d.env.is_flat(env::VOLUME), "undo brings the envelope back");
        assert!(d.cancel_envelopes());
        assert_eq!(d.env, Envs::default());
        assert!(!d.cancel_envelopes());
    }

    #[test]
    fn conform_rate_and_channels() {
        let c = Sample::from_channels(22050, vec![vec![0.0, 1.0, 2.0]], Format::Float32);
        let r = conform(&c, 44100, 2);
        assert_eq!(r.channels(), 2);
        assert_eq!(r.frames(), 6);
        assert_eq!(r.ch[1][1], 0.5);
    }
}
