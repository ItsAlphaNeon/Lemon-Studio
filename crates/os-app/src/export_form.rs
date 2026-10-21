// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! FILE > Export audio (Ctrl+R) and Export MP3 (Shift+Ctrl+R): the Export form (`project_forms::Form::Export`), what
//! its parts do, and the export it starts. `views::export_form` paints it; `export` writes the files.
// spec: audio/export.md §2-3

use std::path::PathBuf;

use winit::keyboard::{Key, NamedKey};

use crate::app::App;
use crate::dialog::{spans, Answer, Body, Field, Kind, Reply, Then};
use crate::export::{self as ex, Options, Tail};
use crate::project_forms::{FHit, Form};

pub const TITLE: &str = "Export audio";
pub const MAIN: &str = "Export";

/// The segmented rows: their labels and segments.
pub const SEGS: [(&str, &[&str]); 3] = [("Play", &["Song", "Pattern"]), ("Range", &["Whole", "Selection"]), ("Tail", &ex::TAILS)];
pub const SEG_HINTS: [&str; 3] = [
    "What to render: the playlist, or one pass of the current pattern",
    "The whole song, or only the time selection",
    "The sound after the end: cut it, let it ring out, or wrap it to the start (a seamless loop)",
];
/// The checks, by `FHit::Check` number.
pub const CHECKS: [&str; 12] = [
    "Dither",
    "HQ for plugins",
    "Split mixer tracks",
    "Include the master",
    "Insert effects",
    "Master effects",
    "Playlist markers",
    "Loop points",
    "Note markers",
    "Tempo",
    "Open the folder when done",
    "Gapless",
];
pub const CHECK_HINTS: [&str; 12] = [
    "Triangular dither on 16 and 24-bit files",
    "Hosted plugins render in their high quality (offline) mode",
    "Also write every mixer track alone (WAV and FLAC), into a stems folder",
    "Write the master's own file with the split tracks",
    "Run the effects on the insert tracks",
    "Run the effects on the master",
    "Write the markers into WAV files",
    "Write the loop (the time selection, else the whole file) into WAV files",
    "Write each note's start as a marker into WAV files (Pattern)",
    "Write the tempo into WAV files",
    "Show the file in the file manager when the export is done",
    "An Info frame first, so players that read it skip the encoder's delay (DAWs such as FL still play it as silence)",
];
pub const TILE_HINTS: [&str; 4] =
    ["WAV: uncompressed audio", "FLAC: lossless compressed audio", "MP3: lossy compressed audio", "OGG: Ogg Vorbis lossy compressed audio"];
/// The combos' hints, by `FHit::Combo` number (`ExportForm::list_items`).
pub const COMBO_HINTS: [&str; 8] = [
    "WAV bit depth",
    "FLAC bit depth",
    "FLAC compression level (higher: smaller files, slower)",
    "MP3 bitrate",
    "OGG bitrate",
    "The files' sample rate (the project renders at the device's)",
    "Stereo, or one channel",
    "Interpolation for sampler channels and sample rate conversion (wider: cleaner, slower)",
];

/// The Export form.
#[derive(Debug, Clone, PartialEq)]
pub struct ExportForm {
    pub name: Field,
    pub folder: PathBuf,
    /// Play: the playlist (true) or the current pattern.
    pub song: bool,
    /// Whether the playlist (has clips) and the current pattern (has notes) have anything to render; Play cannot pick
    /// one without.
    pub song_ok: bool,
    pub pat_ok: bool,
    /// Range: the time selection, while there is one for what plays.
    pub selection: bool,
    /// The time selections (start, end ticks): the playlist's and the piano roll's.
    pub song_sel: Option<(u32, u32)>,
    pub pat_sel: Option<(u32, u32)>,
    /// The lengths in seconds of the song and of the pattern, without the tail (for the summary).
    pub song_secs: f64,
    pub pat_secs: f64,
    pub secs_per_tick: f64,
    pub o: Options,
    /// The device's rate (the render's).
    pub device_rate: u32,
    /// The combo whose list is open.
    pub list: Option<u8>,
}

