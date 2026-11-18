// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Commands fuzzed directly (control-plane step 13, specs roadmap/control-plane.md "Command-level fuzzing"): random
//! lists of commands, made from the ids the project holds at the time, applied to two copies. Each list is whole or
//! not at all and one step; both copies end the same (determinism); its JSON reads back as the same commands; undoing
//! every list gives the project as opened and redoing them the end again; and the end saves and reopens the same.
//! A seeded generator of our own stands for `arbitrary`: a failure names its seed and list, so it can be run again.

#![allow(deprecated)] // writes the project directly (`crate::locked`; control-plane step 14 removes this)
use serde_json::{json, Value};

use super::*;
use crate::app::App;
use crate::control_ui::*;

/// xorshift64*: the same lists for the same seed on every machine.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
    fn chance(&mut self, p: f64) -> bool {
        self.unit() < p
    }
    /// One of `v`, or now and then an id nothing holds (so a list fails and must change nothing).
    fn id(&mut self, v: &[u32]) -> u32 {
        if v.is_empty() || self.chance(0.03) {
            return 999_999;
        }
        v[self.below(v.len())]
    }
    fn color(&mut self) -> String {
        format!("#{:06x}", self.next() & 0xff_ffff)
    }
}

fn app() -> App {
    let mut a = App::new(None);
    a.state.session.project = crate::locked::Locked::new(crate::projects::test_project());
    a.state.session.project.get_mut().arrangements = vec![os_flp::project::default_arrangement()];
    a.state.session.control.settle(a.state.session.project.get_mut());
    a
}

/// The project and the client's view of it (ids included), and the tempo.
fn reading(a: &App) -> (os_model::Project, Value, f64) {
    let mut q = crate::control::query_ui::project(&a.state);
    q.as_object_mut().unwrap().remove("selected_channel");
    (a.state.session.project.clone(), q, a.state.tempo())
}

