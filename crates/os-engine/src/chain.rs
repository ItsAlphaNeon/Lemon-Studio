// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! A container's serial chain (Lemon Studio's Patcher, slice 1): one mixer slot effect that runs other slot effects in
//! order, each with its own wet and dry levels. A member's dry part waits for the delay that member reports, so the
//! two parts meet in time, and the chain reports the sum of its members' delays to the mixer's delay compensation.
//! Tempo, transport and reset reach every member; the slot's sidechain input reaches the first member only.
//! Automation and controller links reach a member as the container slot's parameters: the member's id and an item
//! packed in the parameter index (`os_model::target::member_item`), the item a parameter of the member's effect or the
//! member's wet or dry level.
// spec: plugins/patcher/README.md §3.2, §4 slice 1, §4.2; audio/mixer.md "An effect slot"; audio/delay-compensation.md

use std::collections::VecDeque;

use os_model::target::{member_of, MEMBER_DRY, MEMBER_WET};

use crate::instrument::{HostedEffect, SharedEffect, TransportInfo};

/// One effect of the chain: its id in the container, its levels (the output is `wet` times the effect's output plus
/// `dry` times its input) and the line its dry part waits in.
pub struct Member {
    pub id: u32,
    pub fx: SharedEffect,
    pub wet: f32,
    pub dry: f32,
    /// The delay the effect reported after its last block, and the dry part's line as long as that.
    delay: u32,
    line: [VecDeque<f32>; 2],
}

impl Member {
    pub fn new(id: u32, fx: SharedEffect, wet: f32, dry: f32) -> Member {
        Member { id, fx, wet, dry, delay: 0, line: [VecDeque::new(), VecDeque::new()] }
    }

    /// Whether the member's input reaches its output beside the effect.
    fn blends(&self) -> bool {
        self.dry != 0.0 || self.wet != 1.0
    }

    /// The dry line takes `delay` samples, empty (a new delay, or a reset).
    fn set_delay(&mut self, delay: u32) {
        self.delay = delay;
        for l in &mut self.line {
            l.clear();
            l.resize(delay as usize, 0.0);
        }
    }
}

/// The chain: its members in order, and the buffers one block's dry parts are kept in.
#[derive(Default)]
pub struct Chain {
    pub members: Vec<Member>,
    dry: [Vec<f32>; 2],
}

impl Chain {
    pub fn new(members: Vec<Member>) -> Chain {
        Chain { members, dry: [Vec::new(), Vec::new()] }
    }

    /// Member `i`'s levels (a change in the container's list).
    pub fn set_levels(&mut self, i: usize, wet: f32, dry: f32) {
        if let Some(m) = self.members.get_mut(i) {
            (m.wet, m.dry) = (wet, dry);
        }
    }

    /// The member and item that parameter `index` of the container names.
    fn item(&self, index: u32) -> Option<(&Member, u16)> {
        let (id, item) = member_of(u16::try_from(index).ok()?)?;
        Some((self.members.iter().find(|m| m.id == id)?, item))
    }

    fn run(&mut self, rate: u32, left: &mut [f32], right: &mut [f32], mut key: Option<(&[f32], &[f32])>) {
        let n = left.len().min(right.len());
        for m in &mut self.members {
            let blend = m.blends();
            if blend {
                for (d, src) in self.dry.iter_mut().zip([&left[..n], &right[..n]]) {
                    d.clear();
                    d.extend_from_slice(src);
                }
            }
            // A member still busy elsewhere passes the block through (as a busy slot does).
            let delay = match m.fx.try_lock() {
                Ok(mut fx) => {
                    match key.take() {
                        Some(k) => fx.process_fx_keyed(rate, &mut left[..n], &mut right[..n], k),
                        None => fx.process_fx(rate, &mut left[..n], &mut right[..n]),
                    }
                    fx.reported_delay_fx()
                }
                Err(_) => m.delay,
            };
            if delay != m.delay {
                m.set_delay(delay);
            }
            if !blend {
                continue;
            }
            for (c, out) in [&mut left[..n], &mut right[..n]].into_iter().enumerate() {
                let line = &mut m.line[c];
                for (o, &x) in out.iter_mut().zip(&self.dry[c]) {
                    let late = if m.delay == 0 {
                        x
                    } else {
                        line.push_back(x);
                        line.pop_front().unwrap_or(0.0)
                    };
                    *o = *o * m.wet + late * m.dry;
                }
            }
        }
    }
}