impl ExportForm {
    /// The span chosen, when Range is Selection and there is one.
    pub fn span(&self) -> Option<(u32, u32)> {
        if !self.selection {
            return None;
        }
        if self.song { self.song_sel } else { self.pat_sel }
    }

    /// Whether Range's Selection can be chosen.
    pub fn has_selection(&self) -> bool {
        if self.song { self.song_sel.is_some() } else { self.pat_sel.is_some() }
    }

    /// The segment chosen in segmented row `row`.
    pub fn seg(&self, row: u8) -> usize {
        match row {
            0 => usize::from(!self.song),
            1 => usize::from(self.span().is_some()),
            _ => self.o.tail as usize,
        }
    }

    /// Check `k`: whether it is ticked, and whether it can be changed now.
    pub fn check(&self, k: u8) -> (bool, bool) {
        let o = &self.o;
        let wav = o.formats[ex::WAV];
        match k {
            0 => (o.dither, o.ticked().any(|f| o.integer(f))),
            1 => (o.hq, true),
            2 => (o.split, o.formats[ex::WAV] || o.formats[ex::FLAC]),
            3 => (o.split_master, o.split),
            4 => (o.insert_fx, true),
            5 => (o.master_fx, true),
            6 => (o.markers, wav),
            7 => (o.loops, wav),
            8 => (o.note_markers, wav && !self.song),
            9 => (o.tempo, wav),
            10 => (o.reveal, true),
            _ => (o.mp3_gapless, o.formats[ex::MP3]),
        }
    }

    fn toggle(&mut self, k: u8) {
        let o = &mut self.o;
        let f = match k {
            0 => &mut o.dither,
            1 => &mut o.hq,
            2 => &mut o.split,
            3 => &mut o.split_master,
            4 => &mut o.insert_fx,
            5 => &mut o.master_fx,
            6 => &mut o.markers,
            7 => &mut o.loops,
            8 => &mut o.note_markers,
            9 => &mut o.tempo,
            10 => &mut o.reveal,
            _ => &mut o.mp3_gapless,
        };
        *f = !*f;
    }

    /// Combo `k`'s list: its entries and the chosen one.
    pub fn list_items(&self, k: u8) -> (Vec<String>, usize) {
        let o = &self.o;
        let kbps = |v: u32| (ex::BITRATES.iter().map(|b| format!("{b} kbps")).collect(), ex::BITRATES.iter().position(|&b| b == v).unwrap_or(4));
        match k {
            0 => (ex::WAV_DEPTHS.iter().map(|s| s.to_string()).collect(), o.wav_depth),
            1 => (ex::FLAC_DEPTHS.iter().map(|s| s.to_string()).collect(), o.flac_depth),
            2 => ((0..=8).map(|l| format!("Level {l}")).collect(), usize::from(o.flac_level)),
            3 => kbps(o.mp3_kbps),
            4 => kbps(o.ogg_kbps),
            5 => {
                let mut v = vec![format!("{} Hz (device)", self.device_rate)];
                v.extend(ex::RATES.iter().map(|r| format!("{r} Hz")));
                (v, ex::RATES.iter().position(|&r| r == o.rate).map_or(0, |i| i + 1))
            }
            6 => (ex::CHANNELS.iter().map(|s| s.to_string()).collect(), o.channels),
            _ => (
                ex::POINTS.iter().map(|&p| if p == 2 { "2-point linear".into() } else if p == 27 { "27-point sinc (playback)".into() } else { format!("{p}-point sinc") }).collect(),
                ex::POINTS.iter().position(|&p| p == o.points).unwrap_or(3),
            ),
        }
    }

