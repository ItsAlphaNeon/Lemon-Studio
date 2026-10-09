//! The two toolbar rows.
// spec: ui/main-window.md §1–3

use os_ui::{Face, Rect};

use super::{knob, menus, sunken, tool_button, FrameInfo};
use crate::app::{Action, State, Ui, Win};
use crate::icons::{self, Icon};

/// Placeholder buttons that latch on/off (designed, not functional yet).
pub fn is_toggle(name: &str) -> bool {
    matches!(
        name,
        "metronome" | "wait" | "countdown" | "blend" | "loop" | "typing" | "step" | "slide" | "multilink" | "midi"
    )
}

fn btn(x: f32, y: f32) -> Rect {
    Rect::new(x, y, 30.0, 31.0)
}

pub fn paint(ui: &mut Ui, s: &State, f: &FrameInfo, width: f32) {
    let t = ui.t;
    // Bands.
    ui.c.fill_rect(Rect::new(0.0, 0.0, width, 41.0), t.toolbar);
    ui.c.fill_rect(Rect::new(0.0, 41.0, width, 1.0), t.toolbar_line);
    ui.c.fill_rect(Rect::new(0.0, 42.0, width, 39.0), t.toolbar);
    ui.c.fill_rect(Rect::new(0.0, 81.0, width, 2.0), t.toolbar_line);
    // Empty toolbar areas drag the window (registered first, so controls win).
    ui.hit(Rect::new(0.0, 0.0, width, 81.0), Action::DragWindow, "");
    // Group separators (2 px gaps measured in row 1 and row 2).
    for x in [379.0, 575.0, 876.0, 1048.0, 1204.0] {
        ui.c.fill_rect(Rect::new(x, 2.0, 2.0, 37.0), t.toolbar_line);
    }
    for x in [298.0, 378.0, 576.0, 967.0, 1298.0] {
        ui.c.fill_rect(Rect::new(x, 44.0, 2.0, 35.0), t.toolbar_line);
    }

    row1(ui, s, f, width);
    row2(ui, s, width);

    // Logo spot (spans both rows): OpenStudio placeholder mark.
    let lx = width - 45.0;
    ui.hit(Rect::new(width - 80.0, 0.0, 78.0, 80.0), Action::Placeholder("About"), "OpenStudio (placeholder logo)");
    icons::logo(ui.c, lx, 40.0, 26.0, t.toolbar_line);
}

