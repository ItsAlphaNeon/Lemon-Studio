//! Knobs: one value model for every rotary control, dragged vertically like FL's (spec: ui/knobs.md).
//!
//! Values are normalised 0..1. Knobs backed by the project (mixer pan, channel volume / pan) read and
//! write the model; the rest live in `State::knobs` and are not connected to the engine yet.

use crate::app::State;

/// Normalised value change per logical pixel of vertical drag (1/1024: 512 px sweeps a bipolar knob's range).
pub const PER_PX: f32 = 1.0 / 1024.0;
/// Normalised value change per mouse-wheel notch (1/64).
pub const PER_NOTCH: f32 = 1.0 / 64.0;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum KnobId {
    MasterVol,
    MasterPitch,
    /// Mixer track pan (model).
    MixPan(u16),
    MixSep(u16),
    /// The master strip's output knob.
    MixOut(u16),
    SlotMix(u16, u8),
    /// Equalizer band knob: track, band * 2 + row.
    Eq(u16, u8),
    /// Channel rack row / plugin window (model).
    ChanVol(usize),
    ChanPan(usize),
}

impl KnobId {
    /// Value change per wheel notch: master pitch steps a semitone.
    pub fn notch(self) -> f32 {
        if self == KnobId::MasterPitch { 1.0 / 24.0 } else { PER_NOTCH }
    }

    /// Value restored by a middle click.
    pub fn default_value(self) -> f32 {
        match self {
            KnobId::MasterVol | KnobId::MixOut(_) => 0.8,
            KnobId::ChanVol(_) => 0.78125,
            KnobId::SlotMix(..) => 1.0,
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
            KnobId::ChanVol(i) => self.project.channels.get(i).map_or(0.8, |c| c.volume as f32).clamp(0.0, 1.0),
            KnobId::ChanPan(i) => self.project.channels.get(i).map_or(0.5, |c| 0.5 + c.pan as f32 / 2.0).clamp(0.0, 1.0),
            _ => self.knobs.get(&id).copied().unwrap_or_else(|| id.default_value()),
        }
    }

    pub fn set_knob(&mut self, id: KnobId, v: f32) {
        let mut v = v.clamp(0.0, 1.0);
        if id == KnobId::MasterPitch {
            // Snaps to whole semitones over +-12 (100 cents each).
            v = (v * 24.0).round() / 24.0;
        }
        match id {
            KnobId::MixPan(t) => {
                if let Some(m) = self.project.mixer.iter_mut().find(|m| m.index == t) {
                    m.pan = Some(f64::from((v - 0.5) * 2.0));
                }
            }
            KnobId::ChanVol(i) => {
                if let Some(c) = self.project.channels.get_mut(i) {
                    c.volume = f64::from(v);
                }
            }
            KnobId::ChanPan(i) => {
                if let Some(c) = self.project.channels.get_mut(i) {
                    c.pan = f64::from((v - 0.5) * 2.0);
                }
            }
            _ => {
                self.knobs.insert(id, v);
            }
        }
    }

    /// The hint-bar text of a knob at its current value.
    pub fn knob_text(&self, id: KnobId) -> String {
        let n = self.knob(id);
        let bipolar = (n - 0.5) * 2.0;
        match id {
            KnobId::MasterVol => {
                let g = n / 0.8;
                let db = if g > 0.0 { format!("{:.1}dB", 20.0 * g.log10()) } else { "-inf dB".into() };
                format!("Main volume: {:.0}%  {db}  {g:.2}", g * 100.0)
            }
            KnobId::MasterPitch => format!("Master pitch: {:.0} cents", bipolar * 1200.0),
            KnobId::MixPan(_) => format!("Panning: {}", pan_text(bipolar)),
            KnobId::ChanPan(_) => format!("Channel panning: {}", pan_text(bipolar)),
            KnobId::MixSep(_) => format!("Stereo separation: {:.0}%", bipolar * 100.0),
            KnobId::MixOut(_) => format!("Output level: {:.0}%", n / 0.8 * 100.0),
            KnobId::SlotMix(..) => format!("Mix level: {:.0}%", n * 100.0),
            KnobId::Eq(_, k) => format!("{}: {:.0}%", if k % 2 == 0 { "Band frequency" } else { "Band width" }, n * 100.0),
            KnobId::ChanVol(_) => format!("Channel volume: {:.0}%", n * 100.0),
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
        s.set_knob(KnobId::MasterPitch, 0.5 + 0.4 / 24.0);
        assert_eq!(s.knob_text(KnobId::MasterPitch), "Master pitch: 0 cents");
        s.set_knob(KnobId::MasterPitch, 0.5 + 1.4 / 24.0);
        assert_eq!(s.knob_text(KnobId::MasterPitch), "Master pitch: 100 cents", "snaps to the nearest semitone");
        s.set_knob(KnobId::MasterPitch, 2.0);
        assert_eq!(s.knob(KnobId::MasterPitch), 1.0);
        s.set_knob(KnobId::MixPan(1), 0.75);
        assert_eq!(s.knob_text(KnobId::MixPan(1)), "Panning: 50% right");
    }
}