    /// Combo `k`'s text.
    pub fn combo_text(&self, k: u8) -> String {
        let (items, i) = self.list_items(k);
        let text = items.get(i).cloned().unwrap_or_default();
        if k == 5 && i == 0 { format!("{} Hz", self.device_rate) } else { text }
    }

    fn pick(&mut self, k: u8, i: usize) {
        let o = &mut self.o;
        match k {
            0 => o.wav_depth = i.min(ex::WAV_DEPTHS.len() - 1),
            1 => o.flac_depth = i.min(ex::FLAC_DEPTHS.len() - 1),
            2 => o.flac_level = i.min(8) as u8,
            3 => o.mp3_kbps = ex::BITRATES.get(i).copied().unwrap_or(o.mp3_kbps),
            4 => o.ogg_kbps = ex::BITRATES.get(i).copied().unwrap_or(o.ogg_kbps),
            5 => o.rate = if i == 0 { 0 } else { ex::RATES.get(i - 1).copied().unwrap_or(0) },
            6 => o.channels = i.min(ex::CHANNELS.len() - 1),
            _ => o.points = ex::POINTS.get(i).copied().unwrap_or(o.points),
        }
    }

    /// The format whose combo `k` is (greyed while the format is off); None: always on.
    pub fn combo_format(k: u8) -> Option<usize> {
        match k {
            0 => Some(ex::WAV),
            1 | 2 => Some(ex::FLAC),
            3 => Some(ex::MP3),
            4 => Some(ex::OGG),
            _ => None,
        }
    }

    /// The length rendered in seconds (without the tail) and the files' size estimate in bytes.
    pub fn summary(&self) -> (f64, f64) {
        let secs = match self.span() {
            Some((a, b)) => f64::from(b.saturating_sub(a)) * self.secs_per_tick,
            None if self.song => self.song_secs,
            None => self.pat_secs,
        };
        let rate = if self.o.rate == 0 { self.device_rate } else { self.o.rate };
        let bytes: f64 = self.o.ticked().map(|f| self.o.bytes_per_second(f, rate) * secs).sum();
        (secs, bytes)
    }

    /// The summary line: "1:23 long, about 12.3 MB".
    pub fn summary_text(&self) -> String {
        let (secs, bytes) = self.summary();
        let mb = bytes / 1_048_576.0;
        let size = if mb >= 10.0 { format!("{mb:.0} MB") } else if mb >= 0.1 { format!("{mb:.1} MB") } else { format!("{:.0} KB", bytes / 1024.0) };
        format!("{}:{:02} long, about {size}", (secs / 60.0) as u32, (secs % 60.0) as u32)
    }

