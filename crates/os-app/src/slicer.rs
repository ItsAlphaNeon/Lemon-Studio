// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Slicer in Lemon Studio: compiled in like Citrus, it plays every Fruity Slicer channel of a project (started
//! from the state the file holds, its loop opened from the stored path) and every one added from the picker; saving
//! writes the state it plays back into the channel, so the file keeps its own plugin with its loop and slices. The
//! dump to the piano roll arrives through the host link (`Notes`) and replaces the channel's notes in the current
//! pattern.
//!
// spec: plugins/lemon-slicer.md §4 (dump), §6 (app integration); plugins/fruity-slicer.md §1

use os_model::{Channel, ChannelKind, Note, Project};

use crate::plugins::{Plugins, FX_KEY};

/// The built-in plugin's name (picker, window title).
pub const LEMON_SLICER: &str = "Lemon Slicer";
/// The plugin id project files store for it (event 201); FL's own demos also store "Fruity slicer".
pub const FILE_NAME: &str = "Fruity Slicer";

/// Whether file id `id` names the original's slicer (any case).
pub fn is_file_name(id: &str) -> bool {
    id.eq_ignore_ascii_case(FILE_NAME)
}

/// Whether a channel holds the file's Fruity Slicer.
pub fn is_slicer(ch: &Channel) -> bool {
    ch.kind == ChannelKind::Plugin && ch.plugin.as_deref().is_some_and(is_file_name)
}

impl Plugins {
    /// The Fruity Slicer state the Lemon Slicer on rack row `row` plays, `None` when the row hosts something else.
    pub fn slicer_file_state(&self, row: usize) -> Option<Vec<u8>> {
        let h = self.hosted.get(&row)?;
        let p = h.plugin.lock().ok()?;
        if p.desc().name != LEMON_SLICER || !crate::project_plugins::is_builtin(p.desc()) {
            return None;
        }
        lemon_slicer::params::fl_from_plugin_state(&p.save_state()?)
    }
}

/// Before a save: every hosted Lemon Slicer writes the state it plays into its channel as the file's Fruity Slicer
/// (a channel keeps its own spelling of the id).
pub fn store(proj: &mut Project, plugins: &Plugins) {
    for row in plugins.hosted.range(..FX_KEY).map(|(&k, _)| k).collect::<Vec<_>>() {
        let Some(st) = plugins.slicer_file_state(row) else { continue };
        if let Some(ch) = proj.channels.get_mut(row).filter(|c| c.kind == ChannelKind::Plugin) {
            if !ch.plugin.as_deref().is_some_and(is_file_name) {
                ch.plugin = Some(FILE_NAME.into());
            }
            ch.plugin_state = Some(st);
        }
    }
}

/// The plugin state for a channel's Fruity Slicer state: its loop's stored path resolved for the plugin to open
/// (FL's placeholders made real), the stored path kept in the state.
pub fn plugin_state(fl: &[u8]) -> Vec<u8> {
    let st = lemon_slicer::params::plugin_state_from_fl(fl);
    let stored = lemon_slicer::state::State::parse(fl).map(|s| s.path()).unwrap_or_default();
    let sample = crate::plugins::resolve_sample(&stored);
    if sample.is_empty() {
        return st;
    }
    crate::plugins::with_sample_path(&st, &sample).unwrap_or(st)
}

impl crate::app::App {
    /// Hosts Lemon Slicer on every Fruity Slicer channel without an instance, started from the channel's saved state
    /// (a new instance's when it has none). A row waiting for the picker's own instance is left to it; a row whose
    /// instance failed is not tried again.
    pub(crate) fn sync_slicer(&mut self) {
        if crate::project_plugins::native_first() {
            return;
        }
        let Some(index) = self.plugins.index_of(LEMON_SLICER) else { return };
        let pending = self.state.pending_plugin.map(|(r, _)| r);
        let rows: Vec<usize> = (0..self.state.project.channels.len())
            .filter(|&r| is_slicer(&self.state.project.channels[r]))
            .filter(|&r| !self.state.hosted_rows.contains(&r) && !self.wrap_failed.contains(&r) && Some(r) != pending)
            .collect();
        if rows.is_empty() {
            return;
        }
        let mut added = false;
        for row in rows {
            if self.past_deadline() {
                break;
            }
            let fl = self.state.project.channels[row].plugin_state.clone().unwrap_or_default();
            match self.plugins.create_with(row, index, Some(&plugin_state(&fl))) {
                Ok(_) => {
                    self.state.hosted_rows.push(row);
                    added = true;
                }
                Err(e) => {
                    self.wrap_failed.push(row);
                    self.state.message = Some(format!("{LEMON_SLICER}: {e}"));
                }
            }
        }
        if added {
            self.transport.set_instruments(self.plugins.instruments());
            self.state.piano.dirty = true;
        }
        self.invalidate();
    }

