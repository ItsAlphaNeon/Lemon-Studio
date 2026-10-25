// Copyright (C) 2026 ItsAlphaNeon
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

use std::time::{Duration, Instant};

use os_model::{Project, WorkTime};
use os_ui::{Color, Font, Rect};
use winit::keyboard::{Key, NamedKey};

use crate::app::{Action, App, State, Ui};
use crate::dialog::Field;
use crate::views::settings::{self, InfoEdit, InfoField};

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
    song_since: Option<Instant>,
    /// How opaque the Song info box is painted: 1 until it fades.
    song_alpha: f32,
    /// The minutes of time spent the Info page showed last (`App::work_tick`).
    minutes: u64,
}

/// The project's time spent at `now`, in days: what it was opened or created with, plus this session's clock.
pub fn spent_days(s: &State, now: Instant) -> f64 {
    s.project.work_time.map_or(0.0, |w| w.spent) + s.credits.clock.elapsed(now).as_secs_f64() / 86_400.0
}

/// The project as it is saved (`App::save_to`): its time spent includes the session's clock (spec:
/// flp/events/project-info.md "Writer notes": "then adds the time spent").
pub(crate) fn stamp(s: &State, p: &mut Project) {
    let started = s.project.work_time.map_or_else(|| os_flp::project::work_time_now().started, |w| w.started);
    p.work_time = Some(WorkTime { started, spent: spent_days(s, Instant::now()) });
}

/// A project was opened (`App::open`): one whose file has no start is started now, as a writer creating a project does
/// (spec: flp/events/project-info.md "Writer notes"); the clock starts again; the Song info box shows when the project
/// asks for it (event 10).
pub(crate) fn opened(s: &mut State) {
    s.project.work_time.get_or_insert_with(os_flp::project::work_time_now);
    s.credits.clock.restart(Instant::now());
    s.credits.song_box = s.project.show_info == Some(true);
    (s.credits.song_since, s.credits.song_alpha) = (None, 1.0);
}

