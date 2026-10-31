// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The control plane's tests: every command undoes and redoes exactly, transactions are whole, proposals leave the
//! project alone, ids stay with their things, and the JSON-RPC adapter answers over a real socket.

use serde_json::{json, Value};

use super::*;
use crate::app::App;

fn app() -> App {
    let mut a = App::new(None);
    a.state.project = crate::app::test_project();
    a.state.project.arrangements = vec![os_flp::project::default_arrangement()];
    a.state.control.settle(&mut a.state.project);
    a
}

fn test_client() -> By {
    By::Client("Test".into())
}

/// The project and the rack's state of it, ids included (`==` on the model leaves ids out). Not the selection: an
/// undo leaves it where it is, as before.
fn reading(a: &App) -> (os_model::Project, Value, f64) {
    let mut q = query::project(&a.state);
    q.as_object_mut().unwrap().remove("selected_channel");
    (a.state.project.clone(), q, a.state.tempo)
}

fn ids(a: &App) -> (Vec<u32>, Vec<u32>, Vec<u32>) {
    let p = &a.state.project;
    (p.channels.iter().map(|c| c.id.0).collect(), p.mixer.iter().map(|m| m.id.0).collect(), p.patterns.iter().map(|q| q.id.0).collect())
}

fn cmd(v: Value) -> Command {
    serde_json::from_value(v).unwrap_or_else(|e| panic!("{e}"))
}

/// Applies `c` as a client would and checks that one undo puts everything back and one redo gives the change again.
fn round_trip(a: &mut App, c: Value) -> Vec<Value> {
    let before = reading(a);
    let steps = a.state.history.at();
    let what = c.to_string();
    let r = apply(&mut a.state, &test_client(), None, vec![cmd(c)]).unwrap_or_else(|e| panic!("{what}: {e}"));
    a.state.control.settle(&mut a.state.project);
    let after = reading(a);
    assert_ne!(before, after, "{what} changes something");
    assert_eq!(a.state.history.at(), steps + 1, "{what} is one step");
    crate::history::undo(&mut a.state).unwrap();
    assert_eq!(reading(a), before, "{what} undone");
    crate::history::redo(&mut a.state).unwrap();
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
        json!({"op": "set_channel", "channel": kick, "name": "Big kick", "color": "#ff8000", "volume": 0.5, "pan": -0.25, "pitch": 100, "track": ins2}),
        json!({"op": "load_sample", "channel": clap, "path": "/no/such/folder/snare.wav"}),
        json!({"op": "set_track", "track": ins1, "name": "Drums", "color": "#102030", "volume": 0.6, "pan": 0.5, "stereo": -0.5, "polarity": true, "swap": true, "effects_enabled": false}),
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
    let slot = query::project(&a.state)["mixer"].as_array().unwrap().iter().flat_map(|t| t["slots"].as_array().unwrap().clone()).next().unwrap();
    let track = query::project(&a.state)["mixer"].as_array().unwrap().iter().find(|t| !t["slots"].as_array().unwrap().is_empty()).unwrap()["id"].as_u64().unwrap();
    round_trip(&mut a, json!({"op": "remove_effect", "track": track, "slot": slot["slot"]}));
}

#[test]
fn new_things_name_their_ids() {
    let mut a = app();
    let r = apply(&mut a.state, &test_client(), None, vec![cmd(json!({"op": "add_channel", "name": "Bass"})), cmd(json!({"op": "add_pattern"}))]).unwrap();
    let ch = r[0]["channel"].as_u64().unwrap() as u32;
    let pat = r[1]["pattern"].as_u64().unwrap() as u32;
    let p = &a.state.project;
    assert_eq!(p.channels.last().map(|c| (c.id.0, c.name.clone())), Some((ch, Some("Bass".into()))));
    assert!(p.patterns.iter().any(|q| q.id.0 == pat));
    // The new channel's id names it in the next command.
    apply(&mut a.state, &test_client(), None, vec![cmd(json!({"op": "set_channel", "channel": ch, "volume": 0.1}))]).unwrap();
    assert_eq!(a.state.project.channels.last().unwrap().volume, 0.1);
}

