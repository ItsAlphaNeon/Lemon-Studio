// Copyright (C) 2026 ItsAlphaNeon
// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Settings > Info (OPTIONS > Project info, F11): the project's title, author, genre, comments and web link, edited in
//! their boxes. A box takes the keyboard when clicked and edits with the file dialog's text field (`dialog::Field`:
//! caret, Backspace / Delete, Left / Right / Home / End, Ctrl+A); Comments has several lines. The project keeps the
//! text (`Project::info`), so an edit is an unsaved change and is saved with the project; the window caption shows the
//! title instead of the file name (`App::title`). The button at the Genre box's right end offers FL's genre list.
//!
//! The page's footer tells when the project was started, the time spent working on it (event 237, which counts on
//! while the project is open: `WorkClock`) and what the project holds. "Show info on opening" (event 10) makes opening
//! the project show its Song info box for a few seconds; it then fades out, and any click in the app's windows takes it
//! away at once.
// spec: ui/settings.md "Info", ui/settings-choices.md "Info", flp/events/project-info.md

#![allow(deprecated)] // writes the project directly (`crate::locked`; control-plane step 14 removes this)
use std::time::{Duration, Instant};

use os_model::{Project, WorkTime};


/// The time spent working on the project this session. The spec has the time spent count on while the project is open
/// but does not say what FL counts (spec: flp/events/project-info.md, event 237 and "Open questions"); Lemon Studio's
/// rule: it counts while a project is open and Lemon Studio's main window is the foreground window. The project keeps
/// the time spent it was opened with; saving writes that plus this clock (`stamp`), so the counting itself is no
/// unsaved change.
#[derive(Debug, Default)]
pub struct WorkClock {
    /// Counted before the main window last left the foreground.
    banked: Duration,
    /// Since when the main window is the foreground window.
    since: Option<Instant>,
}

impl WorkClock {
    /// The main window came to the foreground (`on`) or left it.
    pub fn focus(&mut self, on: bool, now: Instant) {
        match (on, self.since) {
            (true, None) => self.since = Some(now),
            (false, Some(t)) => {
                self.banked += now.saturating_duration_since(t);
                self.since = None;
            }
            _ => {}
        }
    }

    /// Counted until `now`.
    pub fn elapsed(&self, now: Instant) -> Duration {
        self.banked + self.since.map_or(Duration::ZERO, |t| now.saturating_duration_since(t))
    }

    /// Back to zero (a project opened or created, its working time reset); it goes on counting while the window is in
    /// front.
    pub fn restart(&mut self, now: Instant) {
        self.banked = Duration::ZERO;
        if self.since.is_some() {
            self.since = Some(now);
        }
    }
}

/// What the Info page keeps beside the project: the working time's clock and the Song info box.
#[derive(Debug, Default)]
pub struct Credits {
    pub clock: WorkClock,
    /// The Song info box is shown (the project was opened with "Show info on opening" on).
    pub song_box: bool,
    /// Since when the Song info box is shown: the first tick after opening (`App::song_info_tick`), so the time a
    /// project takes to open does not count.
    pub(crate) song_since: Option<Instant>,
    /// How opaque the Song info box is painted: 1 until it fades.
    pub(crate) song_alpha: f32,
    /// The minutes of time spent the Info page showed last (`App::work_tick`).
    pub(crate) minutes: u64,
}

/// A day count since 1899-12-30 00:00 (local time) as the Info page shows it: "5/22/2015 4:13 PM" (the spec's
/// examples of the system's short date and time; Lemon Studio always uses this form). `None` for a value no date has.
pub fn date_text(days: f64) -> Option<String> {
    let epoch = chrono::NaiveDate::from_ymd_opt(1899, 12, 30)?.and_hms_opt(0, 0, 0)?;
    let secs = (days * 86_400.0).round();
    if !secs.is_finite() || secs.abs() > 1e12 {
        return None;
    }
    let t = epoch.checked_add_signed(chrono::TimeDelta::try_seconds(secs as i64)?)?;
    Some(t.format("%-m/%-d/%Y %-I:%M %p").to_string())
}

/// The time spent as the Info page words it (spec: ui/settings-choices.md "Info", footer line 2). One hour and one
/// minute are worded "1 hour", "1 minute": FL's wording of them was not seen.
pub fn spent_text(days: f64) -> String {
    let minutes = (days.max(0.0) * 1440.0 + 1e-6).floor() as u64;
    if minutes == 0 {
        return "Less than a minute was spent working on it.".into();
    }
    let (h, m) = (minutes / 60, minutes % 60);
    let word = |n: u64, one: &'static str, many: &'static str| if n == 1 { one } else { many };
    format!("Total time spent on it:  {h} {} {m} {}.", word(h, "hour", "hours"), word(m, "minute", "minutes"))
}

