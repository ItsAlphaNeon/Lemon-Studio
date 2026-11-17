// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The control plane's tests: every command undoes and redoes exactly, transactions are whole, proposals leave the
//! project alone, ids stay with their things, and the JSON-RPC adapter answers over a real socket.

#![allow(deprecated)] // writes the project directly (`crate::locked`; control-plane step 14 removes this)
use serde_json::{json, Value};

use super::*;
use crate::control_ui::*;
use crate::app::App;

fn app() -> App {
    let mut a = App::new(None);
    a.state.session.project = crate::locked::Locked::new(crate::projects::test_project());
    a.state.session.project.get_mut().arrangements = vec![os_flp::project::default_arrangement()];
    a.state.session.control.settle(a.state.session.project.get_mut());
    a
}

fn test_client() -> By {
    By::Client("Test".into())
}

/// The project and the rack's state of it, ids included (`==` on the model leaves ids out). Not the selection: an
/// undo leaves it where it is, as before.
fn reading(a: &App) -> (os_model::Project, Value, f64) {
    let mut q = crate::control::query_ui::project(&a.state);
    q.as_object_mut().unwrap().remove("selected_channel");
    (a.state.session.project.clone(), q, a.state.tempo())
}

fn ids(a: &App) -> (Vec<u32>, Vec<u32>, Vec<u32>) {
    let p = &a.state.session.project;
    (p.channels.iter().map(|c| c.id.0).collect(), p.mixer.iter().map(|m| m.id.0).collect(), p.patterns.iter().map(|q| q.id.0).collect())
}

fn cmd(v: Value) -> Command {
    serde_json::from_value(v).unwrap_or_else(|e| panic!("{e}"))
}

/// Applies `c` as a client would and checks that one undo puts everything back and one redo gives the change again.
fn round_trip(a: &mut App, c: Value) -> Vec<Value> {
    let before = reading(a);
    let steps = a.state.session.history.at();
    let what = c.to_string();
    let r = apply(&mut a.state, &test_client(), None, vec![cmd(c)]).unwrap_or_else(|e| panic!("{what}: {e}"));
    a.state.session.control.settle(a.state.session.project.get_mut());
    let after = reading(a);
    assert_ne!(before, after, "{what} changes something");
    assert_eq!(a.state.session.history.at(), steps + 1, "{what} is one step");
    crate::history_ui::undo(&mut a.state).unwrap();
    assert_eq!(reading(a), before, "{what} undone");
    crate::history_ui::redo(&mut a.state).unwrap();
    assert_eq!(reading(a), after, "{what} redone");
    r
}

#[test]
fn every_command_undoes_and_redoes() {
    let mut a = app();
    let (ch, tr, pa) = ids(&a);
    let (kick, clap) = (ch[0], ch[1]);
    let (master, ins1, ins2) = (tr[0], tr[1], tr[2]);
    let pat = pa[0];
    for c in [
        json!({"op": "set_tempo", "bpm": 97.5}),
        json!({"op": "set_swing", "swing": 0.5}),
        json!({"op": "set_main_volume", "volume": 0.5}),
        json!({"op": "set_main_pitch", "cents": -300}),
        json!({"op": "set_time_signature", "numerator": 3, "denominator": 4}),
        json!({"op": "set_info", "title": "Song", "author": "Me"}),
        json!({"op": "set_project", "beats": 7, "steps": 2, "signature": false, "panning_law": "triangular", "play_truncated": true, "pdc": false}),
        json!({"op": "set_channel", "channel": kick, "name": "Big kick", "color": "#ff8000", "volume": 0.5, "pan": -0.25, "pitch": 100, "track": ins2}),
        json!({"op": "set_channel", "channel": clap, "declick": 3, "swing": 0.25, "truncate_swing": true, "cut": 2, "cut_by": 2}),
        json!({"op": "load_sample", "channel": clap, "path": "/no/such/folder/snare.wav"}),
        json!({"op": "set_track", "track": ins1, "name": "Drums", "color": "#102030", "volume": 0.6, "pan": 0.5, "stereo": -0.5, "polarity": true, "swap": true, "effects_enabled": false}),
        json!({"op": "set_track", "track": ins2, "separator": true, "dock": "right", "threaded": false, "latency_in": 2.5, "latency_out": -1.0}),
        json!({"op": "reset_track", "track": ins1}),
        json!({"op": "move_track", "track": ins1, "to": 3}),
        json!({"op": "set_send", "from": ins1, "to": ins2, "level": 0.5}),
        json!({"op": "set_send", "from": ins1, "to": master}),
        json!({"op": "set_eq", "track": ins1, "band": 1, "gain": 600, "frequency": 30000}),
        json!({"op": "set_slot", "track": ins1, "slot": 3, "enabled": false, "mix": 0.25}),
        json!({"op": "set_pattern", "pattern": pat, "name": "Beat", "color": "#00ff00", "length": 384}),
        json!({"op": "set_notes", "pattern": pat, "channel": kick, "notes": [{"channel": kick, "position": 0, "length": 24, "key": 48}]}),
        json!({"op": "add_notes", "pattern": pat, "notes": [{"channel": clap, "position": 96, "velocity": 80}]}),
        json!({"op": "set_playlist_track", "track": 2, "name": "Lead", "color": "#abcdef", "enabled": false}),
        json!({"op": "move_channel", "channel": kick, "row": 3}),
        json!({"op": "delete_channel", "channel": clap}),
        json!({"op": "add_channel", "name": "Bass", "track": ins2}),
        json!({"op": "add_track", "at": 2}),
        json!({"op": "delete_track", "track": ins1}),
        json!({"op": "add_pattern", "name": "Fill", "color": "#112233"}),
        json!({"op": "add_effect", "track": ins2, "plugin": "Fruity Limiter"}),
        json!({"op": "set_auto_bypass", "on": true}),
        json!({"op": "move_effect", "track": ins2, "slot": 0, "to": 4}),
        json!({"op": "set_slot", "track": ins2, "slot": 4, "name": "Limit", "color": "#204060"}),
    ] {
        round_trip(&mut a, c);
    }
    // The playlist: a clip added, edited and deleted.
    let r = round_trip(&mut a, json!({"op": "add_clip", "track": 3, "position": 384, "pattern": pat}));
    let clip = r[0]["clip"].as_u64().unwrap();
    round_trip(&mut a, json!({"op": "set_clip", "clip": clip, "position": 768, "track": 4, "length": 96, "muted": true}));
    round_trip(&mut a, json!({"op": "delete_clip", "clip": clip}));
    let pat2 = ids(&a).2.last().copied().unwrap();
    round_trip(&mut a, json!({"op": "delete_pattern", "pattern": pat2}));
    let slot = crate::control::query_ui::project(&a.state)["mixer"].as_array().unwrap().iter().flat_map(|t| t["slots"].as_array().unwrap().clone()).next().unwrap();
    let track = crate::control::query_ui::project(&a.state)["mixer"].as_array().unwrap().iter().find(|t| !t["slots"].as_array().unwrap().is_empty()).unwrap()["id"].as_u64().unwrap();
    round_trip(&mut a, json!({"op": "remove_effect", "track": track, "slot": slot["slot"]}));
}

/// A track added or deleted moves the others and makes or takes away the one before the hidden last track: undo and
/// redo put the tracks back with their arm switches, and with the lights they have now, a track that was not there
/// with the light it had, as the whole project's step did (a solo turns off every track).
#[test]
fn adding_or_deleting_a_track_keeps_the_lights() {
    let mut a = app();
    let tr = ids(&a).1;
    let n = a.state.session.project.mixer.len();
    for (i, m) in a.state.session.project.get_mut().mixer.iter_mut().enumerate() {
        (m.enabled, m.armed) = (i == 0 || i == 3 || i + 2 == n, i == 4);
    }
    // `to` with the lights of the tracks in `now` as they are there.
    let lit = |to: &[os_model::MixerTrack], now: &[os_model::MixerTrack]| -> Vec<os_model::MixerTrack> {
        to.iter().map(|m| os_model::MixerTrack { enabled: now.iter().find(|x| x.id.0 == m.id.0).map_or(m.enabled, |x| x.enabled), ..m.clone() }).collect()
    };
    for c in [json!({"op": "delete_track", "track": tr[2]}), json!({"op": "add_track", "at": 2})] {
        let before = a.state.session.project.mixer.clone();
        apply(&mut a.state, &test_client(), None, vec![cmd(c)]).unwrap();
        // A solo after it, and an arm switch.
        for m in a.state.session.project.get_mut().mixer.iter_mut().skip(1) {
            (m.enabled, m.armed) = (!m.enabled, !m.armed);
        }
        let after = a.state.session.project.mixer.clone();
        crate::history_ui::undo(&mut a.state).unwrap();
        same(&a.state.session.project.mixer, &lit(&before, &after));
        let undone = a.state.session.project.mixer.clone();
        crate::history_ui::redo(&mut a.state).unwrap();
        same(&a.state.session.project.mixer, &lit(&after, &undone));
    }
}

#[track_caller]
fn same<T: std::fmt::Debug + PartialEq>(a: &T, b: &T) {
    if a != b {
        let (x, y) = (format!("{a:?}"), format!("{b:?}"));
        let at = x.chars().zip(y.chars()).take_while(|(p, q)| p == q).count();
        let from = at.saturating_sub(300);
        panic!("differs at {at}:\n  now: ...{}\n  was: ...{}", &x[from..(at + 80).min(x.len())], &y[from..(at + 80).min(y.len())]);
    }
}

/// The pattern commands the PATTERNS menu and the pattern selector use.
#[test]
fn pattern_commands_undo_and_redo() {
    let mut a = app();
    let pat = ids(&a).2[0];
    let kick = ids(&a).0[0];
    round_trip(&mut a, json!({"op": "add_notes", "pattern": pat, "notes": [{"channel": kick, "position": 0, "length": 24, "key": 60}]}));
    for c in [
        json!({"op": "add_pattern", "name": "Twelve", "number": 12}),
        json!({"op": "shift_patterns", "from": 1, "by": 1}),
        json!({"op": "shift_patterns", "from": 2, "by": -1}),
        json!({"op": "move_pattern", "from": 1, "to": 5}),
        json!({"op": "clone_pattern", "pattern": pat}),
        json!({"op": "transpose_pattern", "pattern": pat, "semitones": 3}),
        json!({"op": "set_pattern_signature", "pattern": pat, "numerator": 3, "denominator": 4}),
        json!({"op": "set_pattern_signature", "pattern": pat, "numerator": 0, "denominator": 4}),
    ] {
        round_trip(&mut a, c);
    }
    let bad = apply(&mut a.state, &test_client(), None, vec![cmd(json!({"op": "add_pattern", "number": 12}))]).unwrap_err();
    assert!(bad.contains("there already"), "{bad}");
}

/// The rack's edits apply their commands, each one step that undo and redo take back and put again.
#[test]
fn rack_edits_are_one_step_each() {
    let mut a = app();
    let steps = |a: &App| a.state.session.history.done;
    let check = |a: &mut App, what: &str, edit: &dyn Fn(&mut App)| {
        let (before, n) = (a.state.session.project.channels.clone(), steps(a));
        edit(a);
        let after = a.state.session.project.channels.clone();
        assert_ne!(before, after, "{what} changed nothing");
        assert_eq!(steps(a), n + 1, "{what}: one step");
        crate::history_ui::undo(&mut a.state);
        assert_eq!(a.state.session.project.channels, before, "{what}: undo");
        crate::history_ui::redo(&mut a.state);
        assert_eq!(a.state.session.project.channels, after, "{what}: redo");
    };
    check(&mut a, "mute", &|a| crate::rack::toggle_mute(&mut a.state, 0));
    check(&mut a, "solo", &|a| crate::rack::solo(&mut a.state, 1));
    a.state.rack.sel = [0, 1].into_iter().collect();
    check(&mut a, "gradient", &|a| {
        crate::rack::gradient(&mut a.state);
    });
    check(&mut a, "swing", &|a| {
        crate::rack::swing_selected(&mut a.state, 0.5);
    });
    // Two channels on one track: the second one moves to a free one.
    let t = a.state.session.project.channels[0].mixer_track;
    a.state.session.project.get_mut().channels[1].mixer_track = t;
    check(&mut a, "free track", &|a| {
        crate::rack::run(a, crate::rack::RCmd::AssignFreeSel);
    });
    check(&mut a, "cut itself", &|a| {
        crate::rack::run(a, crate::rack::RCmd::CutItself(0));
    });
    check(&mut a, "mixer track", &|a| {
        let t = a.state.session.project.channels[0].mixer_track + 1;
        crate::rack::set_mixer_track(&mut a.state, 0, t, None);
    });
    // Clone, move, sort and delete: one step each.
    let rows = |a: &App| a.state.session.project.channels.iter().map(|c| (c.id.0, c.name.clone())).collect::<Vec<_>>();
    let shape = |a: &mut App, what: &str, edit: &dyn Fn(&mut App)| {
        let (before, n) = (rows(a), steps(a));
        edit(a);
        let after = rows(a);
        assert_ne!(before, after, "{what} changed nothing");
        assert_eq!(steps(a), n + 1, "{what}: one step");
        crate::history_ui::undo(&mut a.state);
        assert_eq!(rows(a), before, "{what}: undo");
        crate::history_ui::redo(&mut a.state);
        assert_eq!(rows(a).len(), after.len(), "{what}: redo");
    };
    shape(&mut a, "clone", &|a| crate::rack::clone_rows(a, &[0, 1]));
    assert_eq!(a.state.session.project.channels[1].name.as_deref(), Some("Kick #2"));
    a.state.rack.sel = [1].into_iter().collect();
    shape(&mut a, "move", &|a| {
        crate::rack::move_selected(a, 1);
    });
    shape(&mut a, "sort", &|a| {
        crate::rack::sort(a, crate::rack::SortBy::Name);
    });
    shape(&mut a, "delete", &|a| crate::rack::delete_step(a, &[1, 2]));
    // The lock makes no step.
    let n = steps(&a);
    crate::rack::lock(&mut a.state, 0);
    assert!(a.state.session.project.channels[0].rack.locked);
    assert_eq!(steps(&a), n);
}

