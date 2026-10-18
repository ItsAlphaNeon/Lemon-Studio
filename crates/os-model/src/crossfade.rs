// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Automatic crossfades: with the project's switch on (event 44), audio clips that overlap on one track crossfade. A
//! crossfade is two ordinary fades in the clip records, the earlier clip's fade-out and the later clip's fade-in,
//! written without their "set by hand" bits (byte 56). An edge whose bit is clear belongs to the switch and follows
//! the clips ([`derive`]); one whose bit is set keeps its own fade. The records keep FL's bytes: the lengths and
//! tensions are the fades' own fields, the bits are never touched here.
//!
// spec: audio/audio-clips.md "Automatic crossfades", flp/events/playlist.md (event 233, bytes 36..56)

use crate::{Channel, Clip, ClipSource, Project};

/// A fade: (milliseconds, tension).
pub type Fade = (f32, f32);

/// A clip's longest overlaps with the audio clips of its track: for its fade-in the earlier clip (index, ticks), for
/// its fade-out the later one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Overlaps {
    pub fade_in: Option<(usize, u32)>,
    pub fade_out: Option<(usize, u32)>,
}

/// Whether the project's automatic crossfades are on (event 44; a file without it: off).
pub fn on(project: &Project) -> bool {
    project.auto_crossfades == Some(true)
}

/// Per channel index, whether its clips are audio clips: sampler and audio clip channels, the clips with fades (the
/// engine plays both as audio clips).
fn audio_channels(channels: &[Channel]) -> Vec<bool> {
    let mut v = vec![false; channels.iter().map(|c| usize::from(c.index) + 1).max().unwrap_or(0)];
    for c in channels.iter().filter(|c| c.kind.is_sampler()) {
        v[usize::from(c.index)] = true;
    }
    v
}

fn audio(kinds: &[bool], c: &Clip) -> bool {
    matches!(c.source, ClipSource::Channel(ch) if kinds.get(usize::from(ch)).copied().unwrap_or(false))
}

/// Whether `c` is an audio clip (a clip of a sampler or audio clip channel): only these crossfade.
pub fn is_audio(channels: &[Channel], c: &Clip) -> bool {
    let ClipSource::Channel(ch) = c.source else { return false };
    channels.iter().find(|x| x.index == ch).is_some_and(|x| x.kind.is_sampler())
}

/// The overlap two clips of one track make, `a` the earlier: `a` starts first, `b` starts before `a` ends and ends
/// after it; the overlap is `a`'s end − `b`'s start, in ticks. A clip lying wholly inside the other, or one that only
/// touches it, makes none (spec: "Lemon Studio" rules).
fn overlap(a: &Clip, b: &Clip) -> Option<u32> {
    (a.track == b.track && a.position < b.position && b.position < a.end() && b.end() > a.end()).then(|| a.end() - b.position)
}

/// Whether candidate `(clip, ticks)` beats `old`: the longer overlap, on a tie the clip that starts first (then the
/// first in the list).
fn better(clips: &[Clip], new: (usize, u32), old: Option<(usize, u32)>) -> bool {
    old.is_none_or(|old| new.1 > old.1 || (new.1 == old.1 && (clips[new.0].position, new.0) < (clips[old.0].position, old.0)))
}

/// Every clip's overlaps, index-aligned with `clips` (pattern and automation clips have none).
pub fn overlaps(channels: &[Channel], clips: &[Clip]) -> Vec<Overlaps> {
    let kinds = audio_channels(channels);
    let mut out = vec![Overlaps::default(); clips.len()];
    let mut order: Vec<usize> = (0..clips.len()).filter(|&i| audio(&kinds, &clips[i])).collect();
    order.sort_by_key(|&i| (clips[i].track, clips[i].position, i));
    for (k, &a) in order.iter().enumerate() {
        let ca = &clips[a];
        for &b in &order[k + 1..] {
            let cb = &clips[b];
            if cb.track != ca.track || cb.position >= ca.end() {
                break;
            }
            let Some(ticks) = overlap(ca, cb) else { continue };
            if better(clips, (a, ticks), out[b].fade_in) {
                out[b].fade_in = Some((a, ticks));
            }
            if better(clips, (b, ticks), out[a].fade_out) {
                out[a].fade_out = Some((b, ticks));
            }
        }
    }
    out
}

