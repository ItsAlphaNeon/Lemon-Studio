// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

//! The two toolbar rows.
// spec: ui/main-window.md §1–3

use os_ui::{Font, Rect};

use super::{menus, sunken, tool_button, FrameInfo};
use crate::app::{Action, Live, State, Ui, Win};
use crate::icons::{self, Icon};

/// Leftmost logo centre: just right of the last toolbar control (the news panel / downloads, x 1473).
const LOGO_MIN_X: f32 = 1520.0;

fn btn(x: f32, y: f32) -> Rect {
    Rect::new(x, y, 30.0, 33.0)
}

pub fn paint(ui: &mut Ui, s: &State, f: &FrameInfo, width: f32) {
    let t = ui.t;
    let lemon = super::lemon(ui);
    // Bands.
    if lemon {
        // Lemon (spec: ui/lemon-design.md §4): flat bands, short rounded separators.
        ui.c.fill_rect(Rect::new(0.0, 0.0, width, 81.0), t.toolbar);
        ui.c.fill_rect(Rect::new(0.0, 41.0, width, 1.0), t.toolbar_line);
        ui.c.fill_rect(Rect::new(0.0, 81.0, width, 2.0), t.toolbar_line);
    } else {
        ui.c.fill_rect_gradient(Rect::new(0.0, 0.0, width, 41.0), t.toolbar.shade(0.025), t.toolbar.shade(-0.015), 0.0);
        ui.c.fill_rect(Rect::new(0.0, 41.0, width, 1.0), t.toolbar_line);
        ui.c.fill_rect_gradient(Rect::new(0.0, 42.0, width, 39.0), t.toolbar.shade(0.02), t.toolbar.shade(-0.02), 0.0);
        ui.c.fill_rect(Rect::new(0.0, 81.0, width, 2.0), t.toolbar_line);
    }
    // Empty toolbar areas drag the window (registered first, so controls win).
    ui.hit(Rect::new(0.0, 0.0, width, 81.0), Action::DragWindow, "");
    // Group separators (2 px gaps measured in row 1 and row 2).
    for x in [379.0, 575.0, 876.0, 1048.0, 1204.0] {
        if lemon {
            ui.c.fill_rrect(Rect::new(x, 9.0, 2.0, 23.0), 1.0, t.toolbar_line);
            continue;
        }
        ui.c.fill_rect(Rect::new(x, 2.0, 1.0, 37.0), t.toolbar_line);
        ui.c.fill_rect(Rect::new(x + 1.0, 2.0, 1.0, 37.0), t.toolbar_edge.with_alpha(110));
    }
    for x in [298.0, 378.0, 576.0, 967.0, 1298.0] {
        if lemon {
            ui.c.fill_rrect(Rect::new(x, 50.0, 2.0, 23.0), 1.0, t.toolbar_line);
            continue;
        }
        ui.c.fill_rect(Rect::new(x, 44.0, 1.0, 35.0), t.toolbar_line);
        ui.c.fill_rect(Rect::new(x + 1.0, 44.0, 1.0, 35.0), t.toolbar_edge.with_alpha(110));
    }

    row1(ui, s, f);
    row2(ui, s, f);

    // Logo spot (spans both rows): the Lemon Studio mark.
    // It keeps to the right edge until the window is too narrow, then the edge crops it
    // (spec ui/main-window.md §12); the toolbar never moves.
    let lx = (width - 45.0).max(LOGO_MIN_X);
    ui.hit(Rect::new(lx - 35.0, 0.0, 78.0, 80.0), Action::MenuItem(menus::HELP_MENU, menus::HELP_ABOUT), "About Lemon Studio");
    icons::logo(ui.c, lx, 40.0, 26.0, if lemon { t.toolbar_edge } else { t.toolbar_line });
}