fn row1(ui: &mut Ui, s: &State, f: &FrameInfo, width: f32) {
    let t = ui.t;
    // Caption buttons.
    for (x, icon, action, hint) in [
        (4.0, Icon::Minimize, Action::Minimize, "Minimize"),
        (25.0, Icon::Restore, Action::Maximize, "Restore / maximize"),
        (44.0, Icon::Close, Action::Close, "Exit OpenStudio"),
    ] {
        let r = Rect::new(x, 11.0, 19.0, 20.0);
        let hov = ui.hit(r, action, hint);
        ui.c.fill_rrect(r, 2.0, if hov { t.button_hi } else { t.button });
        icons::draw(ui.c, icon, r.inset(3.0), t.glyph);
    }

    // Hint panel: project title + hint / message.
    let hp = Rect::span(73.0, 361.0, 5.0, 35.0);
    sunken(ui, hp, t.sunken);
    ui.hit(hp, Action::DragWindow, "Hint bar");
    ui.c.push_clip(hp.inset(2.0));
    ui.c.text(Face::Ui, 11.0, 77.0, 8.0, t.text_dim.shade(-0.1), &f.title);
    let hint: &str = if !f.hint.is_empty() { &f.hint } else { s.message.as_deref().unwrap_or("") };
    ui.c.text(Face::Ui, 12.0, 77.0, 21.0, t.text, hint);
    ui.c.pop_clip();
    icons::draw(ui.c, Icon::MidiControl, Rect::span(364.0, 375.0, 20.0, 34.0), t.glyph_dim);

    // Transport group.
    let ps = Rect::span(385.0, 416.0, 5.0, 35.0);
    ui.c.fill_rrect(ps, 3.0, t.button);
    let pat = Rect::new(388.0, 7.0, 26.0, 12.0);
    let song = Rect::new(388.0, 21.0, 26.0, 12.0);
    ui.hit(pat, Action::PatSong(false), "Pattern mode: play the current pattern");
    ui.hit(song, Action::PatSong(true), "Song mode: play the playlist");
    for (r, label, active) in [(pat, "PAT", !s.song_mode), (song, "SONG", s.song_mode)] {
        if active {
            ui.c.fill_rrect(r, 3.0, t.pill);
        }
        ui.c.text_center(Face::Ui, 9.0, r, if active { t.pill_text } else { t.glyph_dim }, label);
    }
    let play_on = f.playing;
    tool_button(ui, Rect::span(419.0, 449.0, 5.0, 35.0), Icon::Play, play_on, Action::Play, "Play / restart (Space)");
    tool_button(ui, Rect::span(450.0, 481.0, 5.0, 35.0), Icon::Stop, false, Action::Stop, "Stop (Space)");
    let rec = Rect::span(486.0, 513.0, 7.0, 33.0);
    let hov = ui.hit(rec, Action::Record, "Record");
    ui.c.fill_circle(rec.cx(), rec.cy(), 13.5, t.toolbar_line);
    ui.c.fill_circle(rec.cx(), rec.cy(), 12.0, if hov { t.button_hi } else { t.button });
    ui.c.fill_circle(rec.cx(), rec.cy(), 5.0, t.record);
    // Tempo.
    let tb = Rect::span(517.0, 570.0, 6.0, 34.0);
    ui.hit(tb, Action::Tempo, "Tempo (drag up/down)");
    ui.c.fill_rrect(tb, 5.0, t.tempo_box);
    let whole = format!("{}", s.tempo.trunc() as i64);
    let frac = format!(".{:03}", ((s.tempo.fract()) * 1000.0).round() as i64 % 1000);
    let ww = ui.c.text_width(Face::Ui, 19.0, &whole);
    let fw = ui.c.text_width(Face::Ui, 12.0, &frac);
    let x0 = tb.right() - 10.0 - ww - fw;
    ui.c.text(Face::Ui, 19.0, x0, 11.0, t.glyph, &whole);
    ui.c.text(Face::Ui, 12.0, x0 + ww, 17.0, t.glyph_dim.shade(0.3), &frac);
    ui.c.fill_polygon(&[(tb.right() - 7.0, 17.0), (tb.right() - 4.0, 13.0), (tb.right() - 1.0, 17.0)], t.glyph_dim);
    ui.c.fill_polygon(&[(tb.right() - 7.0, 22.0), (tb.right() - 4.0, 26.0), (tb.right() - 1.0, 22.0)], t.glyph_dim);

    // Recording options.
    for (i, (icon, name, hint)) in [
        (Icon::Metronome, "metronome", "Metronome"),
        (Icon::WaitInput, "wait", "Wait for input to start recording"),
        (Icon::Countdown, "countdown", "Countdown before recording"),
        (Icon::Blend, "blend", "Blend recording"),
        (Icon::LoopRecord, "loop", "Loop recording"),
    ]
    .into_iter()
    .enumerate()
    {
        let on = s.toggles.contains(&name);
        tool_button(ui, btn(580.0 + 33.0 * i as f32, 5.0), icon, on, Action::Placeholder(name), hint);
    }

    // Song position (B:S:T).
    let tp = Rect::span(746.0, 872.0, 5.0, 35.0);
    sunken(ui, tp, t.time_panel);
    ui.hit(tp, Action::Placeholder("time display"), "Song position (bar:step:tick)");
    let (bar, step, tick) = bst(f.seconds, s.tempo, 96);
    let main = format!("{bar}:{step:02}");
    let mw = ui.c.text_width(Face::Light, 34.0, &main);
    let tw = ui.c.text_width(Face::Light, 22.0, &format!(":{tick:02}"));
    let x0 = tp.cx() - (mw + tw) / 2.0;
    ui.c.text(Face::Light, 34.0, x0, 6.0, t.glyph, &main);
    ui.c.text(Face::Light, 22.0, x0 + mw, 15.0, t.glyph, &format!(":{tick:02}"));
    ui.c.text(Face::Ui, 7.0, tp.right() - 22.0, 7.0, t.glyph, "B:S:T");

    // Monitors (oscilloscope / peak) and CPU / memory.
    for r in [Rect::span(882.0, 965.0, 5.0, 35.0), Rect::span(970.0, 1044.0, 5.0, 35.0)] {
        sunken(ui, r, t.sunken);
    }
    ui.hit(Rect::span(882.0, 1044.0, 5.0, 35.0), Action::Placeholder("monitor"), "Output monitor");
    let (pl, pr) = f.peaks;
    let lvl = |p: f32| ((1.0 + 20.0 * p.max(1e-6).log10() / 60.0).clamp(0.0, 1.0)) * 80.0;
    let line_y = 20.0;
    ui.c.line(884.0, line_y, 963.0, line_y, 1.0, t.glyph.with_alpha(200));
    ui.c.fill_rect(Rect::new(884.0, 30.0, lvl(pl), 2.0), t.meter_low);
    ui.c.fill_rect(Rect::new(884.0, 33.0, lvl(pr), 2.0), t.meter_low);
    let cpu = Rect::span(1053.0, 1201.0, 4.0, 35.0);
    sunken(ui, cpu, t.sunken_dark);
    ui.hit(cpu, Action::Placeholder("cpu"), "CPU, polyphony and memory");
    ui.c.fill_rect(Rect::new(1055.0, 6.0, 1.5, 12.0), t.glyph);
    ui.c.text_right(Face::Ui, 13.0, 1140.0, 7.0, t.glyph, "0");
    ui.c.line(1055.0, 32.0, 1130.0, 32.0, 1.0, t.glyph.with_alpha(160));
    ui.c.text_right(Face::Ui, 12.0, 1198.0, 7.0, t.glyph, "– MB");
    ui.c.text_right(Face::Ui, 13.0, 1176.0, 20.0, t.glyph, "0");

    // Utility buttons.
    let utils = [
        (Icon::Undo, "Undo history"),
        (Icon::Cut, "Edit tools"),
        (Icon::Mic, "Audio input / recording"),
        (Icon::Help, "Help"),
        (Icon::Save, "Save project (Ctrl+S)"),
        (Icon::SaveRender, "Save and render"),
        (Icon::Info, "Project info"),
        (Icon::Download, "Downloads"),
    ];
    for (i, (icon, hint)) in utils.into_iter().enumerate() {
        let r = btn(1211.0 + 33.0 * i as f32, 5.0);
        if r.right() > width - 85.0 {
            break;
        }
        let action = if icon == Icon::Save { Action::MenuItem(0, menus::FILE_SAVE) } else { Action::Placeholder(hint) };
        tool_button(ui, r, icon, false, action, hint);
    }
}

