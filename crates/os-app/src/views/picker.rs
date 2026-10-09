//! Plugin picker overlay (View > Plugin picker, F8): generators left, effects right, category tabs
//! along the bottom. Tiles are placeholder cards until plugins have their own UIs.
// spec: ui/plugin-picker.md

use os_ui::{Color, Font, Rect};

use crate::app::{Action, State, Ui};

pub struct Plugin {
    pub name: &'static str,
    pub cat: usize,
}

const fn p(name: &'static str, cat: usize) -> Plugin {
    Plugin { name, cat }
}

pub const GEN_CATS: &[&str] = &["Controller", "Drum", "MIDI", "Misc", "Patcher", "Sampler", "Synth classic", "Synth special", "Visual"];
pub const FX_CATS: &[&str] =
    &["Controller", "Delay reverb", "Distortion", "Dynamics", "Filter", "Flanger", "Gain", "Misc", "New", "Patcher", "Visual"];

/// Index 0 is the plain sampler (what "+" in the channel rack adds).
pub const GENERATORS: &[Plugin] = &[
    p("Sampler", 5),
    p("Audio clip", 3),
    p("Automation clip", 3),
    p("Layer", 3),
    p("Patcher", 4),
    p("Placeholder drum", 1),
    p("Placeholder synth", 6),
    p("Placeholder synth 2", 7),
    p("MIDI out", 2),
    p("Placeholder controller", 0),
    p("Placeholder visual", 8),
];
pub const EFFECTS: &[Plugin] = &[
    p("Parametric EQ", 4),
    p("Limiter", 3),
    p("Placeholder reverb", 1),
    p("Placeholder distortion", 2),
    p("Placeholder flanger", 5),
    p("Placeholder gain", 6),
    p("Placeholder patcher", 9),
];

/// Tab centres at 1920 px width (measured), left half then right half.
const GEN_X: [f32; 9] = [70.0, 169.0, 250.0, 326.0, 415.0, 517.0, 639.0, 780.0, 896.0];
const FX_X: [f32; 11] = [1034.0, 1143.0, 1252.0, 1351.0, 1436.0, 1511.0, 1586.0, 1650.0, 1713.0, 1786.0, 1866.0];

const TILE_W: f32 = 104.0;
const TILE_H: f32 = 62.0;
const PITCH_X: f32 = 112.0;
const PITCH_Y: f32 = 88.0;

fn cat_color(i: usize) -> Color {
    const C: [u32; 6] = [0x4c8aa8, 0xa8754c, 0x7fa84c, 0xa84c7f, 0x7f4ca8, 0xa8a04c];
    let v = C[i % C.len()];
    Color::rgb(v)
}

fn rgb(v: u32) -> Color {
    Color::rgb(v)
}

