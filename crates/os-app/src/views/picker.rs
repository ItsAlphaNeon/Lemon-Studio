//! Plugin picker overlay (View > Plugin picker, F8): generators left, effects right, category tabs
//! along the bottom. Tiles are small drawn "faces" (OpenStudio's own art, not FL thumbnails); the name
//! appears in the hover tooltip only, as in FL.
// spec: ui/plugin-picker.md

use os_ui::{Canvas, Color, Font, Rect};

use crate::app::{Action, State, Ui};

/// How a tile is drawn (spec: OpenStudio choices).
#[derive(Clone, Copy)]
pub enum Face {
    /// Name as text instead of a picture (Audio clip, Automation clip).
    Label,
    /// Knob panel over a mini keyboard.
    Synth,
    /// Wide, short rack unit with a row of knobs.
    Rack,
    /// Grid of coloured pads.
    Pads,
    /// Dark display with a response curve (EQ / filter).
    Curve,
    /// Small box of knobs.
    Knobs,
    /// Level meter bars.
    Meter,
}

#[derive(Clone, Copy)]
pub struct Plugin {
    pub name: &'static str,
    pub cat: usize,
    pub face: Face,
    /// Tile size at 1920 px (FL's thumbnails vary per plugin, 40-130 x 20-70).
    pub w: f32,
    pub h: f32,
    pub accent: u32,
}

const fn p(name: &'static str, cat: usize, face: Face, w: u16, h: u16, accent: u32) -> Plugin {
    Plugin { name, cat, face, w: w as f32, h: h as f32, accent }
}

pub const GEN_CATS: &[&str] = &["Controller", "Drum", "MIDI", "Misc", "Patcher", "Sampler", "Synth classic", "Synth special", "Visual"];
pub const FX_CATS: &[&str] =
    &["Controller", "Delay reverb", "Distortion", "Dynamics", "Filter", "Flanger", "Gain", "Misc", "New", "Patcher", "Visual"];

use Face::*;

/// Index 0 is the plain sampler (what "+" in the channel rack adds).
pub const GENERATORS: &[Plugin] = &[
    p("Sampler", 5, Knobs, 92, 62, 0x7fa0b0),
    p("Audio clip", 3, Label, 0, 0, 0),
    p("Automation clip", 3, Label, 0, 0, 0),
    p("Layer", 3, Rack, 80, 34, 0x9aa4aa),
    p("Patcher", 4, Curve, 86, 58, 0x6fb0d0),
    p("Placeholder drum", 1, Pads, 110, 62, 0x5fc8a0),
    p("Placeholder synth", 6, Synth, 120, 62, 0xe08a3c),
    p("Placeholder synth 2", 7, Synth, 96, 58, 0xc05878),
    p("MIDI out", 2, Rack, 96, 30, 0x58a0e0),
    p("Placeholder controller", 0, Knobs, 64, 50, 0xd0b048),
    p("Placeholder visual", 8, Meter, 70, 54, 0x8070e0),
];
pub const EFFECTS: &[Plugin] = &[
    p("Parametric EQ", 4, Curve, 128, 64, 0x60c0e0),
    p("Limiter", 3, Meter, 96, 60, 0xe07050),
    p("Placeholder reverb", 1, Curve, 100, 56, 0x9080d0),
    p("Placeholder distortion", 2, Knobs, 84, 48, 0xd04838),
    p("Placeholder flanger", 5, Rack, 120, 34, 0x50b8a0),
    p("Placeholder gain", 6, Knobs, 44, 40, 0x9ab0b8),
    p("Placeholder patcher", 9, Curve, 86, 58, 0x6fb0d0),
];

/// Built-in generators followed by the hosted instruments (`State::hosted_generators`).
pub fn generators(s: &State) -> Vec<Plugin> {
    let mut v = GENERATORS.to_vec();
    for &name in &s.hosted_generators {
        let cat = if name.contains("Sampler") { 5 } else { 7 };
        v.push(p(name, cat, Knobs, 92, 62, 0x49f1b1));
    }
    v
}

