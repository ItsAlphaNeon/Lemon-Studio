// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The channel's volume and pan from Lemon Studio, as a Lemon instrument's SysEx message: the instrument works them
//! into its voices' final levels. Another host sends none, so its levels act on the output.
// spec: plugins/host-link.md "Channel levels"

use nih_plug::prelude::SysExMessage;

/// The channel's volume and pan (`os_hostlink::levels`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Levels(pub f32, pub f32);

impl SysExMessage for Levels {
    type Buffer = [u8; os_hostlink::levels::LEN];

    fn from_buffer(buffer: &[u8]) -> Option<Self> {
        os_hostlink::levels::decode(buffer).map(|(volume, pan)| Levels(volume, pan))
    }

    fn to_buffer(self) -> (Self::Buffer, usize) {
        (os_hostlink::levels::encode(self.0, self.1), os_hostlink::levels::LEN)
    }
}
