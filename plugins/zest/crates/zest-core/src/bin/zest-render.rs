// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! zest-render: render MIDI through the Zest engine the way `tools/diva.sh render` renders Diva, and compare WAVs.
//!
//! ```text
//! zest-render render <in.mid> <out.wav> [--set FILE]... [--param MODULE/Name=value]... [--tail S] [--pre S]
//!                    [--block N] [--rate R] [--offline]
//! zest-render cmp <a.wav> <b.wav> [--from N] [--to N]
//! zest-render bench [--voices N] [--seconds S] [--h2p PRESET] [--set FILE] [--param K=V]
//! ```
//! Settings files are Diva's: `MODULE/Name = value` per line, `#` comments; text values go through the labels.
//! Parameters Zest does not have are reported and ignored. The render has a pre-roll (default 2 s, as the Diva
//! host) and the output is shifted by the engine latency, so sample 0 is MIDI time 0, like the Diva renders.

use std::fs;
use zest_core::params::{self, COUNT, PARAMS};
use zest_core::{Engine, Event, Transport};

fn usage() -> ! {
    eprintln!("usage: zest-render render IN.mid OUT.wav [--h2p PRESET] [--preset F.zest] [--set F] [--param K=V] [--tail S] [--pre S] [--block N] [--offline]");
    eprintln!("       zest-render cmp A.wav B.wav [--from N] [--to N]");
    eprintln!("       zest-render bench [--voices N] [--seconds S] [--h2p PRESET] [--set FILE] [--param K=V]");
    std::process::exit(2)
}

fn set_param(values: &mut [f32; COUNT], key: &str, val: &str) -> bool {
    let key = key.trim();
    let val = val.trim();
    let alt = key.replace('_', " ");
    let i = match params::index(key)
        .or_else(|| PARAMS.iter().position(|p| p.key.eq_ignore_ascii_case(key)))
        .or_else(|| PARAMS.iter().position(|p| p.key.eq_ignore_ascii_case(&alt)))
    {
        Some(i) => i,
        None => {
            eprintln!("zest-render: no parameter {key} (ignored)");
            return false;
        }
    };
    let p = &PARAMS[i];
    let v = match val.parse::<f32>() {
        Ok(x) => x,
        Err(_) => match p.labels.iter().position(|l| l.eq_ignore_ascii_case(val)) {
            Some(k) => p.min + k as f32,
            None => {
                eprintln!("zest-render: {key}: unknown value {val} (ignored)");
                return false;
            }
        },
    };
    values[i] = v;
    true
}

fn read_settings(values: &mut [f32; COUNT], path: &str) {
    let s = fs::read_to_string(path).unwrap_or_else(|e| panic!("{path}: {e}"));
    for line in s.lines() {
        let line = line.split(" # ").next().unwrap().trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            set_param(values, k, v);
        }
    }
}

struct Mev {
    secs: f64,
    status: u8,
    d: [u8; 2],
}

fn vlq(b: &[u8], p: &mut usize) -> u32 {
    let mut v = 0u32;
    for _ in 0..4 {
        let c = b[*p];
        *p += 1;
        v = (v << 7) | (c & 0x7f) as u32;
        if c & 0x80 == 0 {
            break;
        }
    }
    v
}

