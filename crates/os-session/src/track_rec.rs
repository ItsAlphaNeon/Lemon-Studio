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
//!
//! Loop record: each time playback goes back (the loop's end, or a jump back), the tap notes where in the file the new
//! pass starts. A pass that has ended becomes a file and a clip of its own while the next one records, on a track
//! grouped under the first pass's (Group loop recorded takes), the earlier passes muted unless Blend is on.
// spec: audio/track-io.md §2.5, audio/disk-recording.md §1

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

use crate::session::{Session, View};
use crate::capture::{Recorder, Ring};
use crate::record::Opt;

/// About 20 s of stereo at 48 kHz per track: its writer drains it every few milliseconds.
const RING: usize = 1 << 21;
/// The most passes a take keeps apart; loops after that join the last one.
const PASSES: usize = 1024;

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
    /// Frames the ring took: where the next frame lands in the file.
    taken: AtomicU64,
    /// The tick of the last push (`u64::MAX`: none yet).
    last: AtomicU64,
    /// Each pass after the first (`passes` of them): its first frame in the file and its first tick.
    cuts: Box<[(AtomicU64, AtomicU64)]>,
    passes: AtomicUsize,
}

impl TrackRing {
    fn new(track: usize, len: usize) -> TrackRing {
        TrackRing {
            track,
            ring: Ring::new(len),
            first: AtomicU64::new(u64::MAX),
            dropped: AtomicU64::new(0),
            taken: AtomicU64::new(0),
            last: AtomicU64::new(u64::MAX),
            cuts: (0..PASSES).map(|_| (AtomicU64::new(0), AtomicU64::new(0))).collect(),
            passes: AtomicUsize::new(0),
        }
    }

    /// Passes begun so far.
    fn begun(&self) -> usize {
        if self.first.load(Ordering::Acquire) == u64::MAX {
            0
        } else {
            1 + self.passes.load(Ordering::Acquire)
        }
    }

    /// Pass `k`'s first frame in the file and its first tick, once it has begun.
    fn pass(&self, k: usize) -> Option<(u64, u64)> {
        if k >= self.begun() {
            return None;
        }
        Some(match k {
            0 => (0, self.first.load(Ordering::Acquire)),
            k => (self.cuts[k - 1].0.load(Ordering::Acquire), self.cuts[k - 1].1.load(Ordering::Acquire)),
        })
    }
}

impl os_engine::RecordTap for Takes {
    fn push(&self, track: usize, tick: u64, frames: &[f32]) {
        if !self.on.load(Ordering::Acquire) {
            return;
        }
        let Some(r) = self.tracks.iter().find(|r| r.track == track) else { return };
        // Playback went back: a new pass starts here (one producer per track, so plain loads and stores do).
        let last = r.last.swap(tick, Ordering::AcqRel);
        if last != u64::MAX && tick < last {
            let n = r.passes.load(Ordering::Relaxed);
            if n < PASSES {
                r.cuts[n].0.store(r.taken.load(Ordering::Relaxed), Ordering::Relaxed);
                r.cuts[n].1.store(tick, Ordering::Relaxed);
                r.passes.store(n + 1, Ordering::Release);
            }
        }
        let _ = r.first.compare_exchange(u64::MAX, tick, Ordering::AcqRel, Ordering::Relaxed);
        let went = r.ring.push_with(frames.len(), |k| frames[k]);
        r.taken.fetch_add((went / 2) as u64, Ordering::Release);
        r.dropped.fetch_add(((frames.len() - went) / 2) as u64, Ordering::Relaxed);
    }
}

/// A take under way, or its files being made.
pub struct TrackRec {
    pub takes: Arc<Takes>,
    /// Per ring of `takes`: the mixer track and its writer.
    recs: Vec<(u16, Option<Recorder>)>,
    /// Per ring of `takes`: its passes made so far.
    lanes: Vec<Lane>,
    rate: u32,
    /// PAT mode: the clips go to the song position the take started at, not to the pattern's ticks.
    song_pos: Option<u32>,
    ending: bool,
    project: String,
    dir: PathBuf,
}

/// A mixer track's passes in one take.
#[derive(Default)]
struct Lane {
    /// The mixer track.
    mixer: u16,
    /// The track's name in its files.
    name: String,
    /// The writer's own file: the take's last pass.
    file: PathBuf,
    /// Passes made into files.
    made: usize,
    /// The audio clip channels of its placed passes, oldest first, and the playlist tracks they went on.
    channels: Vec<u16>,
    tracks: Vec<u16>,
}

