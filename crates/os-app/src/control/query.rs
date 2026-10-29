// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The project as clients read it: JSON with everything named by id (the same ids commands take), in the units the
//! commands use. `changes` compares two readings (what a proposal changes).

use os_model::{ChannelKind, ClipSource, Project};
use serde::Serialize;
use serde_json::{json, Map, Value};

use crate::app::State;
use crate::control::command::color_text;

fn kind(k: ChannelKind) -> &'static str {
    match k {
        ChannelKind::Sampler => "sampler",
        ChannelKind::Plugin => "plugin",
        ChannelKind::Layer => "layer",
        ChannelKind::AudioClip => "audio_clip",
        ChannelKind::Automation => "automation",
        ChannelKind::Other(_) => "other",
    }
}

fn track_id(p: &Project, n: u16) -> Option<u32> {
    p.mixer.iter().find(|m| m.index == n).map(|m| m.id.0)
}

/// The project: settings, channels, mixer tracks in use, patterns and the playlist shown. Notes are read per pattern
/// (`notes`).
pub fn project(s: &State) -> Value {
    let p = &s.project;
    let (beats, steps) = p.bar();
    let channels: Vec<Value> = p
        .channels
        .iter()
        .enumerate()
        .map(|(row, c)| {
            json!({
                "id": c.id.0,
                "row": row,
                "name": crate::rack::channel_name(s, row),
                "kind": kind(c.kind),
                "plugin": c.plugin,
                "sample": c.sample_path,
                "color": color_text(c.color),
                "volume": c.volume,
                "pan": c.pan,
                "pitch": c.pitch_cents,
                "enabled": c.enabled,
                "track": track_id(p, c.mixer_track),
            })
        })
        .collect();
    let used = crate::mixer::inserts(s);
    let tracks: Vec<Value> = p
        .mixer
        .iter()
        .filter(|m| m.index <= used)
        .map(|m| {
            let slots: Vec<Value> = m
                .slots
                .iter()
                .enumerate()
                .filter_map(|(k, sl)| {
                    let sl = sl.as_ref()?;
                    Some(json!({ "slot": k, "id": sl.id.0, "plugin": crate::plugins::slot_label(&sl.name), "enabled": m.slot_on[k], "mix": m.slot_mix[k] }))
                })
                .collect();
            let sends: Vec<Value> = m
                .routes
                .iter()
                .map(|&d| {
                    let level = m.send_levels.iter().find(|(x, _)| *x == d).map_or(0.8, |&(_, l)| l);
                    json!({ "to": track_id(p, d), "level": level })
                })
                .collect();
            json!({
                "id": m.id.0,
                "number": m.index,
                "name": crate::mixer::name(s, m.index),
                "color": color_text(m.color),
                "volume": m.volume.unwrap_or(0.8),
                "pan": m.pan.unwrap_or(0.0),
                "stereo": m.stereo_separation.unwrap_or(0.0),
                "enabled": m.enabled,
                "polarity": m.polarity,
                "swap": m.swap,
                "effects_enabled": m.slots_enabled,
                "eq": m.eq,
                "sends": sends,
                "slots": slots,
            })
        })
        .collect();
    let patterns: Vec<Value> = p
        .patterns
        .iter()
        .filter(|q| crate::patterns::used(q) || q.name.is_some())
        .map(|q| {
            json!({
                "id": q.id.0,
                "number": q.number,
                "name": crate::patterns::name_of(s, q.number),
                "color": color_text(q.color),
                "length": crate::playlist::source_len(s, os_model::ClipSource::Pattern(q.number)),
                "auto_length": q.length.is_none(),
                "notes": q.notes.len(),
            })
        })
        .collect();
    let playlist = p.arrangement().map(|a| {
        let clips: Vec<Value> = a
            .clips
            .iter()
            .map(|c| {
                let mut v = json!({ "id": c.id.0, "track": c.track + 1, "position": c.position, "length": c.length, "muted": c.muted });
                match c.source {
                    ClipSource::Pattern(n) => v["pattern"] = json!(p.pattern(n).map(|q| q.id.0)),
                    ClipSource::Channel(i) => v["channel"] = json!(p.channels.iter().find(|ch| ch.index == i).map(|ch| ch.id.0)),
                }
                v
            })
            .collect();
        let tracks: Vec<Value> = a
            .tracks
            .iter()
            .filter(|t| t.name.is_some() || t.color.is_some() || !t.enabled)
            .map(|t| json!({ "track": t.number, "name": t.name, "color": color_text(t.color), "enabled": t.enabled }))
            .collect();
        json!({ "id": a.id.0, "name": a.name, "clips": clips, "tracks": tracks })
    });
    let i = &p.info;
    json!({
        "ppq": p.ppq,
        "tempo": s.tempo,
        "time_signature": if p.bar_is_signature() { json!([beats, 16 / steps.max(1)]) } else { Value::Null },
        "bar": { "beats": beats, "steps_per_beat": steps },
        "swing": f64::from(p.swing.unwrap_or(0)) / 128.0,
        "main_volume": f64::from(p.main_volume.unwrap_or(Project::MAIN_VOLUME_DEFAULT)) / 16000.0,
        "main_pitch": p.main_pitch.unwrap_or(0),
        "info": { "title": i.title, "author": i.author, "genre": i.genre, "comments": i.comments, "url": i.url },
        "file": s.path.as_ref().map(|p| p.display().to_string()),
        "song_mode": s.song_mode,
        "current_pattern": p.pattern(s.pattern).map(|q| q.id.0),
        "selected_channel": p.channels.get(s.selected_channel).map(|c| c.id.0),
        "channels": channels,
        "mixer": tracks,
        "patterns": patterns,
        "playlist": playlist,
    })
}