/// One random command on what `a` holds now.
fn command(r: &mut Rng, a: &App) -> Value {
    let p = &a.state.session.project;
    let ch: Vec<u32> = p.channels.iter().map(|c| c.id.0).collect();
    let tr: Vec<u32> = p.mixer.iter().map(|m| m.id.0).collect();
    let pa: Vec<u32> = p.patterns.iter().map(|q| q.id.0).collect();
    let clips: Vec<u32> = p.arrangement().map(|x| x.clips.iter().map(|c| c.id.0).collect()).unwrap_or_default();
    let note = |r: &mut Rng, ch: &[u32]| json!({"channel": r.id(ch), "position": r.below(4) * 96, "length": 1 + r.below(96), "key": 36 + r.below(48), "velocity": r.below(128)});
    match r.below(59) {
        0 => json!({"op": "set_tempo", "bpm": 40.0 + r.unit() * 200.0}),
        1 => json!({"op": "set_swing", "swing": r.unit()}),
        2 => json!({"op": "set_main_volume", "volume": r.unit()}),
        3 => json!({"op": "set_main_pitch", "cents": r.below(2401) as i64 - 1200}),
        4 => json!({"op": "set_time_signature", "numerator": 1 + r.below(16), "denominator": 2u32 << r.below(4)}),
        5 => json!({"op": "set_info", "title": format!("T{}", r.below(100)), "author": "Fuzz"}),
        6 => json!({"op": "set_channel", "channel": r.id(&ch), "name": format!("C{}", r.below(100)), "color": r.color(), "volume": r.unit(), "pan": r.unit() * 2.0 - 1.0, "pitch": r.below(201) as i64 - 100, "enabled": r.chance(0.5), "track": r.id(&tr), "declick": r.below(6), "swing": r.unit(), "truncate_swing": r.chance(0.5), "cut": r.below(3), "cut_by": r.below(3), "locked": r.chance(0.5)}),
        7 => json!({"op": "set_track", "track": r.id(&tr), "name": format!("M{}", r.below(100)), "color": r.color(), "volume": r.unit(), "pan": r.unit() * 2.0 - 1.0, "stereo": r.unit() * 2.0 - 1.0, "enabled": r.chance(0.5), "polarity": r.chance(0.5), "swap": r.chance(0.5), "effects_enabled": r.chance(0.5)}),
        8 => json!({"op": "set_send", "from": r.id(&tr), "to": r.id(&tr), "level": r.unit()}),
        9 => json!({"op": "set_eq", "track": r.id(&tr), "band": r.below(3), "gain": r.below(3601) as i64 - 1800, "frequency": r.below(65536)}),
        10 => json!({"op": "set_pattern", "pattern": r.id(&pa), "name": format!("P{}", r.below(100)), "color": r.color(), "length": r.below(4) * 384}),
        11 => {
            let c = r.id(&ch);
            json!({"op": "set_notes", "pattern": r.id(&pa), "channel": c, "notes": (0..r.below(4)).map(|_| { let mut n = note(r, &ch); n["channel"] = json!(c); n }).collect::<Vec<_>>()})
        }
        12 => json!({"op": "add_notes", "pattern": r.id(&pa), "notes": (0..1 + r.below(3)).map(|_| note(r, &ch)).collect::<Vec<_>>()}),
        13 => json!({"op": "set_playlist_track", "track": r.below(8), "name": format!("L{}", r.below(100)), "color": r.color(), "enabled": r.chance(0.5)}),
        // Linked to an insert (not the master), or not linked.
        55 => json!({"op": "set_playlist_track", "track": 1 + r.below(8), "link": if r.chance(0.5) && tr.len() > 1 { json!({"mixer": r.id(&tr[1..])}) } else { json!({}) }}),
        14 => json!({"op": "move_channel", "channel": r.id(&ch), "row": r.below(ch.len() + 1)}),
        15 => json!({"op": "delete_channel", "channel": r.id(&ch)}),
        16 => json!({"op": "add_channel", "name": format!("N{}", r.below(100)), "track": r.id(&tr)}),
        17 => json!({"op": "add_track", "at": r.below(tr.len() + 1)}),
        18 => json!({"op": "delete_track", "track": r.id(&tr)}),
        19 => json!({"op": "add_pattern", "name": format!("A{}", r.below(100)), "color": r.color()}),
        20 => json!({"op": "delete_pattern", "pattern": r.id(&pa)}),
        21 => json!({"op": "add_clip", "track": r.below(8), "position": r.below(16) * 96, "pattern": r.id(&pa)}),
        22 => json!({"op": "set_clip", "clip": r.id(&clips), "position": r.below(16) * 96, "track": r.below(8), "length": 1 + r.below(384), "muted": r.chance(0.5)}),
        23 => json!({"op": "delete_clip", "clip": r.id(&clips)}),
        24 => json!({"op": "set_slot", "track": r.id(&tr), "slot": r.below(10), "enabled": r.chance(0.5), "mix": r.unit()}),
        25 => json!({"op": "set_clip_lfo", "channel": r.id(&ch), "on": r.chance(0.5), "amount": r.below(129), "speed": r.below(65537), "tension": r.below(257) as i64 - 128}),
        26 => json!({"op": "set_midi_out", "channel": r.id(&ch), "midi_channel": r.below(16), "patch": r.below(129), "bank": r.below(129) as i64 - 1, "send_levels": r.chance(0.5)}),
        27 => json!({"op": "set_project", "beats": 1 + r.below(32), "steps": 1 + r.below(16), "signature": r.chance(0.5), "panning_law": if r.chance(0.5) { "circular" } else { "triangular" }, "play_truncated": r.chance(0.5), "pdc": r.chance(0.5)}),
        28 => json!({"op": "set_track", "track": r.id(&tr), "separator": r.chance(0.5), "dock": (["left", "middle", "right"][r.below(3)]), "threaded": r.chance(0.5), "latency_in": r.unit() * 20.0, "latency_out": -r.unit() * 20.0, "audio_in": r.below(3) as i32 - 1, "audio_out": r.below(3) as i32 - 1, "rec_source": r.below(6), "monitor": (["off", "armed", "on"][r.below(3)]), "armed": r.chance(0.5)}),
        29 => json!({"op": "reset_track", "track": r.id(&tr)}),
        30 => json!({"op": "set_auto_bypass", "on": r.chance(0.5)}),
        31 => json!({"op": "move_track", "track": r.id(&tr), "to": 1 + r.below(8)}),
        32 => json!({"op": "remove_tracks", "tracks": [r.id(&tr), r.id(&tr)]}),
        33 => json!({"op": "move_effect", "track": r.id(&tr), "slot": r.below(10), "to": r.below(10)}),
        35 => json!({"op": "shift_patterns", "from": 1 + r.below(6), "by": if r.chance(0.5) { 1 } else { -1 }}),
        36 => json!({"op": "move_pattern", "from": 1 + r.below(8), "to": 1 + r.below(8)}),
        37 => json!({"op": "clone_pattern", "pattern": r.id(&pa)}),
        38 => json!({"op": "transpose_pattern", "pattern": r.id(&pa), "semitones": r.below(25) as i64 - 12, "channels": if r.chance(0.5) { vec![r.id(&ch)] } else { Vec::new() }}),
        39 => json!({"op": "split_pattern", "pattern": r.id(&pa)}),
        40 => json!({"op": "set_pattern_signature", "pattern": r.id(&pa), "numerator": r.below(8), "denominator": 4}),
        57 => json!({"op": "set_misc", "channel": r.id(&ch), "values": {"arp_dir": r.below(6) as f64, "echo_feed": r.unit(), "time_gate": r.unit()}}),
        58 => json!({"op": "set_start_value", "target": 0x4000_0001u32 + [0, 1, 4][r.below(3)], "value": (if r.chance(0.3) { Value::Null } else { json!(r.unit()) })}),
        56 => json!({"op": "set_pattern_markers", "pattern": r.id(&pa), "markers": [{"position": r.below(4) * 96, "kind": 8, "numerator": 1 + r.below(8), "denominator": 4}]}),
        41 => json!({"op": "clone_channel", "channel": r.id(&ch)}),
        43 => json!({"op": "set_loops", "pattern": r.id(&pa), "mode": r.below(4), "loops": [{"channel": r.id(&ch), "value": if r.chance(0.5) { Some(1 + r.below(16)) } else { None }}]}),
        44 => json!({"op": "set_groups", "groups": ["A", "Unsorted"], "channels": [{"channel": r.id(&ch), "group": if r.chance(0.5) { Some(r.below(2)) } else { None }}], "shown": if r.chance(0.5) { Some(r.below(2)) } else { None }}),
        53 => json!({"op": "add_arrangement", "name": format!("A{}", r.below(9))}),
        // In position order, as the playlist keeps clips (a list out of order keeps its order, which a file sorts).
        51 => {
            let p = r.below(16) * 96;
            json!({"op": "set_clips", "clips": [{"id": r.id(&clips), "track": 1 + r.below(8), "position": p, "length": 1 + r.below(384)}, {"track": 1 + r.below(8), "position": p + 96 * (1 + r.below(8)), "length": 96, "pattern": 1 + r.below(4)}]})
        }
        52 => json!({"op": "set_markers", "markers": [{"position": r.below(8) * 384, "name": "A"}, {"position": r.below(8) * 384, "kind": r.below(6)}]}),
        50 => json!({"op": "add_marker", "marker": {"position": r.below(8) * 384, "name": format!("M{}", r.below(9)), "kind": r.below(6)}}),
        46 => json!({"op": "swap_playlist_tracks", "track": 1 + r.below(8), "with": 1 + r.below(8)}),
        47 => json!({"op": "insert_playlist_track", "at": 1 + r.below(8)}),
        48 => json!({"op": "delete_playlist_track", "track": 1 + r.below(8)}),
        49 => json!({"op": "reset_playlist_track", "track": 1 + r.below(8)}),
        45 => json!({"op": "replace_instrument", "channel": r.id(&ch), "plugin": if r.chance(0.5) { Some("MIDI Out") } else { None }}),
        42 => json!({"op": "set_step", "pattern": r.id(&pa), "channel": r.id(&ch), "step": r.below(32), "on": r.chance(0.5), "mute": r.chance(0.5)}),
        34 => json!({"op": "set_slot", "track": r.id(&tr), "slot": r.below(10), "name": format!("S{}", r.below(9)), "color": r.color()}),
        _ => json!({"op": "load_sample", "channel": r.id(&ch), "path": "/no/such/folder/snare.wav"}),
    }
}

