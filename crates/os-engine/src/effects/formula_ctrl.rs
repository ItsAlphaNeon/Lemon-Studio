// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! A project's Fruity Formula Controller slots, played by Lemon Formula Controller's processing: a formula of three
//! knobs, the song position, the tempo and the system's clock, worked out once per tick, puts out one controller value
//! for the links that read it (`engine/controllers.rs`). The sound passes through.
//!
// spec: plugins/fruity-formula-controller.md, plugins/lemon-formula-controller.md

use super::Effect;
use crate::formula::{Error, Formula, Rng, Vars};
use crate::instrument::TransportInfo;

/// The state's version (the first int).
pub const VERSION: i32 = 4;
/// The knobs' full value (1.0).
pub const KNOB_ONE: i32 = 65536;

/// A new instance's comments (spec: plugins/fruity-formula-controller.md "State").
const NEW_COMMENTS: &str = "Basic addition\routput = a knob + b knob + c knob";

/// What a new instance's state holds after the knobs: no formula (a 0 length), the comments (their length in UTF-16
/// units, the text), a 0.
pub const NEW_AFTER_KNOBS: [u8; 3 + 2 * NEW_COMMENTS.len()] = {
    let s = NEW_COMMENTS.as_bytes();
    let mut v = [0u8; 3 + 2 * NEW_COMMENTS.len()];
    v[1] = s.len() as u8;
    let mut i = 0;
    while i < s.len() {
        v[2 + 2 * i] = s[i];
        i += 1;
    }
    v
};

/// The settings as the state stores them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// The knobs a, b and c, 0..65536 = 0..1.
    pub knobs: [i32; 3],
    /// The formula as stored (whether it compiles or not).
    pub formula: String,
    /// The comments, as stored: a length byte, that many UTF-16 units, then the rest of the state.
    pub tail: Vec<u8>,
}

impl Default for Settings {
    /// A new instance's.
    fn default() -> Self {
        Settings { knobs: [0; 3], formula: String::new(), tail: NEW_AFTER_KNOBS[1..].to_vec() }
    }
}

impl Settings {
    /// Reads a saved state: the version, the knobs, the formula as a length byte and that many Latin-1 bytes, then the
    /// comments and what follows them, kept as stored. A state too short for its formula is not one.
    pub fn from_state(state: &[u8]) -> Option<Settings> {
        let v = super::ints::<4>(state, 0)?;
        let n = usize::from(*state.get(16)?);
        let formula = state.get(17..17 + n)?.iter().map(|&b| char::from(b)).collect();
        Some(Settings { knobs: [v[1], v[2], v[3]], formula, tail: state[17 + n..].to_vec() })
    }

    /// The state the original saves.
    pub fn to_state(&self) -> Vec<u8> {
        let mut v = super::state_of(&[VERSION, self.knobs[0], self.knobs[1], self.knobs[2]]);
        let f = latin1(&self.formula);
        v.push(f.len() as u8);
        v.extend(f);
        v.extend(&self.tail);
        v
    }

    /// The comments' text.
    pub fn comments(&self) -> String {
        let n = usize::from(self.tail.first().copied().unwrap_or(0));
        let units: Vec<u16> = self.tail.get(1..1 + 2 * n).unwrap_or(&[]).chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        String::from_utf16_lossy(&units)
    }

    /// The knob `k` as 0..1.
    pub fn knob(&self, k: usize) -> f64 {
        f64::from(self.knobs[k]) / f64::from(KNOB_ONE)
    }
}

/// `s` as the state stores a formula: Latin-1, at most 255 bytes (other characters become '?').
pub fn latin1(s: &str) -> Vec<u8> {
    s.chars().map(|c| u8::try_from(u32::from(c)).unwrap_or(b'?')).take(255).collect()
}

/// The clock as the formulas see it: `Time`, milliseconds of a steady count (the original counts from the system's
/// start, Lemon Studio from the program's first formula: only differences and fractions of it mean anything), and
/// `Date`, the whole days since 1899-12-30 (UTC; the original's has no time of day in it).
pub fn clock_now() -> (f64, f64) {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    let ms = START.get_or_init(std::time::Instant::now).elapsed().as_secs_f64() * 1000.0;
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64());
    (ms, (secs / 86400.0).floor() + 25569.0)
}

/// The formula and its variables, worked out once per tick.
#[derive(Debug, Clone)]
pub struct Core {
    pub s: Settings,
    formula: Result<Formula, Error>,
    rng: Rng,
    out: Option<f64>,
}

impl Core {
    pub fn new(s: Settings) -> Core {
        let formula = Formula::compile(&s.formula);
        let mut c = Core { s, formula, rng: Rng::default(), out: None };
        c.out = c.value(0.0, 120.0);
        c
    }

    /// New settings (a new formula is compiled).
    pub fn set(&mut self, s: Settings) {
        if s.formula != self.s.formula {
            self.formula = Formula::compile(&s.formula);
        }
        self.s = s;
    }

    /// Whether the formula compiles, and what is wrong with it if not ("No formula" for an empty one).
    pub fn compiled(&self) -> Result<(), &Error> {
        self.formula.as_ref().map(|_| ())
    }

