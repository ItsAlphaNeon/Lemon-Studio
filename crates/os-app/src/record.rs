// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Recording (the toolbar's Record button): while it is armed and the transport plays, the notes played on the typing
//! keyboard, a MIDI keyboard or the Touch controller are written into the current pattern for the selected channel,
//! each as long as it was held, and channel volume / pan knob moves become the pattern's control points. A take (from its first note to the stop, a pause, disarming or a pattern change) is one
//! undo step and replaces the pattern's earlier notes that start inside it. The right-click menu's checks are program
//! settings in `settings.txt`; the arm itself is not saved.
// spec: ui/recording.md

use crate::app::{App, State};
use crate::knobs::KnobId;
use crate::playlist::Snap;
use crate::step_edit::Src;
use os_model::{ClipSource, Note};

/// The undo step of a take (FL's name for it).
pub const RECORDED: &str = "Recorded items";

/// The right-click menu's checks: key in `settings.txt` and default (spec §2 Right-click menu).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Opt {
    Notes,
    Automation,
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
}

impl Opt {
    fn key(self) -> (&'static str, bool) {
        match self {
            Opt::Notes => ("rec_notes", true),
            Opt::Automation => ("rec_automation", true),
            Opt::StartsPlayback => ("rec_starts_play", false),
            Opt::DisarmOnStop => ("rec_disarm_stop", false),
            Opt::QuantizeStart => ("rec_q_start", true),
            Opt::QuantizeEnd => ("rec_q_end", true),
            Opt::Wait => ("rec_wait", false),
            Opt::WaitFilter => ("rec_wait_filter", true),
            Opt::WaitArmed => ("rec_wait_armed", false),
            Opt::Precount => ("rec_precount", false),
            Opt::Blend => ("rec_blend", false),
            Opt::Loop => ("rec_loop", false),
            Opt::Ask => ("rec_ask", true),
        }
    }

    pub fn on(self) -> bool {
        let (k, d) = self.key();
        crate::settings_file::flag(k, d)
    }

    pub fn toggle(self) -> bool {
        let on = !self.on();
        crate::settings_file::set_flag(self.key().0, on);
        on
    }
}

/// The button's hint: what the filter records (spec §2 The button).
pub fn hint() -> String {
    let what = match (Opt::Notes.on(), Opt::Automation.on()) {
        (true, true) => "notes, automation",
        (true, false) => "notes",
        (false, true) => "automation",
        (false, false) => "nothing",
    };
    format!("Record ({what}) (R) - right-click for options")
}

/// Recording precount's length in bars: 1 or 2 (spec §3), in `settings.txt`.
pub fn precount_bars() -> u32 {
    if crate::settings_file::get("rec_precount_bars").as_deref() == Some("2") { 2 } else { 1 }
}

pub fn set_precount_bars(bars: u32) {
    crate::settings_file::write("rec_precount_bars", if bars == 2 { "2" } else { "1" });
}

/// The three option buttons' hints (spec §3).
pub fn option_hint(o: Opt) -> &'static str {
    match o {
        Opt::Wait => "Start on input (Ctrl+I) - right-click for options",
        Opt::Precount => "Recording precount (Ctrl+P) - right-click for the length",
        Opt::Loop => "Loop record (off: playback runs on past the end while recording)",
        _ => "Blend recorded notes (Ctrl+B)",
    }
}

/// Set while Record is armed and Loop record is off: playback runs on past its end (spec §4). Every playback engine
/// reads it (`Engine::set_run_on`); `App::record_tick` keeps it up to date. One per test thread under tests.
pub fn run_on_flag() -> std::sync::Arc<std::sync::atomic::AtomicBool> {
    use std::sync::{atomic::AtomicBool, Arc};
    #[cfg(test)]
    {
        thread_local!(static TEST: Arc<AtomicBool> = Arc::new(AtomicBool::new(false)));
        TEST.with(Arc::clone)
    }
    #[cfg(not(test))]
    {
        static FLAG: std::sync::OnceLock<Arc<AtomicBool>> = std::sync::OnceLock::new();
        FLAG.get_or_init(|| Arc::new(AtomicBool::new(false))).clone()
    }
}

/// A recording count-in under way (spec §3): when it started, a beat's length in seconds, its beats.
#[derive(Debug, Clone, Copy)]
pub struct CountIn {
    from: std::time::Instant,
    beat: f64,
    beats: u32,
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
struct Held {
    src: Src,
    key: u16,
    velocity: u8,
    channel: u16,
    start: f64,
    at: f64,
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
}

/// The take being recorded: the pattern's notes as they were before it, the channels it recorded, the span from its
/// first start to its last end, and its notes.
#[derive(Debug, Clone)]
struct Take {
    pattern: u16,
    before: Vec<Note>,
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
    held: Vec<Held>,
    take: Option<Take>,
    /// The playback position at the last poll while playing: where held notes end when playback stops.
    last: Option<f64>,
    was_live: bool,
    /// Start on input: Play was pressed and playback waits for the first note (spec §3).
    pub waiting: bool,
    /// The count-in of Recording precount, while it lasts (spec §3).
    pub count: Option<CountIn>,
    /// The count-in's beats left as last drawn.
    shown: u32,
    /// The score logger (spec §6).
    pub log: crate::score_log::Log,
}

impl Rec {
    /// A take is open (for tests and the hint).
    pub fn recording(&self) -> bool {
        self.take.is_some()
    }
}

/// The main snap step that quantizing rounds to; None for Line, Cell and (none), which leave times as played.
fn unit(s: &State) -> Option<u32> {
    match s.global_snap.now {
        Snap::Line | Snap::Cell | Snap::None => None,
        sn => Some(sn.ticks(u32::from(s.project.ppq.max(4)), 0.0).max(1)),
    }
}

fn round(t: u32, u: u32) -> u32 {
    (t + u / 2) / u * u
}

/// A note from `start` to `end` (pattern ticks) quantized as the menu says; never ending at or before its start.
pub fn quantized(s: &State, start: u32, end: u32) -> (u32, u32) {
    let u = unit(s);
    let q = |t: u32, on: bool| match u {
        Some(u) if on => round(t, u),
        _ => t,
    };
    let a = q(start, Opt::QuantizeStart.on());
    let b = q(end, Opt::QuantizeEnd.on());
    let b = if b > a { b } else { a + u.unwrap_or(1) };
    (a, b)
}

impl App {
    /// The Record button and R: disarms, or asks what to record (spec §2 What to record), or arms when asking is off.
    pub(crate) fn toggle_record(&mut self) {
        if self.state.rec.armed {
            self.end_take();
            self.state.rec.armed = false;
        } else if Opt::Ask.on() {
            self.record_question();
        } else {
            self.arm();
        }
        self.invalidate();
    }

