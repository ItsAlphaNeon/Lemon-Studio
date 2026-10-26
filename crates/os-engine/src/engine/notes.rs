// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Channel note rules of instrument channels: cut groups, the voice limit, mono and portamento (the plugin window's
//! Misc tab). They act on the note events the engine sends an instrument, once per tick, so they work with any plugin;
//! the glides need a plugin that takes per-key tuning (`Instrument::tune`: CLAP note expression).
//!
//! The rules keep their own picture of each channel's voices: which notes started on which tick, which have ended
//! but still ring (a one-shot sample plays on past its note's end), which are closing. A one-shot voice is taken to
//! be over once its sample has played out at the note's key; a voice whose length is not known stays until a cut,
//! the limit or the channel's voice cap ends it.
// spec: audio/note-rules.md

use os_model::ChannelMisc;

/// Most voices the rules keep per channel; the oldest goes when a new one would exceed it.
const POOL_CAP: usize = 256;

/// One channel's rules.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Rules {
    /// Most voices at once; 0 = no limit.
    pub max: u32,
    pub mono: bool,
    pub porta: bool,
    /// Glide length in ticks ("The length of a glide").
    pub glide: u32,
    /// The group this channel's notes cut, and the group its voices belong to (0 = none).
    pub cut: u32,
    pub cut_by: u32,
    /// The channel's voices are one-shot voices: a sampler or audio clip channel without a volume envelope, whose
    /// note ends never reach them.
    pub one_shot: bool,
    /// An instrument plugin channel: the plugin limits its voices itself (the oldest held note ends).
    pub plugin: bool,
    /// Output frames of the whole sample played at the root key (sampler channels; infinite when not known), and
    /// that root key.
    pub ring: f64,
    pub root: u8,
    /// Output frames a voice of a hosted sampler that loops its sample sounds after its note's end: its closing fade,
    /// or its volume envelope's release and then the fade (spec: audio/sampler.md "The end of a voice"). Infinite on
    /// other channels, whose voices play their sample out (`ring`).
    pub tail: f64,
    /// Frames of the closing fade of a sampler voice (the channel's declick mode, spec: audio/declick.md).
    pub close: u32,
}

impl Default for Rules {
    fn default() -> Self {
        Rules {
            max: 0,
            mono: false,
            porta: false,
            glide: 0,
            cut: 0,
            cut_by: 0,
            one_shot: false,
            plugin: true,
            ring: f64::INFINITY,
            root: 60,
            tail: f64::INFINITY,
            close: 0,
        }
    }
}

impl Rules {
    /// The rules stored in `m` at `ppq` ticks per quarter note; the engine fills in the channel's kind and sample.
    pub fn new(m: &ChannelMisc, ppq: u16) -> Rules {
        Rules {
            max: m.max_poly,
            mono: m.mono,
            porta: m.porta,
            glide: crate::laws::glide_ticks(m.slide, ppq),
            cut: m.cut,
            cut_by: m.cut_by,
            ..Rules::default()
        }
    }
}

/// Closing fade of a sampler voice for declick mode `mode` in frames at `rate` (spec: audio/declick.md, the mode
/// table's closing column).
pub(super) fn closing_frames(mode: u8, rate: u32) -> u32 {
    let ms = match mode {
        2 | 3 => 20.0,
        4 => 100.0,
        5 => 200.0,
        6 => 2.0,
        _ => 10.0,
    };
    (ms * 0.001 * f64::from(rate)).round_ties_even() as u32
}

/// A note event for an instrument.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Note {
    /// `fine`: the note's fine pitch in semitones, a tuning of its own on top of the channel's; `levels`: its pan and
    /// mod X / Y.
    /// `voice`: an audio clip's voice, named for its gains and its end (`Instrument::voice_start`).
    On { key: u8, vel: f32, start: f32, fine: f32, levels: crate::NoteLevels, voice: Option<u32> },
    /// The note's end: a plugin's own release, a sampler voice's envelope release.
    Off(u8),
    /// A sampler voice's closing fade: the voice named when it is an audio clip's, else the key's voices.
    Choke(u8, Option<u32>),
    /// Tuning of the voices of a key, semitones from the key's own pitch.
    Tune(u8, f32),
}

/// Event velocities below zero: a note off, a choke (an audio clip's end), a slide note, a layer note's own cut.
pub(super) const OFF: f32 = -1.0;
pub(super) const CHOKE: f32 = -2.0;
pub(super) const SLIDE: f32 = -3.0;
pub(super) const CUT: f32 = -4.0;

/// A note event of one tick as the engine collects it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Event {
    pub ch: usize,
    /// Frame in the tick.
    pub off: u32,
    pub key: u8,
    /// Velocity 0..1, or [`OFF`] / [`CHOKE`].
    pub vel: f32,
    /// Sample start (0..1 of the sample).
    pub start: f32,
    /// The note's fine pitch, semitones.
    pub fine: f32,
    /// The note has a length (a step's note has none).
    pub long: bool,
    /// A slide note's length in ticks: its glide takes that many ticks plus one steps.
    pub glide: u32,
    /// The note's own pan and mod X / Y.
    pub levels: crate::NoteLevels,
    /// A note a layer handed on: the group it cuts and the group its voice belongs to as the note carries them
    /// (`None`: the channel's own numbers).
    pub cuts: Option<(u32, u32)>,
    /// The layer channel it came through (its voices answer that layer's slide notes, not the channel's own).
    pub layer: Option<usize>,
    /// The note's own portamento switch: it inverts the channel's portamento for this note (spec:
    /// audio/note-rules.md, "A note's own portamento switch").
    pub porta: bool,
    /// An audio clip's voice (its start, its end): the engine's name for it (`Instrument::voice_start`).
    pub voice: Option<u32>,
}

