// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! os-native-probe: load one FL native plugin from the user's FL install, print its info and
//! parameters, save its default state, and render a test note (generators) to a WAV file.
//!
//! Usage: os-native-probe <plugin name> [out.wav] [--fl DIR] [--sandbox71] [--editor out.bmp]
//!
//! `--sandbox71` installs the experimental sandbox-object answer to the undocumented host
//! dispatcher ID 71 (see `probe71`) and prints which of its vtable slots the plugin touched.

use std::path::PathBuf;

use os_native::{find_plugin, host, Plugin};

#[cfg(windows)]
mod editor;
#[cfg(windows)]
mod probe71;

fn main() {
    let mut args = std::env::args().skip(1);
    let mut name = None;
    let mut out = None;
    let mut fl = PathBuf::from(r"C:\Program Files\Image-Line\FL Studio 2026");
    let mut sandbox71 = false;
    let mut editor_out: Option<PathBuf> = None;
    while let Some(a) = args.next() {
        if a == "--fl" {
            fl = PathBuf::from(args.next().expect("--fl needs a value"));
        } else if a == "--editor" {
            editor_out = Some(PathBuf::from(args.next().expect("--editor needs an output .bmp")));
        } else if a == "--sandbox71" {
            sandbox71 = true;
        } else if name.is_none() {
            name = Some(a);
        } else {
            out = Some(PathBuf::from(a));
        }
    }

    #[cfg(not(windows))]
    if sandbox71 || editor_out.is_some() {
        eprintln!("--sandbox71 and --editor need Windows; ignored");
    }
    #[cfg(windows)]
    if sandbox71 {
        let base = probe71::install();
        eprintln!("[sandbox71] object at {base:#x}; answering host dispatcher ID 71 with it");
        host::set_dispatch_hook(Some(probe71::hook));
    }
    let name = name.expect("usage: os-native-probe <plugin name> [out.wav] [--fl DIR]");
    let dll = find_plugin(&fl, &name).unwrap_or_else(|| PathBuf::from(&name));
    eprintln!("loading {}", dll.display());

    let rate = 44100;
    let block = 512;
    let mut p = match Plugin::load(&dll, &fl, rate, block) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("load failed: {e}");
            report_log();
            std::process::exit(1);
        }
    };
    let i = p.info.clone();
    println!("name: {} ({}), SDK {}, flags {:#x} (generator {}, voice render {}), params {}, poly {}",
        i.long_name, i.short_name, i.sdk_version, i.flags, i.is_generator(), i.uses_voice_render(), i.num_params, i.def_poly);
    for k in 0..i.num_params.min(12) {
        let n = p.param_name(k);
        let v = p.param_value(k);
        println!("  param {k:3}: {n:<32} = {v}");
    }
    if i.num_params > 12 {
        println!("  … {} more", i.num_params - 12);
    }
    println!("methods sharing one function (unimplemented): {:?}", p.shared_methods());
    let state = p.save_state().unwrap_or_default();
    println!("default state: {} bytes, head {:02x?}", state.len(), &state[..state.len().min(24)]);
    report_log();

    #[cfg(windows)]
    if let Some(out) = &editor_out {
        editor::run(&mut p, out);
        report_log();
        drop(p);
        println!("unloaded cleanly");
        editor::after_drop();
        if sandbox71 {
            report_sandbox();
        }
        return;
    }

    if i.is_generator() {
        generator_report(p, rate, out.as_deref());
        report_log();
        println!("unloaded cleanly");
        #[cfg(windows)]
        if sandbox71 {
            report_sandbox();
        }
        return;
    } else {
        // Effect: push a 1 kHz sine through it and report what changed. Effects do not set
        // WantNewTick and must not be sent NewTick (it crashes them).
        p.set_tempo(130.0, 96);
        p.set_enabled(true);
        p.set_playing(true);
        let total = rate as usize; // 1 s
        let freq = 1000.0f64;
        let mut inp = Vec::with_capacity(total * 2);
        for s in 0..total {
            let v = (std::f64::consts::TAU * freq * s as f64 / rate as f64).sin() as f32 * 0.5;
            inp.push(v);
            inp.push(v);
        }
        let mut audio = Vec::with_capacity(total * 2);
        let mut done = 0;
        let mut buf = vec![0.0f32; block * 2];
        while done < total {
            let n = (total - done).min(block);
            buf[..n * 2].copy_from_slice(&inp[done * 2..(done + n) * 2]);
            p.eff_render(&mut buf[..n * 2]);
            audio.extend_from_slice(&buf[..n * 2]);
            done += n;
            p.idle();
        }
        let in_peak = inp.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        let out_peak = audio.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        let in_rms = (inp.iter().map(|s| (s * s) as f64).sum::<f64>() / inp.len() as f64).sqrt();
        let out_rms = (audio.iter().map(|s| (s * s) as f64).sum::<f64>() / audio.len() as f64).sqrt();
        println!(
            "effect: processed 1 s of 1 kHz sine; in peak {in_peak:.4} rms {in_rms:.4} -> out peak {out_peak:.4} rms {out_rms:.4}"
        );
        if let Some(out) = out {
            let mut f = std::io::BufWriter::new(std::fs::File::create(&out).expect("create wav"));
            os_engine::wav::write_f32_stereo(&mut f, rate, &audio).expect("write wav");
            println!("wrote {}", out.display());
        }
        report_log();
    }
    drop(p);
    println!("unloaded cleanly");

    #[cfg(windows)]
    if sandbox71 {
        report_sandbox();
    }
}

