// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The "Import MIDI data" form (`crate::midi_file::MidiForm`), painted in the dialogs' frame: Source (the tracks combo
//! and the sixteen MIDI channels as toggles) over Options (the channel type, FILE's form only, and the lamps), then
//! Accept. A combo's list is drawn over everything, as the Export form does.
// spec: ui/midi-files.md §2, §10.2

use os_ui::{kit, Font, Rect};

use super::dialog::{button, frame, hit, place, section, BTN_H, BTN_W};
use super::export_form::list;
use super::project_forms::{check, label};
use crate::app::{Action, State, Ui};
use crate::dialog::{DHit, Modal};
use crate::midi_file::{self as mf, MidiForm};
use crate::project_forms::FHit;

pub const W: f32 = 440.0;
const TOP: f32 = 34.0;
const GAP: f32 = 6.0;
const SOURCE_H: f32 = 148.0;
/// Options' height: FILE's form has the channel type row over the lamps.
fn options_h(piano: bool) -> f32 {
    if piano { 106.0 } else { 140.0 }
}

/// The form's height.
pub fn height(piano: bool) -> f32 {
    TOP + SOURCE_H + GAP + options_h(piano) + 24.0 + BTN_H
}

/// Paints form `f` of dialog `m`.
pub fn paint(ui: &mut Ui, s: &State, m: &Modal, f: &MidiForm) {
    let r = place(s, m, W, height(f.piano));
    frame(ui, r, mf::TITLE);
    let t = ui.t;
    let font = Font::new(13);
    let lh = ui.c.line_height(font);
    let mut boxes = Vec::new();
    // Source: which tracks, which MIDI channels.
    let g = Rect::new(r.x + 8.0, r.y + TOP, r.w - 16.0, SOURCE_H);
    section(ui, g, "Source");
    let cx = g.x + 180.0;
    let b = Rect::new(cx, g.y + 29.0, g.right() - 12.0 - cx, 22.0);
    label(ui, g.x + 12.0, (b.cy() - lh / 2.0).round(), mf::TRACKS_LABEL, true);
    combo(ui, f, b, 0, "Import every track, or only this one", &mut boxes);
    label(ui, g.x + 12.0, g.y + 62.0, mf::CHANNELS_LABEL, true);
    let tw = ((g.w - 24.0 - 7.0 * 4.0) / 8.0).floor();
    for c in 0..16u8 {
        let (row, col) = (f32::from(c / 8), f32::from(c % 8));
        let tile = Rect::new(g.x + 12.0 + col * (tw + 4.0), g.y + 84.0 + row * 28.0, tw, 24.0);
        let on = f.o.channels[usize::from(c)];
        let hov = hit(ui, tile, DHit::Form(FHit::Tile(c)), "Import this MIDI channel's notes, or leave them out");
        if !on {
            ui.c.stroke_rrect(tile.inset(0.5), 12.0, 1.0, if hov { t.glyph_dim } else { t.text_faint });
        }
        kit::pill(ui.c, t, tile, &(c + 1).to_string(), font, kit::State::new(hov, false, on));
    }
    // Options: the channel type, then the lamps in two columns (the piano roll's form has four).
    let g = Rect::new(g.x, g.bottom() + GAP, g.w, options_h(f.piano));
    section(ui, g, "Options");
    let mut y = g.y + 42.0;
    if !f.piano {
        let b = Rect::new(cx, g.y + 29.0, g.right() - 12.0 - cx, 22.0);
        label(ui, g.x + 12.0, (b.cy() - lh / 2.0).round(), mf::TYPE_LABEL, true);
        combo(ui, f, b, 1, "The instrument the new channels play", &mut boxes);
        y += 28.0;
    }
    let lamps: Vec<(u8, &str, &'static str)> = (0..6u8).filter_map(|k| f.lamp(k).map(|(text, hint)| (k, text, hint))).collect();
    let col_x = [g.x + 12.0, g.x + 12.0 + (g.w / 2.0).round()];
    for (i, (k, text, hint)) in lamps.into_iter().enumerate() {
        check(ui, col_x[i / 3], y + (i % 3) as f32 * 26.0, text, f.o.check(k, f.piano), true, FHit::Check(k), hint);
    }
    // Accept (Esc and the caption's × import nothing).
    let bw = (ui.c.text_width(font, "Accept") + 28.0).max(BTN_W).round();
    let b = Rect::new(r.right() - 12.0 - bw, r.bottom() - 12.0 - BTN_H, bw, BTN_H);
    let act = Action::Dlg(DHit::Form(FHit::Main));
    let down = ui.down(b, &act);
    let hov = ui.hit(b, act, "Import the file with these settings (Enter)");
    button(ui, b, "Accept", true, hov, down, false);
    if let Some(&(k, b)) = boxes.iter().find(|(k, _)| f.list == Some(*k)) {
        let (items, now) = f.list_items(k);
        list(ui, s, &items, now, b);
    }
}

/// Combo `k` in `b`; remembered in `boxes` for its list.
fn combo(ui: &mut Ui, f: &MidiForm, b: Rect, k: u8, hint: &'static str, boxes: &mut Vec<(u8, Rect)>) {
    let t = ui.t;
    let hov = hit(ui, b, DHit::Form(FHit::Combo(k)), hint);
    kit::combo(ui.c, t, b, &f.combo_text(k), Font::new(13), kit::State::new(hov, f.list == Some(k), false));
    boxes.push((k, b));
}
