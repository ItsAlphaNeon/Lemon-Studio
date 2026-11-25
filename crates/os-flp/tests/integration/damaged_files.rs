// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

// spec: flp/container.md, flp/layouts.md
//
// Damaged files (issue #412): a stream with its parts out of place or cut short loads, or fails to, but never panics.
// One hand-built stream per panic found, then a seeded run of damaged and random streams made from a created project.

use os_flp::project::{create, default_arrangement, load};
use os_flp::{Chunk, ChunkBody, Event, FlpFile, Header, Kind, Tag, Value, kind_of};
use os_model::{Channel, ChannelKind, Clip, ClipSource, MixerTrack, Note, Pattern, Project, SlotPlugin};
use std::collections::BTreeMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex};

const VERSION: u8 = 199;
const CH_START: u8 = 64;
const CH_END: u8 = 51;
const ARR_START: u8 = 99;
const PL_TRACK: u8 = 238;
const MX_COLOR_FLAG: u8 = 42;
const MX_FLAGS: u8 = 236;
const MX_PARAMS: u8 = 225;

/// A file of `events`, its header counting the channel blocks.
fn file(events: Vec<Event>) -> FlpFile {
    let channels = events.iter().filter(|e| e.id == CH_START).count() as u16;
    let mut header = 0u16.to_le_bytes().to_vec();
    header.extend(channels.to_le_bytes());
    header.extend(96u16.to_le_bytes());
    FlpFile {
        chunks: vec![
            Chunk { tag: Tag::HEADER, body: ChunkBody::Header(Header { raw: header }) },
            Chunk { tag: Tag::DATA, body: ChunkBody::Events(events) },
        ],
        trailing: Vec::new(),
    }
}

fn version(v: &str) -> Event {
    Event::new(VERSION, Value::var(format!("{v}\0").into_bytes()))
}

/// A mixer track's header and record, as the current layout has them.
fn mixer_track() -> [Event; 2] {
    [Event::new(MX_COLOR_FLAG, Value::U8(0)), Event::new(MX_FLAGS, Value::var(vec![0; 12]))]
}

// ---- one stream per panic -----------------------------------------------------------------------------------------

/// The mixer parameters (225) between two mixer tracks: the last track's events ran from its header to the parameters
/// event before it.
#[test]
fn mixer_params_between_tracks_load() {
    let mut ev = vec![
        version("24.2.0.0"),
        Event::new(CH_START, Value::U16(0)),
        Event::new(CH_END, Value::U8(0)),
        Event::new(ARR_START, Value::U16(0)),
    ];
    ev.extend(mixer_track());
    ev.push(Event::new(MX_PARAMS, Value::var(Vec::new())));
    ev.extend(mixer_track());
    let p = load(&file(ev)).expect("loads");
    assert_eq!(p.mixer.len(), 2);
}

/// The same in a file of an older generation, which is converted on loading: the last track's events ran to the
/// parameters event before its own record.
#[test]
fn legacy_mixer_params_between_tracks_load() {
    let ev = vec![
        version("20.0.0.0"),
        Event::new(CH_START, Value::U16(0)),
        Event::new(MX_FLAGS, Value::var(vec![0; 12])),
        Event::new(MX_PARAMS, Value::var(Vec::new())),
        Event::new(MX_FLAGS, Value::var(vec![0; 12])),
    ];
    let p = load(&file(ev)).expect("loads");
    assert_eq!(p.mixer.len(), 2);
}

/// The mixer parameters before every mixer track in a file of an older generation.
#[test]
fn legacy_mixer_params_before_tracks_load() {
    let ev = vec![
        version("20.0.0.0"),
        Event::new(CH_START, Value::U16(0)),
        Event::new(MX_PARAMS, Value::var(Vec::new())),
        Event::new(MX_FLAGS, Value::var(vec![0; 12])),
    ];
    let p = load(&file(ev)).expect("loads");
    assert_eq!(p.mixer.len(), 1);
}

/// A playlist track record (238) shorter than its four-byte track number in a file of an older generation.
#[test]
fn legacy_short_playlist_track_record_loads() {
    let ev = vec![
        version("20.0.0.0"),
        Event::new(CH_START, Value::U16(0)),
        Event::new(ARR_START, Value::U16(0)),
        Event::new(PL_TRACK, Value::var(vec![1, 0])),
    ];
    let p = load(&file(ev)).expect("loads");
    assert_eq!(p.arrangements.len(), 1);
}

// ---- seeded run ---------------------------------------------------------------------------------------------------