fn read_midi(path: &str) -> Vec<Mev> {
    let b = fs::read(path).unwrap_or_else(|e| panic!("{path}: {e}"));
    assert!(&b[0..4] == b"MThd");
    let ntrk = u16::from_be_bytes([b[10], b[11]]) as usize;
    let ppq = u16::from_be_bytes([b[12], b[13]]) as f64;
    let mut p = 8 + u32::from_be_bytes([b[4], b[5], b[6], b[7]]) as usize;
    let mut raw: Vec<(u64, usize, u8, [u8; 2])> = Vec::new();
    let mut tempo: Vec<(u64, u32)> = vec![(0, 500_000)];
    for _ in 0..ntrk {
        assert!(&b[p..p + 4] == b"MTrk");
        let len = u32::from_be_bytes([b[p + 4], b[p + 5], b[p + 6], b[p + 7]]) as usize;
        let end = p + 8 + len;
        let mut q = p + 8;
        let mut tick = 0u64;
        let mut running = 0u8;
        while q < end {
            tick += vlq(&b, &mut q) as u64;
            let mut st = b[q];
            if st & 0x80 != 0 {
                q += 1;
            } else {
                st = running;
            }
            match st {
                0xff => {
                    let ty = b[q];
                    q += 1;
                    let l = vlq(&b, &mut q) as usize;
                    if ty == 0x51 && l == 3 {
                        tempo.push((tick, (b[q] as u32) << 16 | (b[q + 1] as u32) << 8 | b[q + 2] as u32));
                    }
                    q += l;
                }
                0xf0 | 0xf7 => {
                    let l = vlq(&b, &mut q) as usize;
                    q += l;
                }
                _ => {
                    running = st;
                    let n = if matches!(st & 0xf0, 0xc0 | 0xd0) { 1 } else { 2 };
                    raw.push((tick, raw.len(), st, [b[q], if n == 2 { b[q + 1] } else { 0 }]));
                    q += n;
                }
            }
        }
        p = end;
    }
    tempo.sort();
    raw.sort();
    raw.iter()
        .map(|&(tick, _, st, d)| {
            let mut secs = 0.0;
            let mut last_t = 0u64;
            let mut us = 500_000u32;
            for &(tt, u) in &tempo {
                if tt > tick {
                    break;
                }
                secs += (tt - last_t) as f64 / ppq * us as f64 / 1e6;
                last_t = tt;
                us = u;
            }
            secs += (tick - last_t) as f64 / ppq * us as f64 / 1e6;
            Mev { secs, status: st, d }
        })
        .collect()
}

fn write_wav(path: &str, l: &[f32], r: &[f32], sr: u32) {
    let n = l.len();
    let mut o = Vec::with_capacity(44 + n * 8);
    o.extend_from_slice(b"RIFF");
    o.extend_from_slice(&((36 + n * 8) as u32).to_le_bytes());
    o.extend_from_slice(b"WAVEfmt ");
    o.extend_from_slice(&16u32.to_le_bytes());
    o.extend_from_slice(&3u16.to_le_bytes());
    o.extend_from_slice(&2u16.to_le_bytes());
    o.extend_from_slice(&sr.to_le_bytes());
    o.extend_from_slice(&(sr * 8).to_le_bytes());
    o.extend_from_slice(&8u16.to_le_bytes());
    o.extend_from_slice(&32u16.to_le_bytes());
    o.extend_from_slice(b"data");
    o.extend_from_slice(&((n * 8) as u32).to_le_bytes());
    for i in 0..n {
        o.extend_from_slice(&l[i].to_le_bytes());
        o.extend_from_slice(&r[i].to_le_bytes());
    }
    fs::write(path, o).unwrap();
}

