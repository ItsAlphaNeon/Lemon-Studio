// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The interface's side of `crate::lemon_midi_out` (step 9: the session's side stays there).

#![allow(unused_imports)]
#![allow(deprecated)] // writes the project directly (`crate::locked`; control-plane step 14 removes this)
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use crossbeam_queue::ArrayQueue;
use os_engine::{Instrument, TransportInfo};
use os_model::{Channel, ChannelKind};
#[allow(unused_imports)]
use crate::lemon_midi_out::*;

impl crate::app::App {
    /// Every MIDI Out channel has its instrument, with the channel's settings and levels; rows that are no longer MIDI
    /// Out channels lose theirs. Runs every tick.
    pub(crate) fn sync_midi_outs(&mut self) {
        self.host(sync_midi_outs);
    }

    /// ADD ▸ MIDI Out (spec §6.2): a MIDI Out channel on the master track, its window open. One undo step, "Add
    /// channel".
    pub(crate) fn add_midi_out(&mut self) -> Option<String> {
        crate::control_ui::edit_app(self, crate::history::ADD_CHANNEL, crate::control::command::Region::Project, |app| {
            let s = &mut app.state;
            let row = crate::projects::add_plugin_channel(s.session.project.get_mut(), LEMON_MIDI_OUT);
            make_midi_out(&mut s.session.project.get_mut().channels[row], &Settings::default());
            s.session.dirty = true;
            s.selected_channel = row;
            s.rack.select_only(row);
            crate::rack::autosize(s);
            app.open_channel_window(row);
            Some("Added a MIDI Out channel (Lemon MIDI Out)".into())
        })
    }

    /// TOOLS ▸ Set up for MIDI export: the question first (spec: ui/menus.md "Prepare for MIDI export").
    pub(crate) fn ask_midi_export(&mut self) {
        use crate::dialog::{spans, Answer, Kind, Msg, Then};
        self.show_msg(Msg::new(Kind::Question, "Confirm", spans(&[PREP_CONFIRM]), &[Answer::Ok, Answer::Cancel], Then::MidiExportConfirm));
    }

