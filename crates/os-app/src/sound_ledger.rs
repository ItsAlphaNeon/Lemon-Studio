// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Always-on sound-regression ledger.
//!
//! Renders a compact set of self-contained musical scenarios through the real `os-engine` and linked stock plugins
//! (no external files or environment variables required) on every `cargo test`, verifying:
//! 1. Finite, non-silent audio within expected headroom (no NaN/Inf, no runaway blowup, no silent collapse).
//! 2. Stereo RMS dBFS, Peak dBFS, and spectral balance within tight cross-platform tolerances (±0.08 dB),
//!    catching accidental DSP, gain-law, pan-law, envelope, or routing regressions immediately.
//! 3. Bit-for-bit repeatability on repeated renders and bit-for-bit parity between single-threaded and multi-threaded
//!    (`os_engine::parallel::Pool`) engine runs.
//!
//! When an intentional sound change is made, run with `OS_LEDGER_PRINT=1 cargo test -p os-app sound_ledger -- --nocapture`
//! to inspect the new measured figures and update the recorded ledger entry with a dated reason.

#![cfg(all(
    feature = "stock-trio",
    feature = "stock-citrus",
    feature = "stock-reverb",
    feature = "stock-delay",
    feature = "stock-limiter",
    feature = "stock-parametric-eq-2",
    feature = "stock-soft-clipper",
))]

use os_engine::{Engine, PlayMode, SampleLibrary};
use os_model::{Channel, ChannelKind, MixerTrack, Note, Pattern, Project, SlotPlugin};

const RATE: u32 = 48_000;
const FRAMES: usize = 24_000; // 0.5 s at 48 kHz — fast enough for always-on CI
const BLOCK: usize = 256;
const TOL_DB: f64 = 0.08;

/// Measured audio fingerprint of a rendered stereo buffer.
#[derive(Debug, Clone, Copy)]
struct Metrics {
    rms_l_db: f64,
    rms_r_db: f64,
    peak_l_db: f64,
    peak_r_db: f64,
    hf_ratio_db: f64,
}

impl Metrics {
    fn of(stereo: &[f32]) -> Self {
        let frames = stereo.len() / 2;
        let (mut sum_l, mut sum_r) = (0.0f64, 0.0f64);
        let (mut peak_l, mut peak_r) = (0.0f64, 0.0f64);
        let (mut diff_e, mut tot_e) = (0.0f64, 0.0f64);
        let (mut prev_l, mut prev_r) = (0.0f64, 0.0f64);
        for s in stereo.chunks_exact(2) {
            let (l, r) = (f64::from(s[0]), f64::from(s[1]));
            assert!(l.is_finite() && r.is_finite(), "non-finite sample in render");
            sum_l += l * l;
            sum_r += r * r;
            peak_l = peak_l.max(l.abs());
            peak_r = peak_r.max(r.abs());
            let dl = l - prev_l;
            let dr = r - prev_r;
            diff_e += dl * dl + dr * dr;
            tot_e += l * l + r * r;
            prev_l = l;
            prev_r = r;
        }
        let db = |v: f64| if v > 1e-12 { 20.0 * v.log10() } else { -240.0 };
        let rms_l = (sum_l / frames.max(1) as f64).sqrt();
        let rms_r = (sum_r / frames.max(1) as f64).sqrt();
        // High-frequency energy ratio (first-difference energy vs total energy): catches filter/brightness shifts.
        let hf_ratio = if tot_e > 1e-18 { (diff_e / tot_e).sqrt() } else { 0.0 };
        Metrics {
            rms_l_db: db(rms_l),
            rms_r_db: db(rms_r),
            peak_l_db: db(peak_l),
            peak_r_db: db(peak_r),
            hf_ratio_db: db(hf_ratio),
        }
    }
}

/// Recorded golden baseline for one scenario.
struct Entry {
    name: &'static str,
    rms_l_db: f64,
    rms_r_db: f64,
    peak_l_db: f64,
    peak_r_db: f64,
    hf_ratio_db: f64,
}

fn base_project() -> Project {
    let mut p = Project::default();
    p.ppq = 96;
    p.tempo = Some(120.0);
    p.mixer = (0..=4)
        .map(|i| {
            let mut m = MixerTrack::new(i);
            m.name = Some(if i == 0 { "Master".into() } else { format!("Insert {i}") });
            m.enabled = true;
            m.slots_enabled = true;
            m.volume = Some(0.8);
            m.routes = if i == 0 { Vec::new() } else { vec![0] };
            m
        })
        .collect();
    p.patterns.push(Pattern { number: 1, name: Some("Ledger".into()), ..Pattern::default() });
    p
}

fn plugin_channel(index: u16, name: &str, mixer_track: u16, pan: f64) -> Channel {
    let mut c = crate::projects::test_project().channels[0].clone();
    c.index = index;
    c.kind = ChannelKind::Plugin;
    c.name = Some(name.into());
    c.plugin = Some(name.into());
    c.sample_path = None;
    c.enabled = true;
    c.mixer_track = mixer_track;
    c.volume = 0.78;
    c.pan = pan;
    c
}

