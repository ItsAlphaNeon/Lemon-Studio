// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Writes the README's demo project: an original 64-bar house song in A minor, made only from this file.
//!
//! `cargo run -p os-flp --example demo_project -- <dir>` writes `<dir>/Lemon Studio Demo.flp` and the drum and audio
//! clip samples it plays (`<dir>/samples/*.wav`, synthesised here). Its patterns, notes, arrangement, names and sounds
//! are all generated below, so screenshots of it hold no one else's music.

use os_flp::project::{create, default_arrangement, new_automation};
use os_model::*;
use std::f64::consts::TAU;
use std::path::Path;

const PPQ: u16 = 96;
const BEAT: u32 = 96;
const BAR: u32 = 4 * BEAT;
/// A 16th note.
const STEP: u32 = BEAT / 4;
const TEMPO: f64 = 124.0;
const RATE: u32 = 44100;

/// The chord of each bar of the 4-bar loop (Am, F, C, G): the bass root and three upper voices.
const ROOTS: [u16; 4] = [45, 41, 48, 43];
const CHORDS: [[u16; 3]; 4] = [[57, 60, 64], [57, 60, 65], [55, 60, 64], [55, 59, 62]];

fn main() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| "demo".into());
    let dir = Path::new(&dir);
    std::fs::create_dir_all(dir.join("samples")).expect("create the output folder");
    let dir = dir.canonicalize().expect("output folder");
    let samples = dir.join("samples");
    write_samples(&samples);
    let project = project(&samples);
    let bytes = create(&project).expect("build the project").to_bytes().expect("encode the project");
    let out = dir.join("Lemon Studio Demo.flp");
    std::fs::write(&out, bytes).expect("write the project");
    println!("{}", out.display());
}

/// Channel rack rows: name, colour, mixer insert, and the sample it plays (by file name), if any.
const CHANNELS: [(&str, Rgb, u16, Option<&str>); 16] = [
    ("Kick", Rgb::new(0xE8, 0xB5, 0x2E), 1, Some("kick.wav")),
    ("Clap", Rgb::new(0xE8, 0x8A, 0x2E), 2, Some("clap.wav")),
    ("Closed Hat", Rgb::new(0xD9, 0xC8, 0x5A), 3, Some("hat.wav")),
    ("Open Hat", Rgb::new(0xC9, 0xB0, 0x4A), 3, Some("open_hat.wav")),
    ("Perc", Rgb::new(0xD8, 0x6F, 0x3C), 4, Some("perc.wav")),
    ("Sub Bass", Rgb::new(0xD9, 0x4F, 0x4F), 5, None),
    ("Chords", Rgb::new(0x9B, 0x6B, 0xD9), 6, None),
    ("Lead", Rgb::new(0x3F, 0xC1, 0xC9), 7, None),
    ("Arp", Rgb::new(0x4F, 0x8F, 0xE0), 8, None),
    ("Pad", Rgb::new(0xD9, 0x6F, 0xB0), 9, None),
    ("Piano", Rgb::new(0xC9, 0xA4, 0x6A), 10, None),
    ("Vocal Chop", Rgb::new(0x5F, 0xC2, 0x6A), 11, Some("vocal_chop.wav")),
    ("Riser", Rgb::new(0x8A, 0x9A, 0xA8), 12, Some("riser.wav")),
    ("Impact", Rgb::new(0x7A, 0x84, 0x90), 12, Some("impact.wav")),
    ("Chords Cutoff", Rgb::new(0x9B, 0x6B, 0xD9), 0, None),
    ("Pad Volume", Rgb::new(0xD9, 0x6F, 0xB0), 0, None),
];
const KICK: u16 = 0;
const CLAP: u16 = 1;
const HAT: u16 = 2;
const OPEN_HAT: u16 = 3;
const PERC: u16 = 4;
const BASS: u16 = 5;
const CHORD: u16 = 6;
const LEAD: u16 = 7;
const ARP: u16 = 8;
const PAD: u16 = 9;
const PIANO: u16 = 10;
const VOX: u16 = 11;
const RISER: u16 = 12;
const IMPACT: u16 = 13;
const CUTOFF_AUTO: u16 = 14;
const PAD_AUTO: u16 = 15;
/// The channels that play an audio clip on the playlist, and the automation clip channels.
const AUDIO: [u16; 3] = [VOX, RISER, IMPACT];
const AUTOS: [u16; 2] = [CUTOFF_AUTO, PAD_AUTO];

