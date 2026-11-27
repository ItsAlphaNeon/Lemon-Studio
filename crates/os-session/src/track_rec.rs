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
//! pass starts. A pass that has ended becomes a file and a clip of its own while the next one records. With Loop record
//! on, a take of several passes goes on tracks grouped under the first pass's (Group loop recorded takes), all its
//! passes muted unless Blend is on; a take of one pass, and the pieces of a jump back with Loop record off, go where a
//! take always went (the audio track, or the first free track), as they are.
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
    /// Loop record was on when the take started: its passes are loop takes (grouped, muted with Blend off).
    pub looping: bool,
    /// Every file the take writes or will write: no two passes get one name.
    used: Vec<PathBuf>,
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
    /// The files of its passes so far, the first one's `file`.
    files: Vec<PathBuf>,
    /// The audio clip channels of its placed passes, oldest first: the first one's clip finds the lane's track where
    /// it is now (tracks inserted above move it).
    channels: Vec<u16>,
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
/// "<stem>_3.wav"... as FL names them (spec: audio/disk-recording.md §1); a name taken on disk or by `used` (the take's
/// other files) gets " (2)", " (3)"... instead of being written over.
fn pass_file(file: &std::path::Path, k: usize, used: &[PathBuf]) -> PathBuf {
    match (k, file.file_stem()) {
        (0, _) | (_, None) => file.to_path_buf(),
        (k, Some(stem)) => {
            let dir = file.parent().unwrap_or(std::path::Path::new(""));
            let not: Vec<&std::path::Path> = used.iter().map(PathBuf::as_path).collect();
            free_file(dir, &format!("{}_{}.wav", stem.to_string_lossy(), k + 1), &not)
        }
    }
}

/// Lane `i`'s pass `k` file, chosen once (in pass order) and kept for the take.
fn pass_path(r: &mut TrackRec, i: usize, k: usize) -> PathBuf {
    while r.lanes[i].files.len() <= k {
        let n = r.lanes[i].files.len();
        let p = pass_file(&r.lanes[i].file, n, &r.used);
        r.used.push(p.clone());
        r.lanes[i].files.push(p);
    }
    r.lanes[i].files[k].clone()
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
    let mut used: Vec<PathBuf> = Vec::new();
    for (i, (&t, name)) in armed.iter().zip(&names).enumerate() {
        let takes = takes.clone();
        let file = s.rec_files.remove(&t).unwrap_or_else(|| {
            let not: Vec<&std::path::Path> = used.iter().map(PathBuf::as_path).collect();
            free_file(&dir, &file_name(&project, now, name), &not)
        });
        used.push(file.clone());
        match Recorder::start_from(file.clone(), rate, move |out| takes.tracks[i].ring.pop_into(out)) {
            Ok(w) => recs.push((t, Some(w))),
            Err(e) => {
                s.touched.views.push(View::Message(format!("Disk recording: {e}")));
                recs.push((t, None));
            }
        }
        lanes.push(Lane { mixer: t, name: name.clone(), files: vec![file.clone()], file, ..Lane::default() });
    }
    let song_pos = (!s.song_mode()).then_some(pos);
    let looping = Opt::Loop.on(&s.settings);
    s.transport.record_tracks(Some(takes.clone()));
    s.track_rec = Some(TrackRec { takes, recs, lanes, looping, used, rate, song_pos, ending: false, project, dir });
    s.touched.repaint = true;
}

/// The frames left out of the front of mixer track `t`'s take: the input's delay for a track that records an input
/// (spec: audio/track-io.md §2.5 "Where the take goes"), with its input latency setting on top ("Adjust position of
/// recorded audio"). Disk recording ▸ Latency compensation off leaves out the input's delay only; the track's own
/// setting still applies (audio/disk-recording.md §2).
fn head_of(s: &Session, rate: u32, t: u16) -> i64 {
    let p = &s.project;
    let Some(m) = p.mixer.iter().find(|m| m.index == t).filter(|m| m.audio_in >= 0) else { return 0 };
    let own = (f64::from(m.latency_in) * f64::from(rate) / 1000.0).round() as i64;
    if !s.settings.check(&crate::prefs::REC_LATENCY_COMP) {
        return own;
    }
    let tempo = p.tick_tempo(s.tempo().max(1.0));
    let tick = os_engine::laws::tick_to_frame(1.0, p.ppq.max(1), tempo, rate).max(1);
    s.transport.input_delay(tick) as i64 + own
}

/// A pass made into a file: its mixer track's ring, the file, its frames and the tick it goes to.
type Made = (usize, PathBuf, u64, u32);