fn note(ch: u16, key: u16, pos: u32, len: u32, vel: u8) -> Note {
    Note { key, position: pos, length: len, velocity: vel, ..Note::step(ch, pos) }
}

fn render_with(p: &Project, plugins: &crate::plugins::Plugins, threads: Option<usize>) -> Vec<f32> {
    let mut lib = SampleLibrary::default();
    let insts = plugins.instruments();
    let mut e = Engine::with_instruments(p, &mut lib, RATE, PlayMode::Pattern(1), &insts);
    e.set_looping(false);
    for (track, slot, fx) in plugins.effects() {
        e.set_effect(track, slot, fx);
    }
    if let Some(n) = threads {
        e.set_threads(Some(os_engine::parallel::Pool::new(n, None)));
    }
    let mut out = vec![0.0f32; FRAMES * 2];
    for chunk in out.chunks_mut(BLOCK * 2) {
        e.process(chunk);
    }
    out
}

fn setup_plugins() -> crate::plugins::Plugins {
    let mut plugins = crate::plugins::Plugins::default();
    plugins.available = crate::plugins::builtin();
    plugins.effects = crate::plugins::builtin_effects();
    plugins
}

fn host_inst(plugins: &mut crate::plugins::Plugins, row: usize, name: &str) {
    let idx = plugins.index_of(name).unwrap_or_else(|| panic!("missing stock instrument {name}"));
    plugins.create_with(row, idx, None).unwrap_or_else(|e| panic!("host {name}: {e}"));
}

fn host_fx(p: &mut Project, plugins: &mut crate::plugins::Plugins, track: u16, slot: u8, name: &str) {
    let idx = plugins.effects.iter().position(|a| a.desc.name == name).unwrap_or_else(|| panic!("missing stock effect {name}"));
    p.mixer[usize::from(track)].slots[usize::from(slot)] = Some(SlotPlugin { id: Default::default(), name: name.into(), state: Vec::new() });
    plugins.create_effect(track, slot, idx, None).unwrap_or_else(|e| panic!("host fx {name}: {e}"));
}

fn add_send(p: &mut Project, from: usize, to: u16, level: f64) {
    p.mixer[from].routes.push(to);
    p.mixer[from].send_levels.push((to, level));
}

fn scenario_trio_chord() -> (Project, crate::plugins::Plugins) {
    let mut p = base_project();
    let mut plugins = setup_plugins();
    p.channels.push(plugin_channel(0, crate::osc3::LEMON_TRIO, 1, 0.0));
    p.patterns[0].notes = vec![
        note(0, 48, 0, 144, 100),
        note(0, 60, 0, 144, 96),
        note(0, 64, 24, 120, 92),
        note(0, 67, 48, 96, 88),
    ];
    host_inst(&mut plugins, 0, crate::osc3::LEMON_TRIO);
    (p, plugins)
}

fn scenario_citrus_fx_chain() -> (Project, crate::plugins::Plugins) {
    let mut p = base_project();
    let mut plugins = setup_plugins();
    p.channels.push(plugin_channel(0, crate::citrus::CITRUS, 1, 0.0));
    p.patterns[0].notes = vec![
        note(0, 60, 0, 96, 105),
        note(0, 67, 48, 96, 95),
        note(0, 72, 96, 96, 90),
    ];
    host_inst(&mut plugins, 0, crate::citrus::CITRUS);
    host_fx(&mut p, &mut plugins, 1, 0, crate::plugins::LEMON_EQ);
    host_fx(&mut p, &mut plugins, 1, 1, "Lemon Reverb");
    host_fx(&mut p, &mut plugins, 1, 2, "Lemon Delay");
    host_fx(&mut p, &mut plugins, 1, 3, "Lemon Soft Clipper");
    host_fx(&mut p, &mut plugins, 0, 0, "Lemon Limiter");
    (p, plugins)
}

fn scenario_sends_and_panning() -> (Project, crate::plugins::Plugins) {
    let mut p = base_project();
    let mut plugins = setup_plugins();
    // Hard-left and hard-right channels on separate inserts, both sending to a shared reverb bus on insert 3.
    p.channels.push(plugin_channel(0, crate::osc3::LEMON_TRIO, 1, -1.0));
    p.channels.push(plugin_channel(1, crate::osc3::LEMON_TRIO, 2, 1.0));
    add_send(&mut p, 1, 3, 0.6);
    add_send(&mut p, 2, 3, 0.6);
    p.patterns[0].notes = vec![
        note(0, 60, 0, 96, 100),
        note(1, 67, 48, 96, 100),
    ];
    host_inst(&mut plugins, 0, crate::osc3::LEMON_TRIO);
    host_inst(&mut plugins, 1, crate::osc3::LEMON_TRIO);
    host_fx(&mut p, &mut plugins, 3, 0, "Lemon Reverb");
    (p, plugins)
}