/// Tab centres at 1920 px width (measured), left half then right half.
const GEN_X: [f32; 9] = [70.0, 169.0, 250.0, 326.0, 415.0, 517.0, 639.0, 780.0, 896.0];
const FX_X: [f32; 11] = [1034.0, 1143.0, 1252.0, 1351.0, 1436.0, 1511.0, 1586.0, 1650.0, 1713.0, 1786.0, 1866.0];

const ROW_PITCH: f32 = 88.0;
const MIN_GAP: f32 = 24.0;
const LABEL_FONT: u16 = 17;

fn rgb(v: u32) -> Color {
    // Theme roles (spec: ui/flstheme.md): greys follow the surface / text mapping, coloured accents stay.
    let c = Color::rgb(v);
    let (_, s, l) = c.hsl();
    let p = os_theme::current();
    if s > 0.35 && l > 0.2 && l < 0.9 { c } else if l > 0.55 { p.tx(v) } else { p.s(v) }
}

/// Tile size; text tiles take their label's width.
fn tile_size(c: &mut Canvas, pl: &Plugin) -> (f32, f32) {
    match pl.face {
        Label => (c.text_width(Font::new(LABEL_FONT), &label(pl)) + 4.0, 24.0),
        _ => (pl.w, pl.h),
    }
}

/// "Audio clip" -> "Audio Clip" (FL title-cases these labels).
fn label(pl: &Plugin) -> String {
    pl.name.split(' ').map(|w| w[..1].to_uppercase() + &w[1..]).collect::<Vec<_>>().join(" ")
}

/// Lay `items` (sizes) out in rows inside `area`: rows are justified across the width (the last row
/// keeps the minimum gap), every tile is centred on its row's mid line, and the block of rows is centred
/// vertically. `centred` puts each row in the middle instead (filter preview).
fn flow(sizes: &[(f32, f32)], area: Rect, centred: bool, gap: f32) -> Vec<Rect> {
    let mut rows: Vec<Vec<usize>> = vec![];
    let mut w = 0.0;
    for (i, &(tw, _)) in sizes.iter().enumerate() {
        match rows.last_mut() {
            Some(r) if w + gap + tw <= area.w => {
                r.push(i);
                w += gap + tw;
            }
            _ => {
                rows.push(vec![i]);
                w = tw;
            }
        }
    }
    let block = rows.len() as f32 * ROW_PITCH;
    let top = (area.y + (area.h - block) / 2.0).max(area.y);
    let mut out = vec![Rect::new(0.0, 0.0, 0.0, 0.0); sizes.len()];
    let last = rows.len().saturating_sub(1);
    for (ri, row) in rows.iter().enumerate() {
        let used: f32 = row.iter().map(|&i| sizes[i].0).sum();
        let n = row.len() as f32;
        let (mut x, g) = if centred {
            let total = used + gap * (n - 1.0);
            (area.x + (area.w - total) / 2.0, gap)
        } else if ri == last || row.len() < 2 {
            (area.x, gap)
        } else {
            (area.x, (area.w - used) / (n - 1.0))
        };
        let cy = top + ri as f32 * ROW_PITCH + ROW_PITCH / 2.0;
        for &i in row {
            let (tw, th) = sizes[i];
            out[i] = Rect::new(x.round(), (cy - th / 2.0).round(), tw, th);
            x += tw + g;
        }
    }
    out
}

