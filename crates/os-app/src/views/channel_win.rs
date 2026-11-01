// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Channel plugin window, opened from a channel's name button.
// spec: ui/channel-rack.md §5 (name button). FL shows the generator plugin's own window here; until
// plugins have editors Lemon Studio shows a plain settings window for the channel (Lemon Studio layout).
// An automation clip's window adds its range and its LFO (audio/automation.md "Range", "The LFO"; no page measures
// FL's automation clip window, so the layout is Lemon Studio's own).

use os_model::ChannelKind;
use os_ui::{Font, Rect};

use super::{ball_knob, led, rack, FrameInfo};
use crate::knobs::{KnobId, LfoKnob};
use crate::app::{Action, State, Ui, Win};
use crate::icons::{self, Icon};

pub const SIZE: (f32, f32) = (430.0, 200.0);
/// An automation clip's window: room for the LFO's row.
pub const CLIP_SIZE: (f32, f32) = (430.0, 236.0);

pub fn rect(s: &State) -> Rect {
    let mo = s.project.channels.get(s.plugin_channel).is_some_and(crate::lemon_midi_out::is_midi_out);
    let (w, h) = if s.is_clip_row(s.plugin_channel) || mo { CLIP_SIZE } else { SIZE };
    Rect::new(s.plugin_pos.0, s.plugin_pos.1, w, h)
}

pub fn bounds(s: &State) -> Rect {
    rect(s).outset(1.0)
}

pub fn paint(ui: &mut Ui, s: &State, _f: &FrameInfo) {
    let t = ui.t;
    let win = rect(s);
    let Some(ch) = s.project.channels.get(s.plugin_channel) else { return };
    ui.c.fill_rect(win, t.window_border);
    ui.c.fill_rect(win.inset(1.0), t.window_body);
    ui.hit(win, Action::Swallow, "");
    ui.c.fill_rect(Rect::new(win.x + 1.0, win.y + 1.0, win.w - 2.0, 1.0), t.window_title_hi);
    let bar = Rect::span(win.x + 1.0, win.right() - 1.0, win.y + 1.0, win.y + 29.0);
    ui.hit(bar, Action::WinDrag(Win::Plugin), "Drag to move");
    let name = ch.name.clone().unwrap_or_else(|| format!("Channel {}", ch.index + 1));
    icons::draw(ui.c, Icon::Plug, Rect::new(win.x + 9.0, win.y + 7.0, 15.0, 15.0), t.glyph);
    ui.c.text_clip(Font::new(17), win.x + 30.0, win.y + 5.0, win.w - 60.0, t.text, &name);
    let close = Rect::new(win.right() - 22.0, win.y + 6.0, 16.0, 16.0);
    let hov = ui.hit(close, Action::CloseWin(Win::Plugin), "Close");
    icons::draw(ui.c, Icon::Close, close.inset(1.5), if hov { ui.t.tx(0xffffff) } else { t.glyph });

    let body = Rect::span(win.x + 5.0, win.right() - 6.0, win.y + 30.0, win.bottom() - 6.0);
    ui.c.fill_rect(body.outset(1.0), t.rack_inset_border);
    ui.c.fill_rect(body, t.rack_inset);
    let kind = match ch.kind {
        ChannelKind::Sampler => "Sampler".to_string(),
        ChannelKind::Plugin => crate::project_plugins::plugin_title(ch).unwrap_or_else(|| "Plugin".into()),
        ChannelKind::Layer => "Layer".into(),
        ChannelKind::AudioClip => "Audio clip".into(),
        ChannelKind::Automation => "Automation clip".into(),
        ChannelKind::Other(n) => format!("Unknown type {n}"),
    };
    let sample = match (&ch.sample_path, ch.kind) {
        (Some(p), _) => p.rsplit(['/', '\\']).next().unwrap_or(p).to_string(),
        (None, ChannelKind::Sampler) => "(empty - plays the built-in pluck)".into(),
        (None, _) => "-".into(),
    };
    if ch.kind == ChannelKind::Automation {
        return paint_clip(ui, s, body, kind);
    }
    if crate::lemon_midi_out::is_midi_out(ch) {
        return paint_midi_out(ui, s, body);
    }
    let lines = [
        ("Generator", kind),
        ("Sample", sample),
        ("Mixer track", rack::insert_name(ch.mixer_track)),
        ("Volume", format!("{:.0}%", ch.volume * 100.0)),
        ("Panning", format!("{:+.0}", ch.pan * 100.0)),
        ("Pitch", format!("{:+} cents", ch.pitch_cents)),
    ];
    let f = Font::new(15);
    for (i, (k, v)) in lines.iter().enumerate() {
        let ly = body.y + 10.0 + 22.0 * i as f32;
        ui.c.text(f, body.x + 12.0, ly, t.text_dim, k);
        ui.c.text_clip(f, body.x + 100.0, ly, body.w - 180.0, t.text, v);
    }
    let (vol, pan) = (KnobId::ChanVol(s.plugin_channel), KnobId::ChanPan(s.plugin_channel));
    ui.hit(Rect::new(body.right() - 68.0, body.y + 20.0, 28.0, 28.0), Action::Knob(vol), s.knob_text(vol));
    ball_knob(ui, body.right() - 54.0, body.y + 34.0, 12.0, s.knob(vol));
    ui.hit(Rect::new(body.right() - 68.0, body.y + 70.0, 28.0, 28.0), Action::Knob(pan), s.knob_text(pan));
    ball_knob(ui, body.right() - 54.0, body.y + 84.0, 12.0, s.knob(pan));
    ui.c.text_center(Font::new(13), Rect::new(body.right() - 84.0, body.y + 48.0, 60.0, 14.0), t.text_dim, "VOL");
    ui.c.text_center(Font::new(13), Rect::new(body.right() - 84.0, body.y + 98.0, 60.0, 14.0), t.text_dim, "PAN");
}