/// Patterns: number, name, colour (each is 4 bars long).
const PATTERNS: [(u16, &str, Rgb); 12] = [
    (1, "Kick", Rgb::new(0xE8, 0xB5, 0x2E)),
    (2, "Clap", Rgb::new(0xE8, 0x8A, 0x2E)),
    (3, "Hats", Rgb::new(0xD9, 0xC8, 0x5A)),
    (4, "Open Hat", Rgb::new(0xC9, 0xB0, 0x4A)),
    (5, "Perc", Rgb::new(0xD8, 0x6F, 0x3C)),
    (6, "Clap Roll", Rgb::new(0xE0, 0x70, 0x40)),
    (7, "Bass", Rgb::new(0xD9, 0x4F, 0x4F)),
    (8, "Chords", Rgb::new(0x9B, 0x6B, 0xD9)),
    (9, "Lead", Rgb::new(0x3F, 0xC1, 0xC9)),
    (10, "Arp", Rgb::new(0x4F, 0x8F, 0xE0)),
    (11, "Pad", Rgb::new(0xD9, 0x6F, 0xB0)),
    (12, "Piano", Rgb::new(0xC9, 0xA4, 0x6A)),
];

/// Song sections: marker name and first bar.
const SECTIONS: [(&str, u32); 7] = [("Intro", 0), ("Verse", 8), ("Build", 16), ("Drop", 20), ("Break", 36), ("Drop 2", 44), ("Outro", 60)];

/// Playlist tracks, top to bottom: name, colour, and the bars (start, length) each source plays.
fn tracks() -> Vec<(&'static str, Rgb, ClipSource, Vec<(u32, u32)>)> {
    let every4 = |from: u32, to: u32| (from..to).step_by(4).map(|b| (b, 4)).collect::<Vec<_>>();
    let join = |parts: &[Vec<(u32, u32)>]| parts.concat();
    let p = ClipSource::Pattern;
    let c = ClipSource::Channel;
    let color = |ch: u16| CHANNELS[usize::from(ch)].1;
    vec![
        ("Kick", PATTERNS[0].2, p(1), join(&[every4(4, 16), every4(20, 36), every4(44, 64)])),
        ("Clap", PATTERNS[1].2, p(2), join(&[every4(8, 16), every4(20, 36), every4(44, 60)])),
        ("Hats", PATTERNS[2].2, p(3), join(&[every4(0, 16), every4(20, 36), every4(40, 64)])),
        ("Open Hat", PATTERNS[3].2, p(4), join(&[every4(12, 16), every4(20, 36), every4(44, 60)])),
        ("Perc", PATTERNS[4].2, p(5), join(&[every4(4, 16), every4(24, 36), every4(48, 60)])),
        ("Clap Roll", PATTERNS[5].2, p(6), vec![(16, 4), (40, 4)]),
        ("Bass", PATTERNS[6].2, p(7), join(&[every4(8, 16), every4(20, 36), every4(44, 60)])),
        ("Chords", PATTERNS[7].2, p(8), join(&[every4(4, 36), every4(40, 60)])),
        ("Lead", PATTERNS[8].2, p(9), join(&[every4(20, 36), every4(44, 60)])),
        ("Arp", PATTERNS[9].2, p(10), join(&[every4(16, 20), every4(28, 36), every4(40, 60)])),
        ("Pad", PATTERNS[10].2, p(11), join(&[every4(0, 8), every4(36, 44), every4(60, 64)])),
        ("Piano", PATTERNS[11].2, p(12), every4(36, 44)),
        ("Vocal Chop", color(VOX), c(VOX), [20, 24, 28, 32, 40, 48, 52, 56].iter().map(|&b| (b, 2)).collect()),
        ("Riser", color(RISER), c(RISER), vec![(16, 4), (40, 4)]),
        ("Impact", color(IMPACT), c(IMPACT), vec![(20, 2), (44, 2)]),
        ("Chords Cutoff", color(CUTOFF_AUTO), c(CUTOFF_AUTO), vec![(4, 4), (16, 4)]),
        ("Pad Volume", color(PAD_AUTO), c(PAD_AUTO), vec![(36, 8)]),
    ]
}

