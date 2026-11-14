// Copyright (C) 2026 ItsAlphaNeon
// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The project's initial values (`Project::init_controls`): controls that take a value when the song starts. The
//! Automation group of a control's menu adds one, "Set as song start value" (one undo step, "Initialized
//! control"), and takes it out, "Remove start value" (no undo step, as in FL). With "Initialize controls
//! automatically" on, "Create automation clip" adds the control too, in its own step (`playlist::create_automation`).
//! In song mode, starting playback, moving the song position and playback looping back to the song's start set every
//! control in the list to its value (`apply`; the engine sets them itself where it loops), then automation clips act as
//! usual. The browser lists them under Current project ▸ Initialized controls (`views::browser`); the file holds them
//! in event 216 (`os_flp::project`).
// spec: ui/settings-choices.md "Advanced: Initialize controls automatically (2026-10-16)"

#![allow(deprecated)] // writes the project directly (`crate::locked`; control-plane step 14 removes this)
use os_model::target::{self, Target};
use os_model::InitControl;

use crate::history::{self, Snap};

/// The undo step of "Set as song start value" (FL's name).
pub const INITIALIZED: &str = "Initialized control";
pub const INIT_LABEL: &str = "Set as song start value";
pub const DELETE_LABEL: &str = "Remove start value";

/// Control `t`'s entry in the list.
pub fn entry(s: &crate::session::Session, t: u32) -> Option<InitControl> {
    s.project.init_controls.iter().find(|c| c.target == t).copied()
}

/// Whether control `t` is in the list (the check mark of "Set as song start value").
pub fn listed(s: &crate::session::Session, t: u32) -> bool {
    entry(s, t).is_some()
}

/// Puts control `t`'s entry in the list (`None`: takes it out). An entry already there keeps its place.
pub fn put(s: &mut crate::session::Session, t: u32, e: Option<InitControl>) {
    let list = &mut s.project.get_mut().init_controls;
    match e {
        Some(c) => match list.iter_mut().find(|x| x.target == t) {
            Some(x) => *x = c,
            None => list.push(c),
        },
        None => list.retain(|x| x.target != t),
    }
    // Playback gets the list (the engine sets it where a song loops).
    s.dirty = true;
}

/// "Set as song start value": control `t`, at its value `v` (0..1), goes in the list, replacing an earlier value.
/// One undo step, "Initialized control". Nothing for a control whose stored units aren't known (a plugin parameter).
pub fn init(s: &mut crate::session::Session, t: u32, v: f64) {
    let Some(c) = InitControl::from_unit(t, v) else { return };
    let snap = Snap::Init(t, entry(s, t));
    history::record(s, INITIALIZED, snap);
    put(s, t, Some(c));
}

/// "Remove start value": control `t` leaves the list. No undo step (FL makes none).
pub fn delete(s: &mut crate::session::Session, t: u32) {
    put(s, t, None);
}

/// What "Create automation clip" on control `t` (value `v`) puts in the list: its entry while "Initialize controls
/// automatically" is on, `None` while it is off or for a control whose stored units aren't known.
pub fn automatic(st: &crate::settings_file::SettingsStore, t: u32, v: f64) -> Option<InitControl> {
    st.check(&crate::prefs::INIT_CONTROLS).then(|| InitControl::from_unit(t, v)).flatten()
}

/// In song mode: every control in the list takes its value at once (where playback starts, the song position moves
/// or playback loops back to the song's start). Not an undo step. A record whose units aren't known, or whose control
/// the project doesn't have, is left alone. Returns whether a value changed; the project then goes to playback.
pub fn apply(s: &mut crate::session::Session) -> bool {
    if !s.song_mode() {
        return false;
    }
    let mut changed = false;
    for c in s.project.init_controls.clone() {
        changed |= set_value(s, c);
    }
    if changed {
        s.dirty = true;
    }
    changed
}

/// `*x = v`; whether it changed.
fn set<T: PartialEq>(x: &mut T, v: T) -> bool {
    let changed = *x != v;
    *x = v;
    changed
}

