// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Chopping playlist clips: the clip menu's Chop into ▸ Bar / Beat / Beat Random, and Edit ▸ Slice and insert space.
//! Each is one undo step of the arrangement.
// spec: ui/playlist-chop.md

use crate::app::State;
use crate::playlist::{arr_edit, clips, split, Bars, ARRANGER_EDIT};

/// Where Chop cuts (§1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    Bar,
    Beat,
    /// At every beat, then the beat-long pieces play in a random order (§1).
    BeatRandom,
}

/// The song's bar and beat lines strictly inside `a..b` (ticks), as the playlist's grid has them.
fn lines(s: &State, a: u32, b: u32, unit: Unit) -> Vec<u32> {
    let bars = Bars::new(&s.session);
    let ppq = crate::playlist::ppq(&s.session).max(1);
    let mut out = Vec::new();
    for (_, at, len) in bars.lines(0, b) {
        let step = if unit == Unit::Bar { len.max(1) } else { ppq };
        let mut t = at;
        while t < at + len {
            if t > a && t < b {
                out.push(t);
            }
            t += step;
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// A small deterministic random sequence (xorshift), so a test can follow it.
fn next(seed: &mut u64) -> u64 {
    *seed ^= *seed << 13;
    *seed ^= *seed >> 7;
    *seed ^= *seed << 17;
    *seed
}

/// The ticks Chop cuts clip `i` at (§1).
pub fn cuts(s: &State, i: usize, unit: Unit) -> Vec<u32> {
    let Some(c) = clips(&s.session).get(i) else { return Vec::new() };
    lines(s, c.position, c.end(), unit)
}

/// Chops clips `idx` (§1); Beat Random then shuffles the beat-long pieces' content with `seed`. Returns how many cuts
/// were made.
pub fn chop(s: &mut State, idx: &[usize], unit: Unit, seed: u64) -> usize {
    let mut seed = seed | 1;
    let plan: Vec<(usize, Vec<u32>)> = idx.iter().map(|&i| (i, cuts(s, i, unit))).collect();
    let ppq = crate::playlist::ppq(&s.session).max(1);
    arr_edit(s, ARRANGER_EDIT, |s| {
        let mut n = 0;
        let mut sel = s.playlist.sel.clone();
        for (i, ticks) in plan {
            let mut pieces = vec![i];
            // From the right: clip `i` stays the leftmost piece.
            for &t in ticks.iter().rev() {
                if let Some(j) = split(&mut s.session, i, t) {
                    n += 1;
                    pieces.push(j);
                    if sel.contains(&i) {
                        sel.push(j);
                    }
                }
            }
            if unit == Unit::BeatRandom {
                shuffle(s, &pieces, ppq, &mut seed);
            }
        }
        s.playlist.sel = sel;
        n
    })
}

/// Beat Random's reorder: the content (windows) of the beat-long pieces among `pieces` change places at random; a
/// shorter piece at either end keeps its own.
fn shuffle(s: &mut State, pieces: &[usize], beat: u32, seed: &mut u64) {
    let arr = crate::playlist::arr_mut(&mut s.session);
    let mut full: Vec<usize> = pieces.iter().copied().filter(|&j| arr.clips.get(j).is_some_and(|c| c.length == beat)).collect();
    full.sort_by_key(|&j| arr.clips[j].position);
    let mut windows: Vec<_> = full.iter().map(|&j| arr.clips[j].window).collect();
    for k in (1..windows.len()).rev() {
        let r = (next(seed) % (k as u64 + 1)) as usize;
        windows.swap(k, r);
    }
    for (&j, w) in full.iter().zip(windows) {
        arr.clips[j].window = w;
    }
}

/// The audio clip menu's Chop into ▸ for clip `i` (§1): Bar, Beat, Beat Random; Regions is off (no wave regions are
/// read).
pub fn menu_items(i: usize) -> Vec<crate::playlist::PItem> {
    use crate::playlist::{Cmd, PItem};
    vec![PItem::sub(
        "Chop into",
        vec![
            PItem::new("Bar", Cmd::Chop(i, Unit::Bar)),
            PItem::new("Beat", Cmd::Chop(i, Unit::Beat)),
            PItem::new("Beat Random", Cmd::Chop(i, Unit::BeatRandom)),
            PItem::new("Regions", Cmd::Nothing).off(),
        ],
    )]
}

/// The clips a Chop into entry on clip `i` acts on: the selection when `i` is in it, else `i` alone.
pub fn targets(s: &State, i: usize) -> Vec<usize> {
    if s.playlist.sel.contains(&i) { s.playlist.sel.clone() } else { vec![i] }
}

/// Edit ▸ Slice and insert space (Ctrl+Alt+Ins, §2): the clips crossing the time selection's start are sliced there,
/// and everything from the start on moves right by the selection's length. One undo step.
pub fn slice_and_insert_space(s: &mut State) -> (u32, u32) {
    arr_edit(s, ARRANGER_EDIT, crate::playlist::insert_space)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::playlist::arr_mut;
    use os_model::{ChannelKind, Clip, ClipSource};

    fn app() -> App {
        let mut app = App::new(None);
        app.state.session.project = crate::locked::Locked::new(crate::projects::test_project());
        app.state.session.project.get_mut().channels[0].kind = ChannelKind::AudioClip;
        let ch = app.state.session.project.channels[0].index;
        // 96 PPQ, 4/4: a clip from beat 1 of bar 1 to the middle of bar 3.
        arr_mut(&mut app.state.session).clips = vec![Clip::new(96, 384 * 2 + 96, 0, ClipSource::Channel(ch))];
        app.state.playlist.sel = vec![0];
        app
    }

    fn spans(app: &App) -> Vec<(u32, u32)> {
        let mut v: Vec<(u32, u32)> = clips(&app.state.session).iter().map(|c| (c.position, c.end())).collect();
        v.sort_unstable();
        v
    }

    /// By bar: cuts at the song's bar lines inside the clip; the pieces stay selected; one undo step.
    #[test]
    fn chop_by_bar() {
        let mut app = app();
        assert_eq!(chop(&mut app.state, &[0], Unit::Bar, 1), 2);
        assert_eq!(spans(&app), [(96, 384), (384, 768), (768, 960)]);
        assert_eq!(app.state.playlist.sel.len(), 3);
        crate::history_ui::undo(&mut app.state);
        assert_eq!(spans(&app), [(96, 960)]);
    }

    /// By beat: every beat line. Beat Random: the same cuts, then the pieces' content in another order: every
    /// window is still there once, and the same seed gives the same order.
    #[test]
    fn chop_by_beat_and_random() {
        let mut app = app();
        assert_eq!(chop(&mut app.state, &[0], Unit::Beat, 1), 8);
        assert!(spans(&app).iter().all(|(a, b)| b - a == 96));
        let in_order = windows(&app);
        let random = |seed| {
            let mut app = self::app();
            assert_eq!(chop(&mut app.state, &[0], Unit::BeatRandom, seed), 8);
            assert_eq!(spans(&app).len(), 9);
            windows(&app)
        };
        let (a, b) = (random(7), random(7));
        assert_eq!(a, b, "same seed, same order");
        assert_ne!(a, in_order, "reordered");
        let (mut x, mut y) = (a.clone(), in_order.clone());
        x.sort_by(|p, q| p.partial_cmp(q).unwrap());
        y.sort_by(|p, q| p.partial_cmp(q).unwrap());
        assert_eq!(x, y, "the same pieces");
    }

    /// Each clip's window by position.
    fn windows(app: &App) -> Vec<(f64, f64)> {
        let mut v: Vec<(u32, (f64, f64))> = clips(&app.state.session).iter().map(|c| (c.position, c.window.unwrap())).collect();
        v.sort_by_key(|p| p.0);
        v.into_iter().map(|p| p.1).collect()
    }

    /// A second of a ramp from 0 up (every place in the sample sounds different) at 48 kHz, written once per test.
    fn ramp_wav(name: &str) -> std::path::PathBuf {
        let wav = std::env::temp_dir().join(format!("os-app-{name}-{}.wav", std::process::id()));
        let ramp: Vec<f32> = (0..48000).flat_map(|k| [k as f32 / 48000.0; 2]).collect();
        os_engine::wav::write_f32_stereo(&mut std::fs::File::create(&wav).unwrap(), 48000, &ramp).unwrap();
        wav
    }

    /// `app()` with the ramp as the clip's sample, the clip reversed / pitched, and trimmed to 100..900 ms.
    fn ramp_app(wav: &std::path::Path, rev: bool, semis: i32, trimmed: bool) -> App {
        let mut app = app();
        let path = wav.to_string_lossy().into_owned();
        app.state.session.project.get_mut().channels[0].sample_path = Some(path.clone());
        app.state.session.samples.wave(&app.state.session.settings, &path);
        let c = &mut arr_mut(&mut app.state.session).clips[0];
        c.set_reversed(rev);
        c.set_pitch_semitones(semis);
        if trimmed {
            c.window = Some((100.0, 900.0));
        }
        app
    }

    /// The song as the engine plays it, interleaved stereo.
    fn render(app: &App) -> Vec<f32> {
        let mut lib = os_engine::SampleLibrary::new(None);
        os_engine::Engine::new(&app.state.session.project, &mut lib, 48000, os_engine::PlayMode::Song).render_all(480)
    }

    /// Frames (in `range`) where `a` and `b` differ.
    fn differ(a: &[f32], b: &[f32], range: std::ops::Range<usize>) -> Vec<usize> {
        range.filter(|&f| (a.get(f * 2).copied().unwrap_or(0.0) - b.get(f * 2).copied().unwrap_or(0.0)).abs() > 2e-3).collect()
    }

    const CASES: [(bool, i32, bool); 5] = [(true, 0, false), (false, 12, false), (true, 12, false), (true, -5, true), (false, 7, true)];

    /// Chopped into beats, a reversed or pitched clip's pieces play the same samples as the whole clip did (spec:
    /// ui/audio-clip-pitch-reverse.md "How it plays"): each piece takes the part of the window the whole clip played
    /// there, from the window's end for a reversed clip and at the clip's own speed for a pitched one.
    #[test]
    fn chopped_reversed_and_pitched_clips_play_as_the_whole() {
        let wav = ramp_wav("chop-rev");
        for (rev, semis, trimmed) in CASES {
            let mut app = ramp_app(&wav, rev, semis, trimmed);
            let whole = render(&app);
            assert!(whole.iter().any(|v| v.abs() > 0.1), "the clip plays");
            assert!(chop(&mut app.state, &[0], Unit::Beat, 1) > 1);
            let pieces = render(&app);
            // Pieces start at places between samples where the whole clip did not, so the frame where a piece
            // runs off the sample's end may differ: allow one frame per piece, not a wrong part of the sample.
            let off = differ(&whole, &pieces, 0..whole.len().max(pieces.len()) / 2);
            let n = clips(&app.state.session).len();
            assert!(off.len() <= 2 * n, "reversed {rev}, {semis:+} semitones, trimmed {trimmed}: {} frames differ, from {:?}", off.len(), off.first());
        }
        let _ = std::fs::remove_file(&wav);
    }

    /// Trimmed at either edge, a reversed or pitched clip plays what the whole clip played inside what is left; slipped
    /// right, it plays the same later.
    #[test]
    fn trims_and_slip_keep_reversed_and_pitched_content_in_place() {
        let wav = ramp_wav("trim-rev");
        // 130 BPM at 96 PPQ: 13 ticks are 3000 frames.
        let frames = |ticks: u32| (f64::from(ticks) * 48000.0 * 60.0 / 130.0 / 96.0).round() as usize;
        for (rev, semis, trimmed) in CASES {
            let what = format!("reversed {rev}, {semis:+} semitones, trimmed {trimmed}");
            let mut app = ramp_app(&wav, rev, semis, trimmed);
            assert_eq!(app.state.session.tempo(), 130.0);
            let orig = clips(&app.state.session)[0];
            let whole = render(&app);
            // Left edge in by 13 ticks, right edge in by 26.
            crate::playlist::resize_left(&mut app.state.session, 0, &orig, 13);
            let left = clips(&app.state.session)[0];
            crate::playlist::resize_right(&mut app.state.session, 0, &left, left.length - 26);
            let cut = render(&app);
            let (a, b) = (frames(orig.position + 13), frames(orig.end() - 26));
            let off = differ(&whole, &cut, a + 2..b - 2);
            assert!(off.len() <= 2, "{what}, trimmed: {} frames differ, from {:?}", off.len(), off.first());
            // Slipped 13 ticks right. Untrimmed, the content cannot go further: the clip starts at the sample's
            // start, or a reversed one at its end.
            let mut app = ramp_app(&wav, rev, semis, trimmed);
            crate::playlist::slip(&mut app.state.session, 0, &orig, 13);
            if !trimmed {
                let w = clips(&app.state.session)[0].window.unwrap();
                assert!(if rev { (w.1 - 1000.0).abs() < 1e-6 } else { w.0 == 0.0 }, "{what}: {w:?}");
                continue;
            }
            let slipped = render(&app);
            let d = frames(13);
            let off = differ(&whole[..whole.len() - 2 * d], &slipped[2 * d..], frames(orig.position) + 2..frames(orig.end()) - d - 2);
            assert!(off.len() <= 2, "{what}, slipped: {} frames differ, from {:?}", off.len(), off.first());
        }
        let _ = std::fs::remove_file(&wav);
    }

    /// The audio clip menu has Chop into ▸ with Bar, Beat, Beat Random and (off) Regions; Bar chops the clip.
    #[test]
    fn chop_into_menu() {
        let mut app = app();
        let m = crate::playlist::clip_menu(&app.state.session, 0, 0.0, 0.0);
        let sub = &m.items.iter().find(|i| i.label == "Chop into").expect("Chop into").children;
        assert_eq!(sub.iter().map(|i| i.label.as_ref()).collect::<Vec<_>>(), ["Bar", "Beat", "Beat Random", "Regions"]);
        assert!(!sub[3].enabled && sub[0].enabled);
        crate::playlist::run(&mut app, sub[0].cmd.clone());
        assert_eq!(spans(&app).len(), 3);
    }

    /// Ctrl+Alt+Ins: the clip crossing the selection's start is sliced there and its right part moves right by the
    /// selection's length; the Edit menu lists it.
    #[test]
    fn slice_and_insert_space_key_and_menu() {
        let mut app = app();
        app.state.playlist.range = Some((384, 576));
        let key = crate::keys::Key::Named(crate::keys::NamedKey::Insert);
        assert!(crate::playlist::key(&mut app, &key, true, true));
        assert_eq!(spans(&app), [(96, 384), (576, 1152)]);
        crate::history_ui::undo(&mut app.state);
        assert_eq!(spans(&app), [(96, 960)]);
        let pop = crate::playlist::options_menu(&app.state, 0.0, 0.0);
        let edit = &pop.items.iter().find(|i| i.label == "Edit").unwrap().children;
        assert!(edit.iter().any(|i| i.label == "Slice and insert space" && i.key == "Ctrl+Alt+Ins"));
    }
}
