//! Debug panel for the mixer meters (right-click the mixer's level scale): one slider per `meters::PARAMS` entry,
//! plus Reset / Save / Close. A tuning aid for matching FL by eye, not an FL feature.
//!
// spec: ui/mixer.md §9 (meters)

use super::Ui;
use crate::app::{Action, State};
use crate::meters::PARAMS;
use os_ui::{Color, Rect};

/// Whole panel (over the browser, left of the mixer's strips).
pub const PANEL: Rect = Rect::new(40.0, 130.0, 380.0, 30.0 + 20.0 * PARAMS.len() as f32);
const ROW_Y: f32 = PANEL.y + 26.0;

/// Slider track of parameter `i` (the drag maps x across it to the range).
pub fn slider(i: usize) -> Rect {
    Rect::new(PANEL.x + 160.0, ROW_Y + 20.0 * i as f32 + 6.0, 120.0, 8.0)
}

/// Header buttons: Reset, Save, Close.
pub fn button(n: usize) -> Rect {
    Rect::new(PANEL.x + PANEL.w - 156.0 + 52.0 * n as f32, PANEL.y + 5.0, 48.0, 17.0)
}

pub fn paint(ui: &mut Ui, s: &State) {
    let cfg = &s.mixer.meter_cfg;
    let (bg, edge, ink, dim, fill) =
        (Color::rgb(0x2b2f32), Color::rgb(0x15181a), Color::rgb(0xd6dbde), Color::rgb(0x8a949b), Color::rgb(0xe0a040));
    ui.c.fill_rect(PANEL, edge);
    ui.c.fill_rect(PANEL.inset(1.0), bg);
    ui.hit(PANEL, Action::Swallow, "Meter debug panel: drag a slider, right-click resets it");
    ui.c.text(super::fnt(13.0).bold(), PANEL.x + 8.0, PANEL.y + 5.0, ink, "Mixer meter debug");
    for (n, label) in ["Reset", "Save", "Close"].into_iter().enumerate() {
        let r = button(n);
        ui.c.fill_rect(r, Color::rgb(0x3d4347));
        let w = ui.c.text_width(super::fnt(12.0), label);
        ui.c.text(super::fnt(12.0), r.x + (r.w - w) / 2.0, r.y + 1.0, ink, label);
        let hint = match n {
            0 => "Reset every meter setting to its default",
            1 => "Save the settings to %APPDATA%\\OpenStudio\\meters.txt (loaded at start)",
            _ => "Close the meter debug panel",
        };
        ui.hit(r, Action::MeterBtn(n as u8), hint);
    }
    for (i, p) in PARAMS.iter().enumerate() {
        let y = ROW_Y + 20.0 * i as f32;
        let changed = (cfg.v[i] - p.default).abs() > 1e-4;
        ui.c.text(super::fnt(12.0), PANEL.x + 8.0, y + 1.0, if changed { fill } else { ink }, p.name);
        let t = slider(i);
        ui.c.fill_rect(t, Color::rgb(0x1d2022));
        ui.c.fill_rect(Rect::new(t.x, t.y, (t.w * cfg.norm(i)).round(), t.h), if changed { fill } else { dim });
        ui.c.text(super::fnt(12.0), t.x + t.w + 8.0, y + 1.0, ink, &cfg.text(i));
        ui.hit(Rect::new(t.x - 4.0, y, t.w + 8.0, 20.0), Action::MeterParam(i), p.name);
    }
}