    /// Arms; while stopped this starts playback when "Recording starts playback" is on.
    fn arm(&mut self) {
        self.state.rec.armed = true;
        if !self.transport.playing() && !self.state.rec.waiting && Opt::StartsPlayback.on() {
            self.play_from_stop();
        }
        self.invalidate();
    }

    /// The question popup, under the Record button.
    fn record_question(&mut self) {
        use crate::playlist::{Cmd, PItem, Popup};
        let arm = |label: &'static str, notes: bool, automation: bool| PItem::new(label, Cmd::Rec(RecCmd::Arm { notes, automation }));
        let audio: Vec<PItem> = crate::one_click::items(&self.state).into_iter().skip(1).collect();
        let items = vec![
            PItem::head("What would you like to record?"),
            arm("Notes and automation", true, true),
            arm("Notes only", true, false),
            arm("Automation only", false, true),
            PItem::sub("Audio, into the playlist as an audio clip", audio),
            // The audio editor is to be a plugin of its own.
            PItem::new("Audio, into the audio editor", Cmd::Nothing).off(),
            PItem::sep(),
            PItem::new("Don't ask again", Cmd::Rec(RecCmd::AskOff)),
        ];
        let (x, y) = self.hits.iter().find(|h| h.action == crate::app::Action::Record).map_or((486.0, 35.0), |h| (h.rect.x, h.rect.bottom() + 2.0));
        self.state.playlist.popup = Some(Popup { x, y, items, sub: None });
    }

    /// A record popup's command (`Cmd::Rec`); returns the message to show.
    pub(crate) fn rec_cmd(&mut self, c: RecCmd) -> Option<String> {
        let set = |o: Opt, on: bool| {
            if o.on() != on {
                o.toggle();
            }
        };
        let msg = match c {
            RecCmd::Arm { notes, automation } => {
                set(Opt::Notes, notes);
                set(Opt::Automation, automation);
                self.arm();
                None
            }
            RecCmd::AskOff => {
                set(Opt::Ask, false);
                Some("Record arms at once now (right-click Record > Ask what to record to ask again)".into())
            }
            RecCmd::Precount(bars) => {
                set(Opt::Precount, bars > 0);
                if bars > 0 {
                    set_precount_bars(bars);
                }
                Some(if bars > 0 { format!("Recording precount: {bars} bar{}", if bars == 1 { "" } else { "s" }) } else { "Recording precount off".into() })
            }
            RecCmd::Dump(minutes) => self.dump_score_log(minutes, None),
            RecCmd::DumpAs(minutes, replace) => self.dump_score_log(minutes, Some(replace)),
            RecCmd::ClearLog => {
                self.state.rec.log.clear();
                Some("Score log cleared".into())
            }
        };
        self.invalidate();
        msg
    }

    /// Dumps the last `minutes` of the score log into the current pattern (spec §6). `replace` None: a pattern with
    /// notes asks first (Merge / Replace / Cancel).
    fn dump_score_log(&mut self, minutes: u32, replace: Option<bool>) -> Option<String> {
        use crate::playlist::{Cmd, PItem, Popup};
        let s = &mut self.state;
        let tps = crate::score_log::ticks_per_second(s);
        let channels: Vec<u16> = s.project.channels.iter().map(|c| c.index).collect();
        let mut new = s.rec.log.notes(minutes, std::time::Instant::now(), tps);
        new.retain(|n| channels.contains(&n.channel));
        if new.is_empty() {
            return Some(format!("The score log has no notes from the last {minutes} minutes"));
        }
        let pattern = s.pattern;
        let full = s.project.pattern(pattern).is_some_and(|p| !p.notes.is_empty());
        let replace = match replace {
            Some(r) => r,
            None if full => {
                let (x, y) = self.mouse;
                let items = vec![
                    PItem::head("The pattern isn't empty:"),
                    PItem::new("Merge", Cmd::Rec(RecCmd::DumpAs(minutes, false))),
                    PItem::new("Replace", Cmd::Rec(RecCmd::DumpAs(minutes, true))),
                    PItem::new("Cancel", Cmd::Nothing),
                ];
                self.state.playlist.popup = Some(Popup { x, y, items, sub: None });
                return None;
            }
            None => false,
        };
        let snap = crate::history::Snap::pattern(s, pattern);
        crate::history::record(s, crate::score_log::DUMPED, snap);
        let n = new.len();
        if let Some(p) = s.project.patterns.iter_mut().find(|p| p.number == pattern) {
            if replace {
                p.notes.clear();
            }
            p.notes.extend(new);
        } else {
            s.project.patterns.push(os_model::Pattern { number: pattern, notes: new, ..os_model::Pattern::default() });
        }
        s.piano.dirty = true;
        Some(format!("Dumped {n} note{} into the pattern", if n == 1 { "" } else { "s" }))
    }

    /// Hands the looping time selection to the playback (`Transport::set_loop`); returns it.
    pub(crate) fn sync_loop_range(&self) -> Option<(u32, u32)> {
        let sel = self.selection_loop();
        self.transport.set_loop(sel);
        sel
    }

    /// The time selection that loops playback (spec §5): the piano roll's in PAT mode while it is shown, the
    /// playlist's in SONG mode; one with no length does not count.
    pub(crate) fn selection_loop(&self) -> Option<(u32, u32)> {
        let s = &self.state;
        let r = if s.song_mode { s.playlist.range } else if s.shown(crate::app::Win::PianoRoll) { s.piano.range } else { None };
        r.filter(|r| r.1 > r.0)
    }

    /// Play from stopped (the Play button, Space, Recording starts playback), with the record options (spec §3):
    /// Start on input waits for the first note instead (Play again cancels the wait); Recording precount, armed,
    /// counts in first.
    pub(crate) fn play_from_stop(&mut self) {
        let r = &mut self.state.rec;
        if r.waiting {
            r.waiting = false;
        } else if Opt::Wait.on() && (r.armed || !Opt::WaitArmed.on()) {
            r.waiting = true;
        } else {
            self.play_counted_if_armed();
        }
        self.invalidate();
    }

    /// Play, with Recording precount's count-in when armed.
    pub(crate) fn play_counted_if_armed(&mut self) {
        let bars = if self.state.rec.armed && Opt::Precount.on() { precount_bars() } else { 0 };
        self.start_playback_counted(bars);
        if bars > 0 && self.transport.playing() {
            let p = &self.state.project;
            let tempo = p.tick_tempo(p.tempo.unwrap_or(140.0).max(1.0));
            let beat = f64::from(p.beat_ticks()) / f64::from(p.ppq.max(1)) * 60.0 / tempo;
            let beats = bars * u32::from(p.bar().0.max(1));
            self.state.rec.count = Some(CountIn { from: std::time::Instant::now(), beat, beats });
            self.state.rec.shown = beats;
        }
    }