fn project(samples: &Path) -> Project {
    let channels = CHANNELS
        .iter()
        .enumerate()
        .map(|(i, &(name, color, insert, sample))| {
            let i = i as u16;
            let kind = if AUTOS.contains(&i) {
                ChannelKind::Automation
            } else if AUDIO.contains(&i) {
                ChannelKind::AudioClip
            } else {
                ChannelKind::Sampler
            };
            Channel {
                index: i,
                kind,
                name: Some(name.into()),
                plugin: None,
                plugin_state: None,
                color: Some(color),
                enabled: true,
                volume: 10000.0 / 12800.0,
                pan: 0.0,
                pitch_cents: 0,
                mixer_track: insert,
                sample_path: sample.map(|s| samples.join(s).to_string_lossy().into_owned()),
                layer_children: Vec::new(),
                envelopes: Default::default(),
                rack: ChannelRack { stored_color: Some(color), ..Default::default() },
                misc: Default::default(),
                sound: Default::default(),
            }
        })
        .collect();

    let mut patterns: Vec<Pattern> = PATTERNS
        .iter()
        .map(|&(number, name, color)| Pattern { number, name: Some(name.into()), color: Some(color), length: Some(4 * BAR), ..Default::default() })
        .collect();
    let notes = pattern_notes();
    for (p, n) in patterns.iter_mut().zip(notes) {
        p.notes = n;
        p.notes.sort_by_key(|n| (n.position, n.channel, n.key));
    }

    let mut arr = default_arrangement();
    arr.markers = SECTIONS.iter().map(|&(name, bar)| TimeMarker::new(bar * BAR, name)).collect();
    let mut clips = Vec::new();
    for (row, (name, color, source, spans)) in tracks().into_iter().enumerate() {
        let t = &mut arr.tracks[row];
        t.name = Some(name.into());
        t.color = Some(color);
        for (bar, bars) in spans {
            clips.push(Clip::new(bar * BAR, bars * BAR, row as u16, source));
        }
    }
    clips.sort_by_key(|c| (c.position, c.track));
    for (id, c) in clips.iter_mut().enumerate() {
        c.set_id(id as u32 + 1);
    }
    arr.clips = clips;

    // Automation clips: the chords' filter opening over 4 bars, the pad swelling in and out over 8.
    let mut cutoff = new_automation(CUTOFF_AUTO, target::channel(CHORD, target::CHANNEL_CUTOFF), 0.15, 16.0);
    cutoff.points = vec![point(0.0, 0.12, 0.0), point(8.0, 0.35, 0.4), point(16.0, 0.95, 0.5)];
    let mut swell = new_automation(PAD_AUTO, target::channel(PAD, target::CHANNEL_VOLUME), 0.3, 32.0);
    swell.points = vec![point(0.0, 0.25, 0.0), point(12.0, 0.85, -0.4), point(20.0, 0.85, 0.0), point(32.0, 0.3, 0.4)];

    // Named, coloured inserts, each routed to the master.
    let mut mixer: Vec<MixerTrack> = (0..18u16).map(MixerTrack::new).collect();
    mixer[17].routes.clear();
    let inserts = ["Kick", "Clap", "Hats", "Perc", "Bass", "Chords", "Lead", "Arp", "Pad", "Piano", "Vocal", "FX"];
    for (i, name) in inserts.iter().enumerate() {
        let m = &mut mixer[i + 1];
        m.name = Some((*name).into());
        m.color = CHANNELS.iter().find(|c| usize::from(c.2) == i + 1).map(|c| c.1);
    }

    let mut view = PlaylistView::new(PPQ);
    view.set_ticks_per_px(f64::from(BAR) / 22.0);
    view.set_height(18);

    Project {
        saved_by: String::new(),
        ppq: PPQ,
        tempo: Some(TEMPO),
        info: ProjectInfo {
            title: Some("Lemon Studio Demo".into()),
            author: Some("Lemon Studio".into()),
            genre: Some("House".into()),
            ..Default::default()
        },
        current_pattern: Some(8),
        channels,
        patterns,
        mixer,
        arrangements: vec![arr],
        current_arrangement: Some(0),
        automations: vec![cutoff, swell],
        bar: Some((4, 4)),
        playlist_view: Some(view),
        ..Default::default()
    }
}

