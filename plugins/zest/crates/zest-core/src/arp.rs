// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The arpeggiator and its clock (`pages/arp.md`).

use crate::params::id;
use crate::settings::Settings;

const QUEUE: usize = 256;
const RESTART: [i32; 13] = [0, 4, 5, 6, 7, 8, 9, 10, 12, 14, 16, 24, 32];
const TIME_BASE: [f64; 4] = [32.0, 16.0, 8.0, 4.0];

#[derive(Clone, Copy, Debug, PartialEq)]
struct StepState {
    oct: i32,
    step: i32,
    count: i32,
    dir: i32,
    flag: bool,
}

const ST0: StepState = StepState { oct: 0, step: 0, count: 0, dir: -1, flag: false };

/// What the arpeggiator plays, into the note handling.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Play {
    On(u8, i32),
    Off(u8),
    /// The LFO restart flags are armed.
    ArmLfos,
}

#[derive(Clone, Copy)]
enum Queued {
    Key(u8, i32),
    Pedal(i32),
}

pub struct Arp {
    chord: [(u8, i32); 128],
    n: usize,
    clock_keys: [i32; 128],
    clock_n: i32,
    queue: [Queued; QUEUE],
    q_len: usize,
    pedal: i32,
    pedal_held: [i32; 128],
    cur_key: i32,
    cur_vel: i32,
    st: StepState,
    changed: bool,
    restart: bool,
    extra: bool,
    c: f32,
    rnd: u32,
    pos: f64,
    ipos: i64,
    swing_cur: f32,
    last_tick: f32,
    base: f64,
    rel: f64,
    prev_rel: f64,
    prev_mult: f32,
    strict_run: bool,
    chord_start: bool,
    /// Something is held or sounding (for switching the arpeggiator off).
    dirty: bool,
}

/// snap(m): a value within 0.0051 of a third of 100 is that third.
fn snap(m: f32) -> f32 {
    let a = ((m + 0.0051) * 0.01 * 3.0).floor();
    let b = (m * 0.01 * 3.0).floor();
    let d = ((m - 0.0051) * 0.01 * 3.0).floor();
    if b > d {
        b / 3.0
    } else if b < a {
        a / 3.0
    } else {
        m * 0.01
    }
}

fn warp(p: f64, s: f32) -> f32 {
    let x = (p - 2.0 * (p * 0.5).floor()) as f32 * 0.5;
    let e = -(2.0f32.ln()) / (s * 0.25 + 0.5).ln();
    2.0 * x.powf(e)
}

impl Arp {
    pub fn new() -> Box<Arp> {
        Box::new(Arp {
            chord: [(0, 0); 128],
            n: 0,
            clock_keys: [0; 128],
            clock_n: 0,
            queue: [Queued::Pedal(0); QUEUE],
            q_len: 0,
            pedal: 0,
            pedal_held: [0; 128],
            cur_key: 0,
            cur_vel: 0,
            st: ST0,
            changed: false,
            restart: true,
            extra: false,
            c: 0.0,
            rnd: 1,
            pos: 0.0,
            ipos: 0,
            swing_cur: 0.0,
            last_tick: 0.0,
            base: 0.0,
            rel: 0.0,
            prev_rel: 0.0,
            prev_mult: 0.0,
            strict_run: false,
            chord_start: false,
            dirty: false,
        })
    }

    /// A note-on (a note-off is v = 0), when the event is taken.
    pub fn key_event(&mut self, key: u8, v: i32) {
        let k = key as usize;
        let was = self.clock_n;
        if v > 0 {
            self.clock_keys[k] += 1;
            self.clock_n += 1;
        } else if self.clock_keys[k] > 0 {
            self.clock_keys[k] -= 1;
            self.clock_n -= 1;
        }
        if was == 0 && self.clock_n > 0 {
            self.chord_start = true;
        }
        self.push(Queued::Key(key, v));
    }

    pub fn pedal_event(&mut self, m: i32) {
        self.push(Queued::Pedal(m));
    }

    fn push(&mut self, q: Queued) {
        if self.q_len < QUEUE {
            self.queue[self.q_len] = q;
            self.q_len += 1;
            self.dirty = true;
        }
    }

