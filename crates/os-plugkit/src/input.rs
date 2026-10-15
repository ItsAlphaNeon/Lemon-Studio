// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

//! What an editor window hands its GUI thread on Linux and macOS (`x11`, `mac`): one input type for both, so the plugins
//! run the same editor loop on either. Keys are X11 key symbols on both.

/// Key symbols the editors and boxes look at (`<X11/keysymdef.h>`).
pub mod keysym {
    pub const BACKSPACE: u32 = 0xff08;
    pub const RETURN: u32 = 0xff0d;
    pub const ESCAPE: u32 = 0xff1b;
    pub const HOME: u32 = 0xff50;
    pub const LEFT: u32 = 0xff51;
    pub const UP: u32 = 0xff52;
    pub const RIGHT: u32 = 0xff53;
    pub const DOWN: u32 = 0xff54;
    pub const END: u32 = 0xff57;
    pub const KP_ENTER: u32 = 0xff8d;
    pub const DELETE: u32 = 0xffff;
}

/// The character a key symbol types, if any.
pub fn keysym_char(sym: u32) -> Option<char> {
    match sym {
        0x20..=0x7e | 0xa0..=0xff => char::from_u32(sym),
        0xffb0..=0xffb9 => char::from_digit(sym - 0xffb0, 10),
        0xffaa => Some('*'),
        0xffab => Some('+'),
        0xffad => Some('-'),
        0xffae => Some('.'),
        0xffaf => Some('/'),
        // Unicode key symbols.
        0x0100_0100..=0x0110_ffff => char::from_u32(sym - 0x0100_0000),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Left,
    Right,
    Middle,
}

/// What happened in the window. Positions are the window's device px.
#[derive(Clone, Debug, PartialEq)]
pub enum Input {
    Down { x: f32, y: f32, btn: Button, dbl: bool },
    Up { btn: Button },
    Move { x: f32, y: f32 },
    /// The held cursor moved this far (device px; `lock`).
    Delta { dx: f32, dy: f32 },
    /// Notches, up positive.
    Wheel { x: f32, y: f32, notches: f32 },
    Leave,
    Key { sym: u32, text: Option<char> },
    /// Part of the window needs painting.
    Expose,
    /// The polling period passed, or a parameter changed.
    Tick,
    /// A file dropped on the window (macOS so far: `mac::Window::accept_files`).
    Drop { path: String },
}
