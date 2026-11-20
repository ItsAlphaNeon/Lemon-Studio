// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The interface's side of `crate::record` (step 9: the session's side stays there).

#![allow(unused_imports)]
use crate::app::{App, State};
use crate::session::{Session, View};
use crate::knobs::KnobId;
use crate::playlist::Snap;
use crate::step_edit::Src;
use os_model::{ClipSource, Note, TimeMarker};
#[allow(unused_imports)]
use crate::record::*;

/// The main snap step that quantizing rounds to; None for Line, Cell and (none), which leave times as played.
fn unit(s: &State) -> Option<u32> {
    match s.global_snap.now {
        Snap::Line | Snap::Cell | Snap::None => None,
        sn => Some(sn.ticks(u32::from(s.session.project.ppq.max(4)), 0.0).max(1)),
    }
}

/// A note from `start` to `end` (pattern ticks) quantized as the menu says; never ending at or before its start.
pub fn quantized(s: &State, start: u32, end: u32) -> (u32, u32) {
    quantized_on(&s.session.settings, unit(s), start, end)
}

impl App {
    /// The Record button and R: disarms, or asks what to record (spec §2 What to record), or arms when asking is off.
    pub fn toggle_record(&mut self) {
        if self.state.session.rec.armed {
            self.end_take();
            self.state.session.rec.armed = false;
        } else if Opt::Ask.on(&self.state.session.settings) {
            self.record_question();
        } else {
            self.arm();
        }
        self.invalidate();
    }

    /// Arms; while stopped this starts playback when "Recording starts playback" is on.
    fn arm(&mut self) {
        self.state.session.rec.armed = true;
        if !self.state.session.transport.playing() && !self.state.session.rec.waiting && Opt::StartsPlayback.on(&self.state.session.settings) {
            self.play_from_stop();
        }
        self.invalidate();
    }

    /// The question popup, under the Record button.
    fn record_question(&mut self) {
        use crate::playlist::{Cmd, PItem, Popup};
        let arm = |label: &'static str, notes: bool, automation: bool| PItem::new(label, Cmd::Rec(RecCmd::Arm { notes, automation }));
        let audio = crate::one_click_ui::inputs(false);
        let items = vec![
            PItem::head("What would you like to record?"),
            arm("Notes and automation", true, true),
            arm("Notes only", true, false),
            arm("Automation only", false, true),
            PItem::sub("Audio, into the arranger as an audio clip", audio),
            // Asks for the input, then records into Tesla on the master track (spec: ui/one-click-recording.md §2).
            PItem::new("Audio, into the audio editor", Cmd::OneClickEditor),
            PItem::sep(),
            PItem::new("Don't ask again", Cmd::Rec(RecCmd::AskOff)),
        ];
        let (x, y) = self.hits.iter().find(|h| h.action == crate::app::Action::Record).map_or((486.0, 35.0), |h| (h.rect.x, h.rect.bottom() + 2.0));
        self.state.playlist.popup = Some(Popup { x, y, items, sub: None, scroll: [0; 2] });
    }