/// `a == b`, or a panic naming where they first differ (the whole project is too long to read).
fn same<T: std::fmt::Debug + PartialEq>(a: &T, b: &T, what: &str) {
    if a != b {
        let (x, y) = (format!("{a:?}"), format!("{b:?}"));
        let i = x.bytes().zip(y.bytes()).take_while(|(p, q)| p == q).count();
        let from = x[..i].char_indices().rev().nth(300).map_or(0, |c| c.0);
        panic!("{what}: differs at\n  now: ...{}\n  was: ...{}", &x[from..(i + 200).min(x.len())], &y[from..(i + 200).min(y.len())]);
    }
}

/// The client's view as a reopened project can match it: ids (which a reopened project gives anew) turned into the
/// numbers and rows they name, numbers compared within what the file keeps (`close`).
fn as_saved(q: &Value) -> Value {
    let map = |key: &str, by: &str| -> std::collections::HashMap<u64, Value> {
        q[key].as_array().into_iter().flatten().filter_map(|t| Some((t["id"].as_u64()?, t[by].clone()))).collect()
    };
    let (tracks, patterns, channels) = (map("mixer", "number"), map("patterns", "number"), map("channels", "row"));
    let named = |m: &std::collections::HashMap<u64, Value>, v: &Value| v.as_u64().and_then(|id| m.get(&id).cloned()).unwrap_or(Value::Null);
    let mut q = q.clone();
    let o = q.as_object_mut().unwrap();
    o.remove("selected_channel");
    o.insert("current_pattern".into(), named(&patterns, &o["current_pattern"]));
    for c in o["channels"].as_array_mut().into_iter().flatten() {
        c["track"] = named(&tracks, &c["track"]);
    }
    for m in o["mixer"].as_array_mut().into_iter().flatten() {
        for d in m["sends"].as_array_mut().into_iter().flatten() {
            d["to"] = named(&tracks, &d["to"]);
        }
        // A reopened track lists its sends in the file's order.
        if let Some(v) = m["sends"].as_array_mut() {
            v.sort_by_key(|d| d["to"].to_string());
        }
    }
    for c in o["playlist"]["clips"].as_array_mut().into_iter().flatten() {
        if c.get("pattern").is_some() {
            c["pattern"] = named(&patterns, &c["pattern"]);
        }
        if c.get("channel").is_some() {
            c["channel"] = named(&channels, &c["channel"]);
        }
    }
    fn strip(v: &mut Value) {
        match v {
            Value::Object(m) => {
                m.remove("id");
                m.values_mut().for_each(strip);
            }
            Value::Array(a) => a.iter_mut().for_each(strip),
            _ => {}
        }
    }
    strip(&mut q);
    q
}

