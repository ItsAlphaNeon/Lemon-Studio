// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Tests host the test plugins of [`crate::mock`]. `OS_VST2_PLUGIN=<plugin file>` adds any installed plugin to
//! `hosts_a_plugin_from_env`.

use super::mock::{self, probe, GAIN_ID, SYNTH_ID};
use super::*;
use os_engine::{HostedEffect as _, Instrument as _};
use os_host::{Library as _, ParamEvent, Plugin as _};

fn module(main: Main, name: &str) -> Arc<Module> {
    unsafe { Module::from_main(Path::new(&format!("/plugins/{name}")), main, None) }.expect("module")
}

fn gain() -> (Arc<Module>, Plugin) {
    let m = module(mock::gain_main, "Mock Gain.dll");
    let p = Plugin::new(&m, &id_text(GAIN_ID)).expect("create");
    (m, p)
}

fn synth() -> (Arc<Module>, Plugin) {
    let m = module(mock::synth_main, "Mock Synth.dll");
    let p = Plugin::new(&m, &id_text(SYNTH_ID)).expect("create");
    (m, p)
}

fn logged(prefix: &str) -> Vec<String> {
    probe(|p| p.log.iter().filter(|l| l.starts_with(prefix)).cloned().collect())
}

fn clear() {
    probe(|p| *p = mock::Probe { live: p.live, ..mock::Probe::default() });
}

#[test]
fn a_plugin_file_lists_its_plugin_and_hands_the_describing_instance_on() {
    let m = module(mock::gain_main, "Mock Gain.dll");
    let d = &m.plugins()[0];
    assert_eq!(m.plugins().len(), 1);
    assert_eq!((d.id.as_str(), d.name.as_str(), d.vendor.as_str(), d.version.as_str()), ("4D6B476E", "Mock Gain", "Mock Audio", "1.2.3"));
    assert_eq!((d.format, d.kind), (os_host::Format::Vst2, os_host::Kind::Effect));
    assert_eq!(d.features, ["Fx"]);
    assert_eq!(d.uid(), "vst2:4D6B476E");
    // The instance that described it waits for the first instance asked for; later ones are new.
    assert_eq!(probe(|p| p.live), 1);
    let a = m.clone().create(&d.id).expect("create");
    assert_eq!(probe(|p| p.live), 1);
    let b = m.clone().create(&d.id).expect("create");
    assert_eq!(probe(|p| p.live), 2);
    drop((a, b, m));
    assert_eq!(probe(|p| p.live), 0, "every instance is closed");
    // Before the plugin handed out its effect, the host answered from the instance being made.
    assert!(logged("main rate").iter().all(|l| l == "main rate 44100"));
    let broken = unsafe { Module::from_main(Path::new("/plugins/Broken.dll"), mock::broken_main, None) };
    assert!(broken.is_err_and(|e| e.contains("did not start")));
}

#[test]
fn a_shell_lists_its_plugins_and_creates_each_by_id() {
    let m = module(mock::shell_main, "Mock Shell.dll");
    let list: Vec<_> = m.plugins().iter().map(|d| (d.id.clone(), d.name.clone(), d.kind)).collect();
    assert_eq!(
        list,
        [(id_text(GAIN_ID), "Shell Gain".to_string(), os_host::Kind::Effect), (id_text(SYNTH_ID), "Shell Synth".to_string(), os_host::Kind::Instrument)]
    );
    assert_eq!(probe(|p| p.live), 0, "a shell keeps no instance");
    clear();
    let p = Plugin::new(&m, &id_text(SYNTH_ID)).expect("create");
    assert_eq!(logged("shell main"), ["shell main 2400 4D6B5379"], "the shell is asked for the plugin by id");
    assert!(p.desc().is_instrument());
    drop(p);
    assert_eq!(probe(|p| p.live), 0);
}

