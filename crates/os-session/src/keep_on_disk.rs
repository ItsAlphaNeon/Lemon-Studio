// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! General ▸ Advanced ▸ Auto keep long audio on disk (`prefs::AUTO_KEEP_ON_DISK`): a file of 30 seconds or more loaded
//! into a sampler channel turns on the channel's Keep on disk switch (sampler flags 0x100, Lemon Sampler's
//! "keep_disk"), by its length in time (frames / rate), whatever its rate and channels. A load never turns it off.
//! It comes with the paths that read the file into the channel (`rack::read_tags`: Load sample..., a drop on the rack
//! or on a plugin window, the browser's send), inside their "Load channel sample(s)" history step; project loading and
//! previews don't.
// spec: ui/settings-choices.md "Advanced: Fast sample preview, Auto keep long audio on disk, Threaded project sample
// loading (2026-10-16)"

#![allow(deprecated)] // writes the project directly (`crate::locked`; control-plane step 14 removes this)
use std::path::Path;

use os_model::ChannelSound;

/// The shortest file kept on disk, in seconds.
pub const LONG_SECS: u64 = 30;

/// Sample `path` went into sampler channel `ch`: while the check is on, a file of `LONG_SECS` or more turns Keep on
/// disk on (the channel's other sampler flags stay; a channel without them gets those of a new channel).
pub fn keep_long(st: &crate::settings_file::SettingsStore, ch: &mut os_model::Channel, path: &Path) {
    if !ch.kind.is_sampler() || !st.check(&crate::prefs::AUTO_KEEP_ON_DISK) {
        return;
    }
    let Some((frames, rate)) = os_engine::sample::length(path) else { return };
    if rate > 0 && frames >= LONG_SECS * u64::from(rate) {
        let s = &mut ch.sound;
        s.sampler_flags = Some(s.sampler_flags.unwrap_or(ChannelSound::DEFAULT_FLAGS) | ChannelSound::KEEP_ON_DISK);
    }
}

