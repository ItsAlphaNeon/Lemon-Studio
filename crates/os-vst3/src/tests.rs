// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

//! Tests host Lemon Studio's own plug-ins as VST 3 (built by `cargo run -p bundle-plugins` into `<target>/bundled`), and
//! skip when they are not built. `OS_VST3_PLUGIN=<bundle or module>` adds any other plug-in to `hosts_a_plugin_from_env`.
// spec: plugins/vst3-hosting.md

use super::*;
use os_host::Library as _;

/// `<target>/bundled/<name>.vst3` of this test's own build folder, or of the usual ones.
fn bundle(name: &str) -> Option<PathBuf> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    // target*/debug/deps/x.exe -> target*
    let own = std::env::current_exe().ok().and_then(|e| Some(e.parent()?.parent()?.parent()?.to_path_buf()));
    own.into_iter().chain(["target-dev", "target"].map(|t| root.join(t))).map(|t| t.join("bundled").join(format!("{name}.vst3"))).find(|p| p.exists())
}

fn module(name: &str) -> Option<Arc<Module>> {
    let Some(p) = bundle(name) else {
        eprintln!("{name}.vst3 not built; skipped");
        return None;
    };
    Some(Module::load(&p).expect("load"))
}

fn sampler() -> Option<(Arc<Module>, os_host::SharedPlugin)> {
    let m = module("Lemon Sampler")?;
    let p = m.clone().create(&m.plugins()[0].id).expect("create");
    Some((m, p))
}

/// Lemon Sampler's state with a sample (nih-plug's VST 3 state is its JSON; persisted fields are JSON strings).
fn sampler_state(wav: &Path, params: &str) -> Vec<u8> {
    let inner = format!("\"{}\"", wav.to_string_lossy().replace('\\', "\\\\"));
    let path = format!("\"{}\"", inner.replace('\\', "\\\\").replace('"', "\\\""));
    format!("{{\"version\":\"0.0.1\",\"params\":{{{params}}},\"fields\":{{\"sample-path\":{path}}}}}").into_bytes()
}

fn tone(name: &str, frames: usize) -> PathBuf {
    let wav = std::env::temp_dir().join(name);
    let t: Vec<f32> = (0..frames).flat_map(|i| { let v = (i as f32 * 0.05).sin() * 0.5; [v, v] }).collect();
    os_engine::wav::write_f32_stereo(&mut std::fs::File::create(&wav).unwrap(), 48000, &t).unwrap();
    wav
}

fn peak(v: &[f32]) -> f32 {
    v.iter().fold(0.0f32, |m, x| m.max(x.abs()))
}

#[test]
fn fuid_strings_follow_the_documented_layout() {
    let id = IID { data: *b"OpenStudioSamplr" };
    let other = IID { data: [0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0xA0, 0xB1, 0xC2, 0xD3, 0xE4, 0xF5, 0x06, 0x17] };
    if cfg!(windows) {
        // In memory (COM GUID layout) Data1 / Data2 / Data3 are little-endian numbers.
        assert_eq!(fuid_string(&id), "6E65704F74536475696F53616D706C72");
        assert_eq!(fuid_string(&other), "0403020106050807A0B1C2D3E4F50617");
    } else {
        // Elsewhere the bytes are in string order.
        assert_eq!(fuid_string(&id), "4F70656E53747564696F53616D706C72");
        assert_eq!(fuid_string(&other), "0102030405060708A0B1C2D3E4F50617");
    }
}