impl Event {
    pub fn on(ch: usize, off: u32, key: u8, vel: f32) -> Event {
        Event { ch, off, key, vel, start: 0.0, fine: 0.0, long: true, glide: 0, levels: crate::NoteLevels::default(), cuts: None, layer: None, porta: false, voice: None }
    }
    /// A layer note's own cut group (spec: audio/note-rules.md "Layer channels": it acts before the note is handed on).
    pub fn cut(ch: usize, off: u32, group: u32) -> Event {
        Event { vel: CUT, cuts: Some((group, 0)), ..Event::on(ch, off, 0, 0.0) }
    }
    /// A slide note of `len` ticks to `key`.
    pub fn slide(ch: usize, off: u32, key: u8, len: u32) -> Event {
        Event { vel: SLIDE, glide: len, ..Event::on(ch, off, key, 0.0) }
    }
    pub fn off(ch: usize, off: u32, key: u8) -> Event {
        Event { vel: OFF, ..Event::on(ch, off, key, 0.0) }
    }
    pub fn choke(ch: usize, off: u32, key: u8) -> Event {
        Event { vel: CHOKE, ..Event::on(ch, off, key, 0.0) }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum State {
    /// Its note has not ended.
    Held,
    /// Its note has ended, or a cut released it; it sounds on (a one-shot sample, an envelope's release).
    Released,
    /// In its closing fade.
    Closing,
}

/// A voice as the rules see it.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Voice {
    /// Key sent to the instrument (a mono voice keeps it while it glides to other keys).
    sent: u8,
    /// Tick it started on.
    born: u64,
    /// Cut group it belongs to; 0 = none.
    group: u32,
    state: State,
    /// Frames it may still sound (its sample's rest; the closing fade once closing).
    left: f64,
    /// The layer its note came through.
    layer: Option<usize>,
    /// An audio clip's voice: the engine's name for it.
    voice: Option<u32>,
}

/// A glide of the tuning of instrument key `key` (spec "Glides": `n + 1` equal steps, the first on the tick it
/// starts). A finished glide stays as the key's tuning while its voice lives.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Glide {
    key: u8,
    now: f32,
    step: f32,
    left: u32,
    /// Frame of this tick where its step is sent.
    at: u32,
}

/// One channel's voices and note state.
#[derive(Debug, Clone, Default, PartialEq)]
pub(super) struct Voices {
    /// In FL's pool order: a new voice goes to the end, and when one is over the last takes its place.
    pool: Vec<Voice>,
    /// Mono: the keys of the notes that started and have not ended, oldest first, and the sounding voice (its
    /// instrument key and its own note's key).
    list: Vec<u8>,
    sounding: Option<(u8, u8)>,
    /// Portamento's last levels: the key of the last note that started, or the target of the last glide.
    last: Option<f32>,
    glides: Vec<Glide>,
    /// Ticks done.
    tick: u64,
}

impl Voices {
    /// Instrument keys of the voices whose notes have not ended (they get a note off when the engine goes away).
    pub fn held_keys(&self) -> impl Iterator<Item = u8> + '_ {
        self.pool.iter().filter(|v| v.state == State::Held).map(|v| v.sent)
    }

    /// Current tuning of instrument key `key`.
    fn tuning(&self, key: u8) -> f32 {
        self.glides.iter().find(|g| g.key == key).map_or(0.0, |g| g.now)
    }

    /// Starts a glide of key `key` from where it is to `to` semitones in `n + 1` steps; the first is sent at `at`.
    fn glide(&mut self, key: u8, to: f32, n: u32, at: u32) {
        let now = self.tuning(key);
        self.glides.retain(|g| g.key != key);
        let step = if to == now { 0.0 } else { (to - now) / (n + 1) as f32 };
        self.glides.push(Glide { key, now, step, left: n + 1, at });
    }

    /// Takes voice `i` out of the pool as FL does: the last voice takes its place.
    fn over(&mut self, i: usize) {
        let v = self.pool.swap_remove(i);
        self.voice_gone(v);
    }

    /// Mono bookkeeping when voice `v` (as it was) ends or is released by a cut: the sounding voice's own note leaves
    /// the list and the channel has no sounding voice until the list is empty. A voice whose note had already ended is
    /// not the sounding voice, though a later note may sound on its key.
    fn voice_gone(&mut self, v: Voice) {
        if v.state != State::Held {
            return;
        }
        if let Some((s, own)) = self.sounding {
            if s == v.sent {
                self.sounding = None;
                if let Some(i) = self.list.iter().position(|&k| k == own) {
                    self.list.remove(i);
                }
            }
        }
    }

    /// Releases voice `i` (a cut): what the release is depends on the channel (spec "Cut groups").
    fn release(&mut self, i: usize, r: &Rules, ch: usize, off: u32, out: &mut Vec<(usize, u32, Note)>) {
        let v = self.pool[i];
        if r.plugin {
            if v.state == State::Held {
                out.push((ch, off, Note::Off(v.sent)));
            }
            self.pool.remove(i);
            self.voice_gone(v);
        } else if r.one_shot {
            self.close(i, r, ch, off, out);
        } else if v.state == State::Held {
            out.push((ch, off, Note::Off(v.sent)));
            let p = &mut self.pool[i];
            (p.state, p.left) = (State::Released, p.left.min(r.tail));
            self.voice_gone(v);
        }
    }

    /// Starts voice `i`'s closing fade.
    fn close(&mut self, i: usize, r: &Rules, ch: usize, off: u32, out: &mut Vec<(usize, u32, Note)>) {
        let was = self.pool[i];
        let v = &mut self.pool[i];
        out.push((ch, off, Note::Choke(v.sent, v.voice)));
        v.state = State::Closing;
        v.left = v.left.min(f64::from(r.close));
        self.voice_gone(was);
    }
}