/// xorshift64*: a fixed seed gives the same streams on every run.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
    fn chance(&mut self, one_in: u64) -> bool {
        self.next().is_multiple_of(one_in)
    }
    /// A value with edge cases (0, 1, all ones) more often than chance would give them.
    fn int(&mut self) -> u32 {
        match self.below(6) {
            0 => 0,
            1 => 1,
            2 => u32::MAX,
            3 => self.below(256) as u32,
            _ => self.next() as u32,
        }
    }
    fn bytes(&mut self, max: usize) -> Vec<u8> {
        let n = self.below(max + 1);
        (0..n).map(|_| self.next() as u8).collect()
    }
}

/// A value of the kind the file format gives `id`.
fn value_for(rng: &mut Rng, id: u8) -> Value {
    let v = rng.int();
    match kind_of(id) {
        Kind::U8 => Value::U8(v as u8),
        Kind::U16 => Value::U16(v as u16),
        Kind::U32 => Value::U32(v),
        _ => Value::var(rng.bytes(96)),
    }
}

fn channel(index: u16, name: &str, mixer_track: u16) -> Channel {
    Channel {
        id: os_model::Id::NONE,
        index,
        kind: ChannelKind::Sampler,
        name: Some(name.into()),
        plugin: None,
        plugin_state: None,
        color: None,
        enabled: true,
        volume: 0.78,
        pan: 0.0,
        pitch_cents: 0,
        mixer_track,
        sample_path: Some("C:/samples/kick.wav".into()),
        layer_children: vec![],
        envelopes: Default::default(),
        rack: Default::default(),
        misc: Default::default(),
        sound: Default::default(),
        lemon_icon: None,
    }
}

/// The streams the damaged ones are made from: a created project with channels, patterns, a playlist and a mixer, and
/// the same as a file of each older generation the loader converts.
fn fixtures() -> Vec<Vec<Event>> {
    let mut arrangement = default_arrangement();
    arrangement.tracks.truncate(6);
    arrangement.clips = vec![
        Clip::new(0, 384, 0, ClipSource::Pattern(1)),
        Clip::new(384, 384, 1, ClipSource::Pattern(2)),
        Clip::new(0, 192, 2, ClipSource::Channel(1)),
    ];
    let mut mixer: Vec<MixerTrack> = (0..4).map(MixerTrack::new).collect();
    mixer[1].name = Some("Drums".into());
    mixer[1].slots[0] =
        Some(SlotPlugin { id: os_model::Id::NONE, name: "Fruity Limiter".into(), state: vec![1, 2, 3, 4] });
    mixer[2].routes = vec![0, 3];
    mixer[2].send_levels = vec![(3, 0.5)];
    let project = Project {
        ppq: 96,
        tempo: Some(128.0),
        current_pattern: Some(1),
        current_arrangement: Some(0),
        channels: vec![channel(0, "Kick", 1), channel(1, "Bass", 2)],
        patterns: vec![
            Pattern {
                number: 1,
                name: Some("Intro".into()),
                notes: vec![Note::step(0, 0), Note::step(1, 4)],
                ..Pattern::default()
            },
            Pattern { number: 2, notes: vec![Note::step(0, 8)], length: Some(384), ..Pattern::default() },
        ],
        arrangements: vec![arrangement],
        mixer,
        ..Project::default()
    };
    let current: Vec<Event> = create(&project).expect("fixture").events().cloned().collect();
    let with_version = |v: &str, keep_ends: bool| -> Vec<Event> {
        current
            .iter()
            .filter(|e| keep_ends || e.id != CH_END)
            .map(|e| if e.id == VERSION { version(v) } else { e.clone() })
            .collect()
    };
    // FL 20 (channel blocks without 51) and FL 11 (8-bit text).
    vec![current.clone(), with_version("20.8.4.2576", false), with_version("11.5.0", false)]
}

