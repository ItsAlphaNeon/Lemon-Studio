// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

//! Channel plugin window, opened from a channel's name button.
// spec: ui/channel-rack.md §5 (name button). FL shows the generator plugin's own window here; until
// plugins have editors Lemon Studio shows a plain settings window for the channel (Lemon Studio layout).

use os_model::ChannelKind;
use os_ui::{Font, Rect};

use super::{ball_knob, rack, FrameInfo};
use crate::knobs::KnobId;
use crate::app::{Action, State, Ui, Win};
use crate::icons::{self, Icon};

pub const SIZE: (f32, f32) = (430.0, 200.0);

pub fn rect(s: &State) -> Rect {
    Rect::new(s.plugin_pos.0, s.plugin_pos.1, SIZE.0, SIZE.1)
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