#[test]
fn activation_sets_the_plugin_up_then_resumes_it_and_teardown_reverses_it() {
    let (m, mut p) = gain();
    clear();
    assert!(p.activate(48000));
    assert_eq!(probe(|p| p.log.clone()), ["rate 48000", "block 4096", "resume 1"]);
    assert_eq!(os_host::Plugin::latency(&p), 64, "the latency a resumed plugin reports");
    let (mut l, mut r) = (vec![0.5; 64], vec![0.5; 64]);
    p.process_fx(48000, &mut l, &mut r);
    p.process_fx(48000, &mut l, &mut r);
    assert_eq!(logged("begin").len(), 1, "processing starts once");
    // Another rate: suspended, set up again, resumed.
    clear();
    assert!(p.activate(44100));
    assert_eq!(probe(|p| p.log.clone()), ["end", "resume 0", "rate 44100", "block 4096", "resume 1"]);
    clear();
    drop(p);
    assert_eq!(probe(|p| p.log.clone()), ["resume 0", "close"]);
    drop(m);
    assert_eq!(probe(|p| p.live), 0);
}

#[test]
fn an_inactive_instance_asks_for_its_rate_and_stays_quiet() {
    let (_m, mut p) = synth();
    let (mut l, mut r) = (vec![1.0; 32], vec![1.0; 32]);
    p.process(48000, &mut l, &mut r);
    assert!(l.iter().chain(&r).all(|v| *v == 0.0));
    assert_eq!(probe(|p| p.blocks.len()), 0);
    p.idle();
    assert_eq!(p.rate, 48000, "the next idle activates it at the stream's rate");
    // An effect passes its input through until then.
    let (_m, mut g) = gain();
    let (mut l, mut r) = (vec![0.25; 32], vec![0.25; 32]);
    g.process_fx(48000, &mut l, &mut r);
    assert!(l.iter().all(|v| *v == 0.25));
}

#[test]
fn effects_process_their_input_and_host_edits_are_not_reported_as_the_plugins() {
    let (_m, mut p) = gain();
    p.activate(48000);
    assert!(p.set_param(0, 0.25));
    assert!(!p.set_param(9, 0.5));
    let (mut l, mut r): (Vec<f32>, Vec<f32>) = ((0..100).map(|i| i as f32).collect(), (0..100).map(|i| -(i as f32)).collect());
    p.process_fx(48000, &mut l, &mut r);
    assert_eq!(l[10], 5.0);
    assert_eq!(r[10], -5.0);
    assert!(p.take_param_events().is_empty(), "the plugin's echo of a host edit is no edit");
}

#[test]
fn notes_reach_the_plugin_at_their_frames() {
    let (_m, mut p) = synth();
    p.activate(48000);
    p.note_on(10, 60, 1.0);
    let (mut l, mut r) = (vec![0.0; 64], vec![0.0; 64]);
    p.process(48000, &mut l, &mut r);
    assert_eq!(probe(|p| p.midi.clone()), [(10, 0x90, 60, 127)]);
    assert!(l[..10].iter().all(|v| *v == 0.0) && l[10..].iter().all(|v| *v == 0.5), "the note sounds from its frame on");
    assert_eq!(r, l);
    assert_eq!(p.voices(), 1);
    assert!(p.active());
    p.note_off(5, 60);
    p.process(48000, &mut l, &mut r);
    assert_eq!(probe(|p| p.midi[1]), (5, 0x80, 60, 64));
    assert!(l[..5].iter().all(|v| *v == 0.5) && l[5..].iter().all(|v| *v == 0.0));
    // Velocity in 1..=127 (never a note on with velocity 0, which would end the note).
    p.note_on(0, 61, 0.001);
    p.note_on(0, 62, 0.5);
    p.process(48000, &mut l, &mut r);
    assert_eq!(probe(|p| p.midi[2..].iter().map(|m| m.3).collect::<Vec<_>>()), [1, 64]);
}

/// Glides (spec: audio/channel-voices.md): VST 2 has no tuning of a note, so a key's tuning is a pitch bend over the
/// wrapper's range, centred again before the next note; a stop centres it too.
#[test]
fn glides_are_pitch_bend() {
    let (_m, mut p) = synth();
    p.activate(48000);
    p.set_bend_range(2);
    p.note_on(0, 60, 1.0);
    p.tune(4, 60, 1.0);
    p.tune(6, 60, 1.0);
    p.tune(8, 60, -2.0);
    p.note_on(12, 64, 1.0);
    let (mut l, mut r) = (vec![0.0; 16], vec![0.0; 16]);
    p.process(48000, &mut l, &mut r);
    assert_eq!(probe(|p| p.midi.clone()), [(0, 0x90, 60, 127), (4, 0xE0, 0, 0x60), (8, 0xE0, 0, 0), (12, 0xE0, 0, 0x40), (12, 0x90, 64, 127)]);
    p.tune(0, 64, 2.0);
    p.process(48000, &mut l, &mut r);
    p.reset();
    p.process(48000, &mut l, &mut r);
    assert!(probe(|p| p.midi.contains(&(0, 0xE0, 0, 0x40))), "a stop centres the bend");
}