    /// cc 120, 123, panic, and the arpeggiator switched off.
    pub fn clear(&mut self, play: &mut impl FnMut(Play)) {
        self.n = 0;
        self.clock_keys = [0; 128];
        self.clock_n = 0;
        self.q_len = 0;
        self.pedal_held = [0; 128];
        self.st = ST0;
        self.restart = true;
        self.c = 0.0;
        self.dirty = false;
        if self.cur_vel > 0 {
            self.cur_vel = 0;
            play(Play::Off(self.cur_key as u8));
        }
    }

    /// With the arpeggiator off: the clearing, once, if anything is held or sounding.
    pub fn off(&mut self, play: &mut impl FnMut(Play)) {
        if self.dirty || self.cur_vel > 0 {
            self.clear(play);
        }
    }

    /// Runs at the start of each block, after the block's events were taken.
    pub fn block(&mut self, s: &Settings, beats: f64, playing: bool, bpm: f64, fs: f32, play: &mut impl FnMut(Play)) {
        let r = &s.raw;
        let mult = r[id::CLK_MULTIPLY];
        let t = TIME_BASE[(r[id::CLK_TIMEBASE].round() as i32).clamp(0, 3) as usize];
        let strict = r[id::CLK_STRICT].round() as i32;
        // 1. base
        let mut go = self.chord_start;
        if !playing || strict != 1 {
            if go {
                self.base = beats;
            }
        } else {
            self.base = 0.0;
        }
        // 2.
        self.rel = beats - self.base;
        go = go && !playing;
        // 3. a jump
        if (self.prev_rel - self.rel).abs() > 0.05000000074505806 {
            go = true;
            self.chord_start = false;
        }
        // 4.
        if self.strict_run && mult != self.prev_mult {
            self.strict_run = false;
        }
        if go {
            self.strict_run = true;
        }
        // 5.
        let ratio = snap(mult);
        let sw = {
            let x = snap(r[id::CLK_SWING]);
            if x > 1.0 {
                1.0
            } else if x > 0.0 {
                x
            } else {
                0.0
            }
        };
        // 6.
        if go {
            self.swing_cur = sw;
            self.pos = t * ratio as f64 * self.rel * 0.25;
            self.ipos = self.pos.trunc() as i64;
            self.restart = true;
            self.last_tick = warp(self.pos, self.swing_cur) - 0.0001;
        }
        // 7. the queue
        for i in 0..self.q_len {
            match self.queue[i] {
                Queued::Key(key, v) => self.queued_key(key, v, play),
                Queued::Pedal(m) => {
                    self.pedal = m;
                    if m < 64 {
                        for k in 0..128 {
                            let n = core::mem::take(&mut self.pedal_held[k]);
                            for _ in 0..n {
                                self.chord_change(k as u8, 0, play);
                            }
                        }
                    }
                }
            }
        }
        self.q_len = 0;
        // 8. move
        if !self.strict_run {
            let x = (4.0 * t) as f32 * 4.0 * ratio;
            let mut d = x as f64 * bpm / (fs * 60.0) as f64 + self.pos;
            if d > 8640.0 {
                d -= 8640.0;
            }
            self.pos = d;
        } else {
            let tt = ratio as f64 * self.rel * t * 0.25;
            let mut d = (tt - tt.floor()) - (self.pos - self.pos.floor());
            if d < 0.0 {
                d = if d >= -0.2 { 0.0 } else { d + 1.0 };
            }
            self.pos += d;
        }
        // 9.
        let tr = self.pos.trunc() as i64;
        if tr != self.ipos {
            if tr % 2 == 0 {
                self.swing_cur = sw;
            }
            self.ipos = tr;
        }
        // 10. the tick
        let w = warp(self.pos, self.swing_cur);
        let f = w - self.last_tick;
        let f = f - f.floor();
        if self.last_tick.floor() != w.floor() || f >= 0.01 {
            let a = (self.pos.floor() + w as f64 - w.floor() as f64) as f32;
            self.tick(a, s, play);
            self.last_tick = w;
        }
        // 11.
        self.prev_rel = self.rel;
        self.prev_mult = mult;
        self.chord_start = false;
    }

