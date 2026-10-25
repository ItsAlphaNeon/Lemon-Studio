// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The piano roll's Randomize tool (Tools ▸ Randomize…, Alt+R): its settings and the randomizer itself, free of any
//! window. The Pattern section writes a new random melody over the pattern, the Levels section randomizes the notes'
//! levels. The dialog's session is `piano_dlg.rs`, the window `views/piano_dlg.rs`.
// spec: ui/piano-randomize.md

use os_model::Note;

use crate::chords;
use crate::piano_tools::{glue, next};

/// Length's ends in grid steps (§1).
pub const LEN_MIN: f32 = 1.0 / 6.0;
pub const LEN_MAX: f32 = 3.0;
/// Variation's ends in grid steps, ±.
pub const VAR_MAX: f32 = 3.0;

/// Wheel notches on the controls' 0..1 positions (§1): Octave, Range and Stack one, Length 1/12 of a step, Variation 1/6
/// of a step, Population 1/20 of its travel, a Levels knob 6/64 of its −100…+100 %.
pub const OCTAVE_NOTCH: f32 = 1.0 / 9.0;
pub const COUNT_NOTCH: f32 = 1.0 / 5.0;
pub const LENGTH_NOTCH: f32 = (1.0 / 12.0) / (LEN_MAX - LEN_MIN);
pub const VARIATION_NOTCH: f32 = (1.0 / 6.0) / (2.0 * VAR_MAX);
pub const POPULATION_NOTCH: f32 = 1.0 / 20.0;
pub const LEVEL_NOTCH: f32 = 3.0 / 64.0;

/// The shortest note the Pattern section writes, in ticks (§3; Lemon Studio keeps it at every PPQ, §6).
pub const MIN_LEN: u32 = 4;
/// The highest key: higher picks become it.
const TOP_KEY: u16 = 131;
/// Random portamento's odds (a Lemon Studio choice near FL's measured one in eight, §6).
const PORTA_ODDS: f64 = 1.0 / 8.0;

/// Each Levels property in `quantize::LEVELS` order (PAN … PITCH): half its swing, its largest value and a new note's
/// value (§4).
const LEVELS: [(f64, f64, u8); 6] = [(64.0, 128.0, 64), (64.0, 128.0, 100), (64.0, 128.0, 64), (128.0, 255.0, 128), (128.0, 255.0, 128), (120.0, 240.0, 120)];

/// Salts that keep the two sections' draws apart.
const PATTERN_SALT: u64 = 0x5241_4e44_5041_5454;
const LEVELS_SALT: u64 = 0x5241_4e44_4c45_564c;

/// The dialog's controls (§1), Reset's values by default.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Settings {
    /// The Pattern section: write new notes.
    pub pattern: bool,
    /// 0..=9: the lowest key is 12 × octave + key.
    pub octave: u8,
    /// Octaves the keys span, 1..=6.
    pub range: u8,
    /// The key's semitone (0 = C).
    pub key: u8,
    /// 0 = (none), else an entry of `chords`.
    pub scale: usize,
    /// A note's length in grid steps, `LEN_MIN..=LEN_MAX`.
    pub length: f32,
    /// Up to this many grid steps added (taken away when negative), `-VAR_MAX..=VAR_MAX`.
    pub variation: f32,
    /// The knob's position, 0..1: 1 keeps every note.
    pub population: f32,
    /// Notes on each step, 1..=6.
    pub stack: u8,
    pub porta: bool,
    pub glue: bool,
    /// The Levels section.
    pub levels: bool,
    /// −1..1, in `quantize::LEVELS` order.
    pub amounts: [f32; 6],
    pub reset_before: bool,
    pub bipolar: bool,
    /// The sections' seeds (§2); Reset leaves them.
    pub pattern_seed: u32,
    pub levels_seed: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            pattern: true,
            octave: 4,
            range: 3,
            key: 0,
            scale: 0,
            length: 1.0,
            variation: 0.0,
            population: 1.0,
            stack: 1,
            porta: false,
            glue: true,
            levels: true,
            amounts: [0.0; 6],
            reset_before: false,
            bipolar: true,
            pattern_seed: 0,
            levels_seed: 0,
        }
    }
}