fn point(beat: f64, value: f64, tension: f32) -> AutoPoint {
    AutoPoint { tension, flag: if beat == 0.0 { 0 } else { 2 }, ..AutoPoint::new(beat, value) }
}

/// A note of `channel` at `pos` ticks, `len` ticks long (0 = a step), with `velocity` 0..=128.
fn note(channel: u16, pos: u32, len: u32, key: u16, velocity: u8) -> Note {
    Note { length: len, key, velocity, ..Note::step(channel, pos) }
}

/// A step on the 16th grid of bar `bar`.
fn step(channel: u16, bar: u32, s: u32, velocity: u8) -> Note {
    note(channel, bar * BAR + s * STEP, 0, 60, velocity)
}

/// The notes of each pattern, in `PATTERNS` order.
fn pattern_notes() -> Vec<Vec<Note>> {
    let bars = 0..4u32;
    // Four on the floor, with a skipped beat and a double kick to end the phrase.
    let kick: Vec<Note> = bars
        .clone()
        .flat_map(|b| (0..4).filter(move |&q| !(b == 3 && q == 3)).map(move |q| step(KICK, b, q * 4, 110)))
        .chain([step(KICK, 3, 13, 90), step(KICK, 3, 14, 110)])
        .collect();
    let clap: Vec<Note> = bars.clone().flat_map(|b| [step(CLAP, b, 4, 105), step(CLAP, b, 12, 110)]).chain([step(CLAP, 3, 15, 70)]).collect();
    // 16th hats, accented on the beat and ghosted between, with the offbeats left to the open hat.
    let hats: Vec<Note> = bars
        .clone()
        .flat_map(|b| (0..16).filter(|s| s % 4 != 2).map(move |s| step(HAT, b, s, if s % 4 == 0 { 100 } else { 64 + (s * 7 % 5) as u8 * 6 })))
        .collect();
    let open: Vec<Note> = bars.clone().flat_map(|b| (0..4).map(move |q| step(OPEN_HAT, b, q * 4 + 2, 95))).collect();
    let perc_steps: [&[u32]; 4] = [&[3, 7, 10], &[3, 6, 11, 14], &[3, 7, 10], &[2, 5, 9, 11, 14]];
    let perc: Vec<Note> = bars.clone().flat_map(|b| perc_steps[b as usize].iter().map(move |&s| step(PERC, b, s, 80 + (s % 3) as u8 * 15))).collect();
    // The build: quarters, 8ths, 16ths, then 32nds, getting louder.
    let roll: Vec<Note> = bars
        .clone()
        .flat_map(|b| {
            let every = [BEAT, BEAT / 2, STEP, STEP / 2][b as usize];
            (0..BAR / every).map(move |k| {
                let pos = b * BAR + k * every;
                note(CLAP, pos, 0, 60, (50 + pos * 78 / (4 * BAR)) as u8)
            })
        })
        .collect();
    // Offbeat 8ths on the root, with an octave pickup before each bar.
    let bass: Vec<Note> = bars
        .clone()
        .flat_map(|b| {
            let r = ROOTS[b as usize];
            (0..4)
                .map(move |q| note(BASS, b * BAR + q * BEAT + BEAT / 2, 40, r, if q == 0 { 110 } else { 96 }))
                .chain([note(BASS, b * BAR + 14 * STEP, 20, r + 12, 84)])
        })
        .collect();
    // Tresillo stabs (3 + 3 + 2 16ths), the last bar answering with an extra hit.
    let chords: Vec<Note> = bars
        .clone()
        .flat_map(|b| {
            let hits: &[(u32, u32, u8)] =
                if b == 3 { &[(0, 132, 116), (144, 84, 92), (240, 40, 80), (288, 84, 100)] } else { &[(0, 132, 116), (144, 132, 94), (288, 84, 102)] };
            hits.iter().flat_map(move |&(pos, len, vel)| {
                CHORDS[b as usize].iter().enumerate().map(move |(k, &key)| note(CHORD, b * BAR + pos, len, key, vel - 6 * k as u8))
            })
        })
        .collect();
    // The hook: (16th position, 16ths long, key, velocity).
    let hook: [(u32, u32, u16, u8); 26] = [
        (0, 3, 76, 118),
        (3, 1, 74, 90),
        (4, 2, 72, 100),
        (6, 2, 74, 96),
        (8, 4, 76, 112),
        (12, 2, 79, 104),
        (14, 2, 76, 92),
        (16, 3, 77, 116),
        (19, 1, 76, 88),
        (20, 4, 72, 104),
        (24, 2, 69, 94),
        (26, 2, 72, 98),
        (28, 4, 74, 108),
        (32, 3, 76, 118),
        (35, 1, 74, 90),
        (36, 2, 72, 100),
        (38, 2, 74, 96),
        (40, 4, 79, 114),
        (44, 2, 81, 120),
        (46, 2, 79, 100),
        (48, 4, 74, 112),
        (52, 2, 71, 96),
        (54, 2, 74, 100),
        (56, 4, 71, 106),
        (60, 2, 72, 88),
        (62, 2, 69, 84),
    ];
    let lead: Vec<Note> = hook.iter().map(|&(s, l, key, vel)| note(LEAD, s * STEP, l * STEP - 4, key, vel)).collect();
    // 16ths up and down the chord over two octaves.
    let arp: Vec<Note> = bars
        .clone()
        .flat_map(|b| {
            let [a, c, e] = CHORDS[b as usize];
            let tones = [a + 12, c + 12, e + 12, a + 24];
            let order = [0, 1, 2, 3, 2, 1, 2, 3];
            (0..16).map(move |s| note(ARP, b * BAR + s * STEP, STEP - 4, tones[order[s as usize % 8]], if s % 4 == 0 { 108 } else { 78 }))
        })
        .collect();
    let pad: Vec<Note> = bars
        .clone()
        .flat_map(|b| {
            let r = ROOTS[b as usize] + 12;
            std::iter::once(r).chain(CHORDS[b as usize]).map(move |key| note(PAD, b * BAR, BAR - 8, key, 76))
        })
        .collect();
    // The break's piano: the hook an octave down over half-note triads in the left hand (A minor, then major).
    let left = (0..4u32).flat_map(|b| {
        let r = ROOTS[b as usize];
        let third = if b == 0 { 3 } else { 4 };
        [0, 2].into_iter().flat_map(move |half| {
            [r, r + third, r + 7].into_iter().map(move |key| note(PIANO, b * BAR + half * BEAT, 2 * BEAT - 8, key, if half == 0 { 92 } else { 76 }))
        })
    });
    let right = hook.iter().map(|&(s, l, key, vel)| note(PIANO, s * STEP, l * STEP - 4, key - 12, vel - 10));
    let piano: Vec<Note> = left.chain(right).collect();
    vec![kick, clap, hats, open, perc, roll, bass, chords, lead, arp, pad, piano]
}

