// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

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
    /// Mixer track `.0`'s equaliser: band `.1` (0 low shelf, 1 peaking, 2 high shelf), setting `.2` (0 gain, the
    /// band's fader; 1 frequency; 2 width) (model `MixerTrack::eq`; spec: ui/mixer.md §8, audio/mixer.md "Equaliser").
    Eq(u16, u8, u8),
    /// A knob of the piano roll's Strum dialog (dialog state, not project data; spec: ui/piano-strum.md §1).
    Strum(crate::strum::Knob),
    /// A knob (or the Range box) of the piano roll's Arpeggiate dialog (dialog state; spec: ui/piano-arpeggiate.md §1).
    Arp(crate::arpeggiate::Knob),
    /// A knob or fader of the piano roll's Flam or Scale levels dialog (dialog state; spec: ui/piano-flam.md §1,
    /// ui/piano-level-scaling.md §1).
    PianoDlg(crate::piano_dlg::Knob),
}

/// The equaliser's band names as its hints give them (spec: ui/mixer.md §8).
const EQ_BANDS: [&str; 3] = ["EQ band 1 (low shelf)", "EQ band 2 (peaking)", "EQ band 3 (high shelf)"];

/// A stored equaliser setting as a 0..1 position: gain -1800..1800 (0.01 dB), frequency and width 0..65536.
fn eq_pos(field: u8, v: i32) -> f32 {
    if field == 0 {
        ((v as f32 / 1800.0 + 1.0) / 2.0).clamp(0.0, 1.0)
    } else {
        (v as f32 / 65536.0).clamp(0.0, 1.0)
    }
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
            KnobId::Strum(k) => k.notch(),
            KnobId::Arp(k) => k.notch(),
            KnobId::PianoDlg(k) => k.notch(),
            _ => PER_NOTCH,
        }
    }

    /// Value change per pixel of vertical drag.
    pub fn drag_rate(self) -> f32 {
        match self {
            // spec: ui/channel-rack.md §11 (0.4 % / px volume, 0.8 % / px pan) and §7 (30 px = 17 % swing).
            KnobId::ChanVol(_) | KnobId::ChanPan(_) => 0.004,
            KnobId::Swing => 0.17 / 30.0,
            // A band's gain fader: its handle about follows the mouse over the graph's height.
            KnobId::Eq(_, _, 0) => 1.0 / 160.0,
            // A tension's side is half as many notches as a strength's (spec: ui/piano-strum.md §1).
            KnobId::Strum(k) if k.tension() => 2.0 * PER_PX,
            // The stepped controls move one value per 12 px.
            KnobId::Arp(k @ (crate::arpeggiate::Knob::TimeMul | crate::arpeggiate::Knob::Range)) => k.notch() / 12.0,
            // The level faders' handles follow the mouse.
            KnobId::PianoDlg(k) => k.travel().map_or(PER_PX, |t| 1.0 / t),
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
            KnobId::Arp(k) => crate::arpeggiate::Settings::default().get(k),
            KnobId::PianoDlg(k) => k.default_pos(),
            KnobId::Eq(_, b, f) => eq_pos(f, os_model::EQ_DEFAULT[usize::from(b.min(2))][usize::from(f.min(2))]),
            _ => 0.5,
        }
    }
}

/// A knob or slider of the app's own windows that goes back to its default: a middle click, and "Reset" in its right-click
/// menu (spec: ui/knobs.md "Owner decisions", ui/mixer.md §6.3, ui/channel-rack.md §19). `App::reset_control` resets it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Control {
    Knob(KnobId),
    /// A mixer track's volume fader (spec: ui/mixer.md §4).
    Fader(u16),
    /// Channel row `.0`'s pitch knob in its plugin window (spec: ui/plugin-window.md).
    ChanPitch(usize),
    /// A declared slider of the settings window, by label (`prefs::SLIDERS`).
    Slider(&'static str),
    /// Knob `.0` of Settings > Theme's adjustments (`theme_adjust::KNOBS`): back to the chosen theme's value.
    Theme(u8),
}

impl Control {
    /// The control a main-window hit is, if it is one.
    pub fn of(a: &crate::app::Action) -> Option<Control> {
        use crate::app::Action;
        use crate::views::settings::SAct;
        match *a {
            Action::Knob(id) => Some(Control::Knob(id)),
            Action::Fader(t) => Some(Control::Fader(t)),
            Action::Settings(SAct::Slider(label, ..)) => Some(Control::Slider(label)),
            Action::Settings(SAct::ThemeKnob(i)) => Some(Control::Theme(i)),
            _ => None,
        }
    }
}