    fn queued_key(&mut self, key: u8, v: i32, play: &mut impl FnMut(Play)) {
        if v > 0 && self.n == 0 {
            play(Play::ArmLfos);
        }
        if v == 0 && self.pedal >= 64 {
            self.pedal_held[key as usize] += 1;
            return;
        }
        self.chord_change(key, v, play);
    }

    fn chord_change(&mut self, key: u8, v: i32, play: &mut impl FnMut(Play)) {
        if self.n == 0 && v > 0 {
            self.restart = true;
        }
        if v > 0 {
            if self.n < 128 {
                self.chord[self.n] = (key, v);
                self.n += 1;
            }
        } else if let Some(i) = self.chord[..self.n].iter().position(|e| e.0 == key) {
            self.chord.copy_within(i + 1..self.n, i);
            self.n -= 1;
        }
        if self.n == 0 {
            self.cur_vel = 0;
            self.changed = true;
            play(Play::Off(self.cur_key as u8));
        }
    }

    fn tick(&mut self, a: f32, s: &Settings, play: &mut impl FnMut(Play)) {
        let raw = &s.raw;
        let r = self.restart;
        if r {
            self.c = 0.0;
        }
        let steps = (1i32 << (raw[id::ARP_MULTIPLY].round() as i32).clamp(0, 4)) as f32;
        let mut add = (a - a.floor()) - (self.c - self.c.floor());
        if add < 0.0 {
            add = add + 1.0;
        }
        self.c = self.c + add;
        if self.n == 0 {
            if self.c >= steps {
                self.c = self.c - (self.c / steps).floor() * steps;
            }
            return;
        }
        let len = steps + if self.extra { 1.0 } else { 0.0 };
        if r || self.c >= len {
            self.extra = false;
            let mut next = self.next_state(raw);
            if self.restart {
                next = ST0;
                if self.c > 0.5 {
                    self.extra = true;
                }
            }
            let (key, vel) = self.pick(&next, raw);
            if vel > 0 {
                play(Play::On(key as u8, vel));
            }
            if self.cur_vel > 0 {
                play(Play::Off(self.cur_key as u8));
            }
            self.cur_key = key;
            self.cur_vel = vel;
            self.st = next;
            self.c = self.c - (self.c / len).floor() * len;
            self.restart = false;
        }
    }

    fn next_state(&mut self, raw: &[f32]) -> StepState {
        let n = self.n as i32;
        let dir = (raw[id::ARP_DIRECTION].round() as i32).clamp(0, 5);
        let cycle = match dir {
            3 if n >= 3 => 2 * n - 2,
            4 => 2 * n,
            _ => n,
        };
        let o = (raw[id::ARP_OCTAVES].round() as i32).clamp(1, 4);
        let mut p = self.st;
        let count = if self.changed || p.flag { 0 } else { p.count };
        p.flag = false;
        p.count = count + 1;
        match (raw[id::ARP_ORDER].round() as i32).clamp(0, 3) {
            3 => {
                if p.step < 0 {
                    p.step = 0;
                }
                if self.cur_vel > 0 {
                    p.oct += 1;
                }
                if p.oct >= o {
                    p.oct = 0;
                    p.step += 1;
                    if p.step >= cycle {
                        p.step = 0;
                    }
                }
            }
            2 => {
                p.step += 1;
                if self.cur_vel > 0 {
                    p.oct += 1;
                }
                if p.oct >= o {
                    p.oct = 0;
                }
                if p.step >= cycle {
                    p.step = 0;
                }
            }
            1 => {
                p.step += 1;
                if p.step >= cycle {
                    let before = p.oct;
                    p.oct += p.dir;
                    if p.oct >= o || p.oct < 0 {
                        p.oct = before + p.dir - 2 * p.dir;
                        p.dir = -p.dir;
                    }
                    if o == 1 {
                        p.oct = 0;
                    }
                    p.step = 0;
                }
            }
            _ => {
                p.step += 1;
                if p.step >= cycle {
                    p.oct += 1;
                    if p.oct >= o {
                        p.oct = 0;
                    }
                    p.step = 0;
                }
            }
        }
        let rr = RESTART[(raw[id::ARP_RESTART].round() as i32).clamp(0, 12) as usize];
        if self.changed || (rr >= 1 && count + 1 >= rr) {
            p = ST0;
            self.changed = false;
            p.flag = true;
        }
        if n == 0 {
            p = StepState { step: -1, count: -1, ..ST0 };
        }
        p
    }

