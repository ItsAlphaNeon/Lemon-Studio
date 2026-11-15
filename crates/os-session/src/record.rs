// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Recording (the toolbar's Record button): while it is armed and the transport plays, the notes played on the typing
//! keyboard, a MIDI keyboard or the Touch controller are written into the current pattern for the selected channel,
//! each as long as it was held, and channel volume / pan knob moves become the pattern's control points. A take (from its first note to the stop, a pause, disarming or a pattern change) is one
//! undo step and replaces the pattern's earlier notes that start inside it. The right-click menu's checks are program
//! settings in `settings.txt`, also shown on Settings ▸ Recording (`crate::prefs`); the arm itself is not saved.
// spec: ui/recording.md

#![allow(deprecated)] // writes the project directly (`crate::locked`; control-plane step 14 removes this)
use crate::session::{Session, View};
use crate::step_edit::Src;
use os_model::{ClipSource, Note, TimeMarker};

/// The undo step of a take.
pub const RECORDED: &str = "Recorded items";

/// The right-click menu's checks: key in `settings.txt` and default (spec §2 Right-click menu).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Opt {
    Notes,
    Automation,
    /// Armed mixer tracks record (spec: audio/track-io.md §2.5).
    Audio,
    StartsPlayback,
    DisarmOnStop,
    QuantizeStart,
    QuantizeEnd,
    /// The record options (spec §3): Start on input and its right-click checks, Recording precount, Blend.
    Wait,
    WaitFilter,
    WaitArmed,
    Precount,
    Blend,
    /// Loop record (spec §4).
    Loop,
    /// Ask what to record when arming (spec §2 What to record).
    Ask,
}

/// The record popups' commands (`Cmd::Rec`): the question's picks (spec §2 What to record), the metronome menu's
/// precount (§3), the score logger's dump and clear (§6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecCmd {
    Arm { notes: bool, automation: bool },
    AskOff,
    /// 0: Recording precount off; 1 or 2: on, that many bars.
    Precount(u32),
    /// Dump the last n minutes; asks first when the pattern has notes.
    Dump(u32),
    /// Dump the last n minutes, replacing the pattern's notes (true) or merging with them.
    DumpAs(u32, bool),
    ClearLog,
    /// The audio logger keeps the last n minutes.
    AudioLog(u32),
}

impl Opt {
    /// Its program setting, also on Settings ▸ Recording (`crate::prefs`, spec: ui/settings.md "Recording").
    pub fn check(self) -> &'static crate::prefs::Check {
        use crate::prefs as p;
        match self {
            Opt::Notes => &p::REC_NOTES,
            Opt::Automation => &p::REC_AUTOMATION,
            Opt::Audio => &p::REC_AUDIO,
            Opt::StartsPlayback => &p::REC_STARTS_PLAYBACK,
            Opt::DisarmOnStop => &p::REC_DISARM_ON_STOP,
            Opt::QuantizeStart => &p::REC_QUANTIZE_START,
            Opt::QuantizeEnd => &p::REC_QUANTIZE_END,
            Opt::Wait => &p::REC_WAIT,
            Opt::WaitFilter => &p::REC_WAIT_FILTER,
            Opt::WaitArmed => &p::REC_WAIT_ARMED,
            Opt::Precount => &p::REC_COUNT_IN,
            Opt::Blend => &p::REC_BLEND,
            Opt::Loop => &p::REC_LOOP,
            Opt::Ask => &p::REC_ASK,
        }
    }

    pub fn on(self, st: &crate::settings_file::SettingsStore) -> bool {
        st.check(self.check())
    }

    pub fn toggle(self, st: &crate::settings_file::SettingsStore) -> bool {
        st.toggle(self.check())
    }
}

/// The button's hint: what the filter records (spec §2 The button).
pub fn hint(st: &crate::settings_file::SettingsStore) -> String {
    let what: Vec<&str> = [(Opt::Notes, "notes"), (Opt::Automation, "automation"), (Opt::Audio, "audio")].into_iter().filter(|(o, _)| o.on(st)).map(|(_, w)| w).collect();
    let what = if what.is_empty() { "nothing".to_string() } else { what.join(", ") };
    format!("Record ({what}) (R) - right-click for options")
}