fn row1(ui: &mut Ui, s: &State, f: &FrameInfo) {
    let t = ui.t;
    // Caption buttons.
    for (x, icon, action, hint) in [
        (4.0, Icon::Minimize, Action::Minimize, "Minimize"),
        (24.0, Icon::Restore, Action::Maximize, "Restore / maximize"),
        (44.0, Icon::Close, Action::Close, "Exit Lemon Studio"),
    ] {
        let r = Rect::new(x, 11.0, 19.0, 19.0);
        let hov = ui.hit(r, action, hint);
        super::button_face(ui, r, if hov { super::Look::Hover } else { super::Look::Off });
        icons::draw(ui.c, icon, r.inset(3.5), if hov { ui.t.tx(0xffffff) } else { t.glyph });
    }

    // Hint panel: project title + hint / message.
    let hp = Rect::span(72.0, 376.0, 4.0, 36.0);
    sunken(ui, hp, t.sunken);
    ui.hit(hp, Action::DragWindow, "Hint bar");
    ui.c.text_clip(Font::new(13), 76.0, 6.0, 280.0, t.text_dim.shade(-0.05), &f.title);
    let hint: &str = if !f.hint.is_empty() { &f.hint } else { s.message.as_deref().unwrap_or("") };
    ui.c.text_clip(Font::new(14), 77.0, 19.0, 280.0, t.text, hint);
    icons::draw(ui.c, Icon::MidiControl, Rect::span(360.0, 374.0, 17.0, 32.0), ui.t.s(0x515457));

    // Transport group.
    let lemon = super::lemon(ui);
    let capsule = Rect::span(383.0, 483.0, 4.0, 36.0);
    if lemon {
        // Lemon: one rounded-square tray, each section a rounded square inside it.
        os_ui::kit::lemon::outlined(ui.c, capsule, 7.0, t.toolbar_line.shade(-0.15), t.sunken);
    } else {
        ui.c.fill_rrect(capsule, 15.0, t.toolbar_line.shade(-0.2));
        ui.c.fill_rect_gradient(capsule.inset(1.0), t.sunken.shade(0.03), t.sunken.shade(-0.05), 14.0);
        ui.c.fill_rect(Rect::new(418.0, 6.0, 1.0, 29.0), t.toolbar_line);
        ui.c.fill_rect(Rect::new(449.0, 6.0, 1.0, 29.0), t.toolbar_line);
    }
    // PAT / SONG fill the whole top / bottom half of the capsule's left section.
    let pat = Rect::span(384.0, 418.0, 5.0, 20.0);
    let song = Rect::span(384.0, 418.0, 20.0, 35.0);
    ui.hit(pat, Action::PatSong(false), "Pattern mode: play the current pattern");
    ui.hit(song, Action::PatSong(true), "Song mode: play the playlist");
    for (r, label, active) in [(pat, "PAT", !s.song_mode), (song, "SONG", s.song_mode)] {
        if active && lemon {
            os_ui::kit::lemon::lit(ui.c, Rect::new(r.x + 2.0, r.y + 1.0, r.w - 3.0, r.h - 1.5), 4.0, false);
        } else if active {
            ui.c.push_clip(r);
            ui.c.fill_rrect(capsule.inset(1.0), 14.0, t.pill);
            ui.c.pop_clip();
        }
        ui.c.text_center(Font::condensed(10, 5), Rect::new(r.x + 1.0, r.y, r.w, r.h), if active { t.pill_text } else { t.glyph_dim }, label);
    }
    // Lit while playing, with a pause glyph (Play pauses); unlit with the play glyph when paused.
    let play_on = f.playing && !f.paused;
    for (r, icon, on, action, hint) in [
        (Rect::span(419.0, 449.0, 5.0, 35.0), Icon::Play, play_on, Action::Play, "Play / pause (Space plays / stops)"),
        (Rect::span(450.0, 482.0, 5.0, 35.0), Icon::Stop, false, Action::Stop, "Stop (Space)"),
    ] {
        let hov = ui.hit(r, action.clone(), hint);
        let on = on || ui.down(r, &action);
        // Lit / hovered sections fill their whole slot of the capsule (clipped to its round end).
        if (on || hov) && lemon {
            let seg = Rect::new(r.x + 1.0, r.y + 1.0, r.w - 2.0, r.h - 2.0);
            if on {
                os_ui::kit::lemon::lit(ui.c, seg, 5.0, hov);
            } else {
                ui.c.fill_rrect(seg, 5.0, t.button_hi);
            }
        } else if on || hov {
            ui.c.push_clip(r);
            if on {
                ui.c.fill_rect_gradient(capsule.inset(1.0), t.on_top, t.on_bottom, 14.0);
            } else {
                ui.c.fill_rrect(capsule.inset(1.0), 14.0, t.button_hi.with_alpha(110));
            }
            ui.c.pop_clip();
        }
        let g = Rect::new(r.x + 8.0, r.y + 8.0, r.w - 16.0, r.h - 16.0);
        let c = if on { t.on_glyph } else { t.glyph };
        if icon == Icon::Play && on {
            // Pause: two bars.
            let (w, h) = (g.w * 0.22, g.h * 0.7);
            let y = g.cy() - h / 2.0;
            ui.c.fill_rrect(Rect::new(g.cx() - w * 1.6, y, w, h), 1.0, c);
            ui.c.fill_rrect(Rect::new(g.cx() + w * 0.6, y, w, h), 1.0, c);
        } else {
            icons::draw(ui.c, icon, g, c);
        }
    }
    // Record: not available yet, so drawn greyed out (no hover, no press).
    let rec = Rect::span(486.0, 513.0, 7.0, 33.0);
    ui.off(rec, "Recording");
    if lemon {
        os_ui::kit::button(ui.c, t, Rect::new(rec.cx() - 14.0, rec.cy() - 14.0, 28.0, 28.0), os_ui::kit::State::NORMAL.with_disabled(true));
        ui.c.fill_circle(rec.cx(), rec.cy(), 5.6, t.record.mix(t.toolbar, 0.6));
    } else {
        ui.c.fill_circle(rec.cx(), rec.cy(), 14.0, t.toolbar_line.shade(-0.25));
        ui.c.fill_circle(rec.cx(), rec.cy(), 12.6, t.button.shade(-0.12));
        ui.c.fill_circle(rec.cx(), rec.cy(), 5.6, t.record.mix(t.button, 0.7).shade(-0.15));
    }
    // Tempo.
    let tb = Rect::span(517.0, 570.0, 6.0, 34.0);
    ui.hit(tb, Action::Tempo, "Tempo (drag up/down)");
    if lemon {
        os_ui::kit::lemon::well(ui.c, t, tb, t.time_panel);
    } else {
        ui.c.fill_rrect(tb, 5.0, ui.t.s(0x5b5f61));
        ui.c.fill_rrect(tb.inset(1.0), 4.0, ui.t.s(0x5f6466));
    }
    // Lemon displays show their digits in the brand colour.
    let digits = if lemon { t.pill } else { t.glyph };
    let whole = format!("{}", s.tempo.trunc() as i64);
    let frac = format!(".{:03}", ((s.tempo.fract()) * 1000.0).round() as i64 % 1000);
    let ww = ui.c.text_width(Font::condensed(19, 6), &whole);
    let fw = ui.c.text_width(Font::condensed(13, 5), &frac);
    let x0 = (tb.right() - 11.0 - ww - fw).max(tb.x + 2.0);
    ui.c.push_clip(tb.inset(1.0));
    ui.c.text(Font::condensed(19, 6), x0, 9.0, digits, &whole);
    ui.c.text(Font::condensed(13, 5), x0 + ww, 16.0, t.glyph_dim.shade(0.3), &frac);
    ui.c.pop_clip();
    ui.c.fill_polygon(&[(tb.right() - 9.0, 17.0), (tb.right() - 6.0, 13.0), (tb.right() - 3.0, 17.0)], t.glyph_dim);
    ui.c.fill_polygon(&[(tb.right() - 9.0, 22.0), (tb.right() - 6.0, 26.0), (tb.right() - 3.0, 22.0)], t.glyph_dim);

    // Recording options. The metronome works (spec: ui/metronome.md); the others wait for recording.
    tool_button(ui, btn(580.0, 4.0), Icon::Metronome, crate::metronome::on(), Action::Metronome, "Metronome (Ctrl+M) - right-click for the sound");
    for (i, (icon, hint)) in [
        (Icon::WaitInput, "Wait for input to start recording"),
        (Icon::Countdown, "Countdown before recording"),
        (Icon::Blend, "Blend recording"),
        (Icon::LoopRecord, "Loop recording"),
    ]
    .into_iter()
    .enumerate()
    {
        tool_button(ui, btn(613.0 + 33.0 * i as f32, 4.0), icon, false, Action::Placeholder(hint), hint);
    }

    // Song position (B:S:T).
    let tp = Rect::span(745.0, 873.0, 4.0, 36.0);
    if lemon {
        os_ui::kit::lemon::well(ui.c, t, tp, t.time_panel);
    } else {
        ui.c.fill_rect(tp, ui.t.s(0x212829));
        ui.c.fill_rect(tp.inset(1.0), t.time_panel);
    }
    ui.hit(tp, Action::Swallow, "Song position (bar:step:tick)");
    ui.live(Live::Clock, tp);
    let (bar, step, tick) = bst_ticks(f.tick * 96.0 / f64::from(s.project.ppq.max(1)));
    // Light digits drawn piece by piece (measured: big 36 px condensed to 14, small 24 to 9),
    // right-aligned so long bar numbers grow to the left and never leave the panel.
    let (big, small) = (Font::light(32, 13), Font::light(22, 8));
    let bar_s = bar.to_string();
    let step_s = format!("{step:02}");
    let tick_s = format!("{tick:02}");
    let (bw, sw, kw) = (ui.c.text_width(big, &bar_s), ui.c.text_width(big, &step_s), ui.c.text_width(small, &tick_s));
    let right = 863.0;
    let tick_x = right - kw;
    let step_x = tick_x - 8.0 - sw;
    let bar_x = (step_x - 8.0 - bw).max(tp.x + 2.0);
    ui.c.push_clip(tp.inset(1.0));
    let g = digits;
    ui.c.text(big, bar_x, -3.0, g, &bar_s);
    ui.c.fill_rect(Rect::new(step_x - 5.5, 20.0, 2.0, 2.5), g);
    ui.c.fill_rect(Rect::new(step_x - 5.5, 30.0, 2.0, 2.5), g);
    ui.c.text(big, step_x, -3.0, g, &step_s);
    ui.c.fill_rect(Rect::new(tick_x - 5.0, 21.0, 2.0, 2.0), g);
    ui.c.fill_rect(Rect::new(tick_x - 5.0, 29.5, 2.0, 2.0), g);
    ui.c.text(small, tick_x, 9.0, g, &tick_s);
    ui.c.pop_clip();
    ui.c.text(Font::condensed(7, 3).bold(), 852.0, 5.0, if lemon { t.text_dim } else { t.glyph.shade(-0.1) }, "B:S:T");

    // Monitors (oscilloscope / spectrum analyzer, stereo peak bars) and CPU / memory.
    monitor(ui, s);
    // CPU panel (spec: ui/main-window.md §13): audio load (bar, percent, recent history or the Misc counts), memory,
    // voices (or frames per second), dropouts. The hint is the reading in words (`App::tick`).
    let cpu = crate::app::CPU_RECT;
    sunken(ui, cpu, t.sunken_dark);
    ui.hit(cpu, Action::CpuPanel, "");
    let r = &s.cpu.now;
    let level = |load: u32| match load {
        0..75 => t.glyph,
        75..100 => t.meter_high,
        _ => t.meter_clip,
    };
    let k = (r.load as f32 / 100.0).min(1.0);
    ui.c.fill_rect(Rect::new(1055.0, 6.0, 1.5, 12.0), t.glyph.with_alpha(60));
    if k > 0.0 {
        ui.c.fill_cell(Rect::new(1055.0, 6.0 + 12.0 * (1.0 - k), 1.5, 12.0 * k), level(r.load));
    }
    ui.c.text_right(Font::new(17), 1136.0, 3.0, level(r.load), &format!("{}%", r.load));
    if let Some(m) = r.misc {
        // Misc: handles, threads, fonts in the history's place.
        ui.c.text(Font::new(13), 1058.0, 20.0, t.glyph, &format!("{} H  {} T  {} F", m.handles, m.threads, m.fonts));
    } else {
        // One 2 px column per reading, newest on the right, full height at 100 %.
        for (i, &h) in s.cpu.history.iter().rev().enumerate() {
            let hh = (11.0 * h as f32 / 100.0).min(11.0).round();
            if hh > 0.0 {
                ui.c.fill_cell(Rect::new(1128.0 - 2.0 * i as f32, 32.0 - hh, 2.0, hh), level(h).with_alpha(170));
            }
        }
        ui.c.line(1055.0, 32.0, 1130.0, 32.0, 1.0, t.glyph.with_alpha(160));
    }
    let mem = r.mem_mb.map_or("- MB".to_string(), |m| format!("{m} MB"));
    ui.c.text_right(Font::condensed(18, 6), 1199.0, 3.0, t.glyph, &mem);
    if s.cpu.fps_view {
        let unit = Font::new(13);
        let uw = ui.c.text_width(unit, "fps");
        ui.c.text_right(unit, 1166.0, 21.0, t.glyph, "fps");
        ui.c.text_right(Font::condensed(18, 6), 1166.0 - uw - 2.0, 17.0, t.glyph, &r.fps.to_string());
    } else {
        ui.c.text_right(Font::condensed(18, 6), 1166.0, 17.0, t.glyph, &r.voices.to_string());
    }
    if r.dropouts > 0 {
        ui.c.text_right(Font::condensed(18, 6), 1199.0, 17.0, t.meter_clip, &format!("!{}", r.dropouts.min(9999)));
    }

    // Utility buttons.
    let utils = [
        (Icon::Undo, "Undo history"),
        (Icon::Cut, "Edit tools"),
        (Icon::Mic, "Audio input / recording"),
        (Icon::Help, "Help"),
        (Icon::Save, "Save project (Ctrl+S)"),
        (Icon::SaveRender, "Export the song as a WAV file (Ctrl+R)"),
        (Icon::Info, "Project info"),
        (Icon::Download, "Downloads"),
    ];
    for (i, (icon, hint)) in utils.into_iter().enumerate() {
        let r = btn(1209.0 + 33.0 * i as f32, 4.0);
        let action = match icon {
            Icon::Save => Action::MenuItem(0, menus::FILE_SAVE),
            Icon::SaveRender => Action::MenuItem(0, menus::FILE_EXPORT),
            Icon::Info => Action::MenuItem(menus::OPTIONS_MENU, menus::OPT_INFO),
            _ => Action::Placeholder(hint),
        };
        tool_button(ui, r, icon, false, action, hint);
    }
}

