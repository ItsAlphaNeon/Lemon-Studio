// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Renders the Lemon Parametric EQ editor offscreen:
//! `cargo run -p lemon-parametric-eq-2 --example eq_shot -- out.png [default|demo] [x,y] [spectrum|heatmap]`.
//! `demo` moves a few bands; `x,y` = mouse position (hover looks); `spectrum` analyses pink-ish noise as monitored input.
use std::sync::Arc;

use lemon_parametric_eq_2::analyzer::{Analyzer, Look as ALook};
use lemon_parametric_eq_2::editor::{Btn, Logic, Mods};
use lemon_parametric_eq_2::params::{self, freq_id, level_id, order_id, type_id, width_id, EqParams};
use lemon_parametric_eq_2::view::{self, Look};
use lemon_parametric_eq_2::Shared;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let arg = |i: usize| args.get(i).map(String::as_str).filter(|a| !a.is_empty() && *a != "-");
    let out = arg(1).unwrap_or("lemon-parametric-eq-2.png");
    let mut l = Logic::new(Arc::new(EqParams::default()), Arc::new(Shared::default()), None);
    if arg(2) == Some("demo") {
        l.vals.edit_plain(&level_id(0), 4.5);
        l.vals.edit_plain(&level_id(2), -7.0);
        l.vals.edit_plain(&width_id(2), 0.3);
        l.vals.edit_plain(&level_id(4), 6.0);
        l.vals.edit_plain(&type_id(6), 7.0);
        l.vals.edit_plain(&order_id(6), 1.0);
        l.vals.edit_plain(&freq_id(6), 0.93);
    }
    let mode = arg(4).unwrap_or("");
    let mut spectrum = None;
    if mode == "spectrum" || mode == "heatmap" {
        l.params.edit_extra(|b| {
            params::set_byte(b, params::MONITOR_BYTE, 1);
            params::set_flag(b, params::F_HISTOGRAM, true);
            params::set_flag(b, params::F_HEATMAP, mode == "heatmap");
        });
        // Pink-ish noise: white noise through a one-pole low-pass sum.
        let n = 8192;
        let (mut seed, mut b0, mut b1) = (12345u32, 0.0f32, 0.0f32);
        let s: Vec<f32> = (0..n)
            .map(|_| {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                let w = (seed >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0;
                b0 = 0.997 * b0 + 0.03 * w;
                b1 = 0.6 * b1 + 0.2 * w;
                (b0 + b1 + 0.05 * w) * 0.5
            })
            .collect();
        let mut a = Analyzer::default();
        a.update(&s, 44100.0, view::COLS, &ALook { precision: 2, slope: 2, smoothing: 0, average: false });
        spectrum = Some(a.cols);
    }
    // `OS_SHOT_SCALE=3` renders at that display scale (the picker's high-resolution tiles).
    let k: f32 = std::env::var("OS_SHOT_SCALE").ok().and_then(|v| v.parse().ok()).filter(|k: &f32| *k > 0.0).unwrap_or(1.0);
    let s = |v: u32| (v as f32 * k).round() as u32;
    let mut c = os_ui::Canvas::new(s(view::W), s(view::H), k).unwrap();
    let paint = |l: &mut Logic, c: &mut os_ui::Canvas| {
        let extra = l.params.extra_copy();
        let look = Look { s: l.shown(), extra: &extra, spectrum: spectrum.as_deref(), rate: 44100.0 };
        l.hits = view::paint(c, &look, &l.view);
    };
    paint(&mut l, &mut c);
    if let Some((x, y)) = arg(3).and_then(|m| m.split_once(',')).and_then(|(x, y)| Some((x.parse().ok()?, y.parse().ok()?))) {
        l.moved(x, y, Mods::default());
        if std::env::var_os("OS_PRESS").is_some() {
            l.down(x, y, Btn::Left, Mods::default(), false);
        }
        paint(&mut l, &mut c);
        eprintln!("hint: {}", l.hint);
    }
    if std::env::var_os("OS_BENCH").is_some() {
        let t = std::time::Instant::now();
        for _ in 0..50 {
            paint(&mut l, &mut c);
        }
        eprintln!("paint {:.2} ms", t.elapsed().as_secs_f64() * 1000.0 / 50.0);
    }
    let mut buf = vec![0u32; (s(view::W) * s(view::H)) as usize];
    c.copy_to_xrgb(&mut buf);
    let mut pm = tiny_skia::Pixmap::new(s(view::W), s(view::H)).unwrap();
    for (px, v) in pm.pixels_mut().iter_mut().zip(&buf) {
        *px = tiny_skia::PremultipliedColorU8::from_rgba((v >> 16) as u8, (v >> 8) as u8, *v as u8, 255).unwrap();
    }
    pm.save_png(out).unwrap();
}