#[test]
fn a_stop_ends_held_notes_and_all_sound() {
    let (_m, mut p) = synth();
    p.activate(48000);
    p.note_on(0, 60, 1.0);
    let (mut l, mut r) = (vec![0.0; 16], vec![0.0; 16]);
    p.process(48000, &mut l, &mut r);
    p.note_on(8, 64, 1.0);
    clear();
    p.reset();
    assert_eq!(logged("end"), ["end"], "processing stops");
    assert_eq!(p.voices(), 0);
    p.process(48000, &mut l, &mut r);
    // The queued note is dropped; the held one is ended; all sound and all notes off.
    assert_eq!(probe(|p| p.midi.clone()), [(0, 0x80, 60, 0), (0, 0xB0, 120, 0), (0, 0xB0, 123, 0)]);
    assert!(l.iter().all(|v| *v == 0.0));
    assert_eq!(logged("begin"), ["begin"], "and starts anew");
}

#[test]
fn automation_splits_blocks_where_values_change() {
    let (_m, mut p) = gain();
    p.activate(48000);
    p.param_fx(0, 0, 0.25);
    p.param_fx(100, 0, 0.5);
    // Closer to the last change than the shortest piece: it waits for that piece's end.
    p.param_fx(110, 0, 1.0);
    p.param_fx(5, 9, 1.0);
    clear();
    let (mut l, mut r) = (vec![1.0; 256], vec![1.0; 256]);
    p.process_fx(48000, &mut l, &mut r);
    assert_eq!(probe(|p| p.blocks.clone()), [100, 32, 124]);
    assert_eq!(logged("set"), ["set 0 0.25", "set 0 0.5", "set 0 1"], "an unknown parameter is ignored");
    assert_eq!((l[0], l[99], l[100], l[131], l[132], l[255]), (0.5, 0.5, 1.0, 1.0, 2.0, 2.0));
    // Long blocks are split at the block size.
    clear();
    let (mut l, mut r) = (vec![1.0; 5000], vec![1.0; 5000]);
    p.process_fx(48000, &mut l, &mut r);
    assert_eq!(probe(|p| p.blocks.clone()), [4096, 904]);
}

#[test]
fn the_transport_reaches_the_plugin() {
    use abi::time::*;
    let (_m, mut p) = synth();
    p.activate(48000);
    let (mut l, mut r) = (vec![0.0; 64], vec![0.0; 64]);
    p.process(48000, &mut l, &mut r);
    let (flags, tempo, _, _) = probe(|p| p.times[0]);
    assert_eq!(flags & (PLAYING | TEMPO_VALID), 0, "no transport yet");
    assert_eq!(tempo, 120.0);
    let song = os_engine::TransportInfo {
        playing: true,
        tempo: 140.0,
        frame: 96000,
        beats: 4.0,
        bar_start: 4.0,
        bar: 1,
        numerator: 3,
        denominator: 4,
        looping: Some((0.0, 8.0)),
    };
    p.transport(&song);
    p.process(48000, &mut l, &mut r);
    let (flags, tempo, ppq, frames) = probe(|p| p.times[1]);
    let want = PLAYING | TRANSPORT_CHANGED | TEMPO_VALID | PPQ_POS_VALID | BARS_VALID | TIME_SIG_VALID | CYCLE_ACTIVE | CYCLE_POS_VALID;
    assert_eq!(flags, want);
    assert_eq!((tempo, ppq, frames), (140.0, 4.0, 96000.0));
    let t = p.inst.shared.time();
    assert_eq!((t.time_sig_numerator, t.time_sig_denominator, t.cycle_end_pos, t.bar_start_pos), (3, 4, 8.0, 4.0));
    // Pieces of one block see the position at their own start.
    p.transport(&song);
    p.param(32, 0, 0.9);
    p.process(48000, &mut l, &mut r);
    let at = probe(|p| p.times[3]);
    assert_eq!(at.3, 96032.0);
    assert!((at.2 - (4.0 + 32.0 / (48000.0 * 60.0 / 140.0))).abs() < 1e-9);
    // Without a transport the song is stopped where it was, its tempo still known.
    p.process(48000, &mut l, &mut r);
    let (flags, tempo, ..) = probe(|p| *p.times.last().unwrap());
    assert_eq!(flags & (PLAYING | CYCLE_ACTIVE), 0);
    assert_ne!(flags & (TRANSPORT_CHANGED | TEMPO_VALID), 0);
    assert_eq!(tempo, 140.0);
}

