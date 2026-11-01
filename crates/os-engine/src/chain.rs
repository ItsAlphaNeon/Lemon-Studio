// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! A container's serial chain (Lemon Studio's Patcher, slices 1 and 3): one mixer slot effect, or one channel's
//! instrument, that runs other plugins in order, each with its own wet and dry levels. A member is an effect, which
//! works on what the members before it put out, or an instrument (in a channel's container), which plays the channel's
//! notes and puts out its own sound in place of its input. A member's dry part waits for the delay that member reports,
//! so the two parts meet in time, and the chain reports the sum of its members' delays to the mixer's delay
//! compensation. Tempo, transport and reset reach every member; the slot's sidechain input reaches the first member
//! only; the channel's notes, controllers, voicing and mod X / Y reach every instrument member, while its volume and pan
//! stay the engine's, on the chain's output. Automation and controller links reach a member as the container's
//! parameters: the member's id and an item packed in the parameter index (`os_model::target::member_item`), the item a
//! parameter of the member's plugin or the member's wet or dry level.
// spec: plugins/patcher/README.md Â§3.2, Â§3.3, Â§4 slices 1 and 3, Â§4.2, Â§4.4; audio/mixer.md "An effect slot";
// audio/delay-compensation.md

use std::collections::VecDeque;

use os_model::target::{member_of, MEMBER_DRY, MEMBER_WET};

use crate::instrument::{ChannelLevels, ChannelVoicing, HostedEffect, Instrument, NoteLevels, SharedEffect, SharedInstrument, TransportInfo};

/// What a member plays: an effect on the chain's signal, or an instrument whose sound takes the signal's place.
#[derive(Clone)]
pub enum Node {
    Fx(SharedEffect),
    Inst(SharedInstrument),
}

/// One plugin of the chain: its id in the container, its levels (the output is `wet` times the plugin's output plus
/// `dry` times the member's input) and the line its dry part waits in.
pub struct Member {
    pub id: u32,
    pub node: Node,
    pub wet: f32,
    pub dry: f32,
    /// The delay the plugin reported after its last block, and the dry part's line as long as that.
    delay: u32,
    line: [VecDeque<f32>; 2],
}

impl Member {
    /// An effect member.
    pub fn new(id: u32, fx: SharedEffect, wet: f32, dry: f32) -> Member {
        Member::of(id, Node::Fx(fx), wet, dry)
    }

    /// An instrument member (a channel's container): it plays the channel's notes.
    pub fn instrument(id: u32, inst: SharedInstrument, wet: f32, dry: f32) -> Member {
        Member::of(id, Node::Inst(inst), wet, dry)
    }

    fn of(id: u32, node: Node, wet: f32, dry: f32) -> Member {
        Member { id, node, wet, dry, delay: 0, line: [VecDeque::new(), VecDeque::new()] }
    }

    /// Whether the member's input reaches its output beside the plugin.
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

    /// A parameter value for the member's plugin (a busy plugin misses it, as a busy slot's does).
    fn param(&self, offset: u32, index: u32, value: f64) {
        match &self.node {
            Node::Fx(fx) => {
                if let Ok(mut fx) = fx.try_lock() {
                    fx.param_fx(offset, index, value);
                }
            }
            Node::Inst(inst) => {
                if let Ok(mut inst) = inst.try_lock() {
                    inst.param(offset, index, value);
                }
            }
        }
    }

    fn tick_params(&self) -> bool {
        match &self.node {
            Node::Fx(fx) => fx.try_lock().is_ok_and(|fx| fx.tick_params_fx()),
            Node::Inst(inst) => inst.try_lock().is_ok_and(|i| i.tick_params()),
        }
    }

    fn param_range(&self, index: usize) -> Option<(f64, f64)> {
        match &self.node {
            Node::Fx(fx) => fx.try_lock().ok()?.param_range_fx(index),
            Node::Inst(inst) => inst.try_lock().ok()?.param_range(index),
        }
    }
}

/// The chain: its members in order, the buffers one block's dry parts are kept in, and whether its last block put
/// out sound (an instrument container keeps its effects' tails playing while it does).
#[derive(Default)]
pub struct Chain {
    pub members: Vec<Member>,
    dry: [Vec<f32>; 2],
    sounding: bool,
}

/// Below this, a block's output is taken as silence (`Chain::active`).
const QUIET: f32 = 1e-5;

