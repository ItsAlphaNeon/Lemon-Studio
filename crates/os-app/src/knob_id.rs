// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The knobs a change, an undo step or a controller link names (control-plane step 9: the session's; `crate::knobs`
//! keeps how they behave under the mouse, `KnobUi`), and the dialogs' own knobs they include.

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum KnobId {
    /// The toolbar's main volume (stored 0..16000, 12800 = unity at 0.8) and main pitch (-1200..1200 cents) (model
    /// `Project::main_volume` / `main_pitch`; spec: audio/mixer.md "Main volume", audio/voice-levels.md).
    MasterVol,
    MasterPitch,
    /// Mixer track pan (model).
    MixPan(u16),
    MixSep(u16),
    SlotMix(u16, u8),
    /// Send level from mixer track `.0` to `.1` (model `send_levels`, fader law; spec: ui/mixer.md §5).
    Send(u16, u16),
    /// Channel rack row / plugin window (model).
    ChanVol(usize),
    ChanPan(usize),
    /// Channel rack main swing, 0..100 % (spec: ui/channel-rack.md §7; playback: flp/events/project-info.md).
    Swing,
    /// Mixer track `.0`'s equaliser: band `.1` (0 low shelf, 1 peaking, 2 high shelf), setting `.2` (0 gain, the
    /// band's fader; 1 frequency; 2 width) (model `MixerTrack::eq`; spec: ui/mixer.md §8, audio/mixer.md "Equaliser").
    Eq(u16, u8, u8),
    /// A knob of the piano roll's Strum dialog (dialog state, not project data; spec: ui/piano-strum.md §1).
    Strum(crate::knob_id::strum::Knob),
    /// A knob (or the Range box) of the piano roll's Arpeggiate dialog (dialog state; spec: ui/piano-arpeggiate.md §1).
    Arp(crate::knob_id::arpeggiate::Knob),
    /// A knob or fader of the piano roll's Flam or Scale levels dialog (dialog state; spec: ui/piano-flam.md §1,
    /// ui/piano-level-scaling.md §1).
    PianoDlg(crate::knob_id::piano_dlg::Knob),
    /// A control of the LFO of the automation clip on channel row `.0` (model `Automation::lfo`; spec:
    /// audio/automation.md "The LFO"). Its switch and mode are two-position controls that a click flips.
    AutoLfo(usize, LfoKnob),
    /// A setting of the Lemon MIDI Out on channel row `.0` (its plugin state; spec: plugins/midi-out.md §6). The checks
    /// are two-position controls that a click flips.
    MidiOut(usize, crate::lemon_midi_out::MoKnob),
    /// The wet (`.3`) or dry level of member `.2` of the container in slot `.1` of track `.0`, 0..1 (the container's
    /// window, `crate::container`; Lemon Studio's design: spec plugins/patcher/README.md §4.3). It shows the level the
    /// chain plays, an automated one included.
    Member(u16, u8, u32, bool),
}

/// The controls of an automation clip's LFO, each over its stored range: switch (off, on), mode (multiply, add),
/// amount 0..128, speed 0..65536, tension, skew and pulse width -128..128 (Lemon Studio's ranges for the knobs; the
/// page gives the stored values' meaning).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum LfoKnob {
    On,
    Mode,
    Amount,
    Speed,
    Tension,
    Skew,
    Pulse,
}

impl KnobId {
    /// What the knob is as a target of the value box and of controller links: a container member's level is its own.
    pub fn target(self) -> crate::typein::Target {
        match self {
            KnobId::Member(t, k, id, wet) => crate::typein::Target::Member(t, k, id, wet),
            k => crate::typein::Target::Knob(k),
        }
    }
}

/// The `crate::arpeggiate` dialog's knobs.
pub mod arpeggiate {
    /// The highest Range; 0 is `---`.
    pub const RANGE_MAX: u8 = 6;

    /// The dialog's knobs (Range is a spin box, driven like a knob).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum Knob {
        TimeMul,
        Range,
        Gate,
        /// Index into `LEVELS`.
        Level(u8),
    }

    impl Knob {
        /// Value change per wheel notch: the stepped controls move one value.
        pub fn notch(self) -> f32 {
            match self {
                Knob::TimeMul => 1.0 / 6.0,
                Knob::Range => 1.0 / f32::from(RANGE_MAX),
                Knob::Gate | Knob::Level(_) => 1.0 / 32.0,
            }
        }
    }
}


/// The `crate::strum` dialog's knobs.
pub mod strum {
    /// A knob of the dialog.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum Knob {
        Time,
        TimeTension,
        Vel,
        VelTension,
        EndTime,
        EndTension,
    }

    impl Knob {
        pub fn tension(self) -> bool {
            matches!(self, Knob::TimeTension | Knob::VelTension | Knob::EndTension)
        }

        /// Change of the 0..1 knob position per wheel notch: a strength's side is 32 notches, a tension's 16 (§1).
        pub fn notch(self) -> f32 {
            if self.tension() { 1.0 / 32.0 } else { 1.0 / 64.0 }
        }

        pub fn name(self) -> &'static str {
            match self {
                Knob::Time => "Start strength",
                Knob::TimeTension => "Start tension",
                Knob::Vel => "Velocity strength",
                Knob::VelTension => "Velocity tension",
                Knob::EndTime => "End strength",
                Knob::EndTension => "End tension",
            }
        }
    }
}

/// The `crate::piano_dlg` dialogs' knobs (how they behave: `crate::piano_dlg::DlgKnob`).
pub mod piano_dlg {
    /// A knob or fader of the dialogs.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum Knob {
        FlamTime,
        FlamVel,
        Center,
        Mult,
        Offset,
        Tension,
        ClawTime,
        /// LFO's Start and End columns (Value, Range, Speed) and its Phase.
        LfoValue,
        LfoRange,
        LfoSpeed,
        LfoEndValue,
        LfoEndRange,
        LfoEndSpeed,
        LfoPhase,
        /// Quantize's Start time, Sensitivity and Duration, and its Levels (an index into `quantize::LEVELS`).
        QStart,
        QSens,
        QDur,
        QLevel(u8),
        /// Chop's Time mul and its Levels (an index into `quantize::LEVELS`).
        ChopTimeMul,
        ChopLevel(u8),
        /// Randomize's Octave, Range and Stack boxes (they turn like knobs), its Length, Variation and Population knobs
        /// and its Levels (an index into `quantize::LEVELS`).
        RandOctave,
        RandRange,
        RandStack,
        RandLength,
        RandVariation,
        RandPopulation,
        RandLevel(u8),
    }
}
