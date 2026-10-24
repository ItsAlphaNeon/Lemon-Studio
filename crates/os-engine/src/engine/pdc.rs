// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Plugin delay compensation: how late each place in the mixer is, from the delays the plugins report and the tracks'
//! latency settings; the pads that delay the earlier paths to match; the head of the stream an export (and live
//! playback) skips so the file shows no delay; and the automation values held back for late audio.
//!
// spec: audio/delay-compensation.md, audio/timing.md ("Where the file ends")

use std::collections::VecDeque;

use os_model::target::Target;

use super::{lock_busy, Engine, Fx};

/// A plain delay line of whole stereo frames, empty at the start; one of length 0 is none (spec: "Pads").
#[derive(Debug, Default, Clone)]
pub(super) struct Pad {
    line: Vec<[f32; 2]>,
    at: usize,
}

impl Pad {
    pub fn len(&self) -> usize {
        self.line.len()
    }

    /// A pad whose length changes starts empty; one that keeps its length keeps what it holds.
    pub fn set_len(&mut self, n: usize) {
        if n != self.line.len() {
            self.line = vec![[0.0; 2]; n];
            self.at = 0;
        }
    }

    pub fn clear(&mut self) {
        self.line.fill([0.0; 2]);
        self.at = 0;
    }

    /// Delays interleaved stereo `buf` in place by the pad's length.
    pub fn run(&mut self, buf: &mut [f32]) {
        let n = self.line.len();
        if n == 0 {
            return;
        }
        for f in buf.chunks_exact_mut(2) {
            let d = &mut self.line[self.at];
            let out = *d;
            *d = [f[0], f[1]];
            f[0] = out[0];
            f[1] = out[1];
            self.at += 1;
            if self.at == n {
                self.at = 0;
            }
        }
    }
}

/// `smp(x)`: `x` milliseconds in samples, `Round(x * rate * double(single(0.001)))` (spec: "Milliseconds and
/// samples").
pub(super) fn smp(x: f64, rate: u32) -> i64 {
    (x * f64::from(rate) * f64::from(0.001f32)).round_ties_even() as i64
}

/// `ms(n)`: `n` samples in milliseconds.
pub(super) fn ms(n: i64, rate: u32) -> f64 {
    n as f64 * 1000.0 / f64::from(rate)
}

/// What one mixer track's lateness depends on.
#[derive(Debug, Clone, Default)]
pub(super) struct TrackIn {
    /// The output and input latency settings in milliseconds, either sign (event 236).
    pub out: f32,
    pub inp: f32,
    /// The tracks it sends to: every route, whatever its level.
    pub sends: Vec<usize>,
    /// The largest delay among the instrument channels assigned to it, switched off or not.
    pub chan: u32,
    /// The delays of its slots that run, summed.
    pub slots: u64,
}

/// How late each mixer track is, in samples, and where the export starts.
#[derive(Debug, Clone, Default, PartialEq)]
pub(super) struct Lateness {
    /// `arriving(t)`, `leaving(t)` (spec: "How late each place is").
    pub arriving: Vec<i64>,
    pub leaving: Vec<i64>,
    /// The further delay of what the channels play into the track: `smp(|in|)` for a negative input latency, else 0.
    pub extra: Vec<i64>,
    /// The samples skipped at the head of the stream (spec: "Where the export starts").
    pub start: u64,
}

impl Lateness {
    /// The pad of a channel whose instrument reports `delay`, playing into track `t`.
    pub fn channel_pad(&self, t: usize, delay: u32) -> usize {
        (self.arriving[t] - i64::from(delay) + self.extra[t]).max(0) as usize
    }

    /// The pad of a send from `s` into `d`.
    pub fn send_pad(&self, s: usize, d: usize) -> usize {
        (self.arriving[d] - self.leaving[s]).max(0) as usize
    }
}

/// Works out how late each track is, visiting the tracks in `order` (every track before the tracks it sends to); track
/// 0 is the master. The lateness is counted in samples, the export start apart in milliseconds (spec: "How late each
/// place is", "Where the export starts").
pub(super) fn lateness(tracks: &[TrackIn], order: &[usize], rate: u32) -> Lateness {
    let n = tracks.len();
    let mut l = Lateness { arriving: vec![0; n], leaving: vec![0; n], extra: vec![0; n], start: 0 };
    // The largest `leaving` and `total` of the tracks sending into each track, as the sources are visited.
    let mut from = vec![0i64; n];
    let mut total_from = vec![f64::NEG_INFINITY; n];
    let mut total = vec![0.0f64; n];
    for &t in order {
        let tr = &tracks[t];
        let (inp, out) = (f64::from(tr.inp), f64::from(tr.out));
        let c = ms(i64::from(tr.chan), rate);
        // The channel lateness: at least the input latency when it is positive, more by it when it is negative.
        let chan = if inp >= 0.0 { smp(c.max(inp), rate) } else { smp(c + inp.abs(), rate) };
        l.arriving[t] = chan.max(from[t]);
        l.leaving[t] = l.arriving[t] + tr.slots as i64 + smp(out, rate);
        if inp < 0.0 {
            l.extra[t] = smp(inp.abs(), rate);
        }
        total[t] = inp.max(total_from[t]).max(c) + out + ms(tr.slots as i64, rate);
        for &d in tr.sends.iter().filter(|&&d| d < n) {
            from[d] = from[d].max(l.leaving[t]);
            total_from[d] = total_from[d].max(total[t]);
        }
    }
    if n > 0 {
        l.start = smp(f64::from(total[0] as f32), rate).max(0) as u64;
    }
    l
}