/// `a` and `b` the same, numbers within 0.02 of each other (volumes, pans and stereo are stored as integers).
fn close(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => (x.as_f64().unwrap() - y.as_f64().unwrap()).abs() <= 0.02,
        (Value::Array(x), Value::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(p, q)| close(p, q)),
        (Value::Object(x), Value::Object(y)) => x.len() == y.len() && x.iter().all(|(k, p)| y.get(k).is_some_and(|q| close(p, q))),
        _ => a == b,
    }
}

/// Where `a` and `b` first differ past `close`, as a path and the two values.
fn far(a: &Value, b: &Value, at: &str) -> Option<String> {
    match (a, b) {
        (Value::Array(x), Value::Array(y)) if x.len() == y.len() => x.iter().zip(y).enumerate().find_map(|(i, (p, q))| far(p, q, &format!("{at}[{i}]"))),
        (Value::Object(x), Value::Object(y)) if x.len() == y.len() && x.keys().all(|k| y.contains_key(k)) => x.iter().find_map(|(k, p)| far(p, &y[k], &format!("{at}.{k}"))),
        _ => (!close(a, b)).then(|| format!("{at}: {a} / {b}")),
    }
}

/// Every send goes to a track the mixer holds.
fn sends_land(a: &App, what: &str) {
    let q = crate::control::query_ui::project(&a.state);
    let p = &a.state.session.project;
    let lost: Vec<String> = q["mixer"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["sends"].as_array().unwrap().iter().any(|d| d["to"].is_null()))
        .map(|m| format!("track {}: routes {:?}", m["number"], p.mixer.iter().find(|t| u64::from(t.id.0) == m["id"].as_u64().unwrap()).map(|t| &t.routes)))
        .collect();
    let numbers: Vec<u16> = p.mixer.iter().map(|m| m.index).collect();
    assert!(lost.is_empty(), "{what}: a send to no track: {lost:?} (the mixer's numbers {numbers:?})");
}