    /// Stop ends a wait or a count-in.
    pub(crate) fn record_stopped(&mut self) {
        self.state.rec.waiting = false;
        self.state.rec.count = None;
    }

    /// Start on input: the first input that counts starts playback from the start (spec §3). `what` is what the input
    /// is (notes, automation); with Respect recording filter, the filter must record it.
    fn start_on_input(&mut self, what: Opt) {
        if self.state.rec.waiting && (!Opt::WaitFilter.on() || what.on()) {
            self.state.rec.waiting = false;
            self.start_playback();
            self.invalidate();
        }
    }

    /// The option buttons' right-click menus (spec §3): Start on input's checks, Recording precount's length.
    pub(crate) fn record_option_menu(&mut self, o: Opt, x: f32, y: f32) {
        use crate::playlist::{Cmd, Mark, PItem, Popup};
        let items = match o {
            Opt::Wait => vec![
                PItem::new("Respect recording filter", Cmd::Record(Opt::WaitFilter)).mark(Mark::Check(Opt::WaitFilter.on())),
                PItem::new("Only when recording", Cmd::Record(Opt::WaitArmed)).mark(Mark::Check(Opt::WaitArmed.on())),
            ],
            Opt::Precount => {
                let bars = precount_bars();
                vec![
                    PItem::head("Precount duration"),
                    PItem::new("1 bar", Cmd::RecPrecountBars(1)).mark(Mark::Radio(bars == 1)),
                    PItem::new("2 bars", Cmd::RecPrecountBars(2)).mark(Mark::Radio(bars == 2)),
                ]
            }
            _ => return,
        };
        self.state.playlist.popup = Some(Popup { x, y, items, sub: None });
    }

    /// An option switch (the buttons, OPTIONS menu, Ctrl+I / Ctrl+P / Ctrl+B); turning Start on input off ends a wait.
    pub(crate) fn toggle_record_option(&mut self, o: Opt) {
        let on = o.toggle();
        if o == Opt::Wait && !on {
            self.state.rec.waiting = false;
        }
        let name = match o {
            Opt::Wait => "Start on input",
            Opt::Precount => "Recording precount",
            Opt::Loop => "Loop record",
            _ => "Blend recorded notes",
        };
        self.state.message = Some(format!("{name} {}", if on { "on" } else { "off" }));
        self.invalidate();
    }

    /// The right-click menu (spec §2 Right-click menu).
    pub(crate) fn record_menu(&mut self, x: f32, y: f32) {
        use crate::playlist::{Cmd, Mark, PItem, Popup};
        let it = |label: &'static str, o: Opt| PItem::new(label, Cmd::Record(o)).mark(Mark::Check(o.on()));
        let items = vec![
            PItem::head("Recording filter"),
            it("Notes", Opt::Notes),
            it("Automation", Opt::Automation),
            // Audio (FL: into the audio editor, or the playlist): the audio editor is to be a plugin of its own, and the
            // playlist's audio comes with One-click audio recording.
            PItem::new("Audio", Cmd::Nothing).off(),
            PItem::head("Options"),
            it("Ask what to record", Opt::Ask),
            it("Recording starts playback", Opt::StartsPlayback),
            it("Disarm on stop", Opt::DisarmOnStop),
            PItem::head("Quantize"),
            it("Note start time", Opt::QuantizeStart),
            it("Note end time", Opt::QuantizeEnd),
        ];
        self.state.playlist.popup = Some(Popup { x, y, items, sub: None });
    }

    /// Whether something played now is recorded: armed, `what` on in the filter, playing and not paused.
    /// Nothing is recorded during a count-in (spec §3).
    fn rec_live(&self, what: Opt) -> bool {
        let r = &self.state.rec;
        r.armed && r.count.is_none() && what.on() && self.transport.playing() && !self.transport.paused()
    }

    /// A channel's volume or pan knob moved: its value goes into the current pattern as a control point at the playback
    /// position while recording automation (spec §2 Taking automation).
    pub(crate) fn record_knob(&mut self, id: KnobId) {
        use os_model::ControlPoint;
        if matches!(id, KnobId::ChanVol(_) | KnobId::ChanPan(_)) {
            self.start_on_input(Opt::Automation);
        }
        if !self.rec_live(Opt::Automation) {
            return;
        }
        let (row, param, value) = match id {
            KnobId::ChanVol(i) => (i, ControlPoint::VOLUME, (self.state.knob(id) * 12800.0).round() as i32),
            KnobId::ChanPan(i) => (i, ControlPoint::PAN, (self.state.knob(id) * 12800.0).round() as i32),
            _ => return,
        };
        let Some(channel) = self.state.project.channels.get(row).map(|c| c.index) else { return };
        let song = self.transport.song();
        let t = self.transport.tick();
        self.open_take(song);
        let Some(at) = self.rec_pattern_tick(t, song) else { return };
        let tick = at.max(0.0).round() as u32;
        let kind = ControlPoint::new(tick, channel, param, value).kind;
        let s = &mut self.state;
        let Some(take) = s.rec.take.as_mut() else { return };
        let last = take.controls.iter().find(|c| c.0 == kind).map(|c| c.1);
        take.controls.retain(|c| c.0 != kind);
        take.controls.push((kind, tick));
        if let Some(last) = last.filter(|&l| l < tick && !Opt::Blend.on()) {
            let p = crate::piano::pattern_mut(s);
            p.controls.retain(|c| !(c.kind == kind && c.position > last && c.position < tick));
        }
        crate::piano::set_control(s, channel, param, tick, value);
        s.piano.dirty = true;
    }

    /// A note went down (typing keyboard, MIDI, Touch controller): recorded from the playback position while
    /// recording (spec §2 Taking notes).
    pub(crate) fn record_down_with(&mut self, src: Src, key: u16, velocity: u8, extra: Extra) {
        let target = extra.channel.or_else(|| crate::piano::channel(&self.state));
        if let Some(channel) = target {
            self.state.rec.log.down(src, key, velocity, channel, std::time::Instant::now());
        }
        self.start_on_input(Opt::Notes);
        if !self.rec_live(Opt::Notes) {
            return;
        }
        let Some(channel) = target else { return };
        let song = self.transport.song();
        let start = self.transport.tick();
        self.open_take(song);
        let Some(at) = self.rec_pattern_tick(start, song) else { return };
        let r = &mut self.state.rec;
        r.held.retain(|h| h.src != src);
        r.held.push(Held { src, key, velocity, channel, start, at, extra, release: None });
        if let Some(t) = r.take.as_mut().filter(|t| !t.channels.contains(&channel)) {
            t.channels.push(channel);
        }
    }