#[test]
fn finds_bundles_and_single_file_modules() {
    let d = std::env::temp_dir().join(format!("os-vst3-find-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join("A.vst3").join("Contents").join(BUNDLE_ARCH)).unwrap();
    std::fs::create_dir_all(d.join("Vendor").join("Sub")).unwrap();
    std::fs::write(d.join("Vendor").join("Sub").join("B.vst3"), b"").unwrap();
    std::fs::write(d.join("notes.txt"), b"").unwrap();
    let found = find_bundles(&[d.clone()]);
    assert_eq!(found, vec![d.join("A.vst3"), d.join("Vendor").join("Sub").join("B.vst3")]);
    let module = if cfg!(windows) { "A.vst3" } else if cfg!(target_os = "macos") { "A" } else { "A.so" };
    assert_eq!(module_file(&found[0]), d.join("A.vst3").join("Contents").join(BUNDLE_ARCH).join(module));
    assert_eq!(module_file(&found[1]), found[1]);
    assert!(Module::load(&found[0]).is_err(), "an empty bundle is not a plug-in");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn lists_instrument_and_effect_classes() {
    let Some(m) = module("Lemon Sampler") else { return };
    let d = &m.plugins()[0];
    assert_eq!(m.plugins().len(), 1);
    assert_eq!(d.name, "Lemon Sampler");
    assert!(d.is_instrument(), "{:?}", d.features);
    // Its FUID string spells "OpenStudioSamplr" in hex (the module lays the GUID out so that it reads in order).
    assert_eq!(d.uid(), "vst3:4F70656E53747564696F53616D706C72");
    assert!(!d.vendor.is_empty() && !d.version.is_empty());
    assert_eq!(d.path, bundle("Lemon Sampler").unwrap());
    let Some(eq) = module("Lemon Parametric EQ 2") else { return };
    assert!(eq.plugins()[0].is_effect(), "{:?}", eq.plugins()[0].features);
}

/// An instrument plays notes, saves its state as a `.vstpreset` and loads it into a second instance.
#[test]
fn hosts_lemon_sampler() {
    let Some((m, shared)) = sampler() else { return };
    let mut p = shared.lock().unwrap();
    let wav = tone("os-vst3-test.wav", 4800);
    assert!(p.load_state(&sampler_state(&wav, "")), "a bare component state loads");
    assert!(p.activate(48000));
    let (mut l, mut r) = (vec![0.0f32; 512], vec![0.0f32; 512]);
    p.note_on(10, 60, 100.0 / 128.0);
    p.process(48000, &mut l, &mut r);
    assert_eq!(l[9], 0.0, "silent before the note");
    assert!(peak(&l) > 0.05, "plays the sample (peak {})", peak(&l));
    assert!(p.active());
    let saved = p.save_state().unwrap();
    let preset = preset::Preset::parse(&saved).expect("a .vstpreset");
    assert_eq!(preset.class_id, p.desc().id);
    assert!(String::from_utf8_lossy(&preset.component).contains("os-vst3-test.wav"));
    // Another instance takes the saved bytes; a preset of another class is refused.
    let q = m.clone().create(&p.desc().id).unwrap();
    let mut q = q.lock().unwrap();
    assert!(q.load_state(&saved));
    let again = preset::Preset::parse(&q.save_state().unwrap()).unwrap();
    assert!(String::from_utf8_lossy(&again.component).contains("os-vst3-test.wav"));
    let other = preset::Preset { class_id: "0".repeat(32), ..preset }.to_bytes();
    assert!(!q.load_state(&other));
}

/// Stop: queued notes are dropped and the voices end at once.
#[test]
fn reset_silences_the_instrument() {
    let Some((_m, shared)) = sampler() else { return };
    let mut p = shared.lock().unwrap();
    let wav = tone("os-vst3-reset.wav", 48000);
    assert!(p.load_state(&sampler_state(&wav, "")));
    assert!(p.activate(48000));
    let (mut l, mut r) = (vec![0.0f32; 1024], vec![0.0f32; 1024]);
    p.note_on(0, 60, 0.8);
    p.process(48000, &mut l, &mut r);
    assert!(peak(&l) > 0.05);
    p.note_on(900, 64, 0.8);
    p.reset();
    assert!(!p.active());
    p.process(48000, &mut l, &mut r);
    p.process(48000, &mut l, &mut r);
    assert!(peak(&l) < 1e-4, "silent after reset (peak {})", peak(&l));
}

/// The hosting layer's parameter surface: info, normalized get / set, text, the edit reaching the plug-in's state,
/// and the plug-in's own edits (component handler) coming back as gesture events.
#[test]
fn parameters_through_the_hosting_layer() {
    let Some((_m, shared)) = sampler() else { return };
    let mut p = shared.lock().unwrap();
    assert!(p.param_count() > 20);
    let infos: Vec<os_host::ParamInfo> = (0..p.param_count()).filter_map(|i| p.param_info(i)).collect();
    assert_eq!(infos.len(), p.param_count());
    let start = infos.iter().find(|i| i.name == "Sample start offset").expect("start offset parameter");
    assert!(start.automatable && start.steps == 0 && !start.read_only);
    assert!((p.param_value(start.id).unwrap() - start.default).abs() < 1e-6);
    assert!(p.set_param(start.id, 0.25));
    assert!(!p.set_param(0xdead_beef, 0.5), "unknown ids are refused");
    assert!(p.activate(48000));
    p.idle();
    assert!((p.param_value(start.id).unwrap() - 0.25).abs() < 1e-3, "{:?}", p.param_value(start.id));
    assert!(p.param_text(start.id, 0.25).is_some_and(|t| !t.is_empty()));
    // The value reaches the processor in the next block, and the state.
    let (mut l, mut r) = (vec![0.0f32; 64], vec![0.0f32; 64]);
    p.process(48000, &mut l, &mut r);
    let saved = preset::Preset::parse(&p.save_state().unwrap()).unwrap();
    let json = String::from_utf8_lossy(&saved.component).into_owned();
    assert!(json.contains("\"start_offset\":{\"f32\":0.25"), "{json}");
    assert!(infos.iter().any(|i| i.enumerated || i.steps > 0), "stepped parameters are reported");
}

/// The component handler as a plug-in sees it: gestures are reported to the app and values go to the processor.
#[test]
fn component_handler_reports_gestures() {
    use vst3_sys::vst::IComponentHandler;
    let shared = Arc::new(Shared::default());
    let h = Handler::create(shared.clone());
    unsafe {
        h.get().begin_edit(7);
        h.get().perform_edit(7, 0.5);
        h.get().perform_edit(7, 0.75);
        h.get().end_edit(7);
        h.get().restart_component(LATENCY_CHANGED);
    }
    assert_eq!(
        *shared.events.lock().unwrap(),
        vec![os_host::ParamEvent::Begin(7), os_host::ParamEvent::Value(7, 0.5), os_host::ParamEvent::Value(7, 0.75), os_host::ParamEvent::End(7)]
    );
    assert_eq!(*shared.to_processor.lock().unwrap(), vec![(7, 0.75)], "the processor gets the newest value once");
    assert_eq!(shared.restart.load(Ordering::Acquire), LATENCY_CHANGED);
}

/// The memory stream and parameter queues behave as the interfaces document.
#[test]
fn host_objects_follow_the_interfaces() {
    use vst3_sys::base::IBStream;
    use vst3_sys::vst::{IParamValueQueue, IParameterChanges};
    let s = Stream::create(Vec::new());
    unsafe {
        let mut n = 0;
        s.get().write(b"abcdef".as_ptr() as *const c_void, 6, &mut n);
        assert_eq!(n, 6);
        let mut pos = 0i64;
        s.get().seek(-2, 2, &mut pos);
        assert_eq!(pos, 4);
        let mut buf = [0u8; 8];
        s.get().read(buf.as_mut_ptr() as *mut c_void, 8, &mut n);
        assert_eq!((n, &buf[..2]), (2, &b"ef"[..]));
        s.get().tell(&mut pos);
        assert_eq!(pos, 6);
    }
    let pc = ParamChanges::create();
    pc.get().add(3, 0, 0.1);
    pc.get().add(3, 64, 0.9);
    pc.get().add(5, 10, 0.5);
    unsafe {
        assert_eq!(pc.get().get_parameter_count(), 2);
        let q = pc.get().get_parameter_data(0).upgrade().unwrap();
        assert_eq!((q.get_parameter_id(), q.get_point_count()), (3, 2));
        let (mut o, mut v) = (0, 0.0);
        q.get_point(1, &mut o, &mut v);
        assert_eq!((o, v), (64, 0.9));
    }
    let mut last = Vec::new();
    pc.get().for_each_last(|id, v| last.push((id, v)));
    assert_eq!(last, vec![(3, 0.9), (5, 0.5)]);
    pc.get().clear();
    assert_eq!(unsafe { pc.get().get_parameter_count() }, 0);
}

/// An effect in a mixer slot: the default EQ passes the sound through; the main level raises it.
#[test]
fn hosts_an_effect() {
    let Some(m) = module("Lemon Parametric EQ 2") else { return };
    let shared = m.clone().create(&m.plugins()[0].id).unwrap();
    let mut p = shared.lock().unwrap();
    assert!(p.activate(48000));
    let src: Vec<f32> = (0..2048).map(|i| (i as f32 * 0.02).sin() * 0.25).collect();
    let run = |p: &mut dyn os_host::Plugin| {
        let (mut l, mut r) = (src.clone(), src.clone());
        for _ in 0..4 {
            l.copy_from_slice(&src);
            r.copy_from_slice(&src);
            p.process_fx(48000, &mut l, &mut r);
        }
        peak(&l[1024..])
    };
    let flat = run(&mut *p);
    assert!((flat - 0.25).abs() < 0.03, "passes the input through (peak {flat})");
    let main = (0..p.param_count()).filter_map(|i| p.param_info(i)).find(|i| i.name == "Main level").expect("main level");
    assert!(p.set_param(main.id, 1.0));
    let loud = run(&mut *p);
    assert!(loud > flat * 4.0, "+18 dB main level (peak {loud} vs {flat})");
    assert_eq!(p.latency(), 0);
}

/// Automation (`HostedEffect::param_fx`, by list index) reaches the processor at its frame offset, the controller
/// follows, and without a stream it is flushed on idle.
#[test]
fn automation_is_sample_accurate() {
    use os_engine::HostedEffect;
    use os_host::Plugin as _;
    use vst3_sys::vst::{IParamValueQueue, IParameterChanges};
    let Some(m) = module("Lemon Parametric EQ 2") else { return };
    let mut p = Plugin::new(&m, &m.plugins()[0].id).unwrap();
    assert!(p.activate(48000));
    let index = (0..p.param_count()).find(|&i| p.param_info(i).is_some_and(|i| i.name == "Main level")).expect("main level") as u32;
    let id = p.param_info(index as usize).unwrap().id;
    let src = vec![0.25f32; 2048];
    let (mut l, mut r) = (src.clone(), src.clone());
    p.process_fx(48000, &mut l, &mut r);
    l.copy_from_slice(&src);
    r.copy_from_slice(&src);
    p.param_fx(1024, index, 1.0);
    p.process_fx(48000, &mut l, &mut r);
    // The block carried the point at its frame (nih-plug applies it at the block start unless the plug-in asks for
    // sample-accurate automation, so the sound only shows that it arrived).
    let mut points = Vec::new();
    unsafe {
        for i in 0..p.params_in.get().get_parameter_count() {
            let q = p.params_in.get().get_parameter_data(i).upgrade().unwrap();
            let (mut o, mut v) = (0, 0.0);
            q.get_point(0, &mut o, &mut v);
            points.push((q.get_parameter_id(), o, v));
        }
    }
    assert_eq!(points, vec![(id, 1024, 1.0)]);
    assert!(peak(&l[1100..]) > 1.0, "+18 dB ({})", peak(&l[1100..]));
    p.idle();
    assert!((p.param_value(id).unwrap() - 1.0).abs() < 1e-6, "the controller follows the automation");
    // No stream: the value is flushed by idle after a short wait.
    p.param_fx(0, index, 0.5);
    p.idle();
    std::thread::sleep(std::time::Duration::from_millis(60));
    p.idle();
    assert!(p.auto.is_empty(), "flushed");
    l.copy_from_slice(&src);
    r.copy_from_slice(&src);
    p.process_fx(48000, &mut l, &mut r);
    // (The plug-in glides to a new level within the block.)
    assert!((peak(&l[1800..]) - 0.25).abs() < 0.03, "0 dB after the flushed automation ({})", peak(&l[1800..]));
}

/// The whole path Lemon Studio uses: a pattern on a plug-in channel, played by the engine through the hosted .vst3.
#[test]
fn engine_plays_a_pattern() {
    let Some((_m, shared)) = sampler() else { return };
    {
        let mut p = shared.lock().unwrap();
        assert!(p.load_state(&sampler_state(&tone("os-vst3-engine.wav", 9600), "")));
        assert!(p.activate(48000));
    }
    let ch = os_model::Channel {
        index: 0, kind: os_model::ChannelKind::Plugin, name: None, plugin: Some("Lemon Sampler".into()), plugin_state: None, color: None,
        enabled: true, volume: 0.78125, pan: 0.0, pitch_cents: 0, mixer_track: 0, sample_path: None,
        layer_children: vec![], envelopes: Default::default(), rack: Default::default(), misc: Default::default(), sound: Default::default(),
    };
    let note = os_model::Note { length: 24, ..os_model::Note::step(0, 0) };
    let project = os_model::Project {
        ppq: 96, tempo: Some(130.0), channels: vec![ch],
        patterns: vec![os_model::Pattern { number: 1, notes: vec![note], ..Default::default() }],
        ..Default::default()
    };
    let mut lib = os_engine::SampleLibrary::default();
    let mut e = os_engine::Engine::new(&project, &mut lib, 48000, os_engine::PlayMode::Pattern(1));
    e.set_instrument(0, shared.clone() as os_engine::SharedInstrument);
    let mut out = vec![0.0f32; 1024];
    e.process(&mut out);
    assert!(peak(&out) > 0.05, "engine output through the VST 3 (peak {})", peak(&out));
}

/// Mono portamento on a VST 3 channel: the second key glides the sounding note through its tuning note expression
/// (spec §5) instead of restarting it.
#[test]
fn mono_portamento_glides_through_the_tuning_expression() {
    let Some((_m, shared)) = sampler() else { return };
    // 4 s of a 441 Hz sine at 48 kHz.
    let wav = std::env::temp_dir().join("os-vst3-porta.wav");
    let sine: Vec<f32> = (0..48000 * 4).flat_map(|i| { let v = (i as f32 / 48000.0 * 441.0 * std::f32::consts::TAU).sin() * 0.5; [v, v] }).collect();
    os_engine::wav::write_f32_stereo(&mut std::fs::File::create(&wav).unwrap(), 48000, &sine).unwrap();
    {
        let mut p = shared.lock().unwrap();
        assert!(p.load_state(&sampler_state(&wav, "")));
        assert!(p.activate(48000));
    }
    let misc = os_model::ChannelMisc { mono: true, porta: true, slide: 1.0, ..Default::default() };
    let ch = os_model::Channel {
        index: 0, kind: os_model::ChannelKind::Plugin, name: None, plugin: Some("Lemon Sampler".into()), plugin_state: None, color: None,
        enabled: true, volume: 0.78125, pan: 0.0, pitch_cents: 0, mixer_track: 0, sample_path: None,
        layer_children: vec![], envelopes: Default::default(), rack: Default::default(), misc, sound: Default::default(),
    };
    let project = os_model::Project { ppq: 96, tempo: Some(130.0), channels: vec![ch], ..Default::default() };
    let mut lib = os_engine::SampleLibrary::default();
    let mut e = os_engine::Engine::new(&project, &mut lib, 48000, os_engine::PlayMode::Pattern(1));
    e.clear_schedule();
    e.set_instrument(0, shared.clone() as os_engine::SharedInstrument);
    let mut out = Vec::new();
    let block = 480;
    for b in 0..(48000 * 3 / block) {
        match b {
            10 => e.note_on(0, 60, 100),
            // An octave up at 1 s, while 60 is still held.
            100 => e.note_on(0, 72, 100),
            _ => {}
        }
        let mut buf = vec![0.0f32; block * 2];
        e.process(&mut buf);
        out.extend(buf.chunks_exact(2).map(|f| f[0]));
    }
    let window = |from_s: f32, to_s: f32| &out[(from_s * 48000.0) as usize..(to_s * 48000.0) as usize];
    let crossings = |w: &[f32]| w.windows(2).filter(|p| p[0] <= 0.0 && p[1] > 0.0).count() as f32 / (w.len() as f32 / 48000.0);
    // One step per tick, n + 1 ticks at 130 BPM and 96 PPQ.
    let glide_s = (os_engine::laws::glide_ticks(1.0, 96) + 1) as f32 * 60.0 / (130.0 * 96.0);
    let (before, mid, after) =
        (crossings(window(0.5, 0.95)), crossings(window(1.0 + glide_s * 0.4, 1.0 + glide_s * 0.6)), crossings(window(1.0 + glide_s + 0.05, 2.0)));
    eprintln!("before {before:.1} Hz, mid-glide {mid:.1} Hz, after {after:.1} Hz (glide {glide_s:.3} s)");
    assert!((before - 441.0).abs() < 4.0, "first note in tune: {before}");
    assert!((after - 882.0).abs() < 8.0, "glides to the octave: {after}");
    assert!(mid > 500.0 && mid < 800.0, "between the two keys mid-glide: {mid}");
    // Legato: the note keeps sounding through the glide (no restart at 1 s).
    let (p0, p1) = (peak(window(0.9, 0.99)), peak(window(1.0, 1.05)));
    assert!(p1 > p0 * 0.8, "no retrigger gap: {p0} -> {p1}");
}

/// A plug-in that is not active at the stream's rate stays silent and asks the UI thread to activate it.
#[test]
fn rate_changes_reactivate_on_idle() {
    let Some((_m, shared)) = sampler() else { return };
    let mut p = shared.lock().unwrap();
    assert!(p.load_state(&sampler_state(&tone("os-vst3-rate.wav", 4800), "")));
    assert!(p.activate(48000));
    let (mut l, mut r) = (vec![0.0f32; 256], vec![0.0f32; 256]);
    p.note_on(0, 60, 0.8);
    p.process(44100, &mut l, &mut r);
    assert_eq!(peak(&l), 0.0);
    p.idle();
    p.process(44100, &mut l, &mut r);
    assert!(peak(&l) > 0.05, "plays at 44.1 kHz after the idle reactivation");
}

/// `OS_VST3_PLUGIN=<path>`: any VST 3 plug-in loads, activates, processes a block and saves its state.
#[test]
fn hosts_a_plugin_from_env() {
    let Some(path) = std::env::var_os("OS_VST3_PLUGIN") else { return };
    let m = Module::load(Path::new(&path)).expect("load");
    for d in m.plugins() {
        eprintln!("{} | {} | {} | {:?} | {:?}", d.uid(), d.name, d.vendor, d.kind, d.features);
        let shared = m.clone().create(&d.id).expect("create");
        let mut p = shared.lock().unwrap();
        assert!(p.activate(48000), "{} activates", d.name);
        let (mut l, mut r) = (vec![0.1f32; 512], vec![0.1f32; 512]);
        if d.is_instrument() {
            p.note_on(0, 60, 0.8);
            p.process(48000, &mut l, &mut r);
        } else {
            p.process_fx(48000, &mut l, &mut r);
        }
        let state = p.save_state();
        eprintln!("  params {}, latency {}, editor {}, state {:?} bytes, peak {}", p.param_count(), p.latency(), p.has_editor(), state.as_ref().map(Vec::len), peak(&l));
        if let Some(s) = state {
            assert!(p.load_state(&s));
        }
    }
}

/// A plug-in whose edit controller is a class of its own, the layout of most VST 3 plug-ins (`mock.rs`).
mod separate {
    use super::*;
    use crate::mock::{self, Probe, EXTRA, GAIN, MODE, PEAK};
    use os_engine::{HostedEffect, Instrument};
    use os_host::ParamEvent::{Begin, End, Value};
    use os_host::Plugin as _;

    fn test_module(probe: &Arc<Probe>) -> Arc<Module> {
        unsafe { Module::from_factory(Path::new("Test Gain.vst3"), mock::Factory::create(probe), None) }.expect("module")
    }

    fn instance(probe: &Arc<Probe>) -> (Arc<Module>, Plugin) {
        let m = test_module(probe);
        let id = m.plugins()[0].id.clone();
        let p = Plugin::new(&m, &id).expect("create");
        (m, p)
    }

    fn ones(n: usize) -> (Vec<f32>, Vec<f32>) {
        (vec![1.0; n], vec![1.0; n])
    }

    #[test]
    fn lists_only_audio_classes() {
        let probe = Arc::new(Probe::default());
        let m = test_module(&probe);
        assert_eq!(m.plugins().len(), 1, "the controller class is not a plug-in");
        let d = &m.plugins()[0];
        assert_eq!((d.name.as_str(), d.vendor.as_str(), d.version.as_str()), ("Test Gain", "Test Vendor", "1.2.3"));
        assert!(d.is_effect(), "{:?}", d.features);
        assert_eq!(d.id, fuid_string(&mock::COMPONENT_CID));
        assert_eq!(d.uid(), format!("vst3:{}", d.id));
        assert!(Plugin::new(&m, &fuid_string(&mock::CONTROLLER_CID)).is_err());
    }

    #[test]
    fn setup_connects_the_controller_to_the_component() {
        let probe = Arc::new(Probe::default());
        probe.latency.store(64, Ordering::Release);
        let (_m, mut p) = instance(&probe);
        let log = probe.log.lock().unwrap().clone();
        let at = |s: &str| log.iter().position(|l| l == s).unwrap_or_else(|| panic!("{s} missing in {log:?}"));
        // Spec §4: both initialised with the host context, connected both ways, the handler, then the component's state.
        assert!(at("comp.initialize host=true") < at("ctrl.initialize host=true"));
        assert!(at("ctrl.initialize host=true") < at("comp.connect").min(at("ctrl.connect")));
        assert!(at("ctrl.set_component_handler true") < at("ctrl.set_component_state 0.5"));
        at("comp.set_bus_arrangements true");
        at("comp.activate_bus 0 0 0 1");
        at("comp.activate_bus 0 1 0 1");
        assert!(!p.has_editor(), "no view, no editor");
        // Parameters are the controller's.
        assert_eq!(p.param_count(), 3);
        let g = p.param_info(0).unwrap();
        assert_eq!((g.id, g.name.as_str(), g.unit.as_str(), g.min, g.max, g.default), (GAIN, "Gain", "dB", -24.0, 24.0, 0.5));
        assert!(g.automatable && !g.enumerated && !g.read_only);
        let mode = p.param_info(1).unwrap();
        assert!(mode.enumerated && mode.steps == 2);
        assert!(p.param_info(2).unwrap().read_only);
        assert_eq!(p.param_text(MODE, 1.0).as_deref(), Some("Clip"));
        assert_eq!(p.param_value(GAIN), Some(0.5));
        // Activation: the component tells its controller through a message the host created.
        assert!(p.activate(48000));
        assert_eq!(p.latency(), 64);
        assert_eq!(probe.logged("comp.setup_processing"), ["comp.setup_processing 48000 4096 0"]);
        assert_eq!(probe.logged("ctrl.notify"), ["ctrl.notify activated 64"]);
    }

    #[test]
    fn edits_reach_both_halves() {
        let probe = Arc::new(Probe::default());
        let (_m, mut p) = instance(&probe);
        assert!(p.activate(48000));
        // A host edit: the controller at once, the processor in its next block.
        assert!(p.set_param(GAIN, 0.75));
        assert_eq!(probe.controller().value(GAIN), 0.75);
        let (mut l, mut r) = ones(256);
        p.process_fx(48000, &mut l, &mut r);
        assert_eq!((l[0], r[255]), (1.5, 1.5));
        // The processor's output parameter reaches the controller on idle.
        p.idle();
        assert_eq!(probe.controller().value(PEAK), 1.0);
        // An edit in the plug-in's editor: the gesture for the app, the value for the processor.
        probe.controller().user_edit(GAIN, 0.25);
        assert_eq!(p.take_param_events(), [Begin(GAIN), Value(GAIN, 0.25), End(GAIN)]);
        let (mut l, mut r) = ones(64);
        p.process_fx(48000, &mut l, &mut r);
        assert_eq!(l[0], 0.5);
        // Without a stream an edit is flushed with an empty block.
        assert!(p.set_param(GAIN, 0.6));
        p.idle();
        std::thread::sleep(std::time::Duration::from_millis(60));
        p.idle();
        assert_eq!(probe.gain_points.lock().unwrap().last(), Some(&(0, 0.6)));
        let (mut l, mut r) = ones(16);
        p.process_fx(48000, &mut l, &mut r);
        assert!((l[0] - 1.2).abs() < 1e-6);
    }

    #[test]
    fn automation_is_sample_accurate_and_the_controller_follows() {
        let probe = Arc::new(Probe::default());
        let (_m, mut p) = instance(&probe);
        assert!(p.activate(48000));
        p.param_fx(100, 0, 0.25);
        // Mode has two steps: 0.4 becomes 0.5.
        p.param_fx(0, 1, 0.4);
        // Past this block: moves on to the next one.
        p.param_fx(300, 0, 0.125);
        let (mut l, mut r) = ones(256);
        p.process_fx(48000, &mut l, &mut r);
        assert_eq!((l[99], l[100], r[255]), (1.0, 0.5, 0.5));
        assert_eq!(*probe.gain_points.lock().unwrap(), [(100, 0.25)]);
        p.idle();
        assert_eq!((probe.controller().value(GAIN), probe.controller().value(MODE)), (0.25, 0.5));
        let (mut l, mut r) = ones(256);
        p.process_fx(48000, &mut l, &mut r);
        assert_eq!((l[43], l[44]), (0.5, 0.25), "the later point lands 300 - 256 frames into the second block");
        // Unknown indexes are ignored.
        p.param_fx(0, 99, 1.0);
        p.process_fx(48000, &mut l, &mut r);
    }

    #[test]
    fn state_holds_both_halves() {
        let probe = Arc::new(Probe::default());
        let (_m, mut p) = instance(&probe);
        assert!(p.activate(48000));
        probe.controller().user_edit(GAIN, 0.25);
        probe.controller().user_edit(MODE, 1.0);
        let (mut l, mut r) = ones(32);
        p.process_fx(48000, &mut l, &mut r);
        let saved = p.save_state().unwrap();
        let preset = preset::Preset::parse(&saved).unwrap();
        assert_eq!(preset.class_id, p.desc().id);
        assert_eq!(preset.component, 0.25f64.to_le_bytes());
        assert_eq!(&preset.controller[..4], b"MODE");
        // Change both, then restore.
        p.set_param(GAIN, 0.9);
        p.process_fx(48000, &mut l, &mut r);
        probe.controller().user_edit(MODE, 0.0);
        assert!(p.load_state(&saved));
        assert_eq!(probe.logged("comp.set_state"), ["comp.set_state 0.25"]);
        assert_eq!(probe.logged("ctrl.set_component_state").last().map(String::as_str), Some("ctrl.set_component_state 0.25"));
        assert_eq!(probe.logged("ctrl.set_state"), ["ctrl.set_state 1"]);
        assert_eq!((probe.controller().value(GAIN), probe.controller().value(MODE)), (0.25, 1.0));
        let (mut l, mut r) = ones(32);
        p.process_fx(48000, &mut l, &mut r);
        assert_eq!(l[0], 0.5);
        // Another class's preset is refused; bare component bytes are accepted.
        let other = preset::Preset { class_id: "0".repeat(32), ..preset };
        assert!(!p.load_state(&other.to_bytes()));
        assert!(p.load_state(&0.5f64.to_le_bytes()));
        assert_eq!(probe.controller().value(GAIN), 0.5);
    }

    #[test]
    fn restart_requests_are_handled_on_idle() {
        let probe = Arc::new(Probe::default());
        let (_m, mut p) = instance(&probe);
        assert!(p.activate(48000));
        // Latency change: deactivate, reactivate, read it again (spec §7).
        probe.latency.store(128, Ordering::Release);
        probe.controller().restart(LATENCY_CHANGED);
        assert_eq!(p.latency(), 0, "not before idle");
        p.idle();
        assert_eq!(p.latency(), 128);
        assert_eq!(probe.logged("comp.set_active"), ["comp.set_active 1", "comp.set_active 0", "comp.set_active 1"]);
        assert_eq!(probe.logged("ctrl.notify").last().map(String::as_str), Some("ctrl.notify activated 128"));
        // Parameter list change: the automation indexes follow.
        probe.extra.store(true, Ordering::Release);
        assert_eq!(p.param_value(EXTRA), None);
        probe.controller().restart(PARAM_TITLES_CHANGED);
        p.idle();
        assert_eq!(p.param_count(), 4);
        assert_eq!(p.param_value(EXTRA), Some(0.0));
        p.param_fx(0, 3, 0.7);
        let (mut l, mut r) = ones(32);
        p.process_fx(48000, &mut l, &mut r);
        p.idle();
        assert_eq!(probe.controller().value(EXTRA), 0.7);
        // Stop: processing restarts before the next block.
        p.reset();
        p.process_fx(48000, &mut l, &mut r);
        let processing = probe.logged("comp.set_processing");
        assert_eq!(processing[processing.len() - 2..], ["comp.set_processing 0", "comp.set_processing 1"]);
    }

    /// The engine's transport becomes the process context: playing, tempo, musical position (per chunk of a long
    /// block), bar start and loop; without one the transport reads as stopped (spec §5).
    #[test]
    fn the_transport_reaches_the_process_context() {
        let probe = Arc::new(Probe::default());
        let (_m, mut p) = instance(&probe);
        assert!(p.activate(48000));
        let (mut l, mut r) = ones(64);
        p.process_fx(48000, &mut l, &mut r);
        let song = os_engine::TransportInfo {
            playing: true,
            tempo: 150.0,
            frame: 96000,
            beats: 5.0,
            bar_start: 4.0,
            bar: 1,
            numerator: 4,
            denominator: 4,
            looping: Some((0.0, 8.0)),
        };
        p.transport_fx(&song);
        let (mut l, mut r) = ones(5000);
        p.process_fx(48000, &mut l, &mut r);
        let (mut l, mut r) = ones(64);
        p.process_fx(48000, &mut l, &mut r);
        let c = probe.contexts.lock().unwrap().clone();
        assert_eq!(c.len(), 4, "{c:?}");
        let valid = CONT_TIME_VALID | TEMPO_VALID | TIME_SIG_VALID | PROJECT_TIME_MUSIC_VALID | BAR_POSITION_VALID;
        assert_eq!(c[0].0 & (PLAYING | TEMPO_VALID), 0, "no transport yet: {:#x}", c[0].0);
        assert_eq!(c[1].0, valid | PLAYING | CYCLE_ACTIVE | CYCLE_VALID);
        assert_eq!((c[1].1, c[1].2, c[1].3, c[1].4, c[1].5), (150.0, 5.0, 96000, 4.0, 8.0));
        // The second chunk of the 5000-frame block starts 4096 frames (2.133 beats at 150 BPM) later, in the next bar.
        let beats = 5.0 + 4096.0 / (48000.0 * 60.0 / 150.0);
        assert_eq!(c[2].3, 96000 + 4096);
        assert!((c[2].2 - beats).abs() < 1e-9 && c[2].4 == 4.0 * (beats / 4.0).floor(), "{:?}", c[2]);
        // The next block has no transport: stopped where it was, tempo still known.
        assert_eq!(c[3].0 & (PLAYING | CYCLE_ACTIVE), 0);
        assert_eq!(c[3].0 & TEMPO_VALID, TEMPO_VALID);
        assert_eq!(c[3].3, 96000 + 4096);
    }

    #[test]
    fn teardown_follows_the_documented_order() {
        let probe = Arc::new(Probe::default());
        {
            let (_m, mut p) = instance(&probe);
            assert!(p.activate(48000));
            let (mut l, mut r) = ones(32);
            p.process_fx(48000, &mut l, &mut r);
            probe.log.lock().unwrap().clear();
        }
        let log = probe.log.lock().unwrap().clone();
        let at = |s: &str| log.iter().position(|l| l == s).unwrap_or_else(|| panic!("{s} missing in {log:?}"));
        // Spec §4.7: stop processing, deactivate, disconnect, the controller lets go of the handler and terminates, then
        // the component terminates.
        assert!(at("comp.set_processing 0") < at("comp.set_active 0"));
        assert!(at("comp.set_active 0") < at("comp.disconnect").min(at("ctrl.disconnect")));
        assert!(at("ctrl.disconnect") < at("ctrl.set_component_handler false"));
        assert!(at("ctrl.set_component_handler false") < at("ctrl.terminate"));
        assert!(at("ctrl.terminate") < at("comp.terminate"));
        assert_eq!(probe.live.load(Ordering::Acquire), 0, "every plug-in object was released");
    }
}

#[cfg(windows)]
mod window {
    use super::*;
    use std::time::{Duration, Instant};
    use windows_sys::Win32::Foundation::{HWND, RECT};
    use windows_sys::Win32::Graphics::Gdi::*;
    use windows_sys::Win32::Storage::Xps::PrintWindow;
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    /// A host window (`show`: visible but off every screen and never activated, else hidden).
    fn host_window(show: bool) -> HWND {
        unsafe {
            let class = wide("OsVst3TestHost");
            let wc = WNDCLASSW {
                lpfnWndProc: Some(DefWindowProcW),
                hInstance: GetModuleHandleW(std::ptr::null()),
                lpszClassName: class.as_ptr(),
                hbrBackground: GetStockObject(BLACK_BRUSH) as HBRUSH,
                ..std::mem::zeroed()
            };
            RegisterClassW(&wc);
            let title = wide("os-vst3 test");
            let style = WS_POPUP | WS_CLIPCHILDREN;
            let hwnd = CreateWindowExW(WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE, class.as_ptr(), title.as_ptr(), style, -20000, -20000, 1200, 900, std::ptr::null_mut(), std::ptr::null_mut(), wc.hInstance, std::ptr::null());
            assert!(!hwnd.is_null());
            if show {
                ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            }
            hwnd
        }
    }

    fn pump(p: &mut dyn os_host::Plugin, ms: u64) {
        let end = Instant::now() + Duration::from_millis(ms);
        while Instant::now() < end {
            unsafe {
                let mut m: MSG = std::mem::zeroed();
                while PeekMessageW(&mut m, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                    TranslateMessage(&m);
                    DispatchMessageW(&m);
                }
            }
            p.idle();
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// PrintWindow of `hwnd`'s top-left `w` x `h` as BGRA rows (top-down).
    fn capture(hwnd: HWND, w: i32, h: i32) -> Vec<u8> {
        unsafe {
            let dc = GetDC(std::ptr::null_mut());
            let mem = CreateCompatibleDC(dc);
            let mut bmi: BITMAPINFO = std::mem::zeroed();
            bmi.bmiHeader = BITMAPINFOHEADER { biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32, biWidth: w, biHeight: -h, biPlanes: 1, biBitCount: 32, biCompression: BI_RGB, ..std::mem::zeroed() };
            let mut bits: *mut c_void = std::ptr::null_mut();
            let bmp = CreateDIBSection(dc, &bmi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
            let old = SelectObject(mem, bmp);
            PrintWindow(hwnd, mem, 2);
            let px = std::slice::from_raw_parts(bits as *const u8, (w * h * 4) as usize).to_vec();
            SelectObject(mem, old);
            DeleteObject(bmp);
            DeleteDC(mem);
            ReleaseDC(std::ptr::null_mut(), dc);
            px
        }
    }

    fn write_bmp(path: &Path, w: i32, h: i32, px: &[u8]) {
        let mut f = Vec::new();
        f.extend_from_slice(b"BM");
        f.extend_from_slice(&((54 + px.len()) as u32).to_le_bytes());
        f.extend_from_slice(&[0, 0, 0, 0]);
        f.extend_from_slice(&54u32.to_le_bytes());
        f.extend_from_slice(&40u32.to_le_bytes());
        f.extend_from_slice(&w.to_le_bytes());
        f.extend_from_slice(&(-h).to_le_bytes());
        f.extend_from_slice(&1u16.to_le_bytes());
        f.extend_from_slice(&32u16.to_le_bytes());
        f.extend_from_slice(&[0; 24]);
        f.extend_from_slice(px);
        std::fs::write(path, f).unwrap();
    }

    /// The editor attaches to a host window (never shown), reports its size, and detaches.
    #[test]
    fn editor_attaches_to_a_host_window() {
        let Some((_m, shared)) = sampler() else { return };
        let mut p = shared.lock().unwrap();
        assert!(p.has_editor());
        let hwnd = host_window(false);
        let (w, h) = p.open_editor(hwnd as *mut c_void, 1.0).expect("editor");
        assert!(w > 200 && h > 100, "{w} x {h}");
        assert!(p.editor_open());
        assert_eq!(p.editor_size(), Some((w, h)));
        assert!(!unsafe { GetWindow(hwnd, GW_CHILD) }.is_null(), "the editor is a child of the host window");
        pump(&mut *p, 100);
        p.close_editor();
        assert!(!p.editor_open() && p.editor_size().is_none());
        assert!(unsafe { GetWindow(hwnd, GW_CHILD) }.is_null(), "the editor window is gone");
        // A second open works (the view is created anew).
        p.open_editor(hwnd as *mut c_void, 1.0).expect("editor again");
        p.close_editor();
        unsafe { DestroyWindow(hwnd) };
    }

    /// Offscreen PrintWindow shots of the VST 3 editors in a host window: they paint (not one flat colour).
    /// `OS_SHOT_DIR=<dir>` keeps them as BMP files.
    #[test]
    #[ignore = "shows an offscreen window"]
    fn editor_shots() {
        for (name, setup) in [("Lemon Sampler", true), ("Lemon Parametric EQ 2", false)] {
            let Some(m) = module(name) else { continue };
            let shared = m.clone().create(&m.plugins()[0].id).unwrap();
            let mut p = shared.lock().unwrap();
            if setup {
                assert!(p.load_state(&sampler_state(&tone("os-vst3-shot.wav", 24000), "")));
            }
            assert!(p.activate(48000));
            let hwnd = host_window(true);
            let (w, h) = p.open_editor(hwnd as *mut c_void, 1.0).expect("editor");
            unsafe { SetWindowPos(hwnd, std::ptr::null_mut(), 0, 0, w as i32, h as i32, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE) };
            pump(&mut *p, 600);
            let mut rc: RECT = unsafe { std::mem::zeroed() };
            unsafe { GetClientRect(hwnd, &mut rc) };
            // The first capture of a window that never reached the screen makes it paint; the second sees the result.
            capture(hwnd, w as i32, h as i32);
            pump(&mut *p, 100);
            let px = capture(hwnd, w as i32, h as i32);
            let colours: std::collections::HashSet<[u8; 3]> = px.chunks_exact(4).map(|c| [c[0], c[1], c[2]]).collect();
            eprintln!("{name}: editor {w} x {h}, client {} x {}, {} colours", rc.right, rc.bottom, colours.len());
            if let Some(dir) = std::env::var_os("OS_SHOT_DIR") {
                write_bmp(&Path::new(&dir).join(format!("vst3-{}.bmp", name.replace(' ', "-"))), w as i32, h as i32, &px);
            }
            assert!(colours.len() > 20, "{name} paints its editor ({} colours)", colours.len());
            p.close_editor();
            unsafe { DestroyWindow(hwnd) };
        }
    }
}