    /// The name as typed, trimmed; what is wrong with it.
    fn problem(&self) -> Option<String> {
        let n = self.name.text.trim();
        if n.is_empty() {
            return Some("Type a name for the file.".into());
        }
        if n.chars().any(|c| r#"<>:"/\|?*"#.contains(c)) {
            return Some(r#"A file name cannot contain any of these: < > : " / \ | ? *"#.into());
        }
        None
    }
}

/// The top dialog's Export form.
fn top(app: &mut App) -> Option<&mut ExportForm> {
    match &mut app.state.dialogs.last_mut()?.body {
        Body::Project(Form::Export(f)) => Some(f),
        _ => None,
    }
}

impl App {
    /// FILE > Export audio (Ctrl+R; `mp3`: Export MP3, Shift+Ctrl+R, with MP3 the only format): the Export form with
    /// the last choices, unless there is nothing to render (spec §2).
    // spec: audio/export.md §2
    pub(crate) fn open_export(&mut self, mp3: bool) {
        if self.export.is_some() {
            return;
        }
        let s = &self.state;
        let p = crate::rack::play_project(s);
        let song = s.song_mode;
        // Play starts on the app's mode, else on whichever of the two has something to render (as FL: PAT with an
        // empty pattern still exports the playlist). Only with neither is it refused, in a warning box: a hint-bar
        // message is lost under the menu's own hint at once.
        let song_ok = p.arrangement().is_some_and(|a| !a.clips.is_empty());
        let pat_ok = p.patterns.iter().find(|q| q.number == s.pattern).is_some_and(|q| !q.notes.is_empty());
        if !song_ok && !pat_ok {
            let text = format!("Nothing to export: the playlist is empty and {} has no notes.", crate::patterns::name_of(s, s.pattern));
            return self.tell(Kind::Warning, TITLE, spans(&[&text]));
        }
        let song = if song { song_ok } else { !pat_ok };
        let mut o = Options::load();
        if mp3 {
            o.formats = [false, false, true, false];
        }
        let name = s.path.as_ref().and_then(|p| p.file_stem()).map_or_else(|| "Untitled".to_string(), |n| n.to_string_lossy().into_owned());
        let folder = match crate::settings_file::get("export_folder").map(PathBuf::from).filter(|d| d.is_dir()) {
            // The project's folder first, else the last one used.
            _ if s.path.as_ref().and_then(|p| p.parent()).is_some_and(|d| d.is_dir()) => s.path.as_ref().and_then(|p| p.parent()).unwrap().to_path_buf(),
            Some(d) => d,
            None => crate::save_form::default_root().unwrap_or_else(std::env::temp_dir),
        };
        let device_rate = crate::transport::Transport::device_rate().unwrap_or(44100);
        let tempo = s.project.tick_tempo(s.tempo);
        let secs_per_tick = 60.0 / tempo.max(1.0) / f64::from(s.project.ppq.max(1));
        let song_end = crate::playlist::clips(s).iter().map(|c| c.end()).max().unwrap_or(0);
        let pat_len = crate::piano::pattern_length(s, s.pattern);
        let sel = |r: Option<(u32, u32)>| r.filter(|(a, b)| b > a);
        let mut name_field = Field::default();
        name_field.set(&name);
        name_field.select_all();
        let f = ExportForm {
            name: name_field,
            folder,
            song,
            song_ok,
            pat_ok,
            selection: false,
            song_sel: sel(s.playlist.range),
            pat_sel: sel(s.piano.range),
            song_secs: f64::from(song_end) * secs_per_tick,
            pat_secs: f64::from(pat_len) * secs_per_tick,
            secs_per_tick,
            o,
            device_rate,
            list: None,
        };
        self.show_form(Body::Project(Form::Export(Box::new(f))));
    }

    /// A click on a part of the Export form; `Some` closes it with that answer. With a list open, a click picks from it
    /// or only closes it.
    pub(crate) fn export_form_click(&mut self, h: FHit) -> Option<Reply> {
        let f = top(self)?;
        if let Some(k) = f.list.take() {
            if let FHit::Item(i) = h {
                f.pick(k, usize::from(i));
            }
            self.invalidate();
            return None;
        }
        match h {
            FHit::Seg(0, i) if (if i == 0 { f.song_ok } else { f.pat_ok }) => {
                f.song = i == 0;
                f.selection &= f.has_selection();
            }
            FHit::Seg(1, i) => f.selection = i == 1 && f.has_selection(),
            FHit::Seg(_, i) => f.o.tail = [Tail::Cut, Tail::Ring, Tail::Wrap][usize::from(i).min(2)],
            // A format toggles; the last one ticked stays.
            FHit::Tile(t) => {
                let t = usize::from(t).min(3);
                if !f.o.formats[t] || f.o.ticked().count() > 1 {
                    f.o.formats[t] = !f.o.formats[t];
                }
            }
            FHit::Check(k) if f.check(k).1 => f.toggle(k),
            FHit::Combo(k) if ExportForm::combo_format(k).is_none_or(|t| f.o.formats[t]) => f.list = Some(k),
            FHit::Folder => self.export_folder_pick(),
            FHit::Main => return self.export_form_accept(),
            _ => {}
        }
        self.invalidate();
        None
    }