/// Recording precount's length in bars: 1 or 2 (spec §3), Settings ▸ Recording's Count-in length.
pub fn precount_bars(st: &crate::settings_file::SettingsStore) -> u32 {
    st.index(&crate::prefs::COUNT_IN_LENGTH) as u32 + 1
}

pub fn set_precount_bars(st: &crate::settings_file::SettingsStore, bars: u32) {
    st.set_choice(&crate::prefs::COUNT_IN_LENGTH, if bars == 2 { 1 } else { 0 });
}

/// The three option buttons' hints (spec §3).
pub fn option_hint(o: Opt) -> &'static str {
    match o {
        Opt::Wait => "Start on input (Ctrl+I) - right-click for options",
        Opt::Precount => "Count-in (Ctrl+P) - right-click for the length",
        Opt::Loop => "Loop record (off: playback runs on past the end while recording)",
        _ => "Blend recorded notes (Ctrl+B)",
    }
}

/// A recording count-in under way (spec §3): when it started, a beat's length in seconds, its beats.
#[derive(Debug, Clone, Copy)]
pub struct CountIn {
    pub from: std::time::Instant,
    pub beat: f64,
    pub beats: u32,
}

impl CountIn {
    /// Beats left (the clock's "REC IN n"); 0 once over.
    pub fn left(&self) -> u32 {
        let done = (self.from.elapsed().as_secs_f64() / self.beat.max(1e-6)).floor();
        self.beats.saturating_sub(done.min(f64::from(u32::MAX)) as u32)
    }
}

/// A note held while recording: where it started, in the pattern and in the running playback's ticks.
#[derive(Debug, Clone, Copy)]
pub struct Held {
    src: Src,
    pub key: u16,
    pub velocity: u8,
    pub channel: u16,
    pub start: f64,
    pub at: f64,
    extra: Extra,
    release: Option<u8>,
}

/// What a MIDI note brings besides its key and velocity (spec: ui/midi-settings.md §3): the channel it plays when not
/// the selected one (omni preview), its Mod X / Mod Y (velocity linked to them).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Extra {
    /// Channel index (`os_model::Channel::index`).
    pub channel: Option<u16>,
    pub mod_x: Option<u8>,
    pub mod_y: Option<u8>,
    /// The colour group a recorded note takes: the MIDI channel it came on, on a channel with MIDI channel through
    /// ticked (spec: ui/channel-rack.md §18).
    pub group: Option<u8>,
}

/// The take being recorded: the channels it recorded, the span from its first start to its last end, and its notes.
#[derive(Debug, Clone)]
pub struct Take {
    pub pattern: u16,
    channels: Vec<u16>,
    span: Option<(u32, u32)>,
    new: Vec<Note>,
    /// Each recorded control (`ControlPoint::kind`) and the tick of its last point in this take.
    controls: Vec<(u32, u32)>,
    /// SONG mode: the clip the take records into (track, position), made longer when playback runs past it (spec §2).
    clip: Option<(u16, u32)>,
}

/// Record's state (spec §2).
#[derive(Debug, Default)]
pub struct Rec {
    pub armed: bool,
    /// One-click audio recording is taking in (its button is lit; `crate::one_click`).
    pub audio: bool,
    pub held: Vec<Held>,
    pub take: Option<Take>,
    /// The playback position at the last poll while playing: where held notes end when playback stops.
    last: Option<f64>,
    was_live: bool,
    /// Start on input: Play was pressed and playback waits for the first note (spec §3).
    pub waiting: bool,
    /// The count-in of Recording precount, while it lasts (spec §3).
    pub count: Option<CountIn>,
    /// The count-in's beats left as last drawn.
    pub shown: u32,
    /// The score logger (spec §6).
    pub log: crate::score_log::Log,
    /// The history's done steps when the playback that is recording started: TOOLS ▸ Cancel recording goes back to it.
    pub since: Option<usize>,
    /// SONG mode: playback has passed a Stop recording marker (and no Start recording marker since), so notes are not
    /// recorded until playback stops or goes back (spec: audio/transport.md "Time markers in SONG mode").
    pub punched_out: bool,
    /// The grid notes are quantized to when written: the interface's main snap, set before each call (None: as
    /// played).
    pub unit: Option<u32>,
}