#[test]
fn a_transaction_is_one_named_step_and_all_or_nothing() {
    let mut a = app();
    let kick = ids(&a).0[0];
    let before = reading(&a);
    let steps = a.state.history.at();
    let bad = vec![cmd(json!({"op": "set_tempo", "bpm": 90})), cmd(json!({"op": "set_channel", "channel": 9999, "volume": 0.5}))];
    let e = apply(&mut a.state, &test_client(), Some("Two things"), bad).unwrap_err();
    assert!(e.contains("command 1") && e.contains("no channel 9999"), "{e}");
    assert_eq!(reading(&a), before, "the first command is taken back");
    assert_eq!(a.state.history.at(), steps, "no step");

    let good = vec![cmd(json!({"op": "set_tempo", "bpm": 90})), cmd(json!({"op": "set_channel", "channel": kick, "volume": 0.5}))];
    apply(&mut a.state, &test_client(), Some("Build Drum Kit"), good).unwrap();
    assert_eq!(a.state.history.at(), steps + 1);
    let h = &a.state.history;
    let name = h.entries().into_iter().find(|e| e.0 == h.at()).map(|e| e.1.to_string());
    assert_eq!(name.as_deref(), Some("Test: build Drum Kit"), "named after the client");
    assert_eq!(crate::history::undo(&mut a.state).as_deref(), Some("Test: build Drum Kit"));
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
    crate::history::knob(&mut a.state, crate::knobs::KnobId::ChanVol(0), false);
    a.state.project.channels[0].volume = 0.3;
    apply(&mut a.state, &test_client(), None, vec![cmd(json!({"op": "set_channel", "channel": kick, "volume": 0.6}))]).unwrap();
    assert_eq!(crate::history::undo(&mut a.state).as_deref(), Some("Test: channel settings"));
    assert_eq!(a.state.project.channels[0].volume, 0.3);
    assert_eq!(crate::history::undo(&mut a.state).as_deref(), Some("Channel volume"));
    assert_eq!(a.state.project.channels[0].volume, 0.78125);
    crate::history::redo(&mut a.state);
    crate::history::redo(&mut a.state);
    assert_eq!(a.state.project.channels[0].volume, 0.6);
}

#[test]
fn ids_stay_with_their_things() {
    let mut a = app();
    let (ch, ..) = ids(&a);
    apply(&mut a.state, &test_client(), None, vec![cmd(json!({"op": "delete_channel", "channel": ch[1]}))]).unwrap();
    // The channel below it moved up a row and kept its id; commands still find it.
    assert_eq!(a.state.project.channels[1].id.0, ch[2]);
    apply(&mut a.state, &test_client(), None, vec![cmd(json!({"op": "set_channel", "channel": ch[2], "name": "Still me"}))]).unwrap();
    assert_eq!(a.state.project.channels[1].name.as_deref(), Some("Still me"));
    // The interface's edits keep them too: a clone gets a new id, the original keeps its own.
    crate::rack::clone_rows(&mut a, &[0]);
    a.state.control.settle(&mut a.state.project);
    assert_eq!(a.state.project.channels[0].id.0, ch[0]);
    assert!(!ch.contains(&a.state.project.channels[1].id.0), "the clone's id is new");
    // A deleted channel's id names nothing; an undo brings it back with its id.
    let e = apply(&mut a.state, &test_client(), None, vec![cmd(json!({"op": "set_channel", "channel": ch[1], "volume": 0.5}))]).unwrap_err();
    assert!(e.contains("no channel"), "{e}");
}

#[test]
fn ids_are_not_an_unsaved_change() {
    let mut a = app();
    a.mark_saved();
    assert!(!a.unsaved());
    a.state.project.channels.push(a.state.project.channels[0].clone());
    a.state.control.settle(&mut a.state.project);
    a.state.project.channels.pop();
    assert!(!a.unsaved(), "a copy's new id alone changes nothing");
}