/// Golden sound ledger (recorded 2026-11-27 on main). Update only with an explicit dated reason.
const LEDGER: &[Entry] = &[
    Entry { name: "trio_chord", rms_l_db: -13.091, rms_r_db: -13.091, peak_l_db: -3.408, peak_r_db: -3.408, hf_ratio_db: -27.370 },
    Entry { name: "citrus_fx_chain", rms_l_db: -6.123, rms_r_db: -5.262, peak_l_db: -0.230, peak_r_db: -0.220, hf_ratio_db: -28.433 },
    Entry { name: "sends_and_panning", rms_l_db: -11.985, rms_r_db: -15.088, peak_l_db: -5.433, peak_r_db: -5.271, hf_ratio_db: -24.729 },
];

fn check_entry(entry: &Entry, m: Metrics) {
    if std::env::var_os("OS_LEDGER_PRINT").is_some() {
        eprintln!(
            "Entry {{ name: {:?}, rms_l_db: {:.3}, rms_r_db: {:.3}, peak_l_db: {:.3}, peak_r_db: {:.3}, hf_ratio_db: {:.3} }},",
            entry.name, m.rms_l_db, m.rms_r_db, m.peak_l_db, m.peak_r_db, m.hf_ratio_db
        );
        return;
    }
    assert!(m.rms_l_db > -80.0 && m.rms_r_db > -80.0, "{}: unexpected silence ({m:?})", entry.name);
    assert!(m.peak_l_db < 6.0 && m.peak_r_db < 6.0, "{}: unexpected blowup ({m:?})", entry.name);
    let chk = |field: &str, got: f64, want: f64| {
        let diff = got - want;
        assert!(
            diff.abs() <= TOL_DB,
            "sound regression in `{}` ({field}): got {got:.3} dB, ledger {want:.3} dB (delta {diff:+.3} dB, tol ±{TOL_DB} dB)",
            entry.name
        );
    };
    chk("rms_l", m.rms_l_db, entry.rms_l_db);
    chk("rms_r", m.rms_r_db, entry.rms_r_db);
    chk("peak_l", m.peak_l_db, entry.peak_l_db);
    chk("peak_r", m.peak_r_db, entry.peak_r_db);
    chk("hf_ratio", m.hf_ratio_db, entry.hf_ratio_db);
}

#[test]
fn sound_regression_ledger() {
    let cases: [(&str, fn() -> (Project, crate::plugins::Plugins)); 3] = [
        ("trio_chord", scenario_trio_chord),
        ("citrus_fx_chain", scenario_citrus_fx_chain),
        ("sends_and_panning", scenario_sends_and_panning),
    ];
    for (entry, (name, build)) in LEDGER.iter().zip(cases) {
        assert_eq!(entry.name, name);
        let (p, plugins) = build();
        let a = render_with(&p, &plugins, None);
        let m = Metrics::of(&a);
        check_entry(entry, m);
        // Same scenario built fresh must render bit-for-bit identically.
        let (p2, plugins2) = build();
        let b = render_with(&p2, &plugins2, None);
        assert_eq!(a, b, "{name}: render is not deterministic across runs");
    }
}

#[test]
fn sound_ledger_multicore_matches_single_core_bit_for_bit() {
    let build = || {
        let mut p = base_project();
        let mut plugins = setup_plugins();
        p.channels.push(plugin_channel(0, crate::osc3::LEMON_TRIO, 1, -0.5));
        p.channels.push(plugin_channel(1, crate::citrus::CITRUS, 2, 0.5));
        p.channels.push(plugin_channel(2, crate::osc3::LEMON_TRIO, 3, 0.0));
        add_send(&mut p, 1, 4, 0.5);
        add_send(&mut p, 2, 4, 0.5);
        p.patterns[0].notes = vec![
            note(0, 48, 0, 144, 100),
            note(1, 64, 24, 120, 96),
            note(2, 67, 48, 96, 92),
        ];
        host_inst(&mut plugins, 0, crate::osc3::LEMON_TRIO);
        host_inst(&mut plugins, 1, crate::citrus::CITRUS);
        host_inst(&mut plugins, 2, crate::osc3::LEMON_TRIO);
        host_fx(&mut p, &mut plugins, 4, 0, "Lemon Reverb");
        host_fx(&mut p, &mut plugins, 0, 0, "Lemon Limiter");
        (p, plugins)
    };
    let (p1, pl1) = build();
    let single = render_with(&p1, &pl1, None);
    let (p2, pl2) = build();
    let multi = render_with(&p2, &pl2, Some(3));
    assert!(Metrics::of(&single).rms_l_db > -60.0, "mix is audible");
    assert_eq!(single, multi, "multi-core render diverged from single-core render");
}