/// Control `c.target` set to `c.value` (stored units) in the model. The project counts as written only when a value
/// changed.
fn set_value(s: &mut crate::session::Session, c: InitControl) -> bool {
    let main = matches!(c.target, InitControl::MAIN_VOLUME | InitControl::MAIN_SWING | InitControl::MAIN_PITCH);
    if !main && matches!(target::decode(c.target), Target::Tempo) {
        let bpm = (f64::from(c.value) / 1000.0).clamp(10.0, 522.0);
        let changed = s.tempo() != bpm;
        s.set_tempo(bpm);
        return changed;
    }
    s.project.write_if(|p| set_in_project(p, c))
}

/// `set_value` for a control the project holds.
fn set_in_project(p: &mut os_model::Project, c: InitControl) -> bool {
    let v = c.value;
    let f = f64::from(v);
    match c.target {
        InitControl::MAIN_VOLUME => return set(&mut p.main_volume, Some(v.clamp(0, 16000) as u16)),
        InitControl::MAIN_SWING => return set(&mut p.swing, Some(v.clamp(0, 128) as u8)),
        InitControl::MAIN_PITCH => return set(&mut p.main_pitch, Some(v.clamp(-1200, 1200) as i16)),
        _ => {}
    }
    let last = crate::edit::mixer::max_insert(p);
    match target::decode(c.target) {
        // The rack's mixer box (the track number, at most the last insert) and LED (1 = on) (spec: flp/engine-events.md
        // "Channel rack records").
        Target::ChannelMixerTrack(ch) => p.channels.iter_mut().find(|x| x.index == ch).is_some_and(|x| set(&mut x.mixer_track, v.clamp(0, i32::from(last)) as u16)),
        Target::ChannelOn(ch) => p.channels.iter_mut().find(|x| x.index == ch).is_some_and(|x| set(&mut x.enabled, v != 0)),
        Target::ChannelVolume(ch) => p.channels.iter_mut().find(|x| x.index == ch).is_some_and(|x| set(&mut x.volume, (f / 12800.0).clamp(0.0, 1.0))),
        Target::ChannelPan(ch) => p.channels.iter_mut().find(|x| x.index == ch).is_some_and(|x| set(&mut x.pan, (f / 6400.0 - 1.0).clamp(-1.0, 1.0))),
        Target::ChannelPitch(ch) => p.channels.iter_mut().find(|x| x.index == ch).is_some_and(|x| set(&mut x.pitch_cents, v)),
        Target::MixerVolume(t) => p.mixer.iter_mut().find(|m| m.index == t).is_some_and(|m| set(&mut m.volume, Some((f / 16000.0).clamp(0.0, 1.0)))),
        Target::MixerPan(t) => p.mixer.iter_mut().find(|m| m.index == t).is_some_and(|m| set(&mut m.pan, Some((f / 6400.0).clamp(-1.0, 1.0)))),
        Target::MixerSeparation(t) => {
            p.mixer.iter_mut().find(|m| m.index == t).is_some_and(|m| set(&mut m.stereo_separation, Some((f / 64.0).clamp(-1.0, 1.0))))
        }
        Target::MixerEq(t, b, k) => p
            .mixer
            .iter_mut()
            .find(|m| m.index == t)
            .and_then(|m| m.eq.get_mut(usize::from(b))?.get_mut(usize::from(k)))
            .is_some_and(|x| set(x, v)),
        Target::MixerSend(t, d) => p.mixer.iter_mut().find(|m| m.index == t).is_some_and(|m| {
            let level = (f / 16000.0).clamp(0.0, 1.0);
            match m.send_levels.iter_mut().find(|x| x.0 == d) {
                Some(x) => set(&mut x.1, level),
                None => {
                    m.send_levels.push((d, level));
                    m.send_levels.sort_by_key(|x| x.0);
                    true
                }
            }
        }),
        Target::SlotOn(t, k) => p.mixer.iter_mut().find(|m| m.index == t).is_some_and(|m| set(&mut m.slot_on[usize::from(k).min(9)], v != 0)),
        Target::SlotMix(t, k) => {
            p.mixer.iter_mut().find(|m| m.index == t).is_some_and(|m| set(&mut m.slot_mix[usize::from(k).min(9)], (f / 12800.0).clamp(0.0, 1.0)))
        }
        _ => false,
    }
}

#[cfg(any(test, feature = "test-util"))]
mod tests {
    use super::*;

    fn rec(target: u32, value: i32) -> InitControl {
        InitControl { head: 0, target, value }
    }

}