impl Rec {
    /// Another project opens: its take, held notes, count-in and Cancel recording's step go without being written.
    /// Record stays armed.
    pub fn forget(&mut self) {
        *self = Rec { armed: self.armed, audio: self.audio, unit: self.unit, log: std::mem::take(&mut self.log), ..Rec::default() };
    }

    /// A take is open (for tests and the hint).
    pub fn recording(&self) -> bool {
        self.take.is_some()
    }

    /// Notes are held for the take (the piano roll draws them growing).
    pub fn holding(&self) -> bool {
        self.take.is_some() && !self.held.is_empty()
    }
}

pub fn round(t: u32, u: u32) -> u32 {
    (t + u / 2) / u * u
}

/// `quantized` on grid `u` (None: as played).
pub fn quantized_on(st: &crate::settings_file::SettingsStore, u: Option<u32>, start: u32, end: u32) -> (u32, u32) {
    let q = |t: u32, on: bool| match u {
        Some(u) if on => round(t, u),
        _ => t,
    };
    let a = q(start, Opt::QuantizeStart.on(st));
    let b = q(end, Opt::QuantizeEnd.on(st));
    let b = if b > a { b } else { a + u.unwrap_or(1) };
    (a, b)
}

/// Writes a control point on target word `kind` at the playback position, replacing the move's earlier points.
pub fn point_in(s: &mut Session, kind: u32, value: i32) {
    let song = s.transport.song();
    let t = s.transport.tick();
    open_take_in(s, song);
    let Some(at) = rec_pattern_tick_in(s, t, song) else { return };
    let tick = at.max(0.0).round() as u32;
    let Some(take) = s.rec.take.as_mut() else { return };
    let last = take.controls.iter().find(|c| c.0 == kind).map(|c| c.1);
    take.controls.retain(|c| c.0 != kind);
    take.controls.push((kind, tick));
    if let Some(last) = last.filter(|&l| l < tick && !Opt::Blend.on(&s.settings)) {
        let p = crate::edit::notes::pattern_mut(s);
        p.controls.retain(|c| !(c.kind == kind && c.position > last && c.position < tick));
    }
    crate::edit::notes::set_control(s, (kind >> 16) as u16, kind as u16, tick, value);
    s.dirty = true;
}

/// A note went down on `channel` while recording (the interface has checked that it records): it is held from the
/// playback position.
pub fn down_in(s: &mut Session, src: Src, key: u16, velocity: u8, channel: u16, extra: Extra) {
    let song = s.transport.song();
    if song && s.rec.punched_out {
        return;
    }
    let start = s.transport.tick();
    open_take_in(s, song);
    let Some(at) = rec_pattern_tick_in(s, start, song) else { return };
    let r = &mut s.rec;
    r.held.retain(|h| h.src != src);
    r.held.push(Held { src, key, velocity, channel, start, at, extra, release: None });
    if let Some(t) = r.take.as_mut().filter(|t| !t.channels.contains(&channel)) {
        t.channels.push(channel);
    }
}

/// A note went up: written when it was being recorded.
pub fn up_in(s: &mut Session, src: Src, release: Option<u8>) {
    let r = &mut s.rec;
    let Some(i) = r.held.iter().position(|h| h.src == src) else { return };
    let mut h = r.held.remove(i);
    h.release = release;
    let now = if s.transport.playing() && !s.transport.paused() { Some(s.transport.tick()) } else { r.last };
    write_held_in(s, h, now);
}