impl HostedEffect for Chain {
    fn process_fx(&mut self, rate: u32, left: &mut [f32], right: &mut [f32]) {
        self.run(rate, left, right, None);
    }

    fn process_fx_keyed(&mut self, rate: u32, left: &mut [f32], right: &mut [f32], key: (&[f32], &[f32])) {
        self.run(rate, left, right, Some(key));
    }

    fn reset_fx(&mut self) {
        for m in &mut self.members {
            if let Ok(mut fx) = m.fx.try_lock() {
                fx.reset_fx();
            }
            let d = m.delay;
            m.set_delay(d);
        }
    }

    fn transport_fx(&mut self, info: &TransportInfo) {
        for m in &self.members {
            if let Ok(mut fx) = m.fx.try_lock() {
                fx.transport_fx(info);
            }
        }
    }

    /// The chain's delay: its members' reported delays added up.
    fn reported_delay_fx(&self) -> u32 {
        self.members.iter().map(|m| m.delay).sum()
    }

    /// A member's parameter goes to its effect (a busy effect misses it, as a busy slot does); its wet and dry levels
    /// are set here, for the whole of the next block.
    fn param_fx(&mut self, offset: u32, index: u32, value: f64) {
        let Some((id, item)) = u16::try_from(index).ok().and_then(member_of) else { return };
        let Some(m) = self.members.iter_mut().find(|m| m.id == id) else { return };
        let level = value.clamp(0.0, 1.0) as f32;
        match item {
            MEMBER_WET => m.wet = level,
            MEMBER_DRY => m.dry = level,
            k => {
                if let Ok(mut fx) = m.fx.try_lock() {
                    fx.param_fx(offset, u32::from(k), value);
                }
            }
        }
    }

    /// Each member's effect takes its parameters as it would in a slot; the levels take events.
    fn tick_params_at(&self, index: u32) -> bool {
        match self.item(index) {
            Some((m, k)) if k < MEMBER_WET => m.fx.try_lock().is_ok_and(|fx| fx.tick_params_fx()),
            _ => false,
        }
    }

