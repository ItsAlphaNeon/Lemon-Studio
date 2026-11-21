// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The interface's side of `crate::soundfont` (the session's side stays there).

impl crate::app::App {
    /// Hosts Lemon SoundFont Player on every SoundFont player channel without an instance, started from the channel's
    /// saved state. A row waiting for the picker's own instance is left to it; a row whose instance failed is not tried
    /// again.
    pub fn sync_soundfont(&mut self) {
        self.host(crate::soundfont::sync_soundfont);
    }
}
