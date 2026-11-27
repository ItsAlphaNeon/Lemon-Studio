// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Audio clips' own pitch, pan and reverse (Clip Properties, spec: ui/playlist-clip-properties.md). The Lemon Sampler
//! of a channel with a reversed clip keeps a reversed copy of its sample, so the clip's voice can play backwards
//! (Lemon Studio's design; spec: ui/audio-clip-pitch-reverse.md "How it plays"). A `.lemon` file keeps the settings
//! in its own event ([`os_flp::lemon::CLIP_PROPS`]): where FL's clip record keeps them is not measured.

use os_flp::{Event, Value, lemon};
use os_model::{ClipProps, ClipSource, Project};

use crate::lemon_state::{start, Reader};

/// [`lemon::CLIP_PROPS`]: a u32 count, then per clip with settings its arrangement (u16, 0-based), its index in the
/// arrangement's clip list (u32), semitones, cents and pan (i8 each) and flags (u8: 1 reverse).
pub fn event(project: &Project) -> Event {
    let mut v = Vec::new();
    for (a, arr) in project.arrangements.iter().enumerate() {
        for (i, c) in arr.clips.iter().enumerate().filter(|(_, c)| c.props != ClipProps::default()) {
            v.push((a as u16, i as u32, c.props));
        }
    }
    let mut b = start(v.len());
    for (a, i, p) in v {
        b.extend_from_slice(&a.to_le_bytes());
        b.extend_from_slice(&i.to_le_bytes());
        b.extend_from_slice(&[p.semitones as u8, p.cents as u8, p.pan as u8, u8::from(p.reverse)]);
    }
    Event::new(lemon::CLIP_PROPS, Value::var(b))
}

/// Reads [`event`]'s payload (after its layout byte) back into the clips.
pub fn read(project: &mut Project, d: &[u8]) {
    let mut r = Reader(d);
    for _ in 0..r.u32().unwrap_or(0) {
        let (Some(a), Some(i), Some(s), Some(c), Some(p), Some(f)) = (r.u16(), r.u32(), r.u8(), r.u8(), r.u8(), r.u8())
        else {
            break;
        };
        if let Some(clip) = project.arrangements.get_mut(usize::from(a)).and_then(|a| a.clips.get_mut(i as usize)) {
            clip.set_pitch_semitones(i32::from(s as i8));
            clip.set_fine_cents(i32::from(c as i8));
            clip.set_clip_pan(f32::from(p as i8) / 100.0);
            clip.set_reversed(f & 1 != 0);
        }
    }
}

/// The rack rows (channel indexes into `project.channels`) whose clips, in any arrangement, include a reversed one.
pub fn rows_with_reversed(project: &Project) -> Vec<usize> {
    let mut chans: Vec<u16> = project
        .arrangements
        .iter()
        .flat_map(|a| a.clips.iter())
        .filter(|c| c.reversed())
        .filter_map(|c| match c.source {
            ClipSource::Channel(ch) => Some(ch),
            _ => None,
        })
        .collect();
    chans.sort_unstable();
    chans.dedup();
    project
        .channels
        .iter()
        .enumerate()
        .filter(|(_, c)| c.kind.is_sampler() && chans.binary_search(&c.index).is_ok())
        .map(|(r, _)| r)
        .collect()
}

/// Turns on "Keep a reversed copy" in the Lemon Sampler of every channel with a reversed clip. It stays on when the
/// last reversed clip goes (the copy costs memory only); a new instance starts without it.
pub fn sync(ses: &crate::session::Session) {
    for row in rows_with_reversed(&ses.project) {
        let on = ses
            .plugins
            .param_infos(row)
            .into_iter()
            .find(|i| i.name == lemon_sampler::params::REVERSED_COPY)
            .and_then(|i| ses.plugins.param_value(row, i.id))
            .is_some_and(|v| v > 0.5);
        if !on {
            ses.plugins.set_param_named(row, lemon_sampler::params::REVERSED_COPY, 1.0);
        }
    }
}
