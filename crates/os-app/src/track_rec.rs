// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Disk recording into mixer tracks: while Record is armed with Audio in its filter and the transport plays, every armed
//! mixer track records into a WAV file of its own, taken where its record source says; when playback stops or pauses
//! (or Record is disarmed) each file becomes an audio clip in the playlist where the take started, its channel on the
//! track it was recorded on.
//!
//! The playback's engine pushes each armed track's audio into a ring of its own (`Takes`, the engine's `RecordTap`), and
//! one writer thread per track (`capture::Recorder`) drains it to disk.
// spec: audio/track-io.md §2.5

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use crate::session::{Session, View};
use crate::capture::{Recorder, Ring};
use crate::record::Opt;

/// About 20 s of stereo at 48 kHz per track: its writer drains it every few milliseconds.
const RING: usize = 1 << 21;

/// The rings the playback's armed tracks push into: one per track armed when the take started.
pub struct Takes {
    on: AtomicBool,
    tracks: Vec<TrackRing>,
}

struct TrackRing {
    track: usize,
    ring: Ring,
    /// The song tick its first frames were mixed on (`u64::MAX`: none yet).
    first: AtomicU64,
    /// Frames the ring had no room for.
    dropped: AtomicU64,
}

impl os_engine::RecordTap for Takes {
    fn push(&self, track: usize, tick: u64, frames: &[f32]) {
        if !self.on.load(Ordering::Acquire) {
            return;
        }
        let Some(r) = self.tracks.iter().find(|r| r.track == track) else { return };
        let _ = r.first.compare_exchange(u64::MAX, tick, Ordering::AcqRel, Ordering::Relaxed);
        let went = r.ring.push_with(frames.len(), |k| frames[k]);
        r.dropped.fetch_add(((frames.len() - went) / 2) as u64, Ordering::Relaxed);
    }
}

/// A take under way, or its files being made.
pub struct TrackRec {
    pub(crate) takes: Arc<Takes>,
    /// Per ring of `takes`: the mixer track and its writer.
    recs: Vec<(u16, Option<Recorder>)>,
    rate: u32,
    /// PAT mode: the clips go to the song position the take started at, not to the pattern's ticks.
    song_pos: Option<u32>,
    ending: bool,
}