/// One change of the kind a damaged file has: an event lost, repeated, moved or changed, or its data cut short.
fn damage(rng: &mut Rng, ev: &mut Vec<Event>) {
    let n = ev.len();
    match rng.below(9) {
        0 if n > 0 => {
            ev.remove(rng.below(n));
        }
        1 if n > 0 => {
            let e = ev[rng.below(n)].clone();
            ev.insert(rng.below(n + 1), e);
        }
        2 if n > 0 => {
            let e = ev.remove(rng.below(n));
            ev.insert(rng.below(n), e);
        }
        3 if n > 1 => ev.swap(rng.below(n), rng.below(n)),
        4 if n > 0 => {
            let k = rng.below(n);
            if let Value::Var { data, .. } = &mut ev[k].value {
                let keep = rng.below(data.len() + 1);
                data.truncate(keep);
            }
        }
        5 if n > 0 => {
            let k = rng.below(n);
            if let Value::Var { data, .. } = &mut ev[k].value
                && !data.is_empty()
            {
                let at = rng.below(data.len());
                data[at] = rng.next() as u8;
            }
        }
        6 if n > 0 => {
            let k = rng.below(n);
            let id = ev[k].id;
            ev[k].value = value_for(rng, id);
        }
        7 if n > 0 => {
            // Another id of the same kind keeps the event's value.
            let k = rng.below(n);
            let kind = kind_of(ev[k].id);
            let id = rng.next() as u8;
            if kind_of(id) == kind {
                ev[k].id = id;
            }
        }
        _ => {
            let id = rng.next() as u8;
            let e = Event::new(id, value_for(rng, id));
            ev.insert(rng.below(n + 1), e);
        }
    }
}

/// A stream of random events, mostly of the ids a project file has.
fn random_stream(rng: &mut Rng, fixtures: &[Vec<Event>]) -> Vec<Event> {
    let len = rng.below(60);
    (0..len)
        .map(|_| {
            let id = if rng.chance(4) {
                rng.next() as u8
            } else {
                let f = &fixtures[rng.below(fixtures.len())];
                f[rng.below(f.len())].id
            };
            Event::new(id, value_for(rng, id))
        })
        .collect()
}

const SEED: u64 = 0x4C45_4D4F_4E5F_464C;
const RUNS: usize = 20000;

/// Damaged and random streams load or fail, never panic. Each panic is reported by where it happened, with the first
/// run that hit it.
#[test]
fn damaged_and_random_streams_never_panic() {
    let found: Arc<Mutex<Option<String>>> = Arc::default();
    let hook_found = found.clone();
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if std::thread::current().name() == Some("flp-damage-run") {
            let at = info.location().map(|l| format!("{}:{}", l.file(), l.line())).unwrap_or_default();
            *hook_found.lock().unwrap() = Some(at);
        } else {
            previous(info);
        }
    }));
    let (panics, stats) = std::thread::Builder::new()
        .name("flp-damage-run".into())
        .spawn(move || {
            let fixtures = fixtures();
            for f in &fixtures {
                load(&file(f.clone())).expect("fixture loads");
            }
            let encoded: Vec<Vec<u8>> =
                fixtures.iter().map(|f| file(f.clone()).to_bytes().expect("fixture writes")).collect();
            let mut rng = Rng(SEED);
            let mut panics: BTreeMap<String, (usize, usize)> = BTreeMap::new();
            let (mut ok, mut err) = (0, 0);
            for run in 0..RUNS {
                let rng = &mut rng;
                let case = || -> Result<(), ()> {
                    let f = match run % 10 {
                        // Damaged bytes: the file is read first, as the app opens it.
                        8 => {
                            let mut bytes = encoded[run % encoded.len()].clone();
                            for _ in 0..=rng.below(4) {
                                let at = rng.below(bytes.len());
                                match rng.below(3) {
                                    0 => bytes[at] = rng.next() as u8,
                                    1 => drop(bytes.remove(at)),
                                    _ => bytes.insert(at, rng.next() as u8),
                                }
                            }
                            FlpFile::parse(&bytes).map_err(drop)?
                        }
                        9 => file(random_stream(rng, &fixtures)),
                        _ => {
                            let mut ev = fixtures[run % fixtures.len()].clone();
                            for _ in 0..=rng.below(4) {
                                damage(rng, &mut ev);
                            }
                            file(ev)
                        }
                    };
                    load(&f).map(drop).map_err(drop)
                };
                match catch_unwind(AssertUnwindSafe(case)) {
                    Ok(Ok(())) => ok += 1,
                    Ok(Err(())) => err += 1,
                    Err(_) => {
                        let at = found.lock().unwrap().take().unwrap_or_default();
                        panics.entry(at).or_insert((run, 0)).1 += 1;
                    }
                }
            }
            (panics, (ok, err))
        })
        .unwrap()
        .join()
        .unwrap();
    let report: Vec<String> =
        panics.iter().map(|(at, (first, n))| format!("{at}: {n} runs, first run {first}")).collect();
    eprintln!("{RUNS} streams: {} loaded, {} refused, {} panicked", stats.0, stats.1, RUNS - stats.0 - stats.1);
    assert!(panics.is_empty(), "streams that panicked (seed {SEED:#x}):\n{}", report.join("\n"));
}