/// Clip `i`'s overlaps alone (as [`overlaps`] gives them, without sorting the list).
pub fn overlaps_of(channels: &[Channel], clips: &[Clip], i: usize) -> Overlaps {
    let mut out = Overlaps::default();
    let Some(c) = clips.get(i).filter(|c| is_audio(channels, c)) else { return out };
    let kinds = audio_channels(channels);
    for (j, d) in clips.iter().enumerate().filter(|&(j, d)| j != i && audio(&kinds, d)) {
        if let Some(ticks) = overlap(d, c).filter(|&t| better(clips, (j, t), out.fade_in)) {
            out.fade_in = Some((j, ticks));
        }
        if let Some(ticks) = overlap(c, d).filter(|&t| better(clips, (j, t), out.fade_out)) {
            out.fade_out = Some((j, ticks));
        }
    }
    out
}

/// `ticks` in milliseconds at `tempo` (quarter notes a minute: the project's [`Project::tick_tempo`]) and `ppq`: the
/// unit of a fade's length.
pub fn ms(ticks: u32, tempo: f64, ppq: u16) -> f32 {
    (f64::from(ticks) * 60_000.0 / (tempo.max(1.0) * f64::from(ppq.max(1)))) as f32
}

/// The tension a crossfade's fade-out takes from the later clip's fade-in tension `t`: `-t` (0, not -0, for 0, so the
/// stored bytes are FL's; an unusable value counts as 0).
pub fn opposite(t: f32) -> f32 {
    if t == 0.0 || !t.is_finite() { 0.0 } else { -t }
}

/// Whether a stored length can stay for `want`: equal, or within a thousandth of a millisecond (and a millionth of
/// the length), so the bytes FL wrote for a crossfade are kept rather than written again from our arithmetic.
fn keeps(stored: f32, want: f32) -> bool {
    if want == 0.0 {
        stored == 0.0
    } else {
        (stored - want).abs() <= 1e-3f32.max(want.abs() * 1e-6)
    }
}

/// Sets the edges of `clips` that belong to the switch from their overlaps (see [`derive`]).
fn derive_clips(channels: &[Channel], clips: &mut [Clip], tempo: f64, ppq: u16) -> bool {
    let ov = overlaps(channels, clips);
    let kinds = audio_channels(channels);
    let mut changed = false;
    for i in 0..clips.len() {
        if !audio(&kinds, &clips[i]) {
            continue;
        }
        let o = ov[i];
        if !clips[i].fade_in_set() {
            let want = o.fade_in.map_or(0.0, |(_, t)| ms(t, tempo, ppq));
            if !keeps(clips[i].f32_at(36), want) {
                clips[i].set_fade_in_length(want);
                changed = true;
            }
        }
        if !clips[i].fade_out_set() {
            let want = o.fade_out.map_or(0.0, |(_, t)| ms(t, tempo, ppq));
            if !keeps(clips[i].f32_at(44), want) {
                clips[i].set_fade_out_length(want);
                changed = true;
            }
            if let Some((b, _)) = o.fade_out {
                let t = opposite(clips[b].fade_in_tension());
                if clips[i].fade_out_tension() != t {
                    clips[i].set_fade_out_tension(t);
                    changed = true;
                }
            }
        }
    }
    changed
}

/// With the switch on, sets every audio clip edge that belongs to it (its bit clear) from the overlaps, in every
/// arrangement: a fade-in to its longest overlap with an earlier clip, keeping its own tension `t`; a fade-out to its
/// longest overlap with a later clip, with that clip's fade-in tension negated, so the two gains add up to 1; 0 where
/// nothing overlaps, the tension kept. Edges with their bit set, and the bits, stay. `tempo` is the project's tempo
/// (the lengths are milliseconds, at the tempo the ticks run at). Returns whether a record changed; nothing changes
/// while the switch is off.
pub fn derive(project: &mut Project, tempo: f64) -> bool {
    if !on(project) {
        return false;
    }
    let tempo = project.tick_tempo(tempo);
    let Project { channels, arrangements, ppq, .. } = project;
    let mut changed = false;
    for a in arrangements.iter_mut() {
        changed |= derive_clips(channels, &mut a.clips, tempo, *ppq);
    }
    changed
}

