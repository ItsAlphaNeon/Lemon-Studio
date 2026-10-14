// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

//! Layer channels: which children get a layer's note, at what crossfade gain, and the project's random generator
//! that the random mode draws from.
// spec: audio/note-rules.md "Layer channels", audio/channel-misc.md "FL's random generator"

use os_model::{Channel, LayerSettings};

/// The Mersenne Twister MT19937 with 32-bit outputs (Matsumoto and Nishimura's published algorithm), one for the
/// whole project.
#[derive(Clone)]
pub(super) struct Mt19937 {
    state: [u32; 624],
    index: usize,
}

impl std::fmt::Debug for Mt19937 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Mt19937").field("index", &self.index).finish_non_exhaustive()
    }
}

impl Mt19937 {
    /// The seed the project's generator starts from.
    pub const SEED: u32 = 19650218;

    pub fn new(seed: u32) -> Mt19937 {
        let mut state = [0u32; 624];
        state[0] = seed;
        for i in 1..624 {
            state[i] = 1812433253u32.wrapping_mul(state[i - 1] ^ (state[i - 1] >> 30)).wrapping_add(i as u32);
        }
        Mt19937 { state, index: 624 }
    }

    /// The generator as a render starts it: seeded, with one output taken before the song begins (spec:
    /// audio/channel-misc.md "FL's random generator").
    pub fn for_playback() -> Mt19937 {
        let mut g = Mt19937::new(Mt19937::SEED);
        g.next_u32();
        g
    }

    pub fn next_u32(&mut self) -> u32 {
        if self.index >= 624 {
            for i in 0..624 {
                let y = (self.state[i] & 0x8000_0000) | (self.state[(i + 1) % 624] & 0x7fff_ffff);
                let mut x = self.state[(i + 397) % 624] ^ (y >> 1);
                if y & 1 != 0 {
                    x ^= 0x9908_b0df;
                }
                self.state[i] = x;
            }
            self.index = 0;
        }
        let mut y = self.state[self.index];
        self.index += 1;
        y ^= y >> 11;
        y ^= (y << 7) & 0x9d2c_5680;
        y ^= (y << 15) & 0xefc6_0000;
        y ^ (y >> 18)
    }
}

/// A place among `count` for the output `u`: `Trunc(u * 2^-32 * count)`, in doubles.
pub(super) fn place(u: u32, count: usize) -> usize {
    ((f64::from(u) * 2f64.powi(-32) * count as f64).trunc() as usize).min(count.saturating_sub(1))
}

/// The crossfade gain of the child at place `i` of `count` (a child listed twice counts at its first place) for the
/// stored position `position` (0..256): `x = single(position) * single(1 / 256)`; one child gets `x`, otherwise
/// `max(0, 1 - |x * (count - 1) - i|)` in doubles.
pub(super) fn crossfade_gain(position: i32, count: usize, i: usize) -> f64 {
    let x = f64::from(position as f32 * (1.0f32 / 256.0));
    if count <= 1 {
        x
    } else {
        (1.0 - (x * (count - 1) as f64 - i as f64).abs()).max(0.0)
    }
}

/// A layer channel as the engine plays it.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Layer {
    /// Rack positions, in the order they are played.
    pub children: Vec<usize>,
    pub random: bool,
    pub in_turn: bool,
    /// The crossfade position when the crossfade is on.
    pub crossfade: Option<i32>,
    /// Whose turn it is (in-turn mode), as a place in `children`.
    pub turn: usize,
}

impl Layer {
    pub fn of(ch: &Channel) -> Layer {
        let s = ch.misc.layer;
        Layer {
            children: ch.layer_children.iter().map(|&c| usize::from(c)).collect(),
            random: s.flags & LayerSettings::RANDOM != 0,
            in_turn: s.flags & LayerSettings::IN_TURN != 0,
            crossfade: (s.flags & LayerSettings::CROSSFADE != 0).then_some(s.crossfade),
            turn: 0,
        }
    }