pub fn paint(ui: &mut Ui, s: &State) {
    let (w, h) = ui.c.size();
    let full = Rect::new(0.0, 0.0, w, h);
    let half = w / 2.0;
    ui.c.fill_rect(full, rgb(0x1a1f24));
    ui.c.fill_rect(Rect::new(half, 0.0, half, h), rgb(0x181f25));
    ui.c.fill_rect(Rect::new(0.0, 0.0, w, 14.0), rgb(0x0b0e10));
    ui.c.fill_rect(Rect::new(0.0, h - 70.0, w, 70.0), rgb(0x1e1c23));
    ui.hit(full, Action::PickerBg, "");

    let sx = w / 1920.0;
    let tab_y = h - 40.0;
    // Tabs first: hovering one previews its filter (spec: States).
    let mut preview: Option<(bool, usize)> = None;
    for (effects, cats, xs) in [(false, GEN_CATS, &GEN_X[..]), (true, FX_CATS, &FX_X[..])] {
        for (i, name) in cats.iter().enumerate() {
            let font = Font::new(17);
            let tw = ui.c.text_width(font, name);
            let cx = xs[i] * sx;
            let r = Rect::new(cx - tw / 2.0 - 10.0, tab_y - 12.0, tw + 20.0, 50.0);
            let hov = ui.hit(r, Action::PickerTab(effects, i), "Filter by category");
            let on = s.picker_cat == Some((effects, i));
            if hov {
                preview = Some((effects, i));
            }
            if hov || on {
                let pill = Rect::new(cx - 24.0, tab_y - 32.0, 48.0, 68.0);
                ui.c.fill_rrect(pill, 6.0, rgb(0x2f3d47));
                ui.c.stroke_rrect(pill, 6.0, 1.0, rgb(0x5487a0));
                ui.c.fill_rect(Rect::new(pill.x + 6.0, pill.bottom() - 3.0, pill.w - 12.0, 2.0), rgb(0x75b1d6));
                ui.c.line(cx - 3.0, tab_y - 22.0, cx, tab_y - 25.0, 1.2, rgb(0xd8dde0));
                ui.c.line(cx, tab_y - 25.0, cx + 3.0, tab_y - 22.0, 1.2, rgb(0xd8dde0));
            }
            let c = if hov || on { rgb(0xffffff) } else { rgb(0x6f777c) };
            ui.c.text(font, cx - tw / 2.0, tab_y - 10.0, c, name);
        }
    }
    let all_font = Font::new(26);
    let aw = ui.c.text_width(all_font, "All");
    let all_r = Rect::new(half - aw / 2.0 - 10.0, tab_y - 18.0, aw + 20.0, 40.0);
    ui.hit(all_r, Action::PickerAll, "Show all plugins");
    ui.c.text(all_font, half - aw / 2.0, tab_y - 16.0, rgb(0xffffff), "All");

    let filter = preview.or(s.picker_cat);
    for (effects, list, cats, title) in [(false, GENERATORS, GEN_CATS, "Generators"), (true, EFFECTS, FX_CATS, "Effects")] {
        let x0 = if effects { half + 40.0 * sx } else { 35.0 * sx };
        let area_w = half - 75.0 * sx;
        let cx = if effects { half + half / 2.0 } else { half / 2.0 };
        let tw = ui.c.text_width(Font::new(24), title);
        ui.c.text(Font::new(24), cx - tw / 2.0, 22.0, rgb(0x8a9298), title);
        let shown: Vec<usize> = (0..list.len()).filter(|&i| filter.is_none_or(|(e, c)| e != effects || list[i].cat == c)).collect();
        let filtered = filter.is_some();
        if filtered && filter.is_some_and(|(e, _)| e != effects) {
            continue;
        }
        let cols = ((area_w / PITCH_X) as usize).max(1);
        let rows = shown.len().div_ceil(cols);
        let top = if filtered { (h - 70.0) / 2.0 - TILE_H / 2.0 } else { 110.0 };
        for (n, &i) in shown.iter().enumerate() {
            let (row, col) = (n / cols, n % cols);
            let in_row = if filtered { shown.len().min(cols) } else { cols };
            let row_w = in_row as f32 * PITCH_X - (PITCH_X - TILE_W);
            let tx = if filtered { cx - row_w / 2.0 } else { x0 } + col as f32 * PITCH_X;
            let ty = top + row as f32 * PITCH_Y;
            if rows > 0 && ty + TILE_H > h - 80.0 {
                continue;
            }
            let r = Rect::new(tx, ty, TILE_W, TILE_H);
            let hov = ui.hit(r.outset(4.0), Action::PickerTile(effects, i), list[i].name);
            if hov {
                let fr = r.outset(4.0);
                ui.c.fill_rrect(fr, 6.0, rgb(0x263a42));
                ui.c.stroke_rrect(fr, 6.0, 2.0, rgb(0x5487a0));
            }
            ui.c.fill_rrect(r, 3.0, rgb(0x2b3238));
            ui.c.fill_rect(Rect::new(r.x, r.y, r.w, 4.0), cat_color(list[i].cat));
            let font = Font::new(14);
            ui.c.text_clip(font, r.x + 6.0, r.cy() - 6.0, r.w - 12.0, rgb(0xc1cdd0), list[i].name);
            if hov {
                let name = Font::new(20);
                let cat = Font::new(14);
                let cname = cats[list[i].cat];
                let bw = ui.c.text_width(name, list[i].name).max(ui.c.text_width(cat, cname)) + 20.0;
                let b = Rect::new(r.x, r.bottom() + 8.0, bw, 50.0);
                ui.c.fill_rrect(b, 6.0, rgb(0x090909));
                ui.c.text(name, b.x + 10.0, b.y + 4.0, rgb(0xffffff), list[i].name);
                ui.c.text(cat, b.x + 10.0, b.y + 28.0, rgb(0x8a9298), cname);
            }
        }
    }
}
