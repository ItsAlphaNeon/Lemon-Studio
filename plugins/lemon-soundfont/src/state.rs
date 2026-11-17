// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The original player's saved state, kept whole so what the player does not change comes back out byte for byte.
//! Two layouts are known: version 2 (the 32-bit player's, which also stores the program and bank) and version 6 (the
//! 64-bit player's, which stores the preset's place in the font's sorted list after the path). Both keep the font's
//! path as a length-prefixed string.
//!
// spec: plugins/lemon-soundfont.md §1

use crate::sf2::Font;

/// The fixed head before the path in both versions.
const HEAD: usize = 56;
/// The bytes after the path in a version 2 state.
const TAIL_V2: [u8; 11] = [0xFF, 0xFF, 0xFF, 0xFF, 0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0x00, 0x00];
/// Where a version 6 state keeps the preset's place, counted from the end of the path.
const V6_INDEX: usize = 21;

/// The player's own controls, each a 32-bit value in the state's head.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Control {
    ReverbLevel,
    ChorusLevel,
    Mod,
    Attack,
    Decay,
    Sustain,
    Release,
    LfoDelay,
    LfoAmount,
    LfoSpeed,
    Cutoff,
}

impl Control {
    pub const ALL: [Control; 11] = [
        Control::ReverbLevel, Control::ChorusLevel, Control::Mod, Control::Attack, Control::Decay, Control::Sustain, Control::Release,
        Control::LfoDelay, Control::LfoAmount, Control::LfoSpeed, Control::Cutoff,
    ];

    fn offset(self) -> usize {
        match self {
            Control::ReverbLevel => 12,
            Control::ChorusLevel => 16,
            Control::Mod => 20,
            Control::Attack => 24,
            Control::Decay => 28,
            Control::Sustain => 32,
            Control::Release => 36,
            Control::LfoDelay => 40,
            Control::LfoAmount => 44,
            Control::LfoSpeed => 48,
            Control::Cutoff => 52,
        }
    }

    /// The value range when on.
    pub fn range(self) -> (i32, i32) {
        match self {
            Control::ReverbLevel | Control::ChorusLevel => (0, 2560),
            Control::Attack | Control::Decay | Control::Release => (0, 5940),
            Control::LfoDelay => (0, 5900),
            Control::LfoAmount => (-127, 127),
            Control::Mod | Control::Sustain | Control::LfoSpeed | Control::Cutoff => (0, 127),
        }
    }

    /// The stored value meaning off (the font's value plays).
    pub fn off(self) -> i32 {
        match self {
            Control::LfoAmount => -128,
            Control::Mod => 0,
            _ => -1,
        }
    }