pub fn paint(ui: &mut Ui, s: &State) {
    let (w, h) = ui.c.size();
    let full = Rect::new(0.0, 0.0, w, h);
    let half = w / 2.0;
    let sx = w / 1920.0;
    let bar_h = 70.0;
    let tab_cy = h - 40.0;

    // Background: halves, top vignette, tab bar (spec: Layout).
    ui.c.fill_rect(Rect::new(0.0, 0.0, half, h), rgb(0x1a1f24));
    ui.c.fill_rect(Rect::new(half, 0.0, half, h), rgb(0x181f25));
    ui.c.fill_rect_gradient(Rect::new(0.0, 0.0, w, 60.0), rgb(0x040506), rgb(0x1a1f25).with_alpha(0), 0.0);
    ui.c.fill_rect(Rect::new(0.0, h - bar_h, w, bar_h), rgb(0x1e1c23));
    ui.hit(full, Action::PickerBg, "");

    // Tabs first: hovering one previews its filter (spec: States).
    let mut preview: Option<(bool, usize)> = None;
    let tab_font = Font::new(17);
    let mut tabs = vec![];
    for (effects, cats, xs) in [(false, GEN_CATS, &GEN_X[..]), (true, FX_CATS, &FX_X[..])] {
        for (i, name) in cats.iter().enumerate() {
            let tw = ui.c.text_width(tab_font, name);
            let cx = xs[i] * sx;
            // The pill hugs the label: at least 48 wide, 6 px either side of the text.
            let pw = (tw + 14.0).max(48.0).round();
            let pill = Rect::new((cx - pw / 2.0).round(), tab_cy - 32.0, pw, 66.0);
            let hov = ui.hit(pill, Action::PickerTab(effects, i), "Filter by category");
            if hov {
                preview = Some((effects, i));
            }
            tabs.push((effects, i, *name, cx, tw, pill, hov));
        }
    }
    let filter = preview.or(s.picker_cat);
    if filter.is_some() {
        // The filter preview drops the two-tone halves.
        ui.c.fill_rect(Rect::new(0.0, 60.0, w, h - bar_h - 60.0), rgb(0x1a1f25));
    }
    for (effects, i, name, cx, tw, pill, hov) in tabs {
        let lit = hov || s.picker_cat == Some((effects, i));
        if lit {
            pill_frame(&mut ui.c, pill);
            let ty = pill.y + 6.0;
            ui.c.fill_polygon(&[(cx - 5.0, ty + 5.0), (cx, ty), (cx + 5.0, ty + 5.0)], rgb(0xe6f2f4));
        }
        let tx = (cx - tw / 2.0).round();
        let ty = tab_cy - 11.0;
        if lit {
            ui.c.text(tab_font, tx + 1.0, ty + 1.0, rgb(0x0a0d10).with_alpha(200), name);
        }
        ui.c.text(tab_font, tx, ty, if lit { rgb(0xffffff) } else { rgb(0x6f777c) }, name);
    }
    let all_font = Font::new(26);
    let aw = ui.c.text_width(all_font, "All");
    ui.hit(Rect::new(half - aw / 2.0 - 10.0, tab_cy - 22.0, aw + 20.0, 44.0), Action::PickerAll, "Show all plugins");
    let all_c = if filter.is_none() { rgb(0xffffff) } else { rgb(0x6f777c) };
    ui.c.text(all_font, (half - aw / 2.0).round(), tab_cy - 17.0, all_c, "All");
    for x in [8.0, w - 8.0] {
        let dw = ui.c.text_width(Font::new(14), "...");
        ui.c.text(Font::new(14), x - dw / 2.0, tab_cy - 6.0, rgb(0x6f777c), "...");
    }

    search_icon(&mut ui.c, half, 38.0);

    // Tiles.
    let mut tooltip = None;
    let content = Rect::new(0.0, 62.0, w, h - bar_h - 62.0 - 10.0);
    // Generators: the built-in list, then the hosted (CLAP) instruments found at startup (spec: plugins/plugin-formats.md).
    let gens = generators(s);
    for (effects, list, cats, title) in [(false, &gens[..], GEN_CATS, "Generators"), (true, EFFECTS, FX_CATS, "Effects")] {
        let shown: Vec<usize> = match filter {
            Some((e, c)) if e == effects => (0..list.len()).filter(|&i| list[i].cat == c).collect(),
            Some(_) => vec![],
            None => (0..list.len()).collect(),
        };
        let sizes: Vec<(f32, f32)> = shown.iter().map(|&i| tile_size(&mut ui.c, &list[i])).collect();
        let rects = if filter.is_some() {
            // Preview: one centred row across the whole picker, headings hidden.
            flow(&sizes, Rect::new(60.0 * sx, content.y, w - 120.0 * sx, content.h), true, 46.0)
        } else {
            let font = Font::new(24);
            let tw = ui.c.text_width(font, title);
            let cx = if effects { half + half / 2.0 } else { half / 2.0 };
            ui.c.text(font, (cx - tw / 2.0).round(), 24.0, rgb(0x8a9298), title);
            let x0 = if effects { half + 43.0 * sx } else { 35.0 * sx };
            flow(&sizes, Rect::new(x0, content.y, half - 78.0 * sx, content.h), false, MIN_GAP)
        };
        for (n, &i) in shown.iter().enumerate() {
            let r = rects[n];
            if r.bottom() > content.bottom() {
                continue;
            }
            let fr = hover_frame(r);
            if ui.hit(fr, Action::PickerTile(effects, i), list[i].name) {
                hover_bg(&mut ui.c, fr);
                tooltip = Some((fr, list[i].name, cats[list[i].cat]));
            }
            face(&mut ui.c, &list[i], r);
        }
    }
    // The tooltip goes last so it covers neighbouring tiles.
    if let Some((fr, name, cat)) = tooltip {
        tip(&mut ui.c, fr, name, cat, Rect::new(0.0, 0.0, w, h - bar_h));
    }
}