    /// A note went up: written when it was being recorded.
    pub(crate) fn record_up_with(&mut self, src: Src, release: Option<u8>) {
        self.state.rec.log.up(src, std::time::Instant::now());
        let r = &mut self.state.rec;
        let Some(i) = r.held.iter().position(|h| h.src == src) else { return };
        let mut h = r.held.remove(i);
        h.release = release;
        let now = if self.transport.playing() && !self.transport.paused() { Some(self.transport.tick()) } else { r.last };
        self.write_held(h, now);
    }

    /// The take starts: one undo step for it, with the playlist too in SONG mode (a clip may be placed).
    fn open_take(&mut self, song: bool) {
        let s = &mut self.state;
        if s.rec.take.as_ref().is_some_and(|t| t.pattern == s.pattern) {
            return;
        }
        let pattern = s.pattern;
        let snap = crate::history::Snap::pattern(s, pattern);
        let snap = if song {
            crate::playlist::arr_mut(s);
            crate::history::Snap::Several(vec![crate::history::Snap::Playlist(Box::new(crate::playlist::capture(s))), snap])
        } else {
            snap
        };
        crate::history::record(s, RECORDED, snap);
        let before = s.project.pattern(pattern).map(|p| p.notes.clone()).unwrap_or_default();
        s.rec.take = Some(Take { pattern, before, channels: Vec::new(), span: None, new: Vec::new(), controls: Vec::new(), clip: None });
    }

    /// The current pattern's tick at playback tick `t`: the tick itself in PAT mode; in SONG mode where a clip of the
    /// pattern plays there. Past the end of the take's clip that clip is made longer (to the end of `t`'s bar); with
    /// no clip, one is placed from the start of that bar on the first free track (spec §2).
    fn rec_pattern_tick(&mut self, t: f64, song: bool) -> Option<f64> {
        if !song {
            return Some(t);
        }
        let s = &mut self.state;
        let bar = s.project.bar_ticks().max(1);
        let pattern = s.pattern;
        let whole = |c: &os_model::Clip| c.source == ClipSource::Pattern(pattern) && c.window.is_none();
        let ti = t.max(0.0) as u32;
        let mine = s.rec.take.as_ref().and_then(|k| k.clip);
        if let Some((track, at)) = mine.filter(|&(_, at)| ti >= at) {
            if let Some(c) = crate::playlist::arr_mut(s).clips.iter_mut().find(|c| whole(c) && c.track == track && c.position == at) {
                if ti >= at + c.length {
                    c.length = (ti / bar + 1) * bar - at;
                    s.piano.dirty = true;
                }
                return Some(t - f64::from(at));
            }
        }
        if let Some(p) = crate::piano::pattern_tick_in_song(s, t) {
            // The whole-pattern clip playing there becomes the take's.
            let under = crate::playlist::clips(s)
                .iter()
                .filter(|c| whole(c) && c.position <= ti && (f64::from(c.position) + p - t).abs() < 1e-6)
                .max_by_key(|c| c.position)
                .map(|c| (c.track, c.position));
            if let Some(k) = s.rec.take.as_mut().filter(|k| k.clip.is_none()) {
                k.clip = under;
            }
            return Some(p);
        }
        let at = ti / bar * bar;
        let len = crate::piano::pattern_length(s, pattern).max(bar);
        let track = crate::playlist::free_track(s, 0, at, at + len);
        let clip = os_model::Clip::new(at, len, track, ClipSource::Pattern(pattern));
        crate::playlist::arr_mut(s).clips.push(clip);
        if let Some(k) = s.rec.take.as_mut() {
            k.clip = Some((track, at));
        }
        s.piano.dirty = true;
        crate::piano::pattern_tick_in_song(s, t)
    }

    /// Writes held note `h`, ending at playback tick `now` (None: where it started). A note held over the pattern's
    /// loop ends at the pattern's end.
    fn write_held(&mut self, h: Held, now: Option<f64>) {
        let s = &mut self.state;
        let Some(take) = s.rec.take.as_mut() else { return };
        let start = h.at.max(0.0).round() as u32;
        let end = match now {
            Some(n) if n >= h.start => (h.at + (n - h.start)).round() as u32,
            Some(_) => s.project.pattern_length(take.pattern).max(start),
            None => start,
        };
        let pattern = take.pattern;
        let (a, b) = quantized(s, start, end);
        let take = s.rec.take.as_mut().expect("checked");
        let mut note = Note { length: b - a, key: h.key, velocity: h.velocity, ..Note::step(h.channel, a) };
        note.mod_x = h.extra.mod_x.unwrap_or(note.mod_x);
        note.mod_y = h.extra.mod_y.unwrap_or(note.mod_y);
        note.release = h.release.unwrap_or(note.release);
        take.new.push(note);
        let (lo, hi) = take.span.map_or((a, b), |(lo, hi)| (lo.min(a), hi.max(b)));
        take.span = Some((lo, hi));
        // Blend recorded notes: the earlier notes stay (spec §3).
        let (lo, hi) = if Opt::Blend.on() { (0, 0) } else { (lo, hi) };
        let notes: Vec<Note> = take
            .before
            .iter()
            .filter(|n| !(take.channels.contains(&n.channel) && n.position >= lo && n.position < hi))
            .chain(take.new.iter())
            .cloned()
            .collect();
        if let Some(p) = s.project.patterns.iter_mut().find(|p| p.number == pattern) {
            p.notes = notes;
        } else {
            s.project.patterns.push(os_model::Pattern { number: pattern, notes, ..os_model::Pattern::default() });
        }
        // The time selection stays (it may be looping, spec §5).
        let range = s.piano.range;
        s.piano.select(pattern, h.channel, Vec::new());
        s.piano.range = range;
        s.piano.dirty = true;
        self.invalidate();
    }

    /// The take ends: the notes still held end where playback last was.
    pub(crate) fn end_take(&mut self) {
        let held = std::mem::take(&mut self.state.rec.held);
        let last = self.state.rec.last;
        for h in held {
            self.write_held(h, last);
        }
        self.state.rec.take = None;
    }