    /// A record popup's command (`Cmd::Rec`); returns the message to show.
    pub fn rec_cmd(&mut self, c: RecCmd) -> Option<String> {
        let set = |o: Opt, on: bool| {
            if o.on(&self.state.session.settings) != on {
                o.toggle(&self.state.session.settings);
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
                    set_precount_bars(&self.state.session.settings, bars);
                }
                Some(if bars > 0 { format!("Count-in: {bars} bar{}", if bars == 1 { "" } else { "s" }) } else { "Count-in off".into() })
            }
            RecCmd::Dump(minutes) => self.dump_score_log(minutes, None),
            RecCmd::DumpAs(minutes, replace) => self.dump_score_log(minutes, Some(replace)),
            RecCmd::ClearLog => {
                self.state.session.rec.log.clear();
                Some("Score log cleared".into())
            }
            RecCmd::AudioLog(m) => Some(self.set_audio_log_minutes(m)),
        };
        self.invalidate();
        msg
    }

    /// Dumps the last `minutes` of the score log into the current pattern (spec §6). `replace` None: a pattern with
    /// notes asks first (Merge / Replace / Cancel).
    fn dump_score_log(&mut self, minutes: u32, replace: Option<bool>) -> Option<String> {
        use crate::playlist::{Cmd, PItem, Popup};
        let s = &mut self.state;
        let tps = crate::score_log_ui::ticks_per_second(s);
        let channels: Vec<u16> = s.session.project.channels.iter().map(|c| c.index).collect();
        let mut new = s.session.rec.log.notes(minutes, std::time::Instant::now(), tps);
        new.retain(|n| channels.contains(&n.channel));
        if new.is_empty() {
            return Some(format!("The score log has no notes from the last {minutes} minutes"));
        }
        let pattern = s.pattern();
        let full = s.session.project.pattern(pattern).is_some_and(|p| !p.notes.is_empty());
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
                self.state.playlist.popup = Some(Popup { x, y, items, sub: None, scroll: [0; 2] });
                return None;
            }
            None => false,
        };
        let n = new.len();
        // The pattern (made when it is not there yet), then its notes: the kept ones and the log's after them.
        use crate::control::command::{Command, NoteIn};
        crate::mixer::track_ids(s);
        let first = if s.session.project.pattern(pattern).is_none() { vec![Command::AddPattern { name: None, color: None, number: Some(pattern) }] } else { Vec::new() };
        let r = crate::control_ui::command_then(s, crate::score_log::DUMPED, first, |s, _| {
            let p = &s.session.project;
            let Some(pat) = p.pattern(pattern) else { return Vec::new() };
            let id = |ch: u16| p.channels.iter().find(|c| c.index == ch).map(|c| c.id.0);
            let kept = pat.notes.iter().filter(|_| !replace);
            let list = kept.chain(&new).filter_map(|n| Some(NoteIn::of(n, id(n.channel)?))).collect();
            vec![Command::SetNotes { pattern: pat.id.0, channel: None, notes: list }]
        });
        if let Err(e) = r {
            return Some(e);
        }
        Some(format!("Captured {n} note{} into the pattern", if n == 1 { "" } else { "s" }))
    }

    /// Hands the looping time selection to the playback (`Transport::set_loop`); returns it.
    pub fn sync_loop_range(&self) -> Option<(u32, u32)> {
        let sel = self.selection_loop();
        self.state.session.transport.set_loop(sel);
        sel
    }

    /// The time selection that loops playback (spec §5): the piano roll's in PAT mode while it is shown, the
    /// playlist's in SONG mode; one with no length does not count.
    pub fn selection_loop(&self) -> Option<(u32, u32)> {
        let s = &self.state;
        let r = if s.song_mode() { s.playlist.range } else if s.shown(crate::app::Win::PianoRoll) { s.piano.range } else { None };
        r.filter(|r| r.1 > r.0)
    }

    /// Play from stopped (the Play button, Space, Recording starts playback), with the record options (spec §3):
    /// Start on input waits for the first note instead (Play again cancels the wait); Recording precount, armed,
    /// counts in first.
    pub fn play_from_stop(&mut self) {
        let r = &mut self.state.session.rec;
        if r.waiting {
            r.waiting = false;
        } else if Opt::Wait.on(&self.state.session.settings) && (r.armed || !Opt::WaitArmed.on(&self.state.session.settings)) {
            r.waiting = true;
        } else {
            self.play_counted_if_armed();
        }
        self.invalidate();
    }

    /// Play, with Recording precount's count-in when armed.
    pub fn play_counted_if_armed(&mut self) {
        let bars = if self.state.session.rec.armed && Opt::Precount.on(&self.state.session.settings) { precount_bars(&self.state.session.settings) } else { 0 };
        self.start_playback_counted(bars);
        if bars > 0 && self.state.session.transport.playing() {
            let p = &self.state.session.project;
            let tempo = p.tick_tempo(self.state.tempo().max(1.0));
            let beat = f64::from(p.beat_ticks()) / f64::from(p.ppq.max(1)) * 60.0 / tempo;
            let beats = bars * u32::from(p.bar().0.max(1));
            self.state.session.rec.count = Some(CountIn { from: std::time::Instant::now(), beat, beats });
            self.state.session.rec.shown = beats;
        }
    }

    /// Stop ends a wait or a count-in.
    pub fn record_stopped(&mut self) {
        self.state.session.rec.waiting = false;
        self.state.session.rec.count = None;
    }

    /// Start on input: the first input that counts starts playback from the start (spec §3). `what` is what the input
    /// is (notes, automation); with Respect recording filter, the filter must record it.
    fn start_on_input(&mut self, what: Opt) {
        if self.state.session.rec.waiting && (!Opt::WaitFilter.on(&self.state.session.settings) || what.on(&self.state.session.settings)) {
            self.state.session.rec.waiting = false;
            self.start_playback();
            self.invalidate();
        }
    }

    /// The option buttons' right-click menus (spec §3): Start on input's checks, Recording precount's length.
    pub fn record_option_menu(&mut self, o: Opt, x: f32, y: f32) {
        use crate::playlist::{Cmd, Mark, PItem, Popup};
        let items = match o {
            Opt::Wait => vec![
                PItem::new("Respect recording filter", Cmd::Record(Opt::WaitFilter)).mark(Mark::Check(Opt::WaitFilter.on(&self.state.session.settings))),
                PItem::new("Only when recording", Cmd::Record(Opt::WaitArmed)).mark(Mark::Check(Opt::WaitArmed.on(&self.state.session.settings))),
            ],
            Opt::Precount => {
                let bars = precount_bars(&self.state.session.settings);
                vec![
                    PItem::head("Count-in length"),
                    PItem::new("1 bar", Cmd::RecPrecountBars(1)).mark(Mark::Radio(bars == 1)),
                    PItem::new("2 bars", Cmd::RecPrecountBars(2)).mark(Mark::Radio(bars == 2)),
                ]
            }
            _ => return,
        };
        self.state.playlist.popup = Some(Popup { x, y, items, sub: None, scroll: [0; 2] });
    }

    /// An option switch (the buttons, OPTIONS menu, Ctrl+I / Ctrl+P / Ctrl+B); turning Start on input off ends a wait.
    pub fn toggle_record_option(&mut self, o: Opt) {
        let on = o.toggle(&self.state.session.settings);
        if o == Opt::Wait && !on {
            self.state.session.rec.waiting = false;
        }
        let name = match o {
            Opt::Wait => "Start on input",
            Opt::Precount => "Count-in",
            Opt::Loop => "Loop record",
            _ => "Blend recorded notes",
        };
        self.state.message = Some(format!("{name} {}", if on { "on" } else { "off" }));
        self.invalidate();
    }

    /// The right-click menu (spec §2 Right-click menu).
    pub fn record_menu(&mut self, x: f32, y: f32) {
        use crate::playlist::{Cmd, Mark, PItem, Popup};
        let it = |label: &'static str, o: Opt| PItem::new(label, Cmd::Record(o)).mark(Mark::Check(o.on(&self.state.session.settings)));
        let items = vec![
            PItem::head("Recording filter"),
            it("Notes", Opt::Notes),
            it("Automation", Opt::Automation),
            // Audio: the armed mixer tracks record, into the playlist (spec: audio/track-io.md §2.5).
            it("Audio", Opt::Audio),
            PItem::head("Options"),
            it("Ask what to record", Opt::Ask),
            it("Recording starts playback", Opt::StartsPlayback),
            it("Disarm on stop", Opt::DisarmOnStop),
            PItem::head("Quantize"),
            it("Note start time", Opt::QuantizeStart),
            it("Note end time", Opt::QuantizeEnd),
        ];
        self.state.playlist.popup = Some(Popup { x, y, items, sub: None, scroll: [0; 2] });
    }

    /// Whether something played now is recorded: armed, `what` on in the filter, playing and not paused.
    /// Nothing is recorded during a count-in (spec §3).
    fn rec_live(&self, what: Opt) -> bool {
        let r = &self.state.session.rec;
        r.armed && r.count.is_none() && what.on(&self.state.session.settings) && self.state.session.transport.playing() && !self.state.session.transport.paused()
    }

    /// A knob moved (`record_control`).
    pub fn record_knob(&mut self, id: KnobId) {
        // A container member's level is recorded on its target, as its controller link records it.
        if let KnobId::Member(t, k, m, wet) = id {
            if let Some(target) = crate::container::level_target(&self.state.session, t, k, m, wet) {
                self.record_param(target, (0.0, 1.0), f64::from(self.state.knob(id)));
            }
            return;
        }
        self.record_control(&crate::app::Action::Knob(id));
    }

    /// A control moved (a channel's volume or pan knob, a mixer track's fader, pan, separation, equaliser or slot mix
    /// knob, the tempo): while recording automation its value goes into the current pattern as a control point at the
    /// playback position, in the setting's own unit (spec §2 Taking automation).
    pub fn record_control(&mut self, a: &crate::app::Action) {
        let Some((kind, value)) = point_of(&self.state, a) else { return };
        self.start_on_input(Opt::Automation);
        if self.rec_live(Opt::Automation) {
            self.record_point(kind, value);
        }
    }

    /// A plugin's parameter `target` moved in its own window to normalized `level`; `range` is its plain range as the
    /// plugin states it (spec §2 Taking automation; the value: `os_model::target::plugin_point_value`).
    pub fn record_param(&mut self, target: u32, range: (f64, f64), level: f64) {
        self.start_on_input(Opt::Automation);
        if self.rec_live(Opt::Automation) {
            self.record_point(target, os_model::target::plugin_point_value(range.0, range.1, level));
        }
    }

    /// Writes a control point on target word `kind` at the playback position, replacing the move's earlier points.
    fn record_point(&mut self, kind: u32, value: i32) {
        self.host(|ses| point_in(ses, kind, value));
    }

    /// A note went down (typing keyboard, MIDI, Touch controller): recorded from the playback position while
    /// recording (spec §2 Taking notes).
    pub fn record_down_with(&mut self, src: Src, key: u16, velocity: u8, extra: Extra) {
        let target = extra.channel.or_else(|| crate::piano::channel(&self.state));
        if let Some(channel) = target {
            self.state.session.rec.log.down(src, key, velocity, channel, extra.at.unwrap_or_else(std::time::Instant::now));
        }
        self.start_on_input(Opt::Notes);
        if !self.rec_live(Opt::Notes) {
            return;
        }
        let Some(channel) = target else { return };
        self.host(|ses| down_in(ses, src, key, velocity, channel, extra));
    }

    /// A note went up: written when it was being recorded.
    /// `at`: when it arrived from its device.
    pub fn record_up_with(&mut self, src: Src, release: Option<u8>, at: Option<std::time::Instant>) {
        self.state.session.rec.log.up(src, at.unwrap_or_else(std::time::Instant::now));
        self.state.session.rec.unit = unit(&self.state);
        self.host(|ses| up_in(ses, src, release, at));
    }

    /// The take ends: the notes still held end where playback last was.
    pub fn end_take(&mut self) {
        self.state.session.rec.unit = unit(&self.state);
        self.host(end_take_in);
    }

    /// Each tick: one-click and track recording, then the session's half (`tick_in`) with the looping time selection;
    /// a punch out stops playback.
    pub fn record_tick(&mut self) {
        self.one_click_tick();
        self.track_rec_tick();
        self.state.session.rec.unit = unit(&self.state);
        let sel = self.sync_loop_range();
        if self.host(|ses| tick_in(ses, sel)) {
            self.act(crate::app::Action::Stop);
        }
    }

    /// TOOLS ▸ Cancel recording (spec: ui/cancel-recording.md §2): while Record is armed
    /// and playing (or counting in, or waiting for input), playback stops and every take recorded since it started is
    /// undone; notes still held are dropped. The undone takes can be redone.
    pub fn cancel_recording(&mut self) -> String {
        let r = &self.state.session.rec;
        if !r.armed || !(self.state.session.transport.playing() || r.count.is_some() || r.waiting) {
            return "Nothing is being recorded".into();
        }
        let since = r.since;
        self.state.session.rec.held.clear();
        self.state.session.rec.take = None;
        self.state.session.rec.waiting = false;
        self.state.session.rec.count = None;
        self.act(crate::app::Action::Stop);
        let mut undone = 0;
        while since.is_some_and(|k| self.state.session.history.done() > k) && self.state.session.history.newest_done() == Some(RECORDED) {
            crate::history_ui::undo(&mut self.state);
            undone += 1;
        }
        self.state.session.rec.since = None;
        self.state.session.dirty = true;
        self.invalidate();
        match undone {
            0 => "Recording canceled".into(),
            1 => "Recording canceled: 1 take undone".into(),
            n => format!("Recording canceled: {n} takes undone"),
        }
    }
}

