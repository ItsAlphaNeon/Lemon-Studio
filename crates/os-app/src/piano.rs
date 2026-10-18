// Copyright (C) 2026 ItsAlphaNeon
// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Piano roll state and note editing (view geometry, selection, undo).
// spec: ui/piano-roll.md §3, §5, §7a, §7b, §8, §9; flp/events/patterns.md (note records, control points)

use os_model::{ControlPoint, Note, Pattern};
use os_ui::Rect;

use crate::app::State;
use crate::views::piano::{ROW, TOP_KEY};

/// Piano-roll tools (§9), with their shortcut letters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tool {
    #[default]
    Draw,
    Paint,
    Delete,
    Mute,
    Slice,
    Select,
    Zoom,
    Playback,
}

impl Tool {
    pub fn from_letter(c: &str) -> Option<Tool> {
        Some(match c.to_ascii_lowercase().as_str() {
            "p" => Tool::Draw,
            "b" => Tool::Paint,
            "d" => Tool::Delete,
            "t" => Tool::Mute,
            "c" => Tool::Slice,
            "e" => Tool::Select,
            "z" => Tool::Zoom,
            "y" => Tool::Playback,
            _ => return None,
        })
    }

    /// Tools whose left-drag on a selected note moves the whole selection (§9).
    pub fn moves_groups(self) -> bool {
        matches!(self, Tool::Draw | Tool::Select)
    }
}

/// What a tool drag draws over the grid.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Overlay {
    /// Select tool: marquee (grid pixels).
    Marquee(Rect),
    /// Zoom tool: the box to zoom to.
    ZoomBox(Rect),
    /// Slice tool: the cut time (ticks).
    Slice(u32),
    /// Playback tool: the scrub line (ticks).
    Scrub(f32),
}

#[derive(Debug, Clone)]
pub struct PianoState {
    /// Time scroll in ticks (left edge of the grid).
    pub scroll_x: f32,
    /// Key scroll in pixels from the top of the highest key's row.
    pub scroll_y: f32,
    /// Horizontal zoom (§3: 96 px per beat in the default floating window).
    pub px_per_beat: f32,
    /// Length of the last note added, moved or resized; 0 = not yet (§8: first note is PPQ/2).
    pub last_len: u32,
    /// The last note clicked or edited in the event panel: new notes take its controls (velocity, pan, release, cutoff,
    /// resonance, fine pitch, repeat, colour), as FL does; `None` = the defaults.
    pub last_controls: Option<Note>,
    /// The project changed since the playing engine last got it (notes, steps, mixer, channel
    /// settings, tempo); `App::tick` sends it to the transport.
    pub dirty: bool,
    /// Key held with the mouse (keyboard press or a dragged note), lit on the keyboard (§4).
    pub held: Option<i32>,
    /// Keys held on the typing keyboard as (channel index, key), lit too (ui/typing-keyboard.md).
    pub typed: Vec<(u16, i32)>,
    pub tool: Tool,
    /// Event panel target.
    pub lane: Lane,
    /// Cents at the middle of the channel pitch lane's window (its scroll position, §7b).
    pub pitch_center: i32,
    pub overlay: Option<Overlay>,
    /// Ctrl+C copies (positions as copied).
    pub clipboard: Vec<Note>,
    /// Zoom tool: the view before "zoom out" (px per beat, scroll x, scroll y), for right-click.
    pub zoom_back: Option<(f32, f32, f32)>,
    /// Play-start marker in the ruler (ticks; pattern playback starts here, §10).
    pub marker: u32,
    /// Time selection made by right-dragging the ruler (ticks, start..end); the only selection
    /// that shades the grid (§10).
    pub range: Option<(u32, u32)>,
    /// Key row pitch in pixels (vertical zoom, §3a; the measured default is [`ROW`]).
    pub row_h: f32,
    /// Ghost channels: other channels' notes of the pattern drawn dimmed (§5, Alt+V).
    pub ghosts: bool,
    /// Random state of the Randomize and humanize tools (§11).
    pub rng: u64,
    /// The grid / event panel split (a program setting, spec: ui/event-panel-splitter.md).
    pub split: crate::piano_split::Split,
    /// Last press on the splitter, for its double click.
    pub split_click: Option<std::time::Instant>,
    /// Whether the window was shown at the last tick (`None` = not yet seen), to notice it opening ([`open_tick`]).
    pub was_shown: Option<bool>,
    /// The magnet button's setting (spec: ui/piano-snap.md): a program setting, kept in the user folder (crate::snap).
    pub snap: crate::snap::EditorSnap,
    sel: Vec<usize>,
    sel_key: (u16, u16),
}

impl Default for PianoState {
    fn default() -> Self {
        PianoState {
            scroll_x: 0.0,
            scroll_y: ((TOP_KEY - 72) as f32) * ROW,
            px_per_beat: 96.0,
            last_len: 0,
            last_controls: None,
            dirty: false,
            held: None,
            typed: Vec::new(),
            tool: Tool::Draw,
            lane: Lane::Velocity,
            pitch_center: 0,
            overlay: None,
            clipboard: Vec::new(),
            zoom_back: None,
            marker: 0,
            range: None,
            row_h: ROW,
            ghosts: true,
            rng: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_nanos() as u64) | 1,
            split: crate::piano_split::load(),
            split_click: None,
            was_shown: None,
            snap: crate::snap::EditorSnap::default(),
            sel: Vec::new(),
            sel_key: (0, 0),
        }
    }
}

impl PianoState {
    /// Selected note indices (into the pattern's notes) for this pattern and channel.
    pub fn selection(&self, pattern: u16, channel: Option<u16>) -> Vec<usize> {
        match channel {
            Some(c) if self.sel_key == (pattern, c) => self.sel.clone(),
            _ => Vec::new(),
        }
    }

    pub fn select(&mut self, pattern: u16, channel: u16, sel: Vec<usize>) {
        self.sel_key = (pattern, channel);
        self.sel = sel;
        self.range = None;
    }

    pub fn clear_selection(&mut self) {
        self.sel.clear();
        self.range = None;
    }

    /// Forget everything tied to the previous project.
    pub fn reset(&mut self) {
        *self = PianoState { px_per_beat: self.px_per_beat, row_h: self.row_h, ghosts: self.ghosts, tool: self.tool, rng: self.rng, split: self.split, snap: self.snap, ..PianoState::default() };
    }
}

/// Mapping between grid pixels and ticks / keys for one frame.
pub struct Geom {
    pub grid: Rect,
    pub ppt: f32,
    pub scroll_x: f32,
    pub scroll_y: f32,
    /// Key row pitch in pixels (§3a).
    pub row: f32,
    pub step: u32,
    /// Beat and bar of the pattern's grid at its start.
    pub beat: u32,
    pub bar: u32,
    /// The pattern's own bar grid, `(start, bar, beat)` per time-signature region (`Project::pattern_bars`): its
    /// time-signature markers set its bars and beats (3/8: a bar every 6 steps, spec: ui/pattern-time-signature.md §2).
    pub bars: Vec<(u32, u32, u32)>,
}

