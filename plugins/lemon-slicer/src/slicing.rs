// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Cutting a loop into slices: the WAV file's own markers (`cue ` points named from `LIST adtl labl`) and tempo
//! (`acid`), even slices of the loop's beats, Lemon's transient detector, the zero-cross check, and the loop's tempo
//! when the file gives none.
//!
// spec: plugins/lemon-slicer.md §3, plugins/fruity-slicer.md §3

use crate::state::Slice;
use os_dsp::transients;

/// What a WAV file says about itself beyond its sound.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WavInfo {
    /// `acid`: beats and tempo.
    pub beats: Option<f32>,
    pub bpm: Option<f32>,
    /// `cue ` points (frame, name), in file order.
    pub cues: Vec<(u32, String)>,
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

/// Reads the chunks of a RIFF WAVE file (nothing for other files).
pub fn wav_info(b: &[u8]) -> WavInfo {
    let mut info = WavInfo::default();
    if b.len() < 12 || &b[0..4] != b"RIFF" || &b[8..12] != b"WAVE" {
        return info;
    }
    let mut cues: Vec<(u32, u32)> = Vec::new();
    let mut labels: Vec<(u32, String)> = Vec::new();
    let mut at = 12;
    while at + 8 <= b.len() {
        let id = &b[at..at + 4];
        let size = u32_at(b, at + 4).unwrap_or(0) as usize;
        let body = &b[at + 8..(at + 8 + size).min(b.len())];
        match id {
            // Flags (bit 0: one-shot), root key, two fields, beats, meter, tempo.
            b"acid" if body.len() >= 24 => {
                let beats = u32_at(body, 12).unwrap_or(0);
                let bpm = f32::from_le_bytes(body[20..24].try_into().unwrap());
                if beats > 0 && body[0] & 1 == 0 {
                    info.beats = Some(beats as f32);
                }
                if bpm.is_finite() && bpm > 0.0 {
                    info.bpm = Some(bpm);
                }
            }
            b"cue " => {
                let n = u32_at(body, 0).unwrap_or(0) as usize;
                for i in 0..n {
                    let r = 4 + 24 * i;
                    let (Some(id), Some(pos), Some(offset)) = (u32_at(body, r), u32_at(body, r + 4), u32_at(body, r + 20)) else { break };
                    cues.push((id, if offset != 0 { offset } else { pos }));
                }
            }
            b"LIST" if body.get(0..4) == Some(b"adtl") => {
                let mut s = 4;
                while s + 8 <= body.len() {
                    let sid = &body[s..s + 4];
                    let ssize = u32_at(body, s + 4).unwrap_or(0) as usize;
                    let sb = &body[s + 8..(s + 8 + ssize).min(body.len())];
                    if sid == b"labl" && sb.len() >= 4 {
                        let text = &sb[4..];
                        let end = text.iter().position(|&c| c == 0).unwrap_or(text.len());
                        labels.push((u32_at(sb, 0).unwrap_or(0), String::from_utf8_lossy(&text[..end]).into_owned()));
                    }
                    s += 8 + ssize + (ssize & 1);
                }
            }
            _ => {}
        }
        at += 8 + size + (size & 1);
    }
    info.cues = cues.iter().map(|&(id, frame)| (frame, labels.iter().find(|(l, _)| *l == id).map(|(_, s)| s.clone()).unwrap_or_default())).collect();
    info
}

/// The file's own slices: its cue points in order, named from their labels; the first starts at 0. Empty when the
/// file has none.
pub fn builtin(info: &WavInfo, frames: usize) -> Vec<Slice> {
    let mut cues: Vec<(u32, String)> = info.cues.iter().filter(|(f, _)| (*f as usize) < frames.max(1)).cloned().collect();
    cues.sort_by_key(|(f, _)| *f);
    cues.dedup_by_key(|(f, _)| *f);
    let mut out: Vec<Slice> = cues.into_iter().map(|(f, name)| Slice { name: name.into_bytes(), ..Slice::at(f) }).collect();
    if let Some(first) = out.first_mut() {
        first.start = 0;
    }
    out
}

/// Even slices of the loop's beats, `per_beat` to a beat (6, 4, 3, 2 or 1): named `n` (from 1), and `n - Beat b` on
/// the slices that start a beat.
pub fn by_beats(frames: usize, beats: f64, per_beat: u32) -> Vec<Slice> {
    let per_beat = per_beat.max(1);
    let count = ((beats * f64::from(per_beat)).round() as usize).max(1);
    (0..count)
        .map(|i| {
            let start = (i as f64 * frames as f64 / count as f64).round() as u32;
            let name = if i as u32 % per_beat == 0 { format!("{} - Beat {}", i + 1, i as u32 / per_beat + 1) } else { (i + 1).to_string() };
            Slice { name: name.into_bytes(), ..Slice::at(start) }
        })
        .collect()
}