/// Fundamental of `x` (mono) by autocorrelation: the shortest lag within 95 % of the best one, 30..4000 Hz.
fn pitch_of(x: &[f32], rate: u32) -> Option<f64> {
    let (lo, hi) = ((rate / 4000) as usize, (rate / 30) as usize);
    if x.len() < hi * 2 || x.iter().all(|s| s.abs() < 1e-6) {
        return None;
    }
    let w = x.len() - hi;
    let corr: Vec<f64> = (lo..hi)
        .map(|lag| {
            let (mut c, mut e0, mut e1) = (0.0f64, 0.0f64, 0.0f64);
            for i in 0..w {
                let (a, b) = (f64::from(x[i]), f64::from(x[i + lag]));
                c += a * b;
                e0 += a * a;
                e1 += b * b;
            }
            c / (e0 * e1).sqrt().max(1e-12)
        })
        .collect();
    let best = corr.iter().cloned().fold(f64::MIN, f64::max);
    // The first local peak within 95 % of the best.
    let k = (1..corr.len() - 1).find(|&k| corr[k] >= 0.95 * best && corr[k] >= corr[k - 1] && corr[k] >= corr[k + 1])?;
    // Parabolic refinement.
    let (a, b, c) = (corr[k - 1], corr[k], corr[k + 1]);
    let d = 0.5 * (a - c) / (a - 2.0 * b + c);
    Some(f64::from(rate) / ((lo + k) as f64 + if d.is_finite() { d } else { 0.0 }))
}

fn stats(x: &[f32]) -> (f32, f64) {
    let peak = x.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    let rms = (x.iter().map(|s| f64::from(*s) * f64::from(*s)).sum::<f64>() / x.len().max(1) as f64).sqrt();
    (peak, rms)
}