impl Geom {
    pub fn new(s: &State, grid: Rect) -> Geom {
        let ppq = u32::from(s.project.ppq.max(4));
        // A beat is the signature's note value: a quarter in 4/4, an eighth in 3/8.
        let bars: Vec<(u32, u32, u32)> =
            s.project.pattern_bars(s.pattern).into_iter().map(|(start, bar, (num, _))| (start, bar, (bar / u32::from(num.max(1))).max(1))).collect();
        Geom {
            grid,
            ppt: s.piano.px_per_beat / ppq as f32,
            scroll_x: s.piano.scroll_x,
            scroll_y: s.piano.scroll_y,
            row: s.piano.row_h,
            step: ppq / 4,
            beat: bars[0].2,
            bar: bars[0].1,
            bars,
        }
    }

    /// Whether `tick` is on a bar line and on a beat line of the pattern's grid.
    pub fn line_at(&self, tick: u32) -> (bool, bool) {
        let &(start, bar, beat) = self.bars.iter().rev().find(|r| r.0 <= tick).unwrap_or(&self.bars[0]);
        let d = tick.saturating_sub(start);
        (d % bar == 0, d % beat == 0)
    }

    /// The bar lines before `end` ticks, with their bar numbers from 1.
    pub fn bar_lines(&self, end: f32) -> Vec<(u32, u32)> {
        let mut out = Vec::new();
        for (i, &(start, bar, _)) in self.bars.iter().enumerate() {
            let next = self.bars.get(i + 1).map_or(u32::MAX, |r| r.0);
            let mut t = start;
            while t < next && (t as f32) < end {
                out.push((t, out.len() as u32 + 1));
                t = t.saturating_add(bar);
            }
        }
        out
    }

    pub fn x_of(&self, tick: f32) -> f32 {
        self.grid.x + (tick - self.scroll_x) * self.ppt
    }

    pub fn tick_at(&self, x: f32) -> f32 {
        self.scroll_x + (x - self.grid.x) / self.ppt
    }

    /// Top of `key`'s row (`row - 1` px of fill, then the separator).
    pub fn y_of(&self, key: i32) -> f32 {
        self.grid.y + (TOP_KEY - key) as f32 * self.row - self.scroll_y
    }

    pub fn key_at(&self, y: f32) -> i32 {
        TOP_KEY - ((y - self.grid.y + self.scroll_y) / self.row).floor() as i32
    }

    /// Keys with at least one visible pixel, lowest first.
    pub fn visible_keys(&self) -> (i32, i32) {
        let hi = self.key_at(self.grid.y).min(TOP_KEY);
        let lo = self.key_at(self.grid.bottom() - 0.01).max(0);
        (lo, hi)
    }

    /// A note covers its row plus the separators above and below (§5).
    pub fn note_rect(&self, n: &Note) -> Option<Rect> {
        let key = i32::from(n.key);
        if key > TOP_KEY {
            return None;
        }
        let len = if n.length == 0 { self.step } else { n.length };
        let x = self.x_of(n.position as f32);
        let w = (len as f32 * self.ppt).max(3.0);
        Some(Rect::new(x, self.y_of(key) - 1.0, w, self.row + 1.0))
    }
}

/// Where the current pattern plays in the song: each clip of it that sounds (not muted, on a track that is on), as
/// (song tick of the clip's start, pattern tick there, length), in song order. A clip that plays its whole pattern
/// (no window) lasts the pattern's length.
pub fn song_spans(s: &State) -> Vec<(u32, u32, u32)> {
    let whole = pattern_length(s, s.pattern);
    let track_on = |t: u16| s.project.arrangement().and_then(|a| a.tracks.get(usize::from(t))).is_none_or(|t| t.enabled);
    let mut v: Vec<(u32, u32, u32)> = crate::playlist::clips(s)
        .iter()
        .filter(|c| c.source == os_model::ClipSource::Pattern(s.pattern) && !c.muted && track_on(c.track))
        .map(|c| match c.window {
            Some(w) => (c.position, w.0.max(0.0).round() as u32, c.length),
            None => (c.position, 0, whole),
        })
        .collect();
    v.sort_unstable();
    v
}

/// The pattern tick at song tick `t`, when a clip of the current pattern plays there (the latest-starting one).
pub fn pattern_tick_in_song(s: &State, t: f64) -> Option<f64> {
    song_spans(s)
        .into_iter()
        .rev()
        .find(|&(at, _, len)| t >= f64::from(at) && t < f64::from(at) + f64::from(len))
        .map(|(at, from, _)| f64::from(from) + (t - f64::from(at)))
}

/// The song tick where pattern tick `t` plays: in the clip that plays at song tick `near` if it shows `t`, else in the
/// first clip after `near` that does, else in the first one. None when no clip of the pattern shows `t`.
pub fn song_tick_of(s: &State, t: u32, near: u32) -> Option<u32> {
    let shows: Vec<(u32, u32, u32)> = song_spans(s).into_iter().filter(|&(_, from, len)| t >= from && t < from + len).collect();
    let place = |&(at, from, _): &(u32, u32, u32)| at + (t - from);
    shows
        .iter()
        .find(|&&(at, _, len)| near >= at && near < at + len)
        .or_else(|| shows.iter().find(|&&(at, _, _)| at >= near))
        .or_else(|| shows.first())
        .map(place)
}

/// Length used for drawing: a plain step (length 0) is shown one step long (Lemon Studio choice).
pub fn shown_len(s: &State, n: &Note) -> u32 {
    if n.length == 0 { u32::from(s.project.ppq.max(4)) / 4 } else { n.length }
}

/// Pattern `n`'s length in ticks as the engine loops it: the rack's explicit length, else its last note's end rounded
/// up to a bar line of its own time signature (`Project::pattern_length`, spec: audio/song-structure.md).
pub fn pattern_length(s: &State, n: u16) -> u32 {
    s.project.pattern_length(n)
}

/// What the event panel edits (spec: ui/piano-roll.md §7a, §7b). The first seven are note properties, the
/// last three edit the pattern's channel control points.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lane {
    Velocity,
    Pan,
    Release,
    Cutoff,
    Resonance,
    FinePitch,
    Repeat,
    ChanPan,
    ChanVolume,
    ChanPitch,
}

/// Cents spanned by the full height of the channel pitch lane (1.64 cents per pixel over 294 px; §7b).
const PITCH_SPAN: f32 = 482.0;
/// The pitch lane scrolls over ±4800 cents; an arrow click moves its window by 455 cents (§7b).
pub const PITCH_RANGE: i32 = 4800;
pub const PITCH_STEP: i32 = 455;

/// Window centre limits: the window may not leave the range.
pub fn clamp_pitch_center(c: i32) -> i32 {
    let half = (PITCH_SPAN / 2.0) as i32;
    c.clamp(-(PITCH_RANGE - half), PITCH_RANGE - half)
}
/// Stored value of the top of the channel volume / pan lanes (§7b).
const CTRL_MAX: i32 = 12800;

