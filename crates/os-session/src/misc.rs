// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The Misc tab's values <-> the channel model (`os_model::ChannelMisc`). The page (`misc_page`) works in knob
//! units (0..1, bipolar knobs centred at 0.5); the model keeps bipolar knobs as -1..1.
// spec: ui/plugin-window.md (Misc tab)

#![allow(deprecated)] // writes the project directly (`crate::locked`; control-plane step 14 removes this)
use os_model::{Channel, ChannelMisc};

fn bi_in(v: f32) -> f32 {
    ((v - 0.5) * 2.0).clamp(-1.0, 1.0)
}

fn bi_out(v: f32) -> f32 {
    (v * 0.5 + 0.5).clamp(0.0, 1.0)
}

fn on(v: f32) -> bool {
    v >= 0.5
}

fn flag(b: bool) -> f32 {
    if b {
        1.0
    } else {
        0.0
    }
}

fn whole(v: f32) -> u32 {
    v.round().max(0.0) as u32
}

/// The page values of `misc`.
pub fn vals(misc: &ChannelMisc) -> MiscVals {
    let mut v = MiscVals::default();
    for m in M::ALL {
        v.set(m, get(misc, m));
    }
    v
}

/// The page values of channel `ch`: "Add to key" plays only on sampler channels (`set`).
pub fn vals_of(ch: &Channel) -> MiscVals {
    MiscVals { sampler: ch.kind.is_sampler(), ..vals(&ch.misc) }
}

/// One page value of `misc`.
pub fn get(misc: &ChannelMisc, m: M) -> f32 {
    let (vt, kt) = (&misc.tracking[0], &misc.tracking[1]);
    let (a, e) = (&misc.arp, &misc.echo);
    match m {
        M::LevelPan => bi_out(misc.levels[0]),
        M::LevelVol => bi_out(misc.levels[1]),
        M::LevelModX => bi_out(misc.levels[2]),
        M::LevelModY => bi_out(misc.levels[3]),
        M::PolyMax => misc.max_poly as f32,
        M::PolySlide => misc.slide,
        M::Porta => flag(misc.porta),
        M::Mono => flag(misc.mono),
        M::TimeGate => misc.gate,
        M::TimeShift => misc.shift,
        M::TimeSwing => misc.swing,
        M::FullPorta => flag(misc.full_porta),
        M::VolMid => f32::from(vt.mid),
        M::VolPan => bi_out(vt.pan),
        M::VolModX => bi_out(vt.mod_x),
        M::VolModY => bi_out(vt.mod_y),
        M::KeyMid => f32::from(kt.mid),
        M::KeyPan => bi_out(kt.pan),
        M::KeyModX => bi_out(kt.mod_x),
        M::KeyModY => bi_out(kt.mod_y),
        M::Cut => misc.cut as f32,
        M::CutBy => misc.cut_by as f32,
        M::ArpDir => f32::from(a.direction),
        M::ArpTime => a.time,
        M::ArpGate => a.gate,
        M::ArpRange => a.range as f32,
        M::ArpRepeat => a.repeat as f32,
        M::ArpSlide => flag(a.slide),
        M::ArpChord => a.chord as f32,
        M::EchoFeed => e.feed,
        M::EchoPan => bi_out(e.pan),
        M::EchoModX => bi_out(e.mod_x),
        M::EchoModY => bi_out(e.mod_y),
        M::EchoPitch => bi_out(e.pitch),
        M::EchoTime => e.time,
        M::Echoes => e.echoes as f32,
        M::PingPong => flag(e.ping_pong),
        M::FatMode => flag(e.fat),
        M::Root => f32::from(misc.root),
        M::MainPitch => flag(misc.main_pitch),
        M::AddKey => flag(misc.add_key),
        M::Fine => misc.fine as f32,
    }
}