fn fuzz(seed: u64, lists: usize) {
    let mut r = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
    let (mut a, mut b) = (app(), app());
    let opened = reading(&a);
    let client = By::Client("Fuzz".into());
    let mut done = 0;
    for k in 0..lists {
        let list: Vec<Value> = (0..1 + r.below(4)).map(|_| command(&mut r, &a)).collect();
        let what = format!("seed {seed}, list {k}: {}", Value::Array(list.clone()));
        let cmds: Vec<Command> = list.iter().map(|c| serde_json::from_value(c.clone()).unwrap_or_else(|e| panic!("{what}: {e}"))).collect();
        let back: Vec<Command> = serde_json::from_value(serde_json::to_value(&cmds).unwrap()).unwrap();
        assert_eq!(back, cmds, "{what}: the JSON form reads back");
        let (before, steps) = (reading(&a), a.state.session.history.at());
        let ra = apply(&mut a.state, &client, None, cmds.clone());
        let rb = apply(&mut b.state, &client, None, cmds);
        assert_eq!(ra.is_ok(), rb.is_ok(), "{what}: both copies agree");
        for x in [&mut a, &mut b] {
            x.state.session.control.settle(x.state.session.project.get_mut());
        }
        match ra {
            Ok(_) => {
                assert_eq!(a.state.session.history.at(), steps + 1, "{what}: one step");
                done += 1;
                // Undone and redone at once, which finds the list that breaks.
                let after = reading(&a);
                crate::history_ui::undo(&mut a.state).expect("an undo");
                same(&reading(&a), &before, &format!("{what}: undone"));
                crate::history_ui::redo(&mut a.state).expect("a redo");
                same(&reading(&a), &after, &format!("{what}: redone"));
                sends_land(&a, &what);
            }
            Err(_) => {
                assert_eq!(reading(&a), before, "{what}: a failed list changes nothing");
                assert_eq!(a.state.session.history.at(), steps, "{what}: and makes no step");
            }
        }
        same(&reading(&a), &reading(&b), &format!("{what}: the same list gives the same project"));
    }
    let end = reading(&a);
    for i in 0..done {
        assert!(crate::history_ui::undo(&mut a.state).is_some(), "seed {seed}: undo {i} of {done}");
    }
    same(&reading(&a), &opened, &format!("seed {seed}: undoing every list gives the project as opened"));
    for i in 0..done {
        assert!(crate::history_ui::redo(&mut a.state).is_some(), "seed {seed}: redo {i} of {done}");
    }
    same(&reading(&a), &end, &format!("seed {seed}: redoing them gives the end again"));
    sends_land(&a, &format!("seed {seed}: the end"));
    // Saved and opened again, the client sees the same project.
    let dir = std::env::temp_dir().join(format!("os_fuzz_{seed}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("fuzz.flp");
    a.save_to(file.clone());
    sends_land(&a, &format!("seed {seed}: saved"));
    let mut c = App::new(None);
    c.open(file);
    while c.loading() {
        c.tick();
    }
    let _ = std::fs::remove_dir_all(&dir);
    let (saved, reopened) = (as_saved(&crate::control::query_ui::project(&a.state)), as_saved(&crate::control::query_ui::project(&c.state)));
    if let Some(d) = far(&reopened, &saved, "") {
        panic!("seed {seed}: saved and opened again (reopened / saved): {}", &d[..d.len().min(1500)]);
    }
}

#[test]
fn commands_fuzzed() {
    for seed in 1..=8 {
        fuzz(seed, 90); // under the history's 100 steps
    }
}