impl Chain {
    pub fn new(members: Vec<Member>) -> Chain {
        Chain { members, dry: [Vec::new(), Vec::new()], sounding: false }
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

    /// The instrument members' plugins that are not busy elsewhere, for the channel's notes and settings.
    fn instruments(&self, mut f: impl FnMut(&mut dyn Instrument)) {
        for m in &self.members {
            if let Node::Inst(inst) = &m.node {
                if let Ok(mut i) = inst.try_lock() {
                    f(&mut *i);
                }
            }
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
            // A member still busy elsewhere passes the block through (as a busy slot does); a busy instrument is silent.
            let delay = match &m.node {
                Node::Fx(fx) => match fx.try_lock() {
                    Ok(mut fx) => {
                        match key.take() {
                            Some(k) => fx.process_fx_keyed(rate, &mut left[..n], &mut right[..n], k),
                            None => fx.process_fx(rate, &mut left[..n], &mut right[..n]),
                        }
                        fx.reported_delay_fx()
                    }
                    Err(_) => m.delay,
                },
                Node::Inst(inst) => match inst.try_lock() {
                    Ok(mut i) => {
                        i.process(rate, &mut left[..n], &mut right[..n]);
                        i.reported_delay()
                    }
                    Err(_) => {
                        left[..n].fill(0.0);
                        right[..n].fill(0.0);
                        m.delay
                    }
                },
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

    /// A member's parameter goes to its plugin; its wet and dry levels are set here, for the whole of the next block.
    fn set_param(&mut self, offset: u32, index: u32, value: f64) {
        let Some((id, item)) = u16::try_from(index).ok().and_then(member_of) else { return };
        let Some(m) = self.members.iter_mut().find(|m| m.id == id) else { return };
        let level = value.clamp(0.0, 1.0) as f32;
        match item {
            MEMBER_WET => m.wet = level,
            MEMBER_DRY => m.dry = level,
            k => m.param(offset, u32::from(k), value),
        }
    }

    /// Each member's plugin takes its parameters as it would in a slot or channel; the levels take events.
    fn ticks_at(&self, index: u32) -> bool {
        match self.item(index) {
            Some((m, k)) if k < MEMBER_WET => m.tick_params(),
            _ => false,
        }
    }

    fn range(&self, index: usize) -> Option<(f64, f64)> {
        let (m, k) = self.item(u32::try_from(index).ok()?)?;
        if k >= MEMBER_WET {
            return Some((0.0, 1.0));
        }
        m.param_range(usize::from(k))
    }

    fn reset_all(&mut self) {
        for m in &mut self.members {
            match &m.node {
                Node::Fx(fx) => {
                    if let Ok(mut fx) = fx.try_lock() {
                        fx.reset_fx();
                    }
                }
                Node::Inst(inst) => {
                    if let Ok(mut i) = inst.try_lock() {
                        i.reset();
                    }
                }
            }
            let d = m.delay;
            m.set_delay(d);
        }
        self.sounding = false;
    }

    fn transport_all(&self, info: &TransportInfo) {
        for m in &self.members {
            match &m.node {
                Node::Fx(fx) => {
                    if let Ok(mut fx) = fx.try_lock() {
                        fx.transport_fx(info);
                    }
                }
                Node::Inst(inst) => {
                    if let Ok(mut i) = inst.try_lock() {
                        i.transport(info);
                    }
                }
            }
        }
    }

    /// The chain's delay: its members' reported delays added up.
    fn delay(&self) -> u32 {
        self.members.iter().map(|m| m.delay).sum()
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
        self.reset_all();
    }

    fn transport_fx(&mut self, info: &TransportInfo) {
        self.transport_all(info);
    }

    fn reported_delay_fx(&self) -> u32 {
        self.delay()
    }

    fn param_fx(&mut self, offset: u32, index: u32, value: f64) {
        self.set_param(offset, index, value);
    }

    fn tick_params_at(&self, index: u32) -> bool {
        self.ticks_at(index)
    }

    fn param_range_fx(&self, index: usize) -> Option<(f64, f64)> {
        self.range(index)
    }
}

/// A channel's container: the chain as the channel's instrument (spec: plugins/patcher/README.md Â§4.4). Its input is
/// silence; the instrument members' sound goes through the effects after them.
impl Instrument for Chain {
    fn note_on(&mut self, offset: u32, key: u8, velocity: f32) {
        self.instruments(|i| i.note_on(offset, key, velocity));
    }

    fn note_off(&mut self, offset: u32, key: u8) {
        self.instruments(|i| i.note_off(offset, key));
    }

    fn note_on_from(&mut self, offset: u32, key: u8, velocity: f32, start: f32) {
        self.instruments(|i| i.note_on_from(offset, key, velocity, start));
    }

    fn note_start(&mut self, offset: u32, key: u8, velocity: f32, start: f32, levels: NoteLevels) {
        self.instruments(|i| i.note_start(offset, key, velocity, start, levels));
    }

    fn choke(&mut self, offset: u32, key: u8) {
        self.instruments(|i| i.choke(offset, key));
    }

    fn tune(&mut self, offset: u32, key: u8, semitones: f32) {
        self.instruments(|i| i.tune(offset, key, semitones));
    }

    fn set_bend_range(&mut self, semitones: u32) {
        self.instruments(|i| i.set_bend_range(semitones));
    }

    /// Only when every instrument inside plays a note's tuning: otherwise the engine moves the keys by whole semitones,
    /// which every instrument plays right.
    fn takes_tuning(&self) -> bool {
        let mut all = None;
        self.instruments(|i| all = Some(all.unwrap_or(true) && i.takes_tuning()));
        all.unwrap_or(false)
    }

    fn process(&mut self, rate: u32, left: &mut [f32], right: &mut [f32]) {
        left.fill(0.0);
        right.fill(0.0);
        self.run(rate, left, right, None);
        self.sounding = left.iter().chain(right.iter()).any(|x| x.abs() > QUIET);
    }

    /// While an instrument inside rings, or the chain's last block was not silent (an effect's tail).
    fn active(&self) -> bool {
        let mut any = self.sounding;
        self.instruments(|i| any |= i.active());
        any
    }

    fn voices(&self) -> usize {
        let mut n = 0;
        self.instruments(|i| n += i.voices());
        n
    }

    fn reset(&mut self) {
        self.reset_all();
    }

    fn param(&mut self, offset: u32, index: u32, value: f64) {
        self.set_param(offset, index, value);
    }

    fn tick_params_at(&self, index: u32) -> bool {
        self.ticks_at(index)
    }

    fn param_range(&self, index: usize) -> Option<(f64, f64)> {
        self.range(index)
    }

    /// The instruments inside get the channel's mod X and mod Y, at full volume and centred: the engine puts the
    /// channel's volume and pan on the chain's output (the channel's settings act after the container).
    fn channel_levels(&mut self, levels: ChannelLevels) -> bool {
        let inner = ChannelLevels { volume: 1.0, pan: 0.0, ..levels };
        self.instruments(|i| {
            i.channel_levels(inner);
        });
        false
    }

    fn channel_voicing(&mut self, voicing: &ChannelVoicing) {
        self.instruments(|i| i.channel_voicing(voicing));
    }

    fn transport(&mut self, info: &TransportInfo) {
        self.transport_all(info);
    }

    fn midi(&mut self, offset: u32, bytes: [u8; 3]) {
        self.instruments(|i| i.midi(offset, bytes));
    }

    fn reported_delay(&self) -> u32 {
        self.delay()
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
        let Node::Fx(held) = c.members[0].node.clone() else { panic!("an effect") };
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
        assert!(HostedEffect::tick_params_at(&c, k(4, 0)));
        assert!(!HostedEffect::tick_params_at(&c, k(2, 0)));
        assert!(!HostedEffect::tick_params_at(&c, k(4, MEMBER_WET)));
        assert_eq!(c.param_range_fx(k(2, 0) as usize), Some((0.0, 4.0)));
        assert_eq!(c.param_range_fx(k(2, MEMBER_DRY) as usize), Some((0.0, 1.0)));
        assert_eq!(c.param_range_fx(k(2, 1) as usize), None);
        // The levels sound: member 4 at wet 0.5, dry 1 over member 2 at gain 2.
        let (mut l, mut r) = (vec![1.0; 4], vec![1.0; 4]);
        c.process_fx(48000, &mut l, &mut r);
        assert_eq!(l, vec![(0.5 + 1.0) * 2.0; 4]);
    }
    /// A test instrument: while a key is held it puts out `level` times the velocity on both sides, `late` samples late;
    /// it notes what it was given.
    struct Tone {
        level: f32,
        held: Vec<(u8, f32)>,
        late: u32,
        got: Vec<String>,
        tuning: bool,
    }

    impl Tone {
        fn shared(level: f32, late: u32, tuning: bool) -> Arc<Mutex<Tone>> {
            Arc::new(Mutex::new(Tone { level, held: Vec::new(), late, got: Vec::new(), tuning }))
        }
    }

    impl Instrument for Tone {
        fn note_on(&mut self, _offset: u32, key: u8, velocity: f32) {
            self.held.push((key, velocity));
        }
        fn note_off(&mut self, _offset: u32, key: u8) {
            self.held.retain(|h| h.0 != key);
        }
        fn process(&mut self, _rate: u32, left: &mut [f32], right: &mut [f32]) {
            let v: f32 = self.held.iter().map(|h| h.1 * self.level).sum();
            left.fill(v);
            right.fill(v);
        }
        fn active(&self) -> bool {
            !self.held.is_empty()
        }
        fn voices(&self) -> usize {
            self.held.len()
        }
        fn param(&mut self, offset: u32, index: u32, value: f64) {
            self.got.push(format!("param {offset} {index} {value}"));
        }
        fn channel_levels(&mut self, levels: ChannelLevels) -> bool {
            self.got.push(format!("levels {} {} {}", levels.volume, levels.pan, levels.mod_x));
            true
        }
        fn takes_tuning(&self) -> bool {
            self.tuning
        }
        fn reported_delay(&self) -> u32 {
            self.late
        }
    }

    /// A channel's container: the channel's notes reach every instrument inside, whose sound goes through the effects
    /// after it; an instrument at dry 1 adds its sound to what came before (a layer); the channel's volume and pan stay
    /// the engine's; parameters reach an instrument member by id; an effect's tail keeps the chain active.
    #[test]
    fn an_instrument_container() {
        use os_model::target::member_item;
        let (a, b) = (Tone::shared(1.0, 0, true), Tone::shared(0.25, 0, false));
        let fx = Late::shared(0.5, 0);
        let mut c = Chain::new(vec![
            Member::instrument(1, a.clone(), 1.0, 0.0),
            Member::instrument(3, b.clone(), 1.0, 1.0),
            Member::new(2, fx, 1.0, 0.0),
        ]);
        let inst: &mut dyn Instrument = &mut c;
        assert!(!inst.active());
        inst.note_on(0, 60, 0.8);
        assert_eq!(inst.voices(), 2, "both instruments play the note");
        let (mut l, mut r) = (vec![9.0; 4], vec![9.0; 4]);
        inst.process(48000, &mut l, &mut r);
        // (0.8 from the first, plus the second's 0.2 at dry 1) through the effect's gain of 0.5.
        assert_eq!(l, vec![(0.8 + 0.2) * 0.5; 4]);
        assert_eq!(l, r);
        assert!(inst.active());
        assert!(!inst.takes_tuning(), "the second does not take a tuning");
        assert!(!inst.channel_levels(ChannelLevels { volume: 0.5, pan: -1.0, mod_x: 0.25, mod_y: 0.0 }), "the engine applies the volume");
        assert_eq!(a.lock().unwrap().got, vec!["levels 1 0 0.25".to_string()]);
        let k = |id: u32, item: u16| u32::from(member_item(id, item).unwrap());
        inst.param(7, k(3, 12), 0.5);
        inst.param(0, k(1, MEMBER_DRY), 1.0);
        assert_eq!(b.lock().unwrap().got.last().map(String::as_str), Some("param 7 12 0.5"));
        assert_eq!(c.members[0].dry, 1.0);
        let inst: &mut dyn Instrument = &mut c;
        assert_eq!(inst.param_range(k(3, MEMBER_WET) as usize), Some((0.0, 1.0)));
        inst.note_off(0, 60);
        assert_eq!(inst.voices(), 0);
        let (mut l, mut r) = (vec![0.0; 4], vec![0.0; 4]);
        inst.process(48000, &mut l, &mut r);
        assert!(!inst.active(), "no voice, a silent block");
        // A delay inside an instrument is the chain's; its dry part waits for it.
        let late = Tone::shared(1.0, 2, false);
        let mut c = Chain::new(vec![Member::instrument(1, late, 1.0, 0.0), Member::new(2, Late::shared(1.0, 3), 1.0, 0.0)]);
        let (mut l, mut r) = (vec![0.0; 4], vec![0.0; 4]);
        Instrument::process(&mut c, 48000, &mut l, &mut r);
        assert_eq!(Instrument::reported_delay(&c), 5);
        assert_eq!(HostedEffect::reported_delay_fx(&c), 5);
    }
}