#[test]
fn a_proposal_leaves_the_project_until_accepted() {
    let mut a = app();
    let kick = ids(&a).0[0];
    let before = reading(&a);
    let steps = a.state.history.at();
    let cmds = vec![cmd(json!({"op": "set_tempo", "bpm": 150})), cmd(json!({"op": "set_channel", "channel": kick, "name": "Boom"})), cmd(json!({"op": "add_channel", "name": "New"}))];
    let (n, changes) = propose(&mut a.state, &test_client(), "Faster", cmds).unwrap();
    assert_eq!(reading(&a), before, "nothing changed");
    assert_eq!(a.state.history.at(), steps);
    assert!(a.state.sample_loads.is_empty() && a.state.control.host.is_empty());
    let paths: Vec<&str> = changes.iter().map(|c| c.path.as_str()).collect();
    assert!(paths.contains(&"tempo"), "{paths:?}");
    assert!(paths.contains(&format!("channels.{kick}.name").as_str()), "{paths:?}");
    assert!(paths.iter().any(|p| p.starts_with("channels.") && changes.iter().any(|c| c.path == *p && c.before.is_null())), "the new channel: {paths:?}");
    assert_eq!(a.state.control.proposals.len(), 1);
    accept(&mut a.state, n).unwrap();
    assert_eq!(a.state.tempo, 150.0);
    assert_eq!(a.state.project.channels[0].name.as_deref(), Some("Boom"));
    assert_eq!(a.state.history.at(), steps + 1);
    assert!(a.state.control.proposals.is_empty());
    assert!(accept(&mut a.state, n).is_err(), "only once");
    let (n, _) = propose(&mut a.state, &test_client(), "Slower", vec![cmd(json!({"op": "set_tempo", "bpm": 60}))]).unwrap();
    discard(&mut a.state, n).unwrap();
    assert_eq!(a.state.tempo, 150.0);
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
    assert_eq!(a.state.tempo, 150.0);
    // A client that may only propose can't accept or apply.
    assert!(a.rpc(&mut c, "apply", &json!({"command": {"op": "set_tempo", "bpm": 100}})).is_err());
    assert!(a.rpc(&mut c, "accept", &json!({"proposal": n})).is_err());
    assert_eq!(a.state.tempo, 150.0);
}

#[test]
fn events_tell_clients_what_changed() {
    let mut a = app();
    a.state.control.watched = true;
    let (_, last) = a.state.control.events_since(0);
    apply(&mut a.state, &test_client(), Some("Faster"), vec![cmd(json!({"op": "set_tempo", "bpm": 150}))]).unwrap();
    crate::history::undo(&mut a.state);
    let (ev, newest) = a.state.control.events_since(last);
    assert_eq!(ev.len(), 1);
    assert_eq!((ev[0].kind, ev[0].name.as_str(), ev[0].ops.clone(), ev[0].by.as_str()), ("apply", "Faster", vec!["set_tempo"], "Test"));
    assert_eq!(newest, ev[0].seq);
    // An edit in the interface is an event on the next tick.
    a.state.project.channels[0].volume = 0.1;
    a.state.piano.dirty = true;
    a.tick();
    let (ev, _) = a.state.control.events_since(newest);
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
    assert_eq!(a.state.project.channels[0].volume, 1.0);
    let h = call(&mut a, &mut c, json!({"jsonrpc": "2.0", "id": 4, "method": "history"}));
    assert!(h["result"]["steps"].as_array().unwrap().iter().any(|s| s["name"] == "Assistant: louder kick"), "{h}");
    let r = call(&mut a, &mut c, json!({"jsonrpc": "2.0", "id": 5, "method": "undo"}));
    assert_eq!(r["result"]["step"], "Assistant: louder kick");
    assert_eq!(a.state.project.channels[0].volume, 0.78125);
    // Errors: the codes of JSON-RPC 2.0, and -32000 / -32001 for a failed or refused request.
    let code = |r: Value| r["error"]["code"].as_i64();
    assert_eq!(code(serde_json::from_str(&a.rpc_line(&mut c, "{not json").unwrap()).unwrap()), Some(-32700));
    assert_eq!(code(call(&mut a, &mut c, json!({"id": 6, "method": "state"}))), Some(-32600));
    assert_eq!(code(call(&mut a, &mut c, json!({"jsonrpc": "2.0", "id": 7, "method": "nope"}))), Some(-32601));
    assert_eq!(code(call(&mut a, &mut c, json!({"jsonrpc": "2.0", "id": 8, "method": "apply", "params": {"command": {"op": "fly"}}}))), Some(-32602));
    assert_eq!(code(call(&mut a, &mut c, json!({"jsonrpc": "2.0", "id": 9, "method": "apply", "params": {"command": {"op": "delete_channel", "channel": 9999}}}))), Some(-32000));
    let mut reader = rpc::Client { name: "Reader".into(), level: Level::Read, saves: false, token: None };
    assert_eq!(code(call(&mut a, &mut reader, json!({"jsonrpc": "2.0", "id": 10, "method": "apply", "params": {"command": {"op": "set_tempo", "bpm": 99}}}))), Some(-32001));
    assert_eq!(a.state.tempo, 130.0);
    // A notification (no id) gets no answer.
    assert!(a.rpc_line(&mut c, &json!({"jsonrpc": "2.0", "method": "undo"}).to_string()).is_none());
    // Notes read back as commands write them.
    let pat = a.state.project.patterns[0].id.0;
    call(&mut a, &mut c, json!({"jsonrpc": "2.0", "id": 11, "method": "apply", "params": {"command": {"op": "set_notes", "pattern": pat, "notes": [{"channel": kick, "position": 0, "length": 48, "key": 36, "velocity": 90, "pan": 64}]}}}));
    let n = call(&mut a, &mut c, json!({"jsonrpc": "2.0", "id": 12, "method": "notes", "params": {"pattern": pat}}));
    assert_eq!(n["result"]["notes"], json!([{"channel": kick, "position": 0, "length": 48, "key": 36, "velocity": 90, "pan": 64}]));
}