/// After a Stop, and after an export (which the app ends with the same reset, its last block still playing), the first
/// block of the next Play reports a transport change; what plugins read in between says the song is stopped.
#[test]
fn the_first_block_after_a_stop_or_an_export_reports_a_transport_change() {
    use abi::time::*;
    let (_m, mut p) = synth();
    p.activate(48000);
    let song =
        os_engine::TransportInfo { playing: true, tempo: 120.0, frame: 0, beats: 0.0, bar_start: 0.0, bar: 0, numerator: 4, denominator: 4, looping: None };
    let (mut l, mut r) = (vec![0.0; 64], vec![0.0; 64]);
    let mut block = |p: &mut Plugin, song: Option<&os_engine::TransportInfo>| {
        if let Some(s) = song {
            p.transport(s);
        }
        p.process(48000, &mut l, &mut r);
        probe(|q| q.times.last().unwrap().0) & (PLAYING | TRANSPORT_CHANGED)
    };
    // An export renders offline and ends playing; the app then resets every instrument.
    p.set_offline(true);
    assert_eq!(block(&mut p, Some(&song)), PLAYING | TRANSPORT_CHANGED);
    assert_eq!(block(&mut p, Some(&song)), PLAYING, "nothing changed");
    p.reset();
    p.set_offline(false);
    assert_eq!(p.inst.shared.time().flags & (PLAYING | TRANSPORT_CHANGED), 0, "stopped at once, for plugins that read it in between");
    // The live Play after it.
    assert_eq!(block(&mut p, Some(&song)), PLAYING | TRANSPORT_CHANGED);
    assert_eq!(block(&mut p, Some(&song)), PLAYING);
    // A Stop: a block without the song (stopped) reports the change once, and so does the next Play.
    p.reset();
    assert_eq!(block(&mut p, None), TRANSPORT_CHANGED);
    assert_eq!(block(&mut p, None), 0);
    assert_eq!(block(&mut p, Some(&song)), PLAYING | TRANSPORT_CHANGED);
    // A Stop straight into a Play.
    p.reset();
    assert_eq!(block(&mut p, Some(&song)), PLAYING | TRANSPORT_CHANGED);
}