/// One slice at 0 named `name` (No slicing).
pub fn whole(name: &str) -> Vec<Slice> {
    vec![Slice { name: name.as_bytes().to_vec(), ..Slice::at(0) }]
}

/// The auto slicing's sensitivities: Dull, Medium, Sharp (the rise in dB a slice needs).
pub use os_dsp::transients::AUTO_DB;

/// Lemon's transient detector (`os_dsp::transients::detect`, shared with Tesla's auto-slicing): the level of both
/// channels in 5 ms frames; a slice starts at a frame whose level is `rise_db` above the previous frame's and above
/// −40 dBFS, at least 50 ms after the previous slice, at the frame's first sample that reaches a quarter of the frame's
/// peak. The first slice starts at 0; auto slices have no names.
pub fn auto(left: &[f32], right: &[f32], rate: u32, rise_db: f32) -> Vec<Slice> {
    transients::detect(left, right, rate, rise_db).into_iter().map(|f| Slice::at(f as u32)).collect()
}

/// The zero-cross check: every slice's start moves to the nearest sign change of the summed channels within 2 ms (the
/// sample where the new sign starts); a start with none nearby stays.
pub fn zero_cross(slices: &mut [Slice], left: &[f32], right: &[f32], rate: u32) {
    let mut starts: Vec<usize> = slices.iter().map(|s| s.start as usize).collect();
    transients::zero_cross(&mut starts, left, right, rate);
    for (s, f) in slices.iter_mut().zip(starts) {
        s.start = f as u32;
    }
}

/// The loop's beats and tempo when its file gives none: a whole number of 4/4 bars at 70..180 BPM, the most beats
/// that keep the tempo at most 180; a loop too short for a bar is taken at 120 BPM in whole beats.
pub fn guess_tempo(frames: usize, rate: u32) -> (f32, f32) {
    transients::guess_tempo(frames, rate)
}

#[cfg(test)]
pub(crate) mod tests_support {
    use super::*;

    /// A WAV of `frames` silent 16-bit frames with `extra` chunks after the data.
    pub(crate) fn wav(frames: usize, extra: &[Vec<u8>]) -> Vec<u8> {
        let chunk = |id: &[u8], body: &[u8]| {
            let mut c = id.to_vec();
            c.extend((body.len() as u32).to_le_bytes());
            c.extend(body);
            if body.len() % 2 == 1 {
                c.push(0);
            }
            c
        };
        let mut fmt = Vec::new();
        for v in [1u16, 1] {
            fmt.extend(v.to_le_bytes());
        }
        fmt.extend(44100u32.to_le_bytes());
        fmt.extend(88200u32.to_le_bytes());
        fmt.extend(2u16.to_le_bytes());
        fmt.extend(16u16.to_le_bytes());
        let mut body = b"WAVE".to_vec();
        body.extend(chunk(b"fmt ", &fmt));
        body.extend(chunk(b"data", &vec![0u8; frames * 2]));
        for e in extra {
            body.extend(e);
        }
        let mut b = b"RIFF".to_vec();
        b.extend((body.len() as u32).to_le_bytes());
        b.extend(body);
        b
    }

    pub(crate) fn cue_chunks(cues: &[(u32, &str)]) -> Vec<Vec<u8>> {
        let mut cue = (cues.len() as u32).to_le_bytes().to_vec();
        for (i, (f, _)) in cues.iter().enumerate() {
            for v in [i as u32 + 1, *f] {
                cue.extend(v.to_le_bytes());
            }
            cue.extend(b"data");
            for v in [0u32, 0, *f] {
                cue.extend(v.to_le_bytes());
            }
        }
        let mut list = b"adtl".to_vec();
        for (i, (_, name)) in cues.iter().enumerate() {
            let mut l = (i as u32 + 1).to_le_bytes().to_vec();
            l.extend(name.as_bytes());
            l.push(0);
            list.extend(b"labl");
            list.extend((l.len() as u32).to_le_bytes());
            if l.len() % 2 == 1 {
                l.push(0);
            }
            list.extend(l);
        }
        let head = |id: &[u8], b: &[u8]| {
            let mut c = id.to_vec();
            c.extend((b.len() as u32).to_le_bytes());
            c.extend(b);
            c
        };
        vec![head(b"cue ", &cue), head(b"LIST", &list)]
    }