/// The take's file for a track: "<project>_<YYYY-MM-DD HH-MM-SS>_<track name>.wav" (spec: audio/track-io.md §1.5), the
/// name with the characters a file name can't hold replaced.
pub fn file_name(project: &str, now: chrono::NaiveDateTime, track: &str) -> String {
    let track: String =
        track
            .chars()
            .map(|c| {
                if matches!(c, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control() {
                    '_'
                } else {
                    c
                }
            })
            .collect();
    format!("{project}_{}_{track}.wav", now.format("%Y-%m-%d %H-%M-%S"))
}

pub fn folder(s: &Session) -> std::path::PathBuf {
    crate::data_folder::resolve(&s.project, s.path.as_deref())
        .or_else(crate::user_dir::theme_dir)
        .unwrap_or_else(std::env::temp_dir)
        .join("Recorded")
}

/// `name` in `dir`, or "<stem> (2).wav", "<stem> (3).wav"... when that is taken on disk or by `not`.
fn free_file(dir: &std::path::Path, name: &str, not: &[&std::path::Path]) -> PathBuf {
    let stem = name.strip_suffix(".wav").unwrap_or(name);
    std::iter::once(dir.join(name))
        .chain((2..).map(|k| dir.join(format!("{stem} ({k}).wav"))))
        .find(|p| !p.exists() && !not.contains(&p.as_path()))
        .unwrap_or_else(|| dir.join(name))
}

/// Loop record pass `k`'s file of a take written to `file`: the file itself for the first, then "<stem>_2.wav",
/// "<stem>_3.wav"... as FL names them (spec: audio/disk-recording.md §1).
fn pass_file(file: &std::path::Path, k: usize) -> PathBuf {
    match (k, file.file_stem()) {
        (0, _) | (_, None) => file.to_path_buf(),
        (k, Some(stem)) => file.with_file_name(format!("{}_{}.wav", stem.to_string_lossy(), k + 1)),
    }
}

/// The recording format's bits (Disk recording ▸ 16Bit int / 24Bit int / 32Bit float; spec: audio/disk-recording.md §2).
fn rec_bits(s: &Session) -> u32 {
    match s.settings.index(&crate::prefs::REC_DEPTH) {
        0 => 16,
        1 => 24,
        _ => 32,
    }
}

/// Whether the armed tracks record now: Record armed with Audio in the filter, playing, not paused or counting in.
fn live(s: &Session) -> bool {
    let r = &s.rec;
    r.armed && r.count.is_none() && Opt::Audio.on(&s.settings) && s.transport.playing() && !s.transport.paused()
}

/// Every tick: starts a take when the armed tracks start recording, makes the passes that have ended into clips, ends
/// the take when they stop, and places its last files once they are made.
/// `pos` is the song position (a take in PAT mode goes there), `brush` the playlist's clip brush
/// (`playlist::place_in`).
pub fn tick_in(s: &mut Session, pos: u32, brush: Option<crate::edit::playlist::Brush>) {
    let live = live(s);
    match &s.track_rec {
        None if live => begin(s, pos),
        Some(r) if !r.ending && !live => end(s),
        Some(r) if !r.ending => {
            if r.takes.tracks.iter().zip(&r.lanes).any(|(t, l)| t.begun() > l.made + 1) {
                passes(s, brush);
            }
        }
        Some(r) if r.ending && r.recs.iter().all(|(_, w)| w.as_ref().is_none_or(Recorder::done)) => place(s, brush),
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
    let tracks = armed.iter().map(|&t| TrackRing::new(usize::from(t), RING)).collect();
    let takes = Arc::new(Takes { on: AtomicBool::new(true), tracks });
    let mut names: Vec<String> = Vec::new();
    for &t in &armed {
        // Two tracks of one name get files of their own.
        let name = crate::edit::mixer::name(s, t);
        names.push(if names.contains(&name) { format!("{name} ({t})") } else { name });
    }
    let mut recs = Vec::new();
    let mut lanes = Vec::new();
    for (i, (&t, name)) in armed.iter().zip(&names).enumerate() {
        let takes = takes.clone();
        let file = s.rec_files.remove(&t).unwrap_or_else(|| free_file(&dir, &file_name(&project, now, name), &[]));
        match Recorder::start_from(file.clone(), rate, move |out| takes.tracks[i].ring.pop_into(out)) {
            Ok(w) => recs.push((t, Some(w))),
            Err(e) => {
                s.touched.views.push(View::Message(format!("Disk recording: {e}")));
                recs.push((t, None));
            }
        }
        lanes.push(Lane { mixer: t, name: name.clone(), file, ..Lane::default() });
    }
    let song_pos = (!s.song_mode()).then_some(pos);
    s.transport.record_tracks(Some(takes.clone()));
    s.track_rec = Some(TrackRec { takes, recs, lanes, rate, song_pos, ending: false, project, dir });
    s.touched.repaint = true;
}

/// The frames left out of the front of mixer track `t`'s take: the input's delay for a track that records an input
/// (spec: audio/track-io.md §2.5 "Where the take goes"), with its input latency setting on top ("Adjust position of
/// recorded audio"); none with Disk recording ▸ Latency compensation off (audio/disk-recording.md §2).
fn head_of(s: &Session, rate: u32, t: u16) -> i64 {
    if !s.settings.check(&crate::prefs::REC_LATENCY_COMP) {
        return 0;
    }
    let p = &s.project;
    let tempo = p.tick_tempo(s.tempo().max(1.0));
    let tick = os_engine::laws::tick_to_frame(1.0, p.ppq.max(1), tempo, rate).max(1);
    let delay = s.transport.input_delay(tick) as i64;
    let m = p.mixer.iter().find(|m| m.index == t);
    m.filter(|m| m.audio_in >= 0)
        .map_or(0, |m| delay + (f64::from(m.latency_in) * f64::from(rate) / 1000.0).round() as i64)
}

/// A pass made into a file: its mixer track's ring, the file, its frames and the tick it goes to.
type Made = (usize, PathBuf, u64, u32);

/// Makes ring `i`'s pass `k` (it has ended) into a file of its own, cut from what its writer has on disk.
fn make_pass(s: &Session, r: &TrackRec, i: usize, k: usize, raw: &std::path::Path) -> Option<Result<Made, String>> {
    let ring = &r.takes.tracks[i];
    let (start, tick) = ring.pass(k)?;
    let (end, _) = ring.pass(k + 1)?;
    let lane = &r.lanes[i];
    let head = head_of(s, r.rate, lane.mixer);
    let file = pass_file(&lane.file, k);
    let at = r.song_pos.unwrap_or(u32::try_from(tick).unwrap_or(0));
    Some(
        crate::capture::segment_wav(
            raw,
            &file,
            r.rate,
            rec_bits(s),
            start as i64 + head,
            (end as i64 + head).max(0) as u64,
        )
        .map(|frames| (i, file, frames, at))
        .map_err(|e| e.to_string()),
    )
}

/// While the take goes on: the passes that have ended and whose frames are on disk become files and clips, one undo
/// step "Recorded items" (spec: audio/disk-recording.md §1).
fn passes(s: &mut Session, brush: Option<crate::edit::playlist::Brush>) {
    let Some(mut r) = s.track_rec.take() else { return };
    let mut made = Vec::new();
    for i in 0..r.lanes.len() {
        let Some(w) = r.recs[i].1.as_ref() else { continue };
        let t = r.recs[i].0;
        while let Some((end, _)) = r.takes.tracks[i].pass(r.lanes[i].made + 1) {
            // Not on disk yet: the next tick.
            if end as i64 + head_of(s, r.rate, t) > w.written() as i64 {
                break;
            }
            let raw = w.part().to_path_buf();
            match make_pass(s, &r, i, r.lanes[i].made, &raw) {
                Some(Ok(m)) => made.push(m),
                Some(Err(e)) => s.touched.views.push(View::Message(format!("Disk recording: {e}"))),
                None => break,
            }
            r.lanes[i].made += 1;
        }
    }
    if !made.is_empty() {
        let msg = put(s, &mut r, made, brush);
        s.touched.views.push(View::Message(msg));
    }
    s.track_rec = Some(r);
}

/// The take ends: the engine stops handing over, and each writer makes its file of the last pass, the input's delay
/// left out of a track that records an input (spec: audio/track-io.md §2.5 "Where the take goes").
fn end(s: &mut Session) {
    s.transport.record_tracks(None);
    let Some(r) = s.track_rec.take() else { return };
    r.takes.on.store(false, Ordering::Release);
    for (i, (t, w)) in r.recs.iter().enumerate() {
        let Some(w) = w else { continue };
        let ring = &r.takes.tracks[i];
        let last = ring.begun().max(1) - 1;
        // Passes not made yet (the writer was behind) are cut from the raw file once it is done.
        if last > r.lanes[i].made {
            w.keep_part();
        }
        let from = ring.pass(last).map_or(0, |(f, _)| f) as i64;
        w.set_head(from + head_of(s, r.rate, *t));
        w.set_file(pass_file(&r.lanes[i].file, last));
        w.set_bits(rec_bits(s));
        w.finish();
    }
    s.track_rec = Some(TrackRec { ending: true, ..r });
    s.touched.repaint = true;
}

/// The take's last files as audio clips where their passes started, each on the first playlist track free there (or a
/// grouped track, audio/disk-recording.md §1), its channel on the mixer track it was recorded on: one undo step (or, with Auto-create audio
/// clips off, only the files).
fn place(s: &mut Session, brush: Option<crate::edit::playlist::Brush>) {
    let Some(mut r) = s.track_rec.take() else { return };
    let mut made: Vec<Made> = Vec::new();
    let mut errors = Vec::new();
    let mut lost = 0;
    let recs = std::mem::take(&mut r.recs);
    for (i, (_, w)) in recs.into_iter().enumerate() {
        let Some(w) = w else { continue };
        let ring = &r.takes.tracks[i];
        lost += ring.dropped.load(Ordering::Relaxed);
        let last = ring.begun().max(1) - 1;
        let raw = w.part().to_path_buf();
        let joined = w.join();
        // Passes the writer had not reached while playing.
        if last > r.lanes[i].made {
            while r.lanes[i].made < last {
                match make_pass(s, &r, i, r.lanes[i].made, &raw) {
                    Some(Ok(m)) => made.push(m),
                    Some(Err(e)) => errors.push(e),
                    None => {}
                }
                r.lanes[i].made += 1;
            }
            let _ = std::fs::remove_file(&raw);
        }
        match joined {
            // Under a millisecond is nothing.
            Ok((path, frames)) if frames * 1000 < u64::from(r.rate) => {
                let _ = std::fs::remove_file(path);
            }
            Ok((path, frames)) => {
                let tick = ring.pass(last).map_or(0, |(_, t)| t);
                let at = r.song_pos.unwrap_or(u32::try_from(tick).unwrap_or(0));
                made.push((i, path, frames, at));
            }
            Err(e) => errors.push(e),
        }
        r.lanes[i].made = last + 1;
    }
    let auto = s.settings.check(&crate::prefs::AUTO_AUDIO_CLIPS);
    let mut msg = match (made.len(), auto) {
        (0, _) if r.lanes.iter().any(|l| !l.channels.is_empty()) => "Recording stopped".to_string(),
        (0, _) => errors.first().map_or_else(|| "Nothing was recorded".to_string(), |e| format!("Disk recording: {e}")),
        (_, false) => format!("Recorded {} file(s) into the Recorded folder", made.len()),
        (_, true) => put(s, &mut r, made, brush),
    };
    if lost > 0 {
        msg = format!("{msg} ({lost} frames were lost: the disk was too slow)");
    }
    // Disk recording ▸ Auto-unarm: Stop disarms the armed tracks (audio/disk-recording.md §2).
    // A direct write, as arming is no undo step (control-plane step 14 makes it a command).
    #[allow(deprecated)]
    if s.settings.check(&crate::prefs::AUTO_UNARM) && !s.transport.playing() {
        for m in s.project.get_mut().mixer.iter_mut() {
            m.armed = false;
        }
    }
    s.touched.views.push(View::Message(msg));
    s.touched.repaint = true;
}

/// Places files as audio clips, one undo step "Recorded items" (with Auto-create audio clips off, the files stay in the
/// folder): each at its tick, its channel on its mixer track; a pass after a lane's first on a track grouped under the
/// first's with Group loop recorded takes on, else on the first track free over it; the lane's earlier passes muted
/// unless Blend is on (spec: audio/disk-recording.md §1). Returns the message to show.
fn put(s: &mut Session, r: &mut TrackRec, made: Vec<Made>, brush: Option<crate::edit::playlist::Brush>) -> String {
    if !s.settings.check(&crate::prefs::AUTO_AUDIO_CLIPS) {
        return format!("Recorded {} file(s) into the Recorded folder", made.len());
    }
    crate::edit::playlist::snapshot_as(s, crate::one_click::RECORDED);
    let group = s.settings.check(&crate::prefs::GROUP_LOOP_TAKES);
    let blend = Opt::Blend.on(&s.settings);
    let rename = s.settings.check(&crate::prefs::AUTO_NAME_TRACKS);
    let tempo = s.project.tick_tempo(s.tempo().max(1.0));
    let ppq = f64::from(s.project.ppq.max(1));
    let mut placed = 0;
    let mut last_name = String::new();
    for (i, path, frames, at) in &made {
        let t = r.lanes[*i].mixer;
        let len = (*frames as f64 / f64::from(r.rate) * tempo / 60.0 * ppq).ceil() as u32;
        let lane = &mut r.lanes[*i];
        let end = at + len.max(1);
        // Into the mixer track's audio track when it has one free over the take (spec: ui/playlist.md §6c); with Group
        // loop recorded takes on, else onto a track grouped under it (or under the take's first track), after the group.
        let lead = crate::track_links::audio_tracks_of(s, t).first().copied().or(lane.tracks.first().copied());
        let track = match lead {
            Some(lead)
                if group && (lead != crate::track_links::record_track(s, t, *at, end) || !lane.tracks.is_empty()) =>
            {
                crate::loop_takes::take_track(s, crate::loop_takes::group_end(s, lead) + 1)
            }
            _ => crate::track_links::record_track(s, t, *at, end),
        };
        let Some(clip) =
            crate::edit::playlist::add_audio_clip_on(s, &path.to_string_lossy(), track, *at, Some(t), brush)
        else {
            continue;
        };
        let Some(os_model::ClipSource::Channel(ch)) = crate::edit::playlist::clips(s).get(clip).map(|c| c.source)
        else {
            continue;
        };
        lane.channels.push(ch);
        // Blend off: all of the take's passes are muted, the newest one too (FL, measured).
        if !blend {
            crate::loop_takes::mute_takes(s, &lane.channels);
        }
        let track = crate::edit::playlist::clips(s)[clip].track;
        if rename {
            crate::loop_takes::name_take_track(s, track, &lane.name);
        }
        lane.tracks.push(track);
        placed += 1;
        last_name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    }
    s.dirty = true;
    match placed {
        1 => format!("Recorded {last_name}"),
        n => format!("Recorded {n} takes"),
    }
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
        let takes = Takes { on: AtomicBool::new(true), tracks: vec![TrackRing::new(1, 8), TrackRing::new(3, 8)] };
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
        assert_eq!(
            (takes.tracks[1].first.load(Ordering::Relaxed), takes.tracks[1].dropped.load(Ordering::Relaxed)),
            (40, 1)
        );
        takes.on.store(false, Ordering::Release);
        takes.push(1, 50, &[0.5, 0.5]);
        out.clear();
        takes.tracks[0].ring.pop_into(&mut out);
        assert!(out.is_empty(), "off: nothing taken");
    }

    /// A loop (the tick going back) starts a pass where the file is then; the same tick twice does not.
    #[test]
    fn going_back_starts_a_pass() {
        let takes = Takes { on: AtomicBool::new(true), tracks: vec![TrackRing::new(1, 1 << 10)] };
        let r = &takes.tracks[0];
        assert_eq!((r.begun(), r.pass(0)), (0, None));
        takes.push(1, 96, &[0.0; 20]);
        takes.push(1, 97, &[0.0; 20]);
        takes.push(1, 97, &[0.0; 4]);
        takes.push(1, 96, &[0.0; 6]);
        takes.push(1, 97, &[0.0; 20]);
        takes.push(1, 96, &[0.0; 2]);
        assert_eq!(r.begun(), 3);
        assert_eq!(r.pass(0), Some((0, 96)));
        assert_eq!(r.pass(1), Some((22, 96)));
        assert_eq!(r.pass(2), Some((35, 96)));
        assert_eq!(r.pass(3), None);
    }

    #[test]
    fn a_taken_name_gets_a_number() {
        let dir = std::env::temp_dir().join(format!("lemon_free_file_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.wav"), b"").unwrap();
        let mine = dir.join("a (2).wav");
        assert_eq!(free_file(&dir, "a.wav", &[mine.as_path()]), dir.join("a (3).wav"));
        assert_eq!(free_file(&dir, "b.wav", &[]), dir.join("b.wav"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