/// Applies each channel's rules to one tick's events (sorted by frame; note ends before note starts) and adds the
/// glides' tuning steps; `out` gets `(channel, frame, note)` sorted by channel, then frame. `n` is the tick's length
/// in frames.
pub(super) fn apply(events: &[Event], rules: &[Rules], voices: &mut [Voices], n: u32, out: &mut Vec<(usize, u32, Note)>) {
    out.clear();
    let count = rules.len().min(voices.len());
    for e in events {
        let c = e.ch;
        if c >= count || e.vel == SLIDE {
            continue;
        }
        let r = rules[c];
        if e.vel == CUT {
            cut(e.cuts.map_or(0, |n| n.0), voices, rules, e.off, out);
        } else if e.vel <= CHOKE {
            choke(&mut voices[c], &r, e, out);
        } else if e.vel < 0.0 {
            note_end(&mut voices[c], &r, e, out);
        } else {
            // The cut group acts first, on voices of every channel that started on an earlier tick; a note a layer
            // handed on cuts the group it carries.
            cut(e.cuts.map_or(r.cut, |n| n.0), voices, rules, e.off, out);
            note_start(&mut voices[c], &r, e, out);
        }
    }
    // Slide notes act after the tick's note ends and starts.
    for e in events.iter().filter(|e| e.vel == SLIDE && e.ch < count) {
        slide(&mut voices[e.ch], &rules[e.ch], e);
    }
    // Every voice: one glide step, then the time of the tick passes.
    for (c, v) in voices.iter_mut().enumerate().take(count) {
        for g in &mut v.glides {
            if g.left > 0 {
                g.now += g.step;
                g.left -= 1;
                out.push((c, g.at, Note::Tune(g.key, g.now)));
            }
            g.at = 0;
        }
        let mut i = 0;
        while i < v.pool.len() {
            v.pool[i].left -= f64::from(n);
            if v.pool[i].left <= 0.0 {
                v.over(i);
            } else {
                i += 1;
            }
        }
        let pool = &v.pool;
        v.glides.retain(|g| g.left > 0 || pool.iter().any(|p| p.sent == g.key));
        v.tick += 1;
    }
    out.sort_by_key(|e| (e.0, e.1));
}

/// Releases every voice of every channel that belongs to `group` and started on an earlier tick (spec "Cut groups");
/// group 0 cuts nothing.
fn cut(group: u32, voices: &mut [Voices], rules: &[Rules], off: u32, out: &mut Vec<(usize, u32, Note)>) {
    if group == 0 {
        return;
    }
    for d in 0..rules.len().min(voices.len()) {
        let v = &mut voices[d];
        let mut i = 0;
        while i < v.pool.len() {
            let p = v.pool[i];
            if p.group == group && p.born < v.tick && p.state != State::Closing && !(p.state == State::Released && !rules[d].one_shot) {
                let before = v.pool.len();
                v.release(i, &rules[d], d, off, out);
                if v.pool.len() < before {
                    continue;
                }
            }
            i += 1;
        }
    }
}

/// A slide note (spec: audio/song-structure.md "Slide notes", "Slide notes and these rules" here): every voice of the
/// channel not in its closing fade, released ones and ones started on this tick too, glides by `key - the pitch of the
/// highest held voice` in `len + 1` steps, replacing a glide under way from where it is. A one-shot voice counts as held
/// while it sounds; on a mono channel the notes that only wait count too. Without a held voice nothing moves. The
/// slide's key becomes the last levels for portamento. (The note's colour, velocity, pan and mod X / Y are not used
/// here: instruments take the pitch only.) A layer's slide note moves only the voices its notes started; a slide note
/// of the channel's own only the others (spec "Layer channels").
fn slide(v: &mut Voices, r: &Rules, e: &Event) {
    let held = |p: &Voice| p.layer == e.layer && (p.state == State::Held || (r.one_shot && p.state == State::Released));
    let mut top: Option<f32> = None;
    for p in v.pool.iter().filter(|p| held(p)) {
        let pitch = f32::from(p.sent) + v.tuning(p.sent);
        top = Some(top.map_or(pitch, |t| t.max(pitch)));
    }
    if r.mono {
        for &k in &v.list {
            top = Some(top.map_or(f32::from(k), |t| t.max(f32::from(k))));
        }
    }
    let Some(top) = top else { return };
    let by = f32::from(e.key) - top;
    let mut keys: Vec<u8> = v.pool.iter().filter(|p| p.state != State::Closing && p.layer == e.layer).map(|p| p.sent).collect();
    keys.sort_unstable();
    keys.dedup();
    for k in keys {
        let to = v.tuning(k) + by;
        v.glide(k, to, e.glide, e.off);
    }
    v.last = Some(f32::from(e.key));
}