impl Lane {
    /// Menu order: note properties (the first `NOTE_LANES`), then channel controls.
    pub const ALL: [Lane; 10] = [
        Lane::Pan,
        Lane::Velocity,
        Lane::Release,
        Lane::Cutoff,
        Lane::Resonance,
        Lane::FinePitch,
        Lane::Repeat,
        Lane::ChanPan,
        Lane::ChanVolume,
        Lane::ChanPitch,
    ];
    pub const NOTE_LANES: usize = 7;
    /// Dimmed name over the panel.
    pub fn name(self) -> &'static str {
        match self {
            Lane::Velocity => "Velocity",
            Lane::Pan => "Pan",
            Lane::Release => "Release",
            Lane::Cutoff => "Filter cutoff",
            Lane::Resonance => "Filter resonance",
            Lane::FinePitch => "Fine pitch",
            Lane::Repeat => "Repeat",
            Lane::ChanPan => "Channel panning",
            Lane::ChanVolume => "Channel volume",
            Lane::ChanPitch => "Channel pitch",
        }
    }
    /// Entry in the Control menu.
    pub fn menu_name(self) -> &'static str {
        match self {
            Lane::Velocity => "Note velocity",
            Lane::Pan => "Note pan",
            Lane::Release => "Note release",
            Lane::Cutoff => "Note filter cutoff frequency",
            Lane::Resonance => "Note filter resonance (Q)",
            Lane::FinePitch => "Note fine pitch",
            Lane::Repeat => "Note repeat",
            Lane::ChanPan => "Channel panning",
            Lane::ChanVolume => "Channel volume",
            Lane::ChanPitch => "Channel pitch",
        }
    }
    /// Edits control points of the channel instead of note properties.
    pub fn is_channel(self) -> bool {
        matches!(self, Lane::ChanPan | Lane::ChanVolume | Lane::ChanPitch)
    }
    /// Parameter id of a channel lane in event 223 (spec: flp/events/patterns.md).
    pub fn param(self) -> Option<u16> {
        match self {
            Lane::ChanVolume => Some(ControlPoint::VOLUME),
            Lane::ChanPan => Some(ControlPoint::PAN),
            Lane::ChanPitch => Some(ControlPoint::PITCH),
            _ => None,
        }
    }
    pub fn get(self, n: &Note) -> u8 {
        match self {
            Lane::Velocity => n.velocity,
            Lane::Pan => n.pan,
            Lane::Release => n.release,
            Lane::Cutoff => n.mod_x,
            Lane::Resonance => n.mod_y,
            Lane::FinePitch => n.fine_pitch,
            Lane::Repeat => n.repeat,
            _ => 0,
        }
    }
    pub fn set(self, n: &mut Note, v: u8) {
        match self {
            Lane::Velocity => n.velocity = v,
            Lane::Pan => n.pan = v,
            Lane::Release => n.release = v,
            Lane::Cutoff => n.mod_x = v,
            Lane::Resonance => n.mod_y = v,
            Lane::FinePitch => n.fine_pitch = v,
            Lane::Repeat => n.repeat = v,
            _ => {}
        }
    }
    /// Value at the top of the panel.
    fn scale(self) -> f32 {
        match self {
            Lane::Velocity | Lane::Pan | Lane::Release => 128.0,
            Lane::Cutoff | Lane::Resonance => 256.0,
            Lane::FinePitch => 240.0,
            Lane::Repeat => 14.0,
            _ => 1.0,
        }
    }
    pub fn max(self) -> f32 {
        self.scale().min(255.0)
    }
    /// Bipolar lanes draw their stem from the centre line.
    pub fn center(self) -> Option<u8> {
        match self {
            Lane::Pan => Some(64),
            Lane::Cutoff | Lane::Resonance => Some(128),
            Lane::FinePitch => Some(120),
            _ => None,
        }
    }
    pub fn y_of(self, panel: Rect, v: u8) -> f32 {
        panel.bottom() - f32::from(v) / self.scale() * panel.h
    }
    pub fn value_at(self, panel: Rect, y: f32) -> u8 {
        ((panel.bottom() - y) / panel.h * self.scale()).round().clamp(0.0, self.max()) as u8
    }
    /// Channel lanes: the stored value at height `y` (volume, pan 0..=12800; pitch in cents).
    pub fn control_at(self, panel: Rect, y: f32, center: i32) -> i32 {
        match self {
            Lane::ChanPitch => {
                let v = center as f32 + (panel.y + panel.h / 2.0 - y) / panel.h * PITCH_SPAN;
                v.round().clamp(-(PITCH_RANGE as f32), PITCH_RANGE as f32) as i32
            }
            _ => ((panel.bottom() - y) / panel.h * CTRL_MAX as f32).round().clamp(0.0, CTRL_MAX as f32) as i32,
        }
    }
    /// Height of a stored channel-lane value.
    pub fn control_y(self, panel: Rect, v: i32, center: i32) -> f32 {
        match self {
            Lane::ChanPitch => panel.y + panel.h / 2.0 - (v - center) as f32 / PITCH_SPAN * panel.h,
            _ => panel.bottom() - v as f32 / CTRL_MAX as f32 * panel.h,
        }
    }
    /// Where the bars of a channel lane start: the bottom for volume, the middle line for pan, the zero
    /// line (possibly scrolled out of view) for pitch.
    pub fn control_base(self, panel: Rect, center: i32) -> f32 {
        match self {
            Lane::ChanVolume => panel.bottom(),
            Lane::ChanPitch => self.control_y(panel, 0, center),
            _ => panel.y + panel.h / 2.0,
        }
    }
}

/// Top of a velocity lollipop: `velocity / 128` of the panel height (§7).
pub fn velocity_y(panel: Rect, velocity: u8) -> f32 {
    panel.bottom() - f32::from(velocity) / 128.0 * panel.h
}

pub fn velocity_at(panel: Rect, y: f32) -> u8 {
    ((panel.bottom() - y) / panel.h * 128.0).round().clamp(0.0, 128.0) as u8
}

/// The current pattern, created if it has no data yet.
pub fn pattern_mut(s: &mut State) -> &mut Pattern {
    let number = s.pattern;
    match s.project.patterns.iter().position(|p| p.number == number) {
        Some(i) => &mut s.project.patterns[i],
        None => {
            s.project.patterns.push(Pattern { number, ..Pattern::default() });
            s.project.patterns.last_mut().expect("pushed")
        }
    }
}

pub fn notes(s: &State) -> &[Note] {
    s.project.patterns.iter().find(|p| p.number == s.pattern).map_or(&[], |p| &p.notes)
}

/// The channel index (`Note::channel`) the piano roll edits.
pub fn channel(s: &State) -> Option<u16> {
    s.project.channels.get(s.selected_channel).map(|c| c.index)
}

/// The current pattern's channel control points.
pub fn controls(s: &State) -> &[ControlPoint] {
    s.project.patterns.iter().find(|p| p.number == s.pattern).map_or(&[], |p| &p.controls)
}

/// Set (or replace) the control point of `channel`/`param` at `tick`, keeping the list ordered by
/// position (spec: flp/events/patterns.md, "Control points").
pub fn set_control(s: &mut State, channel: u16, param: u16, tick: u32, value: i32) {
    let point = ControlPoint::new(tick, channel, param, value);
    let p = pattern_mut(s);
    match p.controls.iter_mut().find(|c| c.position == tick && c.kind == point.kind) {
        Some(c) => c.value = value,
        None => {
            let at = p.controls.partition_point(|c| c.position <= tick);
            p.controls.insert(at, point);
        }
    }
    s.piano.dirty = true;
}

