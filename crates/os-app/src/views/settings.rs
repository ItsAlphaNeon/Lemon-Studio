//! Settings window (OPTIONS > MIDI / Audio / General / File / Theme settings, Project settings, Project info,
//! Debugging log; F10 / F11): ten tabs of grouped controls. The controls that work today are listed in `LIVE` and
//! handled in `App::settings_act`; every other control is drawn greyed out and only says it is not available yet.
// spec: ui/settings.md

use std::collections::HashSet;

use os_ui::kit::lemon as lk;
use os_ui::kit::{self, Dir};
use os_ui::{Color, Font, Rect};

use crate::app::{Action, State, Ui};

/// What a click in the settings window does (`Action::Settings`).
#[derive(Debug, Clone, PartialEq)]
pub enum SAct {
    /// The window body (swallows clicks, wheel scrolls the page).
    Body,
    Close,
    Tab(usize),
    /// A working check (radio LED) by its label (`LIVE`).
    Check(&'static str),
    /// A collapsible group header by its title.
    Group(&'static str),
    /// A working button by its label (`LIVE`).
    Button(&'static str),
    /// Theme preset tile: 0 = default, n = `SettingsUi::themes[n - 1]`.
    Preset(usize),
}

/// Session state of the window.
#[derive(Debug, Default)]
pub struct SettingsUi {
    pub open: bool,
    pub tab: usize,
    /// Page scroll in pixels.
    pub scroll: f32,
    /// Groups whose open state differs from their default.
    pub folded: HashSet<&'static str>,
    /// Filled when the window opens: output devices, theme files (name, path), selected preset.
    pub devices: Vec<String>,
    pub device: String,
    /// Theme presets (name, `.flstheme` text): built-ins (spec ui/theme-presets.md), then the user's theme folder.
    pub themes: Vec<(String, String)>,
    pub preset: usize,
}

/// Tabs: label, text centre x, page title, group (0 system / 1 project / 2 info) (spec §2).
pub const TABS: [(&str, f32, &str, u8); 10] = [
    ("MIDI", 27.0, "MIDI input / output devices", 0),
    ("Audio", 82.0, "Audio devices / mixer", 0),
    ("General", 146.0, "System / miscellaneous", 0),
    ("File", 203.0, "Files & folders", 0),
    ("Theme", 257.0, "Look and feel", 0),
    ("Project", 337.0, "Current project", 1),
    ("Info", 392.0, "Project credits", 1),
    ("Debug", 462.0, "Debugging log", 2),
    ("Account", 529.0, "Account information", 2),
    ("About", 595.0, "About", 2),
];
pub const MIDI: usize = 0;
pub const AUDIO: usize = 1;
pub const GENERAL: usize = 2;
pub const FILE: usize = 3;

/// The one working File-tab check: opt in to the experimental FL native plugin host (spec
/// plugins/fruity-abi.md). Its label is the key for `SAct::Check`; `App::settings_act` acts on it.
pub const NATIVE_HOST_CHECK: &str = "Experimental: host installed native plugins";

/// The checks and buttons that work (by label, handled in `App::settings_act`). Every other check, combo, value box,
/// slider and button is not available yet: drawn greyed out, its hint and click say so.
pub const LIVE: [&str; 4] = [NATIVE_HOST_CHECK, "Manage plugins", "Load theme...", "Reset"];

fn live(key: &str) -> bool {
    LIVE.contains(&key)
}

pub const THEME: usize = 4;
pub const PROJECT: usize = 5;
pub const INFO: usize = 6;
pub const DEBUG: usize = 7;

/// OPTIONS menu label -> tab (spec §1).
pub fn tab_for_menu(label: &str) -> Option<usize> {
    Some(match label {
        "MIDI settings" => MIDI,
        "Audio settings" => AUDIO,
        "General settings" => GENERAL,
        "File settings" | "Manage plugins" => FILE,
        "Theme settings" => THEME,
        "Project general settings" => PROJECT,
        "Project info" => INFO,
        "Debugging log" => DEBUG,
        _ => return None,
    })
}

pub const W: f32 = 668.0;
const LABEL: Font = Font::new(13);
const TABS_B: f32 = 64.0;
const GX: f32 = 30.0;
const GW: f32 = 608.0;

/// The window rect: 668 x 900 at (626, 70) on a 1920x1080 screen, centred and shrunk to fit smaller screens.
pub fn rect(s: &State) -> Rect {
    let (sw, sh) = s.screen;
    let h = 900.0_f32.min(sh - 20.0).max(200.0);
    let y = 70.0_f32.min(sh - h - 10.0).max(0.0);
    Rect::new(((sw - W) / 2.0).max(0.0).round(), y, W, h)
}

/// One control row. Labels double as keys (unique per page).
pub enum Row {
    /// Check: label, default.
    Check(&'static str, bool),
    /// Two checks side by side (second at x 340).
    Check2(&'static str, bool, &'static str, bool),
    /// Disabled check (drawn dim).
    Off(&'static str, bool),
    /// Combo box: value, box width, label to its right.
    Combo(String, f32, &'static str),
    /// Red value box: value, label to its right.
    Value(String, &'static str),
    /// Label + red value box on the right side of the panel.
    ValueR(&'static str, String),
    /// Buttons in a row: (label, width).
    Buttons(Vec<(&'static str, f32)>),
    /// Slider: label, 0..1.
    Slider(&'static str, f32),
    /// List: column (title, x), rows of cells, visible rows.
    List(Vec<(&'static str, f32)>, Vec<Vec<String>>, usize),
    /// Plain text line (dim = small grey).
    Text(String, bool),
    /// Large text (About page).
    Big(String),
    /// Theme preset tiles (names; swatch colours).
    Presets(Vec<(String, [Color; 3])>, usize),
    /// Theme adjustment knobs and colour circles.
    ThemeKnobs,
    Gap(f32),
    /// Two controls on one line: left one, right one (right-hand column at x 304).
    Pair(Box<Row>, Box<Row>),
    /// A display that belongs to a feature not available yet (name for the hint): drawn greyed out.
    Dim(&'static str, Box<Row>),
}

pub struct Group {
    pub title: &'static str,
    pub note: &'static str,
    /// Has a fold chevron; `closed` = folded by default.
    pub fold: bool,
    pub closed: bool,
    pub rows: Vec<Row>,
}

fn pair(a: Row, b: Row) -> Row {
    Row::Pair(Box::new(a), Box::new(b))
}
fn g(title: &'static str, rows: Vec<Row>) -> Group {
    Group { title, note: "", fold: false, closed: false, rows }
}
fn gf(title: &'static str, closed: bool, rows: Vec<Row>) -> Group {
    Group { title, note: "", fold: true, closed, rows }
}
fn combo(v: &str, w: f32, label: &'static str) -> Row {
    Row::Combo(v.into(), w, label)
}
fn txt(v: impl Into<String>) -> Row {
    Row::Text(v.into(), false)
}
fn dim(name: &'static str, r: Row) -> Row {
    Row::Dim(name, Box::new(r))
}

/// The groups of page `tab` (spec §4). FL-only groups (cloud, remote app, licence) are left out.
pub fn page(tab: usize, s: &State) -> Vec<Group> {
    use Row::*;
    let st = &s.settings;
    match tab {
        MIDI => vec![
            g("Output", vec![
                dim("MIDI output devices", List(vec![("", 0.0)], vec![], 8)),
                pair(Check2("Send master sync", false, "Send all notes off", true), ValueR("Port", "---".into())),
                pair(Value("0".into(), "Master sync offset (milliseconds)"), combo("MIDI clock", 168.0, "Synchronization type")),
            ]),
            g("Input", vec![
                // The devices that are open: their notes play the selected channel (spec: ui/midi-input.md).
                List(vec![("Open input devices (notes play the selected channel)", 0.0)], s.midi_inputs.iter().map(|d| vec![d.clone()]).collect(), 9),
                pair(Check("Enable", true), ValueR("Port", "---".into())),
                combo("(generic controller)", 250.0, "Controller type"),
                pair(combo("Velocity", 100.0, "Link note on velocity to"), ValueR("Omni preview MIDI channel", "---".into())),
                pair(combo("Release", 100.0, "Link release velocity to"), ValueR("Song marker jump MIDI channel", "---".into())),
                pair(Check("Pickup (takeover mode)", false), ValueR("Performance mode MIDI channel", "---".into())),
                pair(Check("Auto accept detected controller", true), ValueR("Generator muting MIDI channel", "---".into())),
                pair(Check("Foot pedal controls note off", true), Check("Toggle on release", false)),
            ]),
            g("External sync", vec![
                pair(combo("Off", 168.0, "External clock sync"), ValueR("External sync offset (milliseconds)", "0".into())),
                txt("Status: off"),
            ]),
            g("", vec![Buttons(vec![("Refresh device list", 136.0), ("Update MIDI scripts", 136.0)])]),
        ],
        AUDIO => {
            let rate = crate::transport::Transport::device_rate().unwrap_or(44100);
            vec![
                g("Input / output", vec![
                    combo(&st.device, 360.0, "Output device"),
                    combo(&rate.to_string(), 66.0, "Sample rate (Hz)"),
                    Check("Auto close", false),
                    Text(format!("Status: open at {rate}Hz, shared mode (WASAPI)"), true),
                    List(vec![("Output devices", 0.0)], st.devices.iter().map(|d| vec![d.clone()]).collect(), 4),
                    Check2("Mix in buffer switch", false, "Triple buffer", false),
                    combo("Highest", 96.0, "Priority"),
                    Check("Safe overloads", true),
                    combo("Mixer", 96.0, "Playback tracking"),
                ]),
                g("CPU", vec![
                    Check2("Multithreaded generator processing", true, "Smart disable", true),
                    Check2("Multithreaded mixer processing", true, "Align tick lengths", false),
                ]),
                g("Mixer", vec![
                    combo("24-point sinc", 122.0, "Resampling quality"),
                    Value("---".into(), "Browser preview track"),
                    Value("---".into(), "Metronome track"),
                    Check("Play truncated notes on transport", true),
                    Check("Reset plugins on transport", false),
                ]),
            ]
        }
        GENERAL => vec![
            Group { note: " (requires a restart to take effect)", ..gf("Display", false, vec![
                combo("System", 64.0, "Main GUI scaling"),
                dim("Display PPI", txt("Define 1 inch / 2.54 cm or set display's PPI")),
                dim("Display PPI", Text("(96 PPI/advised scaling: 100%)".into(), true)),
                Slider("", 0.25),
                combo("Main", 64.0, "Pop-ups scaling"),
                combo("Main", 64.0, "Toolbars scaling"),
                combo("Off", 64.0, "Transparent windows"),
                combo("Entertain me", 126.0, "Animations"),
                Slider("Smoothing", 0.2),
                combo("Ultrasmooth", 126.0, "Animation refresh rate"),
                Check2("Force refreshes", true, "Transparent menus", true),
                Off("Thick lines", true),
                Check("High visibility", false),
                combo("Off", 146.0, "Colorful waveforms"),
            ]) },
            Group { note: " (requires a restart to take effect)", ..gf("Language", false, vec![
                combo("English (US) (en)", 176.0, "Language"),
                combo("English", 176.0, "Note names"),
                combo("Sharps - #", 96.0, "Black notes"),
                Buttons(vec![("Update languages", 138.0)]),
            ]) },
            gf("Input", false, vec![
                combo("Multi-button mouse", 136.0, "Optimize for"),
                Buttons(vec![("Manage shortcuts", 212.0)]),
                Check("Click-and-hold & special gesture functions", false),
                Check("Use both mouse buttons in step sequencer", false),
                Slider("Vertical scrolling speed", 0.5),
                Slider("Horizontal scrolling speed", 0.5),
                Check("Invert pencil buttons", false),
            ]),
            gf("Undo history", false, vec![
                Value("100".into(), "Maximum undo levels"),
                Check("Undo knob tweaks", true),
                Check("Alternate undo mode", true),
                Check("Put undone recordings in the recycle bin", true),
                Check("Show undo history in the browser when undoing changes", false),
                Check("Show recent changes at the top", false),
            ]),
            gf("Hints and warnings", false, vec![
                Buttons(vec![("Manage messages", 138.0)]),
                combo("100MB (default)", 110.0, "Project data size warning"),
            ]),
            gf("System", false, vec![Check("Associate project files with application", true)]),
            gf("Updates and privacy", false, vec![
                Check("Automatically check for updates", true),
                Check("Automatically download program updates", true),
                Check("Send usage data", false),
            ]),
            gf("Miscellaneous", false, vec![
                Check("Auto name channels", true),
                Check("Auto name effect slots", false),
                Check("Auto zip empty channels", false),
                Check("Auto select linked modules", true),
                Check("Auto zoom in piano roll", false),
                Check("Small scrollbars in editors", false),
                Check2("Detach all new plugins", false, "Open detached plugins with the main window", false),
                Check("Disable typing keyboard priority by default", false),
                Check("Alternate meter scale", false),
                Check("Show startup splash screen", true),
                Check("Restore previous state after solo", false),
                Check("Hide plugin window toolbar by default", true),
                Check("Silent startup", false),
                Check("Monophonic painting in piano roll", false),
                Check("Create automation clips at the playback position", true),
                Check("Select first channel with notes when the pattern changes", false),
                Check("Limit the number of open plugin editors", true),
                combo("Basic with limiter", 160.0, "Default template"),
                combo("Default template", 160.0, "Startup project"),
            ]),
            gf("Advanced", false, vec![
                Check("Don't limit windows to screen", false),
                Check("Fast sample preview", true),
                Check("Auto keep long audio on disk", true),
                Check2("Read sample tempo information", true, "Read sample root note", true),
                Check("Initialize controls automatically", true),
                Check("Force high performance power plan", true),
                Check("Threaded project sample loading", false),
                combo("System", 64.0, "Cursors scaling"),
                combo("Main", 64.0, "Legacy scaling"),
                combo("Show help in a web browser", 160.0, ""),
            ]),
        ],
        FILE => {
            let user = std::env::var("APPDATA").map(|d| format!("{d}\\OpenStudio\\")).unwrap_or_default();
            vec![
                g("Backup", vec![
                    pair(combo("Occasionally (every 10 minutes)", 336.0, "Autosave"), ValueR("Maximum", "100".into())),
                    combo("Save to the project data folder if there is one", 336.0, "Location"),
                ]),
                g("Plugins", vec![
                    Buttons(vec![("Manage plugins", 124.0)]),
                    Text(format!("{} instrument plugins found", s.hosted_generators.len()), true),
                    Check(NATIVE_HOST_CHECK, os_native::enabled()),
                ]),
                g("User data folder", vec![txt(user)]),
                g("Browser extra search folders", vec![dim("Browser extra search folders", List(vec![("Folder", 30.0), ("Browser name", 440.0)], vec![], 12))]),
                g("External tools", vec![
                    dim("External tools", List(vec![("File", 30.0), ("Name", 320.0)], vec![], 4)),
                    dim("External tools", txt("Script editor   C:\\Windows\\System32\\Notepad.exe")),
                ]),
                gf("Troubleshooting", true, vec![Buttons(vec![("Repair optional features", 124.0), ("Reset browser cache", 124.0)])]),
            ]
        }
        THEME => {
            let mut tiles = vec![("(Default)".to_string(), swatch(&os_theme::Palette::default()))];
            for (name, text) in &st.themes {
                tiles.push((name.clone(), swatch(&os_theme::fl::palette(&os_theme::FlTheme::parse(text)))));
            }
            vec![
                g("Presets", vec![Presets(tiles, st.preset)]),
                g("Adjustments", vec![ThemeKnobs]),
                g("Options", vec![Check("Light mode", true), Check("Audio and automation clips use note colors", false)]),
                g("", vec![Buttons(vec![("Load theme...", 110.0), ("Reset", 64.0), ("Save preset...", 100.0)])]),
            ]
        }
        PROJECT => vec![
            g("Data folder", vec![
                combo("Don't use a data folder", 306.0, ""),
                txt("(none)"),
                combo("When saving unsaved projects only", 444.0, "Ask for data folder"),
                Off("Copy used samples to the project folder", false),
            ]),
            g("Time settings", vec![
                combo(&s.project.ppq.to_string(), 44.0, "Timebase (PPQ)"),
                combo("Set as time signature", 196.0, ""),
                combo("4", 44.0, "Numerator"),
                combo("4", 44.0, "Denominator"),
            ]),
            gf("Advanced", true, vec![
                combo("Circular", 82.0, "Panning law"),
                Check("Play truncated notes in clips", true),
                Check("Fast declick for cut groups", false),
            ]),
            gf("Playlist", false, vec![Check2("Automatic crossfades", true, "Enable automatic crossfades in new projects", true)]),
        ],
        INFO => {
            let i = &s.project.info;
            let or = |v: &Option<String>| v.clone().unwrap_or_default();
            let clips: usize = s.project.arrangements.iter().map(|a| a.clips.len()).sum();
            let notes: usize = s.project.patterns.iter().map(|p| p.notes.len()).sum();
            let mut rows = vec![
                Text("Title".into(), true),
                Big(or(&i.title)),
                Text("Author".into(), true),
                Big(or(&i.author)),
                Text("Genre".into(), true),
                Big(or(&i.genre)),
                Text("Comments".into(), true),
            ];
            let comments = or(&i.comments);
            let lines: Vec<String> = comments.lines().map(String::from).collect();
            rows.push(List(vec![("", 0.0)], lines.into_iter().map(|l| vec![l]).collect(), 14));
            rows.push(Text("Web link".into(), true));
            rows.push(Big(or(&i.url)));
            rows.push(Gap(8.0));
            rows.push(txt(format!("Channels: {}, Mixer Tracks: {}", s.project.channels.len(), s.project.mixer.len())));
            rows.push(txt(format!("Arrangements: {}, PL clips: {clips}, Notes: {notes}", s.project.arrangements.len())));
            vec![g("Project credits", rows)]
        }
        DEBUG => {
            let mut lines = vec![
                format!("OpenStudio {}", env!("CARGO_PKG_VERSION")),
                format!("Project: {}", s.path.as_ref().map_or("(new project)".into(), |p| p.display().to_string())),
                format!("Saved by: {}", s.project.saved_by),
                format!("PPQ {}, tempo {:.3}", s.project.ppq, s.tempo),
                format!("Channels {}, patterns {}, mixer tracks {}", s.project.channels.len(), s.project.patterns.len(), s.project.mixer.len()),
                format!("Output device: {}", st.device),
                format!("Device rate: {:?}", crate::transport::Transport::device_rate()),
                format!("Hosted instruments: {}", s.hosted_generators.len()),
            ];
            lines.extend(s.hosted_generators.iter().map(|n| format!("  {n}")));
            if let Some(m) = &s.message {
                lines.push(format!("Last message: {m}"));
            }
            vec![g("", vec![List(vec![("", 0.0)], lines.into_iter().map(|l| vec![l]).collect(), 40)])]
        }
        8 => vec![g("Licence", vec![
            Big("OpenStudio needs no account.".into()),
            txt("It is free software under the GNU General Public License, version 3."),
            txt("No registration, no online services, no usage data."),
        ])],
        _ => vec![g("", vec![
            Gap(20.0),
            Big(format!("OpenStudio {}", env!("CARGO_PKG_VERSION"))),
            txt("A digital audio workstation written in Rust."),
            Gap(10.0),
            txt("Licence: GNU General Public License v3."),
            txt("Fonts: Cuprum (SIL Open Font License). Icons: Tabler Icons (MIT)."),
        ])],
    }
}

/// Three colours that represent a palette on its preset tile: surface, highlight, selection.
fn swatch(p: &os_theme::Palette) -> [Color; 3] {
    [p.s(0x2b2d30), p.h(0xffaf65), p.a(0x9ad04a)]
}

fn row_h(r: &Row) -> f32 {
    match r {
        Row::Check(..) | Row::Check2(..) | Row::Off(..) => 21.0,
        Row::Combo(..) | Row::Value(..) | Row::ValueR(..) => 27.0,
        Row::Buttons(..) => 36.0,
        Row::Slider(..) => 28.0,
        Row::List(c, _, n) => (if c.iter().any(|c| !c.0.is_empty()) { 18.0 } else { 0.0 }) + 17.0 * *n as f32 + 10.0,
        Row::Text(_, dim) => if *dim { 16.0 } else { 20.0 },
        Row::Big(_) => 28.0,
        Row::Presets(t, _) => ((t.len() + 2) / 3) as f32 * 64.0 + 6.0,
        Row::ThemeKnobs => 140.0,
        Row::Gap(h) => *h,
        Row::Pair(a, b) => row_h(a).max(row_h(b)),
        Row::Dim(_, r) => row_h(r),
    }
}

fn group_h(gr: &Group, open: bool) -> f32 {
    let head = if gr.title.is_empty() { 8.0 } else { 30.0 };
    if !open {
        return head;
    }
    head + gr.rows.iter().map(row_h).sum::<f32>() + 8.0
}

fn group_open(st: &SettingsUi, gr: &Group) -> bool {
    !gr.fold || (!gr.closed ^ st.folded.contains(gr.title))
}

/// Total content height of the current page (for scrolling).
pub fn content_h(s: &State) -> f32 {
    page(s.settings.tab, s).iter().map(|gr| group_h(gr, group_open(&s.settings, gr)) + 6.0).sum::<f32>() + 16.0
}

/// Visible page height.
pub fn page_h(s: &State) -> f32 {
    rect(s).h - TABS_B - 4.0
}

pub fn paint(ui: &mut Ui, s: &State) {
    let st = &s.settings;
    let r = rect(s);
    let t = ui.t;
    ui.hit(r, Action::Settings(SAct::Body), "");
    // Shadow, frame, title bar.
    let lemon = super::lemon(ui);
    if lemon {
        // Lemon (spec: ui/lemon-design.md §10): rounded window, pill tabs, rounded groups and square checks.
        ui.c.fill_rrect(r.outset(4.0), lk::r_panel(t) + 4.0, Color::rgb(0).with_alpha(50));
        lk::outlined(ui.c, r, lk::r_panel(t), t.s(0x1c1e21), t.s(0x2b2d30));
        ui.c.fill_rrect(Rect::new(r.x + 1.0, r.y + 1.0, r.w - 2.0, TABS_B - 1.0), lk::r_panel(t) - 1.0, t.s(0x25272a));
        ui.c.fill_rect(Rect::new(r.x + 1.0, r.y + TABS_B - lk::r_panel(t), r.w - 2.0, lk::r_panel(t)), t.s(0x25272a));
    } else {
        ui.c.fill_rrect(r.outset(4.0), 6.0, Color::rgb(0).with_alpha(50));
        ui.c.fill_rect(r, t.s(0x1c1e21));
        ui.c.fill_rect(r.inset(1.0), t.s(0x2b2d30));
        ui.c.fill_rect(Rect::new(r.x + 1.0, r.y + 1.0, r.w - 2.0, TABS_B - 1.0), t.s(0x25272a));
    }
    ui.c.icon(&os_ui::tabler::SETTINGS, Rect::new(r.x + 7.0, r.y + 7.0, 16.0, 16.0), t.text_dim);
    let (_, _, title, _) = TABS[st.tab.min(9)];
    ui.c.text(Font::new(13), r.x + 28.0, r.y + 7.0, t.text, &format!("Settings - {title}"));
    let x = Rect::new(r.right() - 26.0, r.y + 5.0, 20.0, 20.0);
    let hov = ui.hit(x, Action::Settings(SAct::Close), "Close");
    ui.c.icon(&os_ui::tabler::X, x.inset(3.0), if hov { t.glyph } else { t.text });
    // Tabs (spec §2): active text coloured by group.
    let font = Font::new(13);
    for (i, &(label, cx, _, grp)) in TABS.iter().enumerate() {
        let tw = ui.c.text_width(font, label);
        let tr = Rect::span(r.x + cx - tw / 2.0 - 10.0, r.x + cx + tw / 2.0 + 10.0, r.y + 34.0, r.y + TABS_B);
        let hov = ui.hit(tr, Action::Settings(SAct::Tab(i)), TABS[i].2);
        let on = i == st.tab;
        let body = if on { t.s(0x1d1e1f) } else { t.s(0x222425) };
        if lemon {
            let pill = Rect::new(tr.x, tr.y + 2.0, tr.w, tr.h - 6.0);
            if on {
                lk::lit(ui.c, pill, lk::r_btn(t), hov);
            } else {
                ui.c.fill_rrect(pill, lk::r_btn(t), if hov { t.button_hi } else { body });
            }
        } else {
            ui.c.fill_rect(tr, body);
            for k in 0..7 {
                // slanted right edge
                let k = k as f32;
                ui.c.fill_rect(Rect::new(tr.right() + k, tr.y + 2.0 + k * 4.0, 1.0, tr.h - 2.0 - k * 4.0), body);
            }
        }
        let col = if on && lemon {
            lk::ink_on_lemon()
        } else if on {
            match grp {
                0 => t.a(0x2da2ff),
                1 => t.h(0xfdc13d),
                _ => t.a(0xbeff3c),
            }
        } else if hov {
            t.glyph
        } else {
            t.text
        };
        let lh = ui.c.line_height(font);
        let ty = if lemon { tr.cy() - 2.0 } else { tr.cy() };
        ui.c.text(font, tr.x + 10.0, (ty - lh / 2.0).round(), col, label);
    }
    ui.c.fill_rect(Rect::new(r.x + 1.0, r.y + TABS_B, r.w - 2.0, 1.0), t.s(0x1c1e21));
    // Page.
    let area = Rect::new(r.x + 1.0, r.y + TABS_B + 1.0, r.w - 2.0, r.h - TABS_B - 3.0);
    let total = content_h(s);
    let scroll = st.scroll.min((total - area.h).max(0.0)).max(0.0);
    ui.c.push_clip(area);
    let mut y = area.y + 7.0 - scroll;
    for gr in page(st.tab, s) {
        let open = group_open(st, &gr);
        let h = group_h(&gr, open);
        if y + h > area.y && y < area.bottom() {
            paint_group(ui, s, &gr, Rect::new(r.x + GX, y, GW, h), open, area);
        }
        y += h + 6.0;
    }
    ui.c.pop_clip();
    if total > area.h {
        let track = Rect::new(area.right() - 10.0, area.y + 4.0, 7.0, area.h - 8.0);
        kit::scrollbar(ui.c, t, track, true, scroll, area.h, total, kit::State::NORMAL);
    }
}

fn paint_group(ui: &mut Ui, s: &State, gr: &Group, r: Rect, open: bool, clip: Rect) {
    let t = ui.t;
    let st = &s.settings;
    let lemon = super::lemon(ui);
    if lemon {
        lk::outlined(ui.c, r, lk::r_panel(t) - 2.0, t.s(0x1c1e21), t.s(0x25272a));
    } else {
        ui.c.fill_rect(r, t.s(0x1c1e21));
        ui.c.fill_rect(r.inset(1.0), t.s(0x25272a));
    }
    let mut y = r.y + 8.0;
    if !gr.title.is_empty() {
        let f = Font::new(13).bold();
        let tw = ui.c.text_width(f, gr.title);
        let hdr = Rect::new(r.x + 1.0, r.y + 1.0, tw + 14.0, 24.0);
        if !lemon {
            ui.c.fill_rect(hdr, t.s(0x222427));
            for k in 0..6 {
                let k = k as f32;
                ui.c.fill_rect(Rect::new(hdr.right() + k, hdr.y, 1.0, hdr.h - k * 4.0), t.s(0x222427));
            }
        }
        ui.c.text(f, r.x + if lemon { 8.0 } else { 4.0 }, r.y + 4.0, if lemon { t.accent } else { t.text }, gr.title);
        if !gr.note.is_empty() {
            ui.c.text(Font::new(13), r.x + if lemon { 8.0 } else { 4.0 } + tw, r.y + 4.0, t.text_dim, gr.note);
        }
        if gr.fold {
            let bar = Rect::new(r.x, r.y, r.w, 26.0);
            if clip.contains(bar.cx(), bar.cy()) {
                ui.hit(bar, Action::Settings(SAct::Group(gr.title)), if open { "Fold" } else { "Unfold" });
            }
            kit::chevron(ui.c, r.right() - 10.0, r.y + 13.0, 4.0, if open { Dir::Up } else { Dir::Down }, t.text_dim);
        }
        y = r.y + 30.0;
    }
    if !open {
        return;
    }
    for row in &gr.rows {
        let h = row_h(row);
        if y + h > clip.y && y < clip.bottom() {
            paint_row(ui, st, gr.title, row, r, y, h);
        }
        y += h;
    }
}

/// A check showing `on`; one that is not `enabled` (not in `LIVE`) is greyed out.
fn check(ui: &mut Ui, key: &'static str, on: bool, x: f32, cy: f32, enabled: bool) {
    let t = ui.t;
    let tw = ui.c.text_width(Font::new(13), key);
    let hr = Rect::new(x - 8.0, cy - 9.0, tw + 26.0, 18.0);
    let hov = enabled && ui.hit(hr, Action::Settings(SAct::Check(key)), "");
    if !enabled {
        ui.off(hr, key);
    }
    // Round LED (spec §3): orange when on. Lemon: a rounded square check, lit with a tick when on.
    let lemon = super::lemon(ui);
    if !lemon {
        ui.c.fill_circle(x, cy, 6.5, t.s(0x141516));
    }
    if lemon {
        let b = Rect::new(x - 6.5, cy - 6.5, 13.0, 13.0);
        if on {
            lk::lit(ui.c, b, lk::r_small(t), hov);
            ui.c.polyline(&[(x - 3.5, cy), (x - 1.0, cy + 2.5), (x + 3.5, cy - 2.5)], 1.6, lk::ink_on_lemon());
        } else {
            lk::outlined(ui.c, b, lk::r_small(t), t.s(0x141516), if hov { t.s(0x4a4c4e) } else { t.s(0x36393a) });
        }
    } else if on {
        ui.c.fill_circle(x, cy, 5.5, t.h(0x952b15));
        ui.c.fill_circle(x, cy, 4.2, t.h(0xf09672));
        ui.c.fill_circle(x - 1.0, cy - 1.2, 1.6, t.h(0xffd2bc));
    } else {
        ui.c.fill_circle(x, cy, 5.5, if hov { t.s(0x6a6c6e) } else { t.s(0x58595a) });
        ui.c.fill_circle(x, cy, 4.2, t.s(0x36393a));
    }
    let col = if hov { t.glyph } else { t.text };
    let lh = ui.c.line_height(Font::new(13));
    ui.c.text(Font::new(13), x + 11.0, (cy - lh / 2.0).round(), col, key);
    if !enabled {
        grey(ui, hr);
    }
}

/// Greys out what was just drawn in `r` on a group body.
fn grey(ui: &mut Ui, r: Rect) {
    let bg = ui.t.s(0x25272a);
    ui.veil(r, bg);
}

/// The hint name of a control: its label, else its group's title.
fn name_of(label: &'static str, group: &'static str) -> &'static str {
    if !label.is_empty() {
        label
    } else if !group.is_empty() {
        group
    } else {
        "This option"
    }
}

fn paint_row(ui: &mut Ui, st: &SettingsUi, group: &'static str, row: &Row, g: Rect, y: f32, h: f32) {
    paint_row_at(ui, st, group, row, g, y, h, 0.0)
}

#[allow(clippy::too_many_arguments)]
fn paint_row_at(ui: &mut Ui, st: &SettingsUi, group: &'static str, row: &Row, g: Rect, y: f32, h: f32, dx: f32) {
    let t = ui.t;
    let f = LABEL;
    let x0 = g.x + 12.0 + dx;
    let cy = y + h / 2.0;
    let lh = ui.c.line_height(f);
    let ty = (cy - lh / 2.0).round();
    match row {
        // A check that is not available yet shows its default, greyed out.
        Row::Check(k, d) => check(ui, k, *d, x0 + 10.0, cy, live(k)),
        Row::Off(k, d) => check(ui, k, *d, x0 + 10.0, cy, false),
        Row::Check2(a, da, b, db) => {
            check(ui, a, *da, g.x + 22.0, cy, live(a));
            check(ui, b, *db, g.x + 324.0, cy, live(b));
        }
        // Combos and value boxes: none works yet. A combo keeps showing its value (the output device, the sample
        // rate, the project's PPQ), greyed out.
        Row::Combo(v, w, label) => {
            let b = Rect::new(x0, y + 3.0, *w, 21.0);
            ui.off(b, name_of(label, group));
            kit::combo(ui.c, t, b, v, f, kit::State::NORMAL);
            let lw = ui.c.text(f, b.right() + 8.0, ty, t.text, label);
            grey(ui, Rect::new(b.x, b.y, b.w + 8.0 + lw, b.h));
        }
        Row::Value(v, label) => {
            let b = Rect::new(x0, y + 3.0, 38.0, 21.0);
            ui.off(b, name_of(label, group));
            value_red(ui, b, v);
            let lw = ui.c.text(f, b.right() + 8.0, ty, t.text, label);
            grey(ui, Rect::new(b.x, b.y, b.w + 8.0 + lw, b.h));
        }
        Row::ValueR(label, v) => {
            let b = Rect::new(g.right() - 52.0, y + 3.0, 42.0, 21.0);
            ui.off(b, name_of(label, group));
            value_red(ui, b, v);
            ui.c.text_right(f, b.x - 8.0, ty, t.text, label);
            let lw = ui.c.text_width(f, label);
            grey(ui, Rect::span(b.x - 8.0 - lw, b.right(), b.y, b.bottom()));
        }
        Row::Buttons(bs) => {
            let mut x = x0;
            for &(label, w) in bs {
                let b = Rect::new(x, y + 4.0, w, 28.0);
                let on = live(label);
                let hov = on && ui.hit(b, Action::Settings(SAct::Button(label)), "");
                if !on {
                    ui.off(b, label);
                }
                if super::lemon(ui) {
                    lk::outlined(ui.c, b, lk::r_btn(t), t.s(0x161819), lk::face(t.s(0x2a2d2f), kit::State::hover(hov)));
                } else {
                    kit::bordered(ui.c, b, 2.0, t.s(0x161819), kit::face(t.s(0x212425), kit::State::hover(hov)));
                }
                ui.c.text_center(Font::new(13), b, if hov { t.glyph } else { t.text }, label);
                if !on {
                    grey(ui, b);
                }
                x += w + 8.0;
            }
        }
        Row::Slider(label, v) => {
            let tr = Rect::new(x0, cy - 2.0, 150.0, 4.0);
            if super::lemon(ui) {
                ui.c.fill_rrect(tr, 2.0, t.s(0x161819));
                ui.c.fill_rrect(Rect::new(tr.x, tr.y, tr.w * v, tr.h), 2.0, t.accent);
            } else {
                ui.c.fill_rect(tr, t.s(0x161819));
                ui.c.fill_rect(Rect::new(tr.x, tr.y, tr.w * v, tr.h), t.h(0xc8312e));
            }
            let hx = tr.x + tr.w * v;
            ui.c.fill_circle(hx, cy, 8.0, t.s(0x111213));
            ui.c.fill_circle(hx, cy, 7.0, t.s(0x4a4d50));
            ui.c.fill_rect(Rect::new(hx - 0.5, cy - 6.0, 1.0, 5.0), t.s(0x1a1b1c));
            let lw = ui.c.text(f, tr.right() + 12.0, ty, t.text, label);
            let r = Rect::span(tr.x - 9.0, tr.right() + 14.0 + lw, cy - 9.0, cy + 9.0);
            ui.off(r, name_of(label, group));
            grey(ui, r);
        }
        Row::List(cols, rows, n) => {
            let b = Rect::new(x0 - 4.0, y + 2.0, g.w - 16.0, h - 6.0);
            if super::lemon(ui) {
                lk::outlined(ui.c, b, lk::r_btn(t), t.s(0x161819), t.s(0x222527));
            } else {
                ui.c.fill_rect(b, t.s(0x161819));
                ui.c.fill_rect(b.inset(1.0), t.s(0x222527));
            }
            let mut ry = b.y + 2.0;
            if cols.iter().any(|c| !c.0.is_empty()) {
                for (c, cx) in cols {
                    ui.c.text(Font::new(13), b.x + 6.0 + cx, ry, t.text_dim, c);
                }
                ry += 18.0;
            }
            for (i, cells) in rows.iter().take(*n).enumerate() {
                let rr = Rect::new(b.x + 2.0, ry + i as f32 * 17.0, b.w - 4.0, 17.0);
                if i % 2 == 1 {
                    ui.c.fill_rect(rr, t.s(0x26292b));
                }
                for (c, (_, cx)) in cells.iter().zip(cols.iter()) {
                    ui.c.text_clip(Font::new(13), rr.x + 4.0 + cx, rr.y + 1.0, rr.w - cx - 8.0, t.text, c);
                }
            }
        }
        Row::Text(v, dim) => {
            ui.c.text(if *dim { Font::new(13) } else { f }, x0, ty, if *dim { t.text_dim } else { t.text }, v);
        }
        Row::Big(v) => {
            let b = Rect::new(x0 - 4.0, y + 2.0, g.w - 16.0, 24.0);
            ui.c.fill_rect(b, t.s(0x1d1f21));
            let lh = ui.c.line_height(Font::new(14));
            ui.c.text_clip(Font::new(14), b.x + 6.0, (b.cy() - lh / 2.0).round(), b.w - 12.0, t.text, v);
        }
        Row::Presets(tiles, sel) => {
            for (i, (name, sw)) in tiles.iter().enumerate() {
                let (col, rowi) = ((i % 3) as f32, (i / 3) as f32);
                let b = Rect::new(x0 + col * 196.0, y + 4.0 + rowi * 64.0, 186.0, 56.0);
                let hov = ui.hit(b, Action::Settings(SAct::Preset(i)), "Load this theme");
                ui.c.fill_rect(b, if i == *sel { t.a(0x2d7cd0) } else if hov { t.s(0x5a5c5e) } else { t.s(0x161819) });
                let inner = b.inset(2.0);
                ui.c.fill_rect(Rect::new(inner.x, inner.y, inner.w, 30.0), sw[0]);
                ui.c.fill_rect(Rect::new(inner.x + 8.0, inner.y + 10.0, 40.0, 10.0), sw[1]);
                ui.c.fill_rect(Rect::new(inner.x + 54.0, inner.y + 10.0, 24.0, 10.0), sw[2]);
                ui.c.fill_rect(Rect::new(inner.x, inner.y + 30.0, inner.w, inner.h - 30.0), t.s(0x1d1e1f));
                let nr = Rect::new(inner.x, inner.y + 30.0, inner.w, inner.h - 30.0);
                ui.c.text_center(Font::new(13).bold(), nr, t.text, name);
            }
        }
        Row::ThemeKnobs => {
            for (i, k) in ["Hue", "Saturation", "Brightness", "Contrast", "Text"].iter().enumerate() {
                let cx = x0 + 28.0 + i as f32 * 61.0;
                kit::knob(ui.c, t, cx, y + 24.0, 14.0, 0.5, true, kit::KnobLook::Ring, kit::State::NORMAL);
                ui.c.text_center(Font::new(13), Rect::new(cx - 30.0, y + 42.0, 60.0, 18.0), t.text, k);
            }
            let cols = [t.a(0xa6d84a), t.h(0xfb9244), t.led(0x7cd317), t.h(0xe8502a), t.s(0x3a3c3e), t.h(0xf0a040), t.a(0x8fa8c8), t.tx(0xcfd1d2)];
            for (i, k) in ["Selection", "Highlight", "Mute", "Option", "Steps", "Meters", "Waves", "Text"].iter().enumerate() {
                let cx = x0 + 28.0 + i as f32 * 61.0;
                ui.c.fill_circle(cx, y + 92.0, 16.0, t.s(0x111213));
                ui.c.fill_circle(cx, y + 92.0, 14.5, cols[i]);
                ui.c.text_center(Font::new(13), Rect::new(cx - 30.0, y + 110.0, 60.0, 18.0), t.text, k);
            }
            let r = Rect::new(x0, y, g.w - 24.0, 130.0);
            ui.off(r, "Theme adjustments");
            grey(ui, r);
        }
        Row::Gap(_) => {}
        Row::Pair(a, b) => {
            paint_row_at(ui, st, group, a, g, y, h, 0.0);
            paint_row_at(ui, st, group, b, g, y, h, 304.0);
        }
        Row::Dim(name, r) => {
            paint_row_at(ui, st, group, r, g, y, h, dx);
            let b = Rect::span(x0 - 4.0, g.right() - 8.0, y, y + h);
            ui.off(b, name);
            grey(ui, b);
        }
    }
}

fn value_red(ui: &mut Ui, b: Rect, v: &str) {
    let t = ui.t;
    if super::lemon(ui) {
        // Lemon: number fields are the coloured dark with lemon digits.
        use os_theme::design::lemon as brand;
        lk::outlined(ui.c, b, lk::r_small(t), brand::COLORED_DARK.shade(-0.25), brand::COLORED_DARK);
        ui.c.text_center(Font::new(15), b, brand::HIGHLIGHT, v);
        return;
    }
    kit::bordered(ui.c, b, 3.0, t.h(0x5a2229), t.h(0x863c44));
    ui.c.text_center(Font::new(15), b, t.tx(0xf2e4e6), v);
}