    fn export_form_accept(&mut self) -> Option<Reply> {
        let f = top(self)?;
        match f.problem() {
            None => Some(Reply::Button(Answer::Ok)),
            Some(p) => {
                self.tell(Kind::Warning, TITLE, spans(&[&p]));
                None
            }
        }
    }

    /// Keys on the Export form: Esc closes an open list, then the form; Enter exports; the rest types in the name.
    pub(crate) fn export_form_key(&mut self, key: &Key, ctrl: bool, alt: bool) -> Option<Reply> {
        let f = top(self)?;
        if f.list.is_some() {
            if matches!(key, Key::Named(NamedKey::Escape)) {
                f.list = None;
                self.invalidate();
            }
            return None;
        }
        match key {
            Key::Named(NamedKey::Escape) => return Some(Reply::Button(Answer::Cancel)),
            Key::Named(NamedKey::Enter) => return self.export_form_accept(),
            _ => {
                f.name.key(key, ctrl, alt);
            }
        }
        self.invalidate();
        None
    }

    /// The folder button: a folder picker, starting in the form's folder.
    fn export_folder_pick(&mut self) {
        use crate::filedlg::{FileDlg, Mode};
        let start = top(self).map(|f| f.folder.clone()).filter(|d| d.is_dir());
        let dlg = FileDlg::new(Mode::Folder, "Export to", "export", Then::ExportFolder).start(start.as_deref());
        self.file_dialog(dlg);
    }

    /// The picker chose `dir`: the files go there.
    pub(crate) fn export_folder_chosen(&mut self, dir: PathBuf) {
        if let Some(f) = top(self) {
            f.folder = dir;
        }
        self.invalidate();
    }

    /// The form closed: Export keeps the choices and starts the render.
    pub(crate) fn export_form_closed(&mut self, f: ExportForm, reply: Reply) {
        if reply != Reply::Button(Answer::Ok) {
            return;
        }
        f.o.save();
        crate::settings_file::write("export_folder", &f.folder.to_string_lossy());
        let job = self.export_job(&f);
        self.start_export(job, f.song);
    }