    pub(crate) fn acid(beats: u32, bpm: f32) -> Vec<u8> {
        let mut b = b"acid".to_vec();
        b.extend(24u32.to_le_bytes());
        b.extend(0u32.to_le_bytes());
        b.extend(60u16.to_le_bytes());
        b.extend([0u8; 6]);
        b.extend(beats.to_le_bytes());
        b.extend(4u16.to_le_bytes());
        b.extend(4u16.to_le_bytes());
        b.extend(bpm.to_le_bytes());
        b
    }

    #[test]
    fn markers_and_tempo_are_read_from_the_file() {
        let mut extra = cue_chunks(&[(22050, "Snare"), (5, "Kick"), (44100, "HiHat")]);
        extra.push(acid(8, 60.0));
        let info = wav_info(&wav(352_800, &extra));
        assert_eq!((info.beats, info.bpm), (Some(8.0), Some(60.0)));
        assert_eq!(info.cues.len(), 3);
        let s = builtin(&info, 352_800);
        let got: Vec<(u32, String)> = s.iter().map(|s| (s.start, s.name())).collect();
        assert_eq!(got, vec![(0, "Kick".into()), (22050, "Snare".into()), (44100, "HiHat".into())]);
        assert!(builtin(&wav_info(&wav(10, &[])), 10).is_empty());
        assert_eq!(wav_info(b"not a wav"), WavInfo::default());
    }

    /// The measured counts and names (fruity-slicer.md §3: 8 beats of 352 800 frames).
    #[test]
    fn beat_fractions_make_the_originals_slices() {
        for (per, n) in [(6, 48), (4, 32), (3, 24), (2, 16), (1, 8)] {
            assert_eq!(by_beats(352_800, 8.0, per).len(), n);
        }
        let s = by_beats(352_800, 8.0, 3);
        let names: Vec<String> = s.iter().take(4).map(|s| s.name()).collect();
        assert_eq!(names, ["1 - Beat 1", "2", "3", "4 - Beat 2"]);
        assert_eq!((s[1].start, s[3].start), (14700, 44100));
        assert!(s.iter().all(|s| s.root == -1 && s.f == -1.0 && s.flags == 0));
        assert_eq!(whole("DL Breaker")[0].name(), "DL Breaker");
    }

    #[test]
    fn hits_become_slices() {
        let rate = 44100;
        let mut l = vec![0.0f32; rate as usize];
        // Quiet noise, then hits at 0.25 s and 0.5 s (decaying), and one 20 ms after the second (too close).
        for (i, v) in l.iter_mut().enumerate() {
            *v = 0.0005 * ((i * 7919 % 13) as f32 / 13.0 - 0.5);
        }
        for at in [11025usize, 22050, 22050 + 882] {
            for k in 0..2000 {
                l[at + k] += 0.8 * (-(k as f32) / 400.0).exp() * if k % 2 == 0 { 1.0 } else { -1.0 };
            }
        }
        let r = l.clone();
        let s = auto(&l, &r, rate, AUTO_DB[1]);
        let starts: Vec<u32> = s.iter().map(|s| s.start).collect();
        assert_eq!(starts.len(), 3, "{starts:?}");
        assert_eq!(starts[0], 0);
        assert!((10800..=11025).contains(&starts[1]) && (21800..=22050).contains(&starts[2]), "{starts:?}");
    }

    #[test]
    fn starts_move_to_the_nearest_zero_crossing() {
        let rate = 44100;
        let l: Vec<f32> = (0..1000).map(|i| ((i as f32 - 2.5) * 0.01).sin()).collect();
        let mut s = vec![Slice::at(0), Slice::at(500)];
        zero_cross(&mut s, &l, &l, rate);
        // The sine crosses between 2 and 3: the new sign starts at 3.
        assert_eq!(s[0].start, 3);
        // 500 is far from any crossing (the next is ~316 samples away): it stays.
        assert_eq!(s[1].start, 500);
    }

    #[test]
    fn tempo_is_guessed_in_whole_bars() {
        // 8 s: 16 beats would be 120 BPM, 20 beats 150, 24 beats 180.
        assert_eq!(guess_tempo(8 * 44100, 44100), (24.0, 180.0));
        // 2 s: 4 beats at 120.
        assert_eq!(guess_tempo(2 * 44100, 44100), (4.0, 120.0));
        // 0.5 s: no bar fits, one beat at 120.
        assert_eq!(guess_tempo(22050, 44100), (1.0, 120.0));
    }
}
