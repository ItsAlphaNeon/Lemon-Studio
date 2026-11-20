// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The knobs a change, an undo step or a controller link names (control-plane step 9: the session's; `crate::knobs`
//! keeps how they behave under the mouse, `KnobUi`), and the dialogs' own knobs they include; the project's values of them.


#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum KnobId {
    /// The toolbar's main volume (stored 0..16000, 12800 = unity at 0.8) and main pitch (-1200..1200 cents) (model
    /// `Project::main_volume` / `main_pitch`; spec: audio/mixer.md "Main volume", audio/voice-levels.md).
    MasterVol,
    MasterPitch,
    /// Mixer track `.0`'s fader (model `MixerTrack::volume`, 0..1 as the fader shows it; 0.8 unity).
    MixVol(u16),
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
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, serde::Serialize, serde::Deserialize)]
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
    /// What the knob is as a target of the value box and of controller links: a container member's level and a mixer
    /// track's fader are their own.
    pub fn target(self) -> crate::typein::Target {
        match self {
            KnobId::Member(t, k, id, wet) => crate::typein::Target::Member(t, k, id, wet),
            KnobId::MixVol(t) => crate::typein::Target::Fader(t),
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


/// How close (in semitones) master pitch must get to a semitone to snap onto it.
pub const PITCH_SNAP: f32 = 0.2;

impl LfoKnob {
    pub const ALL: [LfoKnob; 7] = [LfoKnob::On, LfoKnob::Mode, LfoKnob::Amount, LfoKnob::Speed, LfoKnob::Tension, LfoKnob::Skew, LfoKnob::Pulse];

    /// A switch a click flips (not turned by dragging).
    pub fn switch(self) -> bool {
        matches!(self, LfoKnob::On | LfoKnob::Mode)
    }

    /// The control's 0..1 position for `l`.
    pub fn pos(self, l: &os_model::clip_lfo::ClipLfo) -> f32 {
        let bi = |v: i32| ((v as f32 + 128.0) / 256.0).clamp(0.0, 1.0);
        match self {
            LfoKnob::On => f32::from(u8::from(l.on())),
            LfoKnob::Mode => f32::from(u8::from(l.adds())),
            LfoKnob::Amount => (l.amount as f32 / 128.0).clamp(0.0, 1.0),
            LfoKnob::Speed => (l.speed as f32 / 65536.0).clamp(0.0, 1.0),
            LfoKnob::Tension => bi(l.tension),
            LfoKnob::Skew => bi(l.skew),
            LfoKnob::Pulse => bi(l.pulse_width),
        }
    }

    /// Stores position `v` (0..1) into `l`.
    pub fn store(self, l: &mut os_model::clip_lfo::ClipLfo, v: f32) {
        let bi = |v: f32| (v * 256.0).round() as i32 - 128;
        match self {
            LfoKnob::On => l.switch = if v >= 0.5 { -1 } else { 0 },
            LfoKnob::Mode => l.mode = u8::from(v >= 0.5),
            LfoKnob::Amount => l.amount = (v * 128.0).round() as i32,
            LfoKnob::Speed => l.speed = (v * 65536.0).round() as i32,
            LfoKnob::Tension => l.tension = bi(v),
            LfoKnob::Skew => l.skew = bi(v),
            LfoKnob::Pulse => l.pulse_width = bi(v),
        }
    }

    /// The hint text at `l`: "LFO amount: 50%", "LFO speed: 1.00 cycles per beat".
    pub fn text(self, l: &os_model::clip_lfo::ClipLfo) -> String {
        let pct = |v: i32| format!("{:+.0}%", v as f32 / 128.0 * 100.0);
        match self {
            LfoKnob::On => format!("LFO: {}", if l.on() { "on" } else { "off" }),
            LfoKnob::Mode => format!("LFO mode: {}", if l.adds() { "add" } else { "multiply" }),
            LfoKnob::Amount => format!("LFO amount: {:.0}%", l.amount as f32 / 128.0 * 100.0),
            LfoKnob::Speed => match os_model::clip_lfo::ClipLfo::cycles_per_beat(l.speed) {
                c if c <= 0.0 => "LFO speed: stopped".into(),
                c if c >= 1.0 => format!("LFO speed: {c:.2} cycles per beat"),
                c => format!("LFO speed: 1 cycle per {:.2} beats", 1.0 / c),
            },
            LfoKnob::Tension => format!("LFO tension: {}", pct(l.tension)),
            LfoKnob::Skew => format!("LFO skew: {}", pct(l.skew)),
            LfoKnob::Pulse => format!("LFO pulse width: {}", pct(l.pulse_width)),
        }
    }
}

/// A stored equaliser setting as a 0..1 position: gain -1800..1800 (0.01 dB), frequency and width 0..65536.
pub fn eq_pos(field: u8, v: i32) -> f32 {
    if field == 0 {
        ((v as f32 / 1800.0 + 1.0) / 2.0).clamp(0.0, 1.0)
    } else {
        (v as f32 / 65536.0).clamp(0.0, 1.0)
    }
}

impl KnobId {
    /// The command setting this control of `s`'s project to knob position `v` (0..1, the main pitch's pull onto a
    /// semitone already made), naming what it sets by id (`s`'s ids settled); `None` for the dialogs' knobs, for those
    /// still set directly, and when the control's channel or track is gone.
    pub fn command(self, s: &crate::session::Session, v: f32) -> Option<crate::control::Command> {
        use crate::control::Command;
        let p = &s.project;
        let track = |t: u16| p.mixer.iter().find(|m| m.index == t).map(|m| m.id.0);
        let channel = |row: usize| p.channels.get(row).map(|c| c.id.0);
        let set_track = |t: u16, volume: Option<f64>, pan: Option<f64>, stereo: Option<f64>| {
            Some(Command::SetTrack { track: track(t)?, name: None, color: None, volume, pan, stereo, enabled: None, polarity: None, swap: None, separator: None, dock: None, threaded: None, latency_in: None, latency_out: None, effects_enabled: None, audio_in: None, audio_out: None, rec_source: None, monitor: None, armed: None })
        };
        let set_channel = |row: usize, volume: Option<f64>, pan: Option<f64>| {
            let mut c = Command::set_channel(channel(row)?);
            if let Command::SetChannel { volume: v, pan: p, .. } = &mut c {
                (*v, *p) = (volume, pan);
            }
            Some(c)
        };
        let bipolar = f64::from((v - 0.5) * 2.0);
        match self {
            KnobId::MasterVol => Some(Command::SetMainVolume { volume: f64::from(v) }),
            KnobId::MasterPitch => Some(Command::SetMainPitch { cents: ((v - 0.5) * 2400.0).round() as i16 }),
            KnobId::Swing => Some(Command::SetSwing { swing: f64::from(v) }),
            KnobId::MixVol(t) => set_track(t, Some(f64::from(v)), None, None),
            KnobId::MixPan(t) => set_track(t, None, Some(bipolar), None),
            // In 64ths, as the file keeps it and FL's knob sets it (12% merged is 8, a typed 1% is 1).
            KnobId::MixSep(t) => set_track(t, None, None, Some((bipolar * 64.0).round() / 64.0)),
            KnobId::SlotMix(t, k) => Some(Command::SetSlot { track: track(t)?, slot: k.min(9), enabled: None, mix: Some(f64::from(v)), name: None, color: None, sidechain: None, auto_bypass: None, threaded: None }),
            KnobId::Send(a, b) => Some(Command::SetSend { from: track(a)?, to: track(b)?, level: Some(f64::from(v)) }),
            KnobId::ChanVol(row) => set_channel(row, Some(f64::from(v)), None),
            KnobId::ChanPan(row) => set_channel(row, None, Some(bipolar)),
            KnobId::Eq(t, b, f) => {
                let stored = if f == 0 { ((v * 2.0 - 1.0) * 1800.0).round() as i32 } else { (v * 65536.0).round() as i32 };
                let field = |k: u8| (f.min(2) == k).then_some(stored);
                Some(Command::SetEq { track: track(t)?, band: b.min(2), gain: field(0), frequency: field(1), width: field(2) })
            }
            KnobId::AutoLfo(row, k) => {
                let mut l = s.clip_lfo(row)?.clone();
                k.store(&mut l, v);
                let field = |want: LfoKnob, x: i32| (k == want).then_some(x);
                Some(Command::SetClipLfo {
                    channel: channel(row)?,
                    on: (k == LfoKnob::On).then(|| l.on()),
                    add: (k == LfoKnob::Mode).then(|| l.adds()),
                    amount: field(LfoKnob::Amount, l.amount),
                    speed: field(LfoKnob::Speed, l.speed),
                    tension: field(LfoKnob::Tension, l.tension),
                    skew: field(LfoKnob::Skew, l.skew),
                    pulse_width: field(LfoKnob::Pulse, l.pulse_width),
                })
            }
            KnobId::MidiOut(row, k) => {
                use crate::lemon_midi_out::MoKnob;
                let mut st = crate::lemon_midi_out::row_settings(p, row)?;
                k.store(&mut st, v);
                Some(Command::SetMidiOut {
                    channel: channel(row)?,
                    midi_channel: (k == MoKnob::Channel).then_some(st.channel),
                    port: (k == MoKnob::Port).then_some(st.port),
                    patch: (k == MoKnob::Patch).then_some(st.patch),
                    bank: (k == MoKnob::Bank).then_some(st.bank[0]),
                    bank2: (k == MoKnob::Bank2).then_some(st.bank[1]),
                    send_levels: (k == MoKnob::SendLevels).then_some(st.send_levels),
                    map_color: (k == MoKnob::MapColor).then_some(st.map_color),
                })
            }
            KnobId::Member(t, k, member, wet) => {
                let container = crate::edit::container::place_id(s, t, k)?.0;
                let level = Some(f64::from(v));
                Some(Command::SetMember { container, member, wet: level.filter(|_| wet), dry: level.filter(|_| !wet), name: None, color: None, icon: None, auto_bypass: None, threaded: None })
            }
            KnobId::Strum(_) | KnobId::Arp(_) | KnobId::PianoDlg(_) => None,
        }
    }
}

/// The project's knobs (`State`'s are these and the interface's own).
impl crate::session::Session {
    /// The project's value of control `id` as its knob shows it (0..1); `None` for the piano roll dialogs' knobs,
    /// which are the interface's.
    pub fn knob(&self, id: KnobId) -> Option<f32> {
        Some(match id {
            KnobId::MixVol(t) => self.project.mixer.iter().find(|m| m.index == t).and_then(|m| m.volume).unwrap_or(0.8) as f32,
            KnobId::MixPan(t) => {
                let pan = self.project.mixer.iter().find(|m| m.index == t).and_then(|m| m.pan).unwrap_or(0.0);
                (0.5 + pan as f32 / 2.0).clamp(0.0, 1.0)
            }
            KnobId::MixSep(t) => {
                let sep = self.project.mixer.iter().find(|m| m.index == t).and_then(|m| m.stereo_separation).unwrap_or(0.0);
                (0.5 + sep as f32 / 2.0).clamp(0.0, 1.0)
            }
            KnobId::SlotMix(t, k) => self.project.mixer.iter().find(|m| m.index == t).map_or(1.0, |m| m.slot_mix[usize::from(k).min(9)] as f32),
            KnobId::Send(a, b) => crate::edit::mixer::send(self, a, b).unwrap_or(0.8) as f32,
            KnobId::ChanVol(i) => self.project.channels.get(i).map_or(0.8, |c| c.volume as f32).clamp(0.0, 1.0),
            KnobId::ChanPan(i) => self.project.channels.get(i).map_or(0.5, |c| 0.5 + c.pan as f32 / 2.0).clamp(0.0, 1.0),
            // Main swing is project data, 0..=128 (spec: flp/events/project-info.md, event 11).
            KnobId::Swing => f32::from(self.project.swing.unwrap_or(0).min(128)) / 128.0,
            KnobId::MasterVol => f32::from(self.project.main_volume.unwrap_or(os_model::Project::MAIN_VOLUME_DEFAULT).min(16000)) / 16000.0,
            KnobId::MasterPitch => (0.5 + f32::from(self.project.main_pitch.unwrap_or(0)) / 2400.0).clamp(0.0, 1.0),
            KnobId::Eq(t, b, f) => {
                let (b, f) = (usize::from(b.min(2)), usize::from(f.min(2)));
                let eq = self.project.mixer.iter().find(|m| m.index == t).map_or(os_model::EQ_DEFAULT, |m| m.eq);
                eq_pos(f as u8, eq[b][f])
            }
            KnobId::Strum(_) | KnobId::Arp(_) | KnobId::PianoDlg(_) => return None,
            KnobId::AutoLfo(row, k) => k.pos(self.clip_lfo(row).unwrap_or(&os_model::clip_lfo::ClipLfo::default())),
            KnobId::MidiOut(row, k) => k.pos(&crate::lemon_midi_out::row_settings(&self.project, row).unwrap_or_default()),
            KnobId::Member(t, k, id, wet) => crate::edit::container::level(self, t, k, id, wet).unwrap_or(0.0),
        })
    }

    /// The LFO of the automation clip on channel row `row`.
    pub fn clip_lfo(&self, row: usize) -> Option<&os_model::clip_lfo::ClipLfo> {
        let ch = self.project.channels.get(row)?;
        self.project.automation(ch.index).map(|a| &a.lfo)
    }

    /// Channel row `row` is an automation clip's: its pan and volume knobs are the clip's minimum and maximum (spec:
    /// audio/automation.md "Range").
    pub fn is_clip_row(&self, row: usize) -> bool {
        self.project.channels.get(row).is_some_and(|c| c.kind == os_model::ChannelKind::Automation)
    }

    /// Control `id` of the project set from its knob's position `v` (0..1); the piano roll dialogs' knobs are the
    /// interface's (`State::set_knob`).
    pub fn set_knob(&mut self, id: KnobId, v: f32) {
        let mut v = v.clamp(0.0, 1.0);
        if id == KnobId::MasterPitch {
            // Smooth, but pulled onto a whole semitone (+-12 range, 100 cents each) when within SNAP of it.
            let nearest = (v * 24.0).round() / 24.0;
            if (v - nearest).abs() * 24.0 < PITCH_SNAP {
                v = nearest;
            }
        }
        // Its command, with no step of its own (a drag's is taken at the press, `history::knob`; a controller's moves
        // make none). Nothing for the dialogs' knobs or a control whose channel, track or member is gone.
        crate::control::settle_in(self);
        if let Some(c) = id.command(self, v) {
            let _ = crate::control::run_in(self, vec![c]);
        }
    }
}
