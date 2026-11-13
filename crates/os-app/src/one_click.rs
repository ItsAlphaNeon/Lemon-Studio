// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! One-click audio recording (spec: ui/one-click-recording.md §2): the toolbar's microphone (and TOOLS > One-click
//! audio recording) picks an input, plays the song with Record armed and turns what came in into a new audio clip
//! channel and its clip in the playlist where playback started, or (Into the audio editor) into Tesla on the master
//! track (spec: plugins/tesla/README.md "Recording into Tesla from Lemon Studio").

use crate::session::{Session, View};
use crate::capture::{self, capture, Input, InputStream, Recorder};
use crate::playlist::{Cmd, Mark, PItem};

/// The undo step a take is placed with (as FL's History names it).
pub const RECORDED: &str = "Recorded items";

/// A recording under way, or its file being made.
pub struct AudioRec {
    /// A system input device's stream (`None`: the ASIO driver's inputs, in its output callback).
    stream: Option<InputStream>,
    recorder: Option<Recorder>,
    /// The system input's rate (`None`: the ASIO driver's, the output's).
    rate: Option<u32>,
    /// Where playback started: the clip goes there.
    start_tick: u32,
    start_secs: f64,
    /// Where the song's start falls in what came in: a running sum and count (`one_click_tick`).
    est: (f64, u32),
    /// Taking in has stopped; the writer is making the file.
    pub(crate) ending: bool,
    /// The take goes into Tesla on the master track instead of the playlist.
    editor: bool,
}

impl AudioRec {
    /// Taking in has stopped; the file is being made.
    pub fn ending(&self) -> bool {
        self.ending
    }
}

/// A stereo and a mono header per device with its inputs, each recording into the audio editor (`editor`) or the
/// playlist; the input picked last has a dot.
pub fn inputs(editor: bool) -> Vec<PItem> {
    let last = capture::last_input();
    let mut v = Vec::new();
    let list = devices();
    for (device, asio, channels) in &list {
        if *channels == 0 {
            v.push(header(device.clone()));
            v.push(PItem::new("(no audio inputs)", Cmd::Nothing).off());
            continue;
        }
        let inputs = Input::of(device, *asio, *channels);
        for stereo in [true, false] {
            let group: Vec<&Input> = inputs.iter().filter(|i| i.stereo == stereo).collect();
            if group.is_empty() {
                continue;
            }
            v.push(header(format!("{device} - {}", if stereo { "stereo" } else { "mono" })));
            for i in group {
                v.push(PItem::new(i.label(), Cmd::OneClick(i.clone(), editor)).mark(Mark::Radio(last.as_ref() == Some(i))));
            }
        }
    }
    if list.is_empty() {
        v.push(PItem::new("(no audio inputs)", Cmd::Nothing).off());
    }
    v
}

fn header(label: String) -> PItem {
    let mut h = PItem::new(label, Cmd::Nothing);
    h.head = true;
    h
}

/// `capture::devices`, listed again at most every few seconds (the menu is drawn on every repaint while open).
fn devices() -> Vec<(String, bool, u16)> {
    #[cfg(test)]
    {
        vec![("Test ASIO".to_string(), true, 2)]
    }
    #[cfg(not(test))]
    {
        use std::sync::Mutex;
        use std::time::{Duration, Instant};
        type Listed = Option<(Instant, Vec<(String, bool, u16)>)>;
        static CACHE: Mutex<Listed> = Mutex::new(None);
        let mut c = CACHE.lock().unwrap_or_else(|e| e.into_inner());
        match &*c {
            Some((at, list)) if at.elapsed() < Duration::from_secs(3) => list.clone(),
            _ => {
                let list = capture::devices();
                *c = Some((Instant::now(), list.clone()));
                list
            }
        }
    }
}

/// `begin`'s work for `App::one_click_begin`: `pos` is the song position, where the clip goes.
pub(crate) fn begin_in(s: &mut Session, input: Input, stream: Option<InputStream>, editor: bool, pos: u32) -> Result<(), String> {
    let rate = stream.as_ref().map(|s| s.rate);
    let dir = crate::data_folder::resolve(&s.project, s.path.as_deref()).or_else(crate::user_dir::theme_dir).unwrap_or_else(std::env::temp_dir).join("Recorded");
    let name = capture::file_name(&crate::backup::project_name(s.path.as_deref()), chrono::Local::now().naive_local());
    capture().start(input.first, input.stereo);
    let recorder = match Recorder::start(dir.join(name), rate.unwrap_or(48000)) {
        Ok(r) => r,
        Err(e) => {
            capture().stop();
            return Err(e);
        }
    };
    let start_tick = pos;
    let p = &s.project;
    let tempo = p.tick_tempo(s.tempo().max(1.0));
    let start_secs = os_engine::laws::tick_to_frame(f64::from(start_tick), p.ppq.max(1), tempo, 48000) as f64 / 48000.0;
    s.rec.audio = true;
    s.audio_rec = Some(AudioRec { stream, recorder: Some(recorder), rate, start_tick, start_secs, est: (0.0, 0), ending: false, editor });
    Ok(())
}