/// A knob with its hit area, hint and caption below it.
fn knob(ui: &mut Ui, s: &State, id: KnobId, cx: f32, cy: f32, caption: &str) {
    let t = ui.t;
    ui.hit(Rect::new(cx - 14.0, cy - 14.0, 28.0, 28.0), Action::Knob(id), s.knob_text(id));
    ball_knob(ui, cx, cy, 12.0, s.knob(id));
    ui.c.text_center(Font::new(13), Rect::new(cx - 30.0, cy + 14.0, 60.0, 14.0), t.text_dim, caption);
}

/// A switch drawn as an LED with its label; a click flips it (`Action::Knob` on a two-position LFO or MIDI Out control).
fn switch(ui: &mut Ui, s: &State, id: KnobId, x: f32, cy: f32, label: &str) {
    let t = ui.t;
    let w = 20.0 + ui.c.text_width(Font::new(13), label);
    ui.hit(Rect::new(x, cy - 9.0, w, 18.0), Action::Knob(id), s.knob_text(id));
    led(ui, x + 6.0, cy, s.knob(id) >= 0.5);
    ui.c.text(Font::new(13), x + 16.0, cy - 8.0, t.text, label);
}

/// Lemon MIDI Out's settings (spec: plugins/midi-out.md §6.3; Lemon Studio's layout): Channel, Port, Patch and the two
/// Bank boxes as value boxes a drag or the wheel changes, the two checks, and the channel's volume and pan, which its
/// reset messages send.
fn paint_midi_out(ui: &mut Ui, s: &State, body: Rect) {
    use crate::lemon_midi_out::{row_settings, MoKnob};
    let t = ui.t;
    let row = s.plugin_channel;
    let st = row_settings(&s.project, row).unwrap_or_default();
    let f = Font::new(15);
    let lines: [(&str, &[MoKnob]); 5] = [("Generator", &[]), ("Channel", &[MoKnob::Channel]), ("Port", &[MoKnob::Port]), ("Patch", &[MoKnob::Patch]), ("Bank", &[MoKnob::Bank, MoKnob::Bank2])];
    for (i, (k, knobs)) in lines.iter().enumerate() {
        let ly = body.y + 10.0 + 24.0 * i as f32;
        ui.c.text(f, body.x + 12.0, ly, t.text_dim, k);
        if knobs.is_empty() {
            ui.c.text(f, body.x + 100.0, ly, t.text, "Lemon MIDI Out");
            continue;
        }
        for (j, &m) in knobs.iter().enumerate() {
            let b = Rect::new(body.x + 100.0 + 56.0 * j as f32, ly - 2.0, 48.0, 20.0);
            let id = KnobId::MidiOut(row, m);
            ui.hit(b, Action::Knob(id), s.knob_text(id));
            let v = match m {
                MoKnob::Patch if st.patch == 0 => "---".to_string(),
                MoKnob::Patch => st.patch.to_string(),
                _ => m.value(&st),
            };
            super::settings::value_red(ui, b, &v);
        }
        if knobs[0] == MoKnob::Patch && st.patch > 0 {
            let name = crate::lemon_midi_out::GM_NAMES[usize::from(st.patch - 1)];
            ui.c.text_clip(Font::new(13), body.x + 156.0, ly + 1.0, body.w - 260.0, t.text, name);
        }
    }
    let ly = body.y + 10.0 + 24.0 * 5.0 + 8.0;
    switch(ui, s, KnobId::MidiOut(row, MoKnob::SendLevels), body.x + 12.0, ly + 8.0, MoKnob::SendLevels.caption());
    switch(ui, s, KnobId::MidiOut(row, MoKnob::MapColor), body.x + 12.0, ly + 32.0, MoKnob::MapColor.caption());
    knob(ui, s, KnobId::ChanVol(row), body.right() - 104.0, body.y + 34.0, "VOL");
    knob(ui, s, KnobId::ChanPan(row), body.right() - 44.0, body.y + 34.0, "PAN");
}