/// `v` to two decimals, trailing zeros off ("1", "1.08", "0.17").
fn num(v: f32) -> String {
    let s = format!("{:.2}", (v * 100.0).round() / 100.0);
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// "1 step", "1.5 steps".
fn steps(v: f32) -> String {
    let n = num(v.abs());
    format!("{}{n} step{}", if v < 0.0 { "-" } else if v > 0.0 { "+" } else { "" }, if n == "1" { "" } else { "s" })
}

impl Settings {
    /// Reset (§1): every control back, the seeds as they are.
    pub fn reset(&self) -> Settings {
        Settings { pattern_seed: self.pattern_seed, levels_seed: self.levels_seed, ..Settings::default() }
    }

    pub fn length_pos(&self) -> f32 {
        (self.length - LEN_MIN) / (LEN_MAX - LEN_MIN)
    }

    pub fn set_length_pos(&mut self, v: f32) {
        self.length = LEN_MIN + v.clamp(0.0, 1.0) * (LEN_MAX - LEN_MIN);
    }

    pub fn variation_pos(&self) -> f32 {
        (self.variation + VAR_MAX) / (2.0 * VAR_MAX)
    }

    pub fn set_variation_pos(&mut self, v: f32) {
        self.variation = v.clamp(0.0, 1.0) * 2.0 * VAR_MAX - VAR_MAX;
    }

    /// The hints (Lemon Studio's, §6: FL's controls write none): "Length: 1 step".
    pub fn length_text(&self) -> String {
        format!("Length: {}", steps(self.length).trim_start_matches('+'))
    }

    /// "Variation: +0.5 steps".
    pub fn variation_text(&self) -> String {
        format!("Variation: {}", steps(self.variation))
    }

    /// "Population: 100%".
    pub fn population_text(&self) -> String {
        format!("Population: {}%", (self.population * 100.0).round())
    }

    /// A Levels knob's: "0%", "+50%".
    pub fn amount_text(&self, i: usize) -> String {
        let p = (self.amounts[i.min(5)] * 100.0).round() as i32;
        if p == 0 { "0%".into() } else { format!("{p:+}%") }
    }

    /// The scale box's text.
    pub fn scale_name(&self) -> &'static str {
        if self.scale == 0 { "(none)" } else { chords::label(self.scale) }
    }
}

/// The two sections' sequences of draws.
fn rng(seed: u32, salt: u64) -> u64 {
    u64::from(seed).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ salt
}

/// A draw in [0, 1).
fn draw(rng: &mut u64) -> f64 {
    (next(rng) >> 11) as f64 / (1u64 << 53) as f64
}

/// The keys a draw picks from (§3 "Keys"): from 12 × octave + key up, every key ((none)) or the scale's, in each of
/// `range` octaves; higher than 131 becomes 131. A chord's notes past the octave count as their pitch classes (a Lemon
/// Studio choice, §6).
pub fn choices(set: &Settings) -> Vec<u16> {
    let base = 12 * u16::from(set.octave.min(9)) + u16::from(set.key % 12);
    let on = if set.scale == 0 { [true; 12] } else { chords::classes(set.scale, 0) };
    let offs: Vec<u16> = (0..12u16).filter(|&i| on[usize::from(i)]).collect();
    (0..u16::from(set.range.clamp(1, 6))).flat_map(|o| offs.iter().map(move |&d| (base + 12 * o + d).min(TOP_KEY))).collect()
}

