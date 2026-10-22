// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The MIDI settings that outlive a run, in `settings.txt` (`crate::settings_file`): each device's options by name,
//! the OPTIONS menu's MIDI switches, the mapping curves, the sync offsets, the external sync device. The page's plain
//! checks, combos and channel boxes are declared in `crate::prefs`.
// spec: ui/midi-settings.md §9

use crate::settings_file::{flag, get, set_flag, write};

/// An input device's options (Settings > MIDI ▸ Input).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InCfg {
    /// Opened and listened to. A device seen for the first time is (spec §2).
    pub enabled: bool,
    /// Its port number, 0..=255; `None` = "---".
    pub port: Option<u8>,
}

impl Default for InCfg {
    fn default() -> InCfg {
        InCfg { enabled: true, port: None }
    }
}

/// An output device's options (Settings > MIDI ▸ Output).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutCfg {
    pub port: Option<u8>,
    /// Send master sync: the clock goes to this device (spec §7).
    pub sync: bool,
    /// Send all notes off when it opens and after an export.
    pub notes_off: bool,
}

impl Default for OutCfg {
    fn default() -> OutCfg {
        OutCfg { port: None, sync: false, notes_off: true }
    }
}

/// A device name as part of a key: `=` would end the key.
fn key(kind: &str, name: &str) -> String {
    format!("{kind}:{}", name.replace('=', "_"))
}

fn port_text(p: Option<u8>) -> String {
    p.map_or_else(|| "---".into(), |p| p.to_string())
}

fn bit(b: bool) -> &'static str {
    if b { "1" } else { "0" }
}

/// `"1|---"`: enabled, port.
pub fn input(name: &str) -> InCfg {
    let Some(v) = get(&key("midi_in", name)) else { return InCfg::default() };
    let mut f = v.split('|');
    let enabled = f.next().is_none_or(|s| s.trim() != "0");
    let port = f.next().and_then(|s| s.trim().parse().ok());
    InCfg { enabled, port }
}

pub fn set_input(name: &str, c: InCfg) {
    write(&key("midi_in", name), &format!("{}|{}", bit(c.enabled), port_text(c.port)));
}

/// `"---|0|1"`: port, send master sync, send all notes off.
pub fn output(name: &str) -> OutCfg {
    let Some(v) = get(&key("midi_out", name)) else { return OutCfg::default() };
    let d = OutCfg::default();
    let mut f = v.split('|');
    let port = f.next().and_then(|s| s.trim().parse().ok());
    let sync = f.next().map_or(d.sync, |s| s.trim() == "1");
    let notes_off = f.next().map_or(d.notes_off, |s| s.trim() != "0");
    OutCfg { port, sync, notes_off }
}

pub fn set_output(name: &str, c: OutCfg) {
    write(&key("midi_out", name), &format!("{}|{}|{}", port_text(c.port), bit(c.sync), bit(c.notes_off)));
}

/// The OPTIONS menu's MIDI switches (spec §5), with FL's defaults.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Switch {
    /// Enable MIDI remote control: off, every input message is ignored.
    Remote,
    /// Enable MIDI output: off, nothing is sent.
    Output,
    /// Enable MIDI master sync: off, no clock.
    MasterSync,
}

impl Switch {
    pub const ALL: [Switch; 3] = [Switch::Remote, Switch::Output, Switch::MasterSync];

    fn key(self) -> &'static str {
        match self {
            Switch::Remote => "midi_remote_control",
            Switch::Output => "midi_output",
            Switch::MasterSync => "midi_master_sync",
        }
    }

    /// Its OPTIONS menu entry.
    pub fn label(self) -> &'static str {
        match self {
            Switch::Remote => "Enable MIDI remote control",
            Switch::Output => "Enable MIDI output",
            Switch::MasterSync => "Enable MIDI master sync",
        }
    }

    pub fn from_label(label: &str) -> Option<Switch> {
        Switch::ALL.into_iter().find(|s| s.label() == label)
    }

    pub fn on(self) -> bool {
        flag(self.key(), self != Switch::MasterSync)
    }

    pub fn set(self, on: bool) {
        set_flag(self.key(), on);
    }
}

/// A mapping curve (spec §3): start and end levels and the height of the line at its middle, all 0..1; the line
/// bends into a power curve through that middle point.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Curve {
    pub lo: f32,
    pub hi: f32,
    pub mid: f32,
}

impl Curve {
    pub const LINE: Curve = Curve { lo: 0.0, hi: 1.0, mid: 0.5 };