    /// Each tick: notes the playback position; when playback stops or pauses, or the pattern changes, the take ends;
    /// a stop disarms when "Disarm on stop" is on.
    pub(crate) fn record_tick(&mut self) {
        self.one_click_tick();
        // A count-in ends after its beats (or with the playback); the clock shows each beat left. A wait ends when
        // something else starts playback.
        if let Some(c) = self.state.rec.count {
            let left = c.left();
            if left == 0 || !self.transport.playing() || self.transport.paused() {
                self.state.rec.count = None;
                self.invalidate();
            } else if left != self.state.rec.shown {
                self.state.rec.shown = left;
                self.invalidate();
            }
        }
        if self.state.rec.waiting && self.transport.playing() {
            self.state.rec.waiting = false;
            self.invalidate();
        }
        let run_on = self.state.rec.armed && !Opt::Loop.on();
        run_on_flag().store(run_on, std::sync::atomic::Ordering::Relaxed);
        let sel = self.sync_loop_range();
        let live = self.transport.playing() && !self.transport.paused();
        // Punch (spec §5): armed with Loop record off, playback stops at the selection's end; held notes end there.
        if let Some((_, end)) = sel.filter(|_| live && run_on && self.state.rec.count.is_none()) {
            let end = f64::from(end);
            if self.transport.tick() >= end && self.state.rec.last.is_some_and(|l| l < end) {
                self.state.rec.last = Some(end);
                self.end_take();
                self.act(crate::app::Action::Stop);
                return;
            }
        }
        // Playback went back to the start (a loop, or a jump back): the pass's take ends there, its held notes at the
        // pattern's end; the next note starts a new take (spec §4).
        let now = self.transport.tick();
        if live && self.state.rec.last.is_some_and(|l| now < l) && self.state.rec.take.is_some() {
            let held = std::mem::take(&mut self.state.rec.held);
            for h in held {
                self.write_held(h, Some(f64::MIN));
            }
            self.state.rec.take = None;
        }
        if live {
            self.state.rec.last = Some(now);
        }
        let r = &self.state.rec;
        let changed = r.take.as_ref().is_some_and(|t| t.pattern != self.state.pattern);
        if (r.was_live && !live) || changed {
            self.end_take();
            if !self.transport.playing() && self.state.rec.armed && Opt::DisarmOnStop.on() {
                self.state.rec.armed = false;
                self.invalidate();
            }
        }
        if !live {
            self.state.rec.last = None;
        }
        self.state.rec.was_live = live;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Action;
    use crate::playlist::{Cmd, Popup};
    use os_ui::{Canvas, Rect};
    use winit::keyboard::KeyCode;

    /// A new project's pattern 1 emptied, the first channel selected, the main snap at Step (24 ticks at 96 PPQ);
    /// Record arms without asking.
    fn app() -> App {
        crate::settings_file::set_flag("rec_ask", false);
        let mut app = App::new(None);
        app.state.project = crate::app::test_project();
        app.state.selected_channel = 0;
        app.state.pattern = 1;
        app.state.project.patterns.iter_mut().for_each(|p| p.notes.clear());
        app.state.global_snap.now = Snap::Step;
        app
    }

    /// Typing key `code` going down or up at playback tick `t`.
    fn key(app: &mut App, code: KeyCode, down: bool, t: f64, song: bool) {
        app.transport.pretend_at(t, song);
        app.record_tick();
        assert!(app.typing_key(code, down, false, false));
    }

    fn stop(app: &mut App) {
        app.transport.stop();
        app.record_tick();
    }

    /// Pattern 1's notes as (position, length, channel).
    fn notes(app: &App) -> Vec<(u32, u32, u16)> {
        let mut v: Vec<_> =
            app.state.project.pattern(1).map_or(vec![], |p| p.notes.iter().map(|n| (n.position, n.length, n.channel)).collect());
        v.sort_unstable();
        v
    }

    fn hit_rect(app: &App, a: &Action) -> Rect {
        app.hits.iter().rev().find(|h| &h.action == a).map(|h| h.rect).unwrap_or_else(|| panic!("no hit for {a:?}"))
    }

    /// A click asks what to record, and the pick sets the filter and arms; a click while armed disarms; with asking off
    /// a click arms at once. The right-click menu holds the filter, the options and the quantize checks, and its
    /// entries toggle program settings (spec §2 The button, What to record, Right-click menu).
    #[test]
    fn button_and_menu() {
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = app();
        crate::settings_file::set_flag("rec_ask", true);
        app.paint(&mut c, None);
        let b = hit_rect(&app, &Action::Record);
        let hint = app.hits.iter().rev().find(|h| h.action == Action::Record).map(|h| h.hint.to_string());
        assert_eq!(hint.as_deref(), Some("Record (notes, automation) (R) - right-click for options"));
        app.mouse_move(b.cx(), b.cy());
        app.mouse_down();
        app.mouse_up();
        assert!(!app.state.rec.armed, "it asks first");
        let Some(Popup { items, x, .. }) = app.state.playlist.popup.clone() else { panic!("no question") };
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_ref()).collect();
        assert_eq!(
            labels,
            [
                "What would you like to record?",
                "Notes and automation",
                "Notes only",
                "Automation only",
                "Audio, into the playlist as an audio clip",
                "Audio, into the audio editor",
                "",
                "Don't ask again"
            ]
        );
        assert_eq!(x, b.x, "under the button");
        assert!(!items[5].enabled);
        app.state.playlist.popup = None;
        crate::playlist::run(&mut app, Cmd::Rec(RecCmd::Arm { notes: true, automation: false }));
        assert!(app.state.rec.armed && Opt::Notes.on() && !Opt::Automation.on());
        app.toggle_record();
        assert!(!app.state.rec.armed && app.state.playlist.popup.is_none(), "disarming never asks");
        crate::playlist::run(&mut app, Cmd::Rec(RecCmd::AskOff));
        assert!(!Opt::Ask.on());
        crate::playlist::run(&mut app, Cmd::Record(Opt::Automation));
        app.mouse_down();
        app.mouse_up();
        assert!(app.state.rec.armed);
        app.mouse_down_right();
        app.mouse_up();
        let Some(Popup { items, .. }) = app.state.playlist.popup.clone() else { panic!("no menu") };
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_ref()).collect();
        assert_eq!(
            labels,
            [
                "Recording filter",
                "Notes",
                "Automation",
                "Audio",
                "Options",
                "Ask what to record",
                "Recording starts playback",
                "Disarm on stop",
                "Quantize",
                "Note start time",
                "Note end time"
            ]
        );
        assert!(Opt::Notes.on() && Opt::Automation.on() && Opt::QuantizeStart.on() && Opt::QuantizeEnd.on() && !Opt::DisarmOnStop.on());
        crate::playlist::run(&mut app, Cmd::Record(Opt::DisarmOnStop));
        assert!(Opt::DisarmOnStop.on());
        app.state.playlist.popup = None;
        app.mouse_down();
        app.mouse_up();
        assert!(!app.state.rec.armed, "a click disarms");
    }