/// Delete the lane's points whose position lies in `from..=to`; returns how many went.
pub fn erase_controls(s: &mut State, channel: u16, param: u16, from: u32, to: u32) -> usize {
    let kind = ControlPoint::new(0, channel, param, 0).kind;
    let p = pattern_mut(s);
    let before = p.controls.len();
    p.controls.retain(|c| !(c.kind == kind && (from..=to).contains(&c.position)));
    let gone = before - p.controls.len();
    s.piano.dirty = true;
    gone
}

/// Remember the current pattern's notes and control points for undo: a step of the project's undo history
/// (`crate::history`).
pub fn snapshot(s: &mut State) {
    snapshot_as(s, "Piano roll edit");
}

/// `snapshot`, the step named `name` (as the browser's History lists it; spec: ui/settings-choices.md "Undo history:
/// the four checks (2026-10-16)").
pub fn snapshot_as(s: &mut State, name: &'static str) {
    s.piano.dirty = true;
    let snap = crate::history::Snap::pattern(s, s.pattern);
    crate::history::record(s, name, snap);
}

/// Undo the newest step of the project's history; returns false when there is nothing to undo.
#[cfg(test)]
pub fn undo(s: &mut State) -> bool {
    crate::history::undo(s).is_some()
}

/// The timebase changed from `from` to `to` ticks per quarter (`Project::set_ppq` rescaled the project): the scroll,
/// the last note length, the play-start marker, the time selection, the clipboard and the undo steps follow, so the
/// view stays on the same beats and an undo brings back nothing of the old timebase.
pub fn rescale(s: &mut State, from: u16, to: u16) {
    use os_model::{rescale_length, rescale_tick};
    let k = f32::from(to.max(1)) / f32::from(from.max(1));
    let p = &mut s.piano;
    p.scroll_x *= k;
    p.last_len = rescale_length(p.last_len, from, to);
    p.marker = rescale_tick(p.marker, from, to);
    p.range = p.range.map(|(a, b)| (rescale_tick(a, from, to), rescale_tick(b, from, to)));
    p.clipboard.iter_mut().for_each(|n| n.rescale(from, to));
    if let Some((_, x, _)) = &mut p.zoom_back {
        *x *= k;
    }
}

/// Add a note with the defaults of a step (patterns.md) at `tick`/`key`; returns its index.
pub fn add_note(s: &mut State, channel: u16, tick: u32, key: i32) -> usize {
    let ppq = u32::from(s.project.ppq.max(4));
    let length = if s.piano.last_len > 0 { s.piano.last_len } else { ppq / 2 };
    let mut note = Note { length, key: key.clamp(0, TOP_KEY) as u16, ..Note::step(channel, tick) };
    if let Some(t) = s.piano.last_controls {
        copy_controls(&t, &mut note);
    }
    let p = pattern_mut(s);
    p.notes.push(note);
    p.notes.len() - 1
}

/// "Monophonic painting in piano roll" (spec: ui/settings-choices.md "Miscellaneous: seven more checks"): with the
/// check on, painting note `i` (a draw-tool click, each note a paint drag lays) deletes the notes of other keys of its
/// channel whose time overlaps it (start before its end, end after its start; a step counts one step long). Notes on
/// its own key are left to the paint rules. Run after the paint's snapshot, so the deletion undoes with the paint.
/// Returns note `i`'s index after the deletion.
pub fn paint_mono(s: &mut State, i: usize) -> usize {
    if !crate::prefs::check(&crate::prefs::MONO_PAINTING) {
        return i;
    }
    let Some(&n) = notes(s).get(i) else { return i };
    let (start, end) = (n.position, n.position + shown_len(s, &n));
    let gone: Vec<usize> = notes(s)
        .iter()
        .enumerate()
        .filter(|&(j, m)| j != i && m.channel == n.channel && m.key != n.key && m.position < end && m.position + shown_len(s, m) > start)
        .map(|(j, _)| j)
        .collect();
    if gone.is_empty() {
        return i;
    }
    let p = pattern_mut(s);
    for &j in gone.iter().rev() {
        p.notes.remove(j);
    }
    s.piano.clear_selection();
    s.piano.dirty = true;
    i - gone.iter().filter(|&&j| j < i).count()
}

/// The controls a new note takes from the last one touched; never its place, length, key or mute.
pub fn copy_controls(from: &Note, to: &mut Note) {
    to.velocity = from.velocity;
    to.pan = from.pan;
    to.release = from.release;
    to.mod_x = from.mod_x;
    to.mod_y = from.mod_y;
    to.fine_pitch = from.fine_pitch;
    to.repeat = from.repeat;
    to.group = (to.group & Note::MUTED) | (from.group & !Note::MUTED);
}

/// Delete notes by index.
pub fn delete(s: &mut State, mut idx: Vec<usize>) {
    idx.sort_unstable();
    idx.dedup();
    let p = pattern_mut(s);
    for &i in idx.iter().rev() {
        if i < p.notes.len() {
            p.notes.remove(i);
        }
    }
    s.piano.clear_selection();
}

/// Restore storage order (sorted by position, patterns.md) after an edit, keeping the selection.
pub fn finish_edit(s: &mut State) {
    let (pattern, sel_key) = (s.pattern, s.piano.sel_key);
    let p = pattern_mut(s);
    let mut order: Vec<usize> = (0..p.notes.len()).collect();
    order.sort_by_key(|&i| (p.notes[i].position, p.notes[i].channel));
    if order.iter().enumerate().all(|(a, &b)| a == b) {
        return;
    }
    let sorted: Vec<Note> = order.iter().map(|&i| p.notes[i]).collect();
    p.notes = sorted;
    if sel_key.0 == pattern {
        let mut new_index = vec![0; order.len()];
        for (new, &old) in order.iter().enumerate() {
            new_index[old] = new;
        }
        s.piano.sel = s.piano.sel.iter().filter(|&&i| i < new_index.len()).map(|&i| new_index[i]).collect();
    }
}

/// This channel's note under (x, y), topmost (last drawn) first.
pub fn note_at(s: &State, g: &Geom, x: f32, y: f32) -> Option<usize> {
    let ch = channel(s);
    notes(s)
        .iter()
        .enumerate()
        .rev()
        .find(|(_, n)| Some(n.channel) == ch && g.note_rect(n).is_some_and(|r| r.contains(x, y)))
        .map(|(i, _)| i)
}

/// This channel's notes whose rect touches `r` (grid pixels).
pub fn notes_in(s: &State, g: &Geom, r: Rect) -> Vec<usize> {
    let ch = channel(s);
    notes(s)
        .iter()
        .enumerate()
        .filter(|(_, n)| Some(n.channel) == ch && g.note_rect(n).is_some_and(|nr| nr.intersect(&r).w > 0.0 && nr.intersect(&r).h > 0.0))
        .map(|(i, _)| i)
        .collect()
}

/// Select this channel's notes starting inside the time range `a..b` and remember the range (§10).
pub fn select_range(s: &mut State, a: u32, b: u32) {
    let Some(ch) = channel(s) else { return };
    let sel = notes(s).iter().enumerate().filter(|(_, n)| n.channel == ch && n.position >= a && n.position < b).map(|(i, _)| i).collect();
    s.piano.select(s.pattern, ch, sel);
    s.piano.range = if b > a { Some((a, b)) } else { None };
}

