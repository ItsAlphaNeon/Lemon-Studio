//! os-native-probe: load one FL native plugin from the user's FL install, print its info and
//! parameters, save its default state, and render a test note (generators) to a WAV file.
//!
//! Usage: os-native-probe <plugin name> [out.wav] [--fl DIR] [--sandbox71]
//!
//! `--sandbox71` installs the experimental sandbox-object answer to the undocumented host
//! dispatcher ID 71 (see `probe71`) and prints which of its vtable slots the plugin touched.

use std::path::PathBuf;

use os_native::{find_plugin, host, Plugin};

#[cfg(windows)]
mod probe71;

fn main() {
    let mut args = std::env::args().skip(1);
    let mut name = None;
    let mut out = None;
    let mut fl = PathBuf::from(r"C:\Program Files\Image-Line\FL Studio 2026");
    let mut sandbox71 = false;
    while let Some(a) = args.next() {
        if a == "--fl" {
            fl = PathBuf::from(args.next().expect("--fl needs a value"));
        } else if a == "--sandbox71" {
            sandbox71 = true;
        } else if name.is_none() {
            name = Some(a);
        } else {
            out = Some(PathBuf::from(a));
        }
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
    let state = p.save_state();
    println!("default state: {} bytes, head {:02x?}", state.len(), &state[..state.len().min(24)]);
    report_log();

    if i.is_generator() {
        p.set_tempo(130.0, 96);
        p.set_enabled(true);
        p.set_playing(true);
        let mut audio = Vec::new();
        let mut buf = vec![0.0f32; block * 2];
        let voice = p.trigger(60.0, 0.78125, 0.0, 1);
        let total = rate as usize * 2;
        let release_at = rate as usize;
        let mut done = 0;
        let mut alive = true;
        let wants_tick = i.flags & os_native::abi::flags::WANT_NEW_TICK != 0;
        // Samples per tick at this tempo/PPQ, so NewTick fires at tick boundaries like FL.
        let spt = (rate as f32 * 60.0 / 130.0 / 96.0).max(1.0);
        let mut next_tick = 0.0f32;
        while done < total {
            buf.fill(0.0);
            if wants_tick {
                while (done as f32) >= next_tick {
                    p.new_tick();
                    next_tick += spt;
                }
            }
            if done >= release_at && done < release_at + block {
                p.release(voice);
            }
            let n = if i.uses_voice_render() {
                if alive {
                    let (n, more) = p.voice_render(voice, &mut buf);
                    alive = more;
                    if n == 0 { block } else { n }
                } else {
                    block
                }
            } else {
                let n = p.gen_render(&mut buf);
                if n == 0 { block } else { n }
            };
            audio.extend_from_slice(&buf[..n * 2]);
            done += n;
            p.idle();
        }
        let killed = p.killed_voices();
        p.kill(voice);
        let peak = audio.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        // How long was there audible output (sustain check)?
        let active = audio.chunks(2).rposition(|s| s[0].abs() > 1e-4 || s[1].abs() > 1e-4).unwrap_or(0);
        println!(
            "rendered {:.2} s, peak {peak:.4}, last audible {:.3} s, plugin-killed voices {:?}",
            audio.len() as f64 / 2.0 / f64::from(rate),
            active as f64 / f64::from(rate),
            killed
        );
        if let Some(out) = out {
            let mut f = std::io::BufWriter::new(std::fs::File::create(&out).expect("create wav"));
            os_engine::wav::write_f32_stereo(&mut f, rate, &audio).expect("write wav");
            println!("wrote {}", out.display());
        }
        report_log();
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