/// A remote control link on a rack row follows the row a command moves, and goes back with the undo.
#[test]
fn links_follow_rows_moved_by_commands() {
    use crate::typein::Target;
    let mut a = app();
    let (ch, ..) = ids(&a);
    let st = a.state.session.settings.clone();
    crate::remote::set_links(&st, &[crate::remote::Link::new(0, 30, Target::RackLed(0))]);
    let on = |st: &crate::settings_file::SettingsStore| crate::remote::links(st)[0].target;
    apply(&mut a.state, &test_client(), None, vec![cmd(json!({"op": "move_channel", "channel": ch[0], "row": 2}))]).unwrap();
    assert_eq!(on(&st), Target::RackLed(2));
    crate::history_ui::undo(&mut a.state);
    assert_eq!(on(&st), Target::RackLed(0));
    crate::history_ui::redo(&mut a.state);
    assert_eq!(on(&st), Target::RackLed(2));
    crate::remote::set_links(&st, &[]);
}

#[test]
fn set_project_checks_its_values() {
    let mut a = app();
    let mut bad = |v: Value| apply(&mut a.state, &test_client(), None, vec![cmd(v)]).unwrap_err();
    assert!(bad(json!({"op": "set_project", "beats": 0})).contains("beats"));
    assert!(bad(json!({"op": "set_project", "steps": 17})).contains("steps"));
    assert!(bad(json!({"op": "set_project", "panning_law": "square"})).contains("panning_law"));
    // A bar value the project already has is kept, even out of the range.
    a.state.session.project.get_mut().bar = Some((40, 4));
    apply(&mut a.state, &test_client(), None, vec![cmd(json!({"op": "set_project", "beats": 40, "steps": 2}))]).unwrap();
    assert_eq!(a.state.session.project.bar(), (40, 2));
}

#[test]
fn new_things_name_their_ids() {
    let mut a = app();
    let r = apply(&mut a.state, &test_client(), None, vec![cmd(json!({"op": "add_channel", "name": "Bass"})), cmd(json!({"op": "add_pattern"}))]).unwrap();
    let ch = r[0]["channel"].as_u64().unwrap() as u32;
    let pat = r[1]["pattern"].as_u64().unwrap() as u32;
    let p = &a.state.session.project;
    assert_eq!(p.channels.last().map(|c| (c.id.0, c.name.clone())), Some((ch, Some("Bass".into()))));
    assert!(p.patterns.iter().any(|q| q.id.0 == pat));
    // The new channel's id names it in the next command.
    apply(&mut a.state, &test_client(), None, vec![cmd(json!({"op": "set_channel", "channel": ch, "volume": 0.1}))]).unwrap();
    assert_eq!(a.state.session.project.channels.last().unwrap().volume, 0.1);
}

#[test]
fn a_transaction_is_one_named_step_and_all_or_nothing() {
    let mut a = app();
    let kick = ids(&a).0[0];
    let before = reading(&a);
    let steps = a.state.session.history.at();
    let bad = vec![cmd(json!({"op": "set_tempo", "bpm": 90})), cmd(json!({"op": "set_channel", "channel": 9999, "volume": 0.5}))];
    let e = apply(&mut a.state, &test_client(), Some("Two things"), bad).unwrap_err();
    assert!(e.contains("command 1") && e.contains("no channel 9999"), "{e}");
    assert_eq!(reading(&a), before, "the first command is taken back");
    assert_eq!(a.state.session.history.at(), steps, "no step");

    let good = vec![cmd(json!({"op": "set_tempo", "bpm": 90})), cmd(json!({"op": "set_channel", "channel": kick, "volume": 0.5}))];
    apply(&mut a.state, &test_client(), Some("Build Drum Kit"), good).unwrap();
    assert_eq!(a.state.session.history.at(), steps + 1);
    let h = &a.state.session.history;
    let name = h.entries(&crate::settings_file::store()).into_iter().find(|e| e.0 == h.at()).map(|e| e.1.to_string());
    assert_eq!(name.as_deref(), Some("Test: build Drum Kit"), "named after the client");
    assert_eq!(crate::history_ui::undo(&mut a.state).as_deref(), Some("Test: build Drum Kit"));
    assert_eq!(reading(&a), before, "one undo takes back both");
    // Out of range values fail before anything changes.
    for c in [json!({"op": "set_tempo", "bpm": 900}), json!({"op": "set_channel", "channel": kick, "pan": 2}), json!({"op": "set_eq", "track": 1, "band": 3})] {
        assert!(apply(&mut a.state, &test_client(), None, vec![cmd(c.clone())]).is_err(), "{c}");
        assert_eq!(reading(&a), before);
    }
}

#[test]
fn the_interface_and_the_commands_share_one_history() {
    let mut a = app();
    let kick = ids(&a).0[0];
    crate::history::knob(&mut a.state.session, crate::knobs::KnobId::ChanVol(0), false);
    a.state.session.project.get_mut().channels[0].volume = 0.3;
    apply(&mut a.state, &test_client(), None, vec![cmd(json!({"op": "set_channel", "channel": kick, "volume": 0.6}))]).unwrap();
    assert_eq!(crate::history_ui::undo(&mut a.state).as_deref(), Some("Test: channel settings"));
    assert_eq!(a.state.session.project.channels[0].volume, 0.3);
    assert_eq!(crate::history_ui::undo(&mut a.state).as_deref(), Some("Channel volume"));
    assert_eq!(a.state.session.project.channels[0].volume, 0.78125);
    crate::history_ui::redo(&mut a.state);
    crate::history_ui::redo(&mut a.state);
    assert_eq!(a.state.session.project.channels[0].volume, 0.6);
}

#[test]
fn ids_stay_with_their_things() {
    let mut a = app();
    let (ch, ..) = ids(&a);
    apply(&mut a.state, &test_client(), None, vec![cmd(json!({"op": "delete_channel", "channel": ch[1]}))]).unwrap();
    // The channel below it moved up a row and kept its id; commands still find it.
    assert_eq!(a.state.session.project.channels[1].id.0, ch[2]);
    apply(&mut a.state, &test_client(), None, vec![cmd(json!({"op": "set_channel", "channel": ch[2], "name": "Still me"}))]).unwrap();
    assert_eq!(a.state.session.project.channels[1].name.as_deref(), Some("Still me"));
    // The interface's edits keep them too: a clone gets a new id, the original keeps its own.
    crate::rack::clone_rows(&mut a, &[0]);
    a.state.session.control.settle(a.state.session.project.get_mut());
    assert_eq!(a.state.session.project.channels[0].id.0, ch[0]);
    assert!(!ch.contains(&a.state.session.project.channels[1].id.0), "the clone's id is new");
    // A deleted channel's id names nothing; an undo brings it back with its id.
    let e = apply(&mut a.state, &test_client(), None, vec![cmd(json!({"op": "set_channel", "channel": ch[1], "volume": 0.5}))]).unwrap_err();
    assert!(e.contains("no channel"), "{e}");
}

#[test]
fn ids_are_not_an_unsaved_change() {
    let mut a = app();
    a.mark_saved();
    assert!(!a.unsaved());
    let first = a.state.session.project.channels[0].clone();
    a.state.session.project.get_mut().channels.push(first);
    a.state.session.control.settle(a.state.session.project.get_mut());
    a.state.session.project.get_mut().channels.pop();
    assert!(!a.unsaved(), "a copy's new id alone changes nothing");
}

#[test]
fn a_proposal_leaves_the_project_until_accepted() {
    let mut a = app();
    let kick = ids(&a).0[0];
    let before = reading(&a);
    let steps = a.state.session.history.at();
    let cmds = vec![cmd(json!({"op": "set_tempo", "bpm": 150})), cmd(json!({"op": "set_channel", "channel": kick, "name": "Boom"})), cmd(json!({"op": "add_channel", "name": "New"}))];
    let (n, changes) = propose(&mut a.state, &test_client(), "Faster", cmds).unwrap();
    assert_eq!(reading(&a), before, "nothing changed");
    assert_eq!(a.state.session.history.at(), steps);
    assert!(a.state.session.sample_loads.is_empty() && a.state.session.control.host.is_empty());
    let paths: Vec<&str> = changes.iter().map(|c| c.path.as_str()).collect();
    assert!(paths.contains(&"tempo"), "{paths:?}");
    assert!(paths.contains(&format!("channels.{kick}.name").as_str()), "{paths:?}");
    assert!(paths.iter().any(|p| p.starts_with("channels.") && changes.iter().any(|c| c.path == *p && c.before.is_null())), "the new channel: {paths:?}");
    assert_eq!(a.state.session.control.proposals.len(), 1);
    accept(&mut a.state, n).unwrap();
    assert_eq!(a.state.tempo(), 150.0);
    assert_eq!(a.state.session.project.channels[0].name.as_deref(), Some("Boom"));
    assert_eq!(a.state.session.history.at(), steps + 1);
    assert!(a.state.session.control.proposals.is_empty());
    assert!(accept(&mut a.state, n).is_err(), "only once");
    let (n, _) = propose(&mut a.state, &test_client(), "Slower", vec![cmd(json!({"op": "set_tempo", "bpm": 60}))]).unwrap();
    discard(&mut a.state.session, n).unwrap();
    assert_eq!(a.state.tempo(), 150.0);
}

#[test]
fn proposals_ask_the_user() {
    let mut a = app();
    a.state.dialogs.clear();
    let mut c = rpc::Client { name: "Helper".into(), level: Level::Propose, saves: false, token: None };
    let r = a.rpc(&mut c, "propose", &json!({"name": "Faster", "commands": [{"op": "set_tempo", "bpm": 150}]})).ok().unwrap();
    let n = r["proposal"].as_u64().unwrap();
    assert_eq!(a.state.dialogs.len(), 1, "a box asks");
    // Apply.
    a.dialog_done(crate::dialog::Then::Proposal(n), crate::dialog::Reply::Button(crate::dialog::Answer::Yes));
    assert_eq!(a.state.tempo(), 150.0);
    // A client that may only propose can't accept or apply.
    assert!(a.rpc(&mut c, "apply", &json!({"command": {"op": "set_tempo", "bpm": 100}})).is_err());
    assert!(a.rpc(&mut c, "accept", &json!({"proposal": n})).is_err());
    assert_eq!(a.state.tempo(), 150.0);
}

#[test]
fn events_tell_clients_what_changed() {
    let mut a = app();
    a.state.session.control.watched = true;
    let (_, last) = a.state.session.control.events_since(0);
    apply(&mut a.state, &test_client(), Some("Faster"), vec![cmd(json!({"op": "set_tempo", "bpm": 150}))]).unwrap();
    crate::history_ui::undo(&mut a.state);
    let (ev, newest) = a.state.session.control.events_since(last);
    assert_eq!(ev.len(), 1);
    assert_eq!((ev[0].kind, ev[0].name.as_str(), ev[0].ops.clone(), ev[0].by.as_str()), ("apply", "Faster", vec!["set_tempo"], "Test"));
    assert_eq!(newest, ev[0].seq);
    // An edit in the interface is an event on the next tick.
    a.state.session.project.get_mut().channels[0].volume = 0.1;
    a.state.session.dirty = true;
    a.tick();
    let (ev, _) = a.state.session.control.events_since(newest);
    assert_eq!(ev.iter().map(|e| (e.kind, e.by.as_str())).collect::<Vec<_>>(), [("edit", "user")]);
}