/// The selection, or every note of the channel when nothing is selected (§9 selection editing).
pub fn targets(s: &State) -> Vec<usize> {
    let ch = channel(s);
    let sel = s.piano.selection(s.pattern, ch);
    if !sel.is_empty() {
        return sel;
    }
    notes(s).iter().enumerate().filter(|(_, n)| Some(n.channel) == ch).map(|(i, _)| i).collect()
}

/// Move notes by `dt` ticks and `dk` keys; refused (false) when any would leave tick 0 / the key range.
pub fn shift(s: &mut State, idx: &[usize], dt: i64, dk: i32) -> bool {
    let ok = idx.iter().filter_map(|&i| notes(s).get(i)).all(|n| {
        i64::from(n.position) + dt >= 0 && (0..=TOP_KEY).contains(&(i32::from(n.key) + dk))
    });
    if !ok || idx.is_empty() {
        return false;
    }
    snapshot(s);
    let p = pattern_mut(s);
    for &i in idx {
        if let Some(n) = p.notes.get_mut(i) {
            n.position = (i64::from(n.position) + dt) as u32;
            n.key = (i32::from(n.key) + dk) as u16;
        }
    }
    finish_edit(s);
    true
}

/// Cut this channel's notes that span `tick` in two (Slice tool, §9). Returns how many were cut.
pub fn slice(s: &mut State, tick: u32) -> usize {
    let ch = channel(s);
    let cut: Vec<usize> = notes(s)
        .iter()
        .enumerate()
        .filter(|(_, n)| Some(n.channel) == ch && n.length > 0 && n.position < tick && tick < n.position + n.length)
        .map(|(i, _)| i)
        .collect();
    if cut.is_empty() {
        return 0;
    }
    snapshot(s);
    let p = pattern_mut(s);
    for &i in &cut {
        let n = p.notes[i];
        p.notes[i].length = tick - n.position;
        p.notes.push(Note { position: tick, length: n.position + n.length - tick, ..n });
    }
    s.piano.clear_selection();
    finish_edit(s);
    cut.len()
}

/// Paste `notes` so the earliest starts at `at`, on this channel; the pasted notes become the selection.
pub fn paste(s: &mut State, clip: &[Note], at: u32) {
    let Some(ch) = channel(s) else { return };
    let Some(first) = clip.iter().map(|n| n.position).min() else { return };
    snapshot(s);
    let p = pattern_mut(s);
    let start = p.notes.len();
    for n in clip {
        p.notes.push(Note { position: n.position - first + at, channel: ch, ..*n });
    }
    let end = p.notes.len();
    s.piano.select(s.pattern, ch, (start..end).collect());
    finish_edit(s);
}

/// Scroll vertically so the channel's notes (or C5) are centred in a grid `view_h` pixels tall.
pub fn center_keys(s: &mut State, view_h: f32) {
    let ch = channel(s);
    let keys: Vec<i32> = notes(s).iter().filter(|n| Some(n.channel) == ch).map(|n| i32::from(n.key)).collect();
    let center = match (keys.iter().min(), keys.iter().max()) {
        (Some(lo), Some(hi)) => (lo + hi) as f32 / 2.0,
        _ => 60.0,
    };
    let row = s.piano.row_h;
    let max = (TOP_KEY + 1) as f32 * row - view_h;
    s.piano.scroll_y = ((TOP_KEY as f32 - center) * row - view_h / 2.0 + row / 2.0).clamp(0.0, max.max(0.0)).round();
    s.piano.scroll_x = 0.0;
}

/// "Auto zoom in piano roll" (spec: ui/settings-choices.md "Miscellaneous"): run on every tick and before each frame
/// (`App::follow_settings`); when the window opens (hidden → shown) with the check on, [`auto_zoom`] on its note grid. Returns whether it zoomed. The
/// window shown when a project opens or the program starts is not an opening.
pub fn open_tick(s: &mut State) -> bool {
    let shown = s.shown(crate::app::Win::PianoRoll);
    let was = s.piano.was_shown.replace(shown);
    if !(shown && was == Some(false) && crate::prefs::check(&crate::prefs::AUTO_ZOOM_PIANO)) {
        return false;
    }
    let grid = crate::views::piano::grid_rect(s);
    if grid.w < 1.0 || grid.h < 1.0 {
        return false; // collapsed to its title bar: nothing to fit
    }
    auto_zoom(s, grid);
    true
}

/// Zoom on the pattern and the channel's notes in a note grid `grid`: from bar 1, the pattern's length plus half a
/// bar fills the width (FL: a 2-bar pattern shows 2.49 bars); the rows are the tallest that fit the channel's pitch
/// range plus four spare rows, whole pixels between [`MIN_ROW`] and the default × 1.25, and the range is centred. A
/// channel without notes: C5 centred at [`MIN_ROW`] rows.
pub fn auto_zoom(s: &mut State, grid: Rect) {
    let ppq = f32::from(s.project.ppq.max(4));
    let span = pattern_length(s, s.pattern) as f32 + s.project.bar_ticks() as f32 / 2.0;
    s.piano.px_per_beat = (grid.w / span * ppq).clamp(6.0, 1600.0);
    let ch = channel(s);
    let keys = notes(s).iter().filter(|n| Some(n.channel) == ch).map(|n| i32::from(n.key));
    let range = keys.clone().min().zip(keys.max());
    s.piano.row_h = match range {
        Some((lo, hi)) => (grid.h / (hi - lo + 1 + 4) as f32).floor().clamp(MIN_ROW, (ROW * 1.25).floor()),
        None => MIN_ROW,
    };
    s.piano.zoom_back = None;
    center_keys(s, grid.h);
}

pub fn clamp_scroll(s: &mut State, view_h: f32) {
    let max = ((TOP_KEY + 1) as f32 * s.piano.row_h - view_h).max(0.0);
    s.piano.scroll_y = s.piano.scroll_y.clamp(0.0, max).round();
    s.piano.scroll_x = s.piano.scroll_x.max(0.0);
}

/// Smallest and largest key row pitch of the vertical zoom (§3a).
pub const MIN_ROW: f32 = 6.0;
pub const MAX_ROW: f32 = 48.0;

/// Ctrl+middle-drag zooms the key rows by [`ROW_ZOOM_STEP`] per [`ROW_ZOOM_PX`] of vertical movement (§3a).
pub const ROW_ZOOM_STEP: f32 = 1.15;
pub const ROW_ZOOM_PX: f32 = 12.0;

/// Vertical zoom: set the key row pitch, keeping the key at window y `y` in place (§3a). Rows stay whole pixels
/// (clamped to [`MIN_ROW`]..[`MAX_ROW`]) so the grid lines stay sharp.
pub fn set_rows(s: &mut State, grid: Rect, y: f32, row: f32) {
    let old = s.piano.row_h;
    let new = row.round().clamp(MIN_ROW, MAX_ROW);
    let at = (y - grid.y).clamp(0.0, grid.h);
    let rows = (at + s.piano.scroll_y) / old;
    s.piano.row_h = new;
    s.piano.scroll_y = rows * new - at;
    clamp_scroll(s, grid.h);
}