    /// Ok to the question (spec §6.6): every sampler and instrument plugin channel becomes a MIDI Out channel, MIDI
    /// channel 1, 2, 3 … in rack order (the 17th is 1 again), port 0, patch 1, on the master track, each with a cut
    /// group of its own (§4); names, places and notes stay. One undo step: the channels take new ids, so their old
    /// instruments, kept by the old ones (`Plugins::keep`), come back with an undo (`control::host`).
    pub(crate) fn prepare_midi_export(&mut self) -> String {
        let rows: Vec<usize> = self.state.session.project.channels.iter().enumerate().filter(|(_, c)| matches!(c.kind, ChannelKind::Sampler | ChannelKind::Plugin)).map(|(r, _)| r).collect();
        if rows.is_empty() {
            return "No channels to set up for MIDI export".into();
        }
        crate::control_ui::edit_app(self, PREP_LABEL, crate::control::command::Region::Project, |app| {
            for &row in &rows {
                app.state.session.plugins.keep(row);
                app.state.session.plugins.drop_key(row);
            }
            app.state.session.hosted_rows.retain(|r| !rows.contains(r));
            app.state.session.plugins.sampler_failed.retain(|r| !rows.contains(r));
            app.state.session.plugins.native_failed.retain(|r| !rows.contains(r));
            app.state.session.plugins.wrap_failed.retain(|r| !rows.contains(r));
            let s = &mut app.state;
            let groups = cut_groups(&rows.iter().map(|&r| s.session.project.channels[r].misc.cut).collect::<Vec<_>>());
            for (i, &row) in rows.iter().enumerate() {
                let ch = &mut s.session.project.get_mut().channels[row];
                // A new id at once, before anything is hosted on the row and takes its channel's.
                ch.id = os_model::Id::NONE;
                make_midi_out(ch, &Settings { channel: (i % 16) as u8, ..Settings::default() });
                (ch.misc.cut, ch.misc.cut_by) = (groups[i], groups[i]);
            }
            s.session.control.settle(s.session.project.get_mut());
            s.session.dirty = true;
            app.sync_midi_outs();
            app.state.session.transport.set_instruments(app.state.session.plugins.instruments());
            app.open_channel_window(rows[0]);
        });
        let n = rows.len();
        format!("Set up for MIDI export: {n} channel{} now play{} Lemon MIDI Out", if n == 1 { "" } else { "s" }, if n == 1 { "s" } else { "" })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::app::App;
    use crate::knobs::KnobId;

    fn app() -> App {
        let mut a = App::new(None);
        a.state.session.project = crate::locked::Locked::new(crate::projects::test_project());
        a.state.session.file = None;
        a.state.session.path = None;
        a.state.session.plugins = crate::plugins::Plugins::default();
        a.state.session.plugins.available = crate::plugins::builtin();
        a
    }

    /// ADD ▸ MIDI Out makes a channel holding the original's plugin and default state; the window's knobs edit the
    /// state in place; a save keeps it byte for byte and a reopened project plays it again (spec §6.1-6.3).
    #[test]
    fn midi_out_channels_save_and_reopen() {
        let path = std::env::temp_dir().join(format!("lemon-midi-out-{}.flp", std::process::id()));
        let mut a = app();
        a.add_midi_out();
        let row = a.state.session.project.channels.len() - 1;
        let ch = &a.state.session.project.channels[row];
        assert!(is_midi_out(ch));
        assert_eq!((ch.name.as_deref(), ch.plugin.as_deref(), ch.mixer_track, ch.color), (Some(LEMON_MIDI_OUT), Some(FILE_NAME), 0, Some(COLOR)));
        assert_eq!(ch.plugin_state.as_deref(), Some(default_state().as_slice()));
        assert_eq!(crate::project_plugins::plugin_title(ch).as_deref(), Some("Lemon MIDI Out"));
        assert!(a.state.shown(crate::app::Win::Plugin) && a.state.plugin_channel == row, "its window opens");
        // The window's values.
        a.state.set_knob(KnobId::MidiOut(row, MoKnob::Channel), 9.0 / 15.0);
        a.state.set_knob(KnobId::MidiOut(row, MoKnob::Port), 5.0 / 255.0);
        a.state.set_knob(KnobId::MidiOut(row, MoKnob::Patch), 34.0 / 128.0);
        a.state.set_knob(KnobId::MidiOut(row, MoKnob::Bank), 0.0);
        a.state.set_knob(KnobId::MidiOut(row, MoKnob::Bank2), 3.0 / 128.0);
        assert_eq!(a.state.knob_text(KnobId::MidiOut(row, MoKnob::Patch)), "MIDI Out patch: 34 Electric Bass (finger)");
        assert_eq!(a.state.knob_text(KnobId::MidiOut(row, MoKnob::Channel)), "MIDI Out channel: 10");
        assert_eq!(a.state.knob_text(KnobId::MidiOut(row, MoKnob::Bank)), "MIDI Out bank: ---");
        let want = Settings { channel: 9, port: 5, patch: 34, bank: [-1, 2], ..Settings::default() };
        assert_eq!(row_settings(&a.state.session.project, row), Some(want));
        // Its instrument follows the settings.
        a.sync_midi_outs();
        assert_eq!(a.state.session.plugins.midi_outs.get(&row).map(|i| i.lock().unwrap().live.settings), Some(want));
        assert!(a.state.session.plugins.instruments().iter().any(|(r, _)| *r == row));
        let state = a.state.session.project.channels[row].plugin_state.clone().unwrap();
        assert_eq!(state.len(), 967);

        a.save_to(path.clone());
        assert!(a.state.message.as_deref().is_some_and(|m| m.starts_with("Saved")), "{:?}", a.state.message);
        let saved = os_flp::project::load(&os_flp::FlpFile::parse(&std::fs::read(&path).unwrap()).unwrap()).unwrap();
        let ch = &saved.channels[row];
        assert_eq!((ch.kind, ch.plugin.as_deref(), ch.plugin_state.as_deref()), (ChannelKind::Plugin, Some(FILE_NAME), Some(state.as_slice())));

        // Reopened: played again, not looked for as an installed plugin; saved again unchanged.
        let mut b = App::new(None);
        b.state.session.plugins.available = crate::plugins::builtin();
        b.open(path.clone());
        b.sync_native();
        b.sync_midi_outs();
        assert!(b.state.session.plugins.midi_outs.contains_key(&row) && !b.state.session.hosted_rows.contains(&row), "{:?}", b.state.message);
        assert_eq!(row_settings(&b.state.session.project, row), Some(want));
        b.save_to(path.clone());
        let again = os_flp::project::load(&os_flp::FlpFile::parse(&std::fs::read(&path).unwrap()).unwrap()).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(again.channels[row], saved.channels[row]);
    }

    /// TOOLS ▸ Set up for MIDI export (spec: ui/menus.md, plugins/midi-out.md §6.6): the question, then every sampler
    /// and instrument channel is a MIDI Out channel on MIDI channel 1, 2, 3 … with its own cut group, its name kept;
    /// an automation clip channel is left alone. One undo step brings the channels and their instruments back.
    #[test]
    fn set_up_for_midi_export() {
        let mut a = app();
        // The five samplers, then a Lemon Trio and an automation clip channel.
        let trio = crate::projects::add_plugin_channel(a.state.session.project.get_mut(), crate::osc3::LEMON_TRIO);
        let auto = crate::projects::add_channel(a.state.session.project.get_mut());
        a.state.session.project.get_mut().channels[auto].kind = ChannelKind::Automation;
        a.state.session.project.get_mut().channels[1].misc.cut = 4;
        for _ in 0..4 {
            a.plugin_tick();
        }
        assert!(a.state.session.hosted_rows.contains(&0) && a.state.session.hosted_rows.contains(&trio), "{:?}", a.state.session.hosted_rows);
        let before = a.state.session.project.channels.clone();

        a.ask_midi_export();
        let Some(crate::dialog::Body::Msg(m)) = a.state.dialogs.last().map(|d| &d.body) else { panic!("the question") };
        assert_eq!((m.title.as_str(), m.text[0].0.as_str(), &m.then), ("Confirm", PREP_CONFIRM, &crate::dialog::Then::MidiExportConfirm));
        a.state.dialogs.clear();

        let msg = a.prepare_midi_export();
        assert_eq!(msg, "Set up for MIDI export: 6 channels now play Lemon MIDI Out");
        let p = &a.state.session.project;
        for (i, ch) in p.channels[..=trio].iter().enumerate() {
            assert!(is_midi_out(ch), "row {i}");
            assert_eq!(ch.name, before[i].name);
            assert_eq!((ch.mixer_track, ch.color, ch.sample_path.as_deref()), (0, Some(COLOR), None));
            assert_eq!(row_settings(p, i), Some(Settings { channel: i as u8, ..Settings::default() }));
        }
        let groups: Vec<u32> = p.channels[..=trio].iter().map(|c| c.misc.cut).collect();
        assert_eq!(groups, vec![1, 4, 2, 3, 5, 6]);
        assert_eq!(p.channels[auto], before[auto]);
        assert!(!a.state.session.hosted_rows.iter().any(|&r| r <= trio), "the instruments went");
        assert_eq!(a.state.session.plugins.midi_outs.len(), 6);
        assert!(a.state.shown(crate::app::Win::Plugin) && a.state.plugin_channel == 0);
        assert_eq!(a.state.session.history.entries().last().map(|e| e.1), Some(PREP_LABEL));

        a.undo_key(false);
        for _ in 0..4 {
            a.plugin_tick();
        }
        for (i, ch) in a.state.session.project.channels.iter().enumerate() {
            assert_eq!(ch, &before[i], "row {i} back");
        }
        assert!(a.state.session.hosted_rows.contains(&0) && a.state.session.hosted_rows.contains(&trio), "the instruments back: {:?}", a.state.session.hosted_rows);
        assert!(a.state.session.plugins.midi_outs.is_empty());
    }

    /// Sampler channels of an opened file, set up for MIDI export and saved: the file's channels are MIDI Out channels
    /// with their states, their names and their notes, no sample, on the master track.
    #[test]
    fn set_up_for_midi_export_saves() {
        let dir = std::env::temp_dir().join(format!("lemon-midi-out-prep-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let (first, second) = (dir.join("a.flp"), dir.join("b.flp"));
        let mut a = app();
        a.state.session.project.get_mut().patterns[0].notes.push(os_model::Note { length: 48, key: 60, ..os_model::Note::step(2, 0) });
        a.save_to(first.clone());
        let mut b = App::new(None);
        b.state.session.plugins.available = crate::plugins::builtin();
        b.open(first.clone());
        assert!(b.state.session.project.channels.iter().all(|c| c.rack.origin.is_some()));
        b.prepare_midi_export();
        let states: Vec<Option<Vec<u8>>> = b.state.session.project.channels.iter().map(|c| c.plugin_state.clone()).collect();
        b.save_to(second.clone());
        assert!(b.state.message.as_deref().is_some_and(|m| m.starts_with("Saved")), "{:?}", b.state.message);
        let saved = os_flp::project::load(&os_flp::FlpFile::parse(&std::fs::read(&second).unwrap()).unwrap()).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        let names = ["Kick", "Clap", "Hat", "Snare", "Pluck"];
        assert_eq!(saved.channels.len(), 5);
        for (i, ch) in saved.channels.iter().enumerate() {
            assert!(is_midi_out(ch), "row {i}: {:?} {:?}", ch.kind, ch.plugin);
            assert_eq!((ch.name.as_deref(), ch.sample_path.as_deref(), ch.mixer_track), (Some(names[i]), None, 0));
            assert_eq!(ch.plugin_state, states[i]);
            assert_eq!(settings(ch.plugin_state.as_deref().unwrap()).channel, i as u8);
        }
        assert_eq!(saved.patterns[0].notes.len(), 1);
    }

    /// A MIDI Out check flips with a click, one undo step; a value is a knob step (spec §6.3).
    #[test]
    fn midi_out_settings_undo() {
        let mut a = app();
        a.add_midi_out();
        let row = a.state.session.project.channels.len() - 1;
        let id = KnobId::MidiOut(row, MoKnob::SendLevels);
        crate::history::knob(&mut a.state.session, id, false);
        a.state.set_knob(id, 0.0);
        assert!(!row_settings(&a.state.session.project, row).unwrap().send_levels);
        a.undo_key(false);
        assert!(row_settings(&a.state.session.project, row).unwrap().send_levels);
        assert_eq!(a.state.session.project.channels[row].plugin_state.as_deref(), Some(default_state().as_slice()));
    }
}
