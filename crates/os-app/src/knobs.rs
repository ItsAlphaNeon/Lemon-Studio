// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

//! Knobs: one value model for every rotary control, dragged vertically like FL's (spec: ui/knobs.md).
//!
//! Values are normalised 0..1. Every knob reads and writes the project model.

use crate::app::State;

/// Normalised value change per logical pixel of vertical drag (1/1024: 512 px sweeps a bipolar knob's range).
pub const PER_PX: f32 = 1.0 / 1024.0;
/// How close (in semitones) master pitch must get to a semitone to snap onto it.
pub const PITCH_SNAP: f32 = 0.2;
/// Normalised value change per mouse-wheel notch (1/64).
pub const PER_NOTCH: f32 = 1.0 / 64.0;

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
}

impl KnobId {
    /// Value change per wheel notch: master pitch steps a semitone.
    pub fn notch(self) -> f32 {
        match self {
            KnobId::MasterPitch => 1.0 / 24.0,
            // spec: ui/channel-rack.md §11 (3 % per notch; pan percent spans half the range), §7 (swing 4-5 %).
            KnobId::ChanVol(_) => 0.03,
            KnobId::ChanPan(_) => 0.015,
            KnobId::Swing => 0.045,
            _ => PER_NOTCH,
        }
    }

    /// Value change per pixel of vertical drag.
    pub fn drag_rate(self) -> f32 {
        match self {
            // spec: ui/channel-rack.md §11 (0.4 % / px volume, 0.8 % / px pan) and §7 (30 px = 17 % swing).
            KnobId::ChanVol(_) | KnobId::ChanPan(_) => 0.004,
            KnobId::Swing => 0.17 / 30.0,
            _ => PER_PX,
        }
    }

    /// Value restored by a middle click.
    pub fn default_value(self) -> f32 {
        match self {
            KnobId::MasterVol | KnobId::Send(..) => 0.8,
            KnobId::ChanVol(_) => 0.78125,
            KnobId::SlotMix(..) => 1.0,
            KnobId::Swing => 0.0,
            _ => 0.5,
        }
    }
}

/// "Centered", "38% right", "2% left".
pub fn pan_text(p: f32) -> String {
    let pct = (p.abs() * 100.0).round() as i32;
    if pct == 0 {
        "Centered".into()
    } else if p > 0.0 {
        format!("{pct}% right")
    } else {
        format!("{pct}% left")
    }
}

impl State {
    pub fn knob(&self, id: KnobId) -> f32 {
        match id {
            KnobId::MixPan(t) => {
                let pan = self.project.mixer.iter().find(|m| m.index == t).and_then(|m| m.pan).unwrap_or(0.0);
                (0.5 + pan as f32 / 2.0).clamp(0.0, 1.0)
            }
            KnobId::MixSep(t) => {
                let sep = self.project.mixer.iter().find(|m| m.index == t).and_then(|m| m.stereo_separation).unwrap_or(0.0);
                (0.5 + sep as f32 / 2.0).clamp(0.0, 1.0)
            }
            KnobId::SlotMix(t, k) => self.project.mixer.iter().find(|m| m.index == t).map_or(1.0, |m| m.slot_mix[usize::from(k).min(9)] as f32),
            KnobId::Send(a, b) => crate::mixer::send(self, a, b).unwrap_or(0.8) as f32,
            KnobId::ChanVol(i) => self.project.channels.get(i).map_or(0.8, |c| c.volume as f32).clamp(0.0, 1.0),
            KnobId::ChanPan(i) => self.project.channels.get(i).map_or(0.5, |c| 0.5 + c.pan as f32 / 2.0).clamp(0.0, 1.0),
            // Main swing is project data, 0..=128 (spec: flp/events/project-info.md, event 11).
            KnobId::Swing => f32::from(self.project.swing.unwrap_or(0).min(128)) / 128.0,
            KnobId::MasterVol => f32::from(self.project.main_volume.unwrap_or(os_model::Project::MAIN_VOLUME_DEFAULT).min(16000)) / 16000.0,
            KnobId::MasterPitch => (0.5 + f32::from(self.project.main_pitch.unwrap_or(0)) / 2400.0).clamp(0.0, 1.0),
        }
    }