/// The take starts: one undo step for it, with the playlist too in SONG mode (a clip may be placed).
fn open_take_in(s: &mut Session, song: bool) {
    if s.rec.take.as_ref().is_some_and(|t| t.pattern == s.pattern()) {
        return;
    }
    let pattern = s.pattern();
    let snap = crate::history::Snap::pattern(s, pattern);
    let snap = if song {
        crate::edit::playlist::arr_mut(s);
        crate::history::Snap::Several(vec![crate::history::Snap::Playlist(Box::new(crate::edit::playlist::capture(s))), snap])
    } else {
        snap
    };
    crate::history::record(s, RECORDED, snap);
    s.rec.take = Some(Take { pattern, channels: Vec::new(), span: None, new: Vec::new(), controls: Vec::new(), clip: None });
}

/// The current pattern's tick at playback tick `t`: the tick itself in PAT mode; in SONG mode where a clip of the
/// pattern plays there. Past the end of the take's clip that clip is made longer (to the end of `t`'s bar); with
/// no clip, a whole-pattern clip just before `t` becomes the take's and grows, else one is placed from the start
/// of that bar on the first free track (spec §2).
fn rec_pattern_tick_in(s: &mut Session, t: f64, song: bool) -> Option<f64> {
    if !song {
        return Some(t);
    }
    if let Some((p, _)) = grow_take_clip(s, t) {
        return Some(p);
    }
    let bar = s.project.bar_ticks().max(1);
    let pattern = s.pattern();
    let whole = |c: &os_model::Clip| c.source == ClipSource::Pattern(pattern) && c.window.is_none();
    let ti = t.max(0.0) as u32;
    if let Some(p) = crate::edit::notes::pattern_tick_in_song(s, t) {
        // The whole-pattern clip playing there becomes the take's.
        let under = crate::edit::playlist::clips(s)
            .iter()
            .filter(|c| whole(c) && c.position <= ti && (f64::from(c.position) + p - t).abs() < 1e-6)
            .max_by_key(|c| c.position)
            .map(|c| (c.track, c.position));
        if let Some(k) = s.rec.take.as_mut().filter(|k| k.clip.is_none()) {
            k.clip = under;
        }
        return Some(p);
    }
    // Right after a whole-pattern clip, less than a bar past the bar line at or after its end, with its track free
    // up to the end of `t`'s bar: that clip becomes the take's and grows instead of a second one being placed.
    let end = (ti / bar + 1) * bar;
    let clips = crate::edit::playlist::clips(s);
    let before = clips
        .iter()
        .filter(|c| whole(c) && !c.muted && c.end() <= ti && ti < c.end().div_ceil(bar) * bar + bar)
        .filter(|c| !clips.iter().any(|o| o.track == c.track && o.position < end && o.end() > c.end()))
        .max_by_key(|c| c.end())
        .map(|c| (c.track, c.position));
    if let Some(k) = s.rec.take.as_mut().filter(|k| k.clip.is_none() && before.is_some()) {
        k.clip = before;
        return grow_take_clip(s, t).map(|(p, _)| p);
    }
    let at = ti / bar * bar;
    let len = crate::edit::notes::pattern_length(s, pattern).max(bar);
    let track = crate::edit::playlist::free_track(s, 0, at, at + len);
    let clip = os_model::Clip::new(at, len, track, ClipSource::Pattern(pattern));
    crate::edit::playlist::arr_mut(s).clips.push(clip);
    if let Some(k) = s.rec.take.as_mut() {
        k.clip = Some((track, at));
    }
    s.dirty = true;
    crate::edit::notes::pattern_tick_in_song(s, t)
}