/// The footer's counts (spec: flp/events/project-info.md "Info counts").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Counts {
    /// Channels that hold a plugin plus mixer effect slots that hold one.
    pub plugins: usize,
    pub channels: usize,
    /// Mixer tracks a channel is routed to or another track routes to (`mixer_tracks`).
    pub mixer_tracks: usize,
    pub arrangements: usize,
    /// Playlist tracks holding at least one clip, and the clips (the current arrangement's).
    pub pl_tracks: usize,
    pub pl_clips: usize,
    /// Notes of all patterns.
    pub notes: usize,
}

pub fn counts(p: &Project) -> Counts {
    let slots = p.mixer.iter().flat_map(|m| m.slots.iter()).filter(|s| s.as_ref().is_some_and(|s| !s.name.is_empty())).count();
    let clips = p.arrangement().map_or(&[][..], |a| &a.clips[..]);
    let mut tracks: Vec<u16> = clips.iter().map(|c| c.track).collect();
    tracks.sort_unstable();
    tracks.dedup();
    Counts {
        plugins: p.channels.iter().filter(|c| c.plugin.as_deref().is_some_and(|n| !n.is_empty())).count() + slots,
        channels: p.channels.len(),
        mixer_tracks: mixer_tracks(p),
        arrangements: p.arrangements.len(),
        pl_tracks: tracks.len(),
        pl_clips: clips.len(),
        notes: p.patterns.iter().map(|q| q.notes.len()).sum(),
    }
}

/// The footer's "Mixer Tracks" (spec: flp/events/project-info.md "Info counts"): the mixer tracks a channel is routed to
/// (the master too, when a channel goes straight to it) together with the tracks that receive a route from another
/// track, the master never counted as a receiver. A track's name, effects and own routes do not count.
pub fn mixer_tracks(p: &Project) -> usize {
    let mut used: Vec<u16> = p.channels.iter().map(|c| c.mixer_track).collect();
    for m in &p.mixer {
        used.extend(m.routes.iter().copied().filter(|&r| r != 0 && r != m.index));
    }
    used.sort_unstable();
    used.dedup();
    used.len()
}

/// How long the Song info box stays, then how long it takes to fade out (Lemon Studio's choice).
pub(crate) const SONG_HOLD: f32 = 5.0;
pub(crate) const SONG_FADE: f32 = 0.5;

/// The Song info box's size and colours (spec: flp/events/project-info.md "The Song info box").
pub(crate) const SONG_BOX: (f32, f32) = (502.0, 272.0);
pub(crate) const SONG_BG: u32 = 0xCCD1D5;
pub(crate) const SONG_BORDER: u32 = 0x8E9397;
pub(crate) const SONG_INK: u32 = 0x1A1C1E;

/// FL's genre menu after "(none)", in its order (spec: ui/settings-choices.md "Info").
pub const GENRES: [&str; 56] = [
    "Acid House",
    "Afro House",
    "Afrobeats",
    "Amapiano",
    "Ambient",
    "Bass House",
    "Boom Bap",
    "Breakbeat",
    "Cinematic",
    "Classic House",
    "Classical",
    "Country",
    "D&B",
    "Dancehall",
    "Deep House",
    "Disco",
    "Downtempo",
    "Drill",
    "Dubstep",
    "EDM",
    "Electro",
    "Electronica",
    "Film Score",
    "Funk",
    "Future Bass",
    "Future Rave",
    "Hardstyle",
    "Hip hop",
    "House",
    "Hybrid Trap",
    "Hyperpop",
    "Hypertrance",
    "Indie",
    "Jazz",
    "Jersey Club",
    "Latin",
    "Latin House",
    "Lo-Fi",
    "Lounge",
    "Melodic Techno",
    "Phonk",
    "Pop",
    "Progressive House",
    "Psytrance",
    "R&B",
    "Rage Trap",
    "Reggaeton",
    "Rock",
    "Slap House",
    "Soul",
    "Synthwave",
    "Tech House",
    "Techno",
    "Trance",
    "Trap",
    "Tropical House",
];