/// Sets one page value in `misc` (clamped to the value's range).
pub fn put(misc: &mut ChannelMisc, m: M, v: f32) {
    let (lo, hi) = range_of(m);
    let v = if v.is_finite() { v.clamp(lo, hi) } else { default_of(m) };
    let key = |v: f32| v.round().clamp(0.0, 131.0) as u8;
    match m {
        M::LevelPan => misc.levels[0] = bi_in(v),
        M::LevelVol => misc.levels[1] = bi_in(v),
        M::LevelModX => misc.levels[2] = bi_in(v),
        M::LevelModY => misc.levels[3] = bi_in(v),
        M::PolyMax => misc.max_poly = whole(v),
        M::PolySlide => misc.slide = v,
        M::Porta => misc.porta = on(v),
        M::Mono => misc.mono = on(v),
        M::TimeGate => misc.gate = v,
        M::TimeShift => misc.shift = v,
        M::TimeSwing => misc.swing = v,
        M::FullPorta => misc.full_porta = on(v),
        M::VolMid => misc.tracking[0].mid = key(v),
        M::VolPan => misc.tracking[0].pan = bi_in(v),
        M::VolModX => misc.tracking[0].mod_x = bi_in(v),
        M::VolModY => misc.tracking[0].mod_y = bi_in(v),
        M::KeyMid => misc.tracking[1].mid = key(v),
        M::KeyPan => misc.tracking[1].pan = bi_in(v),
        M::KeyModX => misc.tracking[1].mod_x = bi_in(v),
        M::KeyModY => misc.tracking[1].mod_y = bi_in(v),
        M::Cut => misc.cut = whole(v),
        M::CutBy => misc.cut_by = whole(v),
        M::ArpDir => misc.arp.direction = whole(v) as u8,
        M::ArpTime => misc.arp.time = v,
        M::ArpGate => misc.arp.gate = v,
        M::ArpRange => misc.arp.range = whole(v),
        M::ArpRepeat => misc.arp.repeat = whole(v),
        M::ArpSlide => misc.arp.slide = on(v),
        M::ArpChord => misc.arp.chord = v.round() as i32,
        M::EchoFeed => misc.echo.feed = v,
        M::EchoPan => misc.echo.pan = bi_in(v),
        M::EchoModX => misc.echo.mod_x = bi_in(v),
        M::EchoModY => misc.echo.mod_y = bi_in(v),
        M::EchoPitch => misc.echo.pitch = bi_in(v),
        M::EchoTime => misc.echo.time = v,
        M::Echoes => misc.echo.echoes = whole(v),
        M::PingPong => misc.echo.ping_pong = on(v),
        M::FatMode => misc.echo.fat = on(v),
        M::Root => misc.root = key(v),
        M::MainPitch => misc.main_pitch = on(v),
        M::AddKey => misc.add_key = on(v),
        M::Fine => misc.fine = v.round() as i32,
    }
}

/// Lemon Sampler parameters that play a Misc value (name, plain value): the sampler does root note and fine tuning
/// itself (spec: plugins/lemon-sampler.md §9).
pub fn sampler_param(misc: &ChannelMisc, m: M) -> Option<(&'static str, f64)> {
    Some(match m {
        M::Root => ("Root note", f64::from(misc.root)),
        M::Fine => ("Fine tuning", f64::from(misc.fine)),
        M::MainPitch => ("Affected by the main pitch", f64::from(flag(misc.main_pitch))),
        M::AddKey => ("Add root note to key", f64::from(flag(misc.add_key))),
        _ => return None,
    })
}

/// `follow` in the session.
pub fn follow_in(s: &mut crate::session::Session, row: usize) {
    sync(s, row, &M::ALL);
    s.touched.repaint = true;
}

/// Rack row `row`'s Lemon Sampler takes Misc values `ms` it plays.
pub fn sync(s: &mut crate::session::Session, row: usize, ms: &[M]) {
    let Some(ch) = s.project.channels.get(row).filter(|c| c.kind.is_sampler()) else { return };
    let misc = ch.misc;
    for &m in ms {
        if let Some((name, plain)) = sampler_param(&misc, m) {
            s.plugins.set_param_named(row, name, plain);
        }
    }
    s.params.reread(row);
}

