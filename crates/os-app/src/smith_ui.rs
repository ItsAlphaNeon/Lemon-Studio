// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The interface's side of `crate::smith` (step 9: the session's side stays there).

#![allow(unused_imports)]
#![allow(deprecated)] // writes the project directly (`crate::locked`; control-plane step 14 removes this)
use std::path::{Path, PathBuf};
use os_model::{Channel, ChannelKind, Project};
use crate::plugins::{Plugins, FX_KEY};
#[allow(unused_imports)]
use crate::smith::*;

impl crate::app::App {
    /// Hosts Smith on every Ogun channel without an instance, started from the channel's saved state (the default patch
    /// when the channel has none). A row waiting for the picker's own instance is left to it; a row whose instance
    /// failed is not tried again.
    pub(crate) fn sync_smith(&mut self) {
        self.host(sync_smith);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use os_engine::SharedInstrument;
    use smith::state::{State, RICH};

    fn patch() -> Vec<u8> {
        let mut st = State::default();
        st.ints[RICH] = 9;
        st.to_bytes()
    }

    /// An Ogun channel added in the app saves no state while it plays the default patch; one loaded with a patch saves
    /// that patch's bytes, and the reopened project hosts it again from them.
    #[test]
    fn ogun_channels_save_and_reopen() {
        let path = std::env::temp_dir().join(format!("openstudio-smith-{}.flp", std::process::id()));
        let mut a = App::new(None);
        a.state.session.project = crate::locked::Locked::new(crate::projects::test_project());
        a.state.session.file = None;
        a.state.session.path = None;
        a.state.session.plugins.available = crate::plugins::builtin();
        let row = crate::projects::add_plugin_channel(a.state.session.project.get_mut(), SMITH);
        let ch = &a.state.session.project.channels[row];
        assert_eq!((ch.kind, ch.plugin.as_deref()), (ChannelKind::Plugin, Some(FILE_NAME)));
        assert_eq!(crate::project_plugins::plugin_title(ch).as_deref(), Some(SMITH));
        a.sync_smith();
        assert!(a.state.session.hosted_rows.contains(&row));
        assert_eq!(a.state.session.plugins.ogun_file_state(row), Some(Vec::new()));
        a.save_to(path.clone());
        let saved = os_flp::project::load(&os_flp::FlpFile::parse(&std::fs::read(&path).unwrap()).unwrap()).unwrap();
        assert_eq!((saved.channels[row].plugin.as_deref(), saved.channels[row].plugin_state.as_deref()), (Some(FILE_NAME), None));

        // A patch goes in (as a preset load does), the project is saved and reopened.
        let fl = patch();
        assert!(a.state.session.plugins.set_state(row, &smith::plugin_state_from_fl(&fl)));
        a.save_to(path.clone());
        let saved = os_flp::project::load(&os_flp::FlpFile::parse(&std::fs::read(&path).unwrap()).unwrap()).unwrap();
        assert_eq!(saved.channels[row].plugin_state.as_deref(), Some(fl.as_slice()));
        let mut b = App::new(None);
        b.state.session.plugins.available = crate::plugins::builtin();
        b.open(path.clone());
        b.sync_smith();
        assert!(b.state.session.hosted_rows.contains(&row), "{:?}", b.state.message);
        assert_eq!(b.state.session.plugins.ogun_file_state(row), Some(fl.clone()));
        b.save_to(path.clone());
        let again = os_flp::project::load(&os_flp::FlpFile::parse(&std::fs::read(&path).unwrap()).unwrap()).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(again.channels[row], saved.channels[row]);
    }

}
