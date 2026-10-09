//! Docked browser (left).
// spec: ui/main-window.md §4

use os_ui::{Color, Face, Rect};

use crate::app::{Action, State, Ui};
use crate::icons::{self, Icon};

/// (label, icon, category color index; 255 = folder)
const ROWS: &[(&str, Icon, u8)] = &[
    ("Current project", Icon::Page, 0),
    ("Recent files", Icon::Back, 1),
    ("Plugin database", Icon::Speaker, 2),
    ("Plugin presets", Icon::Speaker, 3),
    ("Channel presets", Icon::Box, 4),
    ("Mixer presets", Icon::Mixer, 5),
    ("Scores", Icon::Note, 6),
    ("Audio", Icon::Person, 7),
    ("Backup", Icon::Back, 8),
    ("Channel envelopes", Icon::Folder, 255),
    ("Clipboard files", Icon::Folder, 255),
    ("Demo projects", Icon::Folder, 255),
    ("Envelopes", Icon::Folder, 255),
    ("Cloud", Icon::Cloud, 9),
    ("Impulses", Icon::Folder, 255),
    ("Misc", Icon::Folder, 255),
    ("My projects", Icon::Folder, 255),
    ("Packs", Icon::Box, 2),
    ("Project bones", Icon::Folder, 255),
    ("Recorded", Icon::Wave, 2),
    ("Rendered", Icon::Wave, 2),
    ("Sliced audio", Icon::Wave, 2),
    ("Soundfonts", Icon::Folder, 255),
    ("Speech", Icon::Folder, 255),
    ("Templates", Icon::Folder, 255),
    ("User data", Icon::Person, 7),
];

pub fn paint(ui: &mut Ui, s: &State, r: Rect) {
    let t = ui.t;
    ui.hit(r, Action::Swallow, "");
    // Title bar.
    ui.c.fill_rect(Rect::span(0.0, 200.0, 83.0, 105.0), t.browser_title);
    icons::draw(ui.c, Icon::ArrowRight, Rect::new(5.0, 87.0, 12.0, 14.0), t.text_dim);
    icons::draw(ui.c, Icon::ArrowUp, Rect::new(19.0, 87.0, 12.0, 14.0), t.text_dim);
    icons::draw(ui.c, Icon::Back, Rect::new(34.0, 87.0, 12.0, 14.0), t.text_dim);
    ui.c.text(Face::Ui, 12.0, 52.0, 89.0, t.text_dim, "Browser");
    // Tabs.
    ui.c.fill_rect(Rect::span(0.0, 200.0, 106.0, 141.0), t.browser_tabs);
    for (i, icon) in [Icon::Wave, Icon::Page, Icon::Speaker, Icon::Globe, Icon::Star].into_iter().enumerate() {
        let cx = 20.0 + 40.0 * i as f32;
        let rr = Rect::new(cx - 18.0, 108.0, 36.0, 30.0);
        ui.hit(rr, Action::Placeholder("browser tab"), "Browser tab");
        if i == 0 {
            ui.c.stroke_rrect(rr, 4.0, 1.5, t.browser_tab_outline);
        }
        icons::draw(ui.c, icon, Rect::new(cx - 9.0, 114.0, 18.0, 18.0), t.glyph.shade(-0.1));
    }
    // List.
    let list = Rect::span(13.0, 200.0, 142.0, r.bottom() - 28.0);
    ui.c.fill_rect(Rect::new(0.0, 142.0, 13.0, list.h), t.browser_tabs.shade(0.02));
    ui.c.fill_rect(list, t.browser_list);
    ui.c.push_clip(list);
    let cats: [Color; 10] = [
        t.cat_project,
        t.cat_recent,
        t.cat_plugin_db,
        t.cat_plugin_presets,
        t.cat_channel_presets,
        t.cat_mixer_presets,
        t.cat_plugin_db.mix(t.glyph, 0.4),
        t.cat_recent,
        t.cat_recent,
        t.cat_plugin_db,
    ];
    for (i, (label, icon, cat)) in ROWS.iter().enumerate() {
        // Measured text centers: the first seven rows are taller (spec §4).
        let center = [153.0, 176.0, 201.0, 228.0, 255.0, 282.0, 309.0].get(i).copied().unwrap_or_else(|| 334.0 + 23.0 * (i - 7) as f32) + 3.0;
        let row = Rect::new(13.0, center - 14.0, 188.0, 23.0);
        let hov = ui.hit(row, Action::BrowserRow(i), "");
        if i == s.browser_selected || hov {
            ui.c.fill_rect(row, t.browser_row_hi);
        }
        let c = if *cat == 255 { t.browser_folder } else { cats[usize::from(*cat).min(9)] };
        icons::draw(ui.c, *icon, Rect::new(19.0, row.y + 3.0, 17.0, 17.0), c);
        ui.c.text(Face::Ui, 14.8, 38.0, row.y + 3.0, c, label);
    }
    ui.c.pop_clip();
    // Footer.
    let fy = r.bottom() - 27.0;
    ui.c.fill_rect(Rect::new(0.0, fy, 201.0, 27.0), t.browser_footer);
    ui.c.fill_rrect(Rect::new(13.0, fy + 4.0, 41.0, 20.0), 3.0, t.sunken);
    ui.c.text_center(Face::Ui, 11.0, Rect::new(13.0, fy + 4.0, 41.0, 20.0), t.text_dim, "TAGS");
    ui.c.fill_rrect(Rect::new(57.0, fy + 4.0, 119.0, 20.0), 3.0, t.sunken);
    icons::draw(ui.c, Icon::Folder, Rect::new(158.0, fy + 7.0, 14.0, 14.0), t.text_faint);
    icons::draw(ui.c, Icon::Search, Rect::new(180.0, fy + 5.0, 17.0, 17.0), t.browser_search);
    // Splitter.
    ui.c.fill_rect(Rect::new(201.0, 83.0, 6.0, r.h), t.splitter);
    ui.c.fill_rect(Rect::new(207.0, 83.0, 2.0, r.h), t.splitter_dark);
}
