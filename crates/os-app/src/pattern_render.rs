// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! PATTERNS > Quick render as audio clip, Render as audio clip... and Render and replace (spec: ui/pattern-render.md):
//! the current pattern rendered as the Export form renders a pattern, into a WAV in the data folder's Rendered folder,
//! then a new audio clip channel for it, named after the pattern; Render as audio clip... shows the Export form first;
//! Render and replace also puts a clip of it over every clip of the pattern in the playlist and mutes those.
// spec: ui/pattern-render.md

use std::path::{Path, PathBuf};

use os_model::{Channel, ChannelKind, Clip, ClipSource};

use crate::app::{App, State};
use crate::export::{self as ex, Options, Tail};
use crate::playlist::PickKind;

/// The folder the renders go to, in the data folder (§2).
pub const FOLDER: &str = "Rendered";
/// The warning box's title when there is nothing to render.
pub const TITLE: &str = "Render as audio clip";
/// The undo steps (§2, §3).
pub const QUICK: &str = "Quick render as audio clip";
pub const REPLACE: &str = "Render and replace";

/// A render under way: what to do with the file once it is written.
#[derive(Debug, Clone, PartialEq)]
pub struct Pending {
    pub pattern: u16,
    /// Render and replace: the pattern's clips get the audio.
    pub replace: bool,
}

/// Where the renders go (§2): the data folder's Rendered folder; a project without a data folder uses one-click
/// recording's fallback (the user folder, else the system's temporary folder).
pub fn folder(s: &State) -> PathBuf {
    crate::data_folder::folder(s).or_else(crate::app::theme_dir).unwrap_or_else(std::env::temp_dir).join(FOLDER)
}