/// 32-bit float or 16/24-bit PCM stereo/mono WAV -> (L, R)
pub fn read_wav(path: &str) -> (Vec<f32>, Vec<f32>) {
    let b = fs::read(path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let mut p = 12;
    let (mut fmt, mut ch, mut bits) = (0u16, 0u16, 0u16);
    while p + 8 <= b.len() {
        let id = &b[p..p + 4];
        let sz = u32::from_le_bytes([b[p + 4], b[p + 5], b[p + 6], b[p + 7]]) as usize;
        if id == b"fmt " {
            fmt = u16::from_le_bytes([b[p + 8], b[p + 9]]);
            ch = u16::from_le_bytes([b[p + 10], b[p + 11]]);
            bits = u16::from_le_bytes([b[p + 22], b[p + 23]]);
            if fmt == 0xfffe {
                fmt = u16::from_le_bytes([b[p + 32], b[p + 33]]);
            }
        } else if id == b"data" {
            let d = &b[p + 8..(p + 8 + sz).min(b.len())];
            let bps = (bits / 8) as usize;
            let fr = bps * ch as usize;
            let n = d.len() / fr;
            let mut l = Vec::with_capacity(n);
            let mut r = Vec::with_capacity(n);
            for i in 0..n {
                let s = |c: usize| -> f32 {
                    let o = i * fr + c * bps;
                    match (fmt, bits) {
                        (3, 32) => f32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]]),
                        (1, 16) => i16::from_le_bytes([d[o], d[o + 1]]) as f32 / 32768.0,
                        (1, 24) => ((i32::from_le_bytes([0, d[o], d[o + 1], d[o + 2]])) >> 8) as f32 / 8388608.0,
                        _ => panic!("unsupported wav format {fmt}/{bits}"),
                    }
                };
                l.push(s(0));
                r.push(if ch > 1 { s(1) } else { s(0) });
            }
            return (l, r);
        }
        p += 8 + sz + (sz & 1);
    }
    panic!("{path}: no data chunk")
}