/// The output monitor (spec ui/monitor.md): a scope / analyzer panel at x 882..968 and two peak bars at x 970..1047.
fn monitor(ui: &mut Ui, s: &State) {
    use crate::monitor::{Mode, COLS};
    let (edge, fill, ink) = (ui.t.s(0x1d2022), ui.t.s(0x25282a), ui.t.tx(0xb9bfc3));
    let m = &s.monitor;
    // Left panel: border, then 84 x 31 px of picture, bottom edge at y 36. Pictures are drawn as cells, which tile at
    // any scale (spec ui/ui-scaling.md §4).
    let lemon = super::lemon(ui);
    let (fill, ink) = if lemon { (ui.t.time_panel, ui.t.pill.mix(ui.t.time_panel, 0.15)) } else { (fill, ink) };
    if lemon {
        os_ui::kit::lemon::well(ui.c, ui.t, Rect::new(880.0, 4.0, 86.0, 33.0), fill);
    } else {
        ui.c.fill_rect(Rect::new(880.0, 4.0, 86.0, 33.0), edge);
        ui.c.fill_rect(Rect::new(881.0, 5.0, 84.0, 31.0), fill);
    }
    match m.mode {
        Mode::Spectrum => {
            for c in 0..COLS {
                let h = m.bar(c) * 31.0;
                let whole = h.floor();
                let x = 881.0 + c as f32;
                if whole > 0.0 {
                    ui.c.fill_cell(Rect::new(x, 36.0 - whole, 1.0, whole), ink);
                }
                let part = h - whole;
                if part > 0.05 && whole < 31.0 {
                    ui.c.fill_cell(Rect::new(x, 36.0 - whole - 1.0, 1.0, 1.0), ink.with_alpha((part * 255.0) as u8));
                }
            }
        }
        Mode::Scope => {
            for c in 0..COLS {
                let (lo, hi) = m.wave[c];
                let (y0, y1) = (20.0 - hi * 15.0, 20.0 - lo * 15.0);
                let (top, bottom) = (y0.min(y1).round(), y0.max(y1).round());
                ui.c.fill_cell(Rect::new(881.0 + c as f32, top, 1.0, (bottom - top).max(1.0)), ink);
            }
        }
    }
    ui.hit(Rect::new(880.0, 4.0, 86.0, 33.0), Action::Monitor, "Oscilloscope / spectrum analyzer (click to switch)");

    // Right panel: two peak bars (meter range .. 0 dB), light grey fading to cream at the tip, plus the held peak.
    if lemon {
        os_ui::kit::lemon::well(ui.c, ui.t, Rect::new(968.0, 4.0, 77.0, 23.0), fill);
    } else {
        ui.c.fill_rect(Rect::new(968.0, 4.0, 77.0, 23.0), edge);
        ui.c.fill_rect(Rect::new(969.0, 5.0, 75.0, 21.0), fill);
    }
    for i in 0..2 {
        let y = 5.0 + 11.0 * i as f32;
        let hold = (m.meter_pos(m.hold[i].0) * 75.0).round();
        if m.hold[i].1.is_some() && hold >= 1.0 {
            ui.c.fill_rect(Rect::new(969.0 + hold - 1.0, y, 1.0, 10.0), ui.t.h(0xcdc7a8));
        }
        let len = (m.meter_pos(m.meter_db[i]) * 75.0).round();
        if len < 1.0 {
            continue;
        }
        let (a, b) = if lemon { ((0xc9, 0xa9, 0x2c), (0xff, 0xf1, 0xb8)) } else { ((0xc8, 0xce, 0xd1), (0xcd, 0xc7, 0xa8)) };
        let mix = |k: f32, shade: f32| {
            let l = |u: i32, v: i32| (((u as f32 + (v - u) as f32 * k) * shade).round() as u32).min(255);
            os_ui::Color::rgb((l(a.0, b.0) << 16) | (l(a.1, b.1) << 8) | l(a.2, b.2))
        };
        for x in 0..len as i32 {
            let k = x as f32 / len.max(1.0);
            // Tip column darker, left column slightly dark; top and bottom rows shaded like a raised bar.
            let side = if x == len as i32 - 1 { 0.58 } else if x == 0 { 0.82 } else { 1.0 };
            let px = 969.0 + x as f32;
            ui.c.fill_cell(Rect::new(px, y, 1.0, 1.0), mix(k, 0.82 * side));
            ui.c.fill_cell(Rect::new(px, y + 1.0, 1.0, 8.0), mix(k, side));
            ui.c.fill_cell(Rect::new(px, y + 9.0, 1.0, 1.0), mix(k, 0.75 * side));
        }
    }
    ui.hit(Rect::new(968.0, 4.0, 77.0, 23.0), Action::Swallow, "Output level (peak L / R)");
    ui.c.stroke_circle(1037.0, 33.5, 4.0, 2.0, ui.t.s(0x66737d));
}