/// The date and time now, as a project's start (days since 1899-12-30): fixed in a test that runs the motions' clock
/// (`anim::set_clock_at`, the equivalence harness), so a project made new starts the same on every run.
pub(crate) fn work_time_now() -> WorkTime {
    #[cfg(test)]
    if !crate::time::still() {
        return WorkTime { started: 46_000.5, spent: 0.0 };
    }
    os_flp::project::work_time_now()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What the footer counts (spec: flp/events/project-info.md "Info counts"): channels holding a plugin plus effect
    /// slots holding one; the playlist tracks holding a clip; the notes of all patterns.
    #[test]
    fn footer_counts() {
        use os_model::{Arrangement, Clip, ClipSource, MixerTrack, Note, Pattern, SlotPlugin};
        let mut p = crate::projects::default_project();
        let ch = p.channels[0].clone();
        p.channels = vec![ch.clone(), ch.clone(), ch];
        p.channels[0].plugin = Some("Lemon Sampler".into());
        p.channels[1].plugin = Some(String::new());
        p.channels[2].plugin = None;
        (p.channels[0].mixer_track, p.channels[1].mixer_track, p.channels[2].mixer_track) = (1, 1, 0);
        let slot = |name: &str| Some(SlotPlugin { id: os_model::Id::NONE, name: name.into(), state: Vec::new() });
        let mut m = MixerTrack::new(0);
        m.slots[0] = slot("Fruity Limiter");
        m.slots[3] = slot("");
        let mut m2 = MixerTrack::new(1);
        m2.slots[9] = slot("Fruity Reeverb 2");
        p.mixer = vec![m, m2, MixerTrack::new(2)];
        let clip = |pos, track| Clip::new(pos, 96, track, ClipSource::Pattern(1));
        p.arrangements = vec![
            Arrangement { number: 0, clips: vec![clip(0, 0), clip(96, 0), clip(0, 4)], ..Default::default() },
            Arrangement { number: 1, clips: vec![clip(0, 7)], ..Default::default() },
        ];
        p.current_arrangement = None;
        p.patterns = vec![
            Pattern { notes: vec![Note::step(0, 0), Note::step(0, 24)], ..Default::default() },
            Pattern { notes: vec![Note::step(1, 0)], ..Default::default() },
        ];
        let want = Counts { plugins: 3, channels: 3, mixer_tracks: 2, arrangements: 2, pl_tracks: 2, pl_clips: 3, notes: 3 };
        assert_eq!(counts(&p), want);
        // The playlist counts follow the current arrangement.
        p.current_arrangement = Some(1);
        assert_eq!((counts(&p).pl_tracks, counts(&p).pl_clips), (1, 1));
    }

    /// "Mixer Tracks" (spec: flp/events/project-info.md "Info counts"): the tracks channels are routed to, the master
    /// included, and the tracks another track sends to, the master not included; names, effects and a track's own
    /// routes do not count.
    #[test]
    fn mixer_tracks_count() {
        use os_model::{MixerTrack, SlotPlugin};
        let mut p = crate::projects::default_project();
        let ch = p.channels[0].clone();
        // Inserts 1-4 route to the master (as FL's do); one channel straight to the master.
        p.mixer = (0..5).map(MixerTrack::new).collect();
        p.channels = vec![ch.clone()];
        p.channels[0].mixer_track = 0;
        assert_eq!(mixer_tracks(&p), 1, "a channel on the master counts the master; routes to the master do not");
        // A named track holding an effect, which nothing routes to: not counted.
        p.mixer[2].name = Some("Bus".into());
        p.mixer[2].slots[0] = Some(SlotPlugin { id: os_model::Id::NONE, name: "Fruity Limiter".into(), state: Vec::new() });
        assert_eq!(mixer_tracks(&p), 1);
        // A send: insert 1 routes to insert 3 too, so 3 counts, but not 1 for its own routes.
        p.mixer[1].routes = vec![0, 3];
        assert_eq!(mixer_tracks(&p), 2);
        // Channels on inserts: each insert once, and the master no more once no channel goes to it.
        p.channels = vec![ch.clone(), ch.clone(), ch];
        (p.channels[0].mixer_track, p.channels[1].mixer_track, p.channels[2].mixer_track) = (4, 4, 3);
        assert_eq!(mixer_tracks(&p), 2, "inserts 3 and 4");
        p.mixer[4].routes = vec![2];
        assert_eq!(mixer_tracks(&p), 3, "insert 2 receives from insert 4");
    }

    /// The working time counts only while the main window is in front (Lemon Studio's rule, `WorkClock`).
    #[test]
    fn work_clock_counts_in_front() {
        let t = Instant::now();
        let s = Duration::from_secs;
        let mut c = WorkClock::default();
        assert_eq!(c.elapsed(t + s(50)), Duration::ZERO, "not in front yet");
        c.focus(true, t);
        c.focus(true, t + s(5));
        assert_eq!(c.elapsed(t + s(10)), s(10));
        c.focus(false, t + s(10));
        c.focus(false, t + s(20));
        assert_eq!(c.elapsed(t + s(100)), s(10));
        c.focus(true, t + s(100));
        assert_eq!(c.elapsed(t + s(105)), s(15));
        c.restart(t + s(105));
        assert_eq!(c.elapsed(t + s(106)), s(1));
        c.focus(false, t + s(106));
        c.restart(t + s(107));
        assert_eq!(c.elapsed(t + s(200)), Duration::ZERO);
    }

}