#[test]
fn rpc_requests_and_errors() {
    let mut a = app();
    let mut c = rpc::Client { name: "Client 1".into(), level: Level::Edit, saves: false, token: None };
    let call = |a: &mut App, c: &mut rpc::Client, req: Value| -> Value { serde_json::from_str(&a.rpc_line(c, &req.to_string()).unwrap()).unwrap() };
    let r = call(&mut a, &mut c, json!({"jsonrpc": "2.0", "id": 1, "method": "hello", "params": {"name": "Assistant"}}));
    assert_eq!((r["id"].clone(), r["result"]["protocol"].clone(), r["result"]["level"].clone()), (json!(1), json!(rpc::PROTOCOL), json!("edit")));
    assert_eq!(c.name, "Assistant");
    let st = call(&mut a, &mut c, json!({"jsonrpc": "2.0", "id": 2, "method": "state"}));
    let kick = st["result"]["channels"][0]["id"].as_u64().unwrap();
    assert_eq!(st["result"]["channels"][0]["name"], "Kick");
    let r = call(&mut a, &mut c, json!({"jsonrpc": "2.0", "id": 3, "method": "apply", "params": {"name": "Louder kick", "commands": [{"op": "set_channel", "channel": kick, "volume": 1.0}]}}));
    assert!(r.get("error").is_none(), "{r}");
    assert_eq!(a.state.session.project.channels[0].volume, 1.0);
    let h = call(&mut a, &mut c, json!({"jsonrpc": "2.0", "id": 4, "method": "history"}));
    assert!(h["result"]["steps"].as_array().unwrap().iter().any(|s| s["name"] == "Assistant: louder kick"), "{h}");
    let r = call(&mut a, &mut c, json!({"jsonrpc": "2.0", "id": 5, "method": "undo"}));
    assert_eq!(r["result"]["step"], "Assistant: louder kick");
    assert_eq!(a.state.session.project.channels[0].volume, 0.78125);
    // Errors: the codes of JSON-RPC 2.0, and -32000 / -32001 for a failed or refused request.
    let code = |r: Value| r["error"]["code"].as_i64();
    assert_eq!(code(serde_json::from_str(&a.rpc_line(&mut c, "{not json").unwrap()).unwrap()), Some(-32700));
    assert_eq!(code(call(&mut a, &mut c, json!({"id": 6, "method": "state"}))), Some(-32600));
    assert_eq!(code(call(&mut a, &mut c, json!({"jsonrpc": "2.0", "id": 7, "method": "nope"}))), Some(-32601));
    assert_eq!(code(call(&mut a, &mut c, json!({"jsonrpc": "2.0", "id": 8, "method": "apply", "params": {"command": {"op": "fly"}}}))), Some(-32602));
    assert_eq!(code(call(&mut a, &mut c, json!({"jsonrpc": "2.0", "id": 9, "method": "apply", "params": {"command": {"op": "delete_channel", "channel": 9999}}}))), Some(-32000));
    let mut reader = rpc::Client { name: "Reader".into(), level: Level::Read, saves: false, token: None };
    assert_eq!(code(call(&mut a, &mut reader, json!({"jsonrpc": "2.0", "id": 10, "method": "apply", "params": {"command": {"op": "set_tempo", "bpm": 99}}}))), Some(-32001));
    assert_eq!(a.state.tempo(), 130.0);
    // A notification (no id) gets no answer.
    assert!(a.rpc_line(&mut c, &json!({"jsonrpc": "2.0", "method": "undo"}).to_string()).is_none());
    // Notes read back as commands write them.
    let pat = a.state.session.project.patterns[0].id.0;
    call(&mut a, &mut c, json!({"jsonrpc": "2.0", "id": 11, "method": "apply", "params": {"command": {"op": "set_notes", "pattern": pat, "notes": [{"channel": kick, "position": 0, "length": 48, "key": 36, "velocity": 90, "pan": 64}]}}}));
    let n = call(&mut a, &mut c, json!({"jsonrpc": "2.0", "id": 12, "method": "notes", "params": {"pattern": pat}}));
    let id = n["result"]["notes"][0]["id"].clone();
    assert!(id.as_u64().is_some_and(|i| i > 0), "the note has an id");
    assert_eq!(n["result"]["notes"], json!([{"id": id, "channel": kick, "position": 0, "length": 48, "key": 36, "velocity": 90, "pan": 64}]));
}

/// Notes a command makes name their ids in its result, the notes query shows them, and writing the notes back keeps
/// them; `add_notes` takes no id.
#[test]
fn notes_name_their_ids() {
    let mut a = app();
    let (ch, _, pa) = ids(&a);
    let s = &mut a.state.session;
    s.control.settle(s.project.get_mut());
    let r = apply(&mut a.state, &test_client(), None, vec![cmd(json!({"op": "add_notes", "pattern": pa[0], "notes": [{"channel": ch[0], "position": 0}, {"channel": ch[1], "position": 96}]}))]).unwrap();
    let made: Vec<u32> = r[0]["notes"].as_array().unwrap().iter().map(|v| v.as_u64().unwrap() as u32).collect();
    assert_eq!(made.len(), 2);
    assert!(made.iter().all(|&i| i != 0) && made[0] != made[1]);
    let q = |a: &App| crate::control::query::notes(&a.state.session, pa[0]).unwrap()["notes"].clone();
    let listed: Vec<u32> = q(&a).as_array().unwrap().iter().map(|n| n["id"].as_u64().unwrap() as u32).collect();
    assert!(made.iter().all(|i| listed.contains(i)), "the query names them");
    // Read back and written again (one velocity changed): the same ids.
    let mut notes = q(&a);
    notes[0]["velocity"] = json!(50);
    apply(&mut a.state, &test_client(), None, vec![cmd(json!({"op": "set_notes", "pattern": pa[0], "notes": notes}))]).unwrap();
    let again: Vec<u32> = q(&a).as_array().unwrap().iter().map(|n| n["id"].as_u64().unwrap() as u32).collect();
    assert_eq!(again, listed);
    assert!(apply(&mut a.state, &test_client(), None, vec![cmd(json!({"op": "add_notes", "pattern": pa[0], "notes": [{"channel": ch[0], "position": 0, "id": made[0]}]}))]).is_err());
}

#[test]
fn hosted_plugins_follow_deletes_and_undo() {
    let mut a = app();
    a.state.session.plugins = crate::plugins::Plugins::default();
    a.state.session.plugins.available = crate::plugins::builtin();
    a.plugin_tick();
    a.plugin_tick();
    let (ch, ..) = ids(&a);
    let rows = |a: &App| a.state.session.plugins.hosted.range(..crate::plugins::FX_KEY).map(|(&r, h)| (r, h.owner.0)).collect::<Vec<_>>();
    assert_eq!(rows(&a), ch.iter().copied().enumerate().collect::<Vec<_>>(), "each sampler hosted, owned by its channel");
    let mut c = rpc::Client { name: "Test".into(), level: Level::Edit, saves: false, token: None };
    // A parameter set through the control plane, then its channel deleted and brought back.
    let params = a.rpc(&mut c, "params", &json!({"owner": ch[1]})).ok().unwrap();
    let p = params["params"].as_array().unwrap().iter().find(|p| p["steps"] == 0 && p["read_only"] == false && p["value"].as_f64().is_some_and(|v| v < 0.9)).unwrap().clone();
    let pid = p["id"].as_u64().unwrap();
    a.rpc(&mut c, "apply", &json!({"command": {"op": "set_param", "owner": ch[1], "param": pid, "value": 0.9}})).ok().unwrap();
    a.plugin_tick();
    let value = |a: &App, row: usize| a.state.session.plugins.hosted[&row].plugin.lock().unwrap().param_value(pid as u32).unwrap();
    assert!((value(&a, 1) - 0.9).abs() < 1e-6);
    a.rpc(&mut c, "apply", &json!({"command": {"op": "delete_channel", "channel": ch[1]}})).ok().unwrap();
    a.plugin_tick();
    let want: Vec<(usize, u32)> = [ch[0], ch[2], ch[3], ch[4]].into_iter().enumerate().collect();
    assert_eq!(rows(&a), want, "the rows below moved up with their instances");
    assert_eq!(a.state.session.hosted_rows.iter().copied().collect::<std::collections::BTreeSet<_>>(), (0..4).collect());
    a.rpc(&mut c, "undo", &Value::Null).ok().unwrap();
    a.plugin_tick();
    assert_eq!(rows(&a), ch.iter().copied().enumerate().collect::<Vec<_>>(), "back on its row");
    assert!((value(&a, 1) - 0.9).abs() < 1e-6, "with the state it had");
    // The parameter's own undo puts the value before back.
    a.rpc(&mut c, "undo", &Value::Null).ok().unwrap();
    a.plugin_tick();
    assert!((value(&a, 1) - p["value"].as_f64().unwrap()).abs() < 1e-6);
    a.rpc(&mut c, "redo", &Value::Null).ok().unwrap();
    a.plugin_tick();
    assert!((value(&a, 1) - 0.9).abs() < 1e-6);
}

#[test]
fn an_effect_added_by_a_command_is_hosted_without_a_window() {
    let mut a = app();
    a.state.session.plugins = crate::plugins::Plugins::default();
    a.state.session.plugins.effects = crate::plugins::builtin_effects();
    let Some(name) = a.state.session.plugins.effects.first().map(|e| e.desc.name.clone()) else { return };
    let ins1 = ids(&a).1[1];
    let r = apply(&mut a.state, &test_client(), None, vec![cmd(json!({"op": "add_effect", "track": ins1, "slot": 2, "plugin": name}))]).unwrap();
    let slot = r[0]["slot"].as_u64().unwrap() as u32;
    a.plugin_tick();
    let key = crate::plugins::fx_key(1, 2);
    assert_eq!(a.state.session.plugins.hosted.get(&key).map(|h| h.owner.0), Some(slot));
    #[cfg(plugin_windows)]
    assert!(a.state.editor_host.editors.get(&key).is_none_or(|e| e.window.is_none()));
    crate::history_ui::undo(&mut a.state);
    a.plugin_tick();
    assert!(!a.state.session.plugins.hosted.contains_key(&key), "undone: the instance goes");
    crate::history_ui::redo(&mut a.state);
    a.plugin_tick();
    assert_eq!(a.state.session.plugins.hosted.get(&key).map(|h| h.owner.0), Some(slot), "redone: back from what it kept");
}