/// An audio clip's end: its voice closes (the one the event names; another clip's voice on the key plays on).
fn choke(v: &mut Voices, r: &Rules, e: &Event, out: &mut Vec<(usize, u32, Note)>) {
    let mine = |p: &Voice| p.sent == e.key && p.state != State::Closing && (e.voice.is_none() || p.voice == e.voice);
    match v.pool.iter().position(mine) {
        Some(i) => v.close(i, r, e.ch, e.off, out),
        None => out.push((e.ch, e.off, Note::Choke(e.key, e.voice))),
    }
}

/// A note's end.
fn note_end(v: &mut Voices, r: &Rules, e: &Event, out: &mut Vec<(usize, u32, Note)>) {
    if r.mono {
        let Some(at) = v.list.iter().position(|&k| k == e.key) else { return };
        let own = v.sounding.is_some_and(|(_, own)| own == e.key);
        // A one-shot voice has no note end: its own note stays listed while it sounds.
        if own && r.one_shot {
            return;
        }
        v.list.remove(at);
        let Some((sent, _)) = v.sounding else { return };
        match v.list.last().copied() {
            None => {
                if let Some(i) = v.pool.iter().position(|p| p.sent == sent && p.state == State::Held) {
                    v.sounding = None;
                    end_voice(v, r, i, e, out);
                }
            }
            Some(newest) => {
                if own {
                    v.sounding = Some((sent, newest));
                }
                let to = f32::from(newest) - f32::from(sent);
                v.glide(sent, to, r.glide, e.off);
                v.last = Some(f32::from(newest));
            }
        }
        return;
    }
    // The oldest held voice of the key; a note whose voice a cut or the limit ended has nothing left to end.
    if let Some(i) = v.pool.iter().position(|p| p.sent == e.key && p.state == State::Held) {
        end_voice(v, r, i, e, out);
    }
}

/// Voice `i`'s note ends.
fn end_voice(v: &mut Voices, r: &Rules, i: usize, e: &Event, out: &mut Vec<(usize, u32, Note)>) {
    out.push((e.ch, e.off, Note::Off(v.pool[i].sent)));
    if r.plugin {
        v.pool.remove(i);
    } else {
        let p = &mut v.pool[i];
        (p.state, p.left) = (State::Released, p.left.min(r.tail));
    }
}

/// The group a note's voice belongs to on a channel with rules `r` and the numbers `cut`, `cut_by` (spec "Cut
/// groups"): "cut by", except that on a channel whose voices are not one-shot voices "cut itself" leaves a note with a
/// length out of any group.
pub(super) fn voice_group(r: &Rules, cut: u32, cut_by: u32, long: bool) -> u32 {
    if !r.one_shot && cut_by == cut && long { 0 } else { cut_by }
}