/// The hover frame around a tile: 10 px larger on every side, at least 72 x 60.
fn hover_frame(r: Rect) -> Rect {
    let fw = (r.w + 20.0).max(72.0);
    let fh = (r.h + 20.0).max(60.0);
    Rect::new((r.cx() - fw / 2.0).round(), (r.cy() - fh / 2.0).round(), fw, fh)
}

/// Blue-grey rounded panel, light 1 px edge and a glow spot (shared by tile hover and tab pill).
fn hover_bg(c: &mut Canvas, fr: Rect) {
    c.fill_rect_gradient(fr, rgb(0x2e4a54), rgb(0x27333a), 8.0);
    c.stroke_rrect(fr.inset(1.0), 7.0, 1.2, rgb(0x5b7886));
    glow(c, fr, fr.cx(), fr.y + 1.5, fr.w * 0.3);
}

fn pill_frame(c: &mut Canvas, pill: Rect) {
    c.fill_rect_gradient(pill, rgb(0x344a54), rgb(0x2d323a), 7.0);
    c.stroke_rrect(pill.inset(1.0), 6.0, 1.2, rgb(0x5f7886));
    glow(c, pill, pill.cx(), pill.bottom() - 2.0, 18.0);
}

/// Soft light spot centred on an edge of `bounds` (stacked translucent ellipses, clipped to the frame).
fn glow(c: &mut Canvas, bounds: Rect, cx: f32, cy: f32, r: f32) {
    c.push_clip(bounds);
    for k in 0..10 {
        let t = k as f32 / 9.0;
        let rr = r * (1.0 - t * 0.8);
        let gh = 14.0 * (1.0 - t * 0.75);
        c.fill_rrect(Rect::new(cx - rr, cy - gh / 2.0, rr * 2.0, gh), gh / 2.0, rgb(0x7fd4f4).with_alpha(18));
    }
    c.fill_rrect(Rect::new(cx - 5.0, cy - 1.5, 10.0, 3.0), 1.5, rgb(0xd8fcff));
    c.pop_clip();
}

/// Name + category box, hanging off the frame's lower edge and indented 30 px (spec: Tile hover).
fn tip(c: &mut Canvas, fr: Rect, name: &str, cat: &str, bounds: Rect) {
    let (nf, cf) = (Font::new(20), Font::new(16));
    let bw = (c.text_width(nf, name).max(c.text_width(cf, cat)) + 20.0).round();
    let bh = 54.0;
    let x = (fr.x + 30.0).min(bounds.right() - bw - 4.0).max(4.0);
    let mut y = fr.bottom() - 2.0;
    if y + bh > bounds.bottom() {
        y = fr.y - bh + 2.0;
    }
    let b = Rect::new(x, y, bw, bh);
    c.fill_rrect(b.outset(1.0), 9.0, rgb(0x000000).with_alpha(90));
    c.fill_rrect(b, 8.0, rgb(0x090909).with_alpha(245));
    c.text(nf, b.x + 10.0, b.y + 5.0, rgb(0xffffff), name);
    c.text(cf, b.x + 10.0, b.y + 30.0, rgb(0x9aa2a8), cat);
}

/// Circle-and-handle magnifier with a "T" inside (search, not wired yet).
fn search_icon(c: &mut Canvas, cx: f32, cy: f32) {
    let col = rgb(0x4c555c);
    c.stroke_circle(cx - 2.0, cy - 2.0, 18.0, 3.0, col);
    c.line(cx + 11.0, cy + 11.0, cx + 20.0, cy + 20.0, 4.0, col);
    c.line(cx - 8.0, cy - 9.0, cx + 4.0, cy - 9.0, 2.0, col);
    c.line(cx - 2.0, cy - 9.0, cx - 2.0, cy + 6.0, 2.0, col);
}