/// The switch turned off: the crossfades' lengths go to 0 (every audio clip edge without its bit, in every
/// arrangement) and their tensions stay. Returns whether a record changed.
pub fn clear(project: &mut Project) -> bool {
    let kinds = audio_channels(&project.channels);
    let mut changed = false;
    for c in project.arrangements.iter_mut().flat_map(|a| a.clips.iter_mut()).filter(|c| audio(&kinds, c)) {
        if !c.fade_in_set() && c.f32_at(36) != 0.0 {
            c.set_fade_in_length(0.0);
            changed = true;
        }
        if !c.fade_out_set() && c.f32_at(44) != 0.0 {
            c.set_fade_out_length(0.0);
            changed = true;
        }
    }
    changed
}

/// The fades `clips` (one arrangement's) play, index-aligned: (fade-in, fade-out). With the switch off every stored
/// fade plays, bits or not; with it on an edge with its bit set plays as stored and one without it as the overlaps
/// make it ([`derive`]), whatever its record holds.
pub fn playing(project: &Project, clips: &[Clip], tempo: f64) -> Vec<(Option<Fade>, Option<Fade>)> {
    let fades = |c: &Clip| (c.stored_fade_in(), c.stored_fade_out());
    if !on(project) {
        return clips.iter().map(fades).collect();
    }
    let mut own = clips.to_vec();
    derive_clips(&project.channels, &mut own, project.tick_tempo(tempo), project.ppq);
    own.iter().map(fades).collect()
}

/// The earlier clip clip `b` crossfades with, drawn as one crossfade (spec: ui/playlist.md §9a): the switch is on,
/// `b`'s fade-in and that clip's fade-out both belong to it, and each is the other's longest overlap.
pub fn partner_before(project: &Project, clips: &[Clip], b: usize) -> Option<usize> {
    if !on(project) || clips.get(b).is_none_or(Clip::fade_in_set) {
        return None;
    }
    let (a, _) = overlaps_of(&project.channels, clips, b).fade_in?;
    let mutual = overlaps_of(&project.channels, clips, a).fade_out.map(|o| o.0) == Some(b);
    (mutual && !clips[a].fade_out_set()).then_some(a)
}

/// The later clip clip `a` crossfades with ([`partner_before`] from the other side).
pub fn partner_after(project: &Project, clips: &[Clip], a: usize) -> Option<usize> {
    let (b, _) = overlaps_of(&project.channels, clips, a).fade_out?;
    (partner_before(project, clips, b) == Some(a)).then_some(b)
}

/// The tension a crossfade's handle gives for the later clip's gain `m` at the middle (spec: ui/playlist.md §9a, a
/// Lemon Studio rule): `r = m / (1 - m)`, `t = r² - 1` from the middle up; `r = (1 - m) / m`, `t = -(r² - 1)` below it;
/// held within -64..64. The inverse of `fade_curve(0.5, t)`.
pub fn tension_for_mid(m: f64) -> f64 {
    let m = m.clamp(1e-9, 1.0 - 1e-9);
    let t = if m >= 0.5 {
        let r = m / (1.0 - m);
        r * r - 1.0
    } else {
        let r = (1.0 - m) / m;
        -(r * r - 1.0)
    };
    t.clamp(-MAX_TENSION, MAX_TENSION)
}