/// A note's start: mono, the voice limit, then the voice with its portamento glide.
fn note_start(v: &mut Voices, r: &Rules, e: &Event, out: &mut Vec<(usize, u32, Note)>) {
    let key = e.key;
    if r.mono {
        let first = v.list.is_empty();
        v.list.push(key);
        if !first {
            // The note only waits for its end; the sounding voice glides to it.
            if let Some((sent, _)) = v.sounding {
                v.glide(sent, f32::from(key) - f32::from(sent), r.glide, e.off);
                v.last = Some(f32::from(key));
            }
            return;
        }
    } else if r.max > 0 {
        if r.plugin {
            // The plugin is told the number: the oldest held note ends.
            while v.pool.iter().filter(|p| p.state == State::Held).count() >= r.max as usize {
                let Some(i) = v.pool.iter().position(|p| p.state == State::Held) else { break };
                end_voice(v, r, i, e, out);
            }
        } else {
            // Released voices count too; the last released one in pool order closes, else the first held one.
            let mut counted = 0;
            let mut candidate: Option<usize> = None;
            for (i, p) in v.pool.iter().enumerate() {
                if p.state == State::Closing {
                    continue;
                }
                counted += 1;
                if p.state == State::Released || candidate.is_none() {
                    candidate = Some(i);
                }
            }
            if counted >= r.max as usize {
                if let Some(i) = candidate {
                    v.close(i, r, e.ch, e.off, out);
                }
            }
        }
    }
    if v.pool.len() >= POOL_CAP {
        let gone = v.pool.remove(0);
        v.voice_gone(gone);
    }
    // A voice belongs to "cut by"; on a channel whose voices are not one-shot, "cut itself" leaves a note with a
    // length out of the group. A note a layer handed on carries its group.
    let group = match e.cuts {
        Some((_, group)) => group,
        None => voice_group(r, r.cut, r.cut_by, e.long),
    };
    let left = if r.plugin {
        f64::INFINITY
    } else {
        r.ring * f64::from(1.0 - e.start.clamp(0.0, 1.0)) * 2f64.powf((f64::from(r.root) - f64::from(key)) / 12.0)
    };
    // A retriggered key: its old tuning ends with it.
    v.glides.retain(|g| g.key != key);
    out.push((e.ch, e.off, Note::On { key, vel: e.vel, start: e.start, fine: e.fine, levels: e.levels, voice: e.voice }));
    v.pool.push(Voice { sent: key, born: v.tick, group, state: State::Held, left, layer: e.layer, voice: e.voice });
    if r.mono {
        v.sounding = Some((key, key));
    }
    // Portamento: the voice starts at the last levels and glides to its own.
    match v.last {
        // The channel's setting and the note's own switch differ (spec: audio/note-rules.md "Portamento").
        Some(last) if r.porta != e.porta && r.glide >= 1 && last != f32::from(key) => {
            let from = last - f32::from(key);
            v.glides.push(Glide { key, now: from, step: -from / (r.glide + 1) as f32, left: r.glide + 1, at: e.off });
        }
        _ => {}
    }
    v.last = Some(f32::from(key));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One tick of `n` frames.
    fn tick(rules: &[Rules], voices: &mut [Voices], ev: &[Event], n: u32) -> Vec<(usize, u32, Note)> {
        let mut out = Vec::new();
        apply(ev, rules, voices, n, &mut out);
        out
    }

    fn on(key: u8) -> Note {
        Note::On { key, vel: 0.8, start: 0.0, fine: 0.0, levels: crate::NoteLevels::default(), voice: None }
    }

    fn sampler() -> Rules {
        Rules { plugin: false, close: 480, ..Rules::default() }
    }

    #[test]
    fn glide_lengths_follow_the_slide_time() {
        let ticks = crate::laws::glide_ticks;
        let n: Vec<u32> = [100, 200, 300, 400, 500, 600, 700, 820, 900, 1000, 1024].iter().map(|&s| ticks(s as f32 / 1024.0, 96)).collect();
        assert_eq!(n, vec![0, 1, 1, 2, 3, 5, 8, 12, 16, 22, 24]);
        assert_eq!(ticks(700.0 / 1024.0, 192), 16);
        assert_eq!(closing_frames(0, 48000), 480);
        assert_eq!(closing_frames(5, 44100), 8820);
    }

    #[test]
    fn a_plugin_ends_its_oldest_held_note_at_the_limit() {
        let r = [Rules { max: 2, ..Rules::default() }];
        let mut v = vec![Voices::default()];
        let mut ends = Vec::new();
        for key in [60, 62, 64, 65] {
            for e in tick(&r, &mut v, &[Event::on(0, 0, key, 0.8)], 100) {
                if let Note::Off(k) = e.2 {
                    ends.push((key, k));
                }
            }
        }
        // Notes of ticks 0, 24, 48, 72: the third ends the first, the fourth the second.
        assert_eq!(ends, vec![(64, 60), (65, 62)]);
        // The ended notes' own note offs reach nothing.
        assert!(tick(&r, &mut v, &[Event::off(0, 0, 60), Event::off(0, 0, 62)], 100).is_empty());
    }

    #[test]
    fn a_sampler_limit_closes_by_pool_order() {
        // Maximum 2, notes on four ticks, the closing fade shorter than a tick: the fourth note closes the third
        // voice, which took the first one's place in the pool.
        let r = [Rules { max: 2, one_shot: true, ..sampler() }];
        let mut v = vec![Voices::default()];
        let mut closed = Vec::new();
        for key in [60, 62, 64, 65] {
            for e in tick(&r, &mut v, &[Event::on(0, 0, key, 0.8)], 1000) {
                if let Note::Choke(k, _) = e.2 {
                    closed.push((key, k));
                }
            }
        }
        assert_eq!(closed, vec![(64, 60), (65, 64)]);

        // A fade of nearly two ticks keeps the closing voice in its place: notes on ticks 0, 10, 11 and 12 close the
        // first voice, then the voice of tick 10.
        let r = [Rules { max: 2, one_shot: true, close: 1800, ..sampler() }];
        let mut v = vec![Voices::default()];
        let mut closed = Vec::new();
        for (key, gap) in [(60, 10), (62, 1), (64, 1), (65, 1)] {
            for _ in 1..gap {
                tick(&r, &mut v, &[], 1000);
            }
            for e in tick(&r, &mut v, &[Event::on(0, 0, key, 0.8)], 1000) {
                if let Note::Choke(k, _) = e.2 {
                    closed.push((key, k));
                }
            }
        }
        assert_eq!(closed, vec![(64, 60), (65, 62)]);
    }

    #[test]
    fn a_released_voice_is_the_first_candidate() {
        // An envelope voice released and still ringing goes before a held one.
        let r = [Rules { max: 2, ..sampler() }];
        let mut v = vec![Voices::default()];
        tick(&r, &mut v, &[Event::on(0, 0, 60, 0.8)], 100);
        tick(&r, &mut v, &[Event::on(0, 0, 62, 0.8)], 100);
        tick(&r, &mut v, &[Event::off(0, 0, 62)], 100);
        let out = tick(&r, &mut v, &[Event::on(0, 0, 64, 0.8)], 100);
        assert_eq!(out.iter().map(|e| e.2).collect::<Vec<_>>(), vec![Note::Choke(62, None), on(64)]);
    }

    #[test]
    fn one_shot_voices_play_out_and_leave_the_count() {
        // A sample of 250 frames at the root key plays for 125 frames an octave up: over during the second tick.
        let r = [Rules { max: 1, one_shot: true, ring: 250.0, root: 60, ..sampler() }];
        let mut v = vec![Voices::default()];
        tick(&r, &mut v, &[Event::on(0, 0, 72, 0.8)], 100);
        tick(&r, &mut v, &[], 100);
        let out = tick(&r, &mut v, &[Event::on(0, 0, 60, 0.8)], 100);
        assert_eq!(out.iter().map(|e| e.2).collect::<Vec<_>>(), vec![on(60)]);
    }

    #[test]
    fn cut_groups_release_earlier_voices_of_every_channel() {
        // Open hat (0) is cut by group 1; closed hat (1) cuts group 1; both one-shot.
        let hat = Rules { one_shot: true, ..sampler() };
        let r = [Rules { cut_by: 1, ..hat }, Rules { cut: 1, ..hat }];
        let mut v = vec![Voices::default(), Voices::default()];
        tick(&r, &mut v, &[Event::on(0, 0, 60, 0.8)], 100);
        // Its note has ended: the ringing voice is cut all the same.
        tick(&r, &mut v, &[Event::off(0, 0, 60)], 100);
        let out = tick(&r, &mut v, &[Event::on(1, 30, 60, 0.8)], 100);
        assert_eq!(out, vec![(0, 30, Note::Choke(60, None)), (1, 30, on(60))]);
    }

    /// Two audio clips of one channel overlap on one key: a clip's end closes its own voice, the other plays on and is
    /// closed at its own end; a cut closes a clip voice by its name.
    #[test]
    fn a_clip_end_closes_its_own_voice() {
        let r = [Rules { one_shot: true, cut_by: 1, ..sampler() }, Rules { cut: 1, ..sampler() }];
        let mut v = vec![Voices::default(), Voices::default()];
        let clip = |off: u32, voice: u32| Event { voice: Some(voice), ..Event::on(0, off, 60, 100.0 / 128.0) };
        let end = |voice: u32| Event { voice: Some(voice), ..Event::choke(0, 0, 60) };
        let out = tick(&r, &mut v, &[clip(0, 7)], 100);
        assert!(matches!(out[..], [(0, 0, Note::On { key: 60, voice: Some(7), .. })]), "{out:?}");
        tick(&r, &mut v, &[clip(0, 8)], 100);
        assert_eq!(tick(&r, &mut v, &[end(7)], 100), vec![(0, 0, Note::Choke(60, Some(7)))]);
        assert_eq!(tick(&r, &mut v, &[end(8)], 100), vec![(0, 0, Note::Choke(60, Some(8)))]);
        // Nothing of either left to close: the end goes to the instrument as it is.
        assert_eq!(tick(&r, &mut v, &[end(8)], 100), vec![(0, 0, Note::Choke(60, Some(8)))]);
        tick(&r, &mut v, &[clip(0, 9)], 100);
        let out = tick(&r, &mut v, &[Event::on(1, 5, 62, 0.8)], 100);
        assert_eq!(out[0], (0, 5, Note::Choke(60, Some(9))));
    }

    #[test]
    fn a_chord_survives_its_own_tick_and_the_next_note_cuts_all_of_it() {
        let r = [Rules { cut: 1, cut_by: 1, one_shot: true, ..sampler() }];
        let mut v = vec![Voices::default()];
        let out = tick(&r, &mut v, &[Event::on(0, 0, 60, 0.8), Event::on(0, 0, 64, 0.8), Event::on(0, 0, 67, 0.8)], 100);
        assert_eq!(out.iter().filter(|e| matches!(e.2, Note::Choke(..))).count(), 0);
        let out = tick(&r, &mut v, &[Event::on(0, 0, 72, 0.8)], 100);
        assert_eq!(out.iter().map(|e| e.2).collect::<Vec<_>>(), vec![Note::Choke(60, None), Note::Choke(64, None), Note::Choke(67, None), on(72)]);
    }

    #[test]
    fn cut_itself_leaves_notes_with_a_length_alone_on_other_channels() {
        let r = [Rules { cut: 3, cut_by: 3, ..Rules::default() }];
        let mut v = vec![Voices::default()];
        tick(&r, &mut v, &[Event::on(0, 0, 60, 0.8)], 100);
        let out = tick(&r, &mut v, &[Event::on(0, 0, 62, 0.8)], 100);
        assert_eq!(out.iter().map(|e| e.2).collect::<Vec<_>>(), vec![on(62)]);
        // A step's note (no length) belongs to the group and is cut.
        tick(&r, &mut v, &[Event { long: false, ..Event::on(0, 0, 64, 0.8) }], 100);
        let out = tick(&r, &mut v, &[Event::on(0, 0, 65, 0.8)], 100);
        assert_eq!(out.iter().map(|e| e.2).collect::<Vec<_>>(), vec![Note::Off(64), on(65)]);
        // Only one of the two words set does nothing.
        let r = [Rules { cut: 1, ..sampler() }];
        let mut v = vec![Voices::default()];
        tick(&r, &mut v, &[Event::on(0, 0, 60, 0.8)], 100);
        assert_eq!(tick(&r, &mut v, &[Event::on(0, 0, 62, 0.8)], 100).len(), 1);
    }

    #[test]
    fn mono_glides_the_sounding_voice_and_back() {
        // Mono, glide of 3 ticks: a second note moves the first voice in four steps; its end moves it back.
        let r = [Rules { mono: true, glide: 3, ..Rules::default() }];
        let mut v = vec![Voices::default()];
        tick(&r, &mut v, &[Event::on(0, 0, 60, 0.8)], 100);
        let out = tick(&r, &mut v, &[Event::on(0, 10, 64, 0.8)], 100);
        assert_eq!(out, vec![(0, 10, Note::Tune(60, 1.0))]);
        let steps: Vec<Note> = (0..4).flat_map(|_| tick(&r, &mut v, &[], 100)).map(|e| e.2).collect();
        assert_eq!(steps, vec![Note::Tune(60, 2.0), Note::Tune(60, 3.0), Note::Tune(60, 4.0)]);
        let out = tick(&r, &mut v, &[Event::off(0, 0, 64)], 100);
        assert_eq!(out, vec![(0, 0, Note::Tune(60, 3.0))]);
        for _ in 0..3 {
            tick(&r, &mut v, &[], 100);
        }
        // The last note's end releases the voice.
        let out = tick(&r, &mut v, &[Event::off(0, 0, 60)], 100);
        assert_eq!(out, vec![(0, 0, Note::Off(60))]);
        assert!(v[0].list.is_empty() && v[0].sounding.is_none());
    }

    #[test]
    fn the_sounding_voice_outlives_its_own_note_while_others_wait() {
        let r = [Rules { mono: true, glide: 0, ..Rules::default() }];
        let mut v = vec![Voices::default()];
        tick(&r, &mut v, &[Event::on(0, 0, 60, 0.8)], 100);
        // Length 0: a jump on the note's tick.
        let out = tick(&r, &mut v, &[Event::on(0, 0, 67, 0.8)], 100);
        assert_eq!(out, vec![(0, 0, Note::Tune(60, 7.0))]);
        assert!(tick(&r, &mut v, &[Event::off(0, 0, 60)], 100).iter().all(|e| !matches!(e.2, Note::Off(_))));
        let out = tick(&r, &mut v, &[Event::off(0, 0, 67)], 100);
        assert_eq!(out, vec![(0, 0, Note::Off(60))]);
    }

    /// A looped sample's voice (no volume envelope) is held until its note ends, however short the sample, and gets
    /// that end, on a mono channel too; it is over once its closing fade (`tail`) has passed. A one-shot voice of the
    /// same length is over when its sample has played and its note's end has nothing to end.
    #[test]
    fn a_looped_samples_voice_gets_its_note_end() {
        let one_shot = [Rules { one_shot: true, ring: 550.0, ..sampler() }];
        let looped = Rules { ring: f64::INFINITY, tail: 480.0, ..sampler() };
        for (r, what) in [([looped], "poly"), ([Rules { mono: true, ..looped }], "mono")] {
            let mut v = vec![Voices::default()];
            tick(&r, &mut v, &[Event::on(0, 0, 60, 0.8)], 1000);
            for _ in 0..20 {
                tick(&r, &mut v, &[], 1000);
            }
            assert_eq!(tick(&r, &mut v, &[Event::off(0, 0, 60)], 100), vec![(0, 0, Note::Off(60))], "{what}");
            assert_eq!(v[0].pool.len(), 1, "{what}: closing");
            tick(&r, &mut v, &[], 1000);
            assert!(v[0].pool.is_empty(), "{what}: over after its fade");
        }
        let mut v = vec![Voices::default()];
        tick(&one_shot, &mut v, &[Event::on(0, 0, 60, 0.8)], 1000);
        assert!(v[0].pool.is_empty());
        assert!(tick(&one_shot, &mut v, &[Event::off(0, 0, 60)], 100).is_empty());
    }

    /// Mono: a note that starts on the key of a voice whose note has just ended sounds in a voice of its own, and the
    /// old voice's fade passing leaves it alone: its end still ends it (back-to-back pattern clips that cut a looped
    /// sample's note short at each clip's end).
    // spec: audio/note-rules.md "Mono"
    #[test]
    fn mono_a_released_voice_on_the_same_key_leaves_the_next_note_alone() {
        let r = [Rules { mono: true, ring: f64::INFINITY, tail: 480.0, ..sampler() }];
        let mut v = vec![Voices::default()];
        tick(&r, &mut v, &[Event::on(0, 0, 57, 0.8)], 1000);
        let out = tick(&r, &mut v, &[Event::off(0, 0, 57), Event::on(0, 0, 57, 0.8)], 1000);
        assert_eq!(out, vec![(0, 0, Note::Off(57)), (0, 0, on(57))]);
        // The first voice's fade has passed.
        tick(&r, &mut v, &[], 1000);
        assert_eq!(v[0].pool.len(), 1);
        assert_eq!(tick(&r, &mut v, &[Event::off(0, 0, 57)], 1000), vec![(0, 0, Note::Off(57))]);
        assert!(v[0].pool.is_empty() && v[0].list.is_empty() && v[0].sounding.is_none());
    }

    #[test]
    fn porta_glides_each_new_note_from_the_last_key() {
        let r = [Rules { porta: true, glide: 2, ..Rules::default() }];
        let mut v = vec![Voices::default()];
        tick(&r, &mut v, &[Event::on(0, 0, 60, 0.8), Event::off(0, 1, 60)], 100);
        let out = tick(&r, &mut v, &[Event::on(0, 50, 72, 0.8)], 100);
        // The first step falls on the note's own tick: the voice never sounds at the old pitch.
        assert_eq!(out, vec![(0, 50, on(72)), (0, 50, Note::Tune(72, -8.0))]);
        let rest: Vec<Note> = (0..3).flat_map(|_| tick(&r, &mut v, &[], 100)).map(|e| e.2).collect();
        assert_eq!(rest, vec![Note::Tune(72, -4.0), Note::Tune(72, 0.0)]);
        // A slide time that gives no ticks does not glide.
        let r = [Rules { porta: true, glide: 0, ..Rules::default() }];
        let mut v = vec![Voices::default()];
        tick(&r, &mut v, &[Event::on(0, 0, 60, 0.8)], 100);
        assert_eq!(tick(&r, &mut v, &[Event::on(0, 0, 72, 0.8)], 100).len(), 1);
    }

    // spec: audio/note-rules.md "Portamento", "A note's own portamento switch"
    /// A note's own switch inverts the channel's portamento: a switched note glides on a channel without portamento,
    /// and does not on a channel with it.
    #[test]
    fn a_notes_own_switch_inverts_portamento() {
        let switched = |key: u8| Event { porta: true, ..Event::on(0, 0, key, 0.8) };
        let r = [Rules { porta: false, glide: 2, ..Rules::default() }];
        let mut v = vec![Voices::default()];
        tick(&r, &mut v, &[Event::on(0, 0, 60, 0.8), Event::off(0, 1, 60)], 100);
        assert_eq!(tick(&r, &mut v, &[switched(72)], 100), vec![(0, 0, on(72)), (0, 0, Note::Tune(72, -8.0))]);
        let r = [Rules { porta: true, glide: 2, ..Rules::default() }];
        let mut v = vec![Voices::default()];
        tick(&r, &mut v, &[Event::on(0, 0, 60, 0.8), Event::off(0, 1, 60)], 100);
        assert_eq!(tick(&r, &mut v, &[switched(72)], 100), vec![(0, 0, on(72))]);
    }

    // spec: audio/song-structure.md "Slide notes"; "Slide notes and these rules" here
    #[test]
    fn slide_notes_glide_every_voice_from_the_highest_held_one() {
        // A chord of 60 and 64; a slide note to 67 of 2 ticks moves both by 67 - 64 = 3 semitones in three steps,
        // the first on its own tick. It makes no note of its own.
        let r = [sampler()];
        let mut v = vec![Voices::default()];
        tick(&r, &mut v, &[Event::on(0, 0, 60, 0.8), Event::on(0, 0, 64, 0.8)], 100);
        let out = tick(&r, &mut v, &[Event::slide(0, 20, 67, 2)], 100);
        assert_eq!(out, vec![(0, 20, Note::Tune(60, 1.0)), (0, 20, Note::Tune(64, 1.0))]);
        let rest: Vec<Note> = (0..3).flat_map(|_| tick(&r, &mut v, &[], 100)).map(|e| e.2).collect();
        assert_eq!(rest, vec![Note::Tune(60, 2.0), Note::Tune(64, 2.0), Note::Tune(60, 3.0), Note::Tune(64, 3.0)]);
        // A new slide is measured from where the voices are: 64 sounds at 67 now, back to 64 in one step.
        let out = tick(&r, &mut v, &[Event::slide(0, 0, 64, 0)], 100);
        assert_eq!(out, vec![(0, 0, Note::Tune(60, 0.0)), (0, 0, Note::Tune(64, 0.0))]);
        // The highest HELD voice decides: with 64's note ended, 60 is the one measured, and the released 64 moves too.
        tick(&r, &mut v, &[Event::off(0, 0, 64)], 100);
        let out = tick(&r, &mut v, &[Event::slide(0, 0, 62, 0)], 100);
        assert_eq!(out, vec![(0, 0, Note::Tune(60, 2.0)), (0, 0, Note::Tune(64, 2.0))]);
        // The slide's key is the last level: a portamento note glides from it.
        assert_eq!(v[0].last, Some(62.0));
        // Without a held voice nothing moves.
        tick(&r, &mut v, &[Event::off(0, 0, 60)], 100);
        assert!(tick(&r, &mut v, &[Event::slide(0, 0, 72, 0)], 100).is_empty());
        // A one-shot voice counts as held while it sounds, after its note's end too.
        let r = [Rules { one_shot: true, ring: 1e6, ..sampler() }];
        let mut v = vec![Voices::default()];
        tick(&r, &mut v, &[Event::on(0, 0, 60, 0.8)], 100);
        tick(&r, &mut v, &[Event::off(0, 0, 60)], 100);
        assert_eq!(tick(&r, &mut v, &[Event::slide(0, 0, 65, 0)], 100), vec![(0, 0, Note::Tune(60, 5.0))]);
    }
}