/// Synthesises the drum one-shots and the audio clips' sounds into `dir`.
fn write_samples(dir: &Path) {
    let bar_s = 4.0 * 60.0 / TEMPO;
    let mut noise = Noise(0x2545_F491);
    // A sine falling from 150 to 45 Hz (the phase is the integral of 45 + 105 e^(-30 t)).
    let kick = render(0.45, |t, _| (TAU * (45.0 * t + 105.0 / 30.0 * (1.0 - (-t * 30.0).exp()))).sin() * (-t * 7.0).exp());
    let clap = render(0.3, |t, _| {
        let burst = [0.0, 0.011, 0.022].iter().map(|&o| if t >= o { (-(t - o) * 140.0).exp() } else { 0.0 }).sum::<f64>();
        noise.high() * (burst * 0.6 + (-t * 22.0).exp() * 0.4)
    });
    let hat = render(0.08, |t, _| noise.high() * (-t * 60.0).exp());
    let open_hat = render(0.4, |t, _| noise.high() * (-t * 9.0).exp());
    let perc = render(0.15, |t, _| (TAU * 620.0 * t).sin() * (-t * 28.0).exp() + noise.white() * 0.2 * (-t * 60.0).exp());
    // An "ah" sung on A, C and E, chopped on the 16th grid.
    let gates = [1u8, 0, 1, 1, 0, 1, 0, 1, 1, 0, 1, 0, 1, 1, 1, 0];
    let pitches = [220.0, 261.63, 329.63, 261.63];
    let vox = render(2.0 * bar_s, |t, _| {
        let s16 = bar_s / 16.0;
        let k = (t / s16) as usize;
        if gates[k % 16] == 0 {
            return 0.0;
        }
        let local = t - k as f64 * s16;
        let env = (local / 0.005).min(1.0) * (-local * 9.0).exp();
        let f0 = pitches[(k / 8) % 4] * (1.0 + 0.006 * (TAU * 5.5 * t).sin());
        let formant = |f: f64| [(800.0, 1.0), (1150.0, 0.5), (2900.0, 0.25)].iter().map(|&(c, a)| a * (-((f - c) / 180.0).powi(2)).exp()).sum::<f64>();
        let voice = (1..=24).map(|h| formant(f0 * f64::from(h)) * (TAU * f0 * f64::from(h) * t).sin()).sum::<f64>();
        voice * env * 0.6
    });
    // Noise opening up and a tone sweeping up over 4 bars.
    let mut low = 0.0;
    let riser = render(4.0 * bar_s, |t, len| {
        let x = t / len;
        let a = 0.01 + 0.5 * x * x;
        low += a * (noise.white() - low);
        let sweep = (TAU * (200.0 * t + 900.0 * t * x)).sin() * 0.25;
        (low * 1.4 + sweep) * x.powf(1.5)
    });
    let mut boom = 0.0;
    let impact = render(2.0 * bar_s, |t, _| {
        boom += 0.05 * (noise.white() - boom);
        (TAU * 52.0 * t).sin() * (-t * 1.3).exp() * 0.9 + boom * 2.0 * (-t * 4.0).exp()
    });
    for (name, data) in [
        ("kick.wav", kick),
        ("clap.wav", clap),
        ("hat.wav", hat),
        ("open_hat.wav", open_hat),
        ("perc.wav", perc),
        ("vocal_chop.wav", vox),
        ("riser.wav", riser),
        ("impact.wav", impact),
    ] {
        write_wav(&dir.join(name), &data);
    }
}