/// Where the stream was in the song while it rendered one tick (the playhead follows what is heard).
#[derive(Debug, Clone, Copy)]
struct Mark {
    /// The tick's first frame in the stream, and its length.
    at: u64,
    len: u64,
    tick: u64,
    frame: u64,
    held: bool,
}

/// The engine's delay compensation state (spec: audio/delay-compensation.md).
#[derive(Debug, Default)]
pub(super) struct Pdc {
    /// Per channel: the delay its instrument last reported, and the pad between it and its mixer track.
    pub chan_delay: Vec<u32>,
    pub chan_pad: Vec<Pad>,
    /// What the pads were last worked out with, for the automation waits: per track `leaving`, per slot `arriving`
    /// plus the delays of the slots above it that ran; and per track whether a slot was on (the master always counts).
    leaving: Vec<i64>,
    slot_late: Vec<[i64; 10]>,
    any_on: Vec<bool>,
    /// Slots that were off when the project was loaded: values for them never wait.
    off_at_load: Vec<[bool; 10]>,
    /// The longest a sound takes through the pads, and the frames a live engine still plays after its last sound.
    reach: u64,
    pub drain: u64,
    /// Work the pads out again at the start of the next tick, before its values arrive: a delay changed.
    pub due: bool,
    /// The next tick is the engine's first or a seek's: the pads are worked out after its slot values arrived, with
    /// the delays read there and then.
    pub first: bool,
    /// ... and a new head of the stream is skipped (not when an engine takes over playback).
    pub fresh: bool,
    /// A slot was switched: an insert whose slots all went off, or one came on, works the pads out again.
    pub switched: bool,
    /// The first tick's slot values have reached the slots already.
    pub delivered: bool,
    /// The pass's export start: the frames skipped at the head of the stream, and those still to skip. Once the tail
    /// rule has ended the song, the frames still to hand out of what follows (the file keeps its length).
    pub lag: u64,
    pub head: u64,
    pub flush: Option<u64>,
    /// Frames of the stream rendered in this pass, and where the ticks still to be heard were in the song.
    stream: u64,
    marks: VecDeque<Mark>,
    /// Automation values waiting for late audio: the tick they take effect on, the target, the value.
    pub waiting: Vec<(u64, Target, i64)>,
    /// Split mixer tracks: per track, frames of its output still to skip; frames handed out, and handed over with
    /// the tracks' output.
    pub cap_skip: Vec<u64>,
    pub handed: u64,
    pub taken: u64,
}

impl Pdc {
    /// For an engine with `channels` channels whose tracks have `slot_on` switches as loaded.
    pub fn new(channels: usize, slot_on: impl Iterator<Item = [bool; 10]>) -> Pdc {
        Pdc {
            chan_delay: vec![0; channels],
            chan_pad: vec![Pad::default(); channels],
            off_at_load: slot_on.map(|s| s.map(|on| !on)).collect(),
            first: true,
            fresh: true,
            ..Pdc::default()
        }
    }

    /// The whole ticks (of `tick` samples) a value for `target` waits for late audio (spec: "Automation waits for
    /// late audio"): a track's settings wait for its `leaving`, a slot's for the audio reaching it. A slot off at load
    /// never waits, nor does anything for a negative lateness.
    pub fn wait(&self, target: Target, tick: i64) -> u64 {
        let late = match target {
            Target::MixerVolume(t) | Target::MixerPan(t) | Target::MixerSeparation(t) | Target::MixerEq(t, ..) | Target::MixerSend(t, _) => {
                self.leaving.get(usize::from(t)).copied().unwrap_or(0)
            }
            Target::SlotOn(t, s) | Target::SlotMix(t, s) | Target::SlotParam(t, s, _) => {
                let (t, s) = (usize::from(t), usize::from(s));
                if self.off_at_load.get(t).and_then(|o| o.get(s)).copied().unwrap_or(false) {
                    return 0;
                }
                self.slot_late.get(t).and_then(|l| l.get(s)).copied().unwrap_or(0)
            }
            _ => 0,
        };
        if late <= 0 { 0 } else { (late / tick.max(1)) as u64 }
    }