fn row2(ui: &mut Ui, s: &State, width: f32) {
    let t = ui.t;
    // Menu bar.
    let mb = Rect::span(4.0, 296.0, 45.0, 77.0);
    ui.c.fill_rrect(mb, 2.0, t.menu_panel);
    ui.c.stroke_rrect(mb, 2.0, 1.0, t.toolbar_edge);
    for (i, ((title, _), (x0, x1))) in menus::MENUS.iter().zip(menus::SPANS).enumerate() {
        let r = Rect::new(x0 - 2.0, 47.0, x1 - x0 + 5.0, 29.0);
        let hov = ui.hit(r, Action::Menu(i), *title);
        if hov || s.menu_open == Some(i) {
            ui.c.fill_rrect(r, 2.0, t.menu_drop_hi);
        }
        let tw = ui.c.text_width(Face::Ui, 15.0, title);
        let sx = ((x1 - x0 + 1.0) / tw).min(1.0);
        let size = 15.0 * sx.max(0.75);
        ui.c.text(Face::Ui, size, x0, 54.0, t.menu_text, title);
    }

    // Master knobs.
    ui.hit(Rect::span(303.0, 337.0, 45.0, 77.0), Action::Placeholder("master volume"), "Master volume");
    knob(ui, 319.0, 61.0, 14.0, 0.8, false, t.glyph, t.button_hi);
    ui.hit(Rect::span(340.0, 376.0, 45.0, 77.0), Action::Placeholder("master pitch"), "Master pitch");
    knob(ui, 358.0, 61.0, 14.0, 0.5, true, t.glyph, t.button_hi);
    for k in 0..20 {
        let a = 0.75 * std::f32::consts::PI + 1.5 * std::f32::consts::PI * k as f32 / 19.0;
        ui.c.fill_circle(358.0 + 17.5 * a.cos(), 61.0 + 17.5 * a.sin(), 0.7, t.glyph_dim);
    }

    // Main slider panel.
    let sp = Rect::span(381.0, 575.0, 45.0, 77.0);
    ui.hit(sp, Action::Placeholder("song position slider"), "Song position");
    ui.c.fill_rect(Rect::new(390.0, 61.0, 180.0, 2.0), t.sunken_dark);
    ui.c.fill_circle(396.0, 62.0, 6.5, t.sunken_dark);
    ui.c.line(396.0, 57.0, 396.0, 67.0, 2.0, t.pill);

    // Mode buttons.
    for (i, (icon, name, hint)) in [
        (Icon::TypingKeyboard, "typing", "Typing keyboard to piano keyboard"),
        (Icon::StepEdit, "step", "Step editing"),
        (Icon::Slide, "slide", "Slide / portamento"),
        (Icon::Link, "multilink", "Multilink to controllers"),
        (Icon::MidiControl, "midi", "MIDI note controls"),
    ]
    .into_iter()
    .enumerate()
    {
        let on = s.toggles.contains(&name);
        tool_button(ui, btn(580.0 + 33.0 * i as f32, 45.0), icon, on, Action::Placeholder(name), hint);
    }

    // Snap.
    icons::draw(ui.c, Icon::Magnet, Rect::span(748.0, 766.0, 52.0, 70.0), t.glyph_dim.shade(0.3));
    let snap = Rect::span(768.0, 830.0, 50.0, 72.0);
    let hov = ui.hit(snap, Action::Placeholder("snap"), "Global snap");
    ui.c.fill_rrect(snap, 2.0, if hov { t.button_hi } else { t.sunken_dark });
    ui.c.text(Face::Ui, 12.0, snap.x + 6.0, 56.0, t.glyph, "Line");
    icons::draw(ui.c, Icon::ArrowRight, Rect::new(snap.right() - 14.0, 55.0, 12.0, 12.0), t.glyph_dim);

    // Pattern selector.
    let prev = Rect::span(838.0, 851.0, 47.0, 76.0);
    ui.hit(prev, Action::PatternPrev, "Previous pattern");
    ui.c.fill_rrect(prev, 2.0, t.button);
    icons::draw(ui.c, Icon::ArrowRight, prev.inset(2.0), t.glyph);
    let pb = Rect::span(855.0, 948.0, 47.0, 76.0);
    ui.hit(pb, Action::PatternNext, "Pattern selector (click: next pattern)");
    ui.c.fill_rrect(pb, 2.0, t.tempo_box.shade(0.08));
    let name = s
        .project
        .patterns
        .iter()
        .find(|p| p.number == s.pattern)
        .and_then(|p| p.name.clone())
        .unwrap_or_else(|| format!("Pattern {}", s.pattern));
    ui.c.push_clip(pb.inset(2.0));
    ui.c.text_right(Face::Ui, 16.0, pb.right() - 10.0, 53.0, t.glyph, &name);
    ui.c.pop_clip();
    let add = Rect::span(951.0, 962.0, 50.0, 72.0);
    ui.hit(add, Action::PatternAdd, "Add a pattern");
    icons::draw(ui.c, Icon::Plus, add, t.glyph);

    // Window buttons.
    let wins = [
        (Icon::Playlist, Action::ToggleWin(Win::Playlist), s.front == Some(Win::Playlist), "Playlist (F5)"),
        (Icon::PianoRoll, Action::ToggleWin(Win::PianoRoll), s.front == Some(Win::PianoRoll), "Piano roll (F7)"),
        (Icon::ChannelRack, Action::ToggleWin(Win::ChannelRack), s.show_rack, "Channel rack (F6)"),
        (Icon::Mixer, Action::ToggleWin(Win::Mixer), s.front == Some(Win::Mixer), "Mixer (F9)"),
        (Icon::Browser, Action::ToggleWin(Win::Browser), s.show_browser, "Browser (Alt+F8)"),
        (Icon::Project, Action::Placeholder("project"), false, "Project picker"),
        (Icon::Plug, Action::Placeholder("plugin picker"), false, "Plugin picker"),
        (Icon::AudioEditor, Action::Placeholder("audio editor"), false, "Audio editor"),
        (Icon::Touch, Action::Placeholder("touch controller"), false, "Touch controller"),
        (Icon::Store, Action::Placeholder("store"), false, "Plugin store"),
    ];
    for (i, (icon, action, open, hint)) in wins.into_iter().enumerate() {
        let r = btn(975.0 + 33.0 * i as f32, 45.0);
        let hov = ui.hit(r, action, hint);
        let face = if hov { t.button_hi } else { t.button };
        ui.c.fill_rrect(r, 3.0, t.toolbar_line);
        ui.c.fill_rect_gradient(r.inset(1.0), face.shade(0.04), face, 2.5);
        icons::draw(ui.c, icon, r.inset(6.0), if open { t.glyph } else { t.glyph.mix(t.button, 0.35) });
        if open {
            ui.c.fill_rect(Rect::new(r.x + 9.0, r.bottom() - 3.0, 12.0, 1.5), t.accent);
        }
    }

    // News panel.
    let np = Rect::span(1304.0, 1470.0, 45.0, 77.0);
    if np.right() < width - 85.0 {
        ui.c.fill_rrect(np, 2.0, t.news_panel);
        ui.hit(np, Action::Placeholder("news"), "News");
        ui.c.text(Face::Ui, 11.0, 1310.0, 56.0, t.text_faint, "Today");
        ui.c.text(Face::Ui, 11.0, 1344.0, 56.0, t.text_dim.shade(0.2), "Welcome to OpenStudio");
        icons::draw(ui.c, Icon::Globe, Rect::new(1449.0, 52.0, 18.0, 18.0), t.glyph_dim.shade(0.3));
    }
}

/// Bar:step:tick (1-based bar and step, 16 steps per bar, `ppq / 4` ticks per step).
pub fn bst(seconds: f64, tempo: f64, ppq: u32) -> (u32, u32, u32) {
    let ticks = (seconds * tempo / 60.0 * f64::from(ppq)).max(0.0) as u32;
    let per_step = (ppq / 4).max(1);
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