    /// The knobs a, b and c as stored (an edit takes effect at the next tick).
    pub fn set_knobs(&mut self, knobs: [i32; 3]) {
        self.s.knobs = knobs;
    }

    /// The formula's value (0..1) at song position `beats` and `tempo`: 0 without a formula, nothing (the targets are
    /// left as they are) for one that does not compile (spec "Output").
    pub fn value(&mut self, beats: f64, tempo: f64) -> Option<f64> {
        let f = match &self.formula {
            Ok(f) => f,
            Err(Error::Empty) => return Some(0.0),
            Err(_) => return None,
        };
        let (time_ms, date) = clock_now();
        let v = Vars { a: self.s.knob(0), b: self.s.knob(1), c: self.s.knob(2), song_time: beats, tempo, time_ms, date, ..Vars::default() };
        Some(f.value(&v, &mut self.rng))
    }

    /// The tick starting at `beats`: the output the links read from the next tick on.
    pub fn advance(&mut self, beats: f64, tempo: f64) {
        self.out = self.value(beats, tempo);
    }

    /// The output, if there is one.
    pub fn output(&self) -> Option<f64> {
        self.out
    }
}

/// A Fruity Formula Controller slot in the engine, which calls `process` once per tick.
pub struct FormulaController {
    core: Core,
    /// The next tick's song position in beats, and the tempo.
    next: (f64, f64),
}

impl FormulaController {
    pub fn from_state(state: &[u8]) -> Option<Self> {
        Some(FormulaController { core: Core::new(Settings::from_state(state)?), next: (0.0, 120.0) })
    }
}

impl Effect for FormulaController {
    /// Its formula runs on the song, not on the sound: never bypassed.
    fn quiet_hold(&self) -> Option<f32> {
        None
    }
    fn transport(&mut self, t: &TransportInfo, (tick, ppq): (u64, u16)) {
        self.next = ((tick + 1) as f64 / f64::from(ppq.max(1)), t.tempo);
    }

    /// Works the formula out for the next tick: its value acts on the targets from there (the original's targets take
    /// the value of their tick's own start).
    fn process(&mut self, _buf: &mut [f32]) {
        self.core.advance(self.next.0, self.next.1);
    }

    fn reset(&mut self) {}

    fn control(&self, k: usize) -> Option<f32> {
        self.core.output().filter(|_| k == 0).map(|v| v as f32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The LIFE demo project's slot: version 4, the knobs, no formula, a comment (spec: plugins/fruity-formula-controller.md
    /// "State").
    #[test]
    fn states_round_trip() {
        let comment = "Randomness\ra=base level, b=range";
        let mut st = state_of(&[4, 46592, 65536, 32768]);
        st.push(0);
        st.push(comment.encode_utf16().count() as u8);
        st.extend(comment.encode_utf16().flat_map(u16::to_le_bytes));
        st.push(0);
        let s = Settings::from_state(&st).unwrap();
        assert_eq!((s.knobs, s.formula.as_str(), s.comments()), ([46592, 65536, 32768], "", comment.to_string()));
        assert_eq!(s.to_state(), st);
        let with = Settings { formula: "a*b+c".into(), ..s };
        let b = with.to_state();
        assert_eq!(&b[16..22], b"\x05a*b+c");
        assert_eq!(Settings::from_state(&b), Some(with));
        assert!(Settings::from_state(&b[..18]).is_none());
        // A new instance: what the original saves for a slot that had no state (115 bytes, no formula).
        let new = Settings::default();
        let want = "Basic addition\routput = a knob + b knob + c knob";
        assert_eq!((new.to_state().len(), new.formula.as_str(), new.comments()), (115, "", want.to_string()));
        assert_eq!(new.to_state()[114], 0);
    }

    fn state_of(v: &[i32]) -> Vec<u8> {
        v.iter().flat_map(|x| x.to_le_bytes()).collect()
    }

    /// The formula of the tick's start position, read by the links from the next tick on.
    #[test]
    fn works_on_the_ticks() {
        let s = Settings { knobs: [16384, 0, 0], formula: "a+SongTime/48".into(), ..Settings::default() };
        let mut fc = FormulaController::from_state(&s.to_state()).unwrap();
        assert_eq!(fc.control(0), Some(0.25));
        let t = TransportInfo { playing: true, tempo: 120.0, frame: 0, beats: 0.0, bar_start: 0.0, bar: 0, numerator: 4, denominator: 4, looping: None };
        fc.transport(&t, (95, 96));
        fc.process(&mut [0.0; 8]);
        assert_eq!(fc.control(0), Some(0.25 + 1.0 / 48.0));
        assert_eq!(fc.control(1), None);
        // No formula: 0. One that does not compile: no output, the targets keep their values.
        let empty = Settings { knobs: [16384, 0, 0], ..Settings::default() };
        assert_eq!(FormulaController::from_state(&empty.to_state()).unwrap().control(0), Some(0.0));
        let bad = Settings { formula: "a*".into(), ..Settings::default() };
        assert_eq!(FormulaController::from_state(&bad.to_state()).unwrap().control(0), None);
    }
}