/// Makes ring `i`'s pass `k` (it has ended) into `file`, cut from what its writer has on disk.
fn make_pass(
    s: &Session,
    r: &TrackRec,
    i: usize,
    k: usize,
    raw: &std::path::Path,
    file: PathBuf,
) -> Option<Result<Made, String>> {
    let ring = &r.takes.tracks[i];
    let (start, tick) = ring.pass(k)?;
    let (end, _) = ring.pass(k + 1)?;
    let lane = &r.lanes[i];
    let head = head_of(s, r.rate, lane.mixer);
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
        let t = r.recs[i].0;
        while let Some((end, _)) = r.takes.tracks[i].pass(r.lanes[i].made + 1) {
            let Some(w) = r.recs[i].1.as_ref() else { break };
            // Not on disk yet: the next tick.
            if end as i64 + head_of(s, r.rate, t) > w.written() as i64 {
                break;
            }
            let raw = w.part().to_path_buf();
            let k = r.lanes[i].made;
            let file = pass_path(&mut r, i, k);
            match make_pass(s, &r, i, k, &raw, file) {
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
    let Some(mut r) = s.track_rec.take() else { return };
    r.takes.on.store(false, Ordering::Release);
    for i in 0..r.recs.len() {
        let last = r.takes.tracks[i].begun().max(1) - 1;
        let file = pass_path(&mut r, i, last);
        let (t, Some(w)) = &r.recs[i] else { continue };
        let ring = &r.takes.tracks[i];
        // Passes not made yet (the writer was behind) are cut from the raw file once it is done.
        if last > r.lanes[i].made {
            w.keep_part();
        }
        let from = ring.pass(last).map_or(0, |(f, _)| f) as i64;
        w.set_head(from + head_of(s, r.rate, *t));
        w.set_file(file);
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
    let takes = r.takes.clone();
    for (i, (_, w)) in recs.into_iter().enumerate() {
        let Some(w) = w else { continue };
        let ring = &takes.tracks[i];
        lost += ring.dropped.load(Ordering::Relaxed);
        let last = ring.begun().max(1) - 1;
        let raw = w.part().to_path_buf();
        let joined = w.join();
        // Passes the writer had not reached while playing.
        if last > r.lanes[i].made {
            while r.lanes[i].made < last {
                let k = r.lanes[i].made;
                let file = pass_path(&mut r, i, k);
                match make_pass(s, &r, i, k, &raw, file) {
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
/// folder): each at its tick, its channel on its mixer track, on the mixer track's audio track or the first track free
/// over it. A loop take of several passes (Loop record on) goes, with Group loop recorded takes on, on tracks grouped
/// under its first pass's (or the audio track's when that is taken), named after the mixer track with Auto name playlist
/// tracks, all its passes muted unless Blend is on (spec: audio/disk-recording.md §1). Returns the message to show.
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
        // A loop take: Loop record on and more than one pass on this track. A take of one pass, or the pieces of a jump
        // back with Loop record off, are placed as a take always was.
        let looped = r.looping && r.takes.tracks[*i].begun() > 1;
        let lane = &mut r.lanes[*i];
        let end = at + len.max(1);
        // The lane's first pass, on whatever track it is now.
        let first = lane.channels.first().and_then(|&ch| {
            crate::edit::playlist::clips(s)
                .iter()
                .find(|c| c.source == os_model::ClipSource::Channel(ch))
                .map(|c| c.track)
        });
        // Into the mixer track's audio track when it has one free over the take (spec: ui/playlist.md §6c); a loop take
        // with Group loop recorded takes on, else onto a track grouped under it (or under the take's first pass),
        // after the group.
        let lead = crate::track_links::audio_tracks_of(s, t).first().copied().or(first);
        let track = match lead {
            Some(lead)
                if group
                    && looped
                    && (lead != crate::track_links::record_track(s, t, *at, end) || !lane.channels.is_empty()) =>
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
        // Blend off: all of a loop take's passes are muted, the newest one too (FL, measured with six passes).
        if looped && !blend {
            crate::loop_takes::mute_takes(s, &lane.channels);
        }
        let track = crate::edit::playlist::clips(s)[clip].track;
        if rename && looped {
            crate::loop_takes::name_take_track(s, track, &lane.name);
        }
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

    /// Latency compensation off leaves out the input's delay, not the track's own input latency setting.
    #[test]
    #[allow(deprecated)]
    fn latency_compensation_off_keeps_the_tracks_own_latency() {
        let settings = crate::settings_file::SettingsStore::memory();
        let dir = crate::factory_folder::dir(&settings);
        let mut s =
            Session::new(crate::projects::test_project(), crate::transport::Transport::new(dir), settings);
        let m = s.project.get_mut().mixer.iter_mut().find(|m| m.index == 1).unwrap();
        m.audio_in = 0;
        m.latency_in = 10.0;
        s.settings.set_check(&crate::prefs::REC_LATENCY_COMP, false);
        assert_eq!(head_of(&s, 48000, 1), 480, "10 ms at 48 kHz");
        s.settings.set_check(&crate::prefs::REC_LATENCY_COMP, true);
        assert!(head_of(&s, 48000, 1) > 480, "the input's delay on top");
        assert_eq!(head_of(&s, 48000, 2), 0, "no input");
    }

    /// A pass's "<stem>_N.wav" that is on disk already, or that the take uses (another track "Vox_2"), is not written
    /// over: it gets a number of its own.
    #[test]
    fn a_pass_never_writes_over_a_file() {
        let dir = std::env::temp_dir().join(format!("lemon_pass_file_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Vox_2.wav"), b"keep").unwrap();
        let take = dir.join("Vox.wav");
        assert_eq!(pass_file(&take, 0, &[]), take);
        assert_eq!(pass_file(&take, 1, &[]), dir.join("Vox_2 (2).wav"), "on disk");
        assert_eq!(pass_file(&take, 2, &[dir.join("Vox_3.wav")]), dir.join("Vox_3 (2).wav"), "used by the take");
        assert_eq!(pass_file(&take, 3, &[]), dir.join("Vox_4.wav"));
        assert_eq!(std::fs::read(dir.join("Vox_2.wav")).unwrap(), b"keep");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
