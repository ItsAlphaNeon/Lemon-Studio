// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The interface's side of `crate::mix_threads` (step 9: the session's side stays there).

#![allow(unused_imports)]
use crate::mix_threads::*;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mix_threads::physical_cores;
    use crate::prefs::{MULTITHREADED_GENERATORS, MULTITHREADED_MIXER};
    use os_engine::parallel;
    use crate::prefs::set_check;

    /// The machine's physical cores are found, no more than its logical ones.
    #[test]
    fn physical_cores_are_found() {
        let logical = std::thread::available_parallelism().map_or(1, |n| n.get());
        if let Some(n) = physical_cores() {
            assert!((1..=logical).contains(&n), "{n} of {logical}");
        }
    }

    // spec: audio/multicore.md "Settings"
    /// The two switches reach the engines: on by default, each turned off on its own by a click on its check.
    #[test]
    fn the_switches_reach_the_engines() {
        apply();
        assert_eq!((parallel::mixer_on(), parallel::generators_on()), (true, true));
        let mut app = crate::app::App::new(None);
        app.pref_toggle(MULTITHREADED_MIXER.label);
        assert_eq!((parallel::mixer_on(), parallel::generators_on()), (false, true));
        app.pref_toggle(MULTITHREADED_MIXER.label);
        app.pref_toggle(MULTITHREADED_GENERATORS.label);
        assert_eq!((parallel::mixer_on(), parallel::generators_on()), (true, false));
        set_check(&MULTITHREADED_GENERATORS, true);
        apply();
    }
}