/// Pattern `id`'s notes, as `command::NoteIn` takes them (with every stored field a note command sets).
pub fn notes(s: &State, id: u32) -> Result<Value, String> {
    let p = &s.project;
    let q = &p.patterns[crate::control::command::pattern_place(p, id)?];
    let notes: Vec<Value> = q
        .notes
        .iter()
        .map(|n| {
            let ch = p.channels.iter().find(|c| c.index == n.channel).map(|c| c.id.0);
            json!({ "channel": ch, "position": n.position, "length": n.length, "key": n.key, "velocity": n.velocity, "pan": n.pan })
        })
        .collect();
    Ok(json!({ "pattern": id, "notes": notes }))
}

/// One difference between two readings: where (a path of keys and ids), and the values on each side (`null`: not
/// there).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Change {
    pub path: String,
    pub before: Value,
    pub after: Value,
}

/// The differences between readings `a` and `b`. Lists of things with ids are matched by id, so a thing added or
/// gone is one change and the others are compared field by field.
pub fn changes(a: &Value, b: &Value) -> Vec<Change> {
    let mut out = Vec::new();
    diff("", a, b, &mut out);
    out
}

fn by_id(v: &[Value]) -> Option<Map<String, Value>> {
    let mut m = Map::new();
    for x in v {
        let id = x.get("id")?.as_u64()?;
        m.insert(id.to_string(), x.clone());
    }
    Some(m)
}

fn diff(path: &str, a: &Value, b: &Value, out: &mut Vec<Change>) {
    if a == b {
        return;
    }
    let at = |k: &str| if path.is_empty() { k.to_string() } else { format!("{path}.{k}") };
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            for (k, va) in x {
                diff(&at(k), va, y.get(k).unwrap_or(&Value::Null), out);
            }
            for (k, vb) in y.iter().filter(|(k, _)| !x.contains_key(*k)) {
                diff(&at(k), &Value::Null, vb, out);
            }
        }
        (Value::Array(x), Value::Array(y)) => match (by_id(x), by_id(y)) {
            (Some(mx), Some(my)) if !x.is_empty() || !y.is_empty() => diff(path, &Value::Object(mx), &Value::Object(my), out),
            _ => out.push(Change { path: path.to_string(), before: a.clone(), after: b.clone() }),
        },
        _ => out.push(Change { path: path.to_string(), before: a.clone(), after: b.clone() }),
    }
}
