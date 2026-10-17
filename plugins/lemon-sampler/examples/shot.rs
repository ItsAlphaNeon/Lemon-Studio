// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Renders the Lemon Sampler editor offscreen:
//! `cargo run -p lemon-sampler --example shot -- out.png [tab] [sample.wav|""] [full|hosted|-] [sub] [fx-page] [x,y] [spectrum]`.
//! `tab` 0 = sample page, 1 = envelope page; `sub` = envelope target (0 Panning .. 4 Pitch); `fx-page` 0 tools / 1 effects;
//! `x,y` = mouse position (hover looks, painted-frame coordinates).
use lemon_sampler::editor::Vals;
use lemon_sampler::params::SamplerParams;
use lemon_sampler::view::{self, SampleView, ViewState};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let arg = |i: usize| args.get(i).map(String::as_str).filter(|a| !a.is_empty() && *a != "-");
    let out = arg(1).unwrap_or("lemon-sampler.png");
    let tab: u8 = arg(2).and_then(|t| t.parse().ok()).unwrap_or(0);
    let path = arg(3).unwrap_or("");
    let sample = arg(3).map(|p| os_engine::sample::load(std::path::Path::new(p)).expect("sample"));
    let bits = arg(3).map_or(16, |p| lemon_sampler::source_bits(std::path::Path::new(p))) as u16;
    let mode = arg(4).unwrap_or("");
    let sub: u8 = arg(5).and_then(|t| t.parse().ok()).unwrap_or(1);
    let fx_page: u8 = arg(6).and_then(|t| t.parse().ok()).unwrap_or(0);
    let mouse = arg(7).and_then(|m| m.split_once(',')).and_then(|(x, y)| Some((x.parse().ok()?, y.parse().ok()?)));
    let spectrum = arg(8) == Some("spectrum");
    let params = SamplerParams::default();
    let vals = Vals::new(&params);
    let hosted = mode == "hosted";
    // `OS_SHOT_SCALE=3` renders at that display scale (the picker's high-resolution tiles).
    let k: f32 = std::env::var("OS_SHOT_SCALE").ok().and_then(|v| v.parse().ok()).filter(|k: &f32| *k > 0.0).unwrap_or(1.0);
    let s = |v: u32| (v as f32 * k).round() as u32;
    let mut c = os_ui::Canvas::new(s(view::W), s(view::H), k).unwrap();
    let st = ViewState { tab, sub, fx_page, mouse, spectrum, ..ViewState::default() };
    let spec = match (&sample, spectrum) {
        (Some(s), true) => {
            let a = view::wave_area();
            let (w, h) = ((a.w as usize / 2).max(1), (a.h as usize / 2).max(1));
            Some((w, h, view::spectrogram(s, 0.0, s.frames() as f64, w, h)))
        }
        _ => None,
    };
    let sv = SampleView { sample: sample.as_ref(), path, bits, spectrum: spec.as_ref().map(|(w, h, p)| (*w, *h, p.as_slice())), tones: None };
    let theme = os_plugkit::palette();
    view::paint(&mut c, &theme, &vals, &st, &sv, "", hosted);
    if std::env::var_os("OS_BENCH").is_some() {
        // Full-frame paint time (what a knob drag repaints).
        let t = std::time::Instant::now();
        for _ in 0..50 {
            view::paint(&mut c, &theme, &vals, &st, &sv, "", hosted);
        }
        eprintln!("paint {:.2} ms", t.elapsed().as_secs_f64() * 1000.0 / 50.0);
    }
    let mut buf = vec![0u32; (s(view::W) * s(view::H)) as usize];
    c.copy_to_xrgb(&mut buf);
    // The editor the host shows (cropped); `full` paints the whole painted area.
    let (ox, oy, w, h) = if mode == "full" {
        (0, 0, view::W, view::H)
    } else if hosted {
        (view::CROP_X, view::HOSTED_CROP_Y, view::EDITOR_W, view::HOSTED_EDITOR_H)
    } else {
        (view::CROP_X, view::CROP_Y, view::EDITOR_W, view::EDITOR_H)
    };
    let (ox, oy, w, h) = (s(ox), s(oy), s(w), s(h));
    let mut pm = tiny_skia::Pixmap::new(w, h).unwrap();
    for (i, px) in pm.pixels_mut().iter_mut().enumerate() {
        let (x, y) = (i as u32 % w + ox, i as u32 / w + oy);
        let v = buf[(y * s(view::W) + x) as usize];
        *px = tiny_skia::PremultipliedColorU8::from_rgba((v >> 16) as u8, (v >> 8) as u8, v as u8, 255).unwrap();
    }
    pm.save_png(out).unwrap();
}