    /// PAT mode: notes go in the current pattern for the selected channel, quantized to the main snap, as long as they
    /// were held; a second take replaces the earlier notes starting inside it; each take is one undo step.
    #[test]
    fn records_and_replaces_in_pat_mode() {
        let mut app = app();
        // Not armed: nothing is written while playing.
        key(&mut app, KeyCode::KeyZ, true, 10.0, false);
        key(&mut app, KeyCode::KeyZ, false, 50.0, false);
        assert!(notes(&app).is_empty());
        app.toggle_record();
        // 30..100 rounds to 24..96; 190..205 to 192..216 (the end rounds onto the start: one step long).
        key(&mut app, KeyCode::KeyZ, true, 30.0, false);
        assert!(app.state.rec.recording());
        key(&mut app, KeyCode::KeyZ, false, 100.0, false);
        key(&mut app, KeyCode::KeyX, true, 190.0, false);
        key(&mut app, KeyCode::KeyX, false, 205.0, false);
        stop(&mut app);
        assert_eq!(notes(&app), [(24, 72, 0), (192, 24, 0)]);
        assert!(!app.state.rec.recording() && app.state.rec.armed, "a stop ends the take and keeps Record armed");
        assert_eq!(app.state.history.entries().last().map(|e| e.1), Some(RECORDED));

        // Another take from 170 to 260: the note starting at 192 goes, the one at 24 (before the take) stays.
        let steps = app.state.history.len();
        key(&mut app, KeyCode::KeyC, true, 170.0, false);
        key(&mut app, KeyCode::KeyC, false, 260.0, false);
        stop(&mut app);
        assert_eq!(notes(&app), [(24, 72, 0), (168, 96, 0)]);
        assert_eq!(app.state.history.len(), steps + 1);
        crate::history::undo(&mut app.state);
        assert_eq!(notes(&app), [(24, 72, 0), (192, 24, 0)]);

        // A note held when playback stops ends where playback last was; Line leaves times as played.
        app.state.global_snap.now = Snap::Line;
        key(&mut app, KeyCode::KeyV, true, 301.0, false);
        app.transport.pretend_at(333.0, false);
        app.record_tick();
        stop(&mut app);
        assert!(notes(&app).contains(&(301, 32, 0)), "{:?}", notes(&app));
    }

    /// SONG mode: with no clip of the pattern at the song position, one is placed from the start of that bar on the
    /// first free track, and the note lands in it (spec §2 Taking notes).
    #[test]
    fn places_a_clip_in_song_mode() {
        let mut app = app();
        crate::playlist::arr_mut(&mut app.state).clips.clear();
        app.toggle_record();
        let bar = f64::from(app.state.project.bar_ticks());
        key(&mut app, KeyCode::KeyZ, true, bar + 48.0, true);
        key(&mut app, KeyCode::KeyZ, false, bar + 96.0, true);
        stop(&mut app);
        let clips = crate::playlist::clips(&app.state);
        assert_eq!(clips.len(), 1);
        assert_eq!((clips[0].position, clips[0].track, clips[0].source), (bar as u32, 0, ClipSource::Pattern(1)));
        assert_eq!(notes(&app), [(48, 48, 0)]);
        // Undo takes the clip away with the notes.
        crate::history::undo(&mut app.state);
        assert!(crate::playlist::clips(&app.state).is_empty() && notes(&app).is_empty());
    }

    /// SONG mode: one clip per take; when playback runs past its end the clip is made longer to the bar's end instead
    /// of a second clip being placed (spec §2 Taking notes).
    #[test]
    fn extends_the_take_clip_in_song_mode() {
        let mut app = app();
        crate::playlist::arr_mut(&mut app.state).clips.clear();
        app.toggle_record();
        let bar = app.state.project.bar_ticks();
        let b = f64::from(bar);
        key(&mut app, KeyCode::KeyZ, true, 48.0, true);
        key(&mut app, KeyCode::KeyZ, false, 96.0, true);
        key(&mut app, KeyCode::KeyX, true, b + 48.0, true);
        key(&mut app, KeyCode::KeyX, false, b + 72.0, true);
        stop(&mut app);
        let clips = crate::playlist::clips(&app.state);
        assert_eq!(clips.len(), 1, "{clips:?}");
        assert_eq!((clips[0].position, clips[0].length), (0, 2 * bar));
        assert_eq!(notes(&app), [(48, 48, 0), (bar + 48, 24, 0)]);
    }

    /// R arms like a click when the typing keyboard is off; with it on, R is a note.
    #[test]
    fn r_arms_when_the_typing_keyboard_is_off() {
        use winit::keyboard::Key;
        let mut app = app();
        app.state.toggles.retain(|t| *t != "typing");
        app.key(&Key::Character("r".into()), false, false);
        assert!(app.state.rec.armed);
        app.key(&Key::Character("r".into()), false, false);
        assert!(!app.state.rec.armed);
    }