/// The page's values.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum M {
    LevelPan,
    LevelVol,
    LevelModX,
    LevelModY,
    PolyMax,
    PolySlide,
    Porta,
    Mono,
    TimeGate,
    TimeShift,
    TimeSwing,
    FullPorta,
    /// Tracking, volume page.
    VolMid,
    VolPan,
    VolModX,
    VolModY,
    /// Tracking, key page.
    KeyMid,
    KeyPan,
    KeyModX,
    KeyModY,
    Cut,
    CutBy,
    ArpDir,
    ArpTime,
    ArpGate,
    ArpRange,
    ArpRepeat,
    ArpSlide,
    ArpChord,
    EchoFeed,
    EchoPan,
    EchoModX,
    EchoModY,
    EchoPitch,
    EchoTime,
    Echoes,
    PingPong,
    FatMode,
    Root,
    MainPitch,
    AddKey,
    Fine,
}

impl M {
    /// The value's name in `Command::SetMisc`: its name in snake case (`ArpDir`: `arp_dir`).
    pub fn key(self) -> String {
        let mut k = String::new();
        for (i, c) in format!("{self:?}").chars().enumerate() {
            if c.is_ascii_uppercase() && i > 0 {
                k.push('_');
            }
            k.push(c.to_ascii_lowercase());
        }
        k
    }

    /// The value named `key` (`M::key`).
    pub fn of_key(key: &str) -> Option<M> {
        M::ALL.into_iter().find(|m| m.key() == key)
    }

    pub const COUNT: usize = M::Fine as usize + 1;
    pub const ALL: [M; M::COUNT] = [
        M::LevelPan,
        M::LevelVol,
        M::LevelModX,
        M::LevelModY,
        M::PolyMax,
        M::PolySlide,
        M::Porta,
        M::Mono,
        M::TimeGate,
        M::TimeShift,
        M::TimeSwing,
        M::FullPorta,
        M::VolMid,
        M::VolPan,
        M::VolModX,
        M::VolModY,
        M::KeyMid,
        M::KeyPan,
        M::KeyModX,
        M::KeyModY,
        M::Cut,
        M::CutBy,
        M::ArpDir,
        M::ArpTime,
        M::ArpGate,
        M::ArpRange,
        M::ArpRepeat,
        M::ArpSlide,
        M::ArpChord,
        M::EchoFeed,
        M::EchoPan,
        M::EchoModX,
        M::EchoModY,
        M::EchoPitch,
        M::EchoTime,
        M::Echoes,
        M::PingPong,
        M::FatMode,
        M::Root,
        M::MainPitch,
        M::AddKey,
        M::Fine,
    ];

    /// Hint label (Lemon Studio wording).
    pub fn label(self) -> &'static str {
        match self {
            M::LevelPan => "Panning adjustment",
            M::LevelVol => "Volume adjustment",
            M::LevelModX => "Mod A adjustment",
            M::LevelModY => "Mod B adjustment",
            M::PolyMax => "Maximum polyphony",
            M::PolySlide => "Slide time",
            M::Porta => "Portamento",
            M::Mono => "Mono",
            M::TimeGate => "Note gate",
            M::TimeShift => "Note time shift",
            M::TimeSwing => "Note time swing",
            M::FullPorta => "Always glide",
            M::VolMid | M::KeyMid => "Tracking middle point",
            M::VolPan | M::KeyPan => "Tracking to panning",
            M::VolModX | M::KeyModX => "Tracking to Mod A",
            M::VolModY | M::KeyModY => "Tracking to Mod B",
            M::Cut => "Choke group",
            M::CutBy => "Choked by group",
            M::ArpDir => "Arpeggio direction",
            M::ArpTime => "Arpeggio time",
            M::ArpGate => "Arpeggio gate",
            M::ArpRange => "Arpeggio range (octaves)",
            M::ArpRepeat => "Arpeggio note repeat",
            M::ArpSlide => "Arpeggio slide",
            M::ArpChord => "Arpeggio chord",
            M::EchoFeed => "Echo feedback",
            M::EchoPan => "Echo panning",
            M::EchoModX => "Echo Mod A",
            M::EchoModY => "Echo Mod B",
            M::EchoPitch => "Echo pitch",
            M::EchoTime => "Echo time",
            M::Echoes => "Number of echoes",
            M::PingPong => "Ping pong echoes",
            M::FatMode => "Thicken",
            M::Root => "Root note",
            M::MainPitch => "Affected by the main pitch",
            M::AddKey => "Add root note to the key",
            M::Fine => "Fine tuning",
        }
    }
}

