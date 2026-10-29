// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The voices: a note starts every pad on its key; a pad plays its layers whose velocity range holds the note's
//! velocity (one of them, in turn, when the pad's layers take turns), each layer through Lemon Sampler's own voices
//! (`lemon_sampler::dsp::Engine`: its resampler, its declicking, its velocity law) at the pad's and the layer's level,
//! pan and tune. A pad of a cut group stops the pads that group cuts. Pads play their samples to the end (one shot).
//! Muted pads are silent; while a pad is soloed only soloed pads play. The laws are the original's, measured from
//! renders (spec fpc.md §2).
//!
// spec: plugins/lemon-drum-sampler.md §2, plugins/fpc.md §2

use std::f64::consts::{PI, SQRT_2};
use std::sync::Arc;

use lemon_sampler::dsp::{Block, Engine as Voices};
use os_engine::sample::Sample;

use crate::state::{Pad, State, SW_MUTE, SW_SOLO, SW_TAKE_TURNS};

/// The key every layer's voices are started on (their root): a layer plays its sample at its own pitch plus its tune.
const ROOT: u8 = 60;
/// Room made per layer for voices (more grow when needed).
const ROOM: usize = 8;

/// A pad's or layer's volume value on the shared volume curve: value × 1.28 / 127 (measured, spec fpc.md §2).
fn volume_law(v: i32) -> f64 {
    os_engine::laws::volume_curve(f64::from(v.clamp(0, 1024)) * 1.28 / 127.0)
}

/// A pad volume as a gain, unity at a new pad's 99. The original's product of pad and layer gains was measured
/// relative to these defaults; its absolute level against the host was not (spec fpc.md §2).
pub fn pad_gain(v: i32) -> f32 {
    (volume_law(v) / volume_law(crate::state::NEW_PAD_VOLUME)) as f32
}

/// A layer volume as a gain, unity at a new layer's 127.
pub fn layer_gain(v: i32) -> f32 {
    (volume_law(v) / volume_law(crate::state::NEW_LAYER_VOLUME)) as f32
}

/// A pan field (−128..128) as −1..1; beyond the range it clamps (measured).
pub fn pan_of(v: i32) -> f64 {
    (f64::from(v) / 128.0).clamp(-1.0, 1.0)
}

/// A pan's split: constant power, unity at the centre (√2 at a side).
pub fn split(pan: f64) -> (f64, f64) {
    let a = (pan.clamp(-1.0, 1.0) + 1.0) * PI / 4.0;
    (SQRT_2 * a.cos(), SQRT_2 * a.sin())
}

/// The pad's and the layer's splits multiplied (measured: a pad and a layer both at half right give the product, not
/// the split of the sum) as one pan for the voice and a gain: the voice's own split of that pan times the gain is the
/// product.
pub fn pan_pair(pad: f64, layer: f64) -> (f32, f32) {
    let ((a, b), (c, d)) = (split(pad), split(layer));
    let (l, r) = (a * c, b * d);
    let m = ((l * l + r * r) / 2.0).sqrt();
    if m <= 0.0 {
        return (0.0, 0.0);
    }
    ((r.atan2(l) * 4.0 / PI - 1.0) as f32, m as f32)
}

/// A tune field as cents: ±128 = ±1 octave (measured; the pad's and the layer's add).
pub fn tune_cents(v: i32) -> f64 {
    f64::from(v) * 1200.0 / 128.0
}

/// One layer as the voices play it.
#[derive(Clone, Debug)]
pub struct LayerKit {
    pub sample: Option<Arc<Sample>>,
    pub lo: i32,
    pub hi: i32,
    pub gain: f32,
    pub pan: f64,
    pub cents: f64,
}

/// One pad as the voices play it.
#[derive(Clone, Debug)]
pub struct PadKit {
    pub key: Option<u8>,
    pub gain: f32,
    pub pan: f64,
    pub cents: f64,
    pub cut: i32,
    pub cut_by: i32,
    pub mute: bool,
    pub solo: bool,
    pub take_turns: bool,
    pub layers: Vec<LayerKit>,
}

