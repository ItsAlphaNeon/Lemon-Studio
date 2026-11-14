// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The song position slider in the toolbar's main panel (spec: ui/song-position-slider.md): where the song position
//! is within the song (SONG mode) or the pattern (PAT mode), and a way to move it.

use std::time::Duration;


/// The handle's centre runs from `X0` (position 0) to `X0 + W` (the end of the range) (§1).
pub const X0: f32 = 395.0;
pub const W: f32 = 170.0;
/// Half the handle's width: a press this close to its centre grabs it.
pub const GRAB: f32 = 6.5;
/// Holding the button on the track: the first repeat, then the repeat interval (§4).
pub(crate) const FIRST_REPEAT: Duration = Duration::from_millis(250);
pub(crate) const REPEAT: Duration = Duration::from_millis(60);

/// The position under `x`, clamped to 0 … range − 1 tick.
pub fn tick_at(x: f32, range: u32) -> u32 {
    let t = ((x - X0) / W * range as f32).round().max(0.0) as u32;
    t.min(range.saturating_sub(1))
}