/// `seconds` of mono audio from `f(time, length)`, normalised to a peak of 0.9.
fn render(seconds: f64, mut f: impl FnMut(f64, f64) -> f64) -> Vec<f64> {
    let n = (seconds * f64::from(RATE)) as usize;
    let mut v: Vec<f64> = (0..n).map(|i| f(i as f64 / f64::from(RATE), seconds)).collect();
    let peak = v.iter().fold(0.0f64, |m, x| m.max(x.abs())).max(1e-9);
    v.iter_mut().for_each(|x| *x *= 0.9 / peak);
    v
}

/// A 16-bit mono WAV file.
fn write_wav(path: &Path, data: &[f64]) {
    let bytes = (data.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + bytes as usize);
    out.extend(b"RIFF");
    out.extend((36 + bytes).to_le_bytes());
    out.extend(b"WAVEfmt ");
    out.extend(16u32.to_le_bytes());
    out.extend(1u16.to_le_bytes());
    out.extend(1u16.to_le_bytes());
    out.extend(RATE.to_le_bytes());
    out.extend((RATE * 2).to_le_bytes());
    out.extend(2u16.to_le_bytes());
    out.extend(16u16.to_le_bytes());
    out.extend(b"data");
    out.extend(bytes.to_le_bytes());
    for x in data {
        out.extend(((x.clamp(-1.0, 1.0) * 32767.0).round() as i16).to_le_bytes());
    }
    std::fs::write(path, out).expect("write a sample");
}

/// A repeatable white noise (xorshift), and a brighter version of it.
struct Noise(u32);

impl Noise {
    fn white(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        f64::from(self.0) / f64::from(u32::MAX) * 2.0 - 1.0
    }

    fn high(&mut self) -> f64 {
        let a = self.white();
        let b = self.white();
        (a - b) * 0.5
    }
}
