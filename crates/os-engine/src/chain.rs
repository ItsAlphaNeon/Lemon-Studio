// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! A container's serial chain (Lemon Studio's Patcher, slice 1): one mixer slot effect that runs other slot effects in
//! order, each with its own wet and dry levels. A member's dry part waits for the delay that member reports, so the
//! two parts meet in time, and the chain reports the sum of its members' delays to the mixer's delay compensation.
//! Tempo, transport and reset reach every member; the slot's sidechain input reaches the first member only.
// spec: plugins/patcher/README.md §3.2, §4 slice 1; audio/mixer.md "An effect slot"; audio/delay-compensation.md

use std::collections::VecDeque;

use crate::instrument::{HostedEffect, SharedEffect, TransportInfo};

/// One effect of the chain: its levels (the output is `wet` times the effect's output plus `dry` times its input) and
/// the line its dry part waits in.
pub struct Member {
    pub fx: SharedEffect,
    pub wet: f32,
    pub dry: f32,
    /// The delay the effect reported after its last block, and the dry part's line as long as that.
    delay: u32,
    line: [VecDeque<f32>; 2],
}

impl Member {
    pub fn new(fx: SharedEffect, wet: f32, dry: f32) -> Member {
        Member { fx, wet, dry, delay: 0, line: [VecDeque::new(), VecDeque::new()] }
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// A test effect: a gain, put out `late` samples late.
    struct Late {
        gain: f32,
        late: usize,
        line: [VecDeque<f32>; 2],
    }

    impl Late {
        fn shared(gain: f32, late: usize) -> SharedEffect {
            Arc::new(Mutex::new(Late { gain, late, line: [VecDeque::from(vec![0.0; late]), VecDeque::from(vec![0.0; late])] }))
        }
    }

    impl HostedEffect for Late {
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
        let mut c = Chain::new(vec![Member::new(Late::shared(0.5, 3), 1.0, 0.0), Member::new(Late::shared(2.0, 5), 0.25, 0.75)]);
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
}