/// Generators: (1) the plugin's own calls, a voice started at pitch 0 and at pitch 6000 cents (which one is key 60?);
/// (2) the hosted path the app uses: a C5 note at velocity 100 through the adapter for 1 s, note off, then until the
/// plugin has finished the voice (at most 10 s).
fn generator_report(mut p: Plugin, rate: u32, out: Option<&std::path::Path>) {
    use os_engine::instrument::Instrument;
    use os_host::Plugin as _;
    let block = 512;
    p.set_tempo(130.0, 96);
    p.set_enabled(true);
    p.set_playing(true);
    let spt = f64::from(rate) * 60.0 / 130.0 / 96.0;
    // NewTick only for plugins that ask for it (others leave it unimplemented).
    let wants_tick = p.info.flags & os_native::abi::flags::WANT_NEW_TICK != 0;
    for (tag, pitch) in [(1000, 0.0f32), (1001, 6000.0)] {
        let lv = os_native::abi::LevelParams { pan: 0.0, vol: 1.0, pitch, fcut: 0.0, fres: 0.0 };
        let Some(v) = p.trigger(lv, lv, 100.0 / 128.0, tag) else {
            match p.stop_reason() {
                Some(why) => println!("pitch {pitch}: the plugin stopped while starting the voice ({why})"),
                None => println!("pitch {pitch}: the plugin started no voice"),
            }
            continue;
        };
        println!("  voice handle {:#x}", v.handle());
        let mut audio = Vec::new();
        let mut buf = vec![0.0f32; block * 2];
        let mut to_tick = 0.0;
        let mut kills = 0;
        let mut freed = false;
        while audio.len() < rate as usize {
            if to_tick <= 0.0 {
                if wants_tick {
                    p.new_tick();
                }
                to_tick += spt;
            }
            let len = (to_tick.ceil() as usize).clamp(1, block);
            buf[..len * 2].fill(0.0);
            let got = if p.info.uses_voice_render() { p.voice_render(&v, &mut buf[..len * 2]).0 } else { p.gen_render(&mut buf[..len * 2]) };
            audio.extend(buf[..len * 2].chunks(2).map(|f| f[0]));
            let _ = got;
            to_tick -= len as f64;
            let (kill, _) = p.take_voice_requests();
            kills += kill.len();
            freed |= kill.iter().any(|k| k.0 == tag && !k.1);
        }
        // The plugin may have freed the voice itself (a kill request without KillHandle).
        if freed {
            p.forget(v);
        } else {
            p.kill(v);
        }
        let sus = &audio[rate as usize / 5..];
        let (peak, rms) = stats(sus);
        let f = pitch_of(&sus[..sus.len().min(8192)], rate);
        println!("raw voice at pitch {pitch:>6}: {} peak {peak:.4} rms {rms:.4}, kill requests {kills}",
            f.map_or("no pitch".into(), |f| format!("{f:8.2} Hz")));
    }
    p.take_voice_requests();

    let desc = os_host::Descriptor {
        format: os_host::Format::Native,
        id: p.info.long_name.clone(),
        name: p.info.long_name.clone(),
        vendor: String::new(),
        version: String::new(),
        features: Vec::new(),
        kind: os_host::Kind::Instrument,
        path: Default::default(),
    };
    let mut g = os_native::adapter::NativePlugin::new(p, desc);
    if !g.activate(rate) {
        println!("hosted: the plugin stopped while starting");
        return;
    }
    let (mut l, mut r) = (vec![0.0f32; block], vec![0.0f32; block]);
    let mut audio: Vec<f32> = Vec::new();
    g.note_on(0, 60, 100.0 / 128.0);
    let mut released_at = None;
    let mut ended_at = None;
    while audio.len() / 2 < rate as usize * 11 {
        let t = audio.len() / 2;
        if released_at.is_none() && t >= rate as usize {
            g.note_off(0, 60);
            released_at = Some(t);
        }
        g.process(rate, &mut l, &mut r);
        for i in 0..block {
            audio.push(l[i]);
            audio.push(r[i]);
        }
        if released_at.is_some() && !g.active() {
            ended_at = Some(audio.len() / 2);
            break;
        }
    }
    let frames = audio.len() / 2;
    let left: Vec<f32> = audio.chunks(2).map(|f| f[0]).collect();
    let sus = &left[rate as usize / 5..rate as usize];
    let (peak, rms) = stats(sus);
    let (rpeak, rrms) = stats(&audio.chunks(2).map(|f| f[1]).collect::<Vec<_>>()[rate as usize / 5..rate as usize]);
    let f = pitch_of(&sus[..sus.len().min(8192)], rate);
    let audible = left.iter().rposition(|s| s.abs() > 1e-4).unwrap_or(0);
    println!(
        "hosted C5 vel 100: {} L peak {peak:.4} rms {rms:.4} | R peak {rpeak:.4} rms {rrms:.4}; note off at 1.000 s, voice {}, last audible {:.3} s",
        f.map_or("no pitch".into(), |f| format!("{f:8.2} Hz")),
        ended_at.map_or("still alive at 11 s".into(), |e| format!("ended {:.3} s", e as f64 / f64::from(rate))),
        audible as f64 / f64::from(rate),
    );
    g.reset();
    if let Some(out) = out {
        let mut f = std::io::BufWriter::new(std::fs::File::create(out).expect("create wav"));
        os_engine::wav::write_f32_stereo(&mut f, rate, &audio[..frames * 2]).expect("write wav");
        println!("wrote {}", out.display());
    }
    if let Some(code) = os_host::Plugin::stopped(&g) {
        println!("hosted: the plugin stopped ({})", os_native::guard::describe_with(code, &os_native::guard::last_message()));
    }
}

fn report_log() {
    for l in host::take_call_log() {
        println!("  [host] {l}");
    }
}

#[cfg(windows)]
fn report_sandbox() {
    let log = probe71::take_log();
    println!("[sandbox71] {} observation(s):", log.len());
    for l in &log {
        println!("  [71] {l}");
    }
}