    /// Notes a plugin sent for channel row `row` (Lemon Slicer's dump): they replace the channel's notes in the
    /// current pattern, positions rounded to the project's ticks, as one undo step.
    pub(crate) fn notes_from_plugin(&mut self, row: usize, notes: &[os_hostlink::NoteOut]) {
        let s = &mut self.state;
        let Some(ch) = s.project.channels.get(row).map(|c| c.index) else { return };
        if notes.is_empty() {
            return;
        }
        let ppq = f64::from(s.project.ppq.max(1));
        crate::piano::snapshot_as(s, "Dump to piano roll");
        let p = crate::piano::pattern_mut(s);
        p.notes.retain(|n| n.channel != ch);
        for n in notes {
            let start = (n.start * ppq).round().max(0.0) as u32;
            let end = ((n.end * ppq).round().max(0.0) as u32).max(start);
            p.notes.push(Note { length: end - start, key: u16::from(n.key), velocity: n.velocity, ..Note::step(ch, start) });
        }
        crate::piano::finish_edit(s);
        s.piano.dirty = true;
        s.message = Some(format!("Dumped {} notes to the piano roll", notes.len()));
        self.invalidate();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use lemon_slicer::state::{Slice, State, H_BEATS, H_BPM};
    use os_engine::SharedInstrument;

    /// A 1 s stereo 16-bit WAV at 44.1 kHz: a tone in its second half.
    fn loop_file(name: &str) -> std::path::PathBuf {
        let frames = 44100usize;
        let mut data = Vec::new();
        for i in 0..frames {
            let v = if i >= frames / 2 { ((i as f32 * 0.05).sin() * 16000.0) as i16 } else { 0 };
            data.extend(v.to_le_bytes());
            data.extend(v.to_le_bytes());
        }
        let mut fmt = Vec::new();
        for v in [1u16, 2] {
            fmt.extend(v.to_le_bytes());
        }
        fmt.extend(44100u32.to_le_bytes());
        fmt.extend((44100u32 * 4).to_le_bytes());
        fmt.extend(4u16.to_le_bytes());
        fmt.extend(16u16.to_le_bytes());
        let mut body = b"WAVE".to_vec();
        for (id, b) in [(b"fmt ", &fmt), (b"data", &data)] {
            body.extend(id);
            body.extend((b.len() as u32).to_le_bytes());
            body.extend(b);
        }
        let mut f = b"RIFF".to_vec();
        f.extend((body.len() as u32).to_le_bytes());
        f.extend(body);
        let p = std::env::temp_dir().join(format!("openstudio-slicer-{}-{name}.wav", std::process::id()));
        std::fs::write(&p, f).unwrap();
        p
    }

    /// A Fruity Slicer state for `path`: two slices (silence, then the tone), 2 beats at 120 BPM.
    fn state_for(path: &std::path::Path) -> Vec<u8> {
        let mut s = State::default();
        s.set_f32(H_BEATS, 2.0);
        s.set_f32(H_BPM, 120.0);
        s.set_path(&path.to_string_lossy());
        s.slices = vec![Slice { name: b"Rest".to_vec(), ..Slice::at(0) }, Slice::at(22050)];
        s.to_bytes()
    }

    /// The built-in Lemon Slicer starts from a channel's state, opens its loop, plays slice 2 on C#5 and gives the
    /// same bytes back.
    #[test]
    fn builtin_slicer_takes_and_gives_the_file_state() {
        let path = loop_file("plays");
        let mut pl = Plugins::default();
        pl.available = crate::plugins::builtin();
        let index = pl.index_of(LEMON_SLICER).expect("built-in Lemon Slicer");
        assert!(crate::project_plugins::is_builtin(&pl.available[index].desc));
        let fl = state_for(&path);
        pl.create_with(1, index, Some(&plugin_state(&fl))).unwrap();
        assert_eq!(pl.slicer_file_state(1), Some(fl.clone()));
        let inst = pl.hosted[&1].plugin.clone() as SharedInstrument;
        let mut i = inst.lock().unwrap();
        let peak = |i: &mut dyn os_engine::Instrument| {
            let (mut l, mut r) = (vec![0.0f32; 2400], vec![0.0f32; 2400]);
            i.process(48000, &mut l, &mut r);
            l.iter().fold(0.0f32, |m, v| m.max(v.abs()))
        };
        i.note_on(0, 60, 100.0 / 128.0);
        assert!(peak(&mut *i) < 1e-4, "slice 1 is silence");
        i.note_off(0, 60);
        i.note_on(0, 61, 100.0 / 128.0);
        let p = peak(&mut *i);
        assert!(p > 0.2 && p < 1.0, "slice 2 sounds: {p}");
        i.note_off(0, 61);
        drop(i);
        assert_eq!(pl.slicer_file_state(99), None);
        let _ = std::fs::remove_file(path);
    }

    /// A Fruity Slicer channel added in the app saves the original's new-channel state; one with a loop saves its
    /// bytes, and the reopened project hosts it again from them (the file's own spelling of the id kept).
    #[test]
    fn slicer_channels_save_and_reopen() {
        let file = std::env::temp_dir().join(format!("openstudio-slicer-{}.flp", std::process::id()));
        let wav = loop_file("reopen");
        let mut a = App::new(None);
        a.state.project = crate::app::test_project();
        a.state.file = None;
        a.state.path = None;
        a.plugins.available = crate::plugins::builtin();
        let row = crate::app::add_plugin_channel(&mut a.state.project, LEMON_SLICER);
        let ch = &a.state.project.channels[row];
        assert_eq!((ch.kind, ch.plugin.as_deref()), (ChannelKind::Plugin, Some(FILE_NAME)));
        assert_eq!(crate::project_plugins::plugin_title(ch).as_deref(), Some(LEMON_SLICER));
        a.sync_slicer();
        assert!(a.state.hosted_rows.contains(&row));
        a.save_to(file.clone());
        let saved = os_flp::project::load(&os_flp::FlpFile::parse(&std::fs::read(&file).unwrap()).unwrap()).unwrap();
        assert_eq!(saved.channels[row].plugin_state.as_deref(), Some(lemon_slicer::params::default_state().as_slice()));

        let fl = state_for(&wav);
        assert!(a.plugins.set_state(row, &plugin_state(&fl)));
        a.state.project.channels[row].plugin = Some("Fruity slicer".into());
        a.save_to(file.clone());
        let saved = os_flp::project::load(&os_flp::FlpFile::parse(&std::fs::read(&file).unwrap()).unwrap()).unwrap();
        assert_eq!(saved.channels[row].plugin.as_deref(), Some("Fruity slicer"));
        assert_eq!(saved.channels[row].plugin_state.as_deref(), Some(fl.as_slice()));
        let mut b = App::new(None);
        b.plugins.available = crate::plugins::builtin();
        b.open(file.clone());
        b.sync_slicer();
        assert!(b.state.hosted_rows.contains(&row), "{:?}", b.state.message);
        assert_eq!(b.plugins.slicer_file_state(row), Some(fl.clone()));
        let _ = std::fs::remove_file(&file);
        let _ = std::fs::remove_file(wav);
    }

    /// The dump replaces the channel's notes in the current pattern (the other channels' stay), in ticks, as one undo
    /// step.
    #[test]
    fn dumped_notes_replace_the_channels_notes() {
        let mut a = App::new(None);
        a.state.project = crate::app::test_project();
        let row = crate::app::add_plugin_channel(&mut a.state.project, LEMON_SLICER);
        let ch = a.state.project.channels[row].index;
        let other = a.state.project.channels[0].index;
        a.state.project.ppq = 96;
        let p = crate::piano::pattern_mut(&mut a.state);
        p.notes = vec![Note::step(ch, 0), Note::step(other, 48)];
        let notes: Vec<os_hostlink::NoteOut> = [(0.0, 0.5625, 60), (0.5625, 1.125, 61)]
            .into_iter()
            .map(|(start, end, key)| os_hostlink::NoteOut { start, end, key, velocity: 100 })
            .collect();
        a.notes_from_plugin(row, &notes);
        let got: Vec<(u16, u32, u32, u16)> = crate::piano::notes(&a.state).iter().map(|n| (n.channel, n.position, n.length, n.key)).collect();
        assert_eq!(got, [(ch, 0, 54, 60), (other, 48, 0, 60), (ch, 54, 54, 61)]);
        assert!(crate::piano::undo(&mut a.state));
        assert_eq!(crate::piano::notes(&a.state).len(), 2);
    }

    /// `OS_SLICER_FLP=<a.flp>;<b.flp> cargo test -p os-app corpus_slicer -- --ignored`: every Fruity Slicer channel of
    /// the projects comes back byte for byte through Lemon Slicer, and its loop is found.
    #[test]
    #[ignore]
    fn corpus_slicer_states_round_trip() {
        let Ok(list) = std::env::var("OS_SLICER_FLP") else { return };
        let mut seen = 0;
        for f in list.split(';').filter(|f| !f.is_empty()) {
            let Ok(p) = os_flp::project::load(&os_flp::FlpFile::parse(&std::fs::read(f).unwrap()).unwrap()) else { eprintln!("{f}: not read"); continue };
            for ch in p.channels.iter().filter(|c| is_slicer(c)) {
                let fl = ch.plugin_state.clone().unwrap_or_default();
                let st = State::parse(&fl).unwrap_or_else(|| panic!("{f}: {:?} does not parse", ch.name));
                assert_eq!(st.to_bytes(), fl, "{f}: {:?}", ch.name);
                assert_eq!(lemon_slicer::params::fl_from_plugin_state(&plugin_state(&fl)), Some(fl.clone()));
                let sample = crate::plugins::resolve_sample(&st.path());
                eprintln!("{f}: {:?} ({}), {} slices, {:.3} beats at {:.3} BPM, {sample} found: {}", ch.name, ch.plugin.as_deref().unwrap_or(""), st.slices.len(), st.beats(), st.bpm(), std::path::Path::new(&sample).exists());
                seen += 1;
            }
        }
        assert!(seen > 0);
    }
}