/// Writes held note `h`, ending at playback tick `now` (None: where it started). A note held over the pattern's
/// loop ends at the pattern's end.
fn write_held_in(s: &mut Session, h: Held, now: Option<f64>) {
    let Some(take) = s.rec.take.as_mut() else { return };
    let start = h.at.max(0.0).round() as u32;
    let end = match now {
        Some(n) if n >= h.start => (h.at + (n - h.start)).round() as u32,
        Some(_) => s.project.pattern_length(take.pattern).max(start),
        None => start,
    };
    let pattern = take.pattern;
    let (a, b) = quantized_on(&s.settings, s.rec.unit, start, end);
    let take = s.rec.take.as_mut().expect("checked");
    let mut note = Note { length: b - a, key: h.key, velocity: h.velocity, ..Note::step(h.channel, a) };
    note.mod_x = h.extra.mod_x.unwrap_or(note.mod_x);
    note.mod_y = h.extra.mod_y.unwrap_or(note.mod_y);
    if let Some(g) = h.extra.group {
        note.group = note.group & !0x0F | g & 0x0F;
    }
    note.release = h.release.unwrap_or(note.release);
    let (lo, hi) = take.span.map_or((a, b), |(lo, hi)| (lo.min(a), hi.max(b)));
    take.span = Some((lo, hi));
    // Blend recorded notes: the earlier notes stay (spec §3).
    let (lo, hi) = if Opt::Blend.on(&s.settings) { (0, 0) } else { (lo, hi) };
    // The pattern as it is now, so edits made during the take stay: the take's span replaces the other notes on its
    // channels, and the new note joins.
    let patterns = &mut s.project.get_mut().patterns;
    let i = match patterns.iter().position(|p| p.number == pattern) {
        Some(i) => i,
        None => {
            patterns.push(os_model::Pattern { number: pattern, ..os_model::Pattern::default() });
            patterns.len() - 1
        }
    };
    let p = &mut patterns[i];
    p.notes.retain(|n| take.new.contains(n) || !(take.channels.contains(&n.channel) && n.position >= lo && n.position < hi));
    p.notes.push(note);
    take.new.push(note);
    s.touched.views.push(View::Recorded { pattern, channel: h.channel });
    s.dirty = true;
    s.touched.repaint = true;
}

/// The take ends: the notes still held end where playback last was.
pub fn end_take_in(s: &mut Session) {
    let held = std::mem::take(&mut s.rec.held);
    let last = s.rec.last;
    for h in held {
        write_held_in(s, h, last);
    }
    s.rec.take = None;
}

