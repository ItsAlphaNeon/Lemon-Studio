// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

//! Renders the Lemon 3x Osc editor offscreen:
//! `cargo run -p lemon-3x-osc --example osc3_shot -- out.png [osc|env|demo] [x,y] [crop]`.
//! `env` shows the envelope page (volume); `demo` sets a few oscillator controls; `x,y` = mouse position (hover looks,
//! `OS_PRESS` presses); `crop` saves only the editor area the host shows (560 x 437).
use std::sync::Arc;

use lemon_3x_osc::editor::{Btn, Logic, Mods};
use lemon_3x_osc::params::Osc3Params;
use lemon_3x_osc::view;
use lemon_3x_osc::Shared;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let arg = |i: usize| args.get(i).map(String::as_str).filter(|a| !a.is_empty() && *a != "-");
    let out = arg(1).unwrap_or("lemon-3x-osc.png");
    let mut l = Logic::new(Arc::new(Osc3Params::default()), Arc::new(Shared::default()), None);
    match arg(2) {
        Some("env") => {
            l.view.tab = 1;
            l.vals.edit_plain("vol_env", 1.0);
        }
        Some("demo") => {
            l.vals.edit_plain("o1_shape", 3.0);
            l.vals.edit_plain("o2_shape", 2.0);
            l.vals.edit_plain("o3_shape", 1.0);
            l.vals.edit_plain("o2_fine", 7.0);
            l.vals.edit_plain("o2_detune", 12.0);
            l.vals.edit_plain("o1_phase", -20.0);
            l.vals.edit_plain("o3_pan", -32.0);
            l.vals.edit_plain("rand", 40.0);
        }
        _ => {}
    }
    let theme = std::env::var_os("OS_THEME")
        .and_then(|p| std::fs::read(p).ok())
        .map(|b| os_theme::fl::palette(&os_theme::FlTheme::parse(&String::from_utf8_lossy(&b))))
        .unwrap_or_default();
    // `OS_SHOT_SCALE=3` renders at that display scale (the picker's high-resolution tiles).
    let k: f32 = std::env::var("OS_SHOT_SCALE").ok().and_then(|v| v.parse().ok()).filter(|k: &f32| *k > 0.0).unwrap_or(1.0);
    let s = |v: u32| (v as f32 * k).round() as u32;
    let mut c = os_ui::Canvas::new(s(view::W), s(view::H), k).unwrap();
    let paint = |l: &mut Logic, c: &mut os_ui::Canvas| {
        let hint = l.hint.clone();
        l.hits = view::paint(c, &theme, &l.vals, &l.view, &hint, false);
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
    let (ox, oy, w, h) = if arg(4) == Some("crop") { (view::CROP_X, view::CROP_Y, view::EDITOR_W, view::EDITOR_H) } else { (0, 0, view::W, view::H) };
    let (ox, oy, w, h) = (s(ox), s(oy), s(w), s(h));
    let mut pm = tiny_skia::Pixmap::new(w, h).unwrap();
    for (i, px) in pm.pixels_mut().iter_mut().enumerate() {
        let (x, y) = (i as u32 % w + ox, i as u32 / w + oy);
        let v = buf[(y * s(view::W) + x) as usize];
        *px = tiny_skia::PremultipliedColorU8::from_rgba((v >> 16) as u8, (v >> 8) as u8, v as u8, 255).unwrap();
    }
    pm.save_png(out).unwrap();
}