    pub fn set_knob(&mut self, id: KnobId, v: f32) {
        let mut v = v.clamp(0.0, 1.0);
        if id == KnobId::MasterPitch {
            // Smooth, but pulled onto a whole semitone (+-12 range, 100 cents each) when within SNAP of it.
            let nearest = (v * 24.0).round() / 24.0;
            if (v - nearest).abs() * 24.0 < PITCH_SNAP {
                v = nearest;
            }
        }
        match id {
            KnobId::MixPan(t) => {
                if let Some(m) = self.project.mixer.iter_mut().find(|m| m.index == t) {
                    m.pan = Some(f64::from((v - 0.5) * 2.0));
                    self.piano.dirty = true;
                }
            }
            KnobId::MixSep(t) => {
                if let Some(m) = self.project.mixer.iter_mut().find(|m| m.index == t) {
                    m.stereo_separation = Some(f64::from((v - 0.5) * 2.0));
                    self.piano.dirty = true;
                }
            }
            KnobId::SlotMix(t, k) => {
                if let Some(m) = self.project.mixer.iter_mut().find(|m| m.index == t) {
                    m.slot_mix[usize::from(k).min(9)] = f64::from(v);
                    self.piano.dirty = true;
                }
            }
            KnobId::Send(a, b) => {
                if let Some(m) = self.project.mixer.iter_mut().find(|m| m.index == a) {
                    m.send_levels.retain(|&(d, _)| d != b);
                    m.send_levels.push((b, f64::from(v)));
                    m.send_levels.sort_by_key(|&(d, _)| d);
                    self.piano.dirty = true;
                }
            }
            KnobId::ChanVol(i) => {
                if let Some(c) = self.project.channels.get_mut(i) {
                    c.volume = f64::from(v);
                    self.piano.dirty = true;
                }
            }
            KnobId::ChanPan(i) => {
                if let Some(c) = self.project.channels.get_mut(i) {
                    c.pan = f64::from((v - 0.5) * 2.0);
                    self.piano.dirty = true;
                }
            }
            KnobId::Swing => {
                self.project.swing = Some((v * 128.0).round() as u8);
                self.piano.dirty = true;
            }
            KnobId::MasterVol => {
                self.project.main_volume = Some((v * 16000.0).round() as u16);
                self.piano.dirty = true;
            }
            KnobId::MasterPitch => {
                self.project.main_pitch = Some(((v - 0.5) * 2400.0).round() as i16);
                self.piano.dirty = true;
            }
        }
    }

    /// The hint-bar text of a knob at its current value.
    pub fn knob_text(&self, id: KnobId) -> String {
        let n = self.knob(id);
        let bipolar = (n - 0.5) * 2.0;
        match id {
            KnobId::MasterVol => {
                // The gain is the level curve of stored / 12800 (spec: audio/mixer.md "Main volume").
                let g = os_dsp::level::curve(n / 0.8);
                let db = if g > 0.0 { format!("{:.1}dB", 20.0 * g.log10()) } else { "-inf dB".into() };
                format!("Main volume: {:.0}%  {db}  {g:.2}", n / 0.8 * 100.0)
            }
            KnobId::MasterPitch => format!("Master pitch: {:.0} cents", bipolar * 1200.0),
            KnobId::MixPan(_) => format!("Panning: {}", pan_text(bipolar)),
            KnobId::ChanPan(_) => format!("Channel panning: {}", pan_text(bipolar)),
            KnobId::MixSep(_) => crate::mixer::sep_text(f64::from(bipolar)),
            KnobId::Send(_, b) => crate::mixer::level_text(&format!("To {}", crate::mixer::name(self, b)), f64::from(n)),
            KnobId::SlotMix(..) => format!("Mix level: {:.0}%", n * 100.0),
            // spec: ui/channel-rack.md §11: "Channel volume: 78%  -5.2dB  0.55".
            KnobId::ChanVol(_) => {
                let g = os_engine::laws::channel_gain(f64::from(n));
                let db = if g > 0.0 { format!("{:.1}dB", 20.0 * g.log10()) } else { "-INF".into() };
                format!("Channel volume: {:.0}%  {db}  {g:.2}", n * 100.0)
            }
            KnobId::Swing => format!("Main swing: {:.0}%", n * 100.0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texts_and_roundtrip() {
        assert_eq!(pan_text(0.0), "Centered");
        assert_eq!(pan_text(0.394), "39% right");
        assert_eq!(pan_text(-0.02), "2% left");
        let mut app = crate::app::App::new(None);
        let s = &mut app.state;
        s.set_knob(KnobId::MasterVol, 0.8);
        assert_eq!(s.knob_text(KnobId::MasterVol), "Main volume: 100%  0.0dB  1.00");
        s.set_knob(KnobId::MasterPitch, 0.5 + 0.1 / 24.0);
        assert_eq!(s.knob_text(KnobId::MasterPitch), "Master pitch: 0 cents", "close enough: snaps");
        s.set_knob(KnobId::MasterPitch, 0.5 + 1.5 / 24.0);
        assert_eq!(s.knob_text(KnobId::MasterPitch), "Master pitch: 150 cents", "between semitones it is smooth");
        s.set_knob(KnobId::MasterPitch, 2.0);
        assert_eq!(s.knob(KnobId::MasterPitch), 1.0);
        s.set_knob(KnobId::MixPan(1), 0.75);
        assert_eq!(s.knob_text(KnobId::MixPan(1)), "Panning: 50% right");
        assert_eq!(s.knob_text(KnobId::Swing), "Main swing: 0%");
        s.project.channels = vec![os_model::Channel { volume: 0.78, ..crate::app::test_project().channels[0].clone() }];
        assert_eq!(s.knob_text(KnobId::ChanVol(0)), "Channel volume: 78%  -5.2dB  0.55");
    }
}