    /// A seek: compensation starts afresh, with empty pads (the engine empties the tracks' own) and the head skipped
    /// again; nothing waits any more.
    pub fn restart(&mut self) {
        for p in &mut self.chan_pad {
            p.clear();
        }
        self.waiting.clear();
        self.marks.clear();
        (self.first, self.fresh, self.due) = (true, true, false);
        (self.lag, self.head, self.flush, self.stream) = (0, 0, None, 0);
    }

    /// Takes over from `old`, an engine that played until now: the head it skipped, where its stream was heard, its
    /// channels' pads (by rack position) and delays. Its waiting values are dropped: the new engine hands its values on
    /// again. The delays are read again on the first tick.
    pub fn take_over(&mut self, old: &mut Pdc) {
        for (new, o) in self.chan_pad.iter_mut().zip(&mut old.chan_pad) {
            *new = std::mem::take(o);
        }
        for (new, &o) in self.chan_delay.iter_mut().zip(&old.chan_delay) {
            *new = o;
        }
        (self.lag, self.head, self.flush, self.stream, self.drain) = (old.lag, old.head, old.flush, old.stream, old.drain);
        self.marks = std::mem::take(&mut old.marks);
        (self.handed, self.taken) = (old.handed, old.taken);
        (self.first, self.fresh) = (true, false);
    }
}

impl Engine {
    /// The tick length the automation waits count in: `Round(samples per tick)`.
    pub(super) fn tick_len(&self) -> i64 {
        self.clock.samples_per_tick().round_ties_even().max(1.0) as i64
    }

    /// The whole ticks a value for `target` waits for late audio.
    pub(super) fn wait_ticks(&self, target: Target) -> u64 {
        self.pdc.wait(target, self.tick_len())
    }

    /// Whether track `t`'s slots run at all: its "effect slots enabled" switch, and the export's switch for inserts or
    /// the master.
    fn slots_run(&self, t: usize) -> bool {
        self.tracks[t].slots_enabled && if t == 0 { self.master_fx } else { self.insert_fx }
    }

    /// At least one of track `t`'s slots is on (the master always counts as having one).
    fn slots_on(&self, t: usize) -> bool {
        t == 0 || self.tracks[t].effects.iter().any(|f| f.on)
    }

    /// At the start of a tick, before its values arrive: when a delay changed during the last tick, or an insert's
    /// slots all went off (or one came on again), the pads are worked out again now (spec: "When the delays change
    /// during a song").
    pub(super) fn check_delays(&mut self) {
        if std::mem::take(&mut self.pdc.switched) && (0..self.tracks.len()).any(|t| self.pdc.any_on.get(t).is_some_and(|&on| on != self.slots_on(t))) {
            self.pdc.due = true;
        }
        if self.pdc.due && !self.pdc.first {
            self.plan_delays();
        }
    }

    /// The first tick of the engine or of a seek, after its automation: its slot values reach the slots, then the pads
    /// are worked out with the delays as they are now (spec: "Lemon Studio").
    pub(super) fn first_plan(&mut self) {
        for t in 0..self.tracks.len() {
            self.params.deliver_slots(t, &self.tracks[t].effects);
        }
        self.pdc.delivered = true;
        self.read_delays();
        self.plan_delays();
    }

    /// Reads every plugin's delay now, holding it (a busy one keeps the delay last read).
    fn read_delays(&mut self) {
        let (until, wait, export) = (&mut self.busy_until, self.busy_wait, self.exporting);
        for (c, ch) in self.channels.iter().enumerate() {
            let Some(inst) = ch.instrument.0.as_ref().filter(|i| !i.is_poisoned()) else { continue };
            if let (Some(i), Some(d)) = (lock_busy(inst, until, wait, export), self.pdc.chan_delay.get_mut(c)) {
                *d = i.reported_delay();
            }
        }
        for fx in self.tracks.iter_mut().flat_map(|t| t.effects.iter_mut()) {
            fx.delay = match &fx.fx {
                Fx::Own(e) => e.latency() as u32,
                Fx::Hosted(h) => match h.is_poisoned() {
                    true => 0,
                    false => lock_busy(h, until, wait, export).map_or(fx.delay, |h| h.reported_delay_fx()),
                },
            };
        }
    }

