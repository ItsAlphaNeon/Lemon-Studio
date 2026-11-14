// Copyright (C) 2026 ItsAlphaNeon
// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! What moves with the startup logo elsewhere (`surface::intro`): the program's splash (`lemonstudio`) hands out
//! followers of its animation.

/// The logo's lemon (XRGB).
pub const LEMON: u32 = 0xffe066;

/// The keyboard's lemon (`surface::intro`), as the logo's.
pub fn lemon() -> os_model::Rgb {
    os_model::Rgb::new((LEMON >> 16) as u8, (LEMON >> 8) as u8, LEMON as u8)
}

/// A moment of the logo's animation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Moment {
    /// The logo's opacity, and how far its mark has popped (0..=1).
    pub alpha: f32,
    pub pop: f32,
    /// How far round the lemon is filled, 0..=1.
    pub progress: f32,
    /// The groove light's opacity, and where it points (degrees clockwise).
    pub light: f32,
    pub angle: f32,
}

/// A follower of the splash's animation, on any thread.
pub trait Follow: Send {
    /// The animation now; `None` once it has faded away, or the splash has been dropped.
    fn moment(&mut self) -> Option<Moment>;
}