#[test]
fn hosted_plugins_follow_deletes_and_undo() {
    let mut a = app();
    a.plugins = crate::plugins::Plugins::default();
    a.plugins.available = crate::plugins::builtin();
    a.plugin_tick();
    a.plugin_tick();
    let (ch, ..) = ids(&a);
    let rows = |a: &App| a.plugins.hosted.range(..crate::plugins::FX_KEY).map(|(&r, h)| (r, h.owner.0)).collect::<Vec<_>>();
    assert_eq!(rows(&a), ch.iter().copied().enumerate().collect::<Vec<_>>(), "each sampler hosted, owned by its channel");
    let mut c = rpc::Client { name: "Test".into(), level: Level::Edit, saves: false, token: None };
    // A parameter set through the control plane, then its channel deleted and brought back.
    let params = a.rpc(&mut c, "params", &json!({"owner": ch[1]})).ok().unwrap();
    let p = params["params"].as_array().unwrap().iter().find(|p| p["steps"] == 0 && p["read_only"] == false && p["value"].as_f64().is_some_and(|v| v < 0.9)).unwrap().clone();
    let pid = p["id"].as_u64().unwrap();
    a.rpc(&mut c, "apply", &json!({"command": {"op": "set_param", "owner": ch[1], "param": pid, "value": 0.9}})).ok().unwrap();
    a.plugin_tick();
    let value = |a: &App, row: usize| a.plugins.hosted[&row].plugin.lock().unwrap().param_value(pid as u32).unwrap();
    assert!((value(&a, 1) - 0.9).abs() < 1e-6);
    a.rpc(&mut c, "apply", &json!({"command": {"op": "delete_channel", "channel": ch[1]}})).ok().unwrap();
    a.plugin_tick();
    let want: Vec<(usize, u32)> = [ch[0], ch[2], ch[3], ch[4]].into_iter().enumerate().collect();
    assert_eq!(rows(&a), want, "the rows below moved up with their instances");
    assert_eq!(a.state.hosted_rows.iter().copied().collect::<std::collections::BTreeSet<_>>(), (0..4).collect());
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
    a.plugins = crate::plugins::Plugins::default();
    a.plugins.effects = crate::plugins::builtin_effects();
    let Some(name) = a.plugins.effects.first().map(|e| e.desc.name.clone()) else { return };
    let ins1 = ids(&a).1[1];
    let r = apply(&mut a.state, &test_client(), None, vec![cmd(json!({"op": "add_effect", "track": ins1, "slot": 2, "plugin": name}))]).unwrap();
    let slot = r[0]["slot"].as_u64().unwrap() as u32;
    a.plugin_tick();
    let key = crate::plugins::fx_key(1, 2);
    assert_eq!(a.plugins.hosted.get(&key).map(|h| h.owner.0), Some(slot));
    #[cfg(plugin_windows)]
    assert!(a.plugins.hosted[&key].window.is_none());
    crate::history::undo(&mut a.state);
    a.plugin_tick();
    assert!(!a.plugins.hosted.contains_key(&key), "undone: the instance goes");
    crate::history::redo(&mut a.state);
    a.plugin_tick();
    assert_eq!(a.plugins.hosted.get(&key).map(|h| h.owner.0), Some(slot), "redone: back from what it kept");
}