/// Time zoom by `notches` wheel notches around grid x `x` (§3a).
pub fn zoom_time(s: &mut State, g: &Geom, x: f32, notches: f32) {
    let tick = g.tick_at(x);
    let p = &mut s.piano;
    p.px_per_beat = (p.px_per_beat * 1.25f32.powf(notches)).clamp(6.0, 1600.0);
    let ppt = p.px_per_beat / f32::from(s.project.ppq.max(4));
    p.scroll_x = (tick - (x - g.grid.x) / ppt).max(0.0);
}

#[cfg(test)]
mod tests {
    #[test]
    fn new_notes_copy_controls_not_place_or_mute() {
        use super::*;
        let mut from = Note { velocity: 33, pan: 10, release: 90, mod_x: 200, mod_y: 7, fine_pitch: 130, repeat: 3, group: 0x05, ..Note::step(0, 480) };
        from.group |= Note::MUTED;
        let mut to = Note { length: 48, key: 72, ..Note::step(1, 96) };
        copy_controls(&from, &mut to);
        assert_eq!((to.velocity, to.pan, to.release, to.mod_x, to.mod_y, to.fine_pitch, to.repeat), (33, 10, 90, 200, 7, 130, 3));
        assert_eq!(to.group, 0x05, "the colour comes along, the mute does not");
        assert_eq!((to.position, to.length, to.key, to.channel), (96, 48, 72, 1));
    }

    #[test]
    fn lanes_map_values_and_centres() {
        let panel = Rect::new(0.0, 100.0, 10.0, 200.0);
        for lane in Lane::ALL.iter().copied().take(Lane::NOTE_LANES) {
            for v in [0u8, 1, 14, 64, 100, 120] {
                let v = v.min(lane.max() as u8);
                assert_eq!(lane.value_at(panel, lane.y_of(panel, v)), v, "{lane:?} {v}");
            }
            if let Some(c) = lane.center() {
                assert!((lane.y_of(panel, c) - 200.0).abs() < 1.0, "{lane:?} centre sits mid-panel");
            }
        }
        let mut n = Note::step(0, 0);
        Lane::Cutoff.set(&mut n, 77);
        assert_eq!((n.mod_x, Lane::Cutoff.get(&n)), (77, 77));
    }

    /// Repeat lane (spec: ui/piano-roll.md §7a): 15 values in 21 px steps of a 294 px panel, clamped at 14.
    #[test]
    fn repeat_lane_steps() {
        let panel = Rect::new(0.0, 621.0, 100.0, 294.0);
        assert_eq!(Lane::Repeat.value_at(panel, panel.bottom()), 0);
        assert_eq!(Lane::Repeat.value_at(panel, panel.bottom() - 21.0 * 6.0), 6);
        assert_eq!(Lane::Repeat.value_at(panel, panel.y - 40.0), 14);
        let mut n = Note::step(0, 0);
        Lane::Repeat.set(&mut n, 9);
        assert_eq!(n.repeat, 9);
    }

    /// Channel lanes (spec: ui/piano-roll.md §7b): linear value <-> height; volume bars start at the bottom.
    #[test]
    fn channel_lanes_map_values() {
        let panel = Rect::new(0.0, 621.0, 100.0, 294.0);
        for lane in [Lane::ChanVolume, Lane::ChanPan] {
            assert_eq!(lane.control_at(panel, panel.bottom(), 0), 0);
            assert_eq!(lane.control_at(panel, panel.y, 0), 12800);
            assert_eq!(lane.control_at(panel, panel.bottom() - 147.0, 0), 6400);
            assert!((lane.control_y(panel, 6400, 0) - 768.0).abs() < 0.6);
        }
        assert_eq!(Lane::ChanVolume.control_base(panel, 0), panel.bottom());
        assert!((Lane::ChanPan.control_base(panel, 0) - 768.0).abs() < 0.6);
        // Pitch: 1.64 cents per pixel around the window centre.
        assert_eq!(Lane::ChanPitch.control_at(panel, 768.0, 0), 0);
        let c = Lane::ChanPitch.control_at(panel, 768.0 - 100.0, 0);
        assert!((c - 164).abs() <= 1, "{c}");
        // Scrolled: the same height reads a different value; the zero line moves.
        assert_eq!(Lane::ChanPitch.control_at(panel, 768.0, 1365), 1365);
        let y0 = Lane::ChanPitch.control_base(panel, 1365);
        assert!(y0 > panel.bottom(), "zero line scrolled out of view: {y0}");
        assert!((Lane::ChanPitch.control_y(panel, 1365, 1365) - 768.0).abs() < 0.6);
        // Values are limited to the lane's ±4800 cents.
        assert_eq!(Lane::ChanPitch.control_at(panel, -100000.0, 0), PITCH_RANGE);
        assert_eq!(clamp_pitch_center(99999), PITCH_RANGE - 241);
        assert_eq!(clamp_pitch_center(-99999), -(PITCH_RANGE - 241));
        assert_eq!(Lane::ChanPitch.param(), Some(ControlPoint::PITCH));
    }

    /// Control points keep position order, replace the same position and target, erase by range, undo.
    #[test]
    fn control_points_edit_and_undo() {
        let mut a = app();
        let s = &mut a.state;
        snapshot(s);
        set_control(s, 0, ControlPoint::VOLUME, 48, 100);
        set_control(s, 0, ControlPoint::PAN, 24, 6400);
        set_control(s, 0, ControlPoint::VOLUME, 48, 200);
        assert_eq!(controls(s).len(), 2);
        assert_eq!(controls(s)[0].position, 24);
        assert_eq!(controls(s)[1].value, 200);
        assert_eq!(erase_controls(s, 0, ControlPoint::VOLUME, 0, 100), 1);
        assert_eq!(controls(s).len(), 1);
        assert!(undo(s));
        assert!(controls(s).is_empty());
    }

    use super::*;
    use crate::app::App;

    fn app() -> App {
        let mut a = App::new(None);
        a.state.selected_channel = 4;
        a
    }

    #[test]
    fn slice_shift_paste_mute() {
        let mut a = app();
        let s = &mut a.state;
        let i = add_note(s, 4, 0, 60);
        pattern_mut(s).notes[i].length = 96;
        assert_eq!(slice(s, 48), 1);
        let ns: Vec<(u32, u32)> = notes(s).iter().map(|n| (n.position, n.length)).collect();
        assert_eq!(ns, vec![(0, 48), (48, 48)]);
        assert!(shift(s, &[0, 1], 24, 12));
        assert_eq!((notes(s)[0].position, notes(s)[0].key), (24, 72));
        assert!(!shift(s, &[0], -48, 0), "would go before tick 0");
        let clip = notes(s).to_vec();
        paste(s, &clip, 384);
        assert_eq!(notes(s).len(), 4);
        assert_eq!(s.piano.selection(s.pattern, Some(4)).len(), 2);
        assert_eq!(notes(s)[2].position, 384);
        let mut n = notes(s)[0];
        n.set_muted(true);
        assert!(n.muted() && n.group == Note::MUTED);
        n.set_muted(false);
        assert_eq!(n.group, 0);
    }