    /// Works out how late each place is and sets every pad to match (spec: "How late each place is", "Pads"); a pad
    /// whose length changes starts empty. On a pass's first tick also the head of the stream to skip.
    pub(super) fn plan_delays(&mut self) {
        let n = self.tracks.len();
        let mut ins: Vec<TrackIn> = (0..n)
            .map(|t| {
                let m = &self.tracks[t];
                let slots = if self.slots_run(t) { m.effects.iter().filter(|f| f.on).map(|f| u64::from(f.delay)).sum() } else { 0 };
                TrackIn { out: m.latency.0, inp: m.latency.1, sends: m.routes.iter().map(|r| r.dest).collect(), chan: 0, slots }
            })
            .collect();
        // Engine voices report none; a plugin channel counts also while it is switched off.
        for (c, ch) in self.channels.iter().enumerate() {
            if ch.instrument.0.is_some() {
                let d = self.pdc.chan_delay.get(c).copied().unwrap_or(0);
                ins[ch.insert].chan = ins[ch.insert].chan.max(d);
            }
        }
        let l = lateness(&ins, &self.order, self.rate);
        let runs: Vec<bool> = (0..n).map(|t| self.slots_run(t)).collect();
        let on: Vec<bool> = (0..n).map(|t| self.slots_on(t)).collect();
        self.pdc.slot_late = vec![[0; 10]; n];
        for (t, m) in self.tracks.iter_mut().enumerate() {
            // What the engine plays into the track itself (sampler voices, the metronome) reports no delay.
            m.pad.set_len(l.channel_pad(t, 0));
            for r in &mut m.routes {
                r.pad.set_len(l.send_pad(t, r.dest));
            }
            // A slot's dry part waits for its effect.
            for fx in &mut m.effects {
                fx.dry_pad.set_len(fx.delay as usize);
            }
            let mut late = l.arriving[t];
            for k in 0..10 {
                self.pdc.slot_late[t][k] = late;
                if runs[t] {
                    late += m.effects.iter().filter(|f| f.slot == k && f.on).map(|f| i64::from(f.delay)).sum::<i64>();
                }
            }
        }
        // Sidechain inputs: the track each slot listens to, its output delayed to be as late as the audio at the slot
        // (spec: audio/sidechain.md "Sidechain input").
        let mut keyed = vec![false; n];
        for t in 0..n {
            for i in 0..self.tracks[t].effects.len() {
                let fx = &self.tracks[t].effects[i];
                // A hosted plugin's port past the last input is no input (spec: audio/sidechain.md "Hosted plugins'
                // input buses").
                let hosted = matches!(fx.fx, super::Fx::Hosted(_));
                let pick = |k: usize| match k {
                    0 => None,
                    k if hosted => super::input_at(&self.tracks, t, k),
                    k => super::input(&self.tracks, t, k),
                };
                let (src, main) = (pick(fx.key_input()), pick(if hosted { fx.ports[0] } else { 0 }));
                let late = |s: Option<usize>| s.map_or(0, |s| (self.pdc.slot_late[t][fx.slot] - l.leaving[s]).max(0) as usize);
                let (len, main_len) = (late(src), late(main));
                let fx = &mut self.tracks[t].effects[i];
                fx.key = src;
                fx.key_pad.set_len(len);
                fx.main = main;
                fx.main_pad.set_len(main_len);
                for s in [src, main].into_iter().flatten() {
                    keyed[s] = true;
                }
            }
        }
        self.key_out.resize_with(n, || None);
        for (out, keyed) in self.key_out.iter_mut().zip(keyed) {
            if keyed != out.is_some() {
                *out = keyed.then(Vec::new);
            }
        }
        for (c, ch) in self.channels.iter().enumerate() {
            let len = if ch.instrument.0.is_some() { l.channel_pad(ch.insert, self.pdc.chan_delay.get(c).copied().unwrap_or(0)) } else { 0 };
            if let Some(p) = self.pdc.chan_pad.get_mut(c) {
                p.set_len(len);
            }
        }
        self.pdc.reach = (0..n).map(|t| l.leaving[t].max(l.arriving[t]) + l.extra[t]).max().unwrap_or(0).max(0) as u64;
        if self.pdc.fresh {
            // The head of the stream: the export starts this many samples in; so does live playback. Each track's output
            // for Split mixer tracks skips its own lateness, so it lines up with the song (a Lemon Studio choice).
            (self.pdc.lag, self.pdc.head) = (l.start, l.start);
            self.pdc.cap_skip = l.leaving.iter().map(|&v| (v.max(0) as u64).min(l.start)).collect();
        }
        self.pdc.leaving = l.leaving;
        self.pdc.any_on = on;
        (self.pdc.due, self.pdc.first, self.pdc.fresh) = (false, false, false);
    }

