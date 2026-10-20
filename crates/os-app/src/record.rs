// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Recording (the toolbar's Record button): while it is armed and the transport plays, the notes played on the typing
//! keyboard, a MIDI keyboard or the Touch controller are written into the current pattern for the selected channel,
//! each as long as it was held. A take (from its first note to the stop, a pause, disarming or a pattern change) is one
//! undo step and replaces the pattern's earlier notes that start inside it. The right-click menu's checks are program
//! settings in `settings.txt`; the arm itself is not saved.
// spec: ui/recording.md

use crate::app::{App, State};
use crate::playlist::Snap;
use crate::step_edit::Src;
use os_model::{ClipSource, Note};

/// The undo step of a take (FL's name for it).
pub const RECORDED: &str = "Recorded items";

/// The right-click menu's checks: key in `settings.txt` and default (spec §2 Right-click menu).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Opt {
    Notes,
    StartsPlayback,
    DisarmOnStop,
    QuantizeStart,
    QuantizeEnd,
}

impl Opt {
    fn key(self) -> (&'static str, bool) {
        match self {
            Opt::Notes => ("rec_notes", true),
            Opt::StartsPlayback => ("rec_starts_play", false),
            Opt::DisarmOnStop => ("rec_disarm_stop", false),
            Opt::QuantizeStart => ("rec_q_start", true),
            Opt::QuantizeEnd => ("rec_q_end", true),
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

/// A note held while recording: where it started, in the pattern and in the running playback's ticks.
#[derive(Debug, Clone, Copy)]
struct Held {
    src: Src,
    key: u16,
    velocity: u8,
    channel: u16,
    start: f64,
    at: f64,
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
}

/// Record's state (spec §2).
#[derive(Debug, Default)]
pub struct Rec {
    pub armed: bool,
    held: Vec<Held>,
    take: Option<Take>,
    /// The playback position at the last poll while playing: where held notes end when playback stops.
    last: Option<f64>,
    was_live: bool,
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
    /// The Record button: arms or disarms (spec §2 The button). Arming while stopped starts playback when "Recording
    /// starts playback" is on.
    pub(crate) fn toggle_record(&mut self) {
        if self.state.rec.armed {
            self.end_take();
            self.state.rec.armed = false;
        } else {
            self.state.rec.armed = true;
            if !self.transport.playing() && Opt::StartsPlayback.on() {
                self.start_playback();
            }
        }
        self.invalidate();
    }

    /// The right-click menu (spec §2 Right-click menu).
    pub(crate) fn record_menu(&mut self, x: f32, y: f32) {
        use crate::playlist::{Cmd, Mark, PItem, Popup};
        let it = |label: &'static str, o: Opt| PItem::new(label, Cmd::Record(o)).mark(Mark::Check(o.on()));
        let items = vec![
            PItem::head("Recording filter"),
            it("Notes", Opt::Notes),
            PItem::new("Automation", Cmd::Nothing).off(),
            PItem::head("Options"),
            it("Recording starts playback", Opt::StartsPlayback),
            it("Disarm on stop", Opt::DisarmOnStop),
            PItem::head("Quantize"),
            it("Note start time", Opt::QuantizeStart),
            it("Note end time", Opt::QuantizeEnd),
        ];
        self.state.playlist.popup = Some(Popup { x, y, items, sub: None });
    }

    /// Whether a note played now is recorded: armed, notes on, playing and not paused.
    fn rec_live(&self) -> bool {
        self.state.rec.armed && Opt::Notes.on() && self.transport.playing() && !self.transport.paused()
    }

    /// A note went down (typing keyboard, MIDI, Touch controller): recorded from the playback position while
    /// recording (spec §2 Taking notes).
    pub(crate) fn record_down(&mut self, src: Src, key: u16, velocity: u8) {
        if !self.rec_live() {
            return;
        }
        let Some(channel) = crate::piano::channel(&self.state) else { return };
        let song = self.transport.song();
        let start = self.transport.tick();
        self.open_take(song);
        let Some(at) = self.rec_pattern_tick(start, song) else { return };
        let r = &mut self.state.rec;
        r.held.retain(|h| h.src != src);
        r.held.push(Held { src, key, velocity, channel, start, at });
        if let Some(t) = r.take.as_mut().filter(|t| !t.channels.contains(&channel)) {
            t.channels.push(channel);
        }
    }

    /// A note went up: written when it was being recorded.
    pub(crate) fn record_up(&mut self, src: Src) {
        let r = &mut self.state.rec;
        let Some(i) = r.held.iter().position(|h| h.src == src) else { return };
        let h = r.held.remove(i);
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
        s.rec.take = Some(Take { pattern, before, channels: Vec::new(), span: None, new: Vec::new() });
    }

    /// The current pattern's tick at playback tick `t`: the tick itself in PAT mode; in SONG mode where a clip of the
    /// pattern plays there, after placing one from the start of that bar on the first free track when none does.
    fn rec_pattern_tick(&mut self, t: f64, song: bool) -> Option<f64> {
        if !song {
            return Some(t);
        }
        let s = &mut self.state;
        if let Some(p) = crate::piano::pattern_tick_in_song(s, t) {
            return Some(p);
        }
        let bar = s.project.bar_ticks().max(1);
        let at = (t.max(0.0) as u32) / bar * bar;
        let len = crate::piano::pattern_length(s, s.pattern).max(bar);
        let track = crate::playlist::free_track(s, 0, at, at + len);
        let clip = os_model::Clip::new(at, len, track, ClipSource::Pattern(s.pattern));
        crate::playlist::arr_mut(s).clips.push(clip);
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
        let note = Note { length: b - a, key: h.key, velocity: h.velocity, ..Note::step(h.channel, a) };
        take.new.push(note);
        let (lo, hi) = take.span.map_or((a, b), |(lo, hi)| (lo.min(a), hi.max(b)));
        take.span = Some((lo, hi));
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
        s.piano.select(pattern, h.channel, Vec::new());
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
        let live = self.transport.playing() && !self.transport.paused();
        if live {
            self.state.rec.last = Some(self.transport.tick());
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

    /// A new project's pattern 1 emptied, the first channel selected, the main snap at Step (24 ticks at 96 PPQ).
    fn app() -> App {
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

    /// The button arms and disarms; its right-click menu holds the filter, the options and the quantize checks, and its
    /// entries toggle program settings (spec §2 The button, Right-click menu).
    #[test]
    fn button_and_menu() {
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = app();
        app.paint(&mut c, None);
        let b = hit_rect(&app, &Action::Record);
        let hint = app.hits.iter().rev().find(|h| h.action == Action::Record).map(|h| h.hint.to_string());
        assert_eq!(hint.as_deref(), Some("Record (notes) - right-click for options"));
        app.mouse_move(b.cx(), b.cy());
        app.mouse_down();
        app.mouse_up();
        assert!(app.state.rec.armed);
        app.mouse_down_right();
        app.mouse_up();
        let Some(Popup { items, .. }) = app.state.playlist.popup.clone() else { panic!("no menu") };
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_ref()).collect();
        assert_eq!(
            labels,
            ["Recording filter", "Notes", "Automation", "Options", "Recording starts playback", "Disarm on stop", "Quantize", "Note start time", "Note end time"]
        );
        assert!(Opt::Notes.on() && Opt::QuantizeStart.on() && Opt::QuantizeEnd.on() && !Opt::DisarmOnStop.on());
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
        key(&mut app, KeyCode::KeyX, true, 190.0, false);
        key(&mut app, KeyCode::KeyZ, false, 100.0, false);
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
}