fn row2(ui: &mut Ui, s: &State, f: &FrameInfo) {
    let t = ui.t;
    // Menu bar.
    let mb = Rect::span(4.0, 296.0, 45.0, 77.0);
    let lemon = super::lemon(ui);
    if lemon {
        os_ui::kit::lemon::panel(ui.c, mb, 6.0, t.menu_panel);
    } else {
        ui.c.fill_rect_gradient(mb, t.menu_panel.shade(-0.04), t.menu_panel.shade(0.02), 2.5);
        ui.c.stroke_rrect(mb, 2.5, 1.0, t.toolbar_line.shade(-0.1));
    }
    let layout = menus::bar_layout(ui.c);
    for (i, (m, (x0, x1, width))) in menus::MENUS.iter().zip(layout).enumerate() {
        let title = m.title;
        let r = Rect::new(x0 - 3.0, 47.0, x1 - x0 + 6.0, 29.0);
        let hov = ui.hit(r, Action::Menu(i), title);
        let open = s.menu_open == Some(i);
        if lemon && open {
            os_ui::kit::lemon::lit(ui.c, r.inset(1.0), 4.0, false);
        } else if hov || open {
            ui.c.fill_rrect(if lemon { r.inset(1.0) } else { r }, if lemon { 4.0 } else { 2.0 }, t.menu_drop_hi);
        }
        ui.c.text(Font::condensed(menus::BAR_FONT.size, width as u16), x0, 51.0, if lemon && open { t.pill_text } else { t.menu_text }, title);
    }

    // Main volume and main pitch (project values; spec: audio/mixer.md "Main volume", audio/voice-levels.md).
    use crate::knobs::KnobId;
    ui.hit(Rect::span(303.0, 337.0, 45.0, 77.0), Action::Knob(KnobId::MasterVol), s.knob_text(KnobId::MasterVol));
    super::knob(ui, 319.0, 61.0, 14.0, s.knob(KnobId::MasterVol), false, t.glyph, t.button_hi);
    ui.hit(Rect::span(340.0, 376.0, 45.0, 77.0), Action::Knob(KnobId::MasterPitch), s.knob_text(KnobId::MasterPitch));
    super::knob(ui, 358.0, 61.0, 14.0, s.knob(KnobId::MasterPitch), true, t.glyph, t.button_hi);
    for k in 0..20 {
        let a = 0.75 * std::f32::consts::PI + 1.5 * std::f32::consts::PI * k as f32 / 19.0;
        ui.c.fill_circle(358.0 + 17.5 * a.cos(), 61.0 + 17.5 * a.sin(), 0.7, t.glyph_dim);
    }

    // Song position slider (spec: ui/song-position-slider.md): the handle follows the playhead while playing.
    let sp = crate::song_pos::PANEL;
    let range = crate::song_pos::range(s);
    let pos = crate::song_pos::shown(s, f.playing, f.song, f.tick, f.song_pos_drag);
    ui.live(Live::Clock, sp);
    match range {
        Some(_) => {
            ui.hit(sp, Action::SongPos, crate::song_pos::hint(s, pos));
        }
        // An empty playlist in SONG mode: nothing to move through.
        None => {
            ui.hit(sp, Action::Swallow, "Song position: the playlist is empty");
        }
    }
    let hx = range.map(|len| crate::song_pos::handle_x(pos, len).round());
    if lemon {
        ui.c.fill_rrect(Rect::new(390.0, 60.0, 180.0, 4.0), 2.0, t.sunken_dark);
        match hx {
            Some(hx) => {
                use os_theme::design::lemon as brand;
                ui.c.fill_rrect(Rect::new(390.0, 60.0, hx - 390.0, 4.0), 2.0, brand::MAIN);
                ui.c.fill_rrect(Rect::new(hx - 6.0, 55.0, 12.0, 14.0), 3.0, t.button);
                ui.c.fill_rect(Rect::new(hx - 1.0, 58.0, 2.0, 8.0), brand::MAIN);
            }
            None => ui.c.fill_rect(Rect::new(394.0, 58.0, 2.0, 8.0), t.button),
        }
    } else {
        // Classic: the measured colours (§1).
        ui.c.fill_rect(Rect::new(394.0, 60.0, 172.0, 4.0), ui.t.s(0x343739));
        match hx {
            Some(hx) => {
                let green = ui.t.a(0xa8e44a);
                ui.c.fill_rect(Rect::new(394.0, 60.0, hx - 394.0, 4.0), green);
                ui.c.fill_circle(hx + 0.5, 62.0, 6.5, ui.t.s(0x1c1f21));
                ui.c.fill_circle(hx + 0.5, 62.0, 5.5, ui.t.s(0x26292b));
                ui.c.fill_rect(Rect::new(hx, 56.0, 1.0, 2.0), ui.t.s(0x9a9d9f));
                ui.c.fill_rect(Rect::new(hx, 58.0, 1.0, 7.0), green);
                ui.c.fill_rect(Rect::new(hx, 65.0, 1.0, 3.0), ui.t.s(0x686b6d));
            }
            None => ui.c.fill_rect(Rect::new(395.0, 56.0, 1.0, 12.0), ui.t.s(0x282b2d)),
        }
    }

    // Mode buttons (spec: ui/main-window.md §3). Typing keyboard, auto scrolling (spec: ui/auto-scrolling.md) and enable
    // groups (spec: ui/enable-groups.md) work.
    for (i, (icon, hint)) in [
        (Icon::TypingKeyboard, "Typing keyboard to piano keyboard (Ctrl+T)"),
        (Icon::AutoScroll, "Auto scrolling - right-click for continuous scrolling"),
        (Icon::StepEdit, "Step editing (Ctrl+E)"),
        (Icon::Link, "Enable groups (Shift+Alt+G)"),
        (Icon::MidiControl, "Multilink to controllers (Ctrl+J)"),
    ]
    .into_iter()
    .enumerate()
    {
        let (on, action) = match i {
            0 => (s.toggles.contains(&"typing"), Action::Typing),
            1 => (crate::autoscroll::on(), Action::AutoScroll),
            3 => (crate::enable_groups::on(), Action::EnableGroups),
            _ => (false, Action::Placeholder(hint)),
        };
        tool_button(ui, btn(580.0 + 33.0 * i as f32, 45.0), icon, on, action, hint);
    }

    // Snap.
    icons::draw(ui.c, Icon::Magnet, Rect::span(748.0, 766.0, 52.0, 70.0), t.glyph_dim.shade(0.3));
    let snap = Rect::span(768.0, 830.0, 50.0, 72.0);
    ui.off(snap, "Global snap (each editor has its own snap)");
    if lemon {
        os_ui::kit::lemon::well(ui.c, t, snap, t.sunken);
    } else {
        ui.c.fill_rrect(snap, 2.0, t.sunken_dark);
    }
    ui.c.text(Font::new(16), snap.x + 5.0, 51.0, t.glyph, "Line");
    icons::draw(ui.c, Icon::ArrowRight, Rect::new(snap.right() - 14.0, 55.0, 12.0, 12.0), t.glyph_dim);
    ui.veil(Rect::span(746.0, 831.0, 50.0, 72.0), t.toolbar);

    // Pattern selector.
    let prev = Rect::span(838.0, 851.0, 47.0, 76.0);
    ui.hit(prev, Action::PatternPrev, "Pattern options"); // spec: ui/pattern-menu.md §1
    ui.c.fill_rrect(prev, if lemon { 4.0 } else { 2.0 }, t.button);
    icons::draw(ui.c, Icon::ArrowRight, prev.inset(2.0), t.glyph);
    let pb = Rect::span(855.0, 948.0, 47.0, 76.0);
    let hov = ui.hit(pb, Action::PatternNext, crate::patterns::name_of(s, s.pattern.max(1)));
    if lemon {
        os_ui::kit::lemon::well(ui.c, t, pb, if hov { t.sunken.mix(t.button_hi, 0.5) } else { t.sunken });
    } else {
        ui.c.fill_rrect(pb, 2.0, t.tempo_box.shade(if hov { 0.16 } else { 0.08 }));
    }
    let name = s
        .project
        .patterns
        .iter()
        .find(|p| p.number == s.pattern)
        .and_then(|p| p.name.clone())
        .unwrap_or_else(|| format!("Pattern {}", s.pattern));
    ui.c.push_clip(pb.inset(2.0));
    ui.c.text_right(Font::condensed(21, 7), pb.right() - 11.0, 49.0, t.glyph, &name);
    ui.c.pop_clip();
    let add = Rect::span(951.0, 962.0, 50.0, 72.0);
    ui.hit(add, Action::PatternAdd, "New pattern (find first / next empty)");
    icons::draw(ui.c, Icon::Plus, add, t.glyph);

    // Window buttons.
    let wins = [
        (Icon::Playlist, Action::ToggleWin(Win::Playlist), s.shown(Win::Playlist), "Playlist (F5)"),
        (Icon::PianoRoll, Action::ToggleWin(Win::PianoRoll), s.shown(Win::PianoRoll), "Piano roll (F7)"),
        (Icon::ChannelRack, Action::ToggleWin(Win::ChannelRack), s.shown(Win::ChannelRack), "Channel rack (F6)"),
        (Icon::Mixer, Action::ToggleWin(Win::Mixer), s.shown(Win::Mixer), "Mixer (F9)"),
        (Icon::Browser, Action::ToggleWin(Win::Browser), s.show_browser, "Browser (Alt+F8)"),
        (Icon::Project, Action::Placeholder("Project picker"), false, "Project picker"),
        (Icon::Plug, Action::PickerOpen, false, "Add a plugin (F8)"),
        (Icon::Kick, Action::ToggleWin(Win::Tapper), s.shown(Win::Tapper), "Tempo tapper (Ctrl+F5)"),
        (Icon::Touch, Action::Placeholder("Touch controller"), false, "Touch controller"),
        (Icon::Store, Action::Placeholder("Plugin store"), false, "Plugin store"),
    ];
    for (i, (icon, action, open, hint)) in wins.into_iter().enumerate() {
        let r = btn(973.0 + 33.0 * i as f32, 45.0);
        if matches!(action, Action::Placeholder(_)) {
            super::tool_button(ui, r, icon, false, action, hint);
            continue;
        }
        let hov = ui.hit(r, action.clone(), hint) || ui.down(r, &action);
        super::button_face(ui, r, if hov { super::Look::Hot } else { super::Look::Off });
        let _ = open;
        icons::draw(ui.c, icon, Rect::new(r.x + 5.0, r.y + 6.5, 20.0, 20.0), if hov { t.on_glyph } else { t.glyph });
    }

    // News panel.
    let np = Rect::span(1304.0, 1470.0, 45.0, 77.0);
    {
        ui.c.fill_rrect(np, if lemon { 6.0 } else { 2.0 }, t.news_panel);
        ui.off(np, "News");
        let tw = ui.c.text(Font::new(13), 1309.0, 53.0, t.text_faint, "Today");
        let news = "Welcome to Lemon Studio";
        let avail = 1442.0 - 1315.0 - tw;
        let nf = ui.c.fit_font(Font::new(14), news, avail);
        ui.c.text_clip(nf, 1315.0 + tw, 52.0, avail, ui.t.tx(0x9aa8b0), news);
        icons::draw(ui.c, Icon::Globe, Rect::new(1449.0, 52.0, 18.0, 18.0), t.glyph_dim.shade(0.3));
        ui.veil(np, t.toolbar);
    }
}

/// Bar:step:tick (1-based bar and step, 16 steps per bar, `ppq / 4` ticks per step).
#[cfg(test)]
pub fn bst(seconds: f64, tempo: f64, ppq: u32) -> (u32, u32, u32) {
    bst_ticks(seconds * tempo / 60.0 * f64::from(ppq))
}

/// Bar:step:tick of a position given in ticks of a 96-per-quarter clock (the display's own resolution).
pub fn bst_ticks(ticks: f64) -> (u32, u32, u32) {
    let ticks = ticks.max(0.0) as u32;
    let per_step = 96 / 4;
    let steps = ticks / per_step;
    (steps / 16 + 1, steps % 16 + 1, ticks % per_step)
}

#[cfg(test)]
mod tests {
    use super::bst;

    #[test]
    fn bar_step_tick() {
        assert_eq!(bst(0.0, 120.0, 96), (1, 1, 0));
        // 120 BPM: one bar (4 beats) = 2 s.
        assert_eq!(bst(2.0, 120.0, 96), (2, 1, 0));
        assert_eq!(bst(0.125, 120.0, 96), (1, 2, 0));
    }
}