    /// The metronome menu's Recording precount: Off, 1 bar, 2 bars set the precount switch and length (spec §3).
    #[test]
    fn metronome_menu_sets_the_precount() {
        let mut app = app();
        app.metronome_menu(10.0, 10.0);
        let Some(Popup { items, .. }) = app.state.playlist.popup.clone() else { panic!("no menu") };
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_ref()).collect();
        assert_eq!(labels[labels.len() - 4..], ["Recording precount", "Off", "1 bar", "2 bars"]);
        crate::playlist::run(&mut app, Cmd::Rec(RecCmd::Precount(2)));
        assert!(Opt::Precount.on() && precount_bars() == 2);
        crate::playlist::run(&mut app, Cmd::Rec(RecCmd::Precount(0)));
        assert!(!Opt::Precount.on());
    }

    /// A piano roll time selection in PAT mode loops playback (the engines' loop range); armed with Loop record off,
    /// playback stops at its end, held notes end there, and Stop goes back to its start (spec §5).
    #[test]
    fn time_selection_loops_and_punches() {
        let mut app = app();
        if !app.state.shown(crate::app::Win::PianoRoll) {
            app.state.z.push(crate::app::Win::PianoRoll);
        }
        app.state.piano.range = Some((96, 192));
        assert_eq!(app.selection_loop(), Some((96, 192)));
        app.toggle_record();
        key(&mut app, KeyCode::KeyZ, true, 120.0, false);
        app.transport.pretend_at(170.0, false);
        app.record_tick();
        assert_eq!(app.state.piano.range, Some((96, 192)), "recording keeps the selection");
        app.transport.pretend_at(200.0, false);
        app.record_tick();
        assert!(!app.transport.playing(), "punched out");
        assert_eq!(notes(&app), [(120, 72, 0)], "the held note ends at the selection's end");
        assert_eq!(app.state.piano.marker, 96);
        app.state.piano.range = None;
        assert_eq!(app.selection_loop(), None);
    }

    /// Dump score log: notes played (playing or not) go into the pattern at their real time offsets from the first,
    /// unquantized; a full pattern asks Merge / Replace; one undo step (spec §6).
    #[test]
    fn dumps_the_score_log() {
        use std::time::{Duration, Instant};
        let mut app = app();
        let now = Instant::now();
        let t0 = now.checked_sub(Duration::from_secs(10)).expect("a clock");
        let src = |c| crate::step_edit::Src::Typing(c);
        let log = &mut app.state.rec.log;
        log.down(src(KeyCode::KeyZ), 60, 100, 0, t0);
        log.up(src(KeyCode::KeyZ), t0 + Duration::from_millis(300));
        log.down(src(KeyCode::KeyX), 62, 90, 0, t0 + Duration::from_millis(700));
        log.up(src(KeyCode::KeyX), t0 + Duration::from_millis(1000));
        let tps = crate::score_log::ticks_per_second(&app.state);
        let msg = crate::playlist::run(&mut app, Cmd::Rec(RecCmd::Dump(2)));
        assert_eq!(msg.as_deref(), Some("Dumped 2 notes into the pattern"));
        let t = |s: f64| (s * tps).round() as u32;
        assert_eq!(notes(&app), [(0, t(0.3), 0), (t(0.7), t(0.3), 0)]);
        assert_eq!(app.state.history.entries().last().map(|e| e.1), Some(crate::score_log::DUMPED));
        // A full pattern: asks first.
        crate::playlist::run(&mut app, Cmd::Rec(RecCmd::Dump(2)));
        let Some(Popup { items, .. }) = app.state.playlist.popup.clone() else { panic!("no question") };
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_ref()).collect();
        assert_eq!(labels, ["The pattern isn't empty:", "Merge", "Replace", "Cancel"]);
        crate::playlist::run(&mut app, Cmd::Rec(RecCmd::DumpAs(2, false)));
        assert_eq!(notes(&app).len(), 4);
        crate::playlist::run(&mut app, Cmd::Rec(RecCmd::DumpAs(2, true)));
        assert_eq!(notes(&app).len(), 2);
        crate::playlist::run(&mut app, Cmd::Rec(RecCmd::ClearLog));
        let msg = crate::playlist::run(&mut app, Cmd::Rec(RecCmd::Dump(2)));
        assert_eq!(msg.as_deref(), Some("The score log has no notes from the last 2 minutes"));
    }

    /// Pattern 1's control points as (position, channel, param, value).
    fn controls(app: &App) -> Vec<(u32, u16, u16, i32)> {
        app.state.project.pattern(1).map_or(vec![], |p| p.controls.iter().map(|c| (c.position, c.channel(), c.param(), c.value)).collect())
    }

    /// A channel volume knob dragged while recording becomes the pattern's control points at the playback position;
    /// the sweep replaces the earlier points between its points; undo puts them back; pan records too; with
    /// Automation off nothing is written (spec §2 Taking automation).
    #[test]
    fn records_channel_knobs() {
        use os_model::ControlPoint;
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = app();
        let ch = app.state.project.channels[1].index;
        crate::piano::set_control(&mut app.state, ch, ControlPoint::VOLUME, 70, 1000);
        crate::piano::set_control(&mut app.state, ch, ControlPoint::VOLUME, 200, 2000);
        app.paint(&mut c, None);
        let k = hit_rect(&app, &Action::Knob(KnobId::ChanVol(1)));
        app.toggle_record();
        app.transport.pretend_at(48.0, false);
        app.record_tick();
        app.mouse_move(k.cx(), k.cy());
        app.mouse_down();
        app.mouse_move(k.cx(), k.cy() + 10.0);
        let mid = (app.state.knob(KnobId::ChanVol(1)) * 12800.0).round() as i32;
        app.transport.pretend_at(96.0, false);
        app.mouse_move(k.cx(), k.cy() + 20.0);
        app.mouse_up();
        let end = (app.state.knob(KnobId::ChanVol(1)) * 12800.0).round() as i32;
        assert!(end < mid);
        let vol = |app: &App| controls(app).into_iter().filter(|c| c.2 == ControlPoint::VOLUME).map(|c| (c.0, c.3)).collect::<Vec<_>>();
        assert_eq!(vol(&app), [(48, mid), (96, end), (200, 2000)], "the point at 70 is replaced, the one at 200 stays");
        assert!(controls(&app).iter().all(|c| c.1 == ch));

        // Pan, by the wheel path's call.
        app.state.set_knob(KnobId::ChanPan(1), 0.25);
        app.record_knob(KnobId::ChanPan(1));
        assert!(controls(&app).contains(&(96, ch, ControlPoint::PAN, 3200)));
        stop(&mut app);
        crate::history::undo(&mut app.state);
        assert_eq!(vol(&app), [(70, 1000), (200, 2000)]);

        // Automation off: knob moves write nothing.
        Opt::Automation.toggle();
        app.transport.pretend_at(10.0, false);
        app.record_tick();
        app.state.set_knob(KnobId::ChanVol(1), 0.1);
        app.record_knob(KnobId::ChanVol(1));
        assert_eq!(vol(&app), [(70, 1000), (200, 2000)]);
        assert_eq!(crate::record::hint(), "Record (notes) (R) - right-click for options");
    }

    /// "Disarm on stop" disarms at the stop; step edit never takes a recorded note.
    #[test]
    fn disarm_on_stop_and_step_edit() {
        let mut app = app();
        Opt::DisarmOnStop.toggle();
        crate::step_edit::toggle();
        app.toggle_record();
        key(&mut app, KeyCode::KeyZ, true, 0.0, false);
        key(&mut app, KeyCode::KeyZ, false, 24.0, false);
        stop(&mut app);
        assert!(!app.state.rec.armed);
        assert_eq!(notes(&app), [(0, 24, 0)], "only the recorded note: step edit wrote nothing while playing");
    }

    /// The option buttons switch their settings and say so in their hints; Start on input and Recording precount have
    /// right-click menus; Ctrl+I / Ctrl+P / Ctrl+B and the OPTIONS entries switch the same settings (spec §3).
    #[test]
    fn option_buttons_menus_and_keys() {
        use winit::keyboard::{Key, SmolStr};
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = app();
        app.paint(&mut c, None);
        for o in [Opt::Wait, Opt::Precount, Opt::Blend] {
            let hint = app.hits.iter().rev().find(|h| h.action == Action::RecOpt(o)).map(|h| h.hint.to_string());
            assert_eq!(hint.as_deref(), Some(option_hint(o)));
        }
        let b = hit_rect(&app, &Action::RecOpt(Opt::Blend));
        app.mouse_move(b.cx(), b.cy());
        app.mouse_down();
        app.mouse_up();
        assert!(Opt::Blend.on());
        let p = hit_rect(&app, &Action::RecOpt(Opt::Precount));
        app.mouse_move(p.cx(), p.cy());
        app.mouse_down_right();
        app.mouse_up();
        let Some(Popup { items, .. }) = app.state.playlist.popup.take() else { panic!("no menu") };
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_ref()).collect();
        assert_eq!(labels, ["Precount duration", "1 bar", "2 bars"]);
        crate::playlist::run(&mut app, Cmd::RecPrecountBars(2));
        assert_eq!(precount_bars(), 2);
        let w = hit_rect(&app, &Action::RecOpt(Opt::Wait));
        app.mouse_move(w.cx(), w.cy());
        app.mouse_down_right();
        app.mouse_up();
        let Some(Popup { items, .. }) = app.state.playlist.popup.take() else { panic!("no menu") };
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_ref()).collect();
        assert_eq!(labels, ["Respect recording filter", "Only when recording"]);
        let ctrl = |app: &mut App, ch: &str| app.key(&Key::Character(SmolStr::new(ch)), true, false);
        // A focused playlist keeps Ctrl+P (Performance mode) and Ctrl+B (duplicate); elsewhere they switch the options.
        assert!(app.pl_focus());
        ctrl(&mut app, "i");
        ctrl(&mut app, "p");
        ctrl(&mut app, "b");
        assert_eq!((Opt::Wait.on(), Opt::Precount.on(), Opt::Blend.on()), (true, false, true));
        app.state.z.retain(|&w| w != crate::app::Win::Playlist && w != crate::app::Win::PianoRoll);
        ctrl(&mut app, "p");
        ctrl(&mut app, "b");
        assert_eq!((Opt::Wait.on(), Opt::Precount.on(), Opt::Blend.on()), (true, true, false));
        assert_eq!(crate::views::menus::record_option("Start on input"), Some(Opt::Wait));
    }

    /// Start on input: Play waits; the first note starts playback from the start and is recorded there; Play again
    /// cancels a wait; with Respect recording filter and Notes off, a note does not start it (spec §3).
    #[test]
    fn start_on_input() {
        let mut app = app();
        Opt::Wait.toggle();
        app.toggle_record();
        app.act(Action::Play);
        assert!(app.state.rec.waiting && !app.transport.playing());
        app.act(Action::Play);
        assert!(!app.state.rec.waiting, "Play again cancels");
        app.act(Action::Play);
        Opt::Notes.toggle();
        assert!(app.typing_key(KeyCode::KeyZ, true, false, false));
        assert!(app.typing_key(KeyCode::KeyZ, false, false, false));
        assert!(app.state.rec.waiting, "Notes are off in the filter");
        Opt::Notes.toggle();
        assert!(app.typing_key(KeyCode::KeyZ, true, false, false));
        assert!(!app.state.rec.waiting && app.transport.playing() && app.transport.tick() == 0.0);
        assert!(app.state.rec.recording(), "the note that started playback is recorded");
        key(&mut app, KeyCode::KeyZ, false, 48.0, false);
        stop(&mut app);
        assert_eq!(notes(&app), [(0, 48, 0)]);
        // Only when recording: disarmed, Play plays at once.
        Opt::WaitArmed.toggle();
        app.toggle_record();
        app.act(Action::Play);
        assert!(!app.state.rec.waiting && app.transport.playing());
        app.act(Action::Stop);
    }

    /// Recording precount, armed: Play counts in (the clock's beats left) and nothing is recorded until it is over;
    /// disarmed, Play plays at once (spec §3).
    #[test]
    fn precount_counts_in_when_armed() {
        let mut app = app();
        Opt::Precount.toggle();
        app.act(Action::Play);
        assert!(app.state.rec.count.is_none(), "disarmed: no count-in");
        app.act(Action::Stop);
        app.toggle_record();
        app.act(Action::Play);
        let beats = u32::from(app.state.project.bar().0);
        assert_eq!(app.state.rec.count.map(|c| c.left()), Some(beats));
        assert!(app.typing_key(KeyCode::KeyZ, true, false, false));
        assert!(app.typing_key(KeyCode::KeyZ, false, false, false));
        assert!(!app.state.rec.recording(), "not during the count-in");
        app.act(Action::Stop);
        assert!(app.state.rec.count.is_none(), "Stop ends it");
    }

    /// Loop record: the flag the engines read is set while armed with Loop record off; each pass of a loop is a take
    /// of its own, which replaces what the pass before recorded there, and a note held over the loop ends at the
    /// pattern's end (spec §4).
    #[test]
    fn loop_record_passes() {
        use std::sync::atomic::Ordering;
        let mut app = app();
        app.record_tick();
        assert!(!run_on_flag().load(Ordering::Relaxed), "disarmed");
        app.toggle_record();
        app.record_tick();
        assert!(run_on_flag().load(Ordering::Relaxed), "armed, Loop record off: runs on");
        app.toggle_record_option(Opt::Loop);
        app.record_tick();
        assert!(!run_on_flag().load(Ordering::Relaxed), "Loop record on: loops");
        let len = f64::from(app.state.project.pattern_length(1).max(1));
        let steps = app.state.history.len();
        key(&mut app, KeyCode::KeyZ, true, 48.0, false);
        key(&mut app, KeyCode::KeyZ, false, 96.0, false);
        key(&mut app, KeyCode::KeyX, true, len - 24.0, false);
        // The loop: a new pass, a new take.
        key(&mut app, KeyCode::KeyC, true, 48.0, false);
        key(&mut app, KeyCode::KeyC, false, 72.0, false);
        stop(&mut app);
        let l = len as u32;
        assert_eq!(notes(&app), [(48, 24, 0), (l - 24, 24, 0)], "the second pass replaced the first's note at 48");
        assert_eq!(app.state.history.len(), steps + 2, "one undo step per pass");
        crate::history::undo(&mut app.state);
        assert_eq!(notes(&app), [(48, 48, 0), (l - 24, 24, 0)]);
    }

    /// Blend recorded notes: a second take keeps the notes it overlaps (spec §3).
    #[test]
    fn blend_keeps_earlier_notes() {
        let mut app = app();
        app.toggle_record();
        key(&mut app, KeyCode::KeyZ, true, 0.0, false);
        key(&mut app, KeyCode::KeyZ, false, 96.0, false);
        stop(&mut app);
        Opt::Blend.toggle();
        key(&mut app, KeyCode::KeyX, true, 0.0, false);
        key(&mut app, KeyCode::KeyX, false, 48.0, false);
        stop(&mut app);
        assert_eq!(notes(&app), [(0, 48, 0), (0, 96, 0)]);
    }
}