/// The Pattern section (§3): new notes of channel `ch` over `0..end`, `stack` of them on every step of `grid` ticks.
/// Each draws its key, its length, whether it is kept and its portamento switch, in that order (§6), whatever the
/// settings; then the notes of one key are glued, or cut where the next one starts.
pub fn generate(set: &Settings, ch: u16, grid: u32, end: u32) -> Vec<Note> {
    let (keys, grid) = (choices(set), grid.max(1));
    let keep = 0.01 + 0.99 * f64::from(set.population.clamp(0.0, 1.0));
    let mut r = rng(set.pattern_seed, PATTERN_SALT);
    let mut out = Vec::new();
    for t in (0..end).step_by(grid as usize) {
        for _ in 0..set.stack.clamp(1, 6) {
            let (u, w, p, q) = (draw(&mut r), draw(&mut r), draw(&mut r), draw(&mut r));
            if p >= keep {
                continue;
            }
            let len = (f64::from(grid) * (f64::from(set.length) + f64::from(set.variation) * w)).round().max(f64::from(MIN_LEN)) as u32;
            let key = keys[((u * keys.len() as f64) as usize).min(keys.len() - 1)];
            let mut n = Note { length: len.min(end - t), key, ..Note::step(ch, t) };
            n.set_porta(set.porta && q < PORTA_ODDS);
            out.push(n);
        }
    }
    let mut out = if set.glue { glue(out) } else { cut(out) };
    out.sort_by_key(|n| (n.position, n.key));
    out
}

/// Glue same notes off (§3): a note that a later note of its key overlaps ends where that one starts, and goes when
/// that leaves it shorter than `MIN_LEN`.
fn cut(notes: Vec<Note>) -> Vec<Note> {
    let mut order: Vec<usize> = (0..notes.len()).collect();
    order.sort_by_key(|&i| (notes[i].key, notes[i].position, i));
    let mut out = Vec::new();
    for (j, &i) in order.iter().enumerate() {
        let mut n = notes[i];
        if let Some(&k) = order.get(j + 1).filter(|&&k| notes[k].key == n.key && notes[k].position < n.position + n.length) {
            n.length = notes[k].position - n.position;
            if n.length < MIN_LEN {
                continue;
            }
        }
        out.push(n);
    }
    out
}

/// A note's Levels property `i` (`quantize::LEVELS` order).
fn prop(n: &mut Note, i: usize) -> &mut u8 {
    match i {
        0 => &mut n.pan,
        1 => &mut n.velocity,
        2 => &mut n.release,
        3 => &mut n.mod_x,
        4 => &mut n.mod_y,
        _ => &mut n.fine_pitch,
    }
}

/// The Levels section (§4): every knob not at 0 moves its property of each note, in order, by a draw of its own: from
/// the note's value (or a new note's, with Reset before processing) by up to the knob's share of half the property's
/// swing either way (Bipolar), or one way.
pub fn levels(mut notes: Vec<Note>, set: &Settings) -> Vec<Note> {
    if !set.levels || set.amounts.iter().all(|&a| a == 0.0) {
        return notes;
    }
    let mut r = rng(set.levels_seed, LEVELS_SALT);
    for n in &mut notes {
        for (i, &a) in set.amounts.iter().enumerate().filter(|(_, a)| **a != 0.0) {
            let (half, max, fresh) = LEVELS[i];
            let w = draw(&mut r);
            let p = prop(n, i);
            let from = f64::from(if set.reset_before { fresh } else { *p });
            let d = if set.bipolar { 2.0 * w - 1.0 } else { w };
            *p = (from + f64::from(a.clamp(-1.0, 1.0)) * d * half).round().clamp(0.0, max) as u8;
        }
    }
    notes
}