fn render(args: &[String]) {
    if args.len() < 2 {
        usage();
    }
    let (inp, out) = (&args[0], &args[1]);
    let mut values = [0.0f32; COUNT];
    for (v, p) in values.iter_mut().zip(PARAMS.iter()) {
        *v = p.default;
    }
    let (mut tail, mut pre, mut block, mut offline) = (4.0f64, 2.0f64, 512usize, false);
    let mut sr = 48000.0f32;
    let mut tempo = 120.0f64;
    let mut host_transport = false;
    let mut presets: Vec<String> = Vec::new();
    let mut zest_presets: Vec<zest_core::slop::PresetSlop> = Vec::new();
    let mut i = 2;
    while i < args.len() {
        match args[i].as_str() {
            "--set" => {
                read_settings(&mut values, &args[i + 1]);
                i += 1;
            }
            "--h2p" => {
                let text = String::from_utf8_lossy(&std::fs::read(&args[i + 1]).expect("read .h2p")).into_owned();
                values = zest_core::h2p::load(&text, &values).expect("load .h2p");
                presets.push(text);
                i += 1;
            }
            "--preset" => {
                let f = zest_preset::parse(&zest_preset::read(std::path::Path::new(&args[i + 1])).expect("read .zest")).expect("parse .zest");
                values = zest_core::h2p::zest_values(&f, &values);
                zest_presets.push(f.spread.into());
                i += 1;
            }
            "--param" => {
                let (k, v) = args[i + 1].split_once('=').expect("--param K=V");
                set_param(&mut values, k, v);
                i += 1;
            }
            "--tail" => {
                tail = args[i + 1].parse().unwrap();
                i += 1;
            }
            "--pre" => {
                pre = args[i + 1].parse().unwrap();
                i += 1;
            }
            "--rate" => {
                sr = args[i + 1].parse().unwrap();
                i += 1;
            }
            "--tempo" => {
                tempo = args[i + 1].parse().unwrap();
                i += 1;
            }
            "--block" => {
                block = args[i + 1].parse().unwrap();
                i += 1;
            }
            "--offline" => offline = true,
            // render as Diva does with zest-divahost --host-transport (transport pointer in clap_process, no start)
            "--host-transport" => host_transport = true,
            a => panic!("unknown option {a}"),
        }
        i += 1;
    }
    let mev = read_midi(inp);
    let pre_n = (pre * sr as f64).round() as u64;
    let last = mev.iter().map(|e| (e.secs * sr as f64).round() as u64).max().unwrap_or(0);
    let mut eng = Engine::new(sr, block);
    eng.set_offline(offline);
    for t in &presets {
        eng.load_preset_data(t);
    }
    for p in &zest_presets {
        eng.apply_preset_slop(p);
    }
    eng.set_params(&values);
    let lat = eng.latency() as u64;
    let total = pre_n + last + lat + (tail * sr as f64).round() as u64;
    let mut l = vec![0.0f32; total as usize];
    let mut r = vec![0.0f32; total as usize];
    let evs: Vec<(u64, Event)> = mev
        .iter()
        .filter_map(|e| {
            let t = pre_n + (e.secs * sr as f64).round() as u64;
            let ch = e.status & 0xf0;
            let ev = match ch {
                0x90 if e.d[1] > 0 => Event::NoteOn { time: 0, key: e.d[0], velocity: e.d[1] as f32 / 127.0 },
                0x80 | 0x90 => Event::NoteOff { time: 0, key: e.d[0] },
                0xb0 => Event::Controller { time: 0, cc: e.d[0], value: e.d[1] as f32 / 127.0 },
                0xd0 => Event::Pressure { time: 0, value: e.d[0] as f32 / 127.0 },
                0xe0 => {
                    let v = ((e.d[1] as i32) << 7 | e.d[0] as i32) - 8192;
                    Event::PitchBend { time: 0, value: v as f32 / 8192.0 }
                }
                _ => return None,
            };
            Some((t, ev))
        })
        .collect();
    if !host_transport {
        // the Diva references were rendered without the transport pointer in clap_process: Diva's first block reads
        // the transport as stopped, then the transport event says playing (a transport start); a real host passes
        // the pointer from the first block (--host-transport, no start)
        let tr = Transport { tempo, playing: false, beats: 0.0 };
        eng.process(&[], &tr, &mut [], &mut []);
    }
    let t0 = std::time::Instant::now();
    let mut pos = 0u64;
    let mut ei = 0;
    let mut buf: Vec<Event> = Vec::with_capacity(256);
    while pos < total {
        let n = (block as u64).min(total - pos);
        buf.clear();
        while ei < evs.len() && evs[ei].0 < pos + n {
            let off = (evs[ei].0 - pos) as u32;
            let mut e = evs[ei].1;
            match &mut e {
                Event::NoteOn { time, .. }
                | Event::NoteOff { time, .. }
                | Event::PitchBend { time, .. }
                | Event::Controller { time, .. }
                | Event::Pressure { time, .. }
                | Event::Panic { time }
                | Event::Tuning { time, .. } => *time = off,
            }
            buf.push(e);
            ei += 1;
        }
        // the song position as a CLAP host passes it: fixed point, 2^31 per beat
        let beats = (pos as f64 / sr as f64 * tempo / 60.0 * 2147483648.0).round() / 2147483648.0;
        let tr = Transport { tempo, playing: true, beats };
        let (a, b) = (pos as usize, (pos + n) as usize);
        eng.process(&buf, &tr, &mut l[a..b], &mut r[a..b]);
        pos += n;
    }
    let el = t0.elapsed().as_secs_f64();
    let s0 = (pre_n + lat) as usize;
    write_wav(out, &l[s0..], &r[s0..], sr as u32);
    eprintln!("zest-render: {} samples in {:.2} s ({:.1}x real time)", total, el, total as f64 / sr as f64 / el);
}

fn cmp(args: &[String]) {
    if args.len() < 2 {
        usage();
    }
    let (al, ar) = read_wav(&args[0]);
    let (bl, br) = read_wav(&args[1]);
    let mut from = 0usize;
    let mut to = usize::MAX;
    let mut i = 2;
    while i < args.len() {
        match args[i].as_str() {
            "--from" => from = args[i + 1].parse().unwrap(),
            "--to" => to = args[i + 1].parse().unwrap(),
            a => panic!("unknown option {a}"),
        }
        i += 2;
    }
    let n = al.len().min(bl.len()).min(to);
    let mut diff = 0usize;
    let mut first = None;
    let mut maxd = 0.0f64;
    let (mut e2, mut s2) = (0.0f64, 0.0f64);
    for k in from..n {
        for (x, y) in [(al[k], bl[k]), (ar[k], br[k])] {
            let d = (x as f64 - y as f64).abs();
            if x.to_bits() != y.to_bits() {
                diff += 1;
                if first.is_none() {
                    first = Some(k);
                }
            }
            maxd = maxd.max(d);
            e2 += d * d;
            s2 += (x as f64) * (x as f64);
        }
    }
    let db = 10.0 * ((e2 + 1e-300) / (s2 + 1e-300)).log10();
    match first {
        None => println!("BIT-EXACT {} samples ({}..{})", n - from, from, n),
        Some(f) => println!(
            "{} differing values of {} (first at sample {}), max|d| {:.3e}, residual {:.1} dB",
            diff,
            2 * (n - from),
            f,
            maxd,
            db
        ),
    }
}