    /// The places in `children` that get the next note: every child (flags 0), one in turn starting with the first
    /// (flag 4 without flag 1), or one at random from `rng` (flag 1).
    pub fn places(&mut self, rng: &mut Mt19937) -> std::ops::Range<usize> {
        let count = self.children.len();
        if count == 0 {
            return 0..0;
        }
        if self.random {
            let i = place(rng.next_u32(), count);
            i..i + 1
        } else if self.in_turn {
            let i = self.turn % count;
            self.turn = (i + 1) % count;
            i..i + 1
        } else {
            0..count
        }
    }

    /// The child at place `i`'s crossfade gain (1 without the crossfade).
    pub fn gain(&self, i: usize) -> f64 {
        match self.crossfade {
            None => 1.0,
            Some(position) => {
                let child = self.children[i];
                let first = self.children.iter().position(|&c| c == child).unwrap_or(i);
                crossfade_gain(position, self.children.len(), first)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The generator is MT19937: with the reference seed 5489 its first output is 3499211612 and its 10000th is
    /// 4123659995 (the values the C++ standard library requires of `std::mt19937`).
    #[test]
    fn the_generator_is_mt19937() {
        let mut g = Mt19937::new(5489);
        assert_eq!(g.next_u32(), 3499211612);
        for _ in 1..9999 {
            g.next_u32();
        }
        assert_eq!(g.next_u32(), 4123659995);
    }

    /// The sixteen draws among two children of the export the spec compares: outputs 2 to 17 of the generator
    /// seeded with 19650218 (spec: audio/note-rules.md "Layer channels", "Checked against FL").
    #[test]
    fn a_random_layer_draws_as_the_spec_gives() {
        let mut g = Mt19937::for_playback();
        let drawn: Vec<usize> = (0..16).map(|_| place(g.next_u32(), 2)).collect();
        assert_eq!(drawn, [0, 1, 1, 0, 0, 0, 1, 1, 1, 1, 0, 0, 1, 0, 0, 0]);
    }

    /// Position 64 with two children gives 0.75 and 0.25; position 160 with three gives 0, 0.75 and 0.25; one child
    /// gets the position itself.
    #[test]
    fn crossfade_gains() {
        assert_eq!([crossfade_gain(64, 2, 0), crossfade_gain(64, 2, 1)], [0.75, 0.25]);
        assert_eq!([crossfade_gain(160, 3, 0), crossfade_gain(160, 3, 1), crossfade_gain(160, 3, 2)], [0.0, 0.75, 0.25]);
        assert_eq!(crossfade_gain(128, 1, 0), 0.5);
        // A child listed twice counts at its first place.
        let l = Layer { children: vec![4, 5, 4], random: false, in_turn: false, crossfade: Some(0), turn: 0 };
        assert_eq!([l.gain(0), l.gain(1), l.gain(2)], [1.0, 0.0, 1.0]);
    }

    /// All children, in turn, at random.
    #[test]
    fn which_children_get_a_note() {
        let mut rng = Mt19937::for_playback();
        let mut all = Layer { children: vec![3, 4, 5], random: false, in_turn: false, crossfade: None, turn: 0 };
        assert_eq!(all.places(&mut rng), 0..3);
        let mut turn = Layer { in_turn: true, ..all.clone() };
        let got: Vec<usize> = (0..5).map(|_| turn.places(&mut rng).start).collect();
        assert_eq!(got, [0, 1, 2, 0, 1]);
        // Random wins over in turn; it draws from the project's generator.
        let mut random = Layer { random: true, in_turn: true, ..all.clone() };
        let mut check = Mt19937::for_playback();
        for _ in 0..8 {
            let want = place(check.next_u32(), 3);
            assert_eq!(random.places(&mut rng), want..want + 1);
        }
        all.children.clear();
        assert_eq!(all.places(&mut rng), 0..0);
    }
}
