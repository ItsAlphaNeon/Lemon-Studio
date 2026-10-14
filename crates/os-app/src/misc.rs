// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

//! The Misc tab's values <-> the channel model (`os_model::ChannelMisc`). The page (`misc_page`) works in knob
//! units (0..1, bipolar knobs centred at 0.5); the model keeps bipolar knobs as -1..1.
// spec: ui/plugin-window.md (Misc tab)

use crate::misc_page::{self as mp, MiscVals, M};
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

/// The page values of channel `ch`: root note and fine tuning play only on sampler channels (`set`).
pub fn vals_of(ch: &Channel) -> MiscVals {
    MiscVals { tuned: ch.kind.is_sampler(), ..vals(&ch.misc) }
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
    let (lo, hi) = mp::range_of(m);
    let v = if v.is_finite() { v.clamp(lo, hi) } else { mp::default_of(m) };
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
        M::ArpChord => misc.arp.chord = whole(v),
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

/// A Misc tab edit of rack row `row`: the channel model, its hosted Lemon Sampler (root / fine), then playback.
pub fn set(app: &mut crate::app::App, row: usize, m: M, v: f32) {
    let Some(ch) = app.state.project.channels.get_mut(row) else { return };
    put(&mut ch.misc, m, v);
    let misc = ch.misc;
    let sampler = ch.kind.is_sampler();
    if sampler {
        if let Some((name, plain)) = sampler_param(&misc, m) {
            app.plugins.set_param_named(row, name, plain);
        }
    }
    app.state.piano.dirty = true;
    app.invalidate();
}

/// The list menu of a Misc value (chord list, number boxes) at canvas point (x, y); the current value is ticked.
#[cfg(plugin_windows)]
pub fn menu(app: &crate::app::App, row: usize, m: M, x: f32, y: f32) -> crate::playlist::Popup {
    use crate::playlist::{Cmd, Mark, PItem, Popup};
    let cur = app.state.project.channels.get(row).map_or(mp::default_of(m), |ch| get(&ch.misc, m));
    let items = mp::list_entries(m)
        .into_iter()
        .map(|(label, v)| PItem::new(label, Cmd::Misc(row, m, v)).mark(Mark::Radio((v - cur).abs() < 0.5)))
        .collect();
    Popup { x, y, items, sub: None }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_value_round_trips_through_the_model() {
        let mut misc = ChannelMisc::default();
        // Defaults agree with the page's defaults.
        for m in M::ALL {
            assert!((get(&misc, m) - mp::default_of(m)).abs() < 1e-6, "{m:?}: {} vs {}", get(&misc, m), mp::default_of(m));
        }
        for m in M::ALL {
            let (lo, hi) = mp::range_of(m);
            let mut v = if hi > 1.0 { (lo + 3.0).min(hi) } else { 0.25 };
            put(&mut misc, m, v);
            if (get(&misc, m) - v).abs() > 1e-6 {
                // A switch: flip it.
                v = 1.0 - mp::default_of(m);
                put(&mut misc, m, v);
            }
            assert!((get(&misc, m) - v).abs() < 1e-6, "{m:?}");
        }
        assert_eq!(misc.levels[0], -0.5);
        put(&mut misc, M::Fine, 250.0);
        assert_eq!(misc.fine, 100);
    }
}