/// What the voices play: the pads, and the pads on each key.
#[derive(Clone, Debug, Default)]
pub struct Kit {
    pub pads: Vec<PadKit>,
    pub by_key: Vec<Vec<u16>>,
    /// Some pad is soloed: only soloed pads play.
    pub solo: bool,
}

impl Kit {
    /// The kit of a saved state; `open(pad, layer, stored path)` gives a layer's sample (`None`: silent).
    pub fn new(st: &State, open: &mut dyn FnMut(usize, usize, &str) -> Option<Arc<Sample>>) -> Kit {
        let pads: Vec<PadKit> = st.pads().iter().enumerate().map(|(i, p)| pad_kit(i, p, open)).collect();
        let solo = pads.iter().any(|p| p.solo);
        let mut by_key = vec![Vec::new(); 128];
        for (i, p) in pads.iter().enumerate() {
            if let Some(k) = p.key
                && p.layers.iter().any(|l| l.sample.is_some())
                && !p.mute
                && (p.solo || !solo)
            {
                by_key[usize::from(k)].push(i as u16);
            }
        }
        Kit { pads, by_key, solo }
    }

    /// Pads that sound on `key`.
    pub fn pads_on(&self, key: u8) -> &[u16] {
        self.by_key.get(usize::from(key)).map_or(&[], |v| v.as_slice())
    }
}

fn pad_kit(i: usize, p: &Pad, open: &mut dyn FnMut(usize, usize, &str) -> Option<Arc<Sample>>) -> PadKit {
    let layers = p
        .layers
        .iter()
        .enumerate()
        .map(|(j, l)| LayerKit {
            sample: if l.path.is_empty() { None } else { open(i, j, &l.path) },
            lo: l.vel_lo,
            hi: l.vel_hi,
            gain: layer_gain(l.volume),
            pan: pan_of(l.pan),
            cents: tune_cents(l.tune),
        })
        .collect();
    PadKit {
        key: u8::try_from(p.key).ok().filter(|&k| k < 128),
        gain: pad_gain(p.volume),
        pan: pan_of(p.pan),
        cents: tune_cents(p.tune),
        cut: p.cut,
        cut_by: p.cut_by,
        mute: p.switches & SW_MUTE != 0,
        solo: p.switches & SW_SOLO != 0,
        take_turns: p.switches & SW_TAKE_TURNS != 0,
        layers,
    }
}

/// The layers of `pad` that hold note velocity `v` (the host's 0..128): `lo <= v < hi`, and a range up to 127 holds
/// every velocity from its low end (measured: 0–63 / 64–127 sounds neither layer at 63; 10–20 sounds at 10, not at
/// 20; 64–127 sounds at 127 and 128). Every layer that holds it plays.
pub fn layers_for(pad: &PadKit, v: i32) -> Vec<usize> {
    (0..pad.layers.len())
        .filter(|&j| {
            let l = &pad.layers[j];
            l.sample.is_some() && l.lo <= v && (v < l.hi || l.hi >= 127)
        })
        .collect()
}

/// A host note still sounding: its own voice id across the layers, its key and host id, and the layers still playing it.
#[derive(Clone, Copy, Debug)]
struct Live {
    id: i32,
    key: u8,
    host: Option<i32>,
    left: u32,
}

pub struct Engine {
    rate: f32,
    kit: Option<Arc<Kit>>,
    /// Voices per pad, per layer, and the settings each layer plays with.
    voices: Vec<Vec<Voices>>,
    blocks: Vec<Vec<Block>>,
    /// The next layer of each pad whose layers take turns.
    turn: Vec<usize>,
    live: Vec<Live>,
    next_id: i32,
    ended: Vec<(u8, Option<i32>)>,
}

impl Engine {
    pub fn new(rate: f32) -> Self {
        Engine { rate: rate.max(1.0), kit: None, voices: Vec::new(), blocks: Vec::new(), turn: Vec::new(), live: Vec::new(), next_id: 0, ended: Vec::new() }
    }