/// The right-click menu of a control with nothing to offer but its default: "Reset".
pub fn reset_menu(c: Control, x: f32, y: f32) -> crate::playlist::Popup {
    use crate::playlist::{Cmd, PItem, Popup};
    Popup { x, y, items: vec![PItem::new("Reset", Cmd::Reset(c))], sub: None, scroll: [0; 2] }
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
            KnobId::Eq(t, b, f) => {
                let (b, f) = (usize::from(b.min(2)), usize::from(f.min(2)));
                let eq = self.project.mixer.iter().find(|m| m.index == t).map_or(os_model::EQ_DEFAULT, |m| m.eq);
                eq_pos(f as u8, eq[b][f])
            }
            KnobId::Strum(k) => (self.strum.set.get(k) + 1.0) / 2.0,
            KnobId::Arp(k) => self.arp.set.get(k),
            KnobId::PianoDlg(k) => self.piano_dlg.get(k),
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
            KnobId::Eq(t, b, f) => {
                if let Some(m) = self.project.mixer.iter_mut().find(|m| m.index == t) {
                    let stored = if f == 0 { ((v * 2.0 - 1.0) * 1800.0).round() as i32 } else { (v * 65536.0).round() as i32 };
                    m.eq[usize::from(b.min(2))][usize::from(f.min(2))] = stored;
                    self.piano.dirty = true;
                }
            }
            // The notes follow at once (the dialog previews, §1).
            KnobId::Strum(k) => {
                self.strum.set.set(k, v * 2.0 - 1.0);
                crate::strum::preview(self);
            }
            KnobId::Arp(k) => {
                let before = self.arp.set;
                self.arp.set.set(k, v);
                if self.arp.set != before {
                    crate::arpeggiate::preview(self);
                }
            }
            KnobId::PianoDlg(k) => {
                let before = self.piano_dlg.get(k);
                self.piano_dlg.set(k, v);
                if self.piano_dlg.get(k) != before {
                    crate::piano_dlg::preview(self);
                }
            }
        }
    }

    /// Knob `id` back to its default, as an undo step.
    pub fn reset_knob(&mut self, id: KnobId) {
        crate::history::knob(self, id, false);
        self.set_knob(id, id.default_value());
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
            // A sign above zero: "+100 cents" (spec: ui/type-in-value.md §6).
            KnobId::MasterPitch => match (bipolar * 1200.0).round() as i32 {
                c if c > 0 => format!("Master pitch: +{c} cents"),
                c => format!("Master pitch: {c} cents"),
            },
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
            // "EQ band 1 (low shelf) level: 0.0dB", "... freq: 90Hz", "... width: 0.27" (spec: ui/mixer.md §8).
            KnobId::Eq(_, b, f) => {
                let band = EQ_BANDS[usize::from(b.min(2))];
                match f {
                    0 => format!("{band} level: {:.1}dB", ((n * 2.0 - 1.0) * 1800.0).round() / 100.0),
                    1 => format!("{band} freq: {:.0}Hz", os_dsp::mixer::eq_frequency((n * 65536.0).round() as i32)),
                    _ => format!("{band} width: {n:.2}"),
                }
            }
            KnobId::Strum(k) => self.strum.set.text(k, u32::from(self.project.ppq.max(4))),
            KnobId::Arp(k) => self.arp.set.text(k),
            KnobId::PianoDlg(k) => self.piano_dlg.text(k, u32::from(self.project.ppq.max(4))),
        }
    }
}

impl crate::app::App {
    /// Control `c` back to its default (a middle click, or "Reset" in its menu); the hint bar shows the new value.
    pub(crate) fn reset_control(&mut self, c: Control) {
        use std::borrow::Cow;
        let s = &mut self.state;
        match c {
            Control::Knob(id) => {
                s.reset_knob(id);
                self.hint = Cow::Owned(s.knob_text(id));
            }
            Control::Fader(t) => {
                let v = os_model::MixerTrack::new(t).volume.unwrap_or(0.8);
                crate::control::knob_edit(s, crate::history::MIXER_VOLUME, crate::control::command::Region::Track(t), |s| {
                    if let Some(m) = crate::mixer::track_mut(s, t) {
                        m.volume = Some(v);
                        s.piano.dirty = true;
                    }
                });
                self.hint = Cow::Owned(crate::mixer::level_text("Volume", v));
            }
            Control::ChanPitch(row) => {
                crate::control::edit(s, "Channel pitch", crate::control::command::Region::Channel(row), |s| {
                    if let Some(ch) = s.project.channels.get_mut(row) {
                        ch.pitch_cents = 0;
                        s.piano.dirty = true;
                    }
                });
            }
            Control::Slider(label) => {
                if let Some(sl) = crate::prefs::find_slider(label) {
                    crate::prefs::set_slider(sl, sl.default);
                }
            }
            Control::Theme(i) => return self.theme_knob_reset(usize::from(i)),
        }
        self.invalidate();
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
        assert_eq!(s.knob_text(KnobId::MasterPitch), "Master pitch: +150 cents", "between semitones it is smooth");
        s.set_knob(KnobId::MasterPitch, 2.0);
        assert_eq!(s.knob(KnobId::MasterPitch), 1.0);
        s.set_knob(KnobId::MixPan(1), 0.75);
        assert_eq!(s.knob_text(KnobId::MixPan(1)), "Panning: 50% right");
        assert_eq!(s.knob_text(KnobId::Swing), "Main swing: 0%");
        s.project.channels = vec![os_model::Channel { volume: 0.78, ..crate::app::test_project().channels[0].clone() }];
        assert_eq!(s.knob_text(KnobId::ChanVol(0)), "Channel volume: 78%  -5.2dB  0.55");
    }

