// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Step edit (OPTIONS > Step edit, Ctrl+E, the toolbar's third mode button): while it is on and the transport is
//! stopped, the notes played on the typing keyboard or a MIDI keyboard are collected while any of them is held, and
//! written into the current pattern at the song position when the last one is released; then the position moves on
//! one step. The switch is a program setting, like its neighbours Auto scrolling and Enable groups: kept in
//! `settings.txt` (`step_edit`), off by default.
// spec: ui/step-edit.md, ui/global-snap.md "Step edit step"

#![allow(deprecated)] // writes the project directly (`crate::locked`; control-plane step 14 removes this)



/// The switch's key in `settings.txt`.
const KEY: &str = "step_edit";

/// The switch (missing: off).
pub fn on() -> bool {
    crate::settings_file::flag(KEY, false)
}

/// Toggles the switch and keeps it; returns the new state.
pub fn toggle() -> bool {
    let on = !on();
    crate::settings_file::set_flag(KEY, on);
    on
}

/// Where a held note comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Src {
    /// A typing-keyboard key, by the interface's id for it (`step_edit_ui::typing`).
    Typing(u64),
    /// A MIDI keyboard's key.
    Midi(u8),
    /// A MIDI key on the Omni preview MIDI channel (spec: ui/midi-settings.md §3).
    Omni(u8),
    /// A Touch controller pad or key (spec: ui/touch-controller.md §2).
    Touch(u8),
}

/// The chord being collected: the notes held now, and every key pressed since the first of them, with its velocity.
#[derive(Debug, Default)]
pub struct Chord {
    held: Vec<Src>,
    keys: Vec<(u16, u8)>,
}

impl Chord {
    /// A note went down: it is held, and its key joins the chord (once; a key pressed again takes the new velocity).
    pub fn down(&mut self, src: Src, key: u16, velocity: u8) {
        if !self.held.contains(&src) {
            self.held.push(src);
        }
        match self.keys.iter_mut().find(|k| k.0 == key) {
            Some(k) => k.1 = velocity,
            None => self.keys.push((key, velocity)),
        }
    }

    /// A note went up: when it was the last one held, the chord to write, in the order pressed. A note not collected
    /// (pressed while step editing was off or playing) gives nothing.
    pub fn up(&mut self, src: Src) -> Option<Vec<(u16, u8)>> {
        let i = self.held.iter().position(|&h| h == src)?;
        self.held.remove(i);
        self.held.is_empty().then(|| std::mem::take(&mut self.keys))
    }

    /// Forgets the held notes and the chord: nothing is written.
    pub fn clear(&mut self) {
        self.held.clear();
        self.keys.clear();
    }
}