/// The file name (without extension) for pattern `name` in `dir` (§2): the name with the characters a file name
/// cannot hold as `_`, `Pattern` for an empty one; ` #2`, ` #3`... when the WAV is already there (a render never
/// overwrites a file a channel may be playing).
pub fn file_stem(dir: &Path, name: &str) -> String {
    let safe: String = name.chars().map(|c| if c.is_control() || r#"<>:"/\|?*"#.contains(c) { '_' } else { c }).collect();
    let safe = safe.trim().trim_end_matches('.').trim_end().to_string();
    let safe = if safe.is_empty() { "Pattern".to_string() } else { safe };
    let free = |stem: &str| !dir.join(format!("{stem}.wav")).exists();
    if free(&safe) {
        return safe;
    }
    (2..).map(|k| format!("{safe} #{k}")).find(|n| free(n)).unwrap_or(safe)
}

/// The export of the current pattern (§2): the Export form's defaults for one pass of a pattern (a 32-bit float WAV
/// at the render's rate, the tail ringing out, high quality plugins, insert effects) without the master's effects,
/// which the new channel plays through, and without markers or loop points.
pub fn job(s: &State, dir: &Path, stem: &str) -> ex::Job {
    let options = Options {
        formats: [true, false, false, false],
        wav_depth: 2,
        rate: 0,
        channels: 0,
        tail: Tail::Ring,
        split: false,
        master_fx: false,
        markers: false,
        loops: false,
        note_markers: false,
        tempo: false,
        reveal: false,
        ..Options::default()
    };
    let tempo = s.project.tick_tempo(s.tempo);
    let ticks = crate::piano::pattern_length(s, s.pattern);
    ex::Job {
        options,
        folder: dir.to_path_buf(),
        name: stem.to_string(),
        span: None,
        markers: Vec::new(),
        loop_frames: None,
        tempo: (tempo, f64::from(ticks) / f64::from(s.project.ppq.max(1))),
        track_names: Vec::new(),
    }
}

/// The new channel for the file at `path` (§2): an audio clip channel at the end of the rack, as ADD > Audio clip
/// makes one (on the master, the only selected channel, in the rack's "Audio" group, the playlist's clip source)
/// with the file as its sample, named after pattern `pattern` (made unique) and in its colour. No clip is placed.
/// The caller records the undo step. Returns the channel's index.
pub fn add_channel(s: &mut State, path: &str, pattern: u16) -> u16 {
    let index = s.project.channels.iter().map(|c| c.index + 1).max().unwrap_or(0);
    let name = crate::playlist::unique_channel_name(s, &crate::patterns::name_of(s, pattern));
    let color = s.project.pattern(pattern).and_then(|p| p.color).or_else(|| Some(s.clip_colors.new_channel(index)));
    s.project.channels.push(Channel {
        index,
        kind: ChannelKind::AudioClip,
        name: Some(name),
        plugin: None,
        plugin_state: None,
        color,
        enabled: true,
        volume: 0.78125,
        pan: 0.0,
        pitch_cents: 0,
        mixer_track: 0,
        sample_path: Some(path.to_string()),
        layer_children: Vec::new(),
        envelopes: Default::default(), rack: Default::default(), misc: Default::default(),
        sound: os_model::ChannelSound { sampler_flags: Some(os_model::ChannelSound::AUDIO_CLIP_FLAGS), ..Default::default() },
    });
    let row = s.project.channels.len() - 1;
    s.selected_channel = row;
    s.rack.select_only(row);
    crate::rack::join_group(s, row, "Audio");
    s.playlist.source = Some(ClipSource::Channel(index));
    s.playlist.picker = PickKind::Audio;
    s.piano.dirty = true;
    index
}

/// Render and replace (§3): every clip of pattern `pattern` in the current arrangement gets a clip of channel `ch`
/// just above it in drawing order, at the same position and track, as long, cut where the pattern clip was cut (its
/// window converted to the audio's units) and muted if it was; the pattern clips are muted, not removed. The new
/// clips are selected. Returns how many there were.
pub fn replace_clips(s: &mut State, pattern: u16, ch: u16) -> usize {
    let src = ClipSource::Channel(ch);
    let found: Vec<Clip> = crate::playlist::clips(s).iter().filter(|c| c.source == ClipSource::Pattern(pattern)).copied().collect();
    if found.is_empty() {
        return 0;
    }
    let windows: Vec<Option<(f64, f64)>> =
        found.iter().map(|c| c.window.map(|(a, b)| (crate::playlist::to_src(s, src, a), crate::playlist::to_src(s, src, b)))).collect();
    let mut windows = windows.into_iter();
    let a = crate::playlist::arr_mut(s);
    let mut clips = Vec::with_capacity(a.clips.len() + found.len());
    let mut sel = Vec::with_capacity(found.len());
    for mut c in std::mem::take(&mut a.clips) {
        if c.source != ClipSource::Pattern(pattern) {
            clips.push(c);
            continue;
        }
        let mut audio = Clip::new(c.position, c.length, c.track, src);
        audio.window = windows.next().flatten();
        audio.muted = c.muted;
        c.muted = true;
        clips.push(c);
        sel.push(clips.len());
        clips.push(audio);
    }
    a.clips = clips;
    s.playlist.sel = sel;
    s.piano.dirty = true;
    found.len()
}

/// What a finished render does with the file at `path` (§2, §3): the channel (and for Render and replace the clips) in
/// one undo step. Returns the hint bar's message.
pub fn place(s: &mut State, p: &Pending, path: &Path) -> String {
    let text = path.to_string_lossy().into_owned();
    if s.playlist.wave(&text).is_none() {
        return format!("{text} could not be read");
    }
    let before = crate::history::Snap::Rack(Box::new(crate::history::RackSide::now(s, s.project.channels.len())));
    crate::playlist::snapshot_with(s, if p.replace { REPLACE } else { QUICK }, before);
    let ch = add_channel(s, &text, p.pattern);
    let file = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if !p.replace {
        return format!("Rendered {} to {file}", crate::patterns::name_of(s, p.pattern));
    }
    match replace_clips(s, p.pattern, ch) {
        0 => format!("Rendered {} to {file} (no clips of it in the playlist to replace)", crate::patterns::name_of(s, p.pattern)),
        n => format!("Rendered {} to {file}: replaced {n} clip(s)", crate::patterns::name_of(s, p.pattern)),
    }
}

impl App {
    /// PATTERNS > Quick render as audio clip (`replace` false) and Render and replace (§2, §3): renders the current
    /// pattern as an export does, with its progress panel; `pattern_render_done` takes the file. A pattern without
    /// notes is refused in a warning box, as the Export form refuses one.
    pub(crate) fn render_pattern(&mut self, replace: bool) {
        if self.export.is_some() {
            return;
        }
        let s = &self.state;
        let pattern = s.pattern.max(1);
        if !s.project.pattern(pattern).is_some_and(|p| !p.notes.is_empty()) {
            let text = format!("Nothing to render: {} has no notes.", crate::patterns::name_of(s, pattern));
            return self.tell(crate::dialog::Kind::Warning, TITLE, crate::dialog::spans(&[&text]));
        }
        let dir = folder(s);
        if let Err(e) = std::fs::create_dir_all(&dir) {
            self.state.message = Some(format!("Render failed: {}: {e}", dir.display()));
            return self.invalidate();
        }
        let stem = file_stem(&dir, &crate::patterns::name_of(s, pattern));
        let job = job(s, &dir, &stem);
        self.pattern_render = Some(Pending { pattern, replace });
        self.start_export(job, false);
    }

    /// PATTERNS > Render as audio clip... (§1): the Export form in its render mode: Play fixed on the current
    /// pattern, the Rendered folder, the pattern's file name and the quick render's choices, changeable before the
    /// render; Export renders and makes the channel as Quick render does.
    pub(crate) fn open_render_form(&mut self) {
        if self.export.is_some() {
            return;
        }
        let s = &self.state;
        let pattern = s.pattern.max(1);
        if !s.project.pattern(pattern).is_some_and(|p| !p.notes.is_empty()) {
            let text = format!("Nothing to render: {} has no notes.", crate::patterns::name_of(s, pattern));
            return self.tell(crate::dialog::Kind::Warning, TITLE, crate::dialog::spans(&[&text]));
        }
        let dir = folder(s);
        let stem = file_stem(&dir, &crate::patterns::name_of(s, pattern));
        let o = job(s, &dir, &stem).options;
        self.open_export(false);
        let Some(crate::dialog::Modal { body: crate::dialog::Body::Project(crate::project_forms::Form::Export(f)), .. }) = self.state.dialogs.last_mut() else {
            return;
        };
        (f.song, f.song_ok, f.selection, f.to_clip) = (false, false, false, true);
        f.folder = dir;
        f.o = o;
        f.name.set(&stem);
        f.name.select_all();
    }

    /// The render mode's Export (§1): the form's choices for the current pattern into its folder, then the channel.
    pub(crate) fn render_pattern_with(&mut self, f: &crate::export_form::ExportForm) {
        if let Err(e) = std::fs::create_dir_all(&f.folder) {
            self.state.message = Some(format!("Render failed: {}: {e}", f.folder.display()));
            return self.invalidate();
        }
        let job = self.export_job(f);
        self.pattern_render = Some(Pending { pattern: self.state.pattern.max(1), replace: false });
        self.start_export(job, false);
    }

    /// A render has finished (`App::export_tick`): the channel and clips for its file, or what went wrong.
    pub(crate) fn pattern_render_done(&mut self, p: Pending, r: Result<(f64, Vec<PathBuf>), String>) {
        self.state.message = Some(match r {
            Ok((_, files)) => match files.first() {
                Some(f) => {
                    let m = place(&mut self.state, &p, f);
                    let s = &mut self.state;
                    let view_h = crate::views::rack::panel(crate::views::rack::rect(s)).h;
                    s.rack_scroll = (crate::views::rack::content_h(s) - view_h).max(0.0);
                    m
                }
                None => "Render failed: no file was written".into(),
            },
            Err(e) => format!("Render failed: {e}"),
        });
        self.invalidate();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use os_model::{DataFolder, Note};

    fn app() -> App {
        let mut app = App::new(None);
        app.state.project = crate::app::test_project();
        app.state.project.patterns[0].notes.push(Note::step(0, 0));
        app.state.song_mode = false;
        app
    }

    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lemon-pattern-render-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn with_data_folder(app: &mut App, dir: &Path) {
        app.state.project.data_folder = Some(DataFolder { choice: DataFolder::SPECIFIED, folder: crate::data_folder::folder_text(dir) });
    }

    /// The renders go to the data folder's Rendered folder; without a data folder, to one-click recording's fallback
    /// (in tests no user folder: the temporary folder) (§2).
    #[test]
    fn renders_go_to_the_rendered_folder() {
        let mut app = app();
        assert_eq!(folder(&app.state), std::env::temp_dir().join("Rendered"));
        let d = temp("folder");
        with_data_folder(&mut app, &d);
        assert_eq!(folder(&app.state), d.join("Rendered"));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// The file is named after the pattern, made safe, numbered when taken (§2).
    #[test]
    fn file_names() {
        let d = temp("names");
        assert_eq!(file_stem(&d, "Verse"), "Verse");
        assert_eq!(file_stem(&d, "Drums: A/B?"), "Drums_ A_B_");
        assert_eq!(file_stem(&d, "  Intro.. "), "Intro");
        assert_eq!(file_stem(&d, " "), "Pattern");
        std::fs::write(d.join("Verse.wav"), b"").unwrap();
        assert_eq!(file_stem(&d, "Verse"), "Verse #2");
        std::fs::write(d.join("Verse #2.wav"), b"").unwrap();
        assert_eq!(file_stem(&d, "Verse"), "Verse #3");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// The render's choices: one 32-bit float WAV of one pass of the pattern, tail ringing out, no master effects,
    /// no markers or loop points (§2).
    #[test]
    fn the_render_job() {
        let app = app();
        let d = std::env::temp_dir();
        let j = job(&app.state, &d, "Pattern 1");
        let o = &j.options;
        assert_eq!(o.ticked().collect::<Vec<_>>(), [ex::WAV]);
        assert_eq!(ex::WAV_DEPTHS[o.wav_depth], "32-bit float");
        assert!(o.tail == Tail::Ring && o.rate == 0 && o.insert_fx && !o.master_fx && !o.split);
        assert!(!o.markers && !o.loops && !o.note_markers && !o.reveal);
        assert_eq!(j.file(ex::WAV), d.join("Pattern 1.wav"));
        assert!(j.span.is_none());
    }

    /// Render and replace: each clip of the pattern gets an audio clip at its place, as long and cut the same, drawn
    /// above it and selected; the pattern clips are muted; other clips stay (§3).
    #[test]
    fn replace_mutes_and_covers_the_pattern_clips() {
        let mut app = app();
        let s = &mut app.state;
        let ppq = u32::from(s.project.ppq);
        let a = crate::playlist::arr_mut(s);
        a.clips.push(Clip::new(0, 4 * ppq, 2, ClipSource::Pattern(1)));
        a.clips.push(Clip::new(0, 4 * ppq, 3, ClipSource::Pattern(2)));
        let mut cut = Clip::new(8 * ppq, 2 * ppq, 5, ClipSource::Pattern(1));
        cut.window = Some((f64::from(ppq), f64::from(3 * ppq)));
        cut.muted = true;
        a.clips.push(cut);
        let ch = add_channel(s, "x.wav", 1);
        assert_eq!(s.project.channels.last().unwrap().name.as_deref(), Some("Pattern 1"));
        assert_eq!(replace_clips(s, 1, ch), 2);
        let c = crate::playlist::clips(s).to_vec();
        assert_eq!(c.len(), 5);
        assert!(c[0].source == ClipSource::Pattern(1) && c[0].muted);
        assert_eq!((c[1].source, c[1].position, c[1].length, c[1].track, c[1].muted, c[1].window), (ClipSource::Channel(ch), 0, 4 * ppq, 2, false, None));
        assert!(c[2].source == ClipSource::Pattern(2) && !c[2].muted, "another pattern's clip stays");
        assert!(c[3].source == ClipSource::Pattern(1) && c[3].muted);
        assert_eq!((c[4].position, c[4].length, c[4].track), (8 * ppq, 2 * ppq, 5));
        assert!(c[4].muted, "a muted pattern clip gives a muted audio clip");
        let (w0, w1) = c[4].window.expect("cut as the pattern clip");
        let ms = |t: u32| crate::playlist::to_src(s, ClipSource::Channel(ch), f64::from(t));
        assert!((w0 - ms(ppq)).abs() < 1e-9 && (w1 - ms(3 * ppq)).abs() < 1e-9);
        assert_eq!(s.playlist.sel, [1, 4]);
        assert_eq!(replace_clips(s, 7, ch), 0, "no clips: nothing changes");
    }

    /// Runs the render the command started, as the app's tick follows an export.
    fn finish(app: &mut App) {
        let t0 = std::time::Instant::now();
        while app.export.is_some() {
            assert!(t0.elapsed().as_secs() < 60, "the render did not finish");
            app.export_tick();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    /// Quick render: a WAV named after the pattern in the Rendered folder, a new audio clip channel named after the
    /// pattern playing it, nothing in the playlist; one undo step takes the channel away (§2). A pattern without notes
    /// renders nothing.
    #[test]
    fn quick_render_makes_a_channel() {
        let mut app = app();
        let d = temp("quick");
        with_data_folder(&mut app, &d);
        crate::patterns::pat_mut(&mut app.state, 1).name = Some("Verse".into());
        let channels = app.state.project.channels.len();
        app.pattern_cmd(crate::patterns::Cmd::QuickRender);
        assert!(app.export.is_some() && app.pattern_render.is_some(), "the export's progress shows");
        finish(&mut app);
        assert!(app.pattern_render.is_none(), "{:?}", app.state.message);
        assert_eq!(app.state.project.channels.len(), channels + 1, "{:?}", app.state.message);
        let ch = app.state.project.channels.last().unwrap();
        assert_eq!((ch.kind, ch.name.as_deref(), ch.mixer_track), (ChannelKind::AudioClip, Some("Verse"), 0));
        let path = PathBuf::from(ch.sample_path.clone().unwrap());
        assert_eq!(path, d.join("Rendered").join("Verse.wav"));
        assert!(std::fs::metadata(&path).unwrap().len() > 44);
        assert!(crate::playlist::clips(&app.state).is_empty(), "nothing placed");
        assert!(app.state.history.entries().iter().any(|e| e.1 == QUICK));
        crate::history::undo(&mut app.state);
        assert_eq!(app.state.project.channels.len(), channels, "one undo step");
        // A pattern with no notes: a warning, no render.
        app.state.pattern = 2;
        app.pattern_cmd(crate::patterns::Cmd::QuickRender);
        assert!(app.export.is_none() && app.pattern_render.is_none() && !app.state.dialogs.is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Render as audio clip...: the Export form on the current pattern with the Rendered folder, the pattern's name and
    /// the render's choices; Export makes the channel and keeps no choices (§1).
    #[test]
    fn render_form() {
        let mut app = app();
        let d = temp("form");
        with_data_folder(&mut app, &d);
        app.state.song_mode = true;
        let channels = app.state.project.channels.len();
        app.pattern_cmd(crate::patterns::Cmd::RenderAsClip);
        let Some(crate::dialog::Modal { body: crate::dialog::Body::Project(crate::project_forms::Form::Export(f)), .. }) = app.state.dialogs.last() else {
            panic!("the form opens")
        };
        assert!(f.to_clip && !f.song && !f.song_ok && f.o.formats == [true, false, false, false] && !f.o.master_fx);
        assert_eq!((f.folder.clone(), f.name.text.as_str()), (d.join("Rendered"), "Pattern 1"));
        app.dialog_click(crate::dialog::DHit::Form(crate::project_forms::FHit::Main));
        assert!(app.state.dialogs.is_empty() && app.pattern_render.is_some());
        finish(&mut app);
        assert_eq!(app.state.project.channels.len(), channels + 1, "{:?}", app.state.message);
        let ch = app.state.project.channels.last().unwrap();
        assert_eq!(ch.sample_path.as_deref().map(PathBuf::from), Some(d.join("Rendered").join("Pattern 1.wav")));
        assert!(crate::playlist::clips(&app.state).is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Render and replace from the menu: the channel and the clips in one undo step (§3).
    #[test]
    fn render_and_replace() {
        let mut app = app();
        let d = temp("replace");
        with_data_folder(&mut app, &d);
        let ppq = u32::from(app.state.project.ppq);
        crate::playlist::arr_mut(&mut app.state).clips.push(Clip::new(ppq * 4, ppq * 4, 1, ClipSource::Pattern(1)));
        let channels = app.state.project.channels.len();
        app.pattern_cmd(crate::patterns::Cmd::RenderReplace);
        finish(&mut app);
        assert_eq!(app.state.project.channels.len(), channels + 1, "{:?}", app.state.message);
        let ch = app.state.project.channels.last().unwrap().index;
        let c = crate::playlist::clips(&app.state).to_vec();
        assert_eq!(c.len(), 2);
        assert!(c[0].muted && c[1].source == ClipSource::Channel(ch) && (c[1].position, c[1].track) == (ppq * 4, 1));
        assert!(app.state.history.entries().iter().any(|e| e.1 == REPLACE));
        crate::history::undo(&mut app.state);
        let c = crate::playlist::clips(&app.state);
        assert!(c.len() == 1 && !c[0].muted && app.state.project.channels.len() == channels, "one undo step");
        let _ = std::fs::remove_dir_all(&d);
    }
}