    /// The lowest entry: on equal keys the latest in the list.
    fn lowest(&self) -> (i32, i32) {
        let mut b = self.chord[0];
        for e in &self.chord[1..self.n] {
            if e.0 <= b.0 {
                b = *e;
            }
        }
        (b.0 as i32, b.1)
    }

    fn highest(&self) -> (i32, i32) {
        let mut b = self.chord[0];
        for e in &self.chord[1..self.n] {
            if e.0 >= b.0 {
                b = *e;
            }
        }
        (b.0 as i32, b.1)
    }

    fn pick(&mut self, s: &StepState, raw: &[f32]) -> (i32, i32) {
        let n = self.n;
        if n == 0 {
            return (0, 0);
        }
        let cur = self.cur_key - self.st.oct * 12;
        let chord_has = |k: i32, c: &[(u8, i32)]| c.iter().any(|e| e.0 as i32 == k);
        let (key, vel) = if !s.flag && s.step == self.st.step && self.cur_vel > 0 && chord_has(cur, &self.chord[..n]) {
            (cur, self.cur_vel)
        } else {
            let mut d = (raw[id::ARP_DIRECTION].round() as i32).clamp(0, 5);
            let nn = n as i32;
            let mut done = None;
            if d == 3 {
                d = if s.step < nn { 1 } else { 2 };
            } else if d == 4 {
                let e = if s.step == 0 {
                    Some(self.lowest())
                } else if s.step == nn {
                    Some(self.highest())
                } else {
                    None
                };
                if let Some(x) = e {
                    if x.1 > 0 {
                        done = Some(x);
                    }
                }
                d = if s.step <= nn { 1 } else { 2 };
            }
            match done {
                Some(x) => x,
                None => match d {
                    0 => {
                        let mut cand = None;
                        for i in 0..n.saturating_sub(1) {
                            if self.chord[i].0 as i32 == cur {
                                cand = Some(self.chord[i + 1]);
                            }
                        }
                        match cand {
                            Some(e) if e.1 > 0 && s.step != 0 => (e.0 as i32, e.1),
                            _ => (self.chord[0].0 as i32, self.chord[0].1),
                        }
                    }
                    1 => {
                        let mut cand: Option<(u8, i32)> = None;
                        for e in &self.chord[..n] {
                            if e.0 as i32 > cur && cand.is_none_or(|c| e.0 <= c.0) {
                                cand = Some(*e);
                            }
                        }
                        match cand {
                            Some(e) if e.1 > 0 && s.step != 0 => (e.0 as i32, e.1),
                            _ => self.lowest(),
                        }
                    }
                    2 => {
                        let mut cand: Option<(u8, i32)> = None;
                        for e in &self.chord[..n] {
                            if (e.0 as i32) < cur && cand.is_none_or(|c| e.0 >= c.0) {
                                cand = Some(*e);
                            }
                        }
                        match cand {
                            Some(e) if e.1 > 0 && s.step != 0 => (e.0 as i32, e.1),
                            _ => self.highest(),
                        }
                    }
                    _ => {
                        self.rnd = self.rnd.wrapping_mul(214013).wrapping_add(2531011);
                        let r = (self.rnd / 65536) & 32767;
                        let e = self.chord[r as usize % n];
                        (e.0 as i32, e.1)
                    }
                },
            }
        };
        (((key + s.oct * 12).min(127)), vel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snap_and_warp() {
        assert!((snap(33.33) - 1.0 / 3.0).abs() < 1e-6);
        assert!((snap(100.0) - 1.0).abs() < 1e-6);
        assert!((snap(50.0) - 0.5).abs() < 1e-6);
        assert!((warp(0.5, 0.0) - 0.5).abs() < 1e-5);
    }
}
