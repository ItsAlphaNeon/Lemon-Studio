// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The two effect slots (`pages/fx-slots.md`).

mod chorus;
mod delay;
mod phaser;
mod plate;
mod rotary;
mod shared;

use crate::params::id;
use crate::settings::Settings;
use chorus::Chorus;
use delay::Delay;
use phaser::Phaser;
use plate::Plate;
use rotary::Rotary;

/// The distance between a parameter of FX1 and the same parameter of FX2.
const SLOT_STRIDE: usize = id::FX2_MODULE - id::FX1_MODULE;

const NONE: i8 = -1;

struct Slot {
    active_id: usize,
    offset: usize,
    chorus: Box<Chorus>,
    phaser: Box<Phaser>,
    plate: Box<Plate>,
    delay: Box<Delay>,
    rotary: Box<Rotary>,
    /// The module of the previous span, NONE when the slot was off.
    running: i8,
    /// The module whose set-up ran at the last block start.
    current: i8,
    has_run: [bool; 5],
}

impl Slot {
    fn new(fs: f32, active_id: usize, offset: usize) -> Slot {
        Slot { active_id, offset, chorus: Box::new(Chorus::new(fs)), phaser: Box::new(Phaser::new(fs)), plate: Box::new(Plate::new(fs)), delay: Box::new(Delay::new(fs)), rotary: Box::new(Rotary::new(fs)), running: NONE, current: NONE, has_run: [false; 5] }
    }