#[cfg(all(unix, feature = "rpc"))]
#[test]
fn clients_connect_over_a_local_socket() {
    use std::io::{BufRead, BufReader, Write};
    let path = std::env::temp_dir().join(format!("lemon-control-{}.sock", std::process::id()));
    let mut a = app();
    a.listen(server::start(path.to_str().unwrap(), Level::Edit, || {}).unwrap());
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600, "this user only");
    let s = std::os::unix::net::UnixStream::connect(&path).unwrap();
    let mut w = s.try_clone().unwrap();
    let mut r = BufReader::new(s);
    let mut ask = |a: &mut App, req: Value| -> Value {
        writeln!(w, "{req}").unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let mut line = String::new();
        // The UI thread answers on its ticks.
        std::thread::scope(|sc| {
            sc.spawn(|| {
                r.read_line(&mut line).unwrap();
                tx.send(()).unwrap();
            });
            while rx.try_recv().is_err() {
                a.control_tick();
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
        });
        serde_json::from_str(&line).unwrap()
    };
    let r1 = ask(&mut a, json!({"jsonrpc": "2.0", "id": 1, "method": "hello", "params": {"name": "Socket"}}));
    assert_eq!(r1["result"]["name"], "Socket");
    let r2 = ask(&mut a, json!({"jsonrpc": "2.0", "id": 2, "method": "apply", "params": {"command": {"op": "set_tempo", "bpm": 111}}}));
    assert!(r2.get("error").is_none(), "{r2}");
    assert_eq!(a.state.tempo(), 111.0);
    let h = &a.state.session.history;
    assert_eq!(h.entries(&crate::settings_file::store()).into_iter().find(|e| e.0 == h.at()).map(|e| e.1.to_string()).as_deref(), Some("Socket: tempo"), "the client's name stays with its connection");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn command_line_flags() {
    let mut args: Vec<String> = ["song.flp", "--control", "/tmp/x.sock", "--control-level", "propose"].map(String::from).to_vec();
    let f = server::Flags::take(&mut args).unwrap();
    assert_eq!((f.control.as_deref(), f.serve, f.level), (Some("/tmp/x.sock"), None, Some(Level::Propose)));
    assert_eq!(args, ["song.flp"], "the project stays");
    let mut args: Vec<String> = ["--serve", "7777"].map(String::from).to_vec();
    assert_eq!(server::Flags::take(&mut args).unwrap().serve.as_deref(), Some("7777"));
    for bad in [&["--control"][..], &["--control-level", "root"], &["--control", "a", "--serve", "b"], &["--serve", "--control-level"]] {
        let mut args: Vec<String> = bad.iter().map(|s| s.to_string()).collect();
        assert!(server::Flags::take(&mut args).is_err(), "{bad:?}");
    }
}

#[test]
fn saving_asks_the_user_unless_headless() {
    let mut a = app();
    let path = std::env::temp_dir().join(format!("lemon-control-save-{}.flp", std::process::id()));
    let mut c = rpc::Client { name: "Helper".into(), level: Level::Edit, saves: false, token: None };
    a.state.dialogs.clear();
    let r = a.rpc(&mut c, "save", &json!({"path": path})).ok().unwrap();
    assert!(r.get("asked").is_some() && !path.exists() && a.state.dialogs.len() == 1, "{r}");
    a.dialog_done(crate::dialog::Then::ClientSave(path.clone()), crate::dialog::Reply::Button(crate::dialog::Answer::Yes));
    assert!(path.exists(), "saved once the user said yes");
    let _ = std::fs::remove_file(&path);
    let mut c = rpc::Client { name: "Script".into(), level: Level::Edit, saves: true, token: None };
    let r = a.rpc(&mut c, "save", &json!({"path": path})).ok().unwrap();
    assert_eq!(r["saved"], json!(path.display().to_string()));
    let saved = os_flp::project::load(&os_flp::FlpFile::parse(&std::fs::read(&path).unwrap()).unwrap()).unwrap();
    assert_eq!(saved.channels.len(), a.state.session.project.channels.len());
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_first_clip_makes_the_playlist_and_its_undo_takes_it_away() {
    let mut a = app();
    a.state.session.project.get_mut().arrangements.clear();
    let pat = ids(&a).2[0];
    let r = round_trip(&mut a, json!({"op": "add_clip", "track": 1, "position": 0, "pattern": pat}));
    assert!(r[0]["clip"].as_u64().is_some());
    crate::history_ui::undo(&mut a.state);
    assert!(a.state.session.project.arrangements.is_empty());
}

/// Does `f` (an edit in the interface) and checks it is one undo step named `name` that undo and redo take back and
/// give again exactly.
fn ui_round_trip(a: &mut App, name: &str, f: impl FnOnce(&mut App)) {
    let before = reading(a);
    let steps = a.state.session.history.at();
    f(a);
    a.state.session.control.settle(a.state.session.project.get_mut());
    let after = reading(a);
    assert_ne!(before, after, "{name} changes something");
    assert_eq!(a.state.session.history.at(), steps + 1, "{name} is one step");
    assert_eq!(crate::history_ui::undo(&mut a.state).as_deref(), Some(name));
    let now = reading(a);
    assert!(now == before, "{name} undone: {:?}, the model alike: {}", query::changes(&before.1, &now.1), now.0 == before.0);
    assert_eq!(crate::history_ui::redo(&mut a.state).as_deref(), Some(name));
    assert_eq!(reading(a), after, "{name} redone");
}

#[test]
fn the_mixer_s_edits_are_undoable() {
    use crate::mixer::{hit, run, MCmd, MHit};
    let mut a = app();
    ui_round_trip(&mut a, "Mixer track polarity", |a| {
        hit(a, MHit::Polarity(3));
    });
    ui_round_trip(&mut a, "Effect slot", |a| {
        a.state.selected_mixer = 1;
        hit(a, MHit::SlotLed(4));
    });
    ui_round_trip(&mut a, "Mixer track color", |a| {
        run(a, MCmd::Color(2, Some(os_model::Rgb::new(1, 2, 3))));
    });
    ui_round_trip(&mut a, "Mixer routing", |a| {
        a.state.selected_mixer = 1;
        a.state.mixer.sel.clear();
        run(a, MCmd::Route { dst: 3, only: true, side: false });
    });
    ui_round_trip(&mut a, "Move mixer track", |a| {
        run(a, MCmd::MoveRight(1));
    });
    ui_round_trip(&mut a, "Delete mixer track", |a| {
        run(a, MCmd::Delete(2));
    });
    ui_round_trip(&mut a, "Insert mixer track", |a| {
        run(a, MCmd::InsertOne(2));
    });
    ui_round_trip(&mut a, "Mixer track name", |a| {
        crate::playlist::start_edit(&mut a.state, crate::playlist::EditTarget::MixerName(1), (0.0, 0.0));
        a.state.playlist.edit.as_mut().unwrap().text = "Drums".into();
        crate::playlist::commit_edit(&mut a.state);
    });
    ui_round_trip(&mut a, "Mixer track reset", |a| {
        run(a, MCmd::Reset(1));
    });
    crate::edit::mixer::track_mut(&mut a.state.session, 3).unwrap().routes = vec![5];
    ui_round_trip(&mut a, "Mixer routing", |a| {
        a.state.mixer.sel.clear();
        run(a, MCmd::ResetRouting(3));
    });
    ui_round_trip(&mut a, "Delete mixer track", |a| {
        run(a, MCmd::Trim { non_empty: false });
    });
    ui_round_trip(&mut a, crate::history::MIXER_TRACK_IO, |a| {
        run(a, MCmd::Monitor(2, os_model::MONITOR_ON));
    });
    ui_round_trip(&mut a, crate::mixer::LATENCY, |a| {
        crate::mixer::set_latency(&mut a.state, 2, false, 20.0);
    });
    // Effect slots: added, moved, named, removed.
    a.state.session.plugins.effects = vec![crate::plugin_db::fake("Test FX", os_host::Kind::Effect)];
    a.state.selected_mixer = 2;
    a.state.mixer.sel.clear();
    ui_round_trip(&mut a, "Add effect", |a| {
        run(a, MCmd::AddEffect(0, 0));
    });
    run(&mut a, MCmd::AddEffect(0, 0));
    a.state.session.control.settle(a.state.session.project.get_mut());
    ui_round_trip(&mut a, "Move effect", |a| {
        run(a, MCmd::SlotMove(0, 1));
    });
    // A slot's name is kept beside the project (the reading does not show it).
    crate::playlist::start_edit(&mut a.state, crate::playlist::EditTarget::SlotName(2, 1), (0.0, 0.0));
    a.state.playlist.edit.as_mut().unwrap().text = "Glue".into();
    crate::playlist::commit_edit(&mut a.state);
    assert_eq!(crate::edit::mixer::slot_name(&a.state.session, 2, 1).as_deref(), Some("Glue"));
    assert_eq!(crate::history_ui::undo(&mut a.state).as_deref(), Some("Slot name"));
    assert_eq!(crate::edit::mixer::slot_name(&a.state.session, 2, 1).as_deref(), Some("Test FX"));
    assert_eq!(crate::history_ui::redo(&mut a.state).as_deref(), Some("Slot name"));
    assert_eq!(crate::edit::mixer::slot_name(&a.state.session, 2, 1).as_deref(), Some("Glue"));
    ui_round_trip(&mut a, "Remove effect", |a| {
        run(a, MCmd::SlotDelete(1));
    });
    // A fader drag is one step; a press that doesn't move it is none.
    let steps = a.state.session.history.at();
    crate::control::edit_press(&mut a.state.session, "Mixer track volume", command::Region::Track(1));
    crate::history::settle(&mut a.state.session);
    assert_eq!(a.state.session.history.at(), steps, "a click without a drag");
    ui_round_trip(&mut a, "Mixer track volume", |a| {
        crate::control::edit_press(&mut a.state.session, "Mixer track volume", command::Region::Track(1));
        a.state.session.project.get_mut().mixer[1].volume = Some(0.3);
        crate::history::settle(&mut a.state.session);
    });
}

#[test]
fn the_rack_s_and_patterns_edits_are_undoable() {
    use crate::rack::{run, RCmd};
    let mut a = app();
    ui_round_trip(&mut a, "Channel color", |a| {
        run(a, RCmd::RandomColor(1));
    });
    ui_round_trip(&mut a, "Clone channel", |a| {
        run(a, RCmd::Clone(0));
    });
    ui_round_trip(&mut a, "Move channel", |a| {
        a.state.rack.select_only(2);
        run(a, RCmd::MoveSel(-1));
    });
    ui_round_trip(&mut a, "Delete channel", |a| {
        crate::rack::delete_step(a, &[3]);
    });
    ui_round_trip(&mut a, "Channel name", |a| {
        crate::rack::open_box(&mut a.state, crate::rack::RBox::Channel(0), (0.0, 0.0));
        a.state.pat_ui.name_box.as_mut().unwrap().text = "Boom".into();
        crate::patterns::accept(&mut a.state);
    });
    crate::messages::set(&crate::messages::DELETE_PATTERN, false);
    ui_round_trip(&mut a, "Clone pattern", |a| {
        crate::patterns::run(&mut a.state, crate::patterns::Cmd::Clone, (0.0, 0.0));
    });
    ui_round_trip(&mut a, "Delete pattern", |a| {
        crate::patterns::run(&mut a.state, crate::patterns::Cmd::Delete, (0.0, 0.0));
    });
    crate::messages::set(&crate::messages::DELETE_PATTERN, true);
}

#[test]
fn a_deleted_channel_s_instrument_comes_back_with_an_undo() {
    let mut a = app();
    a.state.session.plugins = crate::plugins::Plugins::default();
    a.state.session.plugins.available = crate::plugins::builtin();
    a.plugin_tick();
    a.plugin_tick();
    let ch = ids(&a).0;
    let key = |a: &App, id: u32| a.state.session.plugins.hosted.iter().find(|(_, h)| h.owner.0 == id).map(|(&k, _)| k);
    crate::rack::delete_step(&mut a, &[1]);
    a.plugin_tick();
    assert_eq!(key(&a, ch[1]), None);
    assert_eq!(key(&a, ch[2]), Some(1));
    crate::history_ui::undo(&mut a.state);
    a.plugin_tick();
    assert_eq!((0..5).map(|r| key(&a, ch[r])).collect::<Vec<_>>(), (0..5).map(Some).collect::<Vec<_>>());
    assert_eq!(a.state.session.hosted_rows.iter().copied().collect::<std::collections::BTreeSet<_>>(), (0..5).collect());
}

#[cfg(unix)]
#[test]
fn a_file_in_the_socket_s_place_is_left_alone() {
    let path = std::env::temp_dir().join(format!("lemon-control-file-{}.flp", std::process::id()));
    std::fs::write(&path, b"a project").unwrap();
    assert!(server::start(path.to_str().unwrap(), Level::Edit, || {}).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"a project");
    let _ = std::fs::remove_file(&path);
}

#[cfg(feature = "rpc")]
#[test]
fn clients_connect_over_a_local_port() {
    use std::io::{BufRead, BufReader, Write};
    let mut a = app();
    let server = server::start("0", Level::Read, || {}).unwrap();
    let (at, token) = (server.at.clone(), server.token.clone().unwrap());
    assert!(at.starts_with("127.0.0.1:") && token.len() == 32, "{at} {token}");
    a.listen(server);
    let s = std::net::TcpStream::connect(&at).unwrap();
    let mut w = s.try_clone().unwrap();
    let mut r = BufReader::new(s);
    let mut ask = |a: &mut App, req: &str| -> String {
        writeln!(w, "{req}").unwrap();
        let mut line = String::new();
        std::thread::scope(|sc| {
            let (tx, rx) = std::sync::mpsc::channel();
            let (r, line) = (&mut r, &mut line);
            sc.spawn(move || {
                r.read_line(line).unwrap();
                tx.send(()).unwrap();
            });
            while rx.try_recv().is_err() {
                a.control_tick();
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
        });
        line
    };
    let code = |l: &str| serde_json::from_str::<Value>(l).unwrap()["error"]["code"].as_i64();
    let apply = json!({"jsonrpc": "2.0", "id": 1, "method": "apply", "params": {"command": {"op": "set_tempo", "bpm": 99}}}).to_string();
    assert_eq!(code(&ask(&mut a, &apply)), Some(-32001), "nothing before the token");
    let bad = json!({"jsonrpc": "2.0", "id": 2, "method": "hello", "params": {"token": "guess"}}).to_string();
    assert_eq!(code(&ask(&mut a, &bad)), Some(-32001));
    let hello = json!({"jsonrpc": "2.0", "id": 3, "method": "hello", "params": {"name": "Port", "token": token}}).to_string();
    assert_eq!(code(&ask(&mut a, &hello)), None);
    assert_eq!(code(&ask(&mut a, &apply)), Some(-32001), "a read-only client");
    assert_eq!(a.state.tempo(), 130.0);
    // A web page's request: one parse error, then the connection is closed.
    assert_eq!(code(&ask(&mut a, "POST / HTTP/1.1")), Some(-32700));
    let mut rest = String::new();
    assert_eq!(r.read_line(&mut rest).unwrap(), 0, "closed");
}

#[test]
fn a_replaced_generator_stays_replaced() {
    let mut a = app();
    a.state.session.plugins = crate::plugins::Plugins::default();
    a.state.session.plugins.available = crate::plugins::builtin();
    a.plugin_tick();
    a.plugin_tick();
    let trio = a.state.session.plugins.available.iter().position(|p| p.desc.name == crate::osc3::LEMON_TRIO).unwrap();
    crate::plugin_menu::replace_generator(&mut a, trio, 0);
    for _ in 0..3 {
        a.plugin_tick();
    }
    assert_eq!(a.state.session.plugins.descriptor(0).map(|d| d.name), Some(crate::osc3::LEMON_TRIO.to_string()), "the old sampler is not hosted again");
    assert_eq!(a.state.session.hosted_rows.iter().filter(|&&r| r == 0).count(), 1);
    // And back: Lemon Sampler hosts the channel, not the Lemon Trio it replaced.
    let sampler = a.state.session.plugins.available.iter().position(|p| p.desc.name == crate::plugins::LEMON_SAMPLER).unwrap();
    crate::plugin_menu::replace_generator(&mut a, sampler, 0);
    for _ in 0..3 {
        a.plugin_tick();
    }
    assert_eq!(a.state.session.plugins.descriptor(0).map(|d| d.name), Some(crate::plugins::LEMON_SAMPLER.to_string()));
}

#[test]
fn a_dry_run_leaves_the_instruments_and_the_selections() {
    let mut a = app();
    a.state.session.plugins = crate::plugins::Plugins::default();
    a.state.session.plugins.available = crate::plugins::builtin();
    a.plugin_tick();
    a.plugin_tick();
    let ch = ids(&a).0;
    a.state.rack.select_only(1);
    a.state.selected_channel = 1;
    let live = a.state.session.plugins.hosted[&1].plugin.clone();
    let rows = a.state.session.hosted_rows.clone();
    propose(&mut a.state, &test_client(), "Fewer", vec![cmd(json!({"op": "delete_channel", "channel": ch[1]}))]).unwrap();
    assert_eq!((a.state.selected_channel, a.state.rack.sel.iter().copied().collect::<Vec<_>>()), (1, vec![1]));
    assert_eq!(a.state.session.hosted_rows, rows);
    a.plugin_tick();
    a.plugin_tick();
    assert!(std::sync::Arc::ptr_eq(&a.state.session.plugins.hosted[&1].plugin, &live), "the same instance plays the channel");
    // A transaction that fails half way leaves them too.
    let bad = vec![cmd(json!({"op": "delete_channel", "channel": ch[1]})), cmd(json!({"op": "set_tempo", "bpm": 1}))];
    assert!(apply(&mut a.state, &test_client(), None, bad).is_err());
    a.plugin_tick();
    assert!(std::sync::Arc::ptr_eq(&a.state.session.plugins.hosted[&1].plugin, &live));
    assert_eq!(a.state.session.hosted_rows, rows);
}

#[test]
fn slot_names_come_back_with_their_track() {
    let mut a = app();
    crate::edit::mixer::set_slot_label(&mut a.state.session, 3, 1, Some("Crunch".into()));
    crate::mixer::run(&mut a, crate::mixer::MCmd::Delete(2));
    assert_eq!(crate::mixer::slot_names_of(&a.state.session.project).get(&(2, 1)).map(String::as_str), Some("Crunch"), "moved left with its track");
    crate::history_ui::undo(&mut a.state);
    assert_eq!(crate::mixer::slot_names_of(&a.state.session.project).get(&(3, 1)).map(String::as_str), Some("Crunch"));
    assert_eq!(crate::mixer::slot_names_of(&a.state.session.project).len(), 1);
}

#[test]
fn ticks_past_the_last_are_refused() {
    let mut a = app();
    let pat = ids(&a).2[0];
    let before = reading(&a);
    for c in [
        json!({"op": "add_clip", "track": 1, "position": u32::MAX, "pattern": pat}),
        json!({"op": "add_clip", "track": 1, "position": 0, "length": u32::MAX, "pattern": pat}),
        json!({"op": "set_pattern", "pattern": pat, "name": "Nope", "length": u32::MAX}),
        json!({"op": "add_notes", "pattern": pat, "notes": [{"channel": ids(&a).0[0], "position": u32::MAX}]}),
    ] {
        assert!(apply(&mut a.state, &test_client(), None, vec![cmd(c.clone())]).is_err(), "{c}");
        assert_eq!(reading(&a), before, "{c} changed nothing");
    }
}

/// A plugin with two parameters (ids 7 and 8) whose window's edits a test sends (`events`); its state is the two values.
struct Knob {
    desc: os_host::Descriptor,
    value: [f64; 2],
    events: Vec<os_host::ParamEvent>,
}

impl os_engine::Instrument for Knob {
    fn note_on(&mut self, _: u32, _: u8, _: f32) {}
    fn note_off(&mut self, _: u32, _: u8) {}
    fn process(&mut self, _: u32, _: &mut [f32], _: &mut [f32]) {}
}

impl os_engine::HostedEffect for Knob {
    fn process_fx(&mut self, _: u32, _: &mut [f32], _: &mut [f32]) {}
}

impl os_host::Plugin for Knob {
    fn desc(&self) -> &os_host::Descriptor {
        &self.desc
    }
    fn activate(&mut self, _: u32) -> bool {
        true
    }
    fn idle(&mut self) {}
    fn has_editor(&self) -> bool {
        false
    }
    fn editor_open(&self) -> bool {
        false
    }
    fn open_editor(&mut self, _: *mut std::ffi::c_void, _: f32) -> Result<(u32, u32), String> {
        Err(String::new())
    }
    fn editor_size(&self) -> Option<(u32, u32)> {
        None
    }
    fn close_editor(&mut self) {}
    fn save_state(&self) -> Option<Vec<u8>> {
        Some(knob_state(self.value))
    }
    fn load_state(&mut self, b: &[u8]) -> bool {
        let Some(v) = b.get(..16) else { return false };
        self.value = [0, 8].map(|i| f64::from_le_bytes(v[i..i + 8].try_into().unwrap()));
        true
    }
    fn param_count(&self) -> usize {
        2
    }
    fn param_info(&self, i: usize) -> Option<os_host::ParamInfo> {
        (i < 2).then(|| os_host::ParamInfo {
            id: 7 + i as u32,
            name: ["Cutoff", "Resonance"][i].into(),
            unit: String::new(),
            group: String::new(),
            min: 0.0,
            max: 1.0,
            default: 0.5,
            steps: 0,
            automatable: true,
            read_only: false,
            hidden: false,
            bypass: false,
            enumerated: false,
        })
    }
    fn param_value(&self, id: u32) -> Option<f64> {
        self.value.get(id.checked_sub(7)? as usize).copied()
    }
    fn set_param(&mut self, id: u32, v: f64) -> bool {
        let Some(x) = id.checked_sub(7).and_then(|i| self.value.get_mut(i as usize)) else { return false };
        *x = v;
        true
    }
    fn take_param_events(&mut self) -> Vec<os_host::ParamEvent> {
        std::mem::take(&mut self.events)
    }
}

#[test]
fn edits_in_a_plugin_s_window_are_undo_steps() {
    use os_host::ParamEvent::{Begin, End, Value};
    let mut a = app();
    a.state.session.plugins = crate::plugins::Plugins::default();
    let desc = crate::plugins::builtin()[0].desc.clone();
    let knob = std::sync::Arc::new(std::sync::Mutex::new(Knob { desc, value: [0.25, 0.5], events: Vec::new() }));
    let mut h = crate::plugins::Hosted::new(knob.clone());
    h.owner = a.state.session.project.channels[0].id;
    a.state.session.plugins.hosted.insert(0, h);
    a.params_tick();
    // The plugin moves its knobs as its window is dragged: the values are the plugin's at once, the events follow.
    let drag = |a: &mut App, events: Vec<os_host::ParamEvent>| {
        {
            let mut k = knob.lock().unwrap();
            for e in &events {
                if let Value(id, v) = *e {
                    k.value[(id - 7) as usize] = v;
                }
            }
            k.events = events;
        }
        a.params_tick();
    };
    let value = |a: &mut App| {
        a.host_commands();
        knob.lock().unwrap().value
    };
    let steps = a.state.session.history.at();
    drag(&mut a, vec![Begin(7), Value(7, 0.4), Value(7, 0.6)]);
    assert_eq!(a.state.session.history.at(), steps, "nothing until the gesture ends");
    drag(&mut a, vec![Value(7, 0.7), End(7)]);
    assert_eq!(a.state.session.history.at(), steps + 1, "a gesture is one step");
    assert_eq!(crate::history_ui::undo(&mut a.state).as_deref(), Some(command::PLUGIN_PARAMETER));
    assert_eq!(value(&mut a)[0], 0.25, "back to the value before the gesture");
    // Undo, tweak, undo: the tweak moves from the value the undo put back.
    drag(&mut a, vec![Begin(7), Value(7, 0.5), End(7)]);
    crate::history_ui::undo(&mut a.state);
    assert_eq!(value(&mut a)[0], 0.25);
    crate::history_ui::redo(&mut a.state);
    assert_eq!(value(&mut a)[0], 0.5);
    let steps = a.state.session.history.at();
    // A gesture that ends where it began is none; so is the plugin echoing a value the host set.
    drag(&mut a, vec![Begin(7), Value(7, 0.9), Value(7, 0.5), End(7)]);
    drag(&mut a, vec![Value(7, 0.5)]);
    assert_eq!(a.state.session.history.at(), steps);
    // Values sent without a gesture (an X-Y pad, some VST2 plugins) make one step while they come in a row, whichever
    // parameters they move.
    drag(&mut a, vec![Value(7, 0.1), Value(8, 0.2)]);
    drag(&mut a, vec![Value(7, 0.15), Value(8, 0.3)]);
    drag(&mut a, vec![Value(8, 0.4)]);
    assert_eq!(a.state.session.history.at(), steps + 1);
    crate::history_ui::undo(&mut a.state);
    assert_eq!(value(&mut a), [0.5, 0.5], "both back to before the run");
    crate::history_ui::redo(&mut a.state);
    assert_eq!(value(&mut a), [0.15, 0.4]);
}

fn knob_state(v: [f64; 2]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

/// Lemon Studio's choice: a preset loaded into a plugin at the user's request (its window's Presets box, ◀ ▶, a file
/// dropped on it) is one undo step, "Plugin preset". Undo brings back the state, the preset shown and the name the load
/// gave the channel or slot; redo the state the undo replaced (edits since the load that made no step too). The step
/// goes by the owner's id, so it follows the channel when it moves. A preset that goes into an instance just made for
/// it makes none.
#[test]
fn preset_loads_are_undo_steps() {
    use crate::presets::{save, Header};
    use crate::presets_ui::load_into;
    let mut a = app();
    a.state.session.plugins = crate::plugins::Plugins::default();
    let desc = crate::plugins::builtin()[0].desc.clone();
    let knob = std::sync::Arc::new(std::sync::Mutex::new(Knob { desc: desc.clone(), value: [0.25, 0.5], events: Vec::new() }));
    let mut h = crate::plugins::Hosted::new(knob.clone());
    h.owner = a.state.session.project.channels[0].id;
    a.state.session.plugins.hosted.insert(0, h);
    a.state.session.project.get_mut().channels[0].name = Some("808 Snare".into());
    a.params_tick();
    let dir = std::env::temp_dir().join(format!("os-preset-undo-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let header = Header { plugin: desc.name.clone(), id: desc.uid(), format: desc.format.as_str().into(), body: String::new() };
    let preset = |name: &str, v: [f64; 2]| {
        let p = dir.join(format!("{name}.ospreset"));
        save(&p, &header, &knob_state(v)).unwrap();
        p
    };
    let (bright, dark) = (preset("Bright", [0.9, 0.1]), preset("Dark", [0.1, 0.9]));
    // What the instance on `row` and its channel hold, once the app's tick has put back what an undo or redo took.
    let seen = |a: &mut App, row: usize| {
        a.host_commands();
        (knob.lock().unwrap().value, a.state.session.project.channels[row].name.clone().unwrap_or_default(), a.state.session.plugins.preset_name(row))
    };
    let at = |v: [f64; 2], name: &str, preset: Option<&str>| (v, name.to_string(), preset.map(str::to_string));

    let steps = a.state.session.history.at();
    assert_eq!(load_into(&mut a, 0, &bright).as_deref(), Some("Preset: Bright"));
    assert_eq!((a.state.session.history.at(), a.state.session.history.newest_done()), (steps + 1, Some(command::PLUGIN_PRESET)));
    assert_eq!(seen(&mut a, 0), at([0.9, 0.1], "Bright", Some("Bright")), "Auto name channels is on by default");
    load_into(&mut a, 0, &dark);
    load_into(&mut a, 0, &dark);
    assert_eq!(a.state.session.history.at(), steps + 2, "the same preset again changes nothing");

    // The channel moves without a step of its own: the instance follows it, and so does the undo.
    a.state.session.project.get_mut().channels.swap(0, 1);
    a.follow_ids(true, false);
    assert!(a.state.session.plugins.hosted.contains_key(&1));
    assert_eq!(crate::history_ui::undo(&mut a.state).as_deref(), Some(command::PLUGIN_PRESET));
    assert_eq!(seen(&mut a, 1), at([0.9, 0.1], "Bright", Some("Bright")));
    crate::history_ui::undo(&mut a.state);
    assert_eq!(seen(&mut a, 1), at([0.25, 0.5], "808 Snare", None), "back to before the first preset");
    crate::history_ui::redo(&mut a.state);
    assert_eq!(seen(&mut a, 1), at([0.9, 0.1], "Bright", Some("Bright")));
    crate::history_ui::redo(&mut a.state);
    assert_eq!(seen(&mut a, 1), at([0.1, 0.9], "Dark", Some("Dark")));

    // Several undos and redos before the app's tick (the browser's History, a client) come out the same.
    crate::history_ui::undo(&mut a.state);
    crate::history_ui::undo(&mut a.state);
    crate::history_ui::redo(&mut a.state);
    assert_eq!(seen(&mut a, 1), at([0.9, 0.1], "Bright", Some("Bright")));
    crate::history_ui::redo(&mut a.state);
    assert_eq!(seen(&mut a, 1), at([0.1, 0.9], "Dark", Some("Dark")));

    // An edit the host saw no step for (Undo knob tweaks off, a plugin's own page) comes back with the redo.
    knob.lock().unwrap().value = [0.3, 0.7];
    crate::history_ui::undo(&mut a.state);
    assert_eq!(seen(&mut a, 1).0, [0.9, 0.1]);
    crate::history_ui::redo(&mut a.state);
    assert_eq!(seen(&mut a, 1).0, [0.3, 0.7]);

    // A mixer slot: the name "Auto name effect slots" gave it goes with the undo.
    crate::prefs::set_check(&crate::prefs::AUTO_NAME_EFFECT_SLOTS, true);
    let fx = crate::plugins::fx_key(1, 0);
    let slot_knob = std::sync::Arc::new(std::sync::Mutex::new(Knob { desc, value: [0.5, 0.5], events: Vec::new() }));
    a.state.session.plugins.hosted.insert(fx, crate::plugins::Hosted::new(slot_knob.clone()));
    crate::edit::mixer::track_mut(&mut a.state.session, 1).unwrap().slots[0] = Some(crate::plugins::new_slot("Fruity Delay 3"));
    let slot = |a: &App| crate::edit::mixer::slot_name(&a.state.session, 1, 0).unwrap_or_default();
    let plain = slot(&a);
    let steps = a.state.session.history.at();
    load_into(&mut a, fx, &bright);
    assert_eq!((a.state.session.history.at(), slot(&a), slot_knob.lock().unwrap().value), (steps + 1, "Bright".to_string(), [0.9, 0.1]));
    crate::history_ui::undo(&mut a.state);
    a.host_commands();
    assert_eq!((slot(&a), slot_knob.lock().unwrap().value, a.state.session.plugins.preset_name(fx)), (plain.clone(), [0.5, 0.5], None));
    crate::history_ui::redo(&mut a.state);
    a.host_commands();
    assert_eq!((slot(&a), slot_knob.lock().unwrap().value), ("Bright".to_string(), [0.9, 0.1]));

    // A preset dropped where a new instance was made loads into it as part of the step that made it.
    let steps = a.state.session.history.at();
    a.state.session.plugins.preset_waits.push((1, dark.clone(), std::time::Instant::now()));
    crate::presets_ui::pending_tick(&mut a);
    assert_eq!((a.state.session.history.at(), knob.lock().unwrap().value), (steps, [0.1, 0.9]));
    let _ = std::fs::remove_dir_all(&dir);
}

/// The interface edits the audit of 2026-10-28 found without an undo step have one now; a drag or a run of wheel
/// notches of one box is one step.
#[test]
fn interface_edits_are_undo_steps() {
    let mut a = app();
    ui_round_trip(&mut a, "Channel mixer track", |a| {
        a.channel_control(0, crate::history::ChannelControl::Track, |c| c.mixer_track = 3);
        a.channel_control(0, crate::history::ChannelControl::Track, |c| c.mixer_track = 4);
    });
    ui_round_trip(&mut a, "Pattern time signature", |a| crate::timesig::set_start(&mut a.state, 1, 3, 8));
    ui_round_trip(&mut a, "Add arrangement", |a| {
        crate::playlist::run(a, crate::playlist::Cmd::ArrAdd);
        a.state.playlist.edit = None;
    });
    ui_round_trip(&mut a, "Pattern name", |a| {
        crate::playlist::start_edit(&mut a.state, crate::playlist::EditTarget::PatternName(1), (0.0, 0.0));
        a.state.playlist.edit.as_mut().unwrap().text = "Lead".into();
        crate::playlist::commit_edit(&mut a.state);
    });
}

/// As in FL (measured 2026-10-28): a channel muted, soloed or switched on or off is one step of its own (a knob tweak's:
/// none with "Undo knob tweaks" off); a mixer track's mute and solo make none. An undo or redo of another step leaves
/// every channel and track muted or not as it is (an undo of a channel's volume does not unmute it).
#[test]
fn mutes_follow_fl_s_history() {
    let mut a = app();
    ui_round_trip(&mut a, crate::rack::MUTE_SOLO, |a| crate::rack::toggle_mute(&mut a.state, 0));
    ui_round_trip(&mut a, crate::rack::MUTE_SOLO, |a| crate::rack::solo(&mut a.state, 1));
    let steps = a.state.session.history.at();
    crate::mixer::hit(&mut a, crate::mixer::MHit::Led(2));
    crate::mixer::run(&mut a, crate::mixer::MCmd::Solo(1));
    assert_eq!(a.state.session.history.at(), steps, "a mixer track's mute and solo make no steps");
    let on = |a: &App| (a.state.session.project.channels.iter().map(|c| c.enabled).collect::<Vec<_>>(), a.state.session.project.mixer.iter().map(|m| m.enabled).collect::<Vec<_>>());
    // A step on channel 0, then a mixer track muted: its undo and redo keep the track muted.
    let track = a.state.session.project.channels[0].mixer_track;
    a.channel_control(0, crate::history::ChannelControl::Track, |c| c.mixer_track = 5);
    crate::mixer::hit(&mut a, crate::mixer::MHit::Led(3));
    let now = on(&a);
    crate::history_ui::undo(&mut a.state).unwrap();
    assert_eq!(a.state.session.project.channels[0].mixer_track, track, "undone");
    assert_eq!(on(&a), now, "the mutes stay through the undo");
    crate::history_ui::redo(&mut a.state).unwrap();
    assert_eq!(on(&a), now, "and the redo");
    // With Undo knob tweaks off, a channel's mute makes no step either, and an undo of an earlier step keeps it.
    let was = crate::prefs::check(&crate::prefs::UNDO_KNOB_TWEAKS);
    crate::prefs::set_check(&crate::prefs::UNDO_KNOB_TWEAKS, false);
    let steps = a.state.session.history.at();
    crate::rack::toggle_mute(&mut a.state, 0);
    let now = on(&a);
    crate::prefs::set_check(&crate::prefs::UNDO_KNOB_TWEAKS, was);
    assert_eq!(a.state.session.history.at(), steps, "no step with Undo knob tweaks off");
    crate::history_ui::undo(&mut a.state).unwrap();
    assert_eq!(on(&a), now, "the mute stays through an undo of the step before it");
}

/// A hosted mixer slot (spec: ui/mixer.md §7.2, Lemon Studio's design): Scaling ▸ is enabled, Main ticked, and a pick
/// keeps the scale for the slot's window; Help opens Lemon Studio's page for its own plugins.
#[test]
fn a_hosted_slots_scaling_and_help() {
    use crate::mixer::{run, slot_menu, MCmd};
    use crate::playlist::{Cmd, Mark};
    let mut a = app();
    let desc = crate::plugins::builtin()[0].desc.clone();
    let knob = std::sync::Arc::new(std::sync::Mutex::new(Knob { desc, value: [0.5, 0.5], events: Vec::new() }));
    let fx = crate::plugins::fx_key(1, 0);
    a.state.session.plugins.hosted.insert(fx, crate::plugins::Hosted::new(knob));
    crate::edit::mixer::track_mut(&mut a.state.session, 1).unwrap().slots[0] = Some(crate::plugins::new_slot("Lemon Sampler"));
    crate::mixer::select(&mut a.state, 1, false);
    let scaling = |a: &App| slot_menu(a, 0, 0.0, 0.0).items.into_iter().find(|i| i.label == "Scaling").unwrap();
    let m = scaling(&a);
    assert!(m.enabled);
    let marks: Vec<_> = m.children.iter().map(|i| (i.label.to_string(), i.mark.clone())).collect();
    assert_eq!(marks[0], ("Main".into(), Mark::Radio(true)));
    assert_eq!(marks[2], ("125%".into(), Mark::Radio(false)));
    assert_eq!(m.children[2].cmd, Cmd::Mixer(MCmd::SlotScale(0, Some(125))));
    run(&mut a, MCmd::SlotScale(0, Some(125)));
    assert_eq!((a.state.session.plugins.scale_for(fx, 1.0), scaling(&a).children[2].mark.clone()), (1.25, Mark::Radio(true)));
    run(&mut a, MCmd::SlotScale(0, None));
    assert_eq!(a.state.session.plugins.scale_for(fx, 1.5), 1.5, "Main");
    assert_eq!(run(&mut a, MCmd::SlotHelp(0)), Some(format!("Opening {}", crate::help_links::plugin_page("Lemon Sampler"))));
    // No instance (an effect the engine plays itself has no editor): greyed.
    a.state.session.plugins.hosted.clear();
    assert!(!scaling(&a).enabled);
}

/// Control-plane step 4: playback is handed the project when it was written (`locked::Locked`), and only then.
#[test]
fn idle_ticks_hand_playback_nothing() {
    let mut a = app();
    for _ in 0..3 {
        a.tick();
    }
    let n = a.state.session.transport.updates_for_tests();
    for _ in 0..20 {
        a.tick();
    }
    assert_eq!(a.state.session.transport.updates_for_tests(), n, "an idle tick writes nothing");
}

#[test]
fn a_write_reaches_playback_with_no_flag_set() {
    let mut a = app();
    for _ in 0..3 {
        a.tick();
    }
    let n = a.state.session.transport.updates_for_tests();
    a.state.session.dirty = false;
    a.state.session.project.get_mut().main_volume = Some(1234);
    a.tick();
    assert_eq!(a.state.session.transport.updates_for_tests(), n + 1);
    a.tick();
    assert_eq!(a.state.session.transport.updates_for_tests(), n + 1, "once");
}

#[test]
fn a_proposal_hands_playback_nothing() {
    let mut a = app();
    for _ in 0..3 {
        a.tick();
    }
    let n = a.state.session.transport.updates_for_tests();
    let kick = ids(&a).0[0];
    propose(&mut a.state, &test_client(), "Boom", vec![cmd(json!({"op": "set_channel", "channel": kick, "name": "Boom"}))]).unwrap();
    a.tick();
    assert_eq!(a.state.session.transport.updates_for_tests(), n, "the dry run took its change back");
}

/// A session served without a window (`--serve`), no `App` at all: a client's command applies and its undo takes it
/// back, a proposal changes nothing until accepted, and the client saves the project itself, which then counts as saved
/// and opens again with the change.
#[test]
fn a_session_without_a_window_serves_clients() {
    let mut ses = crate::session::Session::new(crate::projects::test_project(), crate::transport::Transport::new(None), crate::settings_file::store());
    let mut c = rpc::Client { name: "Headless".into(), level: Level::Edit, saves: true, token: None };
    let mut call = |ses: &mut crate::session::Session, m: &str, params: Value| -> Value {
        let mut asks = rpc::Asks::default();
        let line = json!({"jsonrpc": "2.0", "id": 1, "method": m, "params": params}).to_string();
        let out: Value = serde_json::from_str(&rpc::rpc_line_in(ses, &mut c, &line, 0, &mut asks).unwrap()).unwrap();
        assert!(asks.save.is_none(), "{m}: a client that saves is not asked about");
        out
    };
    let before = ses.project.channels.len();
    let r = call(&mut ses, "apply", json!({"command": {"op": "add_channel", "name": "Bass"}}));
    assert!(r.get("result").is_some(), "{r}");
    assert_eq!(ses.project.channels.len(), before + 1);
    call(&mut ses, "undo", json!({}));
    assert_eq!(ses.project.channels.len(), before);
    call(&mut ses, "redo", json!({}));
    let r = call(&mut ses, "propose", json!({"name": "Faster", "command": {"op": "set_tempo", "bpm": 150}}));
    assert!(r["result"]["proposal"].as_u64().is_some(), "{r}");
    assert_ne!(ses.tempo(), 150.0, "a proposal changes nothing yet");
    let path = std::env::temp_dir().join(format!("os-headless-{}.flp", std::process::id()));
    let r = call(&mut ses, "save", json!({"path": path.display().to_string()}));
    assert_eq!(r["result"]["saved"].as_str(), Some(path.display().to_string().as_str()), "{r}");
    assert_eq!(ses.saved.0, ses.project.clone(), "saved");
    let (_, p, _) = crate::project_io::read(&path).unwrap_or_else(|(e, _)| panic!("{e}"));
    assert!(p.channels.iter().any(|c| c.name.as_deref() == Some("Bass")));
    let _ = std::fs::remove_file(&path);
    crate::control::server::serve_tick(&mut ses);
    assert_eq!(ses.touched, Default::default());
}

/// An add's undo removes what it made, and a move's moves it back, by id (control-plane step 13): what else changed
/// meanwhile without a step of its own stays, which putting back the whole project would have undone.
#[test]
fn adds_and_moves_undo_by_their_inverse() {
    let mut a = app();
    let kick = ids(&a).0[0];
    let length = |a: &App| a.state.session.project.patterns[0].length;
    for c in [
        json!({"op": "add_channel", "name": "Bass"}),
        json!({"op": "add_track", "at": 2}),
        json!({"op": "add_pattern", "name": "Fill"}),
        json!({"op": "move_channel", "channel": kick, "row": 2}),
    ] {
        let what = c.to_string();
        let before = ids(&a);
        let r = apply(&mut a.state, &test_client(), None, vec![cmd(c)]).unwrap();
        a.state.session.control.settle(a.state.session.project.get_mut());
        let after = ids(&a);
        // The pattern length box makes no step (`ui/undo-steps.md`).
        let l = Some(length(&a).unwrap_or(0) + 96);
        a.state.session.project.get_mut().patterns[0].length = l;
        crate::history_ui::undo(&mut a.state).unwrap();
        assert_eq!(ids(&a), before, "{what} undone");
        assert_eq!(length(&a), l, "{what}: its undo leaves the pattern's length");
        crate::history_ui::redo(&mut a.state).unwrap();
        assert_eq!(ids(&a), after, "{what} redone with the same ids");
        if let Some(id) = r[0].as_object().and_then(|o| o.values().next()).and_then(Value::as_u64) {
            assert!(after.0.contains(&(id as u32)) || after.1.contains(&(id as u32)) || after.2.contains(&(id as u32)), "{what}: its id is back");
        }
    }
}

/// An added channel's instrument goes with its undo and is hosted again with its redo.
#[test]
fn an_added_channel_s_instrument_follows_its_undo_and_redo() {
    let mut a = app();
    a.state.session.plugins = crate::plugins::Plugins::default();
    a.state.session.plugins.available = crate::plugins::builtin();
    a.plugin_tick();
    a.plugin_tick();
    let rows = |a: &App| a.state.session.plugins.hosted.range(..crate::plugins::FX_KEY).map(|(&r, h)| (r, h.owner.0)).collect::<Vec<_>>();
    let mut c = rpc::Client { name: "Test".into(), level: Level::Edit, saves: false, token: None };
    let before = rows(&a);
    a.rpc(&mut c, "apply", &json!({"command": {"op": "add_channel", "name": "Bass"}})).ok().unwrap();
    a.plugin_tick();
    a.plugin_tick();
    let added = rows(&a);
    assert_eq!(added.len(), before.len() + 1, "the new sampler is hosted");
    a.rpc(&mut c, "undo", &Value::Null).ok().unwrap();
    a.plugin_tick();
    assert_eq!(rows(&a), before, "its instance goes with the undo");
    a.rpc(&mut c, "redo", &Value::Null).ok().unwrap();
    a.plugin_tick();
    a.plugin_tick();
    assert_eq!(rows(&a), added, "and is hosted again for its channel");
}

/// A client's gesture (control-plane step 13): the commands applied between `begin` and `end` are one step that puts
/// back what was there before the gesture; a step in between starts a new one; an ended gesture takes no more.
#[test]
fn a_gesture_is_one_step() {
    let mut a = app();
    let kick = ids(&a).0[0];
    let mut c = rpc::Client { name: "Test".into(), level: Level::Edit, saves: false, token: None };
    let volume = |a: &App| a.state.session.project.channels[0].volume;
    let (v0, t0) = (volume(&a), a.state.tempo());
    let steps = a.state.session.history.at();
    let g = a.rpc(&mut c, "begin", &json!({"name": "Fader drag"})).ok().unwrap()["gesture"].as_u64().unwrap();
    for v in [0.3, 0.4, 0.5] {
        a.rpc(&mut c, "apply", &json!({"gesture": g, "command": {"op": "set_channel", "channel": kick, "volume": v}})).ok().unwrap();
    }
    assert_eq!(a.state.session.history.at(), steps + 1, "one step for the gesture");
    assert_eq!(a.state.session.history.newest_done(), Some("Test: fader drag"));
    a.rpc(&mut c, "undo", &Value::Null).ok().unwrap();
    assert_eq!(volume(&a), v0, "the undo puts back the value before the gesture");
    a.rpc(&mut c, "redo", &Value::Null).ok().unwrap();
    assert_eq!(volume(&a), 0.5, "the redo gives the last");
    // After the undo and redo the gesture goes on in a step of its own.
    a.rpc(&mut c, "apply", &json!({"gesture": g, "command": {"op": "set_channel", "channel": kick, "volume": 0.6}})).ok().unwrap();
    a.rpc(&mut c, "apply", &json!({"gesture": g, "command": {"op": "set_tempo", "bpm": 99.0}})).ok().unwrap();
    assert_eq!(a.state.session.history.at(), steps + 2);
    a.rpc(&mut c, "undo", &Value::Null).ok().unwrap();
    assert_eq!((volume(&a), a.state.tempo()), (0.5, t0), "both regions back as they were before the step");
    a.rpc(&mut c, "end", &json!({"gesture": g})).ok().unwrap();
    assert!(a.rpc(&mut c, "apply", &json!({"gesture": g, "command": {"op": "set_tempo", "bpm": 98.0}})).is_err(), "an ended gesture is gone");
    assert!(a.rpc(&mut c, "end", &json!({"gesture": g})).is_err());
}

// ---------------------------------------------------------------------------------------------------------------
// FL's measured undo history (spec: ui/undo-steps.md), the rows control-plane step 13 found the app differing on.

/// Wheel notches about 120 ms apart are one step; after a pause of 2.5 s the next notch starts another.
#[test]
fn a_pause_in_the_wheel_starts_a_new_step() {
    let mut a = app();
    let id = crate::knob_id::KnobId::ChanVol(0);
    let t0 = std::time::Instant::now();
    let steps = a.state.session.history.at();
    let mut notch = |a: &mut App, ms: u64| {
        a.wheel_pause(t0 + std::time::Duration::from_millis(ms));
        crate::history::knob(&mut a.state.session, id, true);
        let v = a.state.knob(id) - 0.03;
        a.state.set_knob(id, v);
    };
    for ms in [0, 120, 240] {
        notch(&mut a, ms);
    }
    assert_eq!(a.state.session.history.at(), steps + 1, "notches 120 ms apart");
    notch(&mut a, 2740);
    notch(&mut a, 2860);
    assert_eq!(a.state.session.history.at(), steps + 2, "2.5 s later, a step of its own");
}

/// Two drags of one control are two steps (the rack's mixer track box, a plugin window's strip knob), one drag one.
#[test]
fn each_drag_of_a_control_is_a_step() {
    use crate::history::ChannelControl;
    let mut a = app();
    let steps = a.state.session.history.at();
    for t in [3, 4] {
        a.channel_control(0, ChannelControl::Track, |c| c.mixer_track = t);
    }
    a.mouse_up();
    assert_eq!(a.state.session.history.at(), steps + 1, "one drag");
    a.channel_control(0, ChannelControl::Track, |c| c.mixer_track = 5);
    assert_eq!(a.state.session.history.at(), steps + 2, "the next drag");
    // A strip knob's release in a plugin window ends its run the same way (`FrameEvent::RunEnd`).
    for v in [0.3, 0.4] {
        a.channel_control(0, ChannelControl::Volume, |c| c.volume = v);
    }
    a.state.session.history.end_gesture_run();
    a.channel_control(0, ChannelControl::Volume, |c| c.volume = 0.5);
    assert_eq!(a.state.session.history.at(), steps + 4);
}

/// Undo knob tweaks covers every "Knob tweak" step, the mixer track box's and the tempo's drag too.
#[test]
fn undo_knob_tweaks_off_covers_the_mixer_track_box_and_the_tempo() {
    let mut a = app();
    crate::prefs::set_check(&crate::prefs::UNDO_KNOB_TWEAKS, false);
    let steps = a.state.session.history.at();
    a.channel_control(0, crate::history::ChannelControl::Track, |c| c.mixer_track = 3);
    let mut press = App::new(None);
    press.act(crate::app::Action::Tempo);
    let none = press.state.session.history.len() == 0;
    crate::prefs::set_check(&crate::prefs::UNDO_KNOB_TWEAKS, true);
    assert_eq!(a.state.session.project.channels[0].mixer_track, 3);
    assert_eq!(a.state.session.history.at(), steps, "the mixer track box");
    assert!(none, "the tempo's press records no step");
}

/// Group selected (Alt+G, a name typed, Enter) and performance mode switched on are no undo step.
#[test]
fn a_filter_group_and_performance_mode_are_no_step() {
    let mut a = app();
    let steps = a.state.session.history.at();
    a.state.rack.sel = [0, 1].into();
    crate::rack::run(&mut a, crate::rack::RCmd::GroupSel);
    a.state.pat_ui.name_box.as_mut().expect("the name box").text = "Drums".into();
    crate::patterns::accept(&mut a.state);
    assert_eq!(crate::rack::groups(&a.state.session.project).len(), 1, "the group is made");
    crate::playlist::run(&mut a, crate::playlist::Cmd::PerfToggle);
    assert!(crate::perform_ui::on(&a.state));
    assert_eq!(a.state.session.history.at(), steps);
}

/// An effect slot's colour, random or chosen, is one step ("Name/color/icon effect").
#[test]
fn an_effect_slot_colour_is_a_step() {
    let mut a = app();
    a.state.selected_mixer = 2;
    let red = os_model::Rgb::new(0xc8, 0x50, 0x4a);
    ui_round_trip(&mut a, "Effect slot color", |a| {
        crate::mixer::run(a, crate::mixer::MCmd::SlotColor(0, Some(red)));
    });
    ui_round_trip(&mut a, "Effect slot color", |a| {
        crate::mixer::run(a, crate::mixer::MCmd::SlotRandomColor(1));
    });
}

/// Loop type ▸ Advanced looping turns loop mode on, one step ("Set pattern loop mode").
#[test]
fn advanced_looping_turns_loop_mode_on() {
    let mut a = app();
    assert!(!crate::edit::rack::loop_mode(&a.state.session));
    let steps = a.state.session.history.at();
    crate::rack::run(&mut a, crate::rack::RCmd::LoopType(2));
    a.mouse_up();
    assert!(crate::edit::rack::loop_mode(&a.state.session), "loop mode on");
    assert_eq!(crate::edit::rack::loop_type(&a.state.session), 2);
    assert_eq!(a.state.session.history.at(), steps + 1);
    crate::history_ui::undo(&mut a.state).unwrap();
    assert!(!crate::edit::rack::loop_mode(&a.state.session), "undone");
}

/// Arrangement ▸ Add one… with a name typed is one step, as Clone…; a rename is one of its own.
#[test]
fn an_arrangement_added_and_named_is_one_step() {
    let mut a = app();
    for cmd in [crate::playlist::Cmd::ArrAdd, crate::playlist::Cmd::ArrClone] {
        let steps = a.state.session.history.at();
        let count = a.state.session.project.arrangements.len();
        crate::playlist::run(&mut a, cmd.clone());
        a.state.playlist.edit.as_mut().expect("the name box").text = "Verse".into();
        crate::playlist::commit_edit(&mut a.state);
        assert_eq!(a.state.session.history.at(), steps + 1, "{cmd:?}");
        assert_eq!(crate::edit::playlist::arrangement_name(&a.state.session), "Verse");
        crate::history_ui::undo(&mut a.state).unwrap();
        assert_eq!(a.state.session.project.arrangements.len(), count, "{cmd:?} undone, name and all");
        crate::history_ui::redo(&mut a.state).unwrap();
        assert_eq!(crate::edit::playlist::arrangement_name(&a.state.session), "Verse", "{cmd:?} redone with its name");
    }
    let steps = a.state.session.history.at();
    crate::playlist::run(&mut a, crate::playlist::Cmd::ArrRename);
    a.state.playlist.edit.as_mut().unwrap().text = "Chorus".into();
    crate::playlist::commit_edit(&mut a.state);
    assert_eq!(a.state.session.history.at(), steps + 1, "a rename");
}

/// The newest step's name.
fn newest(a: &App) -> String {
    let e = a.state.session.history.entries(&crate::settings_file::SettingsStore::memory());
    e.last().map(|e| e.1.to_string()).unwrap_or_default()
}

/// A LED's right click ▸ Solo and a plugin window's ON switch: one mute and solo step each, which undo takes back.
#[test]
fn the_led_menu_s_solo_and_the_on_switch_are_one_step_each() {
    let mut a = app();
    let steps = a.state.session.history.at();
    crate::rack::run(&mut a, crate::rack::RCmd::Solo(0));
    assert_eq!(a.state.session.history.at(), steps + 1, "Solo");
    assert_eq!(newest(&a), crate::rack::MUTE_SOLO);
    assert!(!a.state.session.project.channels[1].enabled, "the others muted");
    crate::history_ui::undo(&mut a.state);
    assert!(a.state.session.project.channels.iter().all(|c| c.enabled), "undone");
    crate::rack::toggle_on(&mut a.state, 1);
    assert_eq!(a.state.session.history.at(), steps + 1, "ON switch");
    assert_eq!(newest(&a), crate::rack::MUTE_SOLO);
    assert!(!a.state.session.project.channels[1].enabled);
    crate::history_ui::undo(&mut a.state);
    assert!(a.state.session.project.channels[1].enabled, "undone");
}

/// A playlist track's LED: a click (mute) and a Ctrl+click (solo) are one step each, whatever Undo knob tweaks says,
/// and undo takes them back.
#[test]
fn a_playlist_track_s_mute_and_solo_are_one_step_each() {
    let mut a = app();
    crate::prefs::set_check(&crate::prefs::UNDO_KNOB_TWEAKS, false);
    let steps = a.state.session.history.at();
    let on = |a: &App| a.state.session.project.arrangement().expect("an arrangement").tracks.iter().take(3).map(|t| t.enabled).collect::<Vec<_>>();
    crate::playlist::press_led(&mut a, 1, false);
    let muted = on(&a);
    a.ctrl = true;
    crate::playlist::press_led(&mut a, 2, false);
    a.ctrl = false;
    let soloed = on(&a);
    let at = a.state.session.history.at();
    crate::history_ui::undo(&mut a.state);
    let after_one = on(&a);
    crate::history_ui::undo(&mut a.state);
    let after_two = on(&a);
    crate::prefs::set_check(&crate::prefs::UNDO_KNOB_TWEAKS, true);
    assert_eq!(muted, [true, false, true]);
    assert_eq!(soloed, [false, false, true]);
    assert_eq!(at, steps + 2);
    assert_eq!(after_one, muted, "the solo undone");
    assert_eq!(after_two, [true, true, true], "the mute undone");
}

/// A note added with a click in the piano roll is one step, "Piano roll add note".
#[test]
fn a_note_added_in_the_piano_roll_is_one_step() {
    let mut a = app();
    let mut c = os_ui::Canvas::new(1600, 1000, 1.0).expect("a canvas");
    a.state.z = vec![crate::app::Win::PianoRoll];
    a.paint(&mut c, None);
    let g = a.piano_geom().expect("piano roll shown").grid;
    a.mouse_move(g.x + g.w / 3.0, g.y + g.h / 2.0);
    let notes = crate::piano::notes(&a.state.session).len();
    let steps = a.state.session.history.at();
    a.mouse_down();
    a.mouse_up();
    assert_eq!(crate::piano::notes(&a.state.session).len(), notes + 1);
    assert_eq!(a.state.session.history.at(), steps + 1);
    assert_eq!(newest(&a), "Piano roll add note");
}

/// A clip placed with one click in the playlist is one step.
#[test]
fn a_clip_painted_with_one_click_is_one_step() {
    let mut a = app();
    let mut c = os_ui::Canvas::new(1600, 1000, 1.0).expect("a canvas");
    a.state.z = vec![crate::app::Win::Playlist];
    a.paint(&mut c, None);
    let g = a.pl_grid().expect("the arranger");
    a.mouse_move(g.x + g.w / 2.0, g.y + 30.0);
    let clips = crate::playlist::clips(&a.state.session).len();
    let steps = a.state.session.history.at();
    a.mouse_down();
    a.mouse_up();
    assert_eq!(crate::playlist::clips(&a.state.session).len(), clips + 1, "a clip placed");
    assert_eq!(a.state.session.history.at(), steps + 1);
    assert_eq!(newest(&a), "Arranger fill paint clip");
}

/// A channel colour darker than FL keeps (a component under 0x14) is kept as the save will write it, so it does not
/// change after a save and reopen.
#[test]
fn a_dark_channel_colour_shows_as_it_is_saved() {
    let mut a = app();
    let r = apply(&mut a.state, &test_client(), None, vec![cmd(json!({"op": "add_channel", "name": "Dark"}))]).unwrap();
    let ch = r[0]["channel"].as_u64().unwrap() as u32;
    apply(&mut a.state, &test_client(), None, vec![cmd(json!({"op": "set_channel", "channel": ch, "color": "#000a40"}))]).unwrap();
    assert_eq!(a.state.session.project.channels.last().unwrap().color, Some(os_model::Rgb::new(0x14, 0x14, 0x40)));
}

/// The undo of a delete puts back what the delete took, where it was and with its ids: a channel's notes in each
/// pattern, its clips and its place among another channel's layer children; a pattern's clips. Redoing a pattern's
/// delete gives the patterns it makes to fill the numbers the ids they had.
#[test]
fn a_delete_s_undo_puts_back_what_it_took_with_its_ids() {
    let mut a = app();
    for name in ["Two", "Three"] {
        apply(&mut a.state, &test_client(), None, vec![cmd(json!({"op": "add_pattern", "name": name}))]).unwrap();
    }
    let (ch, _, pa) = ids(&a);
    assert!(pa.len() >= 3);
    let s = &mut a.state.session;
    s.control.settle(s.project.get_mut());
    s.project.get_mut().channels[0].layer_children = vec![1, 2];
    for &p in &pa[..2] {
        let notes = json!([{"channel": ch[0], "position": 0}, {"channel": ch[1], "position": 48}, {"channel": ch[2], "position": 96}, {"channel": ch[1], "position": 144}]);
        apply(&mut a.state, &test_client(), None, vec![cmd(json!({"op": "add_notes", "pattern": p, "notes": notes}))]).unwrap();
    }
    for (track, pattern) in [(1, pa[0]), (2, pa[1]), (3, pa[1])] {
        apply(&mut a.state, &test_client(), None, vec![cmd(json!({"op": "add_clip", "track": track, "position": 0, "pattern": pattern}))]).unwrap();
    }
    let notes = |a: &App| -> Vec<Vec<(u32, u16, u32)>> {
        a.state.session.project.patterns.iter().map(|q| q.notes.iter().map(|n| (n.id.0, n.channel, n.position)).collect()).collect()
    };
    let clips = |a: &App| -> Vec<(u32, u16, os_model::ClipSource)> { crate::edit::playlist::clips(&a.state.session).iter().map(|c| (c.id.0, c.track, c.source)).collect() };
    let whole = |a: &App| (notes(a), clips(a), ids(a), a.state.session.project.channels[0].layer_children.clone());
    let before = whole(&a);

    apply(&mut a.state, &test_client(), None, vec![cmd(json!({"op": "delete_channel", "channel": ch[1]}))]).unwrap();
    assert_eq!(a.state.session.project.channels[0].layer_children, vec![1], "the layer's other child moved up");
    assert!(notes(&a).iter().flatten().all(|&(_, c, _)| c != 2), "its notes went");
    crate::history_ui::undo(&mut a.state);
    assert_eq!(whole(&a), before, "a channel's delete undone");

    apply(&mut a.state, &test_client(), None, vec![cmd(json!({"op": "delete_pattern", "pattern": pa[1]}))]).unwrap();
    let deleted = whole(&a);
    assert_eq!(clips(&a).len(), 1, "its clips went");
    crate::history_ui::undo(&mut a.state);
    assert_eq!(whole(&a), before, "a pattern's delete undone");
    crate::history_ui::redo(&mut a.state);
    assert_eq!(whole(&a), deleted, "and redone");
    crate::history_ui::undo(&mut a.state);

    // A mixer track with a send into it and a channel going to it; the tracks after it move down.
    let tr = ids(&a).1;
    let to = |a: &App, i: usize| a.state.session.project.mixer[i].clone();
    apply(&mut a.state, &test_client(), None, vec![
        cmd(json!({"op": "set_send", "from": tr[1], "to": tr[3], "level": 0.5})),
        cmd(json!({"op": "set_send", "from": tr[4], "to": tr[3]})),
        cmd(json!({"op": "set_channel", "channel": ch[2], "track": tr[3]})),
        cmd(json!({"op": "set_channel", "channel": ch[0], "track": tr[4]})),
    ])
    .unwrap();
    let mixer = |a: &App| -> Vec<(u32, u16, Vec<u16>, Vec<(u16, f64)>)> {
        a.state.session.project.mixer.iter().map(|m| (m.id.0, m.index, m.routes.clone(), m.send_levels.clone())).collect()
    };
    let outs = |a: &App| -> Vec<u16> { a.state.session.project.channels.iter().map(|c| c.mixer_track).collect() };
    let before = (mixer(&a), outs(&a), to(&a, 3));
    apply(&mut a.state, &test_client(), None, vec![cmd(json!({"op": "delete_track", "track": tr[3]}))]).unwrap();
    let deleted = (mixer(&a), outs(&a));
    assert_eq!(a.state.session.project.channels[0].mixer_track, 3, "the channel on the next track follows it down");
    assert_eq!(a.state.session.project.channels[2].mixer_track, 0, "the deleted track's channel goes to the master");
    crate::history_ui::undo(&mut a.state);
    assert_eq!((mixer(&a), outs(&a), to(&a, 3)), before, "a mixer track's delete undone");
    crate::history_ui::redo(&mut a.state);
    assert_eq!((mixer(&a), outs(&a)), deleted, "and redone, the empty track it makes with the id it had");
}

/// Every kind of project knob turns through its command (control-plane step 14): a drag is one step named for the
/// knob, its undo gives the project back as it was and its redo as the drag left it, and a knob whose thing is gone
/// makes no step.
#[test]
fn every_project_knob_turns_through_its_command() {
    use crate::knobs::{KnobId, LfoKnob};
    use crate::lemon_midi_out::MoKnob;
    let mut a = App::new(None);
    a.add_midi_out();
    let out = a.state.session.project.channels.len() - 1;
    // Row 0 as an automation clip, with its clip.
    let ch = a.state.session.project.channels[0].index;
    a.state.session.project.get_mut().channels[0].kind = os_model::ChannelKind::Automation;
    a.state.session.project.get_mut().automations.push(os_flp::project::untargeted_automation(ch, 0.5, 4.0));
    let t = a.state.session.project.mixer.iter().position(|m| m.index == 1).unwrap();
    a.state.session.project.get_mut().mixer[t].routes = vec![0];
    let knobs = [
        KnobId::MasterVol,
        KnobId::MasterPitch,
        KnobId::Swing,
        KnobId::MixPan(1),
        KnobId::MixSep(1),
        KnobId::SlotMix(1, 2),
        KnobId::Send(1, 0),
        KnobId::ChanVol(1),
        KnobId::ChanPan(1),
        KnobId::Eq(1, 1, 0),
        KnobId::Eq(1, 2, 2),
        KnobId::AutoLfo(0, LfoKnob::Amount),
        KnobId::AutoLfo(0, LfoKnob::On),
        KnobId::AutoLfo(0, LfoKnob::Skew),
        KnobId::MidiOut(out, MoKnob::Patch),
        KnobId::MidiOut(out, MoKnob::Bank2),
        KnobId::MidiOut(out, MoKnob::SendLevels),
    ];
    for id in knobs {
        let before = a.state.session.project.clone();
        let steps = a.state.session.history.len();
        crate::history::knob(&mut a.state.session, id, false);
        // Through the middle, to the far side from where it starts (a check flips).
        let far = if a.state.knob(id) >= 0.5 { 0.1 } else { 0.9 };
        a.state.set_knob(id, 0.45);
        a.state.set_knob(id, far);
        a.state.session.history.end_gesture_run();
        let after = a.state.session.project.clone();
        assert_ne!(after, before, "{id:?} changes the project");
        assert_eq!(a.state.session.history.len(), steps + 1, "{id:?}: one step");
        crate::history_ui::undo(&mut a.state);
        assert_eq!(a.state.session.project, before, "{id:?} undone");
        crate::history_ui::redo(&mut a.state);
        assert_eq!(a.state.session.project, after, "{id:?} redone");
    }
    // A knob on a track that isn't there.
    let steps = a.state.session.history.len();
    crate::history::knob(&mut a.state.session, KnobId::MixPan(500), false);
    a.state.set_knob(KnobId::MixPan(500), 0.2);
    assert_eq!(a.state.session.history.len(), steps);
}