    /// Where input `x` (0..1) maps to (0..1).
    pub fn map(&self, x: f32) -> f32 {
        let x = x.clamp(0.0, 1.0);
        let mid = self.mid.clamp(0.02, 0.98);
        // x ^ p goes through (0.5, mid).
        let p = mid.ln() / 0.5f32.ln();
        (self.lo + (self.hi - self.lo) * x.powf(p)).clamp(0.0, 1.0)
    }

    pub fn is_line(&self) -> bool {
        *self == Curve::LINE
    }

    fn parse(v: &str) -> Option<Curve> {
        let mut f = v.split(',').map(|s| s.trim().parse::<f32>().ok().filter(|x| x.is_finite()).map(|x| x.clamp(0.0, 1.0)));
        Some(Curve { lo: f.next()??, hi: f.next()??, mid: f.next()?? })
    }
}

/// The two curves: note on velocity and release velocity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CurveOf {
    Velocity,
    Release,
}

impl CurveOf {
    fn key(self) -> &'static str {
        match self {
            CurveOf::Velocity => "midi_vel_curve",
            CurveOf::Release => "midi_rel_curve",
        }
    }

    /// The curve window's title.
    pub fn title(self) -> &'static str {
        match self {
            CurveOf::Velocity => "Note on velocity mapping",
            CurveOf::Release => "Release velocity mapping",
        }
    }
}

pub fn curve(of: CurveOf) -> Curve {
    get(of.key()).and_then(|v| Curve::parse(&v)).unwrap_or(Curve::LINE)
}

pub fn set_curve(of: CurveOf, c: Curve) {
    write(of.key(), &format!("{:.4},{:.4},{:.4}", c.lo, c.hi, c.mid));
}

/// The two millisecond offsets (spec §7, §8), −1000..=1000.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Offset {
    MasterSync,
    External,
}

pub const OFFSET_RANGE: i32 = 1000;

impl Offset {
    fn key(self) -> &'static str {
        match self {
            Offset::MasterSync => "midi_master_sync_offset",
            Offset::External => "midi_ext_sync_offset",
        }
    }
}

pub fn offset(o: Offset) -> i32 {
    get(o.key()).and_then(|v| v.parse::<i32>().ok()).unwrap_or(0).clamp(-OFFSET_RANGE, OFFSET_RANGE)
}

pub fn set_offset(o: Offset, ms: i32) {
    write(o.key(), &ms.clamp(-OFFSET_RANGE, OFFSET_RANGE).to_string());
}

/// External clock sync: the input device whose clock drives playback; `None` = Off (spec §8).
pub fn ext_sync() -> Option<String> {
    get("midi_ext_sync")
}

pub fn set_ext_sync(name: Option<&str>) {
    write("midi_ext_sync", name.unwrap_or(""));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn devices_are_kept_by_name() {
        assert_eq!(input("Keys"), InCfg { enabled: true, port: None }, "a new input is enabled");
        set_input("Keys", InCfg { enabled: false, port: Some(12) });
        assert_eq!(input("Keys"), InCfg { enabled: false, port: Some(12) });
        assert_eq!(output("Synth=1"), OutCfg { port: None, sync: false, notes_off: true });
        set_output("Synth=1", OutCfg { port: Some(3), sync: true, notes_off: false });
        assert_eq!(output("Synth=1"), OutCfg { port: Some(3), sync: true, notes_off: false });
    }

    #[test]
    fn switches_default_as_fl() {
        assert!(Switch::Remote.on() && Switch::Output.on() && !Switch::MasterSync.on());
        Switch::MasterSync.set(true);
        assert!(Switch::MasterSync.on());
        assert_eq!(Switch::from_label("Enable MIDI output"), Some(Switch::Output));
    }

    #[test]
    fn curves() {
        let c = Curve::LINE;
        assert!((c.map(0.25) - 0.25).abs() < 1e-5);
        let soft = Curve { lo: 0.0, hi: 1.0, mid: 0.75 };
        assert!((soft.map(0.5) - 0.75).abs() < 1e-4, "through the handle");
        assert!(soft.map(0.0) == 0.0 && (soft.map(1.0) - 1.0).abs() < 1e-6);
        let raised = Curve { lo: 0.2, hi: 0.8, mid: 0.5 };
        assert!((raised.map(0.0) - 0.2).abs() < 1e-6 && (raised.map(1.0) - 0.8).abs() < 1e-6);
        set_curve(CurveOf::Velocity, soft);
        assert!((curve(CurveOf::Velocity).mid - 0.75).abs() < 1e-4);
        assert_eq!(curve(CurveOf::Release), Curve::LINE);
    }

    #[test]
    fn offsets_are_clamped() {
        set_offset(Offset::MasterSync, -5000);
        assert_eq!(offset(Offset::MasterSync), -1000);
        assert_eq!(offset(Offset::External), 0);
    }
}