    fn reactivate(&mut self, m: i8) {
        match m {
            0 => self.chorus.reactivate(),
            1 => self.phaser.reactivate(),
            2 => self.plate.reactivate(),
            3 => self.delay.reactivate(),
            4 => self.rotary.reactivate(),
            _ => {}
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn setup(&mut self, m: i8, raw: &[f32], tempo: f64, beats: f64, rows: &[f32; 448], l: &[f32], r: &[f32]) {
        match m {
            0 => self.chorus.setup(raw, self.offset),
            1 => self.phaser.setup(raw, self.offset, l, r, tempo, beats),
            2 => self.plate.setup(raw, self.offset),
            3 => self.delay.setup(raw, self.offset, tempo),
            4 => self.rotary.setup(raw, self.offset, rows),
            _ => {}
        }
    }

    #[inline]
    fn chunk(&mut self, m: i8, l: &mut [f32; 16], r: &mut [f32; 16]) {
        match m {
            0 => self.chorus.chunk(l, r),
            1 => self.phaser.chunk(l, r),
            2 => self.plate.chunk(l, r),
            3 => self.delay.chunk(l, r),
            4 => self.rotary.chunk(l, r),
            _ => {}
        }
    }

    #[inline]
    fn wrap_up(&mut self, m: i8) {
        match m {
            1 => self.phaser.wrap_up(),
            2 => self.plate.wrap_up(),
            _ => {}
        }
    }

    /// The span in place; true when the slot is on.
    #[allow(clippy::too_many_arguments)]
    fn process(&mut self, raw: &[f32], t: u64, ns: usize, tempo: f64, beats: f64, rows: &[f32; 448], l: &mut [f32], r: &mut [f32]) -> bool {
        let wanted = if raw[self.active_id] >= 0.5 { raw[id::FX1_MODULE + self.offset].round().clamp(0.0, 4.0) as i8 } else { NONE };
        if wanted != self.running {
            if wanted != NONE {
                if self.has_run[wanted as usize] {
                    self.reactivate(wanted);
                }
                self.has_run[wanted as usize] = true;
            }
            self.running = wanted;
        }
        if t % 64 == 0 {
            self.current = wanted;
        }
        if wanted == NONE {
            return false;
        }
        for k in 0..ns / 16 {
            let at = t + (16 * k) as u64;
            if at % 64 == 0 {
                self.setup(wanted, raw, tempo, beats, rows, &l[16 * k..], &r[16 * k..]);
            }
            if self.current == wanted {
                let lc: &mut [f32; 16] = (&mut l[16 * k..16 * k + 16]).try_into().unwrap();
                let rc: &mut [f32; 16] = (&mut r[16 * k..16 * k + 16]).try_into().unwrap();
                self.chunk(wanted, lc, rc);
                if at % 64 == 48 {
                    self.wrap_up(wanted);
                }
            }
        }
        true
    }
}

pub struct Effects {
    slots: [Slot; 2],
}

impl Effects {
    pub fn new(fs: f32) -> Effects {
        Effects { slots: [Slot::new(fs, id::MAIN_ACTIVE_FX1, 0), Slot::new(fs, id::MAIN_ACTIVE_FX2, SLOT_STRIDE)] }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn process(&mut self, s: &Settings, t: u64, ns: usize, tempo: f64, beats: f64, rows: &[f32; 448], l: &mut [f32], r: &mut [f32]) {
        let raw = &s.raw[..];
        let mut ran = false;
        for slot in &mut self.slots {
            ran |= slot.process(raw, t, ns, tempo, beats, rows, l, r);
        }
        if ran {
            for x in l.iter_mut().chain(r.iter_mut()) {
                *x = shared::flush(*x);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::DenormalGuard;
    use crate::params::{COUNT, PARAMS};

    const ROWS: [f32; 448] = [0.0; 448];

    fn settings(extra: &[(usize, f32)]) -> Settings {
        let mut v: [f32; COUNT] = core::array::from_fn(|i| PARAMS[i].default);
        for (k, x) in extra {
            v[*k] = *x;
        }
        Settings::read(&v)
    }

    fn on(module: f32) -> Vec<(usize, f32)> {
        vec![(id::MAIN_ACTIVE_FX1, 1.0), (id::FX1_MODULE, module)]
    }

    /// Every module and type, by its settings.
    fn all_cases() -> Vec<Vec<(usize, f32)>> {
        let with = |m: f32, k: usize, v: f32| {
            let mut a = on(m);
            a.push((k, v));
            a
        };
        vec![
            with(0.0, id::CHRS1_TYPE, 0.0),
            with(0.0, id::CHRS1_TYPE, 1.0),
            with(0.0, id::CHRS1_TYPE, 2.0),
            with(1.0, id::PHASE1_TYPE, 0.0),
            with(1.0, id::PHASE1_TYPE, 1.0),
            on(2.0),
            on(3.0),
            on(4.0),
        ]
    }

    /// A block of noise.
    fn noise(n: &mut u32) -> ([f32; 64], [f32; 64]) {
        let mut next = || {
            *n = n.wrapping_mul(1664525).wrapping_add(1013904223);
            ((*n >> 8) as f32 / 8388608.0 - 1.0) * 0.3
        };
        (core::array::from_fn(|_| next()), core::array::from_fn(|_| next()))
    }

    #[test]
    fn slots_have_one_stride() {
        assert_eq!(id::CHRS2_TYPE - id::CHRS1_TYPE, SLOT_STRIDE);
        assert_eq!(id::PHASE2_CENTER - id::PHASE1_CENTER, SLOT_STRIDE);
        assert_eq!(id::PLATE2_WET - id::PLATE1_WET, SLOT_STRIDE);
        assert_eq!(id::DELAY2_WOW - id::DELAY1_WOW, SLOT_STRIDE);
        assert_eq!(id::RTARY2_CONTROLLER - id::RTARY1_CONTROLLER, SLOT_STRIDE);
    }

    /// A block rendered in several spans gives the samples of the whole block.
    #[test]
    fn spans_do_not_change_the_result() {
        let _g = DenormalGuard::new();
        for case in all_cases() {
            let s = settings(&case);
            let mut whole = Effects::new(48000.0);
            let mut parts = Effects::new(48000.0);
            let mut n = 1u32;
            let mut t = 0u64;
            for b in 0..200 {
                let (l, r) = noise(&mut n);
                let (mut al, mut ar) = (l, r);
                whole.process(&s, t, 64, 120.0, 0.0, &ROWS, &mut al, &mut ar);
                let (mut bl, mut br) = (l, r);
                let cut = [16usize, 32, 48][b % 3];
                let (l0, l1) = bl.split_at_mut(cut);
                let (r0, r1) = br.split_at_mut(cut);
                parts.process(&s, t, cut, 120.0, 0.0, &ROWS, l0, r0);
                parts.process(&s, t + cut as u64, 64 - cut, 120.0, 0.0, &ROWS, l1, r1);
                assert_eq!((al, ar), (bl, br), "{case:?} block {b}");
                t += 64;
            }
        }
    }

    /// The silent shortcuts give what the full path gives, in the silence and after it.
    #[test]
    fn silent_shortcuts_are_exact() {
        let _g = DenormalGuard::new();
        for case in all_cases() {
            let m = case[1].1 as usize;
            if m == 1 || m == 4 {
                continue;
            }
            let s = settings(&case);
            let mut short = Effects::new(48000.0);
            let mut full = Effects::new(48000.0);
            let mut n = 7u32;
            let mut t = 0u64;
            let (mut al, mut ar) = ([0.0f32; 16], [0.0f32; 16]);
            short.process(&s, 0, 16, 120.0, 0.0, &ROWS, &mut al, &mut ar);
            full.process(&s, 0, 16, 120.0, 0.0, &ROWS, &mut al, &mut ar);
            let slot = &mut full.slots[0];
            slot.chorus.force_full();
            slot.plate.force_full();
            slot.delay.force_full();
            let (mut al, mut ar) = ([0.0f32; 48], [0.0f32; 48]);
            short.process(&s, 16, 48, 120.0, 0.0, &ROWS, &mut al, &mut ar);
            full.process(&s, 16, 48, 120.0, 0.0, &ROWS, &mut al, &mut ar);
            t += 64;
            for b in 0..300 {
                let (mut l, mut r) = if b < 150 { ([0.0; 64], [0.0; 64]) } else { noise(&mut n) };
                let (mut l2, mut r2) = (l, r);
                short.process(&s, t, 64, 120.0, 0.0, &ROWS, &mut l, &mut r);
                full.process(&s, t, 64, 120.0, 0.0, &ROWS, &mut l2, &mut r2);
                assert_eq!((l, r), (l2, r2), "{case:?} block {b}");
                t += 64;
            }
        }
    }

    #[test]
    fn an_off_slot_leaves_the_bus_alone_and_a_running_one_is_cleaned() {
        let _g = DenormalGuard::new();
        let mut fx = Effects::new(48000.0);
        let off = settings(&[]);
        let mut l = [-0.0f32; 16];
        let mut r = [f32::NAN; 16];
        fx.process(&off, 0, 16, 120.0, 0.0, &ROWS, &mut l, &mut r);
        assert!(l[0].is_sign_negative() && r[0].is_nan());
        // a slot started inside a block passes the rest of the block, then the clean-up still applies
        let s = settings(&on(3.0));
        let mut l = [0.25f32; 16];
        let mut r = [f32::INFINITY; 16];
        fx.process(&s, 16, 16, 120.0, 0.0, &ROWS, &mut l, &mut r);
        assert_eq!(l, [0.25f32; 16]);
        assert_eq!(r, [0.0f32; 16]);
    }

    #[test]
    fn reactivation_clears_the_delay_line() {
        let _g = DenormalGuard::new();
        let (a, b) = (settings(&on(3.0)), settings(&[]));
        let mut fx = Effects::new(48000.0);
        let mut n = 3u32;
        let mut t = 0;
        for _ in 0..50 {
            let (mut l, mut r) = noise(&mut n);
            fx.process(&a, t, 64, 120.0, 0.0, &ROWS, &mut l, &mut r);
            t += 64;
        }
        let (mut l, mut r) = ([0.0f32; 64], [0.0f32; 64]);
        fx.process(&b, t, 64, 120.0, 0.0, &ROWS, &mut l, &mut r);
        t += 64;
        for _ in 0..500 {
            let (mut l, mut r) = ([0.0f32; 64], [0.0f32; 64]);
            fx.process(&a, t, 64, 120.0, 0.0, &ROWS, &mut l, &mut r);
            assert_eq!((l, r), ([0.0f32; 64], [0.0f32; 64]));
            t += 64;
        }
    }

    /// A reactivated Rotary is a new one.
    #[test]
    fn a_reactivated_rotary_is_new() {
        let _g = DenormalGuard::new();
        let (a, b) = (settings(&on(4.0)), settings(&[]));
        let mut used = Effects::new(48000.0);
        let mut new = Effects::new(48000.0);
        let mut n = 11u32;
        let mut t = 0;
        for _ in 0..50 {
            let (mut l, mut r) = noise(&mut n);
            used.process(&a, t, 64, 120.0, 0.0, &ROWS, &mut l, &mut r);
            t += 64;
        }
        let (mut l, mut r) = ([0.0f32; 64], [0.0f32; 64]);
        used.process(&b, t, 64, 120.0, 0.0, &ROWS, &mut l, &mut r);
        t += 64;
        for i in 0..200u64 {
            let (mut l, mut r) = noise(&mut n);
            let (mut l2, mut r2) = (l, r);
            used.process(&a, t, 64, 120.0, 0.0, &ROWS, &mut l, &mut r);
            new.process(&a, i * 64, 64, 120.0, 0.0, &ROWS, &mut l2, &mut r2);
            assert_eq!((l, r), (l2, r2));
            t += 64;
        }
    }
}
