//! os-native-probe: load one FL native plugin from the user's FL install, print its info and
//! parameters, save its default state, and render a test note (generators) to a WAV file.
//!
//! Usage: os-native-probe <plugin name> [out.wav] [--fl DIR]

use std::path::PathBuf;

use os_native::{find_plugin, host, Plugin};

fn main() {
    let mut args = std::env::args().skip(1);
    let mut name = None;
    let mut out = None;
    let mut fl = PathBuf::from(r"C:\Program Files\Image-Line\FL Studio 2026");
    while let Some(a) = args.next() {
        if a == "--fl" {
            fl = PathBuf::from(args.next().expect("--fl needs a value"));
        } else if name.is_none() {
            name = Some(a);
        } else {
            out = Some(PathBuf::from(a));
        }
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
        let mut audio = Vec::new();
        let mut buf = vec![0.0f32; block * 2];
        let voice = p.trigger(60.0, 0.78125, 0.0, 1);
        let total = rate as usize * 2;
        let release_at = rate as usize;
        let mut done = 0;
        let mut alive = true;
        while done < total {
            buf.fill(0.0);
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
        p.kill(voice);
        let peak = audio.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        println!("rendered {:.2} s, peak {peak:.4}", audio.len() as f64 / 2.0 / f64::from(rate));
        if let Some(out) = out {
            let mut f = std::io::BufWriter::new(std::fs::File::create(&out).expect("create wav"));
            os_engine::wav::write_f32_stereo(&mut f, rate, &audio).expect("write wav");
            println!("wrote {}", out.display());
        }
        report_log();
    }
    drop(p);
    println!("unloaded cleanly");
}

fn report_log() {
    for l in host::take_call_log() {
        println!("  [host] {l}");
    }
}