/// Each tick: notes the playback position; when playback stops or pauses, or the pattern changes, the take ends;
/// a stop disarms when "Disarm on stop" is on. `sel` is the time selection looping playback. True when a punch out
/// asks for playback to stop.
pub fn tick_in(s: &mut Session, sel: Option<(u32, u32)>) -> bool {
    // A count-in ends after its beats (or with the playback); the clock shows each beat left. A wait ends when
    // something else starts playback.
    if let Some(c) = s.rec.count {
        let left = c.left();
        if left == 0 || !s.transport.playing() || s.transport.paused() {
            s.rec.count = None;
            s.touched.repaint = true;
        } else if left != s.rec.shown {
            s.rec.shown = left;
            s.touched.repaint = true;
        }
    }
    if s.rec.waiting && s.transport.playing() {
        s.rec.waiting = false;
        s.touched.repaint = true;
    }
    let run_on = s.rec.armed && !Opt::Loop.on(&s.settings);
    crate::transport::run_on_flag().store(run_on, std::sync::atomic::Ordering::Relaxed);
    let live = s.transport.playing() && !s.transport.paused();
    // Punch (spec §5): armed with Loop record off, playback stops at the selection's end; held notes end there.
    if let Some((_, end)) = sel.filter(|_| live && run_on && s.rec.count.is_none()) {
        let end = f64::from(end);
        if s.transport.tick() >= end && s.rec.last.is_some_and(|l| l < end) {
            s.rec.last = Some(end);
            end_take_in(s);
            return true;
        }
    }
    // Playback went back to the start (a loop, or a jump back): the pass's take ends there, its held notes at the
    // pattern's end; the next note starts a new take (spec §4).
    let now = s.transport.tick();
    if live && s.rec.last.is_some_and(|l| now < l) && s.rec.take.is_some() {
        let held = std::mem::take(&mut s.rec.held);
        for h in held {
            write_held_in(s, h, Some(f64::MIN));
        }
        s.rec.take = None;
    }
    // SONG mode, Start / Stop recording markers (audio/transport.md): passing a Stop recording marker ends the take
    // there (held notes end on it) and later notes are not recorded; passing a Start recording marker records
    // again (Lemon Studio's choice). Recording before any of them goes on as usual, from the song's start with a
    // Start recording marker later on.
    if live && s.transport.song() {
        let from = s.rec.last.filter(|&l| l <= now);
        let mut passed: Vec<(u32, u8)> = crate::edit::playlist::markers(s)
            .iter()
            .filter(|m| matches!(m.kind, TimeMarker::PUNCH_IN | TimeMarker::PUNCH_OUT))
            .filter(|m| from.is_some_and(|l| f64::from(m.position) > l && f64::from(m.position) <= now))
            .map(|m| (m.position, m.kind))
            .collect();
        passed.sort_unstable();
        for (at, kind) in passed {
            let out = kind == TimeMarker::PUNCH_OUT;
            if out && !s.rec.punched_out && s.rec.take.is_some() {
                s.rec.last = Some(f64::from(at));
                end_take_in(s);
                s.touched.repaint = true;
            }
            s.rec.punched_out = out;
        }
    }
    if !live || s.rec.last.is_some_and(|l| now < l) {
        s.rec.punched_out = false;
    }
    // SONG mode: a note held past the take's clip makes it longer as playback runs (spec §2).
    if live && s.transport.song() && !s.rec.held.is_empty() && grow_take_clip(s, now).is_some_and(|(_, g)| g) {
        s.touched.repaint = true;
    }
    if live {
        s.rec.last = Some(now);
    }
    // Performance mode (spec: ui/performance-mode.md §3b): a stop with Record armed (since playback started) writes
    // what the launched tracks played.
    let r = &s.rec;
    if r.was_live && !live && r.armed && r.since.is_some() && s.transport.song()
        && let Some(stop) = r.last
    {
        let n = crate::perform::record(s, stop.max(0.0) as u32);
        if n > 0 {
            s.touched.views.push(View::Message(format!("Performance recorded: {n} clip{}", if n == 1 { "" } else { "s" })));
            s.touched.repaint = true;
        }
    }
    let r = &s.rec;
    let changed = r.take.as_ref().is_some_and(|t| t.pattern != s.pattern());
    if (r.was_live && !live) || changed {
        end_take_in(s);
        if !s.transport.playing() && s.rec.armed && Opt::DisarmOnStop.on(&s.settings) {
            s.rec.armed = false;
            s.touched.repaint = true;
        }
    }
    if !live {
        s.rec.last = None;
    }
    if live && !s.rec.was_live {
        s.rec.since = s.rec.armed.then(|| s.history.done());
    }
    s.rec.was_live = live;
    false
}

/// SONG mode: the pattern tick at song tick `t` in the take's clip, when `t` is at or past its start; past its end the
/// clip is made longer, to the end of `t`'s bar (spec §2 "one clip per performance"). Also says whether it grew.
fn grow_take_clip(s: &mut Session, t: f64) -> Option<(f64, bool)> {
    let (track, at) = s.rec.take.as_ref().and_then(|k| k.clip)?;
    let ti = t.max(0.0) as u32;
    if ti < at {
        return None;
    }
    let bar = s.project.bar_ticks().max(1);
    let pattern = s.pattern();
    let c = crate::edit::playlist::arr_mut(s)
        .clips
        .iter_mut()
        .find(|c| c.source == ClipSource::Pattern(pattern) && c.window.is_none() && c.track == track && c.position == at)?;
    let grow = ti >= at + c.length;
    if grow {
        c.length = (ti / bar + 1) * bar - at;
        s.dirty = true;
    }
    Some((t - f64::from(at), grow))
}

#[cfg(any(test, feature = "test-util"))]
mod tests {

}