/// Values of the page, indexed by `M`. Knobs are 0..1 (bipolar knobs centred at 0.5), switches 0 / 1, boxes and
/// lists whole numbers (0 = `---` for boxes that can be off), `Root` a MIDI key (60 = C5), `Fine` cents ±100,
/// `*Mid` MIDI keys.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MiscVals {
    pub v: [f32; M::COUNT],
    /// A sampler channel: "Add to key" reaches the sound (Lemon Sampler plays it; the engine has no such setting).
    pub sampler: bool,
}

impl MiscVals {
    pub fn get(&self, m: M) -> f32 {
        self.v[m as usize]
    }
    pub fn set(&mut self, m: M, v: f32) {
        self.v[m as usize] = v;
    }
}

impl Default for MiscVals {
    /// A new channel (the default Sampler channel).
    fn default() -> Self {
        let mut v = [0.0; M::COUNT];
        for m in M::ALL {
            v[m as usize] = default_of(m);
        }
        MiscVals { v, sampler: true }
    }
}

/// The default of one value (middle click resets to it).
pub fn default_of(m: M) -> f32 {
    match m {
        M::LevelPan | M::LevelVol | M::LevelModX | M::LevelModY => 0.5,
        M::VolPan | M::VolModX | M::VolModY | M::KeyPan | M::KeyModX | M::KeyModY => 0.5,
        M::EchoPan | M::EchoModX | M::EchoModY | M::EchoPitch => 0.5,
        M::TimeGate | M::TimeSwing | M::ArpGate => 1.0,
        M::ArpTime => os_model::Arpeggio::TIME_DEFAULT,
        M::EchoTime => os_model::Echo::TIME_DEFAULT,
        M::PolySlide => 500.0 / 1024.0,
        M::FullPorta | M::MainPitch => 1.0,
        M::ArpRange | M::ArpRepeat => 1.0,
        M::ArpChord => -1.0,
        M::Echoes => 4.0,
        M::VolMid => 100.0,
        M::Root | M::KeyMid => 60.0,
        _ => 0.0,
    }
}

/// Smallest and largest value of a box / list / note value.
pub fn range_of(m: M) -> (f32, f32) {
    match m {
        M::PolyMax => (0.0, 999.0),
        M::Cut | M::CutBy => (0.0, 999.0),
        // spec: audio/channel-misc.md "Settings" (the echo count's control range was not found; 0 and 10 were exported).
        M::ArpRange => (0.0, 6.0),
        M::ArpRepeat => (1.0, 6.0),
        M::Echoes => (0.0, 10.0),
        M::ArpDir => (0.0, 5.0),
        M::ArpChord => (-1.0, CHORD_ENTRIES as f32),
        M::Root | M::VolMid | M::KeyMid => (0.0, 131.0),
        M::Fine => (-100.0, 100.0),
        _ => (0.0, 1.0),
    }
}

/// Entries of FL 24.1's chords file, which the stored arpeggio chord `k >= 1` names (spec: audio/channel-misc.md
/// "Settings", "The chords file").
pub const CHORD_ENTRIES: i32 = 88;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_value_round_trips_through_the_model() {
        let mut misc = ChannelMisc::default();
        // Defaults agree with the page's defaults.
        for m in M::ALL {
            assert!((get(&misc, m) - default_of(m)).abs() < 1e-6, "{m:?}: {} vs {}", get(&misc, m), default_of(m));
        }
        for m in M::ALL {
            let (lo, hi) = range_of(m);
            let mut v = if hi > 1.0 { (lo + 3.0).min(hi) } else { 0.25 };
            put(&mut misc, m, v);
            if (get(&misc, m) - v).abs() > 1e-6 {
                // A switch: flip it.
                v = 1.0 - default_of(m);
                put(&mut misc, m, v);
            }
            assert!((get(&misc, m) - v).abs() < 1e-6, "{m:?}");
        }
        assert_eq!(misc.levels[0], -0.5);
        put(&mut misc, M::Fine, 250.0);
        assert_eq!(misc.fine, 100);
    }
}
