// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Renders notes through the player's voices to a 32-bit float WAV, for comparing the modes with recordings.
//!
//! `cargo run --release -p lemon-soundfont --example sf_render -- FONT.sf2 BANK PROG classic|modern OUT.wav SECONDS
//! KEY:START:LEN:VEL[,...] [control=value ...]` — START and LEN in seconds, VEL 0..=128 (FL velocity). Controls are
//! the player's own (attack, decay, sustain, release, lfo_delay, lfo_amount, lfo_speed, cutoff, mod).

use std::sync::Arc;

use lemon_soundfont::dsp::{Engine, Overrides, Patch};
use lemon_soundfont::profile::Mode;
use lemon_soundfont::sf2::Font;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a.len() < 8 {
        eprintln!("usage: sf_render FONT BANK PROG classic|modern OUT.wav SECONDS KEY:START:LEN:VEL[,...] [control=value ...]");
        std::process::exit(2);
    }
    let font = Arc::new(Font::open(std::path::Path::new(&a[1])).expect("font"));
    let (bank, prog): (u16, u16) = (a[2].parse().unwrap(), a[3].parse().unwrap());
    let mode = if a[4] == "modern" { Mode::Modern } else { Mode::Classic };
    let secs: f64 = a[6].parse().unwrap();
    let mut ov = Overrides::default();
    for kv in &a[8..] {
        let (k, v) = kv.split_once('=').expect("control=value");
        let v: i32 = v.parse().unwrap();
        match k {
            "attack" => ov.attack = Some(v),
            "decay" => ov.decay = Some(v),
            "sustain" => ov.sustain = Some(v),
            "release" => ov.release = Some(v),
            "lfo_delay" => ov.lfo_delay = Some(v),
            "lfo_amount" => ov.lfo_amount = Some(v),
            "lfo_speed" => ov.lfo_speed = Some(v),
            "cutoff" => ov.cutoff = Some(v),
            "mod" => ov.mod_wheel = v,
            _ => panic!("unknown control {k}"),
        }
    }
    let rate = 44100.0;
    let preset = font.find(bank, prog).or(Some(0));
    let mut e = Engine::new(rate as f32);
    e.mode = mode;
    e.set_patch(Some(Arc::new(Patch { font, preset, overrides: ov })));
    // (frame, on, key, vel)
    let mut evs: Vec<(usize, bool, u8, f32)> = Vec::new();
    for n in a[7].split(',') {
        let p: Vec<f64> = n.split(':').map(|x| x.parse().unwrap()).collect();
        let (k, s, l, v) = (p[0] as u8, p[1], p[2], p[3]);
        evs.push(((s * rate).round() as usize, true, k, (v / 128.0) as f32));
        evs.push((((s + l) * rate).round() as usize, false, k, 0.0));
    }
    evs.sort_by_key(|e| (e.0, e.1));
    let total = (secs * rate) as usize;
    let (mut l, mut r) = (vec![0f32; total], vec![0f32; total]);
    let mut at = 0;
    for (f, on, k, v) in evs.into_iter().chain(std::iter::once((total, false, 0, 0.0))) {
        let f = f.min(total);
        if f > at {
            e.render(&mut l[at..f], &mut r[at..f]);
            at = f;
        }
        if f < total {
            if on { e.note_on(k, v, None) } else { e.note_off(k, None) }
        }
    }
    let mut data = Vec::with_capacity(total * 8);
    for i in 0..total {
        data.extend(l[i].to_le_bytes());
        data.extend(r[i].to_le_bytes());
    }
    let mut fmt = Vec::new();
    fmt.extend(3u16.to_le_bytes());
    fmt.extend(2u16.to_le_bytes());
    fmt.extend(44100u32.to_le_bytes());
    fmt.extend((44100u32 * 8).to_le_bytes());
    fmt.extend(8u16.to_le_bytes());
    fmt.extend(32u16.to_le_bytes());
    let mut body = b"WAVE".to_vec();
    for (id, b) in [(b"fmt ", &fmt), (b"data", &data)] {
        body.extend(id);
        body.extend((b.len() as u32).to_le_bytes());
        body.extend(b);
    }
    let mut file = b"RIFF".to_vec();
    file.extend((body.len() as u32).to_le_bytes());
    file.extend(body);
    std::fs::write(&a[5], file).expect("write");
}