/// A new project: started now, nothing spent (spec: ui/settings-choices.md "Info": "A new project starts now").
pub(crate) fn created(s: &mut State) {
    s.project.work_time = Some(os_flp::project::work_time_now());
    s.credits.clock.restart(Instant::now());
    s.credits.song_box = false;
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

/// The Info page's footer (spec: ui/settings-choices.md "Info"): the start, plus the version that saved the file when
/// the project was loaded from one (event 199 as loaded: saving does not change it); the time spent; the counts.
pub fn footer(s: &State, now: Instant) -> Vec<String> {
    let p = &s.project;
    let mut lines = Vec::new();
    if let Some(date) = p.work_time.and_then(|w| date_text(w.started)) {
        let saved_by = p.saved_by.trim_matches(|c: char| c == '\0' || c.is_whitespace());
        lines.push(match saved_by {
            "" => format!("This project was started on {date}"),
            v => format!("This project was started on {date}, last saved in {v}"),
        });
    }
    lines.push(spent_text(spent_days(s, now)));
    let c = counts(p);
    lines.push(format!("Plugins: {}, Channels: {}, Mixer Tracks: {}", c.plugins, c.channels, c.mixer_tracks));
    lines.push(format!("Arrangements: {}, PL tracks: {}, PL clips: {}, Notes: {}", c.arrangements, c.pl_tracks, c.pl_clips, c.notes));
    lines
}

/// How long the Song info box stays, then how long it takes to fade out (Lemon Studio's choice).
const SONG_HOLD: f32 = 5.0;
const SONG_FADE: f32 = 0.5;

/// The Song info box's size and colours (spec: flp/events/project-info.md "The Song info box").
const SONG_BOX: (f32, f32) = (502.0, 272.0);
const SONG_BG: u32 = 0xCCD1D5;
const SONG_BORDER: u32 = 0x8E9397;
const SONG_INK: u32 = 0x1A1C1E;

/// The Song info box, centred on the main window of size `screen`.
pub fn song_box_rect(screen: (f32, f32)) -> Rect {
    Rect::new(((screen.0 - SONG_BOX.0) / 2.0).floor(), ((screen.1 - SONG_BOX.1) / 2.0).floor(), SONG_BOX.0, SONG_BOX.1)
}

/// Paints the Song info box while it is shown (spec: flp/events/project-info.md "The Song info box"): borderless, light
/// grey with a 1 px darker frame; "<author> - <title>" in large dark text at the top left, the genre at the top right,
/// the comments with their line breaks, the web link at the bottom right. A click on it closes it (`Action::SongInfo`), as
/// does a click anywhere else (`App::mouse_down`). It is painted see-through while it fades. The app's own fonts.
pub(crate) fn paint_song_info(ui: &mut Ui, s: &State) {
    if !s.credits.song_box {
        return;
    }
    let r = song_box_rect(s.screen);
    ui.hit(r, Action::SongInfo, "Click to close");
    let a = (s.credits.song_alpha.clamp(0.0, 1.0) * 255.0).round() as u8;
    let color = |hex: u32| Color::rgb(hex).with_alpha(a);
    ui.c.fill_rect(r, color(SONG_BORDER));
    ui.c.fill_rect(r.inset(1.0), color(SONG_BG));
    let ink = color(SONG_INK);
    let (big, small) = (Font::new(22).bold(), Font::new(14));
    let inner = r.inset(12.0);
    let i = &s.project.info;
    let head = [i.author.as_deref(), i.title.as_deref()].into_iter().flatten().filter(|t| !t.is_empty()).collect::<Vec<_>>().join(" - ");
    let genre = i.genre.as_deref().unwrap_or("");
    let (big_h, lh) = (ui.c.line_height(big), ui.c.line_height(small));
    let gw = ui.c.text_width(small, genre).min(inner.w / 2.0);
    ui.c.text_clip(small, inner.right() - gw, inner.y + (big_h - lh) / 2.0, gw, ink, genre);
    ui.c.text_clip(big, inner.x, inner.y, inner.w - gw - 12.0, ink, &head);
    let url = i.url.as_deref().unwrap_or("");
    let uw = ui.c.text_width(small, url).min(inner.w);
    ui.c.text_clip(small, inner.right() - uw, inner.bottom() - lh, uw, ink, url);
    // The comments between them, wrapped to the box; what does not fit is cut.
    let text = i.comments.as_deref().unwrap_or("");
    let bottom = inner.bottom() - lh - 6.0;
    let mut y = inner.y + big_h + 10.0;
    for (start, n) in settings::wrap(ui, small, text, inner.w) {
        if y + lh > bottom {
            break;
        }
        let line: String = text.chars().skip(start).take(n).collect();
        ui.c.text(small, inner.x, y, ink, &line);
        y += lh;
    }
}

impl App {
    /// A click on an Info box: it takes the keyboard with the caret at the end of its text. Another box being edited is
    /// accepted first; a click on the box being edited changes nothing.
    pub(crate) fn info_act(&mut self, field: InfoField) {
        let s = &mut self.state;
        if s.settings.info_edit.as_ref().is_some_and(|e| e.field == field) {
            return;
        }
        accept_info(s);
        let mut text = Field::default();
        text.set(field.get(&s.project.info).as_deref().unwrap_or(""));
        s.settings.info_edit = Some(InfoEdit { field, text });
        self.invalidate();
    }

    /// Keys while an Info box has the keyboard. It takes every key, so none reaches the typing keyboard or a shortcut;
    /// false when no box has it.
    pub(crate) fn info_key(&mut self, key: &Key, ctrl: bool, alt: bool) -> bool {
        let s = &mut self.state;
        if s.settings.info_edit.is_none() {
            return false;
        }
        // The box is on screen only while the Info page is.
        if !(s.settings.open && s.settings.tab == settings::INFO) {
            accept_info(s);
            return false;
        }
        let e = s.settings.info_edit.as_mut().expect("checked");
        match key {
            Key::Named(NamedKey::Escape) => s.settings.info_edit = None,
            Key::Named(NamedKey::Enter) if e.field.multiline() => e.text.insert("\n"),
            Key::Named(NamedKey::Enter) => accept_info(s),
            Key::Named(k @ (NamedKey::Home | NamedKey::End | NamedKey::ArrowUp | NamedKey::ArrowDown)) if e.field.multiline() => line_key(&mut e.text, *k),
            _ => {
                e.text.key(key, ctrl, alt);
            }
        }
        self.invalidate();
        true
    }

    /// Whether an Info box has the keyboard (held keys repeat in it).
    pub fn info_editing(&self) -> bool {
        self.state.settings.info_edit.is_some()
    }

    /// The genre button was clicked: the genre menu under its left edge (`x`, `y`: the button's bottom left), "(none)"
    /// then `GENRES`, the project's genre marked (spec: ui/settings-choices.md "Info").
    pub(crate) fn genre_menu(&mut self, x: f32, y: f32) {
        use crate::playlist::{Cmd, Mark, PItem, Popup};
        let now = self.state.project.info.genre.as_deref().unwrap_or("");
        let items = std::iter::once(PItem::new("(none)", Cmd::Genre(None)).mark(Mark::Radio(now.is_empty())))
            .chain(GENRES.iter().map(|&g| PItem::new(g, Cmd::Genre(Some(g))).mark(Mark::Radio(now == g))))
            .collect();
        self.state.playlist.popup = Some(Popup { x, y: y + 1.0, items, sub: None, scroll: [0; 2] });
        self.invalidate();
    }

    /// A genre menu row was picked (`Cmd::Genre`): its name goes into the Genre box as if typed there, "(none)" (None)
    /// empties it. A box being edited is accepted first. Returns the message.
    pub(crate) fn set_genre(&mut self, genre: Option<&'static str>) -> Option<String> {
        let s = &mut self.state;
        accept_info(s);
        let before = s.project.info.clone();
        let value = &mut s.project.info.genre;
        if value.as_deref().unwrap_or("") != genre.unwrap_or("") {
            *value = genre.map(str::to_string);
        }
        crate::history::project_info(s, before);
        self.invalidate();
        Some(format!("Genre: {}", genre.unwrap_or("(none)")))
    }

    /// Lemon Studio's main window came to the foreground (`on`) or left it: the working time counts only while it is in
    /// front (`WorkClock`).
    pub fn main_focused(&mut self, on: bool) {
        self.state.credits.clock.focus(on, Instant::now());
    }

    /// Periodic (`App::tick`): the Info page shows the time spent by the minute; it repaints when that changes.
    pub(crate) fn work_tick(&mut self, now: Instant) {
        let minutes = (spent_days(&self.state, now) * 1440.0) as u64;
        if minutes != self.state.credits.minutes {
            self.state.credits.minutes = minutes;
            if self.state.settings.open && self.state.settings.tab == settings::INFO {
                self.invalidate();
            }
        }
    }

    /// Info ▸ Reset working time: the time spent goes to zero at once, without a question, and the start stays (spec:
    /// ui/settings-choices.md "Info"). It is a change to the project, also when the project held no time spent yet
    /// (the session's time is not in it, `WorkClock`): the saved copy then forgets its time so the two differ.
    pub(crate) fn reset_work_time(&mut self) {
        let s = &mut self.state;
        s.project.work_time.get_or_insert_with(os_flp::project::work_time_now).spent = 0.0;
        s.credits.clock.restart(Instant::now());
        if self.saved.0.work_time == s.project.work_time {
            self.saved.0.work_time = None;
        }
        s.message = Some("Working time reset".into());
        self.invalidate();
    }

    /// Info ▸ Show info on opening (event 10): switched for the project and saved with it.
    pub(crate) fn toggle_show_info(&mut self) {
        let p = &mut self.state.project;
        let on = !p.show_info.unwrap_or(false);
        p.show_info = Some(on);
        self.state.message = Some(format!("Show info on opening: {}", if on { "on" } else { "off" }));
    }

    /// A click on the Song info box closes it, as does a click anywhere else in the main window or in a plugin window.
    pub(crate) fn close_song_info(&mut self) {
        if self.state.credits.song_box {
            self.state.credits.song_box = false;
            self.damage_rect(song_box_rect(self.state.screen));
        }
    }

    /// The Song info box is fading out: the shell ticks at animation rate.
    pub(crate) fn song_info_fading(&self) -> bool {
        self.state.credits.song_box && self.state.credits.song_alpha < 1.0
    }

    /// Periodic (`App::tick`): the Song info box stays for `SONG_HOLD` from the first tick after opening, then fades out
    /// over `SONG_FADE` and goes.
    pub(crate) fn song_info_tick(&mut self, now: Instant) {
        let c = &mut self.state.credits;
        if !c.song_box {
            return;
        }
        let since = *c.song_since.get_or_insert(now);
        let t = now.saturating_duration_since(since).as_secs_f32() - SONG_HOLD;
        if t < 0.0 {
            return;
        }
        if t >= SONG_FADE {
            self.close_song_info();
            return;
        }
        c.song_alpha = 1.0 - t / SONG_FADE;
        self.damage_rect(song_box_rect(self.state.screen));
    }
}

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

/// Writes the Info box being edited into the project. One-line boxes drop leading and trailing blanks; an emptied box
/// clears its field (a file that has the field keeps it, empty, apart from the web link; os-flp `apply`). Unchanged
/// text changes nothing.
pub(crate) fn accept_info(s: &mut State) {
    let Some(e) = s.settings.info_edit.take() else { return };
    let text = if e.field.multiline() { e.text.text } else { e.text.text.trim().to_string() };
    let before = s.project.info.clone();
    let value = e.field.get_mut(&mut s.project.info);
    if value.as_deref().unwrap_or("") != text {
        *value = (!text.is_empty()).then_some(text);
    }
    // One step, "Project info", when the text changed.
    crate::history::project_info(s, before);
}

/// Home / End / Up / Down in Comments go by its lines (between line breaks): Up and Down keep the caret's column, in
/// characters, and go to the start / end of the text from the first / last line.
fn line_key(f: &mut Field, key: NamedKey) {
    let chars: Vec<char> = f.text.chars().collect();
    let c = f.caret.min(chars.len());
    let start = |i: usize| chars[..i].iter().rposition(|&ch| ch == '\n').map_or(0, |p| p + 1);
    let end = |i: usize| chars[i..].iter().position(|&ch| ch == '\n').map_or(chars.len(), |p| i + p);
    let (s0, e0) = (start(c), end(c));
    f.caret = match key {
        NamedKey::Home => s0,
        NamedKey::End => e0,
        NamedKey::ArrowUp if s0 == 0 => 0,
        NamedKey::ArrowUp => {
            let prev = start(s0 - 1);
            prev + (c - s0).min(s0 - 1 - prev)
        }
        NamedKey::ArrowDown if e0 == chars.len() => chars.len(),
        NamedKey::ArrowDown => {
            let next = e0 + 1;
            next + (c - s0).min(end(next) - next)
        }
        _ => c,
    };
    f.all = false;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Action;
    use crate::views::settings::SAct;

    fn app() -> App {
        let mut app = App::new(None);
        app.open_settings(settings::INFO);
        app
    }

    fn typed(app: &mut App, text: &str) {
        for c in text.chars() {
            let key = if c == ' ' { Key::Named(NamedKey::Space) } else { Key::Character(c.to_string().into()) };
            app.key(&key, false, false);
        }
    }

    fn named(app: &mut App, k: NamedKey) {
        app.key(&Key::Named(k), false, false);
    }

    /// A click on a box edits it; typing, the caret keys and Ctrl+A work; Enter accepts into the project, which then has
    /// unsaved changes, and the caption shows the title (spec: ui/settings.md "Info", flp/events/project-info.md).
    #[test]
    fn title_is_typed_and_accepted() {
        let mut app = app();
        assert!(!app.unsaved());
        app.act(Action::Settings(SAct::Info(InfoField::Title)));
        assert!(app.info_editing());
        typed(&mut app, "My tune");
        named(&mut app, NamedKey::Home);
        typed(&mut app, "X");
        named(&mut app, NamedKey::Delete);
        named(&mut app, NamedKey::End);
        named(&mut app, NamedKey::Backspace);
        named(&mut app, NamedKey::ArrowLeft);
        typed(&mut app, "!");
        assert_eq!(app.state.settings.info_edit.as_ref().unwrap().text.text, "Xy tu!n");
        assert!(app.state.project.info.title.is_none(), "nothing written while typing");
        app.key(&Key::Character("a".into()), true, false);
        typed(&mut app, "  Song  ");
        named(&mut app, NamedKey::Enter);
        assert!(!app.info_editing());
        assert_eq!(app.state.project.info.title.as_deref(), Some("Song"), "trimmed");
        assert!(app.unsaved());
        assert_eq!(app.title(), "[Song]");
    }

    /// Esc drops the edit; an emptied box clears its field; an unchanged one changes nothing.
    #[test]
    fn escape_cancels_and_empty_clears() {
        let mut app = app();
        app.state.project.info.author = Some("Someone".into());
        let before = app.state.project.clone();
        app.act(Action::Settings(SAct::Info(InfoField::Author)));
        typed(&mut app, " else");
        named(&mut app, NamedKey::Escape);
        assert!(!app.info_editing() && app.state.settings.open, "Esc leaves the window open");
        assert_eq!(app.state.project.info.author.as_deref(), Some("Someone"));
        app.act(Action::Settings(SAct::Info(InfoField::Author)));
        named(&mut app, NamedKey::Enter);
        assert_eq!(app.state.project, before, "unchanged");
        app.act(Action::Settings(SAct::Info(InfoField::Author)));
        app.key(&Key::Character("a".into()), true, false);
        named(&mut app, NamedKey::Backspace);
        named(&mut app, NamedKey::Enter);
        assert_eq!(app.state.project.info.author, None);
    }

    /// Comments: Enter starts a new line, Home / End / Up / Down go by lines; clicking another box accepts it.
    #[test]
    fn comments_have_lines() {
        let mut app = app();
        app.act(Action::Settings(SAct::Info(InfoField::Comments)));
        typed(&mut app, "abc");
        named(&mut app, NamedKey::Enter);
        typed(&mut app, "de");
        named(&mut app, NamedKey::ArrowUp);
        typed(&mut app, "X");
        named(&mut app, NamedKey::End);
        typed(&mut app, "Y");
        named(&mut app, NamedKey::ArrowDown);
        named(&mut app, NamedKey::Home);
        typed(&mut app, "Z");
        assert_eq!(app.state.settings.info_edit.as_ref().unwrap().text.text, "abXcY\nZde");
        app.act(Action::Settings(SAct::Info(InfoField::Url)));
        assert_eq!(app.state.project.info.comments.as_deref(), Some("abXcY\nZde"));
        assert_eq!(app.state.settings.info_edit.as_ref().map(|e| e.field), Some(InfoField::Url));
    }

    /// Clicking a painted box edits it; a click on another tab accepts the text; long comments wrap and paint while
    /// edited.
    #[test]
    fn clicks_edit_and_accept() {
        let Ok(mut c) = os_ui::Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = app();
        let click = |app: &mut App, c: &mut os_ui::Canvas, a: Action| {
            app.paint(c, None);
            let r = app.hits.iter().rev().find(|h| h.action == a).map(|h| h.rect).unwrap_or_else(|| panic!("no hit for {a:?}"));
            app.mouse_move(r.cx(), r.cy());
            app.mouse_down();
            app.mouse_up();
        };
        click(&mut app, &mut c, Action::Settings(SAct::Info(InfoField::Comments)));
        typed(&mut app, &"word ".repeat(400));
        named(&mut app, NamedKey::Enter);
        typed(&mut app, "end");
        app.paint(&mut c, None);
        click(&mut app, &mut c, Action::Settings(SAct::Info(InfoField::Title)));
        assert!(app.state.project.info.comments.as_deref().is_some_and(|t| t.ends_with(" \nend")));
        typed(&mut app, "Clicked");
        click(&mut app, &mut c, Action::Settings(SAct::Tab(settings::DEBUG)));
        assert_eq!(app.state.settings.tab, settings::DEBUG);
        assert!(!app.info_editing());
        assert_eq!(app.state.project.info.title.as_deref(), Some("Clicked"));
    }

    /// Typed credits are saved in a new project's file, and edited again in the opened file; they read back after
    /// reopening, a cleared one as no text (spec: flp/events/project-info.md).
    #[test]
    fn credits_are_saved() {
        let mut app = app();
        let edit = |app: &mut App, f: InfoField, text: &str| {
            app.act(Action::Settings(SAct::Info(f)));
            app.key(&Key::Character("a".into()), true, false);
            named(app, NamedKey::Backspace);
            typed(app, text);
            accept_info(&mut app.state);
        };
        edit(&mut app, InfoField::Title, "Saved song");
        edit(&mut app, InfoField::Comments, "one");
        let path = std::env::temp_dir().join(format!("lemonstudio-info-{}.flp", std::process::id()));
        app.save_to(path.clone());
        assert!(!app.unsaved(), "{:?}", app.state.message);
        app.open(path.clone());
        assert_eq!((app.state.project.info.title.as_deref(), app.state.project.info.comments.as_deref()), (Some("Saved song"), Some("one")));
        // In the opened file: changed, added and cleared.
        edit(&mut app, InfoField::Title, "Renamed");
        edit(&mut app, InfoField::Url, "https://example.org");
        edit(&mut app, InfoField::Comments, "");
        app.save_to(path.clone());
        app.open(path.clone());
        let _ = std::fs::remove_file(&path);
        let i = &app.state.project.info;
        assert_eq!((i.title.as_deref(), i.url.as_deref(), i.comments.as_deref()), (Some("Renamed"), Some("https://example.org"), None));
    }

    /// While a box has the keyboard, keys do not reach shortcuts or the typing keyboard: F5 does not toggle the
    /// playlist and a letter plays no note.
    #[test]
    fn a_box_keeps_the_keyboard() {
        let mut app = app();
        if !app.typing_on() {
            app.toggle_typing();
        }
        assert!(app.typing_key(winit::keyboard::KeyCode::KeyZ, true, false, false), "Z plays a note");
        app.typing_key(winit::keyboard::KeyCode::KeyZ, false, false, false);
        app.act(Action::Settings(SAct::Info(InfoField::Genre)));
        let shown = app.state.shown(crate::app::Win::Playlist);
        named(&mut app, NamedKey::F5);
        assert_eq!(app.state.shown(crate::app::Win::Playlist), shown);
        assert!(!app.typing_key(winit::keyboard::KeyCode::KeyZ, true, false, false));
        typed(&mut app, "z");
        named(&mut app, NamedKey::Enter);
        assert_eq!(app.state.project.info.genre.as_deref(), Some("z"));
    }

    /// Picks the row labelled `label` of the open popup.
    fn pick(app: &mut App, label: &str) {
        let cmd = app.state.playlist.popup.as_ref().expect("menu").items.iter().find(|i| i.label == label).expect("row").cmd.clone();
        crate::playlist::run(app, cmd);
    }

    /// The genre menu: "(none)" and FL's 56 genres in order, the project's genre marked; a row writes the box, "(none)"
    /// empties it, and the box still takes any typed genre (spec: ui/settings-choices.md "Info").
    #[test]
    fn genre_menu_fills_the_box() {
        use crate::playlist::Mark;
        let mut app = app();
        app.state.project.info.genre = Some("House".into());
        app.act(Action::Settings(SAct::Genres(10.0, 20.0)));
        let p = app.state.playlist.popup.as_ref().expect("menu");
        assert_eq!((p.x, p.y), (10.0, 21.0));
        let labels: Vec<&str> = p.items.iter().map(|i| i.label.as_ref()).collect();
        assert_eq!(labels.len(), 57);
        assert_eq!((labels[0], labels[1], labels[13], labels[56]), ("(none)", "Acid House", "D&B", "Tropical House"));
        let marked: Vec<&str> = p.items.iter().filter(|i| i.mark == Mark::Radio(true)).map(|i| i.label.as_ref()).collect();
        assert_eq!(marked, ["House"]);
        pick(&mut app, "Lo-Fi");
        assert!(app.state.playlist.popup.is_none());
        assert_eq!(app.state.project.info.genre.as_deref(), Some("Lo-Fi"));
        app.act(Action::Settings(SAct::Genres(10.0, 20.0)));
        pick(&mut app, "(none)");
        assert_eq!(app.state.project.info.genre, None);
        // An empty genre (as FL writes it) marks "(none)", and "(none)" leaves it as it is.
        app.state.project.info.genre = Some(String::new());
        app.act(Action::Settings(SAct::Genres(10.0, 20.0)));
        assert_eq!(app.state.playlist.popup.as_ref().unwrap().items[0].mark, Mark::Radio(true));
        pick(&mut app, "(none)");
        assert_eq!(app.state.project.info.genre.as_deref(), Some(""));
        app.act(Action::Settings(SAct::Info(InfoField::Genre)));
        typed(&mut app, "Nu disco");
        named(&mut app, NamedKey::Enter);
        assert_eq!(app.state.project.info.genre.as_deref(), Some("Nu disco"));
    }

    /// The painted genre button: hint "MP3 music genres"; a click accepts the box being edited and opens the menu under
    /// the button; the picked genre is saved like a typed one (event 206; spec: flp/events/project-info.md).
    #[test]
    fn genre_button_opens_the_menu() {
        let Ok(mut c) = os_ui::Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = app();
        let click_at = |app: &mut App, r: os_ui::Rect| {
            app.mouse_move(r.cx(), r.cy());
            app.mouse_down();
            app.mouse_up();
        };
        app.paint(&mut c, None);
        let r = app.hits.iter().rev().find(|h| h.action == Action::Settings(SAct::Info(InfoField::Genre))).expect("Genre box").rect;
        click_at(&mut app, r);
        typed(&mut app, "x");
        app.paint(&mut c, None);
        let h = app.hits.iter().rev().find(|h| matches!(h.action, Action::Settings(SAct::Genres(..)))).expect("genre button");
        assert_eq!(h.hint, "MP3 music genres");
        let r = h.rect;
        click_at(&mut app, r);
        assert!(!app.info_editing());
        assert_eq!(app.state.project.info.genre.as_deref(), Some("x"));
        let p = app.state.playlist.popup.as_ref().expect("menu");
        assert_eq!((p.x, p.y), (r.x, r.bottom() + 1.0));
        pick(&mut app, "Jazz");
        assert_eq!(app.state.project.info.genre.as_deref(), Some("Jazz"));
        assert!(app.unsaved());
        let path = std::env::temp_dir().join(format!("lemonstudio-genre-{}.flp", std::process::id()));
        app.save_to(path.clone());
        app.open(path.clone());
        let _ = std::fs::remove_file(&path);
        assert_eq!(app.state.project.info.genre.as_deref(), Some("Jazz"));
    }

    /// The footer's wording (spec: ui/settings-choices.md "Info"): the start in short date and time, the time spent in
    /// hours and minutes with two spaces after the colon, "1 hour" / "1 minute" singular (Lemon Studio's choice).
    #[test]
    fn footer_words() {
        assert_eq!(date_text(42_146.675_951_990_74).as_deref(), Some("5/22/2015 4:13 PM"));
        assert_eq!(date_text(46_310.901_388_888_9).as_deref(), Some("10/15/2026 9:38 PM"));
        assert_eq!(date_text(45_974.409_722_222_2).as_deref(), Some("11/13/2025 9:50 AM"));
        assert_eq!(date_text(f64::NAN), None);
        assert_eq!(date_text(1e300), None);
        let min = WorkTime::MINUTE;
        assert_eq!(spent_text(0.0), "Less than a minute was spent working on it.");
        assert_eq!(spent_text(min * 0.99), "Less than a minute was spent working on it.");
        assert_eq!(spent_text(min), "Total time spent on it:  0 hours 1 minute.");
        assert_eq!(spent_text(min * 61.5), "Total time spent on it:  1 hour 1 minute.");
        assert_eq!(spent_text(min * 120.0), "Total time spent on it:  2 hours 0 minutes.");
        // The spec's example: 6 days 2 h 57 min, plus a minute open.
        assert_eq!(spent_text(6.0 + min * (2.0 * 60.0 + 58.0)), "Total time spent on it:  146 hours 58 minutes.");
        // A project loaded from a file names the version that saved it; a new one does not.
        let mut app = App::new(None);
        let lines = footer(&app.state, Instant::now());
        assert_eq!(lines.len(), 4);
        assert!(lines[0].starts_with("This project was started on ") && !lines[0].contains("last saved"), "{lines:?}");
        assert_eq!(lines[1], "Less than a minute was spent working on it.");
        app.state.project.saved_by = "24.1.1.4285\0".into();
        app.state.project.work_time = Some(WorkTime { started: 42_146.675_951_990_74, spent: 6.122_916_666_7 });
        let lines = footer(&app.state, Instant::now());
        assert_eq!(lines[0], "This project was started on 5/22/2015 4:13 PM, last saved in 24.1.1.4285");
        assert_eq!(lines[1], "Total time spent on it:  146 hours 57 minutes.");
        let c = counts(&app.state.project);
        assert_eq!(lines[2], format!("Plugins: {}, Channels: {}, Mixer Tracks: {}", c.plugins, c.channels, c.mixer_tracks));
        assert_eq!(lines[3], format!("Arrangements: {}, PL tracks: {}, PL clips: {}, Notes: {}", c.arrangements, c.pl_tracks, c.pl_clips, c.notes));
    }

    /// What the footer counts (spec: flp/events/project-info.md "Info counts"): channels holding a plugin plus effect
    /// slots holding one; the playlist tracks holding a clip; the notes of all patterns.
    #[test]
    fn footer_counts() {
        use os_model::{Arrangement, Clip, ClipSource, MixerTrack, Note, Pattern, SlotPlugin};
        let mut p = crate::app::default_project();
        let ch = p.channels[0].clone();
        p.channels = vec![ch.clone(), ch.clone(), ch];
        p.channels[0].plugin = Some("Lemon Sampler".into());
        p.channels[1].plugin = Some(String::new());
        p.channels[2].plugin = None;
        (p.channels[0].mixer_track, p.channels[1].mixer_track, p.channels[2].mixer_track) = (1, 1, 0);
        let slot = |name: &str| Some(SlotPlugin { name: name.into(), state: Vec::new() });
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
        let mut p = crate::app::default_project();
        let ch = p.channels[0].clone();
        // Inserts 1-4 route to the master (as FL's do); one channel straight to the master.
        p.mixer = (0..5).map(MixerTrack::new).collect();
        p.channels = vec![ch.clone()];
        p.channels[0].mixer_track = 0;
        assert_eq!(mixer_tracks(&p), 1, "a channel on the master counts the master; routes to the master do not");
        // A named track holding an effect, which nothing routes to: not counted.
        p.mixer[2].name = Some("Bus".into());
        p.mixer[2].slots[0] = Some(SlotPlugin { name: "Fruity Limiter".into(), state: Vec::new() });
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

    /// The Song info box stays 5 s from the first tick after opening, then fades out over half a second and goes; a
    /// left, right or middle press anywhere takes it away at once (Lemon Studio's choice).
    #[test]
    fn song_info_fades_and_any_click_closes() {
        let secs = Duration::from_secs_f32;
        let mut app = app();
        app.state.project.show_info = Some(true);
        opened(&mut app.state);
        let t = Instant::now() + secs(60.0);
        app.song_info_tick(t);
        app.song_info_tick(t + secs(4.9));
        assert!(app.state.credits.song_box && !app.song_info_fading(), "shown, not fading yet");
        app.song_info_tick(t + secs(5.25));
        assert!(app.song_info_fading());
        assert!((app.state.credits.song_alpha - 0.5).abs() < 0.01, "{}", app.state.credits.song_alpha);
        app.song_info_tick(t + secs(5.5));
        assert!(!app.state.credits.song_box && !app.song_info_fading(), "gone");
        // Opened again: shown again, from its first tick.
        opened(&mut app.state);
        app.song_info_tick(t + secs(6.0));
        app.song_info_tick(t + secs(10.0));
        assert!(app.state.credits.song_box && app.state.credits.song_alpha == 1.0);
        for press in [App::mouse_down, App::mouse_down_right, App::mouse_down_middle] {
            let mut app = self::app();
            app.state.project.show_info = Some(true);
            opened(&mut app.state);
            app.mouse_move(2.0, 2.0);
            press(&mut app);
            assert!(!app.state.credits.song_box, "a press elsewhere closes it");
        }
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

    /// A new project starts now with nothing spent; saving writes the time spent with this session's counting
    /// (event 237), which itself is no unsaved change; reopening goes on from the saved time.
    #[test]
    fn working_time_is_saved() {
        let mut app = app();
        let w = app.state.project.work_time.expect("a new project starts now");
        assert_eq!(w.spent, 0.0);
        assert!((w.started - os_flp::project::work_time_now().started).abs() < 1.0 / 24.0);
        app.state.project.work_time = Some(WorkTime { started: 42_146.5, spent: 1.0 });
        app.saved.0.work_time = app.state.project.work_time;
        app.state.credits.clock = WorkClock::default();
        app.state.credits.clock.focus(true, Instant::now() - Duration::from_secs(600));
        assert!(!app.unsaved(), "counting is no change");
        let path = std::env::temp_dir().join(format!("lemonstudio-worktime-{}.flp", std::process::id()));
        app.save_to(path.clone());
        assert!(!app.unsaved(), "{:?}", app.state.message);
        let bytes = std::fs::read(&path).expect("saved");
        let saved = os_flp::project::load(&os_flp::FlpFile::parse(&bytes).expect("parse")).expect("load").work_time.expect("237");
        assert_eq!(saved.started, 42_146.5);
        assert!((saved.spent - (1.0 + 10.0 * WorkTime::MINUTE)).abs() < WorkTime::MINUTE / 2.0, "{saved:?}");
        app.open(path.clone());
        let _ = std::fs::remove_file(&path);
        assert_eq!(app.state.project.work_time, Some(saved));
        assert!(spent_days(&app.state, Instant::now()) - saved.spent < WorkTime::MINUTE / 2.0, "the clock starts again");
    }

    /// Reset working time is live: the time spent goes to zero at once and the start stays; the project has unsaved
    /// changes, also when nothing was spent yet. The button sits at the page's bottom right.
    #[test]
    fn reset_working_time() {
        let mut app = app();
        app.state.project.work_time = Some(WorkTime { started: 42_146.5, spent: 0.25 });
        app.saved.0 = app.state.project.clone();
        app.state.credits.clock.focus(true, Instant::now() - Duration::from_secs(300));
        assert!(!app.unsaved());
        app.act(Action::Settings(SAct::Button(settings::RESET_WORK_TIME)));
        assert_eq!(app.state.project.work_time, Some(WorkTime { started: 42_146.5, spent: 0.0 }));
        assert!(spent_days(&app.state, Instant::now()) < WorkTime::MINUTE / 2.0);
        assert!(app.unsaved());
        assert!(footer(&app.state, Instant::now())[1].starts_with("Less than a minute"));
        // A new project (nothing spent): still a change.
        let mut app = self::app();
        assert!(!app.unsaved());
        app.reset_work_time();
        assert!(app.unsaved());
        // The painted button: live, below the footer's first lines and right-aligned with the boxes.
        let Ok(mut c) = os_ui::Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = self::app();
        app.paint(&mut c, None);
        let b = app.hits.iter().rev().find(|h| h.action == Action::Settings(SAct::Button(settings::RESET_WORK_TIME))).expect("button").rect;
        let url = app.hits.iter().rev().find(|h| h.action == Action::Settings(SAct::Info(InfoField::Url))).expect("Web link box").rect;
        assert_eq!((b.w, b.right()), (150.0, url.right()));
        assert!(b.y > url.bottom() + 16.0 * 2.0);
        assert!(!app.hits.iter().any(|h| h.rect == b && matches!(h.action, Action::Placeholder(_))), "not greyed out");
        app.state.project.work_time.as_mut().unwrap().spent = 1.0;
        app.mouse_move(b.cx(), b.cy());
        app.mouse_down();
        app.mouse_up();
        assert_eq!(app.state.project.work_time.unwrap().spent, 0.0);
    }

    /// "Show info on opening" is live and saved (event 10); opening the project then shows the Song info box, 502 x 272
    /// centred on the main window, which a click closes (spec: flp/events/project-info.md "The Song info box").
    #[test]
    fn show_info_on_opening() {
        assert_eq!(song_box_rect((1920.0, 1040.0)), Rect::new(709.0, 384.0, 502.0, 272.0));
        let mut app = app();
        assert_eq!(app.state.project.show_info, None);
        app.act(Action::Settings(SAct::Check(settings::SHOW_INFO_CHECK)));
        assert_eq!(app.state.project.show_info, Some(true));
        assert!(app.unsaved());
        assert!(!app.state.credits.song_box, "only opening shows it");
        app.state.project.info.title = Some("Bien Duro".into());
        app.state.project.info.author = Some("Astes".into());
        app.state.project.info.comments = Some("line one\nline two".into());
        let path = std::env::temp_dir().join(format!("lemonstudio-showinfo-{}.flp", std::process::id()));
        app.save_to(path.clone());
        let bytes = std::fs::read(&path).expect("saved");
        assert_eq!(os_flp::project::load(&os_flp::FlpFile::parse(&bytes).expect("parse")).expect("load").show_info, Some(true));
        app.open(path.clone());
        assert!(app.state.credits.song_box);
        if let Ok(mut c) = os_ui::Canvas::new(1920, 1040, 1.0) {
            app.state.settings.open = false;
            app.paint(&mut c, None);
            let h = app.hits.iter().rev().find(|h| h.action == Action::SongInfo).expect("Song info box");
            assert_eq!((h.rect, h.hint.as_ref()), (song_box_rect(app.state.screen), "Click to close"));
            let r = h.rect;
            app.mouse_move(r.cx(), r.cy());
            app.mouse_down();
            app.mouse_up();
            assert!(!app.state.credits.song_box, "a click closes it");
            app.paint(&mut c, None);
            assert!(!app.hits.iter().any(|h| h.action == Action::SongInfo));
        }
        // Switched off: saved as 0, and opening shows nothing.
        app.act(Action::Settings(SAct::Check(settings::SHOW_INFO_CHECK)));
        assert_eq!(app.state.project.show_info, Some(false));
        app.save_to(path.clone());
        app.open(path.clone());
        let _ = std::fs::remove_file(&path);
        assert_eq!(app.state.project.show_info, Some(false));
        assert!(!app.state.credits.song_box);
    }
}