/// The largest tension a crossfade's handle sets (either sign).
pub const MAX_TENSION: f64 = 64.0;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{fade_curve, Arrangement, ChannelKind};

    fn channel(index: u16, kind: ChannelKind) -> Channel {
        Channel {
            index,
            kind,
            name: None,
            plugin: None,
            plugin_state: None,
            color: None,
            enabled: true,
            volume: 0.78125,
            pan: 0.0,
            pitch_cents: 0,
            mixer_track: 0,
            sample_path: None,
            layer_children: vec![],
            envelopes: Default::default(),
            rack: Default::default(),
            misc: Default::default(),
            sound: Default::default(),
        }
    }

    /// Channel 0 an audio clip channel, 1 an automation channel, 2 a plugin; 130 BPM, PPQ 96, the switch on.
    fn project(clips: Vec<Clip>) -> Project {
        Project {
            ppq: 96,
            tempo: Some(130.0),
            auto_crossfades: Some(true),
            channels: vec![channel(0, ChannelKind::AudioClip), channel(1, ChannelKind::Automation), channel(2, ChannelKind::Plugin)],
            arrangements: vec![Arrangement { clips, ..Default::default() }],
            ..Project::default()
        }
    }

    fn audio(position: u32, length: u32, track: u16) -> Clip {
        Clip::new(position, length, track, ClipSource::Channel(0))
    }

    fn clips(p: &Project) -> &[Clip] {
        &p.arrangements[0].clips
    }

    /// (fade-in, fade-out) lengths as the records hold them.
    fn lengths(p: &Project) -> Vec<(f32, f32)> {
        clips(p).iter().map(|c| (c.stored_fade_in().map_or(0.0, |f| f.0), c.stored_fade_out().map_or(0.0, |f| f.0))).collect()
    }

    /// 240 ticks at 130 BPM and PPQ 96 (the spec's measured crossfade).
    const XF: f32 = 1153.846_2;

    #[test]
    fn two_overlapping_clips_crossfade() {
        let mut p = project(vec![audio(0, 480, 0), audio(240, 480, 0)]);
        assert!(derive(&mut p, 130.0));
        let l = lengths(&p);
        assert!((l[0].1 - XF).abs() < 1e-3 && (l[1].0 - XF).abs() < 1e-3, "{l:?}");
        assert_eq!((l[0].0, l[1].1), (0.0, 0.0), "the outer edges overlap nothing");
        // Written without the bits, at tension 0 (+0, not -0).
        let (a, b) = (clips(&p)[0], clips(&p)[1]);
        assert_eq!(a.raw[56] & 3, 0);
        assert_eq!(b.raw[56] & 3, 0);
        assert_eq!((a.fade_out(), b.fade_in()), (None, None), "not fades set by hand");
        assert_eq!(&a.raw[48..52], &[0; 4]);
        // Again: nothing to do.
        assert!(!derive(&mut p, 130.0));
        // The later clip's tension t gives the earlier one -t.
        p.arrangements[0].clips[1].set_fade_in_tension(2.289);
        assert!(derive(&mut p, 130.0));
        assert_eq!(clips(&p)[0].fade_out_tension(), -2.289);
        assert_eq!(clips(&p)[1].fade_in_tension(), 2.289, "the fade-in keeps its own");
        // Moved apart: both lengths back to 0, the tensions kept; overlapped again: set again.
        p.arrangements[0].clips[1].position = 600;
        assert!(derive(&mut p, 130.0));
        assert_eq!(lengths(&p), vec![(0.0, 0.0); 2]);
        assert_eq!((clips(&p)[0].fade_out_tension(), clips(&p)[1].fade_in_tension()), (-2.289, 2.289));
        p.arrangements[0].clips[1].position = 360;
        derive(&mut p, 130.0);
        let l = lengths(&p);
        assert!((l[0].1 - XF / 2.0).abs() < 1e-3 && (l[1].0 - XF / 2.0).abs() < 1e-3, "{l:?}");
    }

    #[test]
    fn the_crossfade_plays_at_unity() {
        let mut p = project(vec![audio(0, 480, 0), audio(240, 480, 0)]);
        for t in [0.0f32, 1.0, -2.0, 8.0, 2.289, -4.736] {
            p.arrangements[0].clips[1].set_fade_in_tension(t);
            let f = playing(&p, clips(&p), 130.0);
            let ((l_in, t_in), (l_out, t_out)) = (f[1].0.unwrap(), f[0].1.unwrap());
            assert_eq!((l_in, t_out), (l_out, opposite(t_in)));
            for k in 0..=100 {
                let x = f64::from(k) / 100.0;
                // At x into the overlap the later clip's fade-in is at x, the earlier clip's fade-out at 1 - x.
                let sum = fade_curve(x, f64::from(t_in)) + fade_curve(1.0 - x, f64::from(t_out));
                assert!((sum - 1.0).abs() < 1e-12, "t {t} x {x}: {sum}");
            }
        }
    }

    #[test]
    fn three_in_a_row() {
        // A 0..480, B 240..720, C 600..900: B fades in from A and out into C.
        let mut p = project(vec![audio(0, 480, 0), audio(240, 480, 0), audio(600, 300, 0)]);
        derive(&mut p, 130.0);
        let l = lengths(&p);
        let ms = |t: u32| ms(t, 130.0, 96);
        assert_eq!(l, vec![(0.0, ms(240)), (ms(240), ms(120)), (ms(120), 0.0)]);
        // A reaching into C too: C's fade-in takes the longer overlap (with B, 120 ticks, against A's 20).
        p.arrangements[0].clips[0].length = 620;
        p.arrangements[0].clips[2].length = 400;
        derive(&mut p, 130.0);
        let o = overlaps(&p.channels, clips(&p));
        assert_eq!(o[2].fade_in, Some((1, 120)));
        assert_eq!(o[0].fade_out, Some((1, 380)), "A's fade-out: its longest, B");
        assert_eq!(lengths(&p)[2].0, ms(120));
        for i in 0..3 {
            assert_eq!(overlaps_of(&p.channels, clips(&p), i), o[i], "clip {i} alone");
        }
        assert_eq!(partner_before(&p, clips(&p), 1), Some(0));
        assert_eq!(partner_before(&p, clips(&p), 2), Some(1));
        assert_eq!(partner_after(&p, clips(&p), 0), Some(1));
        assert_eq!(partner_after(&p, clips(&p), 2), None);
    }

    #[test]
    fn contained_touching_and_other_tracks_make_none() {
        // Contained: B inside A; touching: C starts where A ends; D on another track; E starts with A (and is longer).
        let mut p = project(vec![audio(0, 480, 0), audio(100, 200, 0), audio(480, 200, 0), audio(240, 480, 1), audio(0, 600, 2), audio(0, 480, 2)]);
        // Stale lengths without bits: set back to 0.
        for c in &mut p.arrangements[0].clips {
            c.set_fade_in_length(300.0);
            c.set_fade_out_length(300.0);
        }
        assert!(derive(&mut p, 130.0));
        assert_eq!(lengths(&p), vec![(0.0, 0.0); 6]);
        assert!(overlaps(&p.channels, clips(&p)).iter().all(|o| *o == Overlaps::default()));
    }

    #[test]
    fn only_audio_clips() {
        // A pattern clip, an automation clip and a plugin channel's clip over an audio clip: no crossfade; their
        // records are not touched.
        let mut pat = Clip::new(240, 480, 0, ClipSource::Pattern(1));
        pat.set_fade_in_length(50.0);
        let auto = Clip::new(240, 480, 0, ClipSource::Channel(1));
        let plug = Clip::new(240, 480, 0, ClipSource::Channel(2));
        let mut p = project(vec![audio(0, 480, 0), pat, auto, plug]);
        let before = p.clone();
        assert!(!derive(&mut p, 130.0));
        assert_eq!(p, before);
        // A sampler channel's clips play as audio clips: they crossfade.
        p.channels[2].kind = ChannelKind::Sampler;
        derive(&mut p, 130.0);
        assert!((lengths(&p)[0].1 - XF).abs() < 1e-3);
        assert!((lengths(&p)[3].0 - XF).abs() < 1e-3);
    }

    #[test]
    fn edges_set_by_hand_are_kept() {
        let mut p = project(vec![audio(0, 480, 0), audio(240, 480, 0), audio(1000, 100, 0)]);
        let c = &mut p.arrangements[0].clips;
        c[0].set_fade_out(Some((100.0, 3.0)));
        c[1].set_fade_in(Some((50.0, -1.0)));
        c[2].set_fade_in(Some((20.0, 0.5)));
        let bits: Vec<u8> = clips(&p).iter().map(|c| c.raw[56]).collect();
        derive(&mut p, 130.0);
        let c = clips(&p);
        assert_eq!((c[0].fade_out(), c[1].fade_in(), c[2].fade_in()), (Some((100.0, 3.0)), Some((50.0, -1.0)), Some((20.0, 0.5))));
        assert_eq!(c.iter().map(|c| c.raw[56]).collect::<Vec<_>>(), bits, "the bits stay");
        // Only one side set by hand: the other still gets the overlap; the fade-out takes the fade-in's tension.
        p.arrangements[0].clips[0].set_fade_out(None);
        derive(&mut p, 130.0);
        let c = clips(&p);
        assert!((c[0].stored_fade_out().unwrap().0 - XF).abs() < 1e-3);
        assert_eq!(c[0].fade_out_tension(), 1.0);
        assert_eq!(c[1].fade_in(), Some((50.0, -1.0)));
    }

    #[test]
    fn switch_off_plays_the_records_and_clears_crossfades() {
        let mut p = project(vec![audio(0, 480, 0), audio(240, 480, 0), audio(2000, 480, 0)]);
        p.arrangements[0].clips[1].set_fade_in_tension(4.0);
        derive(&mut p, 130.0);
        // A fade without its bit on an edge that overlaps nothing: not played with the switch on, whatever the record
        // holds; played with it off.
        p.arrangements[0].clips[2].set_fade_in_length(200.0);
        assert_eq!(playing(&p, clips(&p), 130.0)[2].0, None);
        p.auto_crossfades = Some(false);
        assert_eq!(playing(&p, clips(&p), 130.0)[2].0, Some((200.0, 0.0)));
        assert!(!derive(&mut p, 130.0), "nothing follows the clips while it is off");
        // Turned off: the crossfades' lengths go to 0, their tensions stay; edges set by hand stay.
        p.arrangements[0].clips[2].set_fade_out(Some((80.0, 1.0)));
        assert!(clear(&mut p));
        assert_eq!(lengths(&p), vec![(0.0, 0.0), (0.0, 0.0), (0.0, 80.0)]);
        assert_eq!((clips(&p)[0].fade_out_tension(), clips(&p)[1].fade_in_tension()), (-4.0, 4.0));
        // A file without event 44: off.
        p.auto_crossfades = None;
        assert!(!on(&p));
        // Turned on again: set again.
        p.auto_crossfades = Some(true);
        assert!(derive(&mut p, 130.0));
        assert!((lengths(&p)[1].0 - XF).abs() < 1e-3);
    }

    #[test]
    fn lengths_follow_the_tempo() {
        let mut p = project(vec![audio(0, 480, 0), audio(240, 480, 0)]);
        derive(&mut p, 130.0);
        assert!(derive(&mut p, 65.0));
        assert!((lengths(&p)[1].0 - 2.0 * XF).abs() < 1e-3);
        assert_eq!(playing(&p, clips(&p), 120.0)[1].0.unwrap().0, ms(240, 120.0, 96));
    }

    #[test]
    fn lengths_follow_the_bars_beat() {
        // In 3/8 the tempo counts eighths: the ticks run at half the speed, so 240 ticks take twice as long.
        let mut p = project(vec![audio(0, 480, 0), audio(240, 480, 0)]);
        p.bar = Some((3, 2));
        assert!(derive(&mut p, 130.0));
        assert!((lengths(&p)[1].0 - 2.0 * XF).abs() < 1e-3, "{:?}", lengths(&p));
        assert_eq!(playing(&p, clips(&p), 130.0)[1].0.unwrap().0, ms(240, 65.0, 96));
    }

    #[test]
    fn stored_lengths_close_to_ours_are_kept() {
        // FL's own bytes for the measured crossfade, an ulp or so from ours: kept as they are.
        let mut p = project(vec![audio(0, 480, 0), audio(240, 480, 0)]);
        let fl = f32::from_bits(ms(240, 130.0, 96).to_bits() + 1);
        p.arrangements[0].clips[0].set_fade_out_length(fl);
        p.arrangements[0].clips[1].set_fade_in_length(fl);
        assert!(!derive(&mut p, 130.0));
        assert_eq!(clips(&p)[1].stored_fade_in().unwrap().0.to_bits(), fl.to_bits());
    }

    #[test]
    fn the_handle_inverts_the_middle() {
        for t in [-64.0, -8.0, -1.0, -0.25, 0.0, 0.5, 2.289, 16.0, 64.0] {
            let m = fade_curve(0.5, t);
            assert!((tension_for_mid(m) - t).abs() < 1e-9 * t.abs().max(1.0), "t {t}: {}", tension_for_mid(m));
        }
        // The two drags measured at 100 % (18 px up from 0, then 40 px down; FL stored 2.289, then -4.736) at 1/120 of
        // the gain per pixel: r = 0.65 / 0.35, then r = (1 - 0.65 + 1/3) / (0.65 - 1/3).
        let up = tension_for_mid(0.5 + 18.0 / 120.0);
        assert!((up - 120.0 / 49.0).abs() < 1e-9, "{up}");
        let down = tension_for_mid(fade_curve(0.5, up) - 40.0 / 120.0);
        let r = (0.35 + 1.0 / 3.0) / (0.65 - 1.0 / 3.0);
        assert!((down + (r * r - 1.0)).abs() < 1e-9, "{down}");
        assert_eq!((tension_for_mid(0.99), tension_for_mid(0.01)), (64.0, -64.0));
    }
}