    #[test]
    fn add_sort_select_undo() {
        let mut a = app();
        let s = &mut a.state;
        snapshot(s);
        let i = add_note(s, 4, 192, 72);
        assert_eq!(notes(s)[i].length, 48, "first note is PPQ/2 long");
        assert_eq!(notes(s)[i].velocity, 100);
        snapshot(s);
        let j = add_note(s, 4, 0, 60);
        s.piano.select(s.pattern, 4, vec![j]);
        finish_edit(s);
        assert_eq!(notes(s)[0].position, 0, "sorted by position");
        assert_eq!(s.piano.selection(s.pattern, Some(4)), vec![0], "selection follows the sort");
        assert!(undo(s));
        assert_eq!(notes(s).len(), 1);
        assert!(undo(s));
        assert!(notes(s).is_empty());
        assert!(!undo(s));
    }

    #[test]
    fn geometry_round_trips() {
        let a = app();
        let g = Geom::new(&a.state, Rect::new(282.0, 147.0, 1600.0, 700.0));
        for key in [0, 59, 60, 72, 131] {
            let y = g.y_of(key) + 8.0;
            assert_eq!(g.key_at(y), key);
        }
        assert_eq!(crate::piano_snap::place(&a.state, &g, g.tick_at(g.x_of(100.0)), false), 96);
        assert_eq!(key_name_check(60), "C5");
    }

    /// A new project's empty channels play the built-in tone, and the pattern ends on the bar.
    #[test]
    fn new_project_notes_are_audible() {
        let mut a = app();
        add_note(&mut a.state, 4, 0, 72);
        let mut lib = os_engine::SampleLibrary::new(None);
        let mut e = os_engine::Engine::new(&a.state.project, &mut lib, 48000, os_engine::PlayMode::Pattern(1));
        let mut buf = vec![0.0f32; 2 * 512];
        e.process(&mut buf);
        assert_eq!(e.sounding().collect::<Vec<_>>(), vec![(4, 72)], "the note is reported while it plays");
        let mut peak = buf.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        while !e.past_end() {
            e.process(&mut buf);
            peak = buf.iter().fold(peak, |m, v| m.max(v.abs()));
        }
        assert!(peak > 0.05, "peak {peak}");
        // One bar at 130 BPM (PPQ 96): 4 beats = 88615 frames at 48 kHz; blocks of 512.
        assert!(e.position().abs_diff(88_615) < 512, "loop point {}", e.position());
    }

    /// An edit swapped in mid-pattern is heard right away: notes ahead of the playhead play, notes
    /// behind it are not retriggered.
    #[test]
    fn live_edits_continue_at_position() {
        let mut a = app();
        let mut lib = os_engine::SampleLibrary::new(None);
        let mode = os_engine::PlayMode::Pattern(1);
        add_note(&mut a.state, 4, 0, 60);
        let mut old = os_engine::Engine::new(&a.state.project, &mut lib, 48000, mode);
        let mut buf = vec![0.0f32; 2 * 512];
        for _ in 0..40 {
            old.process(&mut buf); // ~0.43 s: past beat 1 (0.46 s at 130 BPM is beat 2)
        }
        let pos = old.position();
        add_note(&mut a.state, 4, 96, 72); // beat 2, ahead of the playhead
        let mut e = os_engine::Engine::new(&a.state.project, &mut lib, 48000, mode);
        e.continue_from(&mut old);
        assert_eq!(e.position(), pos);
        let mut keys = Vec::new();
        for _ in 0..20 {
            e.process(&mut buf);
            keys.extend(e.sounding().map(|(_, k)| k));
        }
        assert!(keys.contains(&72), "new note ahead plays");
        assert!(!keys.contains(&60), "old note is not retriggered");
    }

    fn peak(buf: &[f32]) -> f32 {
        buf.iter().fold(0.0f32, |m, v| m.max(v.abs()))
    }

    /// Pattern playback loops inside the engine, exactly on the bar line (no gap, no rebuild).
    #[test]
    fn pattern_loop_is_sample_accurate() {
        let mut a = app();
        add_note(&mut a.state, 4, 0, 60);
        let mut lib = os_engine::SampleLibrary::new(None);
        let mut e = os_engine::Engine::new(&a.state.project, &mut lib, 48000, os_engine::PlayMode::Pattern(1));
        e.set_looping(true);
        let mut buf = vec![0.0f32; 2 * 512];
        let mut retriggered = false;
        for i in 0..200 {
            e.process(&mut buf);
            // One bar at 130 BPM is 88615 frames: block 173 holds the loop point.
            if i == 173 {
                retriggered = e.sounding().any(|(_, k)| k == 60);
            }
        }
        assert!(!e.finished());
        assert_eq!(e.position(), 200 * 512 - 88_615, "wrapped on the exact frame");
        assert!(retriggered, "the first note plays again at the loop point");
    }

    /// Channel volume / mute and mixer fader edits reach a voice that is already ringing.
    #[test]
    fn mixer_and_channel_edits_reach_ringing_voices() {
        let mode = os_engine::PlayMode::Pattern(1);
        let mut lib = os_engine::SampleLibrary::new(None);
        let mut buf = vec![0.0f32; 2 * 512];
        for edit in 0..3 {
            let mut a = app();
            add_note(&mut a.state, 4, 0, 60);
            let mut old = os_engine::Engine::new(&a.state.project, &mut lib, 48000, mode);
            for _ in 0..4 {
                old.process(&mut buf);
            }
            assert!(peak(&buf) > 0.05, "pluck rings");
            let p = &mut a.state.project;
            match edit {
                0 => p.channels[4].volume = 0.0,
                1 => p.channels[4].enabled = false,
                _ => {
                    let t = p.channels[4].mixer_track;
                    p.mixer.iter_mut().find(|m| m.index == t).unwrap().volume = Some(0.0);
                }
            }
            let mut e = os_engine::Engine::new(&a.state.project, &mut lib, 48000, mode);
            e.continue_from(&mut old);
            // The rest of the tick already rendered, then the ramp down: a mixer gain moves at most 0.001 per sample at
            // 44.1 kHz (about 1000 samples from unity to silence; spec: audio/mixer.md "Gains ramp").
            for _ in 0..3 {
                e.process(&mut buf);
            }
            e.process(&mut buf);
            assert_eq!(peak(&buf), 0.0, "edit {edit} silences the ringing voice");
        }
    }

    /// A tempo change keeps the musical position: the tick goes on where it was, the frames continue, and the ticks
    /// after it follow the new tempo.
    #[test]
    fn tempo_change_keeps_tick() {
        let mut a = app();
        add_note(&mut a.state, 4, 0, 60);
        let mut lib = os_engine::SampleLibrary::new(None);
        let mode = os_engine::PlayMode::Pattern(1);
        a.state.project.tempo = Some(130.0);
        let mut old = os_engine::Engine::new(&a.state.project, &mut lib, 48000, mode);
        let mut buf = vec![0.0f32; 2 * 500];
        for _ in 0..10 {
            old.process(&mut buf);
        }
        let (tick, frame) = (old.tick_position(), old.position());
        a.state.project.tempo = Some(65.0);
        let mut e = os_engine::Engine::new(&a.state.project, &mut lib, 48000, mode);
        e.continue_from(&mut old);
        assert_eq!((e.tick_position(), e.position()), (tick, frame), "same tick, same frame");
        // Past the tick already rendered, ticks are 48000 * 60 / (65 * 96) = 461.5 frames long.
        e.process(&mut buf);
        let (t0, f0) = (e.tick_position(), e.position());
        let mut ten = vec![0.0f32; 2 * 4615];
        e.process(&mut ten);
        let per_tick = (e.position() - f0) as f64 / (e.tick_position() - t0);
        assert!((per_tick - 461.5).abs() < 1.0, "{per_tick} frames per tick");
    }