    /// What the audio thread needs for form `f`: the span, the markers, loop and tempo at the device's rate, the
    /// mixer tracks' names.
    fn export_job(&self, f: &ExportForm) -> ex::Job {
        let s = &self.state;
        let o = f.o.clone();
        let rate = f.device_rate;
        let tempo = s.project.tick_tempo(s.tempo);
        let ppq = s.project.ppq;
        let start = f.span().map_or(0, |(a, _)| a);
        let end = f.span().map(|(_, b)| b);
        let frame = |tick: u32| os_engine::laws::tick_to_frame(f64::from(tick.saturating_sub(start)), ppq, tempo, rate) as u64;
        let inside = |tick: u32| tick >= start && end.is_none_or(|b| tick < b);
        let mut markers = Vec::new();
        if o.markers {
            let list = if f.song { s.project.arrangement().map(|a| a.markers.clone()) } else { s.project.pattern(s.pattern).map(|p| p.markers.clone()) };
            for m in list.unwrap_or_default().iter().filter(|m| m.kind == 0 && inside(m.position)) {
                markers.push(ex::Marker { frame: frame(m.position), name: m.name.clone() });
            }
        }
        if o.note_markers && !f.song {
            let mut starts: Vec<u32> = s.project.pattern(s.pattern).map(|p| p.notes.iter().map(|n| n.position).filter(|&t| inside(t)).collect()).unwrap_or_default();
            starts.sort_unstable();
            starts.dedup();
            markers.extend(starts.into_iter().map(|t| ex::Marker { frame: frame(t), name: String::new() }));
        }
        markers.sort_by_key(|m| m.frame);
        // Loop points: the time selection when the whole song renders, else the file.
        let loop_frames = if f.span().is_none() {
            let sel = if f.song { f.song_sel } else { f.pat_sel };
            sel.map(|(a, b)| (frame(a), frame(b)))
        } else {
            None
        };
        let ticks = match f.span() {
            Some((a, b)) => b - a,
            None if f.song => crate::playlist::clips(s).iter().map(|c| c.end()).max().unwrap_or(0),
            None => crate::piano::pattern_length(s, s.pattern),
        };
        let tracks = s.project.mixer.iter().map(|m| m.index).max().unwrap_or(0);
        ex::Job {
            options: o,
            folder: f.folder.clone(),
            name: f.name.text.trim().to_string(),
            span: f.span().map(|(a, b)| (u64::from(a), u64::from(b))),
            markers,
            loop_frames,
            tempo: (tempo, f64::from(ticks) / f64::from(ppq.max(1))),
            track_names: (0..=tracks).map(|t| crate::mixer::name(s, t)).collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dialog::DHit;
    use crate::views::menus::{FILE_EXPORT, FILE_EXPORT_MP3};
    use os_model::Note;

    fn form(app: &App) -> Option<&ExportForm> {
        match &app.state.dialogs.last()?.body {
            Body::Project(Form::Export(f)) => Some(f),
            _ => None,
        }
    }

    fn click(app: &mut App, h: FHit) {
        app.dialog_click(DHit::Form(h));
    }

    /// A pattern with notes, played in PAT mode.
    fn app() -> App {
        let mut app = App::new(None);
        app.state.project = crate::app::test_project();
        app.state.project.patterns[0].notes.push(Note::step(0, 0));
        app.state.song_mode = false;
        app
    }

    /// Nothing to render: no form, a message. Export MP3 opens with MP3 the only format; Export audio with the last
    /// choices (spec §2).
    #[test]
    fn opens_with_the_last_choices() {
        let mut empty = App::new(None);
        empty.state.project = crate::app::test_project();
        empty.state.project.patterns[0].notes.clear();
        empty.state.song_mode = false;
        empty.act(crate::app::Action::MenuItem(0, FILE_EXPORT));
        assert!(form(&empty).is_none());
        assert_eq!(empty.state.dialogs.len(), 1, "a warning says why");
        // PAT with an empty pattern still exports the playlist (and SONG with an empty playlist the pattern).
        empty.state.dialogs.clear();
        crate::playlist::arr_mut(&mut empty.state).clips.push(os_model::Clip::new(0, 96, 0, os_model::ClipSource::Pattern(1)));
        empty.act(crate::app::Action::MenuItem(0, FILE_EXPORT));
        let f = form(&empty).expect("the Export form");
        assert!(f.song && f.song_ok && !f.pat_ok, "Play starts on Song");
        click(&mut empty, FHit::Seg(0, 1));
        assert!(form(&empty).unwrap().song, "Pattern has nothing to render");

        let mut app = app();
        app.act(crate::app::Action::MenuItem(0, FILE_EXPORT_MP3));
        let f = form(&app).expect("the Export form");
        assert_eq!(f.o.formats, [false, false, true, false]);
        assert!(!f.song && f.name.text == "Untitled");
        app.dialog_key(&Key::Named(NamedKey::Escape), false, false);
        assert!(form(&app).is_none(), "Esc cancels");

        app.act(crate::app::Action::MenuItem(0, FILE_EXPORT));
        click(&mut app, FHit::Tile(ex::FLAC as u8));
        click(&mut app, FHit::Main);
        // The render started (and keeps going in the background); the next form remembers FLAC.
        while app.export.is_some() {
            app.plugin_tick();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        app.act(crate::app::Action::MenuItem(0, FILE_EXPORT));
        assert_eq!(form(&app).unwrap().o.formats, [true, true, false, false], "WAV (the default) and FLAC");
        app.dialog_key(&Key::Named(NamedKey::Escape), false, false);
        for e in ["wav", "flac"] {
            let _ = std::fs::remove_file(form_folder().join(format!("Untitled.{e}")));
        }
    }

    fn form_folder() -> PathBuf {
        crate::settings_file::get("export_folder").map(PathBuf::from).unwrap_or_default()
    }

    /// Segments, tiles, checks and combos (spec §3): Selection only with a selection; the last format stays ticked;
    /// greyed checks and combos do nothing; a list picks.
    #[test]
    fn the_parts_change_the_choices() {
        let mut app = app();
        app.state.piano.range = Some((0, 96));
        crate::playlist::arr_mut(&mut app.state).clips.push(os_model::Clip::new(0, 96, 0, os_model::ClipSource::Pattern(1)));
        app.act(crate::app::Action::MenuItem(0, FILE_EXPORT_MP3));
        click(&mut app, FHit::Seg(1, 1));
        assert_eq!(form(&app).unwrap().span(), Some((0, 96)), "the piano roll's selection");
        click(&mut app, FHit::Seg(0, 0));
        assert_eq!(form(&app).unwrap().span(), None, "the playlist has no selection");
        click(&mut app, FHit::Seg(2, 2));
        assert_eq!(form(&app).unwrap().o.tail, Tail::Wrap);

        click(&mut app, FHit::Tile(ex::MP3 as u8));
        assert_eq!(form(&app).unwrap().o.formats, [false, false, true, false], "the last format stays");
        // MP3 only: Dither (no integer format) and Split (WAV and FLAC only) are greyed.
        let dither = form(&app).unwrap().o.dither;
        click(&mut app, FHit::Check(0));
        click(&mut app, FHit::Check(2));
        assert_eq!((form(&app).unwrap().o.dither, form(&app).unwrap().o.split), (dither, false));
        click(&mut app, FHit::Combo(0));
        assert_eq!(form(&app).unwrap().list, None, "WAV's depth is greyed");
        click(&mut app, FHit::Tile(ex::WAV as u8));
        click(&mut app, FHit::Check(2));
        assert!(form(&app).unwrap().o.split);

        click(&mut app, FHit::Combo(3));
        assert_eq!(form(&app).unwrap().list, Some(3));
        click(&mut app, FHit::Item(ex::BITRATES.len() as u8 - 1));
        assert_eq!((form(&app).unwrap().list, form(&app).unwrap().o.mp3_kbps), (None, 320));
        click(&mut app, FHit::Combo(5));
        click(&mut app, FHit::Item(2));
        assert_eq!(form(&app).unwrap().o.rate, 48000);
        click(&mut app, FHit::Combo(7));
        app.dialog_key(&Key::Named(NamedKey::Escape), false, false);
        assert!(form(&app).is_some_and(|f| f.list.is_none()), "Esc closes the list first");
    }

    /// A bad name warns and keeps the form; the summary tells the length and size.
    #[test]
    fn names_and_summary() {
        let mut app = app();
        app.act(crate::app::Action::MenuItem(0, FILE_EXPORT));
        app.dialog_key(&Key::Character("a?b".into()), false, false);
        click(&mut app, FHit::Main);
        assert!(app.state.dialogs.len() >= 2, "a warning over the form");
        let f = form_below(&app);
        assert_eq!(f.name.text, "a?b");
        let mut g = f.clone();
        g.o.formats = [true, false, false, false];
        g.o.wav_depth = 0;
        g.o.channels = 0;
        g.o.rate = 44100;
        g.selection = false;
        g.song = false;
        g.pat_secs = 60.0;
        // 16-bit stereo 44.1 kHz: 176400 bytes a second.
        assert_eq!(g.summary_text(), "1:00 long, about 10 MB");
    }

    fn form_below(app: &App) -> &ExportForm {
        app.state.dialogs.iter().rev().find_map(|d| match &d.body {
            Body::Project(Form::Export(f)) => Some(f.as_ref()),
            _ => None,
        })
        .expect("the Export form")
    }
}
