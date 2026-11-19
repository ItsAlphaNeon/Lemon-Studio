// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon SoundFont Player in Lemon Studio: compiled in like Lemon Drum Sampler, it plays every SoundFont player
//! channel of a project (started from the state the file holds, its font opened from the stored path) and every one
//! added from the picker; saving writes the state it plays back into the channel byte for byte, so the file keeps its
//! own plugin with its font, preset and controls.
//!
// spec: plugins/lemon-soundfont.md §5 (app integration)

use os_model::{Channel, ChannelKind, Project};

use crate::plugins::{Plugins, FX_KEY};

/// The built-in plugin's name (picker, window title).
pub const LEMON_SOUNDFONT: &str = "Lemon SoundFont Player";
/// The plugin id new channels store for it (event 201): the 32-bit player's, which the 64-bit player opens too.
pub const FILE_NAME: &str = "Fruity soundfont player";
/// The 64-bit player's own id.
pub const FILE_NAME_64: &str = "SoundFont Player";

/// Whether file id `id` names the original's SoundFont player (either spelling, any case).
pub fn is_file_name(id: &str) -> bool {
    id.eq_ignore_ascii_case(FILE_NAME) || id.eq_ignore_ascii_case(FILE_NAME_64)
}

/// Whether a channel holds the file's SoundFont player.
pub fn is_soundfont(ch: &Channel) -> bool {
    ch.kind == ChannelKind::Plugin && ch.plugin.as_deref().is_some_and(is_file_name)
}

impl Plugins {
    /// The state the Lemon SoundFont Player on rack row `row` plays, `None` when the row hosts something else.
    pub fn soundfont_file_state(&self, row: usize) -> Option<Vec<u8>> {
        let h = self.hosted.get(&row)?;
        let p = h.lock_for_state()?;
        if p.desc().name != LEMON_SOUNDFONT || !crate::project_plugins::is_builtin(p.desc()) {
            return None;
        }
        lemon_soundfont::params::fl_from_plugin_state(&p.save_state()?)
    }
}

/// Before a save: every hosted Lemon SoundFont Player writes the state it plays into its channel (a channel keeps its
/// own spelling of the id).
pub fn store(proj: &mut Project, plugins: &Plugins) {
    for row in plugins.hosted.range(..FX_KEY).map(|(&k, _)| k).collect::<Vec<_>>() {
        let Some(st) = plugins.soundfont_file_state(row) else { continue };
        if let Some(ch) = proj.channels.get_mut(row).filter(|c| c.kind == ChannelKind::Plugin) {
            if !ch.plugin.as_deref().is_some_and(is_file_name) {
                ch.plugin = Some(FILE_NAME.into());
            }
            ch.plugin_state = Some(st);
        }
    }
}

/// The plugin state for a channel's state: the stored font path resolved for the plugin to open (placeholders made
/// real), the stored path kept in the state.
pub fn plugin_state(fl: &[u8]) -> Vec<u8> {
    let stored = lemon_soundfont::params::stored_path(fl);
    let path = if stored.is_empty() { String::new() } else { crate::plugins::resolve_sample(&lemon_soundfont::resolve(&stored)) };
    lemon_soundfont::params::plugin_state_from_fl(fl, &path)
}

impl crate::app::App {
    /// Hosts Lemon SoundFont Player on every SoundFont player channel without an instance, started from the channel's
    /// saved state. A row waiting for the picker's own instance is left to it; a row whose instance failed is not
    /// tried again.
    pub(crate) fn sync_soundfont(&mut self) {
        if crate::project_plugins::native_first() {
            return;
        }
        let Some(index) = self.plugins.index_of(LEMON_SOUNDFONT) else { return };
        let pending = self.state.pending_plugin.map(|(r, _)| r);
        let rows: Vec<usize> = (0..self.state.project.channels.len())
            .filter(|&r| is_soundfont(&self.state.project.channels[r]))
            .filter(|&r| !self.state.hosted_rows.contains(&r) && !self.wrap_failed.contains(&r) && Some(r) != pending)
            .collect();
        if rows.is_empty() {
            return;
        }
        let mut added = false;
        for row in rows {
            if self.past_deadline() {
                break;
            }
            let fl = self.state.project.channels[row].plugin_state.clone().unwrap_or_default();
            match self.plugins.create_with(row, index, Some(&plugin_state(&fl))) {
                Ok(_) => {
                    self.state.hosted_rows.push(row);
                    added = true;
                }
                Err(e) => {
                    self.wrap_failed.push(row);
                    self.state.message = Some(format!("{LEMON_SOUNDFONT}: {e}"));
                }
            }
        }
        if added {
            self.transport.set_instruments(self.plugins.instruments());
            self.state.piano.dirty = true;
        }
        self.invalidate();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_ids_name_the_player() {
        assert!(is_file_name("Fruity soundfont player"));
        assert!(is_file_name("SoundFont Player"));
        assert!(is_file_name("fruity SoundFont Player"));
        assert!(!is_file_name("Fruity Slicer"));
    }

    /// A channel's state reaches the plugin whole, and its font path is resolved for opening.
    #[test]
    fn the_channel_state_reaches_the_plugin() {
        let mut st = lemon_soundfont::state::State::default();
        st.set_path("C:\\fonts\\piano.sf2");
        let fl = st.to_bytes();
        let blob = plugin_state(&fl);
        assert_eq!(lemon_soundfont::params::fl_from_plugin_state(&blob), Some(fl));
        assert!(String::from_utf8_lossy(&blob).contains("piano.sf2"));
    }
}