/// A take's message, with the frames the disk lost.
pub(crate) fn with_lost(placed: String) -> String {
    match capture().dropped() {
        0 => placed,
        lost => format!("{placed} ({lost} frames were lost: the disk was too slow)"),
    }
}

/// Every tick: follows where the song's start falls in the input; on stop, pause or disarm finishes the file;
/// once it is made, places it (spec §2 The take).
/// `lock_len` as `playlist::place_in` takes it. A finished take for the audio editor comes back for the interface.
pub(crate) fn tick_in(s: &mut Session, lock_len: Option<(os_model::ClipSource, u32)>) -> Option<std::path::PathBuf> {
    let Some(r) = s.audio_rec.as_mut() else { return None };
    if !r.ending {
        let t = &s.transport;
        let live = t.playing() && !t.paused() && s.rec.count.is_none();
        let device = t.opened();
        let rate = r.rate.or(device.as_ref().map(|d| d.rate)).unwrap_or(48000);
        if live && t.song() {
            r.est.0 += capture().frames() as f64 - (t.seconds() - r.start_secs) * f64::from(rate);
            r.est.1 += 1;
        }
        if t.playing() && !t.paused() && s.rec.armed {
            return None;
        }
        // The input heard with the song's start came in after the output's and the input's latencies (ASIO).
        let latency = match (&r.rate, device.filter(|d| d.host == crate::device::ASIO)) {
            (None, Some(d)) => i64::from(d.latency.unwrap_or(0)) + i64::from(d.input_latency.unwrap_or(0)),
            _ => 0,
        };
        let head = if r.est.1 > 0 { (r.est.0 / f64::from(r.est.1)).round() as i64 + latency } else { capture().frames() as i64 };
        capture().stop();
        r.stream = None;
        r.rate = Some(rate);
        if let Some(w) = &r.recorder {
            w.set_rate(rate);
            w.set_head(head);
            w.finish();
        }
        r.ending = true;
        s.rec.audio = false;
        s.touched.repaint = true;
        return None;
    }
    if !r.recorder.as_ref().is_none_or(Recorder::done) {
        return None;
    }
    let Some(r) = s.audio_rec.take() else { return None };
    let rate = r.rate.unwrap_or(48000).max(1);
    let message = match r.recorder.map_or(Err("no recorder".into()), Recorder::join) {
        Ok((path, frames)) if frames * 1000 >= u64::from(rate) => {
            if r.editor {
                return Some(path);
            }
            with_lost(place_take(s, &path, frames, rate, r.start_tick, lock_len))
        }
        Ok((path, _)) => {
            let _ = std::fs::remove_file(path);
            "Nothing was recorded".into()
        }
        Err(e) => format!("Audio recording: {e}"),
    };
    s.touched.views.push(View::Message(message));
    s.touched.repaint = true;
    None
}

/// The take's audio clip channel and clip at `start`, on the first track free over it, which becomes an audio track
/// on the take's mixer track: one undo step.
fn place_take(s: &mut Session, path: &std::path::Path, frames: u64, rate: u32, start: u32, lock_len: Option<(os_model::ClipSource, u32)>) -> String {
    let p = &s.project;
    let tempo = p.tick_tempo(s.tempo().max(1.0));
    let len = (frames as f64 / f64::from(rate) * tempo / 60.0 * f64::from(p.ppq.max(1))).ceil() as u32;
    let track = crate::playlist::free_track(s, 0, start, start + len.max(1));
    let text = path.to_string_lossy();
    let added = s.samples.wave(&text).and_then(|_| {
        crate::playlist::snapshot_as(s, RECORDED);
        crate::playlist::add_audio_clip_on(s, &text, track, start, None, lock_len)
    });
    match added {
        Some(i) => {
            // The take's track becomes an audio track on the take's mixer track, as the original binds the track
            // it records into to a mixer track (spec: ui/one-click-recording.md §1, ui/playlist.md §6c).
            crate::track_links::bind_take(s, i);
            format!("Recorded {}", path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default())
        }
        None => format!("{text} could not be read"),
    }
}

#[cfg(test)]
mod tests {

    /// The tests that record share the one input capture.
    static RECORDING: std::sync::Mutex<()> = std::sync::Mutex::new(());

}
