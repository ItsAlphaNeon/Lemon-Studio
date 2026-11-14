// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The input ports of the hosted effects in the mixer's slots, as the project saves them, and attaching the effects to
//! an engine with them (spec: audio/sidechain.md "Hosted plugins' input buses").

use os_flp::wrapper::{self, WRAPPER};
use os_model::Project;

/// The plugin name a project file stores for Vocodex (file-format identifier, kept for interop): slots with it are
/// played and edited by Lemon Vocoder (spec: plugins/lemon-vocoder.md §6).
pub const VOCODEX_FILE_NAME: &str = "Vocodex";

/// The input ports a mixer slot's wrapped plugin is saved with: its main and first auxiliary input bus, each the number
/// of a track's input counted from 1, 0 for none (spec: audio/sidechain.md "Hosted plugins' input buses"). A Vocodex
/// slot (played by Lemon Vocoder) feeds its sidechain bus the input its carrier selector names, or else its modulator
/// selector's (spec: plugins/lemon-vocoder.md §1).
pub fn slot_ports(project: &Project, track: usize, slot: usize) -> (usize, usize) {
    let Some(s) = project.mixer.iter().find(|m| usize::from(m.index) == track).and_then(|m| m.slots.get(slot)?.as_ref()) else {
        return (0, 0);
    };
    if s.name.eq_ignore_ascii_case(VOCODEX_FILE_NAME) {
        let routing = os_engine::effects::vocoder::Settings::from_state(&s.state).unwrap_or_default().routing();
        return (0, routing.key_input());
    }
    if s.name != WRAPPER {
        return (0, 0);
    }
    let ports = wrapper::input_ports(&s.state);
    let port = |b: usize| ports.get(b).map_or(0, |&p| p.max(0) as usize);
    (port(0), port(1))
}

/// Attaches hosted effects to `e`, each with the input ports its slot is saved with in `project`.
pub fn attach_effects(e: &mut os_engine::Engine, project: &Project, fx: impl IntoIterator<Item = (usize, usize, os_engine::SharedEffect)>) {
    for (t, s, f) in fx {
        e.set_effect(t, s, f);
        let (main, aux) = slot_ports(project, t, s);
        e.set_effect_ports(t, s, main, aux);
    }
}