    /// Live notes ring until note off, then fade out within the release; the engine with no
    /// schedule stays idle (finished) until a note arrives.
    #[test]
    fn live_note_on_off() {
        let a = app();
        let mut lib = os_engine::SampleLibrary::new(None);
        let mut e = os_engine::Engine::new(&a.state.project, &mut lib, 48000, os_engine::PlayMode::Pattern(1));
        e.clear_schedule();
        assert!(e.finished());
        let mut buf = vec![0.0f32; 2 * 256];
        e.note_on(4, 60, 100);
        assert!(!e.finished());
        e.process(&mut buf);
        assert!(peak(&buf) > 0.01, "sounds in the first block");
        assert_eq!(e.sounding().collect::<Vec<_>>(), vec![(4, 60)]);
        for _ in 0..20 {
            e.process(&mut buf); // held: rings on
        }
        assert!(peak(&buf) > 0.001 && e.sounding().count() == 1, "held note rings");
        e.note_off(4, 60);
        assert_eq!(e.sounding().count(), 0);
        for _ in 0..5 {
            e.process(&mut buf); // the rest of the tick already rendered, the 15 ms fade (720 frames), a silent block
        }
        assert_eq!(peak(&buf), 0.0, "released");
        assert!(e.finished());
    }

    /// Typing keyboard: physical keys map to notes on the selected channel, light the keys, take the
    /// tool letters, shift octaves, and every note is released.
    #[test]
    fn typing_keyboard_keys() {
        use winit::keyboard::KeyCode;
        let mut a = app();
        assert!(a.typing_on(), "on by default");
        assert!(a.typing_key(KeyCode::KeyZ, true, false, false));
        assert!(a.typing_key(KeyCode::KeyQ, true, false, false));
        assert!(a.typing_key(KeyCode::KeyQ, true, false, false), "repeat is taken, not replayed");
        assert_eq!(a.state.piano.typed, vec![(4, 48), (4, 60)]);
        assert!(a.typing_key(KeyCode::KeyZ, false, false, false));
        assert_eq!(a.state.piano.typed, vec![(4, 60)]);
        assert!(!a.typing_key(KeyCode::KeyP, true, true, false), "Ctrl+letter stays a shortcut");
        assert!(!a.typing_key(KeyCode::Space, true, false, false));
        assert!(a.typing_key(KeyCode::NumpadAdd, true, false, false));
        assert!(a.typing_key(KeyCode::KeyC, true, false, false));
        assert_eq!(a.state.piano.typed, vec![(4, 60), (4, 64)], "octave up: C is E5");
        a.toggle_typing();
        assert!(!a.typing_on());
        assert!(a.state.piano.typed.is_empty(), "switching off releases held notes");
        assert!(!a.typing_key(KeyCode::KeyZ, true, false, false), "letters are shortcuts again");
    }

    /// Mixer and channel knobs, the fader and mute mark the project for the playing engine.
    #[test]
    fn playback_edits_are_flagged() {
        use crate::knobs::KnobId;
        let mut a = app();
        for id in [KnobId::MixPan(1), KnobId::ChanVol(0), KnobId::ChanPan(0), KnobId::MasterVol, KnobId::MasterPitch] {
            a.state.piano.dirty = false;
            a.state.set_knob(id, 0.2);
            assert!(a.state.piano.dirty, "{id:?}");
        }
        a.state.piano.dirty = false;
        a.act(crate::app::Action::ChannelEnable(0));
        assert!(a.state.piano.dirty, "channel mute");
    }

    fn key_name_check(k: i32) -> String {
        crate::views::piano::key_name(k)
    }

    /// "Auto zoom in piano roll" (spec: ui/settings-choices.md "Miscellaneous"): opening the window zooms on the
    /// pattern (from bar 1, its length plus half a bar fills the grid) and on the channel's pitch range (centred, rows
    /// the tallest fitting it plus four spare rows, at most the default × 1.25); without notes, C5 is centred at the
    /// smallest rows. Off: the zoom stays.
    #[test]
    fn auto_zoom_in_piano_roll() {
        use super::*;
        use crate::app::{Action, App, Win};
        use crate::prefs::{set_check, AUTO_ZOOM_PIANO};
        for on in [false, true] {
            set_check(&AUTO_ZOOM_PIANO, on);
            let mut app = App::new(None);
            app.state.project = crate::app::test_project();
            app.state.selected_channel = 0;
            // A floating window: a grid about 300 px tall.
            let ws = &mut app.state.wins[Win::PianoRoll.slot().expect("a window")];
            (ws.maxed, ws.rect) = (false, Rect::new(300.0, 150.0, 1000.0, 560.0));
            // C4, D4 ... F#5 (19 keys), an eighth each from the second eighth on: they end in bar 2, a 2-bar pattern.
            let notes: Vec<Note> = (0..10u16).map(|k| Note { key: 48 + 2 * k, length: 48, ..Note::step(0, 48 * (u32::from(k) + 1)) }).collect();
            pattern_mut(&mut app.state).notes = notes;
            let open = |app: &mut App| {
                app.state.hide(Win::PianoRoll);
                app.tick();
                app.act(Action::ToggleWin(Win::PianoRoll));
                app.tick();
                let grid = crate::views::piano::grid_rect(&app.state);
                (grid, Geom::new(&app.state, grid))
            };
            let before = (app.state.piano.px_per_beat, app.state.piano.row_h);
            let (grid, g) = open(&mut app);
            if !on {
                assert_eq!((app.state.piano.px_per_beat, app.state.piano.row_h), before, "off: the zoom stays");
                continue;
            }
            let bar = app.state.project.bar_ticks() as f32;
            assert!((grid.w / g.ppt / bar - 2.5).abs() < 0.01, "2 bars + half a bar fill the grid: {}", grid.w / g.ppt / bar);
            assert_eq!(g.scroll_x, 0.0, "from bar 1");
            assert_eq!(g.row, (grid.h / 23.0).floor(), "19 keys + 4 spare rows in {} px", grid.h);
            assert!(g.row < 22.0);
            let mid = (g.y_of(66) + g.y_of(48) + g.row) / 2.0;
            assert!((mid - grid.cy()).abs() <= g.row, "the range is centred: {mid} in {grid:?}");
            // The grid opened is where the window paints it.
            if let Ok(mut c) = os_ui::Canvas::new(1920, 1080, 1.0) {
                app.paint(&mut c, None);
                assert_eq!(app.piano_grid, Some(grid));
            }
            // Short notes on few keys: the rows stop at the default × 1.25.
            pattern_mut(&mut app.state).notes.truncate(1);
            let (_, g) = open(&mut app);
            assert_eq!(g.row, (ROW * 1.25).floor());
            // No notes: C5 centred at the smallest rows.
            pattern_mut(&mut app.state).notes.clear();
            let (grid, g) = open(&mut app);
            assert_eq!(g.row, MIN_ROW);
            assert!((g.y_of(60) + g.row / 2.0 - grid.cy()).abs() <= g.row, "C5 centred");
        }
        set_check(&AUTO_ZOOM_PIANO, false);
    }
}
