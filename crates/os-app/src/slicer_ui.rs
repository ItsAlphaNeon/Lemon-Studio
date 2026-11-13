// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The interface's side of `crate::slicer` (step 9: the session's side stays there).

#![allow(unused_imports)]
#![allow(deprecated)] // writes the project directly (`crate::locked`; control-plane step 14 removes this)
use os_model::{Channel, ChannelKind, Note, Project};
use crate::plugins::{Plugins, FX_KEY};
#[allow(unused_imports)]
use crate::slicer::*;

impl crate::app::App {
    /// Hosts Lemon Slicer on every Fruity Slicer channel without an instance, started from the channel's saved state
    /// (a new instance's when it has none). A row waiting for the picker's own instance is left to it; a row whose
    /// instance failed is not tried again.
    pub(crate) fn sync_slicer(&mut self) {
        self.host(sync_slicer);
    }

    /// Notes a plugin sent for channel row `row` (Lemon Slicer's dump): they replace the channel's notes in the
    /// current pattern, positions rounded to the project's ticks, as one undo step. An effect's notes (`row` a mixer
    /// slot's key: Tesla's Convert to score) go to the selected channel.
    pub(crate) fn notes_from_plugin(&mut self, row: usize, notes: &[os_hostlink::NoteOut]) {
        let row = if crate::plugins::fx_of(row).is_some() { self.state.selected_channel } else { row };
        let s = &mut self.state;
        let Some(ch) = s.session.project.channels.get(row).map(|c| c.index) else { return };
        if notes.is_empty() {
            return;
        }
        let ppq = f64::from(s.session.project.ppq.max(1));
        crate::piano::snapshot_as(&mut s.session, "Dump to piano roll");
        let p = crate::piano::pattern_mut(&mut s.session);
        p.notes.retain(|n| n.channel != ch);
        for n in notes {
            let start = (n.start * ppq).round().max(0.0) as u32;
            let end = ((n.end * ppq).round().max(0.0) as u32).max(start);
            p.notes.push(Note { length: end - start, key: u16::from(n.key), velocity: n.velocity, ..Note::step(ch, start) });
        }
        crate::piano::finish_edit(s);
        s.session.dirty = true;
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

    /// A Fruity Slicer channel added in the app saves the original's new-channel state; one with a loop saves its
    /// bytes, and the reopened project hosts it again from them (the file's own spelling of the id kept).
    #[test]
    fn slicer_channels_save_and_reopen() {
        let file = std::env::temp_dir().join(format!("openstudio-slicer-{}.flp", std::process::id()));
        let wav = loop_file("reopen");
        let mut a = App::new(None);
        a.state.session.project = crate::locked::Locked::new(crate::app::test_project());
        a.state.session.file = None;
        a.state.session.path = None;
        a.state.session.plugins.available = crate::plugins::builtin();
        let row = crate::app::add_plugin_channel(a.state.session.project.get_mut(), LEMON_SLICER);
        let ch = &a.state.session.project.channels[row];
        assert_eq!((ch.kind, ch.plugin.as_deref()), (ChannelKind::Plugin, Some(FILE_NAME)));
        assert_eq!(crate::project_plugins::plugin_title(ch).as_deref(), Some(LEMON_SLICER));
        a.sync_slicer();
        assert!(a.state.session.hosted_rows.contains(&row));
        a.save_to(file.clone());
        let saved = os_flp::project::load(&os_flp::FlpFile::parse(&std::fs::read(&file).unwrap()).unwrap()).unwrap();
        assert_eq!(saved.channels[row].plugin_state.as_deref(), Some(lemon_slicer::params::default_state().as_slice()));

        let fl = state_for(&wav);
        assert!(a.state.session.plugins.set_state(row, &plugin_state(&fl)));
        a.state.session.project.get_mut().channels[row].plugin = Some("Fruity slicer".into());
        a.save_to(file.clone());
        let saved = os_flp::project::load(&os_flp::FlpFile::parse(&std::fs::read(&file).unwrap()).unwrap()).unwrap();
        assert_eq!(saved.channels[row].plugin.as_deref(), Some("Fruity slicer"));
        assert_eq!(saved.channels[row].plugin_state.as_deref(), Some(fl.as_slice()));
        let mut b = App::new(None);
        b.state.session.plugins.available = crate::plugins::builtin();
        b.open(file.clone());
        b.sync_slicer();
        assert!(b.state.session.hosted_rows.contains(&row), "{:?}", b.state.message);
        assert_eq!(b.state.session.plugins.slicer_file_state(row), Some(fl.clone()));
        let _ = std::fs::remove_file(&file);
        let _ = std::fs::remove_file(wav);
    }

    /// A file Tesla sends to a new Lemon Slicer channel: a hosted Fruity Slicer channel named after the file, sliced at
    /// its regions, selected; undone, it goes (spec: plugins/tesla/README.md "Sending slices to Lemon Slicer").
    #[test]
    fn a_sent_file_becomes_a_sliced_slicer_channel() {
        use tesla::sample::{Cue, Format, Sample};
        let path = std::env::temp_dir().join(format!("openstudio-to-slicer-{}.wav", std::process::id()));
        let mut s = Sample::from_channels(44100, vec![vec![0.25; 44100]], Format::Int16);
        for (i, (pos, len)) in [(0usize, 11025usize), (11025, 11025), (22050, 22050)].into_iter().enumerate() {
            s.cues.push(Cue { id: i as u32 + 1, name: format!("Part {}", i + 1), pos, len: Some(len) });
        }
        std::fs::write(&path, tesla::wav::write(&s, tesla::wav::Depth::Own)).unwrap();
        let mut a = App::new(None);
        a.state.session.project = crate::locked::Locked::new(crate::app::test_project());
        a.state.session.plugins.available = crate::plugins::builtin();
        let before = a.state.session.project.channels.len();
        a.sample_from_plugin(path.to_string_lossy().into_owned(), os_hostlink::AudioTo::Slicer);
        assert_eq!(a.state.session.project.channels.len(), before + 1, "{:?}", a.state.message);
        let row = before;
        let ch = &a.state.session.project.channels[row];
        assert!(is_slicer(ch));
        assert_eq!(ch.name.as_deref(), Some(format!("openstudio-to-slicer-{}", std::process::id()).as_str()));
        let st = State::parse(ch.plugin_state.as_deref().unwrap()).unwrap();
        assert_eq!(st.slices.iter().map(|s| s.start).collect::<Vec<_>>(), [0, 11025, 22050]);
        assert_eq!(st.slices[1].name(), "Part 2");
        assert_eq!(a.state.selected_channel, row);
        assert!(a.state.session.hosted_rows.contains(&row));
        assert!(a.state.message.as_deref().is_some_and(|m| m.contains("3 slices")));
        assert_eq!(crate::history_ui::undo(&mut a.state).as_deref(), Some(crate::history::ADD_CHANNEL));
        assert_eq!(a.state.session.project.channels.len(), before);
        let _ = std::fs::remove_file(&path);
    }

    /// A loop dropped on Lemon Slicer's window, from the system or the browser (Discord feedback 2026-10-30), replaces its
    /// loop and is sliced as its own Load does, on the same channel; a file it can't read leaves the loop as it was and
    /// says why; another plugin's window doesn't take it this way.
    #[test]
    fn a_dropped_loop_goes_into_the_slicer() {
        use tesla::sample::{Cue, Format, Sample};
        let first = std::env::temp_dir().join(format!("openstudio-drop-slicer-{}.wav", std::process::id()));
        let mut s = Sample::from_channels(44100, vec![vec![0.25; 44100]], Format::Int16);
        s.cues.push(Cue { id: 1, name: "Part 1".into(), pos: 0, len: Some(22050) });
        s.cues.push(Cue { id: 2, name: "Part 2".into(), pos: 22050, len: Some(22050) });
        std::fs::write(&first, tesla::wav::write(&s, tesla::wav::Depth::Own)).unwrap();
        let mut a = App::new(None);
        a.state.session.project = crate::locked::Locked::new(crate::app::test_project());
        a.state.session.plugins.available = crate::plugins::builtin();
        a.sample_from_plugin(first.to_string_lossy().into_owned(), os_hostlink::AudioTo::Slicer);
        let row = a.state.session.project.channels.len() - 1;
        let channels = a.state.session.project.channels.len();
        let loaded = |a: &App| State::parse(&a.state.session.plugins.slicer_file_state(row).unwrap()).unwrap();
        assert_eq!(loaded(&a).slices.len(), 2);

        let dropped = loop_file("dropped");
        a.state.session.plugin_edited = false;
        a.file_on_plugin_window(row, dropped.clone());
        let st = loaded(&a);
        assert_eq!(st.path(), dropped.to_string_lossy());
        assert!(!st.slices.is_empty() && st.slices.iter().all(|s| s.name() != "Part 2"), "sliced anew");
        assert_eq!(a.state.session.project.channels.len(), channels, "no new channel");
        assert!(a.state.message.as_deref().is_some_and(|m| m.contains("loaded in Lemon Slicer")), "{:?}", a.state.message);
        assert!(a.state.session.plugin_edited, "the project counts as changed");

        let bad = std::env::temp_dir().join(format!("openstudio-drop-slicer-{}.wav.txt", std::process::id()));
        std::fs::write(&bad, b"not audio").unwrap();
        assert!(a.loop_to_slicer(row, &bad.to_string_lossy()));
        assert_eq!(loaded(&a).path(), dropped.to_string_lossy(), "the loop stays");
        assert!(a.state.message.as_deref().is_some_and(|m| m.starts_with("Lemon Slicer: ")), "{:?}", a.state.message);
        assert!(!a.loop_to_slicer(0, &dropped.to_string_lossy()), "row 0 hosts no Lemon Slicer");
        for p in [first, dropped, bad] {
            let _ = std::fs::remove_file(p);
        }
    }

    /// The dump replaces the channel's notes in the current pattern (the other channels' stay), in ticks, as one undo
    /// step.
    #[test]
    fn dumped_notes_replace_the_channels_notes() {
        let mut a = App::new(None);
        a.state.session.project = crate::locked::Locked::new(crate::app::test_project());
        let row = crate::app::add_plugin_channel(a.state.session.project.get_mut(), LEMON_SLICER);
        let ch = a.state.session.project.channels[row].index;
        let other = a.state.session.project.channels[0].index;
        a.state.session.project.get_mut().ppq = 96;
        let p = crate::piano::pattern_mut(&mut a.state.session);
        p.notes = vec![Note::step(ch, 0), Note::step(other, 48)];
        let notes: Vec<os_hostlink::NoteOut> = [(0.0, 0.5625, 60), (0.5625, 1.125, 61)]
            .into_iter()
            .map(|(start, end, key)| os_hostlink::NoteOut { start, end, key, velocity: 100 })
            .collect();
        a.notes_from_plugin(row, &notes);
        let got: Vec<(u16, u32, u32, u16)> = crate::piano::notes(&a.state.session).iter().map(|n| (n.channel, n.position, n.length, n.key)).collect();
        assert_eq!(got, [(ch, 0, 54, 60), (other, 48, 0, 60), (ch, 54, 54, 61)]);
        assert!(crate::piano::undo(&mut a.state));
        assert_eq!(crate::piano::notes(&a.state.session).len(), 2);
        // An effect's notes (Tesla's Convert to score, from a mixer slot) go to the selected channel.
        a.state.selected_channel = 0;
        a.notes_from_plugin(crate::plugins::fx_key(0, 3), &notes[..1]);
        let mut got: Vec<(u16, u32, u16)> = crate::piano::notes(&a.state.session).iter().map(|n| (n.channel, n.position, n.key)).collect();
        got.sort();
        let mut want = vec![(ch, 0, 60), (other, 0, 60)];
        want.sort();
        assert_eq!(got, want);
    }

}
