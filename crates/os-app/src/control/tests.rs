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

/// The project and the rack's state of it, ids included (`==` on the model leaves ids out).
fn reading(a: &App) -> (os_model::Project, Value, f64) {
    (a.state.project.clone(), query::project(&a.state), a.state.tempo)
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
        json!({"op": "set_channel", "channel": kick, "name": "Big kick", "color": "#ff8000", "volume": 0.5, "pan": -0.25, "pitch": 100, "enabled": false, "track": ins2}),
        json!({"op": "load_sample", "channel": clap, "path": "/no/such/folder/snare.wav"}),
        json!({"op": "set_track", "track": ins1, "name": "Drums", "color": "#102030", "volume": 0.6, "pan": 0.5, "stereo": -0.5, "enabled": false, "polarity": true, "swap": true, "effects_enabled": false}),
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
    let mut c = rpc::Client { name: "Helper".into(), level: Level::Propose };
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
    let mut c = rpc::Client { name: "Client 1".into(), level: Level::Edit };
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
    let mut reader = rpc::Client { name: "Reader".into(), level: Level::Read };
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
    let mut c = rpc::Client { name: "Test".into(), level: Level::Edit };
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
    a.rpc_server = Some(server::start(path.to_str().unwrap(), Level::Edit, || {}).unwrap());
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