fn bench(args: &[String]) {
    let mut voices = 16usize;
    let mut secs = 10.0f64;
    let mut values = [0.0f32; COUNT];
    for (v, p) in values.iter_mut().zip(PARAMS.iter()) {
        *v = p.default;
    }
    values[params::id::VCC_VOICES] = 7.0; // 16
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--voices" => voices = args[i + 1].parse().unwrap(),
            "--seconds" => secs = args[i + 1].parse().unwrap(),
            "--set" => read_settings(&mut values, &args[i + 1]),
            "--h2p" => {
                let text = String::from_utf8_lossy(&std::fs::read(&args[i + 1]).expect("read .h2p")).into_owned();
                values = zest_core::h2p::load(&text, &values).expect("load .h2p");
            }
            "--param" => {
                let (k, v) = args[i + 1].split_once('=').expect("--param K=V");
                set_param(&mut values, k, v);
            }
            a => panic!("unknown option {a}"),
        }
        i += 2;
    }
    let sr = 48000.0f32;
    let mut eng = Engine::new(sr, 512);
    eng.set_params(&values);
    let mut l = vec![0.0f32; 512];
    let mut r = vec![0.0f32; 512];
    let evs: Vec<Event> =
        (0..voices).map(|k| Event::NoteOn { time: (k * 3) as u32, key: 36 + (k as u8 * 5) % 48, velocity: 0.8 }).collect();
    let tr = Transport::default();
    eng.process(&evs, &tr, &mut l, &mut r);
    let blocks = (secs * sr as f64 / 512.0) as usize;
    let t0 = std::time::Instant::now();
    let mut peak = 0.0f32;
    for _ in 0..blocks {
        eng.process(&[], &tr, &mut l, &mut r);
        peak = peak.max(l.iter().fold(0.0f32, |a, &x| a.max(x.abs())));
    }
    let el = t0.elapsed().as_secs_f64();
    let audio = blocks as f64 * 512.0 / sr as f64;
    println!(
        "{voices} voices, {:.1} s audio in {:.3} s: {:.1}x real time, {:.2} % of a core ({:.3} % per voice), peak {:.3}",
        audio,
        el,
        audio / el,
        100.0 * el / audio,
        100.0 * el / audio / voices as f64,
        peak
    );
}

/// `zest-render convert IN.h2p OUT.zest [NAME]`: a Diva preset as a Zest preset file.
fn convert(a: &[String]) {
    let (Some(inp), Some(out)) = (a.first(), a.get(1)) else { usage() };
    let text = String::from_utf8_lossy(&std::fs::read(inp).expect("read .h2p")).into_owned();
    let stem = std::path::Path::new(inp).file_stem().unwrap().to_string_lossy().into_owned();
    let name = a.get(2).cloned().unwrap_or(stem);
    let f = zest_core::h2p::to_zest(&text, &name).expect("convert");
    std::fs::write(out, zest_preset::write(&f)).expect("write .zest");
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    match a.first().map(|s| s.as_str()) {
        Some("render") => render(&a[1..]),
        Some("cmp") => cmp(&a[1..]),
        Some("bench") => bench(&a[1..]),
        Some("convert") => convert(&a[1..]),
        _ => usage(),
    }
}
