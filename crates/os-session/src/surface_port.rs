// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The control surfaces' ports as the session's MIDI input sees them: which system port is a recognised keyboard's
//! control port (`crate::surface` opens it; `crate::midi_in` leaves it alone).
// spec: controllers/launchkey-mk4.md

/// Whether `name` (a system MIDI port) is a control port this module opens.
pub fn claims(name: &str) -> bool {
    Model::of_port(name).is_some()
}

/// Which keyboard: the SysEx header's product byte and the controls it has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Model {
    /// Launchkey 25, 37, 49, 61 MK4: whether it has faders (49, 61).
    Mk4 { faders: bool },
    Mini,
}

impl Model {
    /// The model a DAW port's system name belongs to ("Launchkey MK4 61:Launchkey MK4 61 DAW In 24:1" on Linux,
    /// "MIDIIN2 (Launchkey MK4 61)" on Windows).
    pub fn of_port(name: &str) -> Option<Model> {
        let lower = name.to_ascii_lowercase();
        let daw = lower.contains(" daw") || lower.starts_with("midiin2 (") || lower.starts_with("midiout2 (");
        if !daw || !lower.contains("launchkey") || !lower.contains("mk4") {
            return None;
        }
        if lower.contains("mini") {
            return Some(Model::Mini);
        }
        Some(Model::Mk4 { faders: lower.contains("mk4 49") || lower.contains("mk4 61") })
    }

    /// Whether `name` is a Launchkey MK4's keys port (its "MIDI" port, the first on Windows).
    pub fn is_keys_port(name: &str) -> bool {
        let lower = name.to_ascii_lowercase();
        lower.contains("launchkey") && lower.contains("mk4") && Model::of_port(name).is_none()
    }

    pub fn product(self) -> u8 {
        match self {
            Model::Mk4 { .. } => 0x14,
            Model::Mini => 0x13,
        }
    }

    /// A SysEx message: Novation's header, `body`, the end.
    pub fn sysex(self, body: &[u8]) -> Vec<u8> {
        let mut m = vec![0xf0, 0x00, 0x20, 0x29, 0x02, self.product()];
        m.extend_from_slice(body);
        m.push(0xf7);
        m
    }

    /// The name Settings > MIDI shows as the device's controller type.
    pub fn type_name(self) -> &'static str {
        match self {
            Model::Mk4 { .. } => "Novation Launchkey MK4 DAW",
            Model::Mini => "Novation Launchkey Mini MK4 DAW",
        }
    }
}