#[test]
fn states_round_trip_as_chunks_or_parameter_lists() {
    let (m, mut p) = gain();
    p.set_param(0, 0.75);
    let saved = p.save_state().expect("state");
    assert_eq!(&saved[..5], &[0xF7, 0xFF, 0xFF, 0xFF, 12], "FL Studio's header");
    let mut q = Plugin::new(&m, &id_text(GAIN_ID)).expect("create");
    assert!(q.load_state(&saved));
    assert_eq!(q.param_value(0), Some(0.75));
    assert!(q.take_param_events().is_empty(), "restoring is no edit");
    assert!(!q.load_state(b"not a state"));
    // A plugin without chunks: its parameters, in FL Studio's list (which keeps no program).
    let (sm, mut s) = synth();
    s.set_param(0, 0.3);
    s.inst.dispatch(op::SET_PROGRAM, 0, 2, std::ptr::null_mut(), 0.0);
    let saved = s.save_state().expect("state");
    assert_eq!(&saved[..5], &[0xF7, 0xFF, 0xFF, 0xFF, 4], "FL Studio's header for a parameter list");
    let mut t = Plugin::new(&sm, &id_text(SYNTH_ID)).expect("create");
    clear();
    assert!(t.load_state(&saved));
    assert_eq!(t.param_value(0), Some(f64::from(0.3f32)));
    assert!(logged("program").is_empty());
    // Lemon Studio's first list (kind 13, `LSVP`) still restores the program too.
    let mut first = vec![0xF7, 0xFF, 0xFF, 0xFF, 13, 0xFE, 0xFF, 0xFF, 0xFF, 16, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    first.extend(b"LSVP");
    first.extend(2i32.to_le_bytes());
    first.extend(1u32.to_le_bytes());
    first.extend(0.6f32.to_le_bytes());
    clear();
    assert!(t.load_state(&first));
    assert_eq!(t.param_value(0), Some(f64::from(0.6f32)));
    assert_eq!(logged("program"), ["program 2"]);
    // Another plugin's chunk is not taken as a parameter list.
    assert!(!t.load_state(&p.save_state().unwrap()));
}

#[test]
fn parameters_describe_themselves() {
    let (_m, p) = gain();
    assert_eq!(p.param_count(), 2);
    let g = p.param_info(0).unwrap();
    assert_eq!((g.id, g.name.as_str(), g.unit.as_str(), g.default, g.steps, g.group.as_str()), (0, "Gain", "dB", 0.5, 0, ""));
    let mode = p.param_info(1).unwrap();
    assert_eq!((mode.name.as_str(), mode.steps, mode.group.as_str()), ("Mode", 1, "Main"), "a switch, from its properties");
    assert!(p.param_info(2).is_none());
    assert_eq!(p.param_text(0, 0.5).as_deref(), Some("1.00 dB"));
    assert_eq!(p.param_text(0, 0.9), None, "only the current value has a text");
}

#[test]
fn the_editor_opens_in_its_parent_reports_edits_and_follows_resizes() {
    let (_m, mut p) = gain();
    assert!(p.has_editor());
    clear();
    let size = p.open_editor(0x1234 as *mut c_void, 1.0).expect("editor");
    assert_eq!(size, (300, 200));
    assert_eq!(logged("editor open"), ["editor open 4660"]);
    // Outside macOS the scale is told even at 1: a plugin keeps the last one it was told.
    if cfg!(target_os = "macos") {
        assert!(logged("scale").is_empty());
    } else {
        assert_eq!(logged("scale"), ["scale 1"]);
    }
    assert!(p.editor_open());
    unsafe { mock::edit(p.inst.effect, 0, 0.6) };
    assert_eq!(p.take_param_events(), [ParamEvent::Begin(0), ParamEvent::Value(0, 0.6f32 as f64), ParamEvent::End(0)]);
    unsafe { mock::ask_size(p.inst.effect, 500, 400) };
    assert_eq!(p.editor_size(), Some((500, 400)));
    p.idle();
    assert_eq!(probe(|p| p.idles), 1, "the editor gets its idle calls");
    p.close_editor();
    assert_eq!(logged("editor close").len(), 1);
    assert_eq!(p.editor_size(), None);
    // At twice the density: macOS editors measure in points; elsewhere the plugin is told the scale.
    clear();
    let size = p.open_editor(0x1234 as *mut c_void, 2.0).expect("editor");
    if cfg!(target_os = "macos") {
        assert_eq!(size, (600, 400));
        assert!(logged("scale").is_empty());
    } else {
        assert_eq!(size, (300, 200));
        assert_eq!(logged("scale"), ["scale 2"]);
    }
    let (_sm, s) = synth();
    assert!(!s.has_editor());
}

#[test]
fn a_change_of_outputs_and_latency_is_taken_on_idle() {
    let (_m, mut p) = gain();
    p.activate(48000);
    unsafe { mock::go_mono(p.inst.effect, 128) };
    clear();
    p.idle();
    assert_eq!(probe(|p| p.log.clone()), ["resume 0", "rate 48000", "block 4096", "resume 1"]);
    assert_eq!((p.outs, os_host::Plugin::latency(&p)), (1, 128));
    let (mut l, mut r) = (vec![0.5; 32], vec![0.25; 32]);
    p.process_fx(48000, &mut l, &mut r);
    // One output, heard on both sides.
    assert_eq!((l[0], r[0]), (0.5, 0.5));
}

#[test]
fn the_host_answers_what_plugins_ask() {
    let (_m, p) = gain();
    let e = p.inst.effect;
    let ask = |op: i32, s: &std::ffi::CStr| unsafe { mock::ask(e, op, s.as_ptr() as *mut c_void) };
    assert_eq!(ask(abi::host_op::SUPPORTS, c"sendVstTimeInfo"), 1);
    assert_eq!(ask(abi::host_op::SUPPORTS, c"sizeWindow"), 1);
    assert_eq!(ask(abi::host_op::SUPPORTS, c"offline"), 0);
    let mut buf = [0u8; 64];
    unsafe { mock::ask(e, abi::host_op::VENDOR_NAME, buf.as_mut_ptr().cast()) };
    assert_eq!(text(&buf), "Lemon Studio");
    assert_eq!(unsafe { mock::ask(e, abi::host_op::VERSION, std::ptr::null_mut()) }, 2400);
    assert_eq!(unsafe { mock::ask(e, abi::host_op::GET_BLOCK_SIZE, std::ptr::null_mut()) }, 1024, "the usual default before activation");
    if !cfg!(target_os = "macos") {
        let dir = unsafe { mock::ask(e, abi::host_op::DIRECTORY, std::ptr::null_mut()) } as *const std::ffi::c_char;
        assert_eq!(unsafe { std::ffi::CStr::from_ptr(dir) }.to_str().unwrap(), "/plugins");
    }
    assert_eq!(unsafe { host::callback(std::ptr::null_mut(), abi::host_op::VERSION, 0, 0, std::ptr::null_mut(), 0.0) }, 2400);
    assert_eq!(unsafe { host::callback(std::ptr::null_mut(), abi::host_op::GET_TIME, 0, 0, std::ptr::null_mut(), 0.0) }, 0, "nothing to answer from");
}

#[test]
fn versions_and_kinds() {
    assert_eq!(version_text(0x01_02_03, 1), "1.2.3");
    // Observed pairs: each DPF plugin's VST 2 vendor version and the version its VST3 copy reports.
    for (vendor, vst3) in [(256, "0.1.0"), (0x1_0000, "1.0.0"), (0x1_0100, "1.1.0"), (0x3_020A, "3.2.10")] {
        assert_eq!(version_text(vendor, vendor), vst3);
    }
    assert_eq!(version_text(0, 0x100), "0.1.0", "the effect's own field when the vendor gives none");
    assert_eq!(version_text(0, 0), "");
    // Older plugins' decimal versions (observed: Dimension Expander, VST Speek), and hex ones in the same range.
    assert_eq!(version_text(1005, 1005), "1.0.0.5");
    assert_eq!(version_text(1000, 1000), "1.0.0.0");
    for (v, hex) in [(0x0900, "0.9.0"), (0x0414, "0.4.20"), (0x2710 - 1, "0.39.15")] {
        assert_eq!(version_text(v, v), hex);
    }
    assert_eq!(id_text(abi::fourcc(*b"4FTP")), "34465450");
    assert_eq!(id_text(-1), "FFFFFFFF");
    assert_eq!(kind_of(flags::INSTRUMENT, category::UNKNOWN, 2), os_host::Kind::Instrument);
    assert_eq!(kind_of(0, category::INSTRUMENT, 2), os_host::Kind::Instrument);
    assert_eq!(kind_of(0, category::METERING, 0), os_host::Kind::Other, "nothing to hear");
    assert_eq!(kind_of(0, category::OFFLINE, 2), os_host::Kind::Other);
    assert_eq!(features_of(os_host::Kind::Effect, category::DELAY_OR_ECHO), ["Fx", "Reverb"]);
}

/// `OS_VST2_PLUGIN=<plugin file>`: lists it, creates its first plugin, plays a block and saves / restores its state.
#[test]
fn hosts_a_plugin_from_env() {
    let Some(path) = std::env::var_os("OS_VST2_PLUGIN").map(PathBuf::from) else { return };
    let m = Module::load(&path).expect("load");
    for d in m.plugins() {
        eprintln!("{} {:?} {} by {} ({}) {:?}", d.id, d.kind, d.name, d.vendor, d.version, d.features);
    }
    let d = m.plugins().first().expect("a plugin").clone();
    let mut p = Plugin::new(&m, &d.id).expect("create");
    assert!(p.activate(48000));
    let (mut l, mut r) = (vec![0.0; 512], vec![0.0; 512]);
    if d.is_instrument() {
        p.note_on(0, 60, 0.8);
        p.process(48000, &mut l, &mut r);
        p.note_off(0, 60);
    }
    p.process(48000, &mut l, &mut r);
    eprintln!("{} parameters, latency {}, editor {}", p.param_count(), os_host::Plugin::latency(&p), p.has_editor());
    if let Some(s) = p.save_state() {
        eprintln!("state: {} bytes", s.len());
        assert!(p.load_state(&s));
    }
}