    /// Hands the tick just rendered into `self.out` out: the head of the stream is skipped, after the song's end only
    /// what is left of the skipped frames follows (`flushing`), and the position handed out with it is where the song
    /// was when what is heard first was made: `lag` frames earlier in the stream (spec: "Lemon Studio").
    pub(super) fn hand_out(&mut self, tick: u64, frame: u64, held: bool, flushing: bool) {
        let n = (self.out.len() / 2) as u64;
        let at = self.pdc.stream;
        self.pdc.stream += n;
        let skip = self.pdc.head.min(n);
        self.pdc.head -= skip;
        if skip > 0 {
            self.out.drain(..2 * skip as usize);
        }
        // Once they are out, an engine kept going (live notes still held) plays on as it renders.
        if let (true, Some(f)) = (flushing, self.pdc.flush.as_mut().filter(|f| **f > 0)) {
            let keep = (*f).min(n - skip);
            self.out.truncate(2 * keep as usize);
            *f -= keep;
        }
        self.pdc.handed += (self.out.len() / 2) as u64;
        // A live engine plays on while a sound is still on its way through the pads.
        if !self.voices.is_empty() || self.inst_tail > 0 || self.tap_clicks.sounding() {
            self.pdc.drain = self.pdc.reach;
        } else {
            self.pdc.drain = self.pdc.drain.saturating_sub(n);
        }
        self.out_pos = 0;
        if self.pdc.lag == 0 {
            (self.out_tick, self.out_frame, self.out_held, self.out_len) = (tick as f64, frame, held, n);
            return;
        }
        let marks = &mut self.pdc.marks;
        marks.push_back(Mark { at, len: n, tick, frame, held });
        let heard = (at + skip).saturating_sub(self.pdc.lag);
        while marks.len() > 1 && marks[1].at <= heard {
            marks.pop_front();
        }
        let m = marks[0];
        let into = heard.saturating_sub(m.at).min(m.len);
        self.out_len = m.len.max(1);
        (self.out_tick, self.out_frame, self.out_held) =
            if m.held { (m.tick as f64, m.frame, true) } else { (m.tick as f64 + into as f64 / self.out_len as f64, m.frame + into, false) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn milliseconds_take_the_single_nearest_to_a_thousandth() {
        // 220.5 exactly would round to 220.
        assert_eq!(smp(5.0, 44100), 221);
        assert_eq!(smp(-5.0, 44100), -221);
        assert_eq!(smp(ms(176, 44100), 44100), 176);
        assert_eq!(smp(ms(88, 48000), 48000), 88);
    }

    #[test]
    fn a_pad_delays_by_its_length_and_starts_empty_when_it_changes() {
        let mut p = Pad::default();
        let mut buf: Vec<f32> = (1..=8).map(|v| v as f32).collect();
        p.run(&mut buf);
        assert_eq!(buf, (1..=8).map(|v| v as f32).collect::<Vec<_>>(), "length 0 is none");
        p.set_len(3);
        let mut a = vec![1.0, -1.0, 2.0, -2.0];
        p.run(&mut a);
        let mut b = vec![3.0, -3.0, 4.0, -4.0, 5.0, -5.0];
        p.run(&mut b);
        assert_eq!((a, b), (vec![0.0; 4], vec![0.0, 0.0, 1.0, -1.0, 2.0, -2.0]));
        p.set_len(3);
        let mut c = vec![0.0; 2];
        p.run(&mut c);
        assert_eq!(c, vec![3.0, -3.0], "the same length keeps what it holds");
        p.set_len(1);
        let mut d = vec![9.0, 9.0, 0.0, 0.0];
        p.run(&mut d);
        assert_eq!(d, vec![0.0, 0.0, 9.0, 9.0], "a new length starts empty");
    }

    /// The spec's settings table ("Checked against FL"): impulses from a channel on insert 1 through it into the
    /// master, whose FLEX channel reports 176 samples at 44100 Hz; optionally a second insert 2 that insert 1 also
    /// sends to and that sends to the master. Returns the export start and the delay on the path through insert 1 into
    /// the master, and on the path through insert 2.
    fn impulses(out: [f32; 3], inp: [f32; 3], second: bool) -> (u64, i64, i64) {
        let mut t = vec![TrackIn::default(); 3];
        t[0].chan = 176;
        t[1].sends = if second { vec![0, 2] } else { vec![0] };
        if second {
            t[2].sends = vec![0];
        }
        for k in 0..3 {
            (t[k].out, t[k].inp) = (out[k], inp[k]);
        }
        let l = lateness(&t, &[2, 1, 0], 44100);
        // Processing order puts every source before its destinations: 1 sends to 2, so 1 is first.
        let l2 = lateness(&t, &[1, 2, 0], 44100);
        assert_eq!(l, l2, "only sources before destinations matter");
        let chan = l.channel_pad(1, 0) as i64;
        let direct = chan + l.send_pad(1, 0) as i64;
        let through = chan + l.send_pad(1, 2) as i64 + l.send_pad(2, 0) as i64;
        (l.start, direct, through)
    }

    #[test]
    fn latency_settings_move_the_paths_and_the_export_start_as_in_fl() {
        let z = [0.0; 3];
        // Without settings: the path waits for the FLEX channel, and the export starts that late.
        let (s, p, _) = impulses(z, z, false);
        assert_eq!((s, p), (176, 176));
        // (setting, export start, path delay); the impulses move by the path delay minus the export start.
        for (out, inp, start, path) in [
            ([0.0, 5.0, 0.0], z, 221, 0),
            ([0.0, -5.0, 0.0], z, 176, 397),
            (z, [0.0, 5.0, 0.0], 221, 221),
            (z, [0.0, -5.0, 0.0], 176, 442),
            ([5.0, 0.0, 0.0], z, 397, 176),
            ([-5.0, 0.0, 0.0], z, 0, 176),
        ] {
            let (s, p, _) = impulses(out, inp, false);
            assert_eq!((s, p), (start, path), "out {out:?} in {inp:?}");
        }
        // The second insert: the direct path does not move, the path through the second insert moves by -221 / +221.
        let (s, direct, through) = impulses([0.0, 0.0, 5.0], z, true);
        assert_eq!((direct - s as i64, through - s as i64), (0, -221));
        let (s, direct, through) = impulses([0.0, 0.0, -5.0], z, true);
        assert_eq!((direct - s as i64, through - s as i64), (0, 221));
        // A negative input latency on the second insert, which no channel plays into: 221 on both paths, both +45.
        assert_eq!(impulses(z, [0.0, 0.0, -5.0], true), (176, 221, 221));
    }

    #[test]
    fn slots_that_run_and_channels_add_up_through_sends() {
        // Insert 1 with a limiter (88) and insert 2 with none, both into the master; insert 3 with two delayed slots
        // sends into insert 1.
        let mut t = vec![TrackIn::default(); 4];
        t[1] = TrackIn { sends: vec![0], slots: 88, ..TrackIn::default() };
        t[2] = TrackIn { sends: vec![0], ..TrackIn::default() };
        t[3] = TrackIn { sends: vec![1], slots: 300, chan: 40, ..TrackIn::default() };
        let l = lateness(&t, &[3, 2, 1, 0], 44100);
        assert_eq!(l.arriving, vec![428, 340, 0, 40]);
        assert_eq!(l.leaving, vec![428, 428, 0, 340]);
        assert_eq!((l.send_pad(3, 1), l.send_pad(1, 0), l.send_pad(2, 0)), (0, 0, 428));
        assert_eq!((l.channel_pad(3, 40), l.channel_pad(3, 0), l.channel_pad(1, 0)), (0, 40, 340));
        assert_eq!(l.start, 428);
    }

    // The engine with delay compensation: 125 BPM at 96 PPQ and 48 kHz, so a tick is exactly 240 frames.

    use std::sync::{Arc, Mutex};

    use os_model::{AutoPoint, Automation, Channel, ChannelKind, Clip, ClipSource, MixerTrack, Note, Pattern, Project};

    use crate::{Engine, PlayMode, SampleLibrary};

    const TICK: usize = 240;

    /// A sawtooth while its note is held, from the note's very frame; with `drone` it sounds from the start and never
    /// stops (and says so).
    #[derive(Default)]
    struct Gate {
        on: Option<u32>,
        off: Option<u32>,
        held: bool,
        n: u32,
        drone: bool,
    }

    impl crate::Instrument for Gate {
        fn note_on(&mut self, offset: u32, _key: u8, _velocity: f32) {
            self.on = Some(offset);
        }
        fn note_off(&mut self, offset: u32, _key: u8) {
            self.off = Some(offset);
        }
        fn process(&mut self, _rate: u32, l: &mut [f32], r: &mut [f32]) {
            for i in 0..l.len() {
                if self.on == Some(i as u32) {
                    (self.held, self.on) = (true, None);
                }
                if self.off == Some(i as u32) {
                    (self.held, self.off) = (false, None);
                }
                let v = if self.held || self.drone { 0.25 + (self.n % 97) as f32 / 400.0 } else { 0.0 };
                self.n += 1;
                (l[i], r[i]) = (v, -v);
            }
        }
        fn active(&self) -> bool {
            self.drone
        }
    }

    /// An effect that delays by its frames and reports them, as one with look-ahead does; the delay it reports can be
    /// changed from outside.
    struct Late {
        line: std::collections::VecDeque<[f32; 2]>,
        reports: Arc<std::sync::atomic::AtomicU32>,
    }

    impl Late {
        fn new(frames: usize) -> Late {
            Late { line: vec![[0.0; 2]; frames].into(), reports: Arc::new((frames as u32).into()) }
        }
    }

    impl crate::HostedEffect for Late {
        fn process_fx(&mut self, _rate: u32, l: &mut [f32], r: &mut [f32]) {
            for i in 0..l.len() {
                self.line.push_back([l[i], r[i]]);
                [l[i], r[i]] = self.line.pop_front().unwrap();
            }
        }
        fn reported_delay_fx(&self) -> u32 {
            self.reports.load(std::sync::atomic::Ordering::Relaxed)
        }
    }

    fn gate_channel(index: u16, insert: u16) -> Channel {
        Channel {
            index,
            kind: ChannelKind::Plugin,
            name: None,
            plugin: None,
            plugin_state: None,
            color: None,
            enabled: true,
            volume: 0.78125,
            pan: 0.0,
            pitch_cents: 0,
            mixer_track: insert,
            sample_path: None,
            layer_children: vec![],
            envelopes: Default::default(),
            rack: Default::default(),
            misc: Default::default(),
            sound: Default::default(),
        }
    }

    /// One bar: a plugin channel on each of `inserts`, each holding a note over the first two beats, and `tracks`
    /// mixer tracks routed to the master.
    fn project(inserts: &[u16], tracks: u16) -> Project {
        let notes = (0..inserts.len()).map(|c| Note { length: 192, ..Note::step(c as u16, 0) }).collect();
        Project {
            ppq: 96,
            tempo: Some(125.0),
            channels: inserts.iter().enumerate().map(|(c, &t)| gate_channel(c as u16, t)).collect(),
            patterns: vec![Pattern { number: 1, notes, ..Pattern::default() }],
            mixer: (0..tracks).map(MixerTrack::new).collect(),
            ..Project::default()
        }
    }

    /// The engine for `p` with a `Gate` on every plugin channel (droning with `drone`) and the effects `fx` (track,
    /// slot, effect) in the mixer.
    fn engine(p: &Project, fx: Vec<(usize, usize, Late)>, drone: bool) -> Engine {
        let mode = if p.arrangements.is_empty() { PlayMode::Pattern(1) } else { PlayMode::Song };
        let mut e = Engine::new(p, &mut SampleLibrary::new(None), 48000, mode);
        for c in p.channels.iter().filter(|c| c.kind == ChannelKind::Plugin) {
            e.set_instrument(usize::from(c.index), Arc::new(Mutex::new(Gate { drone, ..Gate::default() })));
        }
        for (t, s, f) in fx {
            e.set_effect(t, s, Arc::new(Mutex::new(f)));
        }
        e
    }

    fn export(p: &Project, fx: Vec<(usize, usize, Late)>) -> Vec<f32> {
        engine(p, fx, false).render_all(0)
    }

    /// The first frame that is not silent.
    fn onset(s: &[f32]) -> usize {
        s.iter().position(|&v| v != 0.0).unwrap_or(s.len()) / 2
    }

    #[test]
    fn an_export_shows_no_delay_where_one_insert_is_late() {
        // Channels on inserts 1 and 2, both into the master; insert 1 holds an effect 100 frames late.
        let p = project(&[1, 2], 3);
        let reference = export(&p, vec![(1, 0, Late::new(0))]);
        assert_eq!(onset(&reference), 0);
        let late = export(&p, vec![(1, 0, Late::new(100))]);
        assert_eq!(late.len(), reference.len(), "the file keeps its length");
        assert!(late == reference, "every sample in place");
        // Two late slots on one insert add up; a slot switched off at load counts nothing.
        let late = export(&p, vec![(1, 0, Late::new(60)), (1, 3, Late::new(40))]);
        assert!(late == reference);
        let mut off = p.clone();
        off.mixer[1].slot_on[0] = false;
        let late = export(&off, vec![(1, 0, Late::new(100))]);
        assert!(late == export(&off, vec![(1, 0, Late::new(0))]));
    }

    #[test]
    fn a_slot_mixed_below_full_keeps_its_dry_part_in_line() {
        let mut p = project(&[1], 2);
        p.mixer[1].slot_mix[0] = 0.5;
        let reference = export(&p, vec![(1, 0, Late::new(0))]);
        let late = export(&p, vec![(1, 0, Late::new(100))]);
        assert!(late == reference, "the dry part waits for the effect");
    }

    #[test]
    fn a_send_waits_for_the_path_through_a_late_track() {
        // Insert 1 into the master and into insert 2, which is late and goes into the master.
        let mut p = project(&[1], 3);
        p.mixer[1].routes = vec![0, 2];
        let reference = export(&p, vec![(2, 0, Late::new(0))]);
        let late = export(&p, vec![(2, 0, Late::new(130))]);
        assert!(late == reference, "both paths meet in the master");
        let mut e = engine(&p, vec![(2, 0, Late::new(130))], false);
        e.next_tick();
        let pads = |e: &Engine| e.tracks[1].routes.iter().map(|r| (r.dest, r.pad.len())).collect::<Vec<_>>();
        assert_eq!(pads(&e), vec![(0, 130), (2, 0)]);
        assert_eq!((e.tracks[2].routes[0].pad.len(), e.pdc.lag), (0, 130));
    }

    #[test]
    fn latency_settings_are_compensated_like_a_slots_delay() {
        // An output latency of 5 ms on insert 1 (240 frames at 48 kHz) delays insert 2 and starts the export as late.
        let with_out = |p: &mut Project| p.mixer[1].latency_out = 5.0;
        let mut p = project(&[1, 2], 3);
        with_out(&mut p);
        let mut e = engine(&p, vec![], false);
        e.next_tick();
        assert_eq!((e.pdc.lag, e.tracks[1].routes[0].pad.len(), e.tracks[2].routes[0].pad.len()), (240, 0, 240));
        // So a channel on insert 2 is on time in the file, ...
        let reference = export(&project(&[2], 3), vec![]);
        let mut p = project(&[2], 3);
        with_out(&mut p);
        let set = export(&p, vec![]);
        assert!(set == reference);
        // ... and one on insert 1, which the setting does not delay, is early by it.
        let reference = export(&project(&[1], 3), vec![]);
        let mut p = project(&[1], 3);
        with_out(&mut p);
        let set = export(&p, vec![]);
        assert_eq!(set.len(), reference.len());
        assert!(set[..set.len() - 2 * TICK] == reference[2 * TICK..]);
    }

    #[test]
    fn automation_waits_the_whole_ticks_its_track_is_late() {
        use os_model::target;
        // A volume ramp on insert 1, whose effect is two ticks late: the export equals the one without the delay.
        let mut p = project(&[1], 2);
        p.channels.push(Channel { kind: ChannelKind::Automation, ..gate_channel(1, 0) });
        let points = vec![AutoPoint::new(0.0, 0.8), AutoPoint::new(4.0, 0.1)];
        p.automations.push(Automation { channel: 1, targets: vec![target::mixer(1, target::MIXER_VOLUME)], points, header: vec![0; 21], tail: vec![] });
        p.arrangements = vec![os_model::Arrangement {
            clips: vec![Clip::new(0, 384, 0, ClipSource::Pattern(1)), Clip::new(0, 384, 1, ClipSource::Channel(1))],
            tracks: (1..=2).map(os_model::PlaylistTrack::new).collect(),
            ..Default::default()
        }];
        let reference = export(&p, vec![(1, 0, Late::new(0))]);
        let late = export(&p, vec![(1, 0, Late::new(2 * TICK))]);
        assert_eq!(late.len(), reference.len());
        assert!(late == reference, "the ramp meets the audio it was for");
        // 1.5 ticks late: the values wait one tick, so the ramp is half a tick early in the file.
        let late = export(&p, vec![(1, 0, Late::new(TICK + TICK / 2))]);
        assert!(late != reference);
        let mut e = engine(&p, vec![(1, 0, Late::new(TICK + TICK / 2))], false);
        e.next_tick();
        assert_eq!(e.wait_ticks(os_model::target::Target::MixerVolume(1)), 1);
        assert_eq!(e.wait_ticks(os_model::target::Target::SlotMix(1, 0)), 0, "nothing reaches the slot late");
    }

    #[test]
    fn a_delay_of_a_tick_or_more_ends_the_song_on_its_first_tick_after_the_end() {
        // An instrument that never stops: the tail test sees none of the tick and calls it quiet.
        let p = project(&[1], 2);
        let mut s = Vec::new();
        assert!(engine(&p, vec![(1, 0, Late::new(TICK))], true).render_ticks(1000, &mut s));
        assert_eq!(s.len(), 2 * 385 * TICK, "the bar and one tick");
        // Less than a tick: the rest of the tick is tested, and the drone goes on (to the tail's limit).
        assert!(!engine(&p, vec![(1, 0, Late::new(TICK - 1))], true).render_ticks(1000, &mut s));
    }

    #[test]
    fn live_playback_skips_the_same_head_and_the_playhead_follows_what_is_heard() {
        let p = project(&[1, 2], 3);
        let export = export(&p, vec![(1, 0, Late::new(100))]);
        let mut e = engine(&p, vec![(1, 0, Late::new(100))], false);
        let mut heard = Vec::new();
        let mut blocks = Vec::new();
        while let Some((s, tick, frame, held, len)) = e.next_tick() {
            heard.extend_from_slice(s);
            blocks.push((s.len() / 2, tick, frame, held, len));
        }
        assert!(heard == export, "playback hears the export");
        // The first block lacks the head; each one after starts where the song was when its first frame was made.
        assert_eq!(blocks[0], (TICK - 100, 0.0, 0, false, TICK as u64));
        assert_eq!(blocks[1], (TICK, (TICK - 100) as f64 / TICK as f64, (TICK - 100) as u64, false, TICK as u64));
        assert_eq!(blocks[5].1, 4.0 + (TICK - 100) as f64 / TICK as f64);
        // A seek starts again: empty pads and the head skipped again.
        let mut e = engine(&p, vec![(1, 0, Late::new(100))], false);
        for _ in 0..10 {
            e.next_tick();
        }
        e.seek_tick(96);
        let (n, tick) = e.next_tick().map(|(s, t, ..)| (s.len() / 2, t)).unwrap();
        assert_eq!((n, tick), (TICK - 100, 96.0));
    }

    #[test]
    fn a_delay_that_changes_moves_the_pads_from_the_next_tick() {
        let p = project(&[1, 2], 3);
        let late = Late::new(100);
        let reports = late.reports.clone();
        let mut e = engine(&p, vec![(1, 0, late)], false);
        e.next_tick();
        assert_eq!((e.tracks[2].routes[0].pad.len(), e.pdc.lag), (100, 100));
        reports.store(150, std::sync::atomic::Ordering::Relaxed);
        e.next_tick();
        assert!(e.pdc.due, "read after the slot processed");
        e.next_tick();
        assert_eq!((e.tracks[2].routes[0].pad.len(), e.pdc.lag), (150, 100), "no new head mid-song");
        // An insert whose only slot goes off: worked out again with the next tick.
        e.tracks[1].set_slot_on(0, false);
        e.pdc.switched = true;
        e.next_tick();
        assert_eq!(e.tracks[2].routes[0].pad.len(), 0);
    }
}