/// An automation clip's settings: its curve, its range (the channel's pan knob is the minimum, its volume knob the
/// maximum) and its LFO.
fn paint_clip(ui: &mut Ui, s: &State, body: Rect, kind: String) {
    let t = ui.t;
    let row = s.plugin_channel;
    let Some(ch) = s.project.channels.get(row) else { return };
    let points = s.project.automation(ch.index).map_or(0, |a| a.points.len());
    let (min, max) = (s.knob(KnobId::ChanPan(row)), s.knob(KnobId::ChanVol(row)));
    let lines = [("Generator", kind), ("Points", points.to_string()), ("Range", format!("{:.0}% to {:.0}%", min * 100.0, max * 100.0))];
    let f = Font::new(15);
    for (i, (k, v)) in lines.iter().enumerate() {
        let ly = body.y + 10.0 + 22.0 * i as f32;
        ui.c.text(f, body.x + 12.0, ly, t.text_dim, k);
        ui.c.text_clip(f, body.x + 100.0, ly, body.w - 200.0, t.text, v);
    }
    knob(ui, s, KnobId::ChanPan(row), body.right() - 104.0, body.y + 34.0, "MIN");
    knob(ui, s, KnobId::ChanVol(row), body.right() - 44.0, body.y + 34.0, "MAX");

    // The LFO's row.
    let ly = body.y + 96.0;
    ui.c.fill_rect(Rect::new(body.x + 8.0, ly, body.w - 16.0, 1.0), t.rack_inset_border);
    ui.c.text(Font::new(15), body.x + 12.0, ly + 10.0, t.text_dim, "LFO");
    switch(ui, s, KnobId::AutoLfo(row, LfoKnob::On), body.x + 12.0, ly + 50.0, "On");
    switch(ui, s, KnobId::AutoLfo(row, LfoKnob::Mode), body.x + 12.0, ly + 74.0, "Add");
    let knobs = [(LfoKnob::Amount, "AMOUNT"), (LfoKnob::Speed, "SPEED"), (LfoKnob::Tension, "TENSION"), (LfoKnob::Skew, "SKEW"), (LfoKnob::Pulse, "PULSE")];
    for (i, (k, caption)) in knobs.into_iter().enumerate() {
        knob(ui, s, KnobId::AutoLfo(row, k), body.x + 110.0 + 60.0 * i as f32, ly + 50.0, caption);
    }
}