#[cfg(unix)]
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
    assert_eq!(a.state.tempo, 111.0);
    let h = &a.state.history;
    assert_eq!(h.entries().into_iter().find(|e| e.0 == h.at()).map(|e| e.1.to_string()).as_deref(), Some("Socket: tempo"), "the client's name stays with its connection");
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
    assert_eq!(saved.channels.len(), a.state.project.channels.len());
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_first_clip_makes_the_playlist_and_its_undo_takes_it_away() {
    let mut a = app();
    a.state.project.arrangements.clear();
    let pat = ids(&a).2[0];
    let r = round_trip(&mut a, json!({"op": "add_clip", "track": 1, "position": 0, "pattern": pat}));
    assert!(r[0]["clip"].as_u64().is_some());
    crate::history::undo(&mut a.state);
    assert!(a.state.project.arrangements.is_empty());
}

/// Does `f` (an edit in the interface) and checks it is one undo step named `name` that undo and redo take back and
/// give again exactly.
fn ui_round_trip(a: &mut App, name: &str, f: impl FnOnce(&mut App)) {
    let before = reading(a);
    let steps = a.state.history.at();
    f(a);
    a.state.control.settle(&mut a.state.project);
    let after = reading(a);
    assert_ne!(before, after, "{name} changes something");
    assert_eq!(a.state.history.at(), steps + 1, "{name} is one step");
    assert_eq!(crate::history::undo(&mut a.state).as_deref(), Some(name));
    let now = reading(a);
    assert!(now == before, "{name} undone: {:?}, the model alike: {}", query::changes(&before.1, &now.1), now.0 == before.0);
    assert_eq!(crate::history::redo(&mut a.state).as_deref(), Some(name));
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
    crate::mixer::track_mut(&mut a.state, 3).unwrap().routes = vec![5];
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
    a.plugins.effects = vec![crate::plugin_db::fake("Test FX", os_host::Kind::Effect)];
    a.state.selected_mixer = 2;
    a.state.mixer.sel.clear();
    ui_round_trip(&mut a, "Add effect", |a| {
        run(a, MCmd::AddEffect(0, 0));
    });
    run(&mut a, MCmd::AddEffect(0, 0));
    a.state.control.settle(&mut a.state.project);
    ui_round_trip(&mut a, "Move effect", |a| {
        run(a, MCmd::SlotMove(0, 1));
    });
    // A slot's name is kept beside the project (the reading does not show it).
    crate::playlist::start_edit(&mut a.state, crate::playlist::EditTarget::SlotName(2, 1), (0.0, 0.0));
    a.state.playlist.edit.as_mut().unwrap().text = "Glue".into();
    crate::playlist::commit_edit(&mut a.state);
    assert_eq!(crate::mixer::slot_name(&a.state, 2, 1).as_deref(), Some("Glue"));
    assert_eq!(crate::history::undo(&mut a.state).as_deref(), Some("Slot name"));
    assert_eq!(crate::mixer::slot_name(&a.state, 2, 1).as_deref(), Some("Test FX"));
    assert_eq!(crate::history::redo(&mut a.state).as_deref(), Some("Slot name"));
    assert_eq!(crate::mixer::slot_name(&a.state, 2, 1).as_deref(), Some("Glue"));
    ui_round_trip(&mut a, "Remove effect", |a| {
        run(a, MCmd::SlotDelete(1));
    });
    // A fader drag is one step; a press that doesn't move it is none.
    let steps = a.state.history.at();
    crate::control::edit_press(&mut a.state, "Mixer track volume", command::Region::Track(1));
    crate::history::settle(&mut a.state);
    assert_eq!(a.state.history.at(), steps, "a click without a drag");
    ui_round_trip(&mut a, "Mixer track volume", |a| {
        crate::control::edit_press(&mut a.state, "Mixer track volume", command::Region::Track(1));
        a.state.project.mixer[1].volume = Some(0.3);
        crate::history::settle(&mut a.state);
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
    a.plugins = crate::plugins::Plugins::default();
    a.plugins.available = crate::plugins::builtin();
    a.plugin_tick();
    a.plugin_tick();
    let ch = ids(&a).0;
    let key = |a: &App, id: u32| a.plugins.hosted.iter().find(|(_, h)| h.owner.0 == id).map(|(&k, _)| k);
    crate::rack::delete_step(&mut a, &[1]);
    a.plugin_tick();
    assert_eq!(key(&a, ch[1]), None);
    assert_eq!(key(&a, ch[2]), Some(1));
    crate::history::undo(&mut a.state);
    a.plugin_tick();
    assert_eq!((0..5).map(|r| key(&a, ch[r])).collect::<Vec<_>>(), (0..5).map(Some).collect::<Vec<_>>());
    assert_eq!(a.state.hosted_rows.iter().copied().collect::<std::collections::BTreeSet<_>>(), (0..5).collect());
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
    assert_eq!(a.state.tempo, 130.0);
    // A web page's request: one parse error, then the connection is closed.
    assert_eq!(code(&ask(&mut a, "POST / HTTP/1.1")), Some(-32700));
    let mut rest = String::new();
    assert_eq!(r.read_line(&mut rest).unwrap(), 0, "closed");
}

#[test]
fn a_replaced_generator_stays_replaced() {
    let mut a = app();
    a.plugins = crate::plugins::Plugins::default();
    a.plugins.available = crate::plugins::builtin();
    a.plugin_tick();
    a.plugin_tick();
    let trio = a.plugins.available.iter().position(|p| p.desc.name == crate::osc3::LEMON_TRIO).unwrap();
    crate::plugin_menu::replace_generator(&mut a, trio, 0);
    for _ in 0..3 {
        a.plugin_tick();
    }
    assert_eq!(a.plugins.descriptor(0).map(|d| d.name), Some(crate::osc3::LEMON_TRIO.to_string()), "the old sampler is not hosted again");
    assert_eq!(a.state.hosted_rows.iter().filter(|&&r| r == 0).count(), 1);
    // And back: Lemon Sampler hosts the channel, not the Lemon Trio it replaced.
    let sampler = a.plugins.available.iter().position(|p| p.desc.name == crate::plugins::LEMON_SAMPLER).unwrap();
    crate::plugin_menu::replace_generator(&mut a, sampler, 0);
    for _ in 0..3 {
        a.plugin_tick();
    }
    assert_eq!(a.plugins.descriptor(0).map(|d| d.name), Some(crate::plugins::LEMON_SAMPLER.to_string()));
}

#[test]
fn a_dry_run_leaves_the_instruments_and_the_selections() {
    let mut a = app();
    a.plugins = crate::plugins::Plugins::default();
    a.plugins.available = crate::plugins::builtin();
    a.plugin_tick();
    a.plugin_tick();
    let ch = ids(&a).0;
    a.state.rack.select_only(1);
    a.state.selected_channel = 1;
    let live = a.plugins.hosted[&1].plugin.clone();
    let rows = a.state.hosted_rows.clone();
    propose(&mut a.state, &test_client(), "Fewer", vec![cmd(json!({"op": "delete_channel", "channel": ch[1]}))]).unwrap();
    assert_eq!((a.state.selected_channel, a.state.rack.sel.iter().copied().collect::<Vec<_>>()), (1, vec![1]));
    assert_eq!(a.state.hosted_rows, rows);
    a.plugin_tick();
    a.plugin_tick();
    assert!(std::sync::Arc::ptr_eq(&a.plugins.hosted[&1].plugin, &live), "the same instance plays the channel");
    // A transaction that fails half way leaves them too.
    let bad = vec![cmd(json!({"op": "delete_channel", "channel": ch[1]})), cmd(json!({"op": "set_tempo", "bpm": 1}))];
    assert!(apply(&mut a.state, &test_client(), None, bad).is_err());
    a.plugin_tick();
    assert!(std::sync::Arc::ptr_eq(&a.plugins.hosted[&1].plugin, &live));
    assert_eq!(a.state.hosted_rows, rows);
}

#[test]
fn slot_names_come_back_with_their_track() {
    let mut a = app();
    a.state.mixer.slot_names.insert((3, 1), "Crunch".into());
    crate::mixer::run(&mut a, crate::mixer::MCmd::Delete(2));
    assert_eq!(a.state.mixer.slot_names.get(&(2, 1)).map(String::as_str), Some("Crunch"), "moved left with its track");
    crate::history::undo(&mut a.state);
    assert_eq!(a.state.mixer.slot_names.get(&(3, 1)).map(String::as_str), Some("Crunch"));
    assert_eq!(a.state.mixer.slot_names.len(), 1);
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
    a.plugins = crate::plugins::Plugins::default();
    let desc = crate::plugins::builtin()[0].desc.clone();
    let knob = std::sync::Arc::new(std::sync::Mutex::new(Knob { desc, value: [0.25, 0.5], events: Vec::new() }));
    let mut h = crate::plugins::Hosted::new(knob.clone());
    h.owner = a.state.project.channels[0].id;
    a.plugins.hosted.insert(0, h);
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
    let steps = a.state.history.at();
    drag(&mut a, vec![Begin(7), Value(7, 0.4), Value(7, 0.6)]);
    assert_eq!(a.state.history.at(), steps, "nothing until the gesture ends");
    drag(&mut a, vec![Value(7, 0.7), End(7)]);
    assert_eq!(a.state.history.at(), steps + 1, "a gesture is one step");
    assert_eq!(crate::history::undo(&mut a.state).as_deref(), Some(command::PLUGIN_PARAMETER));
    assert_eq!(value(&mut a)[0], 0.25, "back to the value before the gesture");
    // Undo, tweak, undo: the tweak moves from the value the undo put back.
    drag(&mut a, vec![Begin(7), Value(7, 0.5), End(7)]);
    crate::history::undo(&mut a.state);
    assert_eq!(value(&mut a)[0], 0.25);
    crate::history::redo(&mut a.state);
    assert_eq!(value(&mut a)[0], 0.5);
    let steps = a.state.history.at();
    // A gesture that ends where it began is none; so is the plugin echoing a value the host set.
    drag(&mut a, vec![Begin(7), Value(7, 0.9), Value(7, 0.5), End(7)]);
    drag(&mut a, vec![Value(7, 0.5)]);
    assert_eq!(a.state.history.at(), steps);
    // Values sent without a gesture (an X-Y pad, some VST2 plugins) make one step while they come in a row, whichever
    // parameters they move.
    drag(&mut a, vec![Value(7, 0.1), Value(8, 0.2)]);
    drag(&mut a, vec![Value(7, 0.15), Value(8, 0.3)]);
    drag(&mut a, vec![Value(8, 0.4)]);
    assert_eq!(a.state.history.at(), steps + 1);
    crate::history::undo(&mut a.state);
    assert_eq!(value(&mut a), [0.5, 0.5], "both back to before the run");
    crate::history::redo(&mut a.state);
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
    use crate::presets::{load_into, save, Header};
    let mut a = app();
    a.plugins = crate::plugins::Plugins::default();
    let desc = crate::plugins::builtin()[0].desc.clone();
    let knob = std::sync::Arc::new(std::sync::Mutex::new(Knob { desc: desc.clone(), value: [0.25, 0.5], events: Vec::new() }));
    let mut h = crate::plugins::Hosted::new(knob.clone());
    h.owner = a.state.project.channels[0].id;
    a.plugins.hosted.insert(0, h);
    a.state.project.channels[0].name = Some("808 Snare".into());
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
        (knob.lock().unwrap().value, a.state.project.channels[row].name.clone().unwrap_or_default(), a.plugins.preset_name(row))
    };
    let at = |v: [f64; 2], name: &str, preset: Option<&str>| (v, name.to_string(), preset.map(str::to_string));

    let steps = a.state.history.at();
    assert_eq!(load_into(&mut a, 0, &bright).as_deref(), Some("Preset: Bright"));
    assert_eq!((a.state.history.at(), a.state.history.newest_done()), (steps + 1, Some(command::PLUGIN_PRESET)));
    assert_eq!(seen(&mut a, 0), at([0.9, 0.1], "Bright", Some("Bright")), "Auto name channels is on by default");
    load_into(&mut a, 0, &dark);
    load_into(&mut a, 0, &dark);
    assert_eq!(a.state.history.at(), steps + 2, "the same preset again changes nothing");

    // The channel moves without a step of its own: the instance follows it, and so does the undo.
    a.state.project.channels.swap(0, 1);
    a.follow_ids(true, false);
    assert!(a.plugins.hosted.contains_key(&1));
    assert_eq!(crate::history::undo(&mut a.state).as_deref(), Some(command::PLUGIN_PRESET));
    assert_eq!(seen(&mut a, 1), at([0.9, 0.1], "Bright", Some("Bright")));
    crate::history::undo(&mut a.state);
    assert_eq!(seen(&mut a, 1), at([0.25, 0.5], "808 Snare", None), "back to before the first preset");
    crate::history::redo(&mut a.state);
    assert_eq!(seen(&mut a, 1), at([0.9, 0.1], "Bright", Some("Bright")));
    crate::history::redo(&mut a.state);
    assert_eq!(seen(&mut a, 1), at([0.1, 0.9], "Dark", Some("Dark")));

    // Several undos and redos before the app's tick (the browser's History, a client) come out the same.
    crate::history::undo(&mut a.state);
    crate::history::undo(&mut a.state);
    crate::history::redo(&mut a.state);
    assert_eq!(seen(&mut a, 1), at([0.9, 0.1], "Bright", Some("Bright")));
    crate::history::redo(&mut a.state);
    assert_eq!(seen(&mut a, 1), at([0.1, 0.9], "Dark", Some("Dark")));

    // An edit the host saw no step for (Undo knob tweaks off, a plugin's own page) comes back with the redo.
    knob.lock().unwrap().value = [0.3, 0.7];
    crate::history::undo(&mut a.state);
    assert_eq!(seen(&mut a, 1).0, [0.9, 0.1]);
    crate::history::redo(&mut a.state);
    assert_eq!(seen(&mut a, 1).0, [0.3, 0.7]);

    // A mixer slot: the name "Auto name effect slots" gave it goes with the undo.
    crate::prefs::set_check(&crate::prefs::AUTO_NAME_EFFECT_SLOTS, true);
    let fx = crate::plugins::fx_key(1, 0);
    let slot_knob = std::sync::Arc::new(std::sync::Mutex::new(Knob { desc, value: [0.5, 0.5], events: Vec::new() }));
    a.plugins.hosted.insert(fx, crate::plugins::Hosted::new(slot_knob.clone()));
    crate::mixer::track_mut(&mut a.state, 1).unwrap().slots[0] = Some(crate::plugins::new_slot("Fruity Delay 3"));
    let slot = |a: &App| crate::mixer::slot_name(&a.state, 1, 0).unwrap_or_default();
    let plain = slot(&a);
    let steps = a.state.history.at();
    load_into(&mut a, fx, &bright);
    assert_eq!((a.state.history.at(), slot(&a), slot_knob.lock().unwrap().value), (steps + 1, "Bright".to_string(), [0.9, 0.1]));
    crate::history::undo(&mut a.state);
    a.host_commands();
    assert_eq!((slot(&a), slot_knob.lock().unwrap().value, a.plugins.preset_name(fx)), (plain.clone(), [0.5, 0.5], None));
    crate::history::redo(&mut a.state);
    a.host_commands();
    assert_eq!((slot(&a), slot_knob.lock().unwrap().value), ("Bright".to_string(), [0.9, 0.1]));

    // A preset dropped where a new instance was made loads into it as part of the step that made it.
    let steps = a.state.history.at();
    a.plugins.preset_waits.push((1, dark.clone(), std::time::Instant::now()));
    crate::presets::pending_tick(&mut a);
    assert_eq!((a.state.history.at(), knob.lock().unwrap().value), (steps, [0.1, 0.9]));
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
    let steps = a.state.history.at();
    crate::mixer::hit(&mut a, crate::mixer::MHit::Led(2));
    crate::mixer::run(&mut a, crate::mixer::MCmd::Solo(1));
    assert_eq!(a.state.history.at(), steps, "a mixer track's mute and solo make no steps");
    let on = |a: &App| (a.state.project.channels.iter().map(|c| c.enabled).collect::<Vec<_>>(), a.state.project.mixer.iter().map(|m| m.enabled).collect::<Vec<_>>());
    // A step on channel 0, then a mixer track muted: its undo and redo keep the track muted.
    let track = a.state.project.channels[0].mixer_track;
    a.channel_control(0, crate::history::ChannelControl::Track, |c| c.mixer_track = 5);
    crate::mixer::hit(&mut a, crate::mixer::MHit::Led(3));
    let now = on(&a);
    crate::history::undo(&mut a.state).unwrap();
    assert_eq!(a.state.project.channels[0].mixer_track, track, "undone");
    assert_eq!(on(&a), now, "the mutes stay through the undo");
    crate::history::redo(&mut a.state).unwrap();
    assert_eq!(on(&a), now, "and the redo");
    // With Undo knob tweaks off, a channel's mute makes no step either, and an undo of an earlier step keeps it.
    let was = crate::prefs::check(&crate::prefs::UNDO_KNOB_TWEAKS);
    crate::prefs::set_check(&crate::prefs::UNDO_KNOB_TWEAKS, false);
    let steps = a.state.history.at();
    crate::rack::toggle_mute(&mut a.state, 0);
    let now = on(&a);
    crate::prefs::set_check(&crate::prefs::UNDO_KNOB_TWEAKS, was);
    assert_eq!(a.state.history.at(), steps, "no step with Undo knob tweaks off");
    crate::history::undo(&mut a.state).unwrap();
    assert_eq!(on(&a), now, "the mute stays through an undo of the step before it");
}