    fn param_range_fx(&self, index: usize) -> Option<(f64, f64)> {
        let (m, k) = self.item(u32::try_from(index).ok()?)?;
        if k >= MEMBER_WET {
            return Some((0.0, 1.0));
        }
        let fx = m.fx.try_lock().ok()?;
        fx.param_range_fx(usize::from(k))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// A test effect: a gain, put out `late` samples late. Its parameter 0 sets the gain (0..1 to 0..4); it takes values
    /// per tick when `ticks`.
    struct Late {
        gain: f32,
        late: usize,
        line: [VecDeque<f32>; 2],
        ticks: bool,
        got: Vec<(u32, u32, f64)>,
    }

    impl Late {
        fn shared(gain: f32, late: usize) -> SharedEffect {
            Arc::new(Mutex::new(Late::new(gain, late)))
        }

        fn new(gain: f32, late: usize) -> Late {
            Late { gain, late, line: [VecDeque::from(vec![0.0; late]), VecDeque::from(vec![0.0; late])], ticks: false, got: Vec::new() }
        }
    }

    impl HostedEffect for Late {
        fn param_fx(&mut self, offset: u32, index: u32, value: f64) {
            self.got.push((offset, index, value));
            if index == 0 {
                self.gain = value as f32 * 4.0;
            }
        }
        fn tick_params_fx(&self) -> bool {
            self.ticks
        }
        fn param_range_fx(&self, index: usize) -> Option<(f64, f64)> {
            (index == 0).then_some((0.0, 4.0))
        }
        fn process_fx(&mut self, _rate: u32, left: &mut [f32], right: &mut [f32]) {
            for (c, buf) in [left, right].into_iter().enumerate() {
                for x in buf.iter_mut() {
                    self.line[c].push_back(*x * self.gain);
                    *x = self.line[c].pop_front().unwrap();
                }
            }
        }
        fn reported_delay_fx(&self) -> u32 {
            self.late as u32
        }
    }

    fn impulse(chain: &mut Chain, len: usize) -> Vec<f32> {
        let mut out = Vec::new();
        for b in 0..4 {
            let mut l = vec![0.0; len];
            let mut r = vec![0.0; len];
            if b == 1 {
                l[0] = 1.0;
                r[0] = 1.0;
            }
            chain.process_fx(48000, &mut l, &mut r);
            assert_eq!(l, r);
            out.extend(l);
        }
        out
    }

    /// Members run in order; the chain's delay is the sum of theirs; a member's dry part meets its wet part in time.
    #[test]
    fn a_serial_chain_with_wet_and_dry() {
        let mut c = Chain::new(vec![Member::new(1, Late::shared(0.5, 3), 1.0, 0.0), Member::new(2, Late::shared(2.0, 5), 0.25, 0.75)]);
        // The first block finds out the delays.
        impulse(&mut c, 16);
        assert_eq!(c.reported_delay_fx(), 8);
        let out = impulse(&mut c, 16);
        // In at sample 16: the first member puts out 0.5 at 19; the second takes it 5 later at 24, times 2 times wet
        // 0.25 plus its dry 0.5 times 0.75, delayed to the same 24.
        let at: Vec<(usize, f32)> = out.iter().copied().enumerate().filter(|&(_, v)| v != 0.0).collect();
        assert_eq!(at, vec![(24, 0.5 * 2.0 * 0.25 + 0.5 * 0.75)]);
        c.set_levels(1, 0.0, 1.0);
        let out = impulse(&mut c, 16);
        assert_eq!(out.iter().position(|&v| v != 0.0), Some(24), "dry only: still delayed to match");
        // A busy member passes the block through.
        let held = c.members[0].fx.clone();
        let _guard = held.lock().unwrap();
        let mut l = vec![1.0; 4];
        let mut r = vec![1.0; 4];
        c.members[1].wet = 1.0;
        c.members[1].dry = 0.0;
        c.reset_fx();
        c.process_fx(48000, &mut l, &mut r);
        assert_eq!(c.reported_delay_fx(), 8);
    }

    /// The container's parameters reach its members by id (wherever they are in the chain): a parameter to the effect,
    /// the levels to the member; an id or item the chain has not goes nowhere.
    #[test]
    fn members_parameters_and_levels() {
        use os_model::target::member_item;
        let ticked = Late { ticks: true, ..Late::new(1.0, 0) };
        let (a, b) = (Arc::new(Mutex::new(Late::new(1.0, 0))), Arc::new(Mutex::new(ticked)));
        let mut c = Chain::new(vec![Member::new(4, b.clone(), 1.0, 0.0), Member::new(2, a.clone(), 1.0, 0.0)]);
        let k = |id: u32, item: u16| u32::from(member_item(id, item).unwrap());
        c.param_fx(5, k(2, 0), 0.5);
        c.param_fx(0, k(2, 9), 0.25);
        assert_eq!(a.lock().unwrap().got, vec![(5, 0, 0.5), (0, 9, 0.25)]);
        assert_eq!(a.lock().unwrap().gain, 2.0);
        assert!(b.lock().unwrap().got.is_empty());
        c.param_fx(0, k(4, MEMBER_WET), 0.5);
        c.param_fx(0, k(4, MEMBER_DRY), 1.5);
        assert_eq!((c.members[0].wet, c.members[0].dry), (0.5, 1.0), "levels clamp to 0..1");
        // Not a member: a plain index, an id not in the chain.
        c.param_fx(0, 3, 1.0);
        c.param_fx(0, k(3, 0), 1.0);
        assert_eq!(a.lock().unwrap().got.len(), 2);
        assert!(b.lock().unwrap().got.is_empty());
        // Per tick as the member's effect takes them; the levels and other indexes as events.
        assert!(c.tick_params_at(k(4, 0)));
        assert!(!c.tick_params_at(k(2, 0)));
        assert!(!c.tick_params_at(k(4, MEMBER_WET)));
        assert_eq!(c.param_range_fx(k(2, 0) as usize), Some((0.0, 4.0)));
        assert_eq!(c.param_range_fx(k(2, MEMBER_DRY) as usize), Some((0.0, 1.0)));
        assert_eq!(c.param_range_fx(k(2, 1) as usize), None);
        // The levels sound: member 4 at wet 0.5, dry 1 over member 2 at gain 2.
        let (mut l, mut r) = (vec![1.0; 4], vec![1.0; 4]);
        c.process_fx(48000, &mut l, &mut r);
        assert_eq!(l, vec![(0.5 + 1.0) * 2.0; 4]);
    }
}