/// The take's file for a track: "<project>_<YYYY-MM-DD HH-MM-SS>_<track name>.wav" (spec: audio/track-io.md §1.5), the
/// name with the characters a file name can't hold replaced.
pub fn file_name(project: &str, now: chrono::NaiveDateTime, track: &str) -> String {
    let track: String = track.chars().map(|c| if matches!(c, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control() { '_' } else { c }).collect();
    format!("{project}_{}_{track}.wav", now.format("%Y-%m-%d %H-%M-%S"))
}

pub(crate) fn folder(s: &Session) -> std::path::PathBuf {
    crate::data_folder::resolve(&s.project, s.path.as_deref()).or_else(crate::user_dir::theme_dir).unwrap_or_else(std::env::temp_dir).join("Recorded")
}

/// Whether the armed tracks record now: Record armed with Audio in the filter, playing, not paused or counting in.
fn live(s: &Session) -> bool {
    let r = &s.rec;
    r.armed && r.count.is_none() && Opt::Audio.on() && s.transport.playing() && !s.transport.paused()
}

/// Every tick: starts a take when the armed tracks start recording, ends it when they stop, and places its files
/// once they are made.
/// `pos` is the song position (a take in PAT mode goes there), `lock_len` the playlist's last clip length
/// (`playlist::place_in`).
pub(crate) fn tick_in(s: &mut Session, pos: u32, lock_len: Option<(os_model::ClipSource, u32)>) {
    let live = live(s);
    match &s.track_rec {
        None if live => begin(s, pos),
        Some(r) if !r.ending && !live => end(s),
        Some(r) if r.ending && r.recs.iter().all(|(_, w)| w.as_ref().is_none_or(Recorder::done)) => place(s, lock_len),
        _ => {}
    }
}

/// A take of the tracks armed now (none armed: nothing starts, and a track armed later starts one).
fn begin(s: &mut Session, pos: u32) {
    let armed: Vec<u16> = s.project.mixer.iter().filter(|m| m.armed).map(|m| m.index).collect();
    if armed.is_empty() {
        return;
    }
    let rate = s.transport.opened().map_or(48000, |d| d.rate);
    let dir = folder(s);
    let now = chrono::Local::now().naive_local();
    let project = crate::backup::project_name(s.path.as_deref());
    let tracks = armed.iter().map(|&t| TrackRing { track: usize::from(t), ring: Ring::new(RING), first: AtomicU64::new(u64::MAX), dropped: AtomicU64::new(0) }).collect();
    let takes = Arc::new(Takes { on: AtomicBool::new(true), tracks });
    let mut names: Vec<String> = Vec::new();
    for &t in &armed {
        // Two tracks of one name get files of their own.
        let name = crate::edit::mixer::name(s, t);
        names.push(if names.contains(&name) { format!("{name} ({t})") } else { name });
    }
    let mut recs = Vec::new();
    for (i, (&t, name)) in armed.iter().zip(&names).enumerate() {
        let takes = takes.clone();
        match Recorder::start_from(dir.join(file_name(&project, now, name)), rate, move |out| takes.tracks[i].ring.pop_into(out)) {
            Ok(w) => recs.push((t, Some(w))),
            Err(e) => {
                s.touched.views.push(View::Message(format!("Disk recording: {e}")));
                recs.push((t, None));
            }
        }
    }
    let song_pos = (!s.song_mode()).then_some(pos);
    s.transport.record_tracks(Some(takes.clone()));
    s.track_rec = Some(TrackRec { takes, recs, rate, song_pos, ending: false });
    s.touched.repaint = true;
}

/// The take ends: the engine stops handing over, and each writer makes its file, the input's delay left out of a
/// track that records an input (spec: audio/track-io.md §2.5 "Where the take goes").
fn end(s: &mut Session) {
    s.transport.record_tracks(None);
    let p = &s.project;
    let tempo = p.tick_tempo(s.tempo().max(1.0));
    let Some(r) = s.track_rec.as_mut() else { return };
    r.takes.on.store(false, Ordering::Release);
    let tick = os_engine::laws::tick_to_frame(1.0, p.ppq.max(1), tempo, r.rate).max(1);
    let delay = s.transport.input_delay(tick) as i64;
    for (t, w) in &r.recs {
        let Some(w) = w else { continue };
        let m = p.mixer.iter().find(|m| m.index == *t);
        // The track's input latency setting moves the take on top of that ("Adjust position of recorded audio").
        let head = m.filter(|m| m.audio_in >= 0).map_or(0, |m| delay + (f64::from(m.latency_in) * f64::from(r.rate) / 1000.0).round() as i64);
        w.set_head(head);
        w.finish();
    }
    r.ending = true;
    s.touched.repaint = true;
}

/// The take's files as audio clips where it started, each on the first playlist track free there, its channel on
/// the mixer track it was recorded on: one undo step (or, with Auto-create audio clips off, only the files).
fn place(s: &mut Session, lock_len: Option<(os_model::ClipSource, u32)>) {
    let Some(r) = s.track_rec.take() else { return };
    let auto = crate::prefs::check(&crate::prefs::AUTO_AUDIO_CLIPS);
    let mut made: Vec<(u16, std::path::PathBuf, u64, u32)> = Vec::new();
    let mut errors = Vec::new();
    let mut lost = 0;
    for (i, (t, w)) in r.recs.into_iter().enumerate() {
        let Some(w) = w else { continue };
        let ring = &r.takes.tracks[i];
        lost += ring.dropped.load(Ordering::Relaxed);
        match w.join() {
            // Under a millisecond is nothing.
            Ok((path, frames)) if frames * 1000 < u64::from(r.rate) => {
                let _ = std::fs::remove_file(path);
            }
            Ok((path, frames)) => {
                let first = ring.first.load(Ordering::Acquire);
                let at = r.song_pos.unwrap_or(u32::try_from(first).unwrap_or(0));
                made.push((t, path, frames, at));
            }
            Err(e) => errors.push(e),
        }
    }
    let mut msg = match (made.len(), auto) {
        (0, _) => errors.first().map_or_else(|| "Nothing was recorded".to_string(), |e| format!("Disk recording: {e}")),
        (_, false) => format!("Recorded {} file(s) into the Recorded folder", made.len()),
        (_, true) => {
            crate::edit::playlist::snapshot_as(s, crate::one_click::RECORDED);
            let p = &s.project;
            let tempo = p.tick_tempo(s.tempo().max(1.0));
            let ppq = f64::from(p.ppq.max(1));
            let mut placed = 0;
            for (t, path, frames, at) in &made {
                let len = (*frames as f64 / f64::from(r.rate) * tempo / 60.0 * ppq).ceil() as u32;
                // Into the mixer track's audio track when it has one free over the take (spec: ui/playlist.md §6c).
                let track = crate::track_links::record_track(s, *t, *at, at + len.max(1));
                if crate::playlist::add_audio_clip_on(s, &path.to_string_lossy(), track, *at, Some(*t), lock_len).is_some() {
                    placed += 1;
                }
            }
            s.dirty = true;
            match (placed, made.first()) {
                (1, Some((_, path, ..))) => format!("Recorded {}", path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default()),
                (n, _) => format!("Recorded {n} mixer tracks"),
            }
        }
    };
    if lost > 0 {
        msg = format!("{msg} ({lost} frames were lost: the disk was too slow)");
    }
    s.touched.views.push(View::Message(msg));
    s.touched.repaint = true;
}

#[cfg(test)]
mod tests {
    use super::*;
    use os_engine::RecordTap;

    #[test]
    fn file_names_carry_the_track() {
        let at = chrono::NaiveDate::from_ymd_opt(2026, 10, 29).unwrap().and_hms_opt(14, 5, 9).unwrap();
        assert_eq!(file_name("Song", at, "Vocals"), "Song_2026-10-29 14-05-09_Vocals.wav");
        assert_eq!(file_name("Song", at, "A/B: mic?"), "Song_2026-10-29 14-05-09_A_B_ mic_.wav");
    }

    /// The tap: only its tracks, only while on, the first tick kept, what has no room counted.
    #[test]
    fn the_tap_fills_each_tracks_ring() {
        let tr = |t| TrackRing { track: t, ring: Ring::new(8), first: AtomicU64::new(u64::MAX), dropped: AtomicU64::new(0) };
        let takes = Takes { on: AtomicBool::new(true), tracks: vec![tr(1), tr(3)] };
        takes.push(3, 40, &[0.5, 0.25]);
        takes.push(1, 41, &[0.1, 0.2, 0.3, 0.4]);
        takes.push(2, 41, &[1.0, 1.0]);
        takes.push(3, 42, &[0.5; 8]);
        let mut out = Vec::new();
        takes.tracks[0].ring.pop_into(&mut out);
        assert_eq!(out, [0.1, 0.2, 0.3, 0.4]);
        out.clear();
        takes.tracks[1].ring.pop_into(&mut out);
        assert_eq!(out.len(), 8, "the ring holds 8");
        assert_eq!((takes.tracks[1].first.load(Ordering::Relaxed), takes.tracks[1].dropped.load(Ordering::Relaxed)), (40, 1));
        takes.on.store(false, Ordering::Release);
        takes.push(1, 50, &[0.5, 0.5]);
        out.clear();
        takes.tracks[0].ring.pop_into(&mut out);
        assert!(out.is_empty(), "off: nothing taken");
    }
}