    /// Every kind of control goes back to its default through `App::reset_control`, as an undo step for the project's.
    #[test]
    fn controls_reset() {
        use crate::app::Action;
        use crate::views::settings::SAct;
        let mut app = crate::app::App::new(None);
        app.state.project = crate::app::test_project();
        assert_eq!(Control::of(&Action::Knob(KnobId::Swing)), Some(Control::Knob(KnobId::Swing)));
        assert_eq!(Control::of(&Action::Fader(2)), Some(Control::Fader(2)));
        assert_eq!(Control::of(&Action::Settings(SAct::ThemeKnob(3))), Some(Control::Theme(3)));
        assert_eq!(Control::of(&Action::Settings(SAct::Slider("Smoothing", 0.0, 9.0))), Some(Control::Slider("Smoothing")));
        assert_eq!(Control::of(&Action::Tempo), None, "the tempo is a value box");
        for id in [KnobId::MasterVol, KnobId::MasterPitch, KnobId::ChanVol(0), KnobId::ChanPan(0), KnobId::Swing, KnobId::MixPan(1), KnobId::MixSep(1), KnobId::SlotMix(1, 0)] {
            app.state.set_knob(id, if id.default_value() < 0.5 { 0.875 } else { 0.125 });
            app.reset_control(Control::Knob(id));
            assert_eq!(app.state.knob(id), id.default_value(), "{id:?}");
            assert_eq!(app.hint, app.state.knob_text(id));
        }
        assert!(crate::history::undo(&mut app.state).is_some(), "an undo step");
        assert_ne!(app.state.knob(KnobId::SlotMix(1, 0)), 1.0, "undone");
        crate::mixer::track_mut(&mut app.state, 1).unwrap().volume = Some(0.2);
        app.reset_control(Control::Fader(1));
        assert_eq!(crate::mixer::track(&app.state, 1).unwrap().volume, Some(0.8));
        assert_eq!(app.hint, "Volume: 100%  0.0dB  1.00");
        app.state.project.channels[0].pitch_cents = 250;
        app.reset_control(Control::ChanPitch(0));
        assert_eq!(app.state.project.channels[0].pitch_cents, 0);
        let s = &crate::prefs::SMOOTHING;
        crate::prefs::set_slider(s, 3);
        app.reset_control(Control::Slider(s.label));
        assert_eq!(crate::prefs::slider(s), s.default);
    }

    #[test]
    fn eq_knobs() {
        let mut app = crate::app::App::new(None);
        let s = &mut app.state;
        let t = s.project.mixer[1].index;
        assert_eq!(s.knob_text(KnobId::Eq(t, 0, 0)), "EQ band 1 (low shelf) level: 0.0dB");
        assert_eq!(s.knob_text(KnobId::Eq(t, 0, 1)), "EQ band 1 (low shelf) freq: 90Hz");
        assert_eq!(s.knob_text(KnobId::Eq(t, 2, 1)), "EQ band 3 (high shelf) freq: 8000Hz");
        assert_eq!(s.knob_text(KnobId::Eq(t, 1, 2)), "EQ band 2 (peaking) width: 0.27");
        for (b, f) in [(0, 0), (1, 1), (2, 2)] {
            assert_eq!(s.knob(KnobId::Eq(t, b, f)), KnobId::Eq(t, b, f).default_value(), "starts at the default");
        }
        s.set_knob(KnobId::Eq(t, 1, 0), 0.25);
        assert_eq!(s.project.mixer[1].eq[1][0], -900);
        assert_eq!(s.knob_text(KnobId::Eq(t, 1, 0)), "EQ band 2 (peaking) level: -9.0dB");
        s.set_knob(KnobId::Eq(t, 2, 1), 0.5);
        assert_eq!(s.project.mixer[1].eq[2][1], 32768);
        assert!(s.piano.dirty);
    }
}