/// Draw one tile face (all original OpenStudio art).
fn face(c: &mut Canvas, pl: &Plugin, r: Rect) {
    let acc = rgb(pl.accent);
    match pl.face {
        Label => {
            c.text(Font::new(LABEL_FONT), r.x + 2.0, r.y + 2.0, rgb(0xe8e8b0), &label(pl));
            return;
        }
        _ => {}
    }
    // Device body: dark panel with a lighter top lip and a shadow.
    c.fill_rrect(r.outset(1.0), 3.0, rgb(0x000000).with_alpha(110));
    c.fill_rect_gradient(r, rgb(0x3a4248), rgb(0x272d32), 2.0);
    c.fill_rect(Rect::new(r.x, r.y, r.w, 1.0), rgb(0x59636a));
    let inner = r.inset(4.0);
    match pl.face {
        Label => {}
        Synth => {
            c.fill_rect(Rect::new(r.x, r.y + 1.0, r.w, 3.0), acc);
            let kb_h = (r.h * 0.3).round();
            let panel = Rect::new(inner.x, inner.y + 2.0, inner.w, inner.h - kb_h - 3.0);
            knob_grid(c, panel, acc);
            keyboard(c, Rect::new(r.x + 2.0, r.bottom() - kb_h - 2.0, r.w - 4.0, kb_h));
        }
        Rack => {
            c.fill_rect(Rect::new(r.x + 3.0, r.y + 3.0, 3.0, r.h - 6.0), acc);
            c.fill_rect(Rect::new(r.right() - 6.0, r.y + 3.0, 3.0, r.h - 6.0), acc.with_alpha(120));
            let n = ((inner.w - 10.0) / 11.0) as usize;
            let kr = (inner.h * 0.28).min(4.0);
            for k in 0..n {
                let x = inner.x + 8.0 + k as f32 * 11.0 + 3.0;
                knob(c, x, inner.cy(), kr, if k % 3 == 0 { acc } else { rgb(0xc8d0d4) });
            }
        }
        Pads => {
            let screen = Rect::new(inner.x, inner.y, inner.w * 0.42, inner.h);
            let cols = 4;
            let rows = 3;
            let pw = (screen.w - 3.0 * (cols as f32 - 1.0)) / cols as f32;
            let ph = (screen.h - 3.0 * (rows as f32 - 1.0)) / rows as f32;
            const PAD: [u32; 5] = [0x5fc8a0, 0x60b0e0, 0xe0c050, 0x70d070, 0xd06060];
            for y in 0..rows {
                for x in 0..cols {
                    let col = rgb(PAD[(x * 2 + y * 3) % PAD.len()]);
                    c.fill_rect(Rect::new(screen.x + x as f32 * (pw + 3.0), screen.y + y as f32 * (ph + 3.0), pw, ph), col);
                }
            }
            let d = Rect::new(screen.right() + 5.0, inner.y, inner.right() - screen.right() - 5.0, inner.h);
            c.fill_rect(d, rgb(0x1a1f23));
            curve(c, d.inset(2.0), acc, false);
        }
        Curve => {
            let d = Rect::new(inner.x, inner.y, inner.w, inner.h * 0.7);
            c.fill_rect(d, rgb(0x182026));
            for k in 1..4 {
                let gx = d.x + d.w * k as f32 / 4.0;
                c.fill_rect(Rect::new(gx, d.y, 1.0, d.h), rgb(0x24303a));
            }
            curve(c, d.inset(2.0), acc, true);
            let kr = 2.5;
            let n = ((inner.w - 4.0) / 10.0) as usize;
            for k in 0..n {
                knob(c, inner.x + 5.0 + k as f32 * 10.0, inner.bottom() - kr - 1.0, kr, rgb(0xb0bcc2));
            }
        }
        Knobs => {
            c.fill_rect(Rect::new(r.x, r.y + 1.0, r.w, 2.0), acc);
            knob_grid(c, Rect::new(inner.x, inner.y + 2.0, inner.w, inner.h - 2.0), acc);
        }
        Meter => {
            let d = inner;
            c.fill_rect(d, rgb(0x14191d));
            let n = ((d.w - 4.0) / 6.0) as usize;
            for k in 0..n {
                let lvl = 0.35 + 0.6 * (((k * 7 + 3) % 11) as f32 / 10.0);
                let bh = (d.h - 4.0) * lvl;
                let x = d.x + 3.0 + k as f32 * 6.0;
                c.fill_rect_gradient(Rect::new(x, d.bottom() - 2.0 - bh, 4.0, bh), acc, rgb(0x3c8a5a), 0.0);
            }
        }
    }
}