/// The notes held for the current pattern's take, from their start (quantized as they will be written) to playback
/// tick `now` (spec §2 "Shown while held").
pub fn held_notes(s: &State, now: f64) -> Vec<Note> {
    if s.session.rec.take.as_ref().is_none_or(|t| t.pattern != s.pattern()) {
        return Vec::new();
    }
    let note = |h: &Held| {
        let start = h.at.max(0.0).round() as u32;
        let end = (h.at + (now - h.start)).round().max(0.0) as u32;
        let (a, _) = quantized(s, start, end.max(start));
        Note { length: end.saturating_sub(a).max(1), key: h.key, velocity: h.velocity, ..Note::step(h.channel, a) }
    };
    s.session.rec.held.iter().filter(|h| now >= h.start).map(note).collect()
}

/// The control point a control's present value makes: its target word and the value in the setting's own unit (spec:
/// audio/automation.md "Pattern control points"). The main volume and pitch are not recorded (spec: ui/recording.md
/// "Taking automation").
fn point_of(s: &State, a: &crate::app::Action) -> Option<(u32, i32)> {
    use crate::app::Action;
    use os_model::target;
    let track = |t: u16| s.session.project.mixer.iter().find(|m| m.index == t);
    let r = |v: f64| v.round() as i32;
    Some(match *a {
        Action::Knob(id @ (KnobId::ChanVol(row) | KnobId::ChanPan(row))) => {
            let param = if matches!(id, KnobId::ChanVol(_)) { target::CHANNEL_VOLUME } else { target::CHANNEL_PAN };
            (target::channel(s.session.project.channels.get(row)?.index, param), r(f64::from(s.knob(id)) * 12800.0))
        }
        Action::Fader(t) => (target::mixer(t, target::MIXER_VOLUME), r(track(t)?.volume.unwrap_or(0.8) * 16000.0)),
        Action::Knob(KnobId::MixPan(t)) => (target::mixer(t, target::MIXER_PAN), r(track(t)?.pan.unwrap_or(0.0) * 6400.0)),
        Action::Knob(KnobId::MixSep(t)) => (target::mixer(t, target::MIXER_SEPARATION), r(track(t)?.stereo_separation.unwrap_or(0.0) * 64.0)),
        Action::Knob(KnobId::SlotMix(t, k)) => (target::slot(t, k.min(9), target::SLOT_MIX), r(track(t)?.slot_mix[usize::from(k.min(9))] * 12800.0)),
        Action::Knob(KnobId::Eq(t, b, f)) => {
            let (b, f) = (b.min(2), f.min(2));
            let id = [target::EQ_GAIN, target::EQ_FREQ, target::EQ_WIDTH][usize::from(f)] + b;
            (target::mixer(t, id), track(t)?.eq[usize::from(b)][usize::from(f)])
        }
        Action::Tempo => (target::TEMPO, r(s.tempo() * 1000.0)),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Action;
    use crate::playlist::{Cmd, Popup};
    use os_ui::{Canvas, Rect};
    use crate::keys::KeyCode;

    /// A new project's pattern 1 emptied, the first channel selected, the main snap at Step (24 ticks at 96 PPQ);
    /// Record arms without asking.
    fn app() -> App {
        crate::settings_file::set_flag("rec_ask", false);
        let mut app = App::new(None);
        app.state.session.project = crate::locked::Locked::new(crate::app::test_project());
        app.state.selected_channel = 0;
        app.state.set_pattern(1);
        app.state.session.project.get_mut().patterns.iter_mut().for_each(|p| p.notes.clear());
        app.state.global_snap.now = Snap::Step;
        app
    }

    /// Typing key `code` going down or up at playback tick `t`.
    fn key(app: &mut App, code: KeyCode, down: bool, t: f64, song: bool) {
        app.state.session.transport.pretend_at(t, song);
        app.record_tick();
        assert!(app.typing_key(code, down, false, false));
    }

    fn stop(app: &mut App) {
        app.state.session.transport.stop();
        app.record_tick();
    }

    /// Pattern 1's notes as (position, length, channel).
    fn notes(app: &App) -> Vec<(u32, u32, u16)> {
        let mut v: Vec<_> =
            app.state.session.project.pattern(1).map_or(vec![], |p| p.notes.iter().map(|n| (n.position, n.length, n.channel)).collect());
        v.sort_unstable();
        v
    }

    fn hit_rect(app: &App, a: &Action) -> Rect {
        app.hits.iter().rev().find(|h| &h.action == a).map(|h| h.rect).unwrap_or_else(|| panic!("no hit for {a:?}"))
    }

    /// An edit made to the pattern during a take stays when the take writes its next note.
    #[test]
    fn an_edit_during_a_take_stays() {
        let mut app = app();
        app.toggle_record();
        key(&mut app, KeyCode::KeyZ, true, 30.0, false);
        key(&mut app, KeyCode::KeyZ, false, 100.0, false);
        // Drawn while recording, outside the take's span and on another channel.
        if let Some(p) = app.state.session.project.get_mut().patterns.iter_mut().find(|p| p.number == 1) {
            p.notes.push(Note { length: 24, ..Note::step(0, 600) });
            p.notes.push(Note { length: 24, ..Note::step(1, 48) });
        }
        key(&mut app, KeyCode::KeyX, true, 190.0, false);
        key(&mut app, KeyCode::KeyX, false, 205.0, false);
        stop(&mut app);
        assert_eq!(notes(&app), [(24, 72, 0), (48, 24, 1), (192, 24, 0), (600, 24, 0)]);
    }

    /// New forgets what the session and the interface remembered of the old project: a take and its held note go
    /// unwritten, what performance mode launched, the selected channel and track, the mixer's solo memory and the
    /// windows showing the old project's things.
    #[test]
    fn new_forgets_the_old_project() {
        let mut app = app();
        app.toggle_record();
        key(&mut app, KeyCode::KeyZ, true, 30.0, false);
        assert!(app.state.session.rec.holding());
        app.state.session.perform.origin = (2, 3);
        app.state.session.perform.held = Some(1);
        app.state.selected_channel = 3;
        app.state.selected_mixer = 7;
        app.state.mixer.solo = Some((vec![7], vec![(1, true)]));
        app.state.raise(crate::app::Win::Events);
        assert!(app.state.z.contains(&crate::app::Win::Events));
        app.empty_project();
        app.record_tick();
        assert!(!app.state.session.rec.recording() && app.state.session.rec.armed);
        assert!(app.state.session.project.patterns.iter().all(|p| p.notes.is_empty()), "the held note was not written into the new project");
        assert_eq!(app.state.session.perform, crate::perform::Perform::default());
        assert_eq!((app.state.selected_channel, app.state.selected_mixer), (0, 1));
        assert!(app.state.mixer.solo.is_none());
        assert!(!app.state.z.contains(&crate::app::Win::Events));
    }

    /// MIDI channel through (spec: ui/channel-rack.md §18): a note recorded from a MIDI device takes the MIDI channel it
    /// came on as its colour group; with the entry unticked it keeps the first.
    #[test]
    fn midi_notes_record_their_channel_as_colour() {
        use crate::midi_in::Msg;
        let mut app = app();
        app.toggle_record();
        let groups = |app: &App| -> Vec<u8> { app.state.session.project.pattern(1).map_or(vec![], |p| p.notes.iter().map(|n| n.group & 0x0F).collect()) };
        app.state.session.transport.pretend_at(0.0, false);
        app.record_tick();
        app.midi_notes(vec![Msg::On { ch: 4, key: 60, vel: 100 }]);
        app.state.session.transport.pretend_at(24.0, false);
        app.record_tick();
        app.midi_notes(vec![Msg::Off { ch: 4, key: 60, vel: 64 }]);
        stop(&mut app);
        assert_eq!(groups(&app), [4]);
        crate::rack::run(&mut app, crate::rack::RCmd::Through(0));
        assert!(!crate::rack::through(&app.state, 0));
        app.state.session.transport.pretend_at(48.0, false);
        app.record_tick();
        app.midi_notes(vec![Msg::On { ch: 4, key: 62, vel: 100 }]);
        app.state.session.transport.pretend_at(72.0, false);
        app.record_tick();
        app.midi_notes(vec![Msg::Off { ch: 4, key: 62, vel: 64 }]);
        stop(&mut app);
        assert_eq!(groups(&app), [4, 0], "unticked: the first colour");
    }

    /// SONG mode: a take starting less than a bar past a whole-pattern clip's bar line takes that clip and makes it
    /// longer; further on, or with the track taken after the clip, a clip of its own is placed (spec §2 "right after a
    /// clip", issue 192).
    #[test]
    fn takes_the_clip_just_before_in_song_mode() {
        let clips =
            |app: &App| -> Vec<(u16, u32, u32)> { crate::playlist::clips(&app.state.session).iter().map(|c| (c.track, c.position, c.length)).collect() };
        let take = |other: bool, at: f64| {
            let mut app = app();
            crate::playlist::arr_mut(&mut app.state.session).clips.clear();
            crate::playlist::place(&mut app.state, 0, 0);
            let bar = app.state.session.project.bar_ticks();
            if other {
                let c = os_model::Clip::new(bar + bar / 2, bar, 0, ClipSource::Pattern(2));
                crate::playlist::arr_mut(&mut app.state.session).clips.push(c);
            }
            app.toggle_record();
            key(&mut app, KeyCode::KeyW, true, at, true);
            key(&mut app, KeyCode::KeyW, false, at + 24.0, true);
            stop(&mut app);
            (app, bar)
        };
        let (app, bar) = take(false, 400.0);
        assert_eq!(clips(&app), [(0, 0, 2 * bar)]);
        assert_eq!(notes(&app), [(408, 24, 0)]);
        let (app, bar) = take(false, f64::from(2 * bar + 16));
        assert_eq!(clips(&app), [(0, 0, bar), (0, 2 * bar, bar)]);
        let (app, bar) = take(true, 400.0);
        assert_eq!(clips(&app), [(0, 0, bar), (0, bar + bar / 2, bar), (1, bar, bar)]);
    }

    /// SONG mode: a note held past the take's clip makes it longer while playback runs, so the written note ends inside
    /// it; the piano roll's held note grows to the playhead (spec §2, issue 192).
    #[test]
    fn a_held_note_grows_the_clip_and_shows() {
        let mut app = app();
        crate::playlist::arr_mut(&mut app.state.session).clips.clear();
        crate::playlist::place(&mut app.state, 0, 0);
        let bar = app.state.session.project.bar_ticks();
        app.toggle_record();
        key(&mut app, KeyCode::KeyW, true, 50.0, true);
        assert!(app.state.session.rec.holding());
        assert_eq!(held_notes(&app.state, 50.0).iter().map(|n| (n.position, n.length)).collect::<Vec<_>>(), [(48, 2)]);
        let held = |at: f64| held_notes(&app.state, at).iter().map(|n| (n.position, n.length)).collect::<Vec<_>>();
        assert_eq!(held(500.0), [(48, 452)]);
        app.state.session.transport.pretend_at(f64::from(2 * bar + 100), true);
        app.record_tick();
        assert_eq!(crate::playlist::clips(&app.state.session)[0].length, 3 * bar);
        key(&mut app, KeyCode::KeyW, false, f64::from(2 * bar + 200), true);
        assert!(!app.state.session.rec.holding() && held_notes(&app.state, 1000.0).is_empty());
        stop(&mut app);
        let n = notes(&app);
        assert_eq!(n.len(), 1);
        assert!(n[0].0 + n[0].1 <= 3 * bar, "{n:?}");
        assert_eq!(crate::playlist::clips(&app.state.session).len(), 1);
    }

    /// R arms like a click when the typing keyboard is off; with it on, R is a note.
    #[test]
    fn r_arms_when_the_typing_keyboard_is_off() {
        use crate::keys::Key;
        let mut app = app();
        app.state.toggles.retain(|t| *t != "typing");
        app.key(&Key::Character("r".into()), false, false);
        assert!(app.state.session.rec.armed);
        app.key(&Key::Character("r".into()), false, false);
        assert!(!app.state.session.rec.armed);
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
        app.state.session.transport.pretend_at(170.0, false);
        app.record_tick();
        assert_eq!(app.state.piano.range, Some((96, 192)), "recording keeps the selection");
        app.state.session.transport.pretend_at(200.0, false);
        app.record_tick();
        assert!(!app.state.session.transport.playing(), "punched out");
        assert_eq!(notes(&app), [(120, 72, 0)], "the held note ends at the selection's end");
        assert_eq!(app.state.session.pat_pos, 96);
        app.state.piano.range = None;
        assert_eq!(app.selection_loop(), None);
    }

    /// Pattern 1's control points as (position, channel, param, value).
    fn controls(app: &App) -> Vec<(u32, u16, u16, i32)> {
        app.state.session.project.pattern(1).map_or(vec![], |p| p.controls.iter().map(|c| (c.position, c.channel(), c.param(), c.value)).collect())
    }

    /// A channel volume knob dragged while recording becomes the pattern's control points at the playback position;
    /// the sweep replaces the earlier points between its points; undo puts them back; pan records too; with
    /// Automation off nothing is written (spec §2 Taking automation).
    #[test]
    fn records_channel_knobs() {
        use os_model::ControlPoint;
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = app();
        let ch = app.state.session.project.channels[1].index;
        crate::piano::set_control(&mut app.state.session, ch, ControlPoint::VOLUME, 70, 1000);
        crate::piano::set_control(&mut app.state.session, ch, ControlPoint::VOLUME, 200, 2000);
        app.paint(&mut c, None);
        let k = hit_rect(&app, &Action::Knob(KnobId::ChanVol(1)));
        app.toggle_record();
        app.state.session.transport.pretend_at(48.0, false);
        app.record_tick();
        app.mouse_move(k.cx(), k.cy());
        app.mouse_down();
        app.mouse_move(k.cx(), k.cy() + 10.0);
        let mid = (app.state.knob(KnobId::ChanVol(1)) * 12800.0).round() as i32;
        app.state.session.transport.pretend_at(96.0, false);
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
        crate::history_ui::undo(&mut app.state);
        assert_eq!(vol(&app), [(70, 1000), (200, 2000)]);

        // Automation off: knob moves write nothing.
        Opt::Automation.toggle(&crate::settings_file::store());
        app.state.session.transport.pretend_at(10.0, false);
        app.record_tick();
        app.state.set_knob(KnobId::ChanVol(1), 0.1);
        app.record_knob(KnobId::ChanVol(1));
        assert_eq!(vol(&app), [(70, 1000), (200, 2000)]);
        assert_eq!(crate::record::hint(&crate::settings_file::store()), "Record (notes) (R) - right-click for options");
    }

    #[test]
    fn records_mixer_controls_the_tempo_and_plugin_parameters() {
        use os_model::target;
        let mut app = app();
        if crate::mixer::track(&app.state.session, 1).is_none() {
            app.state.session.project.get_mut().mixer = (0..3).map(os_model::MixerTrack::new).collect();
        }
        app.toggle_record();
        app.state.session.transport.pretend_at(24.0, false);
        app.record_tick();
        let m = crate::mixer::track_mut(&mut app.state.session, 1).unwrap();
        (m.volume, m.pan) = (Some(0.5), Some(-0.25));
        app.record_control(&Action::Fader(1));
        app.record_control(&Action::Knob(KnobId::MixPan(1)));
        app.state.session.transport.pretend_at(48.0, false);
        app.state.set_tempo(150.0);
        app.record_control(&Action::Tempo);
        app.record_param(target::slot_param(1, 2, 0), (-64.0, 64.0), 0.75);
        app.record_param(target::slot_param(1, 2, 1), (0.0, 1.0), 0.25);
        app.record_knob(KnobId::Swing);
        let points = |app: &App| {
            let p = app.state.session.project.pattern(app.state.pattern()).map(|p| p.controls.clone()).unwrap_or_default();
            p.into_iter().map(|c| (c.position, c.kind, c.value)).collect::<Vec<_>>()
        };
        assert_eq!(
            points(&app),
            [
                (24, target::mixer(1, target::MIXER_VOLUME), 8000),
                (24, target::mixer(1, target::MIXER_PAN), -1600),
                (48, target::TEMPO, 150_000),
                (48, target::slot_param(1, 2, 0), 32),
                (48, target::slot_param(1, 2, 1), 16384),
            ]
        );
        stop(&mut app);
        crate::history_ui::undo(&mut app.state);
        assert_eq!(points(&app), []);
    }

    /// The option buttons switch their settings and say so in their hints; Start on input and Recording precount have
    /// right-click menus; Ctrl+I / Ctrl+P / Ctrl+B and the OPTIONS entries switch the same settings (spec §3).
    #[test]
    fn option_buttons_menus_and_keys() {
        use crate::keys::Key;
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
        assert!(Opt::Blend.on(&crate::settings_file::store()));
        let p = hit_rect(&app, &Action::RecOpt(Opt::Precount));
        app.mouse_move(p.cx(), p.cy());
        app.mouse_down_right();
        app.mouse_up();
        let Some(Popup { items, .. }) = app.state.playlist.popup.take() else { panic!("no menu") };
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_ref()).collect();
        assert_eq!(labels, ["Count-in length", "1 bar", "2 bars"]);
        crate::playlist::run(&mut app, Cmd::RecPrecountBars(2));
        assert_eq!(precount_bars(&crate::settings_file::store()), 2);
        let w = hit_rect(&app, &Action::RecOpt(Opt::Wait));
        app.mouse_move(w.cx(), w.cy());
        app.mouse_down_right();
        app.mouse_up();
        let Some(Popup { items, .. }) = app.state.playlist.popup.take() else { panic!("no menu") };
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_ref()).collect();
        assert_eq!(labels, ["Respect recording filter", "Only when recording"]);
        let ctrl = |app: &mut App, ch: &str| app.key(&Key::Character(String::from(ch)), true, false);
        // A focused playlist keeps Ctrl+P (Performance mode) and Ctrl+B (duplicate); elsewhere they switch the options.
        assert!(app.pl_focus());
        ctrl(&mut app, "i");
        ctrl(&mut app, "p");
        ctrl(&mut app, "b");
        assert_eq!((Opt::Wait.on(&crate::settings_file::store()), Opt::Precount.on(&crate::settings_file::store()), Opt::Blend.on(&crate::settings_file::store())), (true, false, true));
        app.state.z.retain(|&w| w != crate::app::Win::Playlist && w != crate::app::Win::PianoRoll);
        ctrl(&mut app, "p");
        ctrl(&mut app, "b");
        assert_eq!((Opt::Wait.on(&crate::settings_file::store()), Opt::Precount.on(&crate::settings_file::store()), Opt::Blend.on(&crate::settings_file::store())), (true, true, false));
    }

    /// A session with no interface records a take: notes held from a source, written into the current pattern on
    /// the grid it was given, one undo step; the stop ends the take and the session says the pattern changed.
    #[test]
    fn a_session_records_without_the_interface() {
        crate::settings_file::set_flag("rec_ask", false);
        let mut ses = crate::session::Session::new(crate::projects::test_project(), crate::transport::Transport::new(None), crate::settings_file::store());
        ses.set_pattern(1);
        ses.project.get_mut().patterns.iter_mut().for_each(|p| p.notes.clear());
        ses.rec.armed = true;
        ses.rec.unit = Some(24);
        let done = ses.history.done();
        ses.transport.pretend_at(30.0, false);
        assert!(!tick_in(&mut ses, None));
        down_in(&mut ses, Src::Midi(60), 60, 100, 0, Extra::default());
        ses.transport.pretend_at(100.0, false);
        assert!(!tick_in(&mut ses, None));
        up_in(&mut ses, Src::Midi(60), None, None);
        ses.transport.stop();
        tick_in(&mut ses, None);
        let notes: Vec<(u32, u32, u16)> = ses.project.pattern(1).map(|p| p.notes.iter().map(|n| (n.position, n.length, n.key)).collect()).unwrap_or_default();
        assert_eq!(notes, [(24, 72, 60)]);
        assert_eq!(ses.history.done(), done + 1);
        assert_eq!(ses.history.newest_done(), Some(RECORDED));
        assert!(ses.touched.views.contains(&crate::session::View::Recorded { pattern: 1, channel: 0 }));
        assert!(!ses.rec.recording());
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
        assert!(!app.state.session.rec.armed, "it asks first");
        let Some(Popup { items, x, .. }) = app.state.playlist.popup.clone() else { panic!("no question") };
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_ref()).collect();
        assert_eq!(
            labels,
            [
                "What would you like to record?",
                "Notes and automation",
                "Notes only",
                "Automation only",
                "Audio, into the arranger as an audio clip",
                "Audio, into the audio editor",
                "",
                "Don't ask again"
            ]
        );
        assert_eq!(x, b.x, "under the button");
        assert!(items[5].enabled && items[5].cmd == Cmd::OneClickEditor);
        app.state.playlist.popup = None;
        crate::playlist::run(&mut app, Cmd::Rec(RecCmd::Arm { notes: true, automation: false }));
        assert!(app.state.session.rec.armed && Opt::Notes.on(&crate::settings_file::store()) && !Opt::Automation.on(&crate::settings_file::store()));
        app.toggle_record();
        assert!(!app.state.session.rec.armed && app.state.playlist.popup.is_none(), "disarming never asks");
        crate::playlist::run(&mut app, Cmd::Rec(RecCmd::AskOff));
        assert!(!Opt::Ask.on(&crate::settings_file::store()));
        crate::playlist::run(&mut app, Cmd::Record(Opt::Automation));
        app.mouse_down();
        app.mouse_up();
        assert!(app.state.session.rec.armed);
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
        assert!(Opt::Notes.on(&crate::settings_file::store()) && Opt::Automation.on(&crate::settings_file::store()) && Opt::QuantizeStart.on(&crate::settings_file::store()) && Opt::QuantizeEnd.on(&crate::settings_file::store()) && !Opt::DisarmOnStop.on(&crate::settings_file::store()));
        crate::playlist::run(&mut app, Cmd::Record(Opt::DisarmOnStop));
        assert!(Opt::DisarmOnStop.on(&crate::settings_file::store()));
        app.state.playlist.popup = None;
        app.mouse_down();
        app.mouse_up();
        assert!(!app.state.session.rec.armed, "a click disarms");
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
        assert!(app.state.session.rec.recording());
        key(&mut app, KeyCode::KeyZ, false, 100.0, false);
        key(&mut app, KeyCode::KeyX, true, 190.0, false);
        key(&mut app, KeyCode::KeyX, false, 205.0, false);
        stop(&mut app);
        assert_eq!(notes(&app), [(24, 72, 0), (192, 24, 0)]);
        assert!(!app.state.session.rec.recording() && app.state.session.rec.armed, "a stop ends the take and keeps Record armed");
        assert_eq!(app.state.session.history.entries(&crate::settings_file::store()).last().map(|e| e.1), Some(RECORDED));

        // Another take from 170 to 260: the note starting at 192 goes, the one at 24 (before the take) stays.
        let steps = app.state.session.history.len();
        key(&mut app, KeyCode::KeyC, true, 170.0, false);
        key(&mut app, KeyCode::KeyC, false, 260.0, false);
        stop(&mut app);
        assert_eq!(notes(&app), [(24, 72, 0), (168, 96, 0)]);
        assert_eq!(app.state.session.history.len(), steps + 1);
        crate::history_ui::undo(&mut app.state);
        assert_eq!(notes(&app), [(24, 72, 0), (192, 24, 0)]);

        // A note held when playback stops ends where playback last was; Line leaves times as played.
        app.state.global_snap.now = Snap::Line;
        key(&mut app, KeyCode::KeyV, true, 301.0, false);
        app.state.session.transport.pretend_at(333.0, false);
        app.record_tick();
        stop(&mut app);
        assert!(notes(&app).contains(&(301, 32, 0)), "{:?}", notes(&app));
    }

    /// SONG mode: with no clip of the pattern at the song position, one is placed from the start of that bar on the
    /// first free track, and the note lands in it (spec §2 Taking notes).
    #[test]
    fn places_a_clip_in_song_mode() {
        let mut app = app();
        crate::playlist::arr_mut(&mut app.state.session).clips.clear();
        app.toggle_record();
        let bar = f64::from(app.state.session.project.bar_ticks());
        key(&mut app, KeyCode::KeyZ, true, bar + 48.0, true);
        key(&mut app, KeyCode::KeyZ, false, bar + 96.0, true);
        stop(&mut app);
        let clips = crate::playlist::clips(&app.state.session);
        assert_eq!(clips.len(), 1);
        assert_eq!((clips[0].position, clips[0].track, clips[0].source), (bar as u32, 0, ClipSource::Pattern(1)));
        assert_eq!(notes(&app), [(48, 48, 0)]);
        // Undo takes the clip away with the notes.
        crate::history_ui::undo(&mut app.state);
        assert!(crate::playlist::clips(&app.state.session).is_empty() && notes(&app).is_empty());
    }

    /// SONG mode: one clip per take; when playback runs past its end the clip is made longer to the bar's end instead
    /// of a second clip being placed (spec §2 Taking notes).
    /// SONG mode: the take's clip, placed on the first track free for the pattern's length, grew over a clip further
    /// along that track. It moves to the first track free over its whole length.
    #[test]
    fn a_growing_take_clip_leaves_a_taken_track() {
        let mut app = app();
        crate::playlist::arr_mut(&mut app.state.session).clips.clear();
        let bar = app.state.session.project.bar_ticks();
        let b = f64::from(bar);
        // Another pattern's clip on track 1 from bar 2.
        let other = app.state.pattern() + 1;
        crate::playlist::arr_mut(&mut app.state.session).clips.push(os_model::Clip::new(bar, bar, 0, ClipSource::Pattern(other)));
        app.toggle_record();
        key(&mut app, KeyCode::KeyZ, true, 48.0, true);
        key(&mut app, KeyCode::KeyZ, false, 96.0, true);
        key(&mut app, KeyCode::KeyX, true, b + 48.0, true);
        key(&mut app, KeyCode::KeyX, false, b + 72.0, true);
        stop(&mut app);
        let clips = crate::playlist::clips(&app.state.session);
        let take = clips.iter().find(|c| c.source == ClipSource::Pattern(app.state.pattern())).expect("the take's clip");
        assert_eq!((take.position, take.length), (0, 2 * bar));
        assert!(!clips.iter().any(|o| o.track == take.track && o.position < take.end() && o.end() > take.position && o.source != take.source), "{clips:?}");
        assert_eq!(notes(&app), [(48, 48, 0), (bar + 48, 24, 0)]);
    }

    #[test]
    fn extends_the_take_clip_in_song_mode() {
        let mut app = app();
        crate::playlist::arr_mut(&mut app.state.session).clips.clear();
        app.toggle_record();
        let bar = app.state.session.project.bar_ticks();
        let b = f64::from(bar);
        key(&mut app, KeyCode::KeyZ, true, 48.0, true);
        key(&mut app, KeyCode::KeyZ, false, 96.0, true);
        key(&mut app, KeyCode::KeyX, true, b + 48.0, true);
        key(&mut app, KeyCode::KeyX, false, b + 72.0, true);
        stop(&mut app);
        let clips = crate::playlist::clips(&app.state.session);
        assert_eq!(clips.len(), 1, "{clips:?}");
        assert_eq!((clips[0].position, clips[0].length), (0, 2 * bar));
        assert_eq!(notes(&app), [(48, 48, 0), (bar + 48, 24, 0)]);
    }

    /// The metronome menu's Recording precount: Off, 1 bar, 2 bars set the precount switch and length (spec §3).
    #[test]
    fn metronome_menu_sets_the_precount() {
        let mut app = app();
        app.metronome_menu(10.0, 10.0);
        let Some(Popup { items, .. }) = app.state.playlist.popup.clone() else { panic!("no menu") };
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_ref()).collect();
        assert_eq!(labels[labels.len() - 4..], ["Count-in", "Off", "1 bar", "2 bars"]);
        crate::playlist::run(&mut app, Cmd::Rec(RecCmd::Precount(2)));
        assert!(Opt::Precount.on(&crate::settings_file::store()) && precount_bars(&crate::settings_file::store()) == 2);
        crate::playlist::run(&mut app, Cmd::Rec(RecCmd::Precount(0)));
        assert!(!Opt::Precount.on(&crate::settings_file::store()));
    }

    /// SONG mode, a Stop recording marker (audio/transport.md "Time markers in SONG mode"): the take ends on it, a note
    /// held over it ends there, later notes are not recorded; a Start recording marker before playback reaches it
    /// changes nothing, and one passed after the Stop records again. Stopping clears it.
    #[test]
    fn stop_recording_marker_punches_out() {
        let mut app = app();
        crate::playlist::arr_mut(&mut app.state.session).clips.clear();
        crate::playlist::arr_mut(&mut app.state.session).markers = vec![
            TimeMarker { kind: TimeMarker::PUNCH_IN, ..TimeMarker::new(768, "In") },
            TimeMarker { kind: TimeMarker::PUNCH_OUT, ..TimeMarker::new(1536, "Out") },
            TimeMarker { kind: TimeMarker::PUNCH_IN, ..TimeMarker::new(2304, "In") },
        ];
        app.toggle_record();
        // From the song's start: recorded before the Start recording marker too.
        key(&mut app, KeyCode::KeyZ, true, 24.0, true);
        key(&mut app, KeyCode::KeyZ, false, 72.0, true);
        // Held over the Stop recording marker: it ends there.
        key(&mut app, KeyCode::KeyX, true, 1488.0, true);
        app.state.session.transport.pretend_at(1560.0, true);
        app.record_tick();
        assert!(!app.state.session.rec.recording(), "the take ended on the marker");
        key(&mut app, KeyCode::KeyX, false, 1600.0, true);
        // Past it: not recorded.
        key(&mut app, KeyCode::KeyC, true, 1700.0, true);
        key(&mut app, KeyCode::KeyC, false, 1750.0, true);
        assert_eq!(notes(&app).iter().map(|n| (n.0, n.1)).collect::<Vec<_>>(), [(24, 48), (1488, 48)]);
        // Past the next Start recording marker: recorded again.
        key(&mut app, KeyCode::KeyV, true, 2400.0, true);
        key(&mut app, KeyCode::KeyV, false, 2448.0, true);
        assert_eq!(notes(&app).len(), 3);
        stop(&mut app);
        assert!(!app.state.session.rec.punched_out);
    }

    /// Dump score log: notes played (playing or not) go into the pattern at their real time offsets from the first,
    /// unquantized; a full pattern asks Merge / Replace; one undo step (spec §6).
    #[test]
    fn dumps_the_score_log() {
        use std::time::{Duration, Instant};
        let mut app = app();
        let now = Instant::now();
        let t0 = now.checked_sub(Duration::from_secs(10)).expect("a clock");
        let src = crate::step_edit_ui::typing;
        let log = &mut app.state.session.rec.log;
        log.down(src(KeyCode::KeyZ), 60, 100, 0, t0);
        log.up(src(KeyCode::KeyZ), t0 + Duration::from_millis(300));
        log.down(src(KeyCode::KeyX), 62, 90, 0, t0 + Duration::from_millis(700));
        log.up(src(KeyCode::KeyX), t0 + Duration::from_millis(1000));
        let tps = crate::score_log_ui::ticks_per_second(&app.state);
        let msg = crate::playlist::run(&mut app, Cmd::Rec(RecCmd::Dump(2)));
        assert_eq!(msg.as_deref(), Some("Captured 2 notes into the pattern"));
        let t = |s: f64| (s * tps).round() as u32;
        assert_eq!(notes(&app), [(0, t(0.3), 0), (t(0.7), t(0.3), 0)]);
        assert_eq!(app.state.session.history.entries(&crate::settings_file::store()).last().map(|e| e.1), Some(crate::score_log::DUMPED));
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

    /// A mixer track's fader and pan, the tempo and a plugin's parameter record into the current pattern as control
    /// points on their targets, each in its setting's own unit (the plugin's parameter: its plain value in a whole
    /// range), in the take's one undo step; a dialog's knob records nothing (spec §2 Taking automation).
    /// What is timed in beats follows the tempo playing, not the tempo the project was last opened or saved with.
    #[test]
    fn timing_follows_the_tempo_playing() {
        let mut app = app();
        app.state.session.project.get_mut().tempo = Some(140.0);
        app.state.set_tempo(70.0);
        let p = &app.state.session.project;
        let at_70 = f64::from(p.ppq) * p.tick_tempo(70.0) / 60.0;
        assert!((crate::score_log_ui::ticks_per_second(&app.state) - at_70).abs() < 1e-9);
    }

    /// "Disarm on stop" disarms at the stop; step edit never takes a recorded note.
    #[test]
    fn disarm_on_stop_and_step_edit() {
        let mut app = app();
        Opt::DisarmOnStop.toggle(&crate::settings_file::store());
        crate::step_edit::toggle(&crate::settings_file::store());
        app.toggle_record();
        key(&mut app, KeyCode::KeyZ, true, 0.0, false);
        key(&mut app, KeyCode::KeyZ, false, 24.0, false);
        stop(&mut app);
        assert!(!app.state.session.rec.armed);
        assert_eq!(notes(&app), [(0, 24, 0)], "only the recorded note: step edit wrote nothing while playing");
    }

    /// Start on input: Play waits; the first note starts playback from the start and is recorded there; Play again
    /// cancels a wait; with Respect recording filter and Notes off, a note does not start it (spec §3).
    #[test]
    fn start_on_input() {
        let mut app = app();
        Opt::Wait.toggle(&crate::settings_file::store());
        app.toggle_record();
        app.act(Action::Play);
        assert!(app.state.session.rec.waiting && !app.state.session.transport.playing());
        app.act(Action::Play);
        assert!(!app.state.session.rec.waiting, "Play again cancels");
        app.act(Action::Play);
        Opt::Notes.toggle(&crate::settings_file::store());
        assert!(app.typing_key(KeyCode::KeyZ, true, false, false));
        assert!(app.typing_key(KeyCode::KeyZ, false, false, false));
        assert!(app.state.session.rec.waiting, "Notes are off in the filter");
        Opt::Notes.toggle(&crate::settings_file::store());
        assert!(app.typing_key(KeyCode::KeyZ, true, false, false));
        assert!(!app.state.session.rec.waiting && app.state.session.transport.playing() && app.state.session.transport.tick() == 0.0);
        assert!(app.state.session.rec.recording(), "the note that started playback is recorded");
        key(&mut app, KeyCode::KeyZ, false, 48.0, false);
        stop(&mut app);
        assert_eq!(notes(&app), [(0, 48, 0)]);
        // Only when recording: disarmed, Play plays at once.
        Opt::WaitArmed.toggle(&crate::settings_file::store());
        app.toggle_record();
        app.act(Action::Play);
        assert!(!app.state.session.rec.waiting && app.state.session.transport.playing());
        app.act(Action::Stop);
    }

    /// Recording precount, armed: Play counts in (the clock's beats left) and nothing is recorded until it is over;
    /// disarmed, Play plays at once (spec §3).
    #[test]
    fn precount_counts_in_when_armed() {
        let mut app = app();
        Opt::Precount.toggle(&crate::settings_file::store());
        app.act(Action::Play);
        assert!(app.state.session.rec.count.is_none(), "disarmed: no count-in");
        app.act(Action::Stop);
        app.toggle_record();
        app.act(Action::Play);
        let beats = u32::from(app.state.session.project.bar().0);
        assert_eq!(app.state.session.rec.count.map(|c| c.left()), Some(beats));
        assert!(app.typing_key(KeyCode::KeyZ, true, false, false));
        assert!(app.typing_key(KeyCode::KeyZ, false, false, false));
        assert!(!app.state.session.rec.recording(), "not during the count-in");
        app.act(Action::Stop);
        assert!(app.state.session.rec.count.is_none(), "Stop ends it");
    }

    /// Loop record: the flag the engines read is set while armed with Loop record off; each pass of a loop is a take
    /// of its own, which replaces what the pass before recorded there, and a note held over the loop ends at the
    /// pattern's end (spec §4).
    #[test]
    fn loop_record_passes() {
        use std::sync::atomic::Ordering;
        let mut app = app();
        app.record_tick();
        assert!(!crate::transport::run_on_flag().load(Ordering::Relaxed), "disarmed");
        app.toggle_record();
        app.record_tick();
        assert!(crate::transport::run_on_flag().load(Ordering::Relaxed), "armed, Loop record off: runs on");
        app.toggle_record_option(Opt::Loop);
        app.record_tick();
        assert!(!crate::transport::run_on_flag().load(Ordering::Relaxed), "Loop record on: loops");
        let len = f64::from(app.state.session.project.pattern_length(1).max(1));
        let steps = app.state.session.history.len();
        key(&mut app, KeyCode::KeyZ, true, 48.0, false);
        key(&mut app, KeyCode::KeyZ, false, 96.0, false);
        key(&mut app, KeyCode::KeyX, true, len - 24.0, false);
        // The loop: a new pass, a new take.
        key(&mut app, KeyCode::KeyC, true, 48.0, false);
        key(&mut app, KeyCode::KeyC, false, 72.0, false);
        stop(&mut app);
        let l = len as u32;
        assert_eq!(notes(&app), [(48, 24, 0), (l - 24, 24, 0)], "the second pass replaced the first's note at 48");
        assert_eq!(app.state.session.history.len(), steps + 2, "one undo step per pass");
        crate::history_ui::undo(&mut app.state);
        assert_eq!(notes(&app), [(48, 48, 0), (l - 24, 24, 0)]);
    }

    /// TOOLS ▸ Cancel recording: the takes of the playback that is recording are undone and playback stops; a take
    /// of an earlier playback stays; not recording, nothing happens.
    #[test]
    fn cancel_recording_undoes_this_playbacks_takes() {
        let mut app = app();
        app.toggle_record();
        // An earlier playback's take stays.
        key(&mut app, KeyCode::KeyZ, true, 30.0, false);
        key(&mut app, KeyCode::KeyZ, false, 100.0, false);
        stop(&mut app);
        assert_eq!(notes(&app), [(24, 72, 0)]);
        assert_eq!(app.cancel_recording(), "Nothing is being recorded");
        assert_eq!(notes(&app), [(24, 72, 0)]);
        // This playback: two takes (the pattern loops back between them), and a note still held.
        app.state.global_snap.now = Snap::Line;
        key(&mut app, KeyCode::KeyX, true, 190.0, false);
        key(&mut app, KeyCode::KeyX, false, 210.0, false);
        key(&mut app, KeyCode::KeyC, true, 20.0, false);
        key(&mut app, KeyCode::KeyC, false, 40.0, false);
        key(&mut app, KeyCode::KeyV, true, 300.0, false);
        assert!(notes(&app).len() > 1, "{:?}", notes(&app));
        assert_eq!(app.cancel_recording(), "Recording canceled: 2 takes undone");
        app.record_tick();
        assert_eq!(notes(&app), [(24, 72, 0)]);
        assert!(!app.state.session.transport.playing() && !app.state.session.rec.recording() && app.state.session.rec.armed);
        assert_eq!(app.state.session.history.newest_done(), Some(RECORDED), "the earlier take is still in the history");
    }

    /// Blend recorded notes: a second take keeps the notes it overlaps (spec §3).
    #[test]
    fn blend_keeps_earlier_notes() {
        let mut app = app();
        app.toggle_record();
        key(&mut app, KeyCode::KeyZ, true, 0.0, false);
        key(&mut app, KeyCode::KeyZ, false, 96.0, false);
        stop(&mut app);
        Opt::Blend.toggle(&crate::settings_file::store());
        key(&mut app, KeyCode::KeyX, true, 0.0, false);
        key(&mut app, KeyCode::KeyX, false, 48.0, false);
        stop(&mut app);
        assert_eq!(notes(&app), [(0, 48, 0), (0, 96, 0)]);
    }
}