/// The whole tool: with Pattern on, `notes` (the channel's) give way to new ones over `0..end`; the Levels section
/// then works on the new notes, or on `notes` (the targets) with Pattern off.
pub fn randomize(notes: Vec<Note>, set: &Settings, ch: u16, grid: u32, end: u32) -> Vec<Note> {
    let notes = if set.pattern { generate(set, ch, grid, end) } else { notes };
    levels(notes, set)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spans(ns: &[Note]) -> Vec<(u32, u32, u16)> {
        ns.iter().map(|n| (n.position, n.length, n.key)).collect()
    }

    #[test]
    fn keys_come_from_the_octaves_and_the_scale() {
        let mut set = Settings::default();
        assert_eq!(choices(&set), (48..84).collect::<Vec<_>>(), "Reset: three octaves from C4 (48)");
        set.key = 2;
        assert_eq!((choices(&set)[0], *choices(&set).last().unwrap()), (50, 85), "on D");
        set.scale = chords::FIRST_SCALE; // Major
        set.range = 1;
        assert_eq!(choices(&set), [50, 52, 54, 55, 57, 59, 61]);
        set.scale = 5; // the minor chord
        set.key = 0;
        assert_eq!(choices(&set), [48, 51, 55]);
        set.octave = 9;
        set.scale = 0;
        set.range = 6;
        assert_eq!(*choices(&set).last().unwrap(), 131, "keys above 131 become 131");
    }

    #[test]
    fn reset_fills_every_step_of_the_pattern() {
        let set = Settings { glue: false, ..Settings::default() };
        let ns = generate(&set, 3, 24, 768);
        assert_eq!(ns.len(), 32, "Glue off: no same-key overlaps at Length 1");
        assert!(ns.iter().enumerate().all(|(i, n)| n.position == 24 * i as u32 && n.length == 24 && n.channel == 3));
        assert!(ns.iter().all(|n| (48..84).contains(&n.key) && n.velocity == 100 && n.pan == 64 && n.fine_pitch == 120 && !n.porta()));
        assert_eq!(generate(&set, 3, 24, 768), ns, "the same seed gives the same notes");
        let other = generate(&Settings { pattern_seed: 1, ..set }, 3, 24, 768);
        assert_ne!(spans(&other), spans(&ns), "another seed, another melody");
    }

    #[test]
    fn lengths_follow_length_and_variation() {
        let lens = |set: Settings| generate(&Settings { glue: false, key: 0, ..set }, 0, 24, 768).iter().map(|n| n.length).collect::<Vec<_>>();
        assert!(lens(Settings { length: 13.0 / 12.0, ..Settings::default() }).iter().all(|&l| l == 26 || l == 24));
        assert!(lens(Settings { length: LEN_MIN, ..Settings::default() }).iter().all(|&l| l == 4));
        let v = lens(Settings { variation: 1.0 / 6.0, ..Settings::default() });
        assert!(v.iter().all(|&l| (24..=28).contains(&l)) && v.iter().any(|&l| l > 24));
        let v = lens(Settings { variation: -VAR_MAX, ..Settings::default() });
        assert!(v.iter().all(|&l| (4..=24).contains(&l)) && v.contains(&4), "at least 4 ticks");
        let ns = generate(&Settings { length: LEN_MAX, glue: false, ..Settings::default() }, 0, 24, 768);
        assert!(ns.iter().all(|n| n.position + n.length <= 768), "cut at the pattern's end");
        assert!(ns.iter().any(|n| n.length < 72), "cut where the next note of its key starts");
    }

    #[test]
    fn grid_one_tick_keeps_four_ticks_apart() {
        let ns = generate(&Settings { glue: false, ..Settings::default() }, 0, 1, 1920);
        assert!(ns.len() > 1700, "{}", ns.len());
        let mut by_key = ns.clone();
        by_key.sort_by_key(|n| (n.key, n.position));
        assert!(by_key.windows(2).all(|w| w[0].key != w[1].key || w[1].position >= w[0].position + MIN_LEN));
        assert!(ns.iter().all(|n| n.length == MIN_LEN || n.position + n.length == 1920), "short only at the end");
    }

    #[test]
    fn glue_joins_touching_notes() {
        let set = Settings { range: 1, scale: 5, ..Settings::default() }; // three keys: neighbours often match
        let glued = generate(&set, 0, 24, 768);
        let plain = generate(&Settings { glue: false, ..set }, 0, 24, 768);
        assert!(glued.len() < plain.len() && glued.iter().any(|n| n.length >= 48));
        assert_eq!(glued.iter().map(|n| n.length).sum::<u32>(), 768, "nothing lost or doubled");
    }

    #[test]
    fn population_stack_and_porta() {
        let n = |set: Settings| generate(&Settings { glue: false, ..set }, 0, 4, 7680).len();
        assert_eq!(n(Settings::default()), 1920);
        let half = n(Settings { population: 0.5, ..Settings::default() });
        assert!((900..1100).contains(&half), "{half}");
        let least = n(Settings { population: 0.0, ..Settings::default() });
        assert!((5..50).contains(&least), "about 1 % at the bottom: {least}");
        let porta = generate(&Settings { porta: true, glue: false, ..Settings::default() }, 0, 4, 7680);
        let share = porta.iter().filter(|n| n.porta()).count();
        assert!((180..300).contains(&share), "about one in eight: {share}");
        let base = generate(&Settings { glue: false, ..Settings::default() }, 0, 4, 7680);
        assert_eq!(spans(&porta), spans(&base), "portamento changes no key or length");
        let kept = generate(&Settings { population: 0.5, glue: false, ..Settings::default() }, 0, 4, 7680);
        assert!(kept.iter().all(|n| base.iter().any(|b| b.position == n.position && b.key == n.key)), "population drops notes, keys stay");
        let st = generate(&Settings { stack: 3, glue: false, ..Settings::default() }, 0, 24, 384);
        assert!(st.iter().all(|n| n.position % 24 == 0) && st.len() > 16 && st.len() <= 48);
    }

    #[test]
    fn levels_move_around_the_value() {
        let mid: Vec<Note> = (0..32).map(|i| Note { velocity: 64, length: 24, ..Note::step(0, 24 * i) }).collect();
        let set = Settings { pattern: false, ..Settings::default() };
        assert_eq!(randomize(mid.clone(), &set, 0, 24, 768), mid, "every knob at 0: nothing");
        let vel = |set: Settings| levels(mid.clone(), &set).iter().map(|n| n.velocity).collect::<Vec<_>>();
        let mut amounts = [0.0; 6];
        amounts[1] = 6.0 / 64.0;
        let v = vel(Settings { amounts, ..set });
        assert!(v.iter().all(|&x| (58..=70).contains(&x)) && v.iter().any(|&x| x != 64), "{v:?}");
        amounts[1] = 1.0;
        let v = vel(Settings { amounts, ..set });
        assert!(v.iter().all(|&x| x <= 128) && v.iter().any(|&x| x < 20) && v.iter().any(|&x| x > 108));
        let v = vel(Settings { amounts, bipolar: false, ..set });
        assert!(v.iter().all(|&x| x >= 64), "one way");
        let v = vel(Settings { amounts, bipolar: false, reset_before: true, ..set });
        assert!(v.iter().all(|&x| x >= 100), "from a new note's 100");
        amounts[1] = -1.0;
        let v = vel(Settings { amounts, bipolar: false, ..set });
        assert!(v.iter().all(|&x| x <= 64));
        let ns = levels(mid.clone(), &Settings { amounts: [1.0; 6], ..set });
        assert!(ns.iter().all(|n| n.pan <= 128 && n.release <= 128 && n.fine_pitch <= 240) && ns.iter().any(|n| n.mod_x > 200));
        assert_eq!(levels(mid.clone(), &Settings { amounts: [1.0; 6], levels: false, ..set }), mid, "Levels off");
        let a = levels(mid.clone(), &Settings { amounts, ..set });
        assert_ne!(a, levels(mid.clone(), &Settings { amounts, levels_seed: 1, ..set }));
        assert_eq!(levels(mid[..5].to_vec(), &Settings { amounts, ..set }), a[..5], "the draws follow the notes' order");
    }

    #[test]
    fn hints_and_reset() {
        let mut set = Settings { pattern_seed: 7, levels_seed: 2, octave: 6, ..Settings::default() };
        assert_eq!((set.length_text(), set.variation_text(), set.population_text(), set.amount_text(1)), ("Length: 1 step".into(), "Variation: 0 steps".into(), "Population: 100%".into(), "0%".into()));
        set.set_length_pos(set.length_pos() + LENGTH_NOTCH);
        set.set_variation_pos(set.variation_pos() + VARIATION_NOTCH);
        set.amounts[1] = -0.5;
        assert_eq!((set.length_text(), set.variation_text(), set.amount_text(1)), ("Length: 1.08 steps".into(), "Variation: +0.17 steps".into(), "-50%".into()));
        assert_eq!(set.reset(), Settings { pattern_seed: 7, levels_seed: 2, ..Settings::default() });
        assert_eq!(set.scale_name(), "(none)");
    }
}