    pub fn set_rate(&mut self, rate: f32) {
        self.rate = rate.max(1.0);
        let kit = self.kit.take();
        self.voices.clear();
        self.set_kit(kit);
    }

    /// Plays `kit` from now on: layers keeping their sample keep their voices; the others start silent.
    pub fn set_kit(&mut self, kit: Option<Arc<Kit>>) {
        let pads = kit.as_ref().map_or(0, |k| k.pads.len());
        self.voices.resize_with(pads, Vec::new);
        self.blocks.resize_with(pads, Vec::new);
        self.turn.resize(pads, 0);
        if let Some(k) = &kit {
            for (i, p) in k.pads.iter().enumerate() {
                let vs = &mut self.voices[i];
                vs.truncate(p.layers.len());
                for (j, l) in p.layers.iter().enumerate() {
                    let same = vs.get(j).is_some_and(|v| match (&v.sample, &l.sample) {
                        (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                        (None, None) => true,
                        _ => false,
                    });
                    if !same {
                        let mut v = Voices::with_room(self.rate, ROOM);
                        v.sample = l.sample.clone();
                        if j < vs.len() {
                            vs[j] = v;
                        } else {
                            vs.push(v);
                        }
                    }
                }
                self.blocks[i] = p.layers.iter().map(|l| Block { cents: p.cents + l.cents, ..Block::default() }).collect();
            }
        }
        self.kit = kit;
    }

    pub fn reset(&mut self) {
        for v in self.voices.iter_mut().flatten() {
            v.reset();
        }
        self.live.clear();
    }

    pub fn has_voices(&self) -> bool {
        self.voices.iter().flatten().any(|v| v.has_voices())
    }

    /// Host voices that ended since the last call (key, host voice id).
    pub fn take_ended(&mut self) -> Vec<(u8, Option<i32>)> {
        std::mem::take(&mut self.ended)
    }

    /// A note: every pad on `key` plays; keys without a pad are silent (and their note ends at once).
    pub fn note_on(&mut self, key: u8, velocity: f32, host: Option<i32>) {
        let Some(kit) = self.kit.clone() else {
            self.ended.push((key, host));
            return;
        };
        let pads = kit.pads_on(key);
        if pads.is_empty() {
            self.ended.push((key, host));
            return;
        }
        self.next_id = self.next_id.wrapping_add(1) & 0x3fff_ffff;
        let id = self.next_id;
        let v128 = (velocity.clamp(0.0, 1.0) * 128.0).round() as i32;
        let mut left = 0;
        for &p in pads {
            let p = usize::from(p);
            let pad = &kit.pads[p];
            // A pad of a cut group stops the pads that group cuts (itself too, when it is one of them).
            if pad.cut >= 0 {
                for (q, other) in kit.pads.iter().enumerate() {
                    if other.cut_by == pad.cut {
                        for (v, b) in self.voices[q].iter_mut().zip(&self.blocks[q]) {
                            v.choke(None, ROOT, b);
                        }
                    }
                }
            }
            let mut layers = layers_for(pad, v128);
            if pad.take_turns && layers.len() > 1 {
                let n = layers.len();
                let pick = layers[self.turn[p] % n];
                self.turn[p] = (self.turn[p] + 1) % n;
                layers = vec![pick];
            }
            for j in layers {
                let l = &pad.layers[j];
                let (v, b) = (&mut self.voices[p][j], &self.blocks[p][j]);
                let (pan, g) = pan_pair(pad.pan, l.pan);
                v.note_on_id(ROOT, velocity, Some(id), b);
                v.volume(Some(id), ROOT, pad.gain * l.gain * g);
                v.pan(Some(id), ROOT, pan);
                left += 1;
            }
        }
        if left == 0 {
            self.ended.push((key, host));
        } else {
            self.live.push(Live { id, key, host, left });
        }
    }

    /// Every voice stops with a short fade.
    pub fn all_off(&mut self) {
        for v in self.voices.iter_mut().flatten() {
            v.all_off();
        }
    }

    /// Adds the voices into `left` / `right`.
    pub fn render(&mut self, left: &mut [f32], right: &mut [f32]) {
        for (vs, bs) in self.voices.iter_mut().zip(&self.blocks) {
            for (v, b) in vs.iter_mut().zip(bs) {
                if v.has_voices() {
                    v.render(left, right, b);
                }
                for (_, id) in v.take_ended() {
                    let Some(id) = id else { continue };
                    if let Some(i) = self.live.iter().position(|n| n.id == id) {
                        self.live[i].left -= 1;
                        if self.live[i].left == 0 {
                            let n = self.live.remove(i);
                            self.ended.push((n.key, n.host));
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{self, State};

    fn tone(rate: u32, frames: usize, amp: f32) -> Arc<Sample> {
        let v: Vec<f32> = (0..frames).map(|i| (i as f32 * 0.07).sin() * amp).collect();
        Arc::new(Sample { rate, left: v.clone(), right: v })
    }

    /// A kit whose layers are tones: the kick pad (key 36) one, pad 1 (key 37) a soft and a hard one.
    fn kit() -> Arc<Kit> {
        let st = state::tests::kit("kick.wav", "soft.wav", "hard.wav");
        Arc::new(Kit::new(&st, &mut |_, _, p| Some(tone(44100, 4410, if p == "soft.wav" { 0.1 } else { 0.5 }))))
    }

    fn peak(e: &mut Engine, n: usize) -> f32 {
        let (mut l, mut r) = (vec![0.0; n], vec![0.0; n]);
        e.render(&mut l, &mut r);
        l.iter().chain(&r).fold(0.0f32, |m, v| m.max(v.abs()))
    }

    #[test]
    fn a_pads_key_plays_its_sample_and_other_keys_are_silent() {
        let mut e = Engine::new(44100.0);
        e.set_kit(Some(kit()));
        e.note_on(60, 1.0, Some(7));
        assert_eq!(e.take_ended(), [(60, Some(7))], "no pad on C5: the note ends at once");
        assert!(peak(&mut e, 512) == 0.0);
        e.note_on(36, 100.0 / 127.0, Some(8));
        let p = peak(&mut e, 512);
        assert!(p > 0.3 && p < 0.7, "{p}");
        // The sample plays to its end, then the note's end is reported.
        for _ in 0..20 {
            peak(&mut e, 512);
        }
        assert!(!e.has_voices());
        assert_eq!(e.take_ended(), [(36, Some(8))]);
    }

    #[test]
    fn velocity_picks_the_layer() {
        let mut e = Engine::new(44100.0);
        e.set_kit(Some(kit()));
        e.note_on(37, 40.0 / 128.0, None);
        let soft = peak(&mut e, 512);
        e.reset();
        e.note_on(37, 1.0, None);
        let hard = peak(&mut e, 512);
        assert!(soft > 0.0 && hard > 4.0 * soft, "{soft} {hard}");
        // Soft 0–63, hard 64–127: 63 is in neither (the high end is not held), 127 and 128 in the hard one.
        let k = kit();
        let at = |v| layers_for(&k.pads[0], v);
        assert_eq!((at(0), at(62), at(63), at(64), at(127), at(128)), (vec![0], vec![0], vec![], vec![1], vec![1], vec![1]));
    }

    #[test]
    fn a_cut_group_stops_the_pads_it_cuts() {
        let mut st = state::tests::kit("kick.wav", "soft.wav", "hard.wav");
        st.set_sample(2, "hat.wav");
        st.set_sample(6, "open.wav");
        // Closed hat (pad 3) and open hat (pad 7) in group 0, cut by group 0.
        for p in [2, 6] {
            st.set_pad(p, state::PAD_CUT, 0);
            st.set_pad(p, state::PAD_CUT_BY, 0);
        }
        let kit = Arc::new(Kit::new(&st, &mut |_, _, _| Some(tone(44100, 44100, 0.5))));
        let mut e = Engine::new(44100.0);
        e.set_kit(Some(kit));
        e.note_on(46, 1.0, Some(1));
        e.note_on(36, 1.0, Some(2));
        peak(&mut e, 256);
        e.note_on(42, 1.0, Some(3));
        for _ in 0..8 {
            peak(&mut e, 256);
        }
        // The open hat ended (cut); the kick and the closed hat play on.
        assert_eq!(e.take_ended(), [(46, Some(1))]);
    }

    #[test]
    fn layers_taking_turns_play_one_at_a_time() {
        let mut st = State::default();
        st.set_sample(1, "a.wav");
        if let Some(state::Node::Tree(_, kids)) = st.nodes.get_mut(1) {
            let at = kids.iter().position(|n| n.id() == state::P_LAYER).unwrap() + 1;
            kids.insert(at, state::new_layer("b.wav"));
        }
        st.set_pad(1, state::PAD_SWITCHES, SW_TAKE_TURNS);
        let kit = Arc::new(Kit::new(&st, &mut |_, j, _| Some(tone(44100, 4410, if j == 0 { 0.1 } else { 0.5 }))));
        let mut e = Engine::new(44100.0);
        e.set_kit(Some(kit));
        let mut peaks = Vec::new();
        for _ in 0..3 {
            e.note_on(36, 100.0 / 127.0, None);
            peaks.push(peak(&mut e, 256));
            e.reset();
        }
        assert!(peaks[0] < 0.2 && peaks[1] > 0.3 && peaks[2] < 0.2, "{peaks:?}");
    }

    /// The measured laws: volumes on the shared curve at value × 1.28 / 127, pans multiplied, ±128 = ±1 octave.
    #[test]
    fn the_laws_match_the_measurements() {
        let db = |g: f32| 20.0 * g.log10();
        // Pad volume 25 / 50 / 75 / 200 and layer volume 32 / 64 / 96 / 200 (dB against the defaults).
        for (v, m) in [(25, -21.57), (50, -12.54), (75, -5.75), (200, 21.96)] {
            assert!((db(pad_gain(v)) - m).abs() < 0.03, "pad {v}: {}", db(pad_gain(v)));
        }
        for (v, m) in [(32, -24.90), (64, -14.89), (96, -6.99), (200, 15.67)] {
            assert!((db(layer_gain(v)) - m).abs() < 0.03, "layer {v}: {}", db(layer_gain(v)));
        }
        assert_eq!(pad_gain(0), 0.0);
        // Pad and layer both at half right: L −10.67 dB, R +4.65 dB.
        let (pan, g) = pan_pair(pan_of(64), pan_of(64));
        let (l, r) = split(f64::from(pan));
        assert!((db(l as f32 * g) + 10.67).abs() < 0.02 && (db(r as f32 * g) - 4.65).abs() < 0.02, "{pan} {g}");
        assert_eq!(pan_pair(0.0, 0.0), (0.0, 1.0));
        assert!((tune_cents(100) - 937.5).abs() < 1e-9 && tune_cents(-128) == -1200.0);
    }

    #[test]
    fn muted_pads_are_silent_and_a_solo_silences_the_others() {
        let mut st = state::tests::kit("kick.wav", "soft.wav", "hard.wav");
        st.set_pad(1, state::PAD_SWITCHES, state::SW_MUTE);
        let k = Kit::new(&st, &mut |_, _, _| Some(tone(44100, 4410, 0.5)));
        assert!(k.pads_on(36).is_empty() && !k.pads_on(37).is_empty());
        st.set_pad(1, state::PAD_SWITCHES, state::SW_SOLO);
        let k = Kit::new(&st, &mut |_, _, _| Some(tone(44100, 4410, 0.5)));
        assert!(!k.pads_on(36).is_empty() && k.pads_on(37).is_empty());
    }
}