fn knob(c: &mut Canvas, x: f32, y: f32, r: f32, mark: Color) {
    c.fill_circle(x, y, r, rgb(0x15191c));
    c.fill_circle(x, y, r - 0.8, rgb(0x4a5258));
    c.line(x, y, x + r * 0.5, y - r * 0.75, 1.0, mark);
}

fn knob_grid(c: &mut Canvas, r: Rect, acc: Color) {
    let kr = 3.5;
    let pitch = 11.0;
    let cols = ((r.w - 2.0) / pitch).floor().max(1.0) as usize;
    let rows = ((r.h - 2.0) / pitch).floor().max(1.0) as usize;
    let ox = r.x + (r.w - cols as f32 * pitch) / 2.0 + pitch / 2.0;
    let oy = r.y + (r.h - rows as f32 * pitch) / 2.0 + pitch / 2.0;
    for y in 0..rows {
        for x in 0..cols {
            let m = if (x + y) % 4 == 1 { acc } else { rgb(0xc8d0d4) };
            knob(c, ox + x as f32 * pitch, oy + y as f32 * pitch, kr, m);
        }
    }
}

fn keyboard(c: &mut Canvas, r: Rect) {
    c.fill_rect(r, rgb(0xe8eaec));
    let kw = 4.0;
    let n = (r.w / kw) as usize;
    for k in 1..n {
        c.fill_rect(Rect::new(r.x + k as f32 * kw, r.y, 0.6, r.h), rgb(0x8a8e92));
    }
    for k in 0..n {
        if matches!(k % 7, 0 | 1 | 3 | 4 | 5) && k + 1 < n {
            c.fill_rect(Rect::new(r.x + k as f32 * kw + kw * 0.65, r.y, kw * 0.7, r.h * 0.6), rgb(0x1a1c1e));
        }
    }
}

/// A smooth band curve (bell when `bell`, low-pass shoulder otherwise) with a faint fill below it.
fn curve(c: &mut Canvas, r: Rect, acc: Color, bell: bool) {
    let n = 24;
    let y_of = |t: f32| {
        let v = if bell {
            0.5 - 0.35 * (-((t - 0.4) * 6.0).powi(2)).exp() + 0.2 * (-((t - 0.8) * 8.0).powi(2)).exp()
        } else {
            0.3 + 0.6 / (1.0 + (-(t - 0.7) * 14.0).exp())
        };
        r.y + r.h * v
    };
    let mut prev = (r.x, y_of(0.0));
    for k in 1..=n {
        let t = k as f32 / n as f32;
        let pt = (r.x + r.w * t, y_of(t));
        c.fill_polygon(&[prev, pt, (pt.0, r.bottom()), (prev.0, r.bottom())], acc.with_alpha(40));
        c.line(prev.0, prev.1, pt.0, pt.1, 1.2, acc);
        prev = pt;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flow_justifies_and_centres() {
        let sizes = vec![(100.0, 60.0); 5];
        let area = Rect::new(0.0, 0.0, 330.0, 400.0);
        let r = flow(&sizes, area, false, 24.0);
        // Three fit on a row (3*100 + 2*24 = 348 > 330 -> two per row).
        assert_eq!(r[0].x, 0.0);
        assert_eq!(r[1].right(), 330.0, "full rows span the area");
        assert_eq!(r[4].x, 0.0);
        // Three rows of 88 centred in 400: top = 68, first row mid = 112.
        assert_eq!(r[0].cy(), 112.0);
        let c = flow(&sizes[..2], area, true, 46.0);
        assert_eq!(c[0].x, ((330.0 - 246.0) / 2.0f32).round());
    }
}