    /// Whether the control can be off.
    pub fn can_be_off(self) -> bool {
        !matches!(self, Control::Mod | Control::ReverbLevel | Control::ChorusLevel)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct State {
    raw: Vec<u8>,
}

impl Default for State {
    /// A new player's state: version 2, program 1 of bank 0, both effect levels at 128, no font.
    fn default() -> Self {
        let mut raw = Vec::with_capacity(HEAD + 1 + TAIL_V2.len());
        for v in [2u32, 1, 0, 128, 128, 0] {
            raw.extend(v.to_le_bytes());
        }
        raw.extend([0xFF; 20]);
        raw.extend([0x80, 0xFF, 0xFF, 0xFF]);
        raw.extend([0xFF; 8]);
        raw.push(0);
        raw.extend(TAIL_V2);
        State { raw }
    }
}

impl State {
    pub fn parse(b: &[u8]) -> Option<State> {
        let s = State { raw: b.to_vec() };
        (s.version() >= 2 && b.len() > HEAD && HEAD + 1 + usize::from(b[HEAD]) <= b.len()).then_some(s)
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        self.raw.clone()
    }

    fn u32_at(&self, o: usize) -> u32 {
        self.raw.get(o..o + 4).map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn set_u32(&mut self, o: usize, v: u32) {
        if let Some(b) = self.raw.get_mut(o..o + 4) {
            b.copy_from_slice(&v.to_le_bytes());
        }
    }

    pub fn version(&self) -> u32 {
        self.u32_at(0)
    }

    fn path_end(&self) -> usize {
        HEAD + 1 + usize::from(self.raw[HEAD])
    }

    /// The font's path as stored (FL may write `%USERPROFILE%`-style placeholders).
    pub fn path(&self) -> String {
        self.raw[HEAD + 1..self.path_end()].iter().map(|&c| c as char).collect()
    }

    pub fn set_path(&mut self, p: &str) {
        let bytes: Vec<u8> = p.chars().map(|c| if (c as u32) < 256 { c as u8 } else { b'?' }).take(255).collect();
        let end = self.path_end();
        let mut v = self.raw[..HEAD].to_vec();
        v.push(bytes.len() as u8);
        v.extend(bytes);
        v.extend_from_slice(&self.raw[end..]);
        self.raw = v;
    }

    /// The effect send levels (reverb, chorus), 0..=128.
    pub fn levels(&self) -> (u32, u32) {
        (self.u32_at(12), self.u32_at(16))
    }

    /// A control's stored value (`None` = off: the font's value plays).
    pub fn control(&self, c: Control) -> Option<i32> {
        let v = self.u32_at(c.offset()) as i32;
        let (lo, hi) = c.range();
        (v >= lo).then_some(v.min(hi))
    }

    pub fn set_control(&mut self, c: Control, v: Option<i32>) {
        let (lo, hi) = c.range();
        let raw = match v {
            Some(v) => v.clamp(lo, hi),
            None => c.off(),
        };
        self.set_u32(c.offset(), raw as u32);
    }

    /// The mixer track effect `fx` (0 reverb, 1 chorus) sends to (−1 = none).
    pub fn send(&self, fx: usize) -> i32 {
        self.u32_at(self.path_end() + 5 * fx) as i32
    }

    pub fn set_send(&mut self, fx: usize, track: i32) {
        let o = self.path_end() + 5 * fx;
        self.set_u32(o, track.max(-1) as u32);
    }

    /// Whether effect `fx` (0 reverb, 1 chorus) uses the player's own.
    pub fn built_in(&self, fx: usize) -> bool {
        self.raw.get(self.path_end() + 4 + 5 * fx).is_some_and(|&b| b != 0)
    }

    pub fn set_built_in(&mut self, fx: usize, on: bool) {
        let o = self.path_end() + 4 + 5 * fx;
        if let Some(b) = self.raw.get_mut(o) {
            *b = u8::from(on);
        }
    }

    /// The player's controls as the voices take them.
    pub fn overrides(&self) -> crate::dsp::Overrides {
        crate::dsp::Overrides {
            attack: self.control(Control::Attack),
            decay: self.control(Control::Decay),
            sustain: self.control(Control::Sustain),
            release: self.control(Control::Release),
            lfo_delay: self.control(Control::LfoDelay),
            lfo_amount: self.control(Control::LfoAmount),
            lfo_speed: self.control(Control::LfoSpeed),
            cutoff: self.control(Control::Cutoff),
            mod_wheel: self.control(Control::Mod).unwrap_or(0),
        }
    }

    /// The preset this state names in `font`: by program and bank (version 2), by place (version 6).
    pub fn preset_in(&self, font: &Font) -> Option<usize> {
        if font.presets.is_empty() {
            return None;
        }
        if self.version() == 2 {
            let (prog, bank) = (self.u32_at(4).saturating_sub(1), self.u32_at(8));
            font.find(bank as u16, prog as u16).or(Some(0))
        } else {
            let i = self.u32_at(self.path_end() + V6_INDEX) as usize;
            Some(i.min(font.presets.len() - 1))
        }
    }

    pub fn set_preset(&mut self, font: &Font, i: usize) {
        let Some(p) = font.presets.get(i) else { return };
        if self.version() == 2 {
            self.set_u32(4, u32::from(p.program) + 1);
            self.set_u32(8, u32::from(p.bank));
        } else {
            let o = self.path_end() + V6_INDEX;
            self.set_u32(o, i as u32);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_state_is_the_originals_default_layout() {
        let s = State::default();
        assert_eq!(s.version(), 2);
        assert_eq!(s.path(), "");
        assert_eq!(s.to_bytes().len(), 68);
        let mut t = s.clone();
        t.set_path("C:\\fonts\\x.sf2");
        assert_eq!(t.path(), "C:\\fonts\\x.sf2");
        assert_eq!(&t.to_bytes()[t.to_bytes().len() - 11..], &TAIL_V2);
        assert_eq!(State::parse(&t.to_bytes()), Some(t));
    }

    /// Both versions' layouts as the originals wrote them.
    #[test]
    fn both_versions_name_their_preset() {
        let font = Font::parse(&crate::sf2::tests::font_bytes(&[0; 8], 44100, (0, 0), &[], 55)).unwrap();
        let mut v2 = State::default();
        v2.set_preset(&font, 0);
        assert_eq!(v2.u32_at(4), 56);
        assert_eq!(v2.preset_in(&font), Some(0));
        let mut raw = vec![6u8, 0, 0, 0, 1, 0, 0, 0];
        raw.extend([0u8; 4]);
        raw.extend([0x80, 0, 0, 0, 0x80, 0, 0, 0, 0, 0, 0, 0]);
        raw.extend([0xFF; 20]);
        raw.extend([1, 0, 0, 0]);
        raw.extend([0xFF; 8]);
        raw.push(3);
        raw.extend(b"a.b");
        raw.extend([0xFF, 0xFF, 0xFF, 0xFF, 0, 0xFF, 0xFF, 0xFF, 0xFF, 0, 0, 1, 1, 0xFF, 0xFF, 0xFF, 0xFF, 1, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 0, 0, 0, 0]);
        let v6 = State::parse(&raw).unwrap();
        assert_eq!(v6.path(), "a.b");
        assert_eq!(v6.preset_in(&font), Some(0));
    }
}
