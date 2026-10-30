// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! What a Launchkey MK4's controls do in Lemon Studio, and what its lights and screen show. Runs on the UI thread, from
//! `App::midi_tick`: the port's messages first, then the lights and displays brought up to date with the project.
// spec: controllers/launchkey-mk4.md

use std::time::{Duration, Instant};

use super::launchkey::{self as lk, button, display, Colour, Encoders, Faders, In, Led, Model, Pads, Sticky, Text};
use super::Port;
use crate::app::{App, State, Win};
use crate::control::command::Region;
use crate::knobs::KnobId;
use crate::record::{Extra, Opt, RecCmd};
use crate::step_edit::Src;

/// A held arrow repeats after 6 ticks; banks then step every 6 ticks, the selected mixer track every 2. A tick is taken
/// as 20 ms, a guess (spec: controllers/launchkey-mk4.md "Mixer bank (Track ◀ / ▶ without Shift)").
const REPEAT_DELAY: Duration = Duration::from_millis(6 * 20);
const REPEAT_SLOW: Duration = Duration::from_millis(6 * 20);
const REPEAT_FAST: Duration = Duration::from_millis(2 * 20);
/// The master fader's display is written again 5 ticks after a move, with the settled value.
const MASTER_AGAIN: Duration = Duration::from_millis(5 * 20);
/// Encoder turns closer together than this are one undo step.
const GESTURE: Duration = Duration::from_secs(1);
/// What the stationary display shows outside the Transport layout.
const NAME: &str = "Lemon Studio";
/// Mixer tracks and channels the faders, encoders and pads take at a time.
const BANK: usize = 8;
/// Parameters with up to this many steps move a step per encoder detent.
const MAX_STEPPED: u32 = 32;
/// How long Plugin mode's parameter texts are shown before the plugin is read again (each read holds the plugin, which
/// the audio thread may be waiting for); a turn reads it at once.
const PLUGIN_REREAD: Duration = Duration::from_millis(250);

/// The Mixer encoder layout's pages.
const MIXER_PAGES: [&str; 5] = ["Channel Volume", "Channel Pan", "Mixer Volume", "Mixer Pan", "Mixer EQ"];

/// The keyboard as the driver keeps it.
pub struct Drive {
    pub port: Port,
    lights: lk::Lights,
    screen: lk::Screen,
    /// The keyboard confirmed DAW mode.
    daw: bool,
    pads: Option<Pads>,
    encoders: Option<Encoders>,
    faders: Option<Faders>,
    shift: bool,
    /// Buttons held, with when they next repeat (the arrows) and whether the press counted.
    held: Vec<(u8, Instant)>,
    /// The first mixer track of the faders' and Mixer pages' bank (a multiple of 8).
    mixer_bank: usize,
    /// The channel rack bank: the pads show rows 8·bank… (in the rack's current filter).
    channel_bank: usize,
    mixer_page: usize,
    /// Fader buttons arm (true) or mute.
    arm: bool,
    /// Pads held in the DAW layout: pad, rack row.
    pads_held: Vec<(u8, usize)>,
    /// The plugin window the encoders opened (a rack row), closed again when the encoders move on.
    opened: Option<usize>,
    /// Plugin mode's page, and the instance it is of (a new one starts on the first page).
    plugin_page: usize,
    plugin_key: Option<usize>,
    /// Transport encoder 5's turns toward the next or previous marker.
    marker_turns: i32,
    master_again: Option<Instant>,
    /// Faders whose next move first takes over, by fader (8: master).
    rearm: [bool; 9],
    /// A fader's last position and whether it has caught the value it controls (Soft takeover).
    takeover: [(Option<f64>, bool); 9],
    sticky: [Option<Sticky>; 8],
    /// Each encoder's last turn: when, and what it turned (a gesture's first turn makes the undo step).
    last_turn: [Option<(Instant, Target)>; 8],
    /// A stepped parameter's turning not yet enough for a step.
    carry: [f64; 8],
    /// The tempo the Transport layout last showed.
    tempo_shown: Option<f64>,
    /// Plugin mode's value texts as last read: when, of which instance and page.
    plugin_shown: Option<(Instant, usize, usize, [String; 8])>,
}

/// What an encoder or fader turns.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Target {
    Knob(KnobId),
    Fader(u16),
    Param(usize, usize),
    Tempo,
}

impl Drive {
    fn new(port: Port) -> Drive {
        Drive {
            port,
            lights: lk::Lights::default(),
            screen: lk::Screen::default(),
            daw: false,
            pads: None,
            encoders: None,
            faders: None,
            shift: false,
            held: Vec::new(),
            mixer_bank: 0,
            channel_bank: 0,
            mixer_page: 0,
            arm: true,
            pads_held: Vec::new(),
            opened: None,
            plugin_page: 0,
            plugin_key: None,
            marker_turns: 0,
            master_again: None,
            rearm: [true; 9],
            takeover: [(None, false); 9],
            sticky: [None; 8],
            last_turn: [None; 8],
            carry: [0.0; 8],
            tempo_shown: None,
            plugin_shown: None,
        }
    }

    fn model(&self) -> Model {
        self.port.model
    }

    fn send(&mut self, m: &[u8]) {
        self.port.send(m);
    }

    /// Takes the keyboard into DAW mode, its drum pads and the starting layouts (which it reports back).
    fn start(&mut self) {
        self.lights.forget();
        self.screen.forget();
        (self.daw, self.pads, self.encoders, self.faders, self.shift) = (false, None, None, None, false);
        (self.mixer_bank, self.channel_bank, self.mixer_page, self.arm) = (0, 0, 0, true);
        self.pads_held.clear();
        self.held.clear();
        self.rearm = [true; 9];
        self.send(&lk::DAW_ON);
        self.send(&lk::DRUMS_OWNED);
        for m in lk::START_LAYOUTS {
            if m[1] != 0x1f || matches!(self.model(), Model::Mk4 { .. }) {
                self.send(&m);
            }
        }
    }

    /// Everything dark, the stationary display back to the name, and the keyboard back to its own (standalone) mode.
    fn stop(&mut self) {
        self.lights.all_off();
        if let Some(m) = self.lights.flush(self.model()) {
            self.send(&m);
        }
        self.send(&lk::DRUMS_RELEASED);
        self.send(&lk::DAW_OFF);
        self.daw = false;
    }
}

/// The colour a channel or mixer track without its own shows on the LEDs.
const DEFAULT_COLOUR: os_model::Rgb = os_model::Rgb { r: 0x5f, g: 0x6f, b: 0x7f };

/// Pan 0..1 as the screen shows it: "C", "25L", "100R".
fn pan_text(v: f64) -> String {
    let p = v * 2.0 - 1.0;
    if p.abs() < 1e-6 {
        "C".into()
    } else {
        format!("{:.0}{}", p.abs() * 100.0, if p < 0.0 { "L" } else { "R" })
    }
}

/// A gain as the screen shows it: "-3.5 dB", "-Inf dB".
fn db_text(gain: f64) -> String {
    let db = 20.0 * gain.max(1e-12).log10();
    if db < -200.0 { "-Inf dB".into() } else { format!("{db:.1} dB") }
}

/// The text after a knob text's label ("EQ band 1 (low shelf) level: 0.0dB" → "0.0dB").
fn value_part(s: String) -> String {
    match s.split_once(": ") {
        Some((_, v)) => v.to_string(),
        None => s,
    }
}

/// The mixer tracks the banks step over: master and the inserts.
fn track_count(s: &State) -> usize {
    usize::from(crate::mixer::inserts(s)) + 1
}

/// The rack rows the pads and channel pages step over (the rack's filter).
fn rows(s: &State) -> Vec<usize> {
    crate::rack::shown(s)
}

/// A plugin parameter map: the parameters encoders 1–8 turn, by the plugin's parameter index, with a name (the
/// plugin's own when `None`) and a centre the value stops at.
struct Slot {
    index: usize,
    name: Option<&'static str>,
    centre: Option<(f64, f64)>,
    /// Names of the value's ranges, by lower bound.
    regions: &'static [(f64, &'static str)],
}

const fn p(index: usize, name: &'static str) -> Option<Slot> {
    Some(Slot { index, name: Some(name), centre: None, regions: &[] })
}

const fn c(index: usize, name: &'static str, centre: f64) -> Option<Slot> {
    Some(Slot { index, name: Some(name), centre: Some((centre, 0.1)), regions: &[] })
}

const OSC_SHAPES: &[(f64, &str)] = &[
    (0.0, "Sine Wave"),
    (1.0 / 12.0, "Triangle Wave"),
    (3.0 / 12.0, "Square Wave"),
    (5.0 / 12.0, "Saw Wave"),
    (7.0 / 12.0, "Rounded Saw Wave"),
    (9.0 / 12.0, "Noise Wave"),
    (11.0 / 12.0, "Custom Wave"),
];

const fn shape(index: usize, name: &'static str) -> Option<Slot> {
    Some(Slot { index, name: Some(name), centre: None, regions: OSC_SHAPES })
}

/// The maps of FL's plugins that Lemon Studio plays (its recreations keep FL's parameter order), by FL's name.
const MAPS: &[(&str, [Option<Slot>; 8])] = &[
    ("3x Osc", [shape(1, "Osc 1 Shape"), shape(8, "Osc 2 Shape"), shape(15, "Osc 3 Shape"), p(9, "Osc 2 Coarse"), p(16, "Osc 3 Coarse"), None, None, None]),
    ("Fruity Reeverb 2", [p(0, "Low Cut"), p(1, "High Cut"), p(3, "Room Size"), p(4, "Diffusion"), p(13, "Mod Speed"), p(14, "Mod Depth"), p(10, "Dry Level"), p(12, "Wet Level")]),
    ("Fruity Filter", [p(0, "Cutoff Frequency"), p(1, "Resonance"), p(2, "Low Pass"), p(3, "Band Pass"), p(4, "High Pass"), p(5, "X2"), None, None]),
    ("Fruity Fast Dist", [p(0, "Pre Gain"), p(1, "Threshold"), p(2, "Type"), p(3, "Mix"), p(4, "Post Gain"), None, None, None]),
    ("Fruity Delay 3", [p(0, "Input"), p(4, "Delay Time"), p(1, "Delay Model"), c(14, "Feedback Level", 0.8), p(16, "Feedback Cutoff"), p(15, "Filter Type"), p(11, "Diffusion Level"), p(18, "Distortion Level")]),
    ("Fruity soft clipper", [p(0, "Threshold"), c(1, "Gain", 0.5), None, None, None, None, None, None]),
    ("Fruity Limiter", [c(0, "Gain", 0.5), p(1, "Saturation"), c(2, "Limiter Ceiling", 0.5), p(3, "Limiter Attack"), p(5, "Limiter Release"), p(7, "Limiter Sustain"), p(15, "Noise Gain"), p(16, "Noise Threshold")]),
    ("Transient Processor", [p(6, "Split Freq"), p(0, "Attack"), p(1, "Drive"), p(7, "Split Balance"), p(2, "Release"), c(3, "Gain", 0.5), p(4, "Attack Shape"), p(5, "Release Shape")]),
    ("Fruity Parametric EQ 2", [c(0, "Band 1 Level", 0.5), p(7, "Band 1 Freq"), c(2, "Band 3 Level", 0.5), p(9, "Band 3 Freq"), c(4, "Band 5 Level", 0.5), p(11, "Band 5 Freq"), c(6, "Band 7 Level", 0.5), p(13, "Band 7 Freq")]),
    ("Fruity Chorus", [p(0, "Delay"), p(1, "Depth"), p(2, "Stereo"), p(3, "LFO 1 Freq"), p(4, "LFO 2 Freq"), p(5, "LFO 3 Freq"), p(10, "Cross Cutoff"), p(11, "Wet Only")]),
    ("Fruity Flanger", [p(0, "Delay"), p(1, "Depth"), p(2, "Rate"), p(3, "Phase"), p(6, "Feed"), p(9, "Dry"), p(10, "Wet"), p(11, "Cross")]),
    ("Fruity Phaser", [p(0, "Sweep Freq"), p(1, "Min Depth"), p(2, "Max Depth"), p(3, "Freq Range"), p(4, "Stereo"), p(6, "Feedback"), c(7, "Dry-Wet", 0.5), c(8, "Out Gain", 0.5)]),
    ("Fruity Balance", [c(0, "Balance", 0.5), c(1, "Volume", 0.8), None, None, None, None, None, None]),
    ("Fruity Stereo Enhancer", [c(2, "Stereo Sep", 0.5), c(3, "Phase Offset", 0.5), p(4, "Phase Position"), p(5, "Phase Inversion"), c(0, "Pan", 0.5), p(1, "Volume"), None, None]),
    ("Fruity Stereo Shaper", [c(0, "R into L Vol", 0.5), c(1, "Left Vol", 0.5), c(2, "Right Vol", 0.5), c(3, "L into R Vol", 0.5), c(4, "L/R Delay", 0.5), c(5, "L/R Phase", 0.5), None, None]),
];

/// A sampler channel's encoders, on Lemon Sampler's parameters by name: name on the screen, Lemon Sampler's name.
const SAMPLER: [(&str, &str); 4] =
    [("Filter Cutoff", "Filter frequency (modulation X)"), ("Filter Resonance", "Filter bandwidth (modulation Y)"), ("Filter Type", "Filter type"), ("Sample Start", "Sample start")];

const FILTER_TYPES: &[(f64, &str)] = &[
    (0.0, "Fast LP"),
    (0.0714, "LP"),
    (0.2143, "BP"),
    (0.3571, "HP"),
    (0.5, "BS"),
    (0.6429, "LPx2"),
    (0.7857, "SVF LP"),
    (0.9286, "SVF LPx2"),
];

/// Letters and digits only, lower case: how maps are looked up by plugin name.
fn squash(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_alphanumeric()).map(|c| c.to_ascii_lowercase()).collect()
}

/// The map of a plugin named `name` (FL's name, or the Lemon Studio plugin that recreates it).
fn map_of(name: &str) -> Option<&'static [Option<Slot>; 8]> {
    let fl = crate::plugins::FL_RECREATIONS.iter().find(|(_, l)| l.eq_ignore_ascii_case(name)).map(|(f, _)| *f);
    let fl = fl.unwrap_or(match name {
        crate::osc3::LEMON_TRIO => "3x Osc",
        crate::plugins::LEMON_EQ => "Fruity Parametric EQ 2",
        _ => name,
    });
    let want = squash(fl);
    MAPS.iter().find(|(n, _)| squash(n) == want).map(|(_, m)| m)
}

/// Maps of Lemon Studio's own plugins: encoder n's parameter by its index and the name the plugin
/// gives it (a slot whose parameter has moved or been renamed is dropped), and the name the screen shows.
const LEMON_MAPS: &[(&str, [Option<(usize, &str, &str)>; 8])] = &[(
    "Zest",
    [
        Some((149, "Frequency", "Cutoff")),
        Some((150, "Resonance", "Resonance")),
        Some((152, "Freq Mod Depth", "Filter Env")),
        Some((182, "Attack", "Attack")),
        Some((183, "Decay", "Decay")),
        Some((184, "Sustain", "Sustain")),
        Some((185, "Release", "Release")),
        Some((0, "Output", "Output")),
    ],
)];

/// An encoder's parameter in Plugin mode: its index, screen name, centre and ranges.
type PluginSlot = (usize, String, Option<(f64, f64)>, &'static [(f64, &'static str)]);

/// What Plugin mode's encoders turn: an instance (a rack row, or a mixer slot's key), its page of eight parameters (the
/// first is the plugin's map when it has one; then all its parameters, eight at a time) and the number of pages.
struct PluginTarget {
    key: usize,
    /// The title line: the channel's name, or the mixer track's for an effect.
    title: String,
    slots: [Option<PluginSlot>; 8],
    page: usize,
    pages: usize,
    /// The first page is a map.
    mapped: bool,
}

fn plugin_target(s: &State, page: usize) -> Option<PluginTarget> {
    // A focused effect window first, then the selected channel.
    let (key, title, name, sampler) = match s.focused() {
        Some(Win::Hosted(key)) if crate::plugins::fx_of(key).is_some() => {
            let (t, slot) = crate::plugins::fx_of(key)?;
            let plugin = crate::mixer::track(s, t)?.slots.get(usize::from(slot))?.as_ref()?.name.clone();
            (key, crate::mixer::name(s, t), plugin, false)
        }
        _ => {
            let row = s.selected_channel;
            let ch = s.project.channels.get(row)?;
            let title = crate::rack::channel_name(s, row);
            match ch.kind {
                os_model::ChannelKind::Sampler | os_model::ChannelKind::AudioClip => (row, title, String::new(), true),
                os_model::ChannelKind::Plugin => (row, title, ch.plugin.clone()?, false),
                _ => return None,
            }
        }
    };
    let list = s.params.lists.get(&key)?;
    let mut map: [Option<PluginSlot>; 8] = Default::default();
    let mut mapped = true;
    if sampler {
        for (i, (shown, theirs)) in SAMPLER.iter().enumerate() {
            if let Some(index) = list.params.iter().position(|e| e.name == *theirs) {
                map[i] = Some((index, shown.to_string(), None, if i == 2 { FILTER_TYPES } else { &[] }));
            }
        }
    } else if let Some(m) = map_of(&name) {
        for (i, slot) in m.iter().enumerate() {
            if let Some(sl) = slot.as_ref().filter(|sl| sl.index < list.params.len()) {
                let n = sl.name.map(String::from).unwrap_or_else(|| list.params[sl.index].name.clone());
                map[i] = Some((sl.index, n, sl.centre, sl.regions));
            }
        }
    } else if let Some((_, m)) = LEMON_MAPS.iter().find(|(n, _)| n.eq_ignore_ascii_case(&name)) {
        for (i, slot) in m.iter().enumerate() {
            if let Some((index, theirs, shown)) = slot.filter(|(index, theirs, _)| list.params.get(*index).is_some_and(|e| e.name == *theirs)) {
                let _ = theirs;
                map[i] = Some((index, shown.to_string(), None, &[]));
            }
        }
    } else {
        mapped = false;
    }
    // Then every parameter the plugin shows, eight at a time.
    let shown: Vec<usize> = list.params.iter().enumerate().filter(|(_, e)| !e.hidden).map(|(i, _)| i).collect();
    let pages = usize::from(mapped) + shown.len().div_ceil(8).max(usize::from(!mapped));
    let page = page.min(pages - 1);
    let slots = if mapped && page == 0 {
        map
    } else {
        let first = (page - usize::from(mapped)) * 8;
        let mut slots: [Option<PluginSlot>; 8] = Default::default();
        for (i, &index) in shown.iter().skip(first).take(8).enumerate() {
            slots[i] = Some((index, list.params[index].name.clone(), None, &[]));
        }
        slots
    };
    Some(PluginTarget { key, title, slots, page, pages, mapped })
}

/// A page's name on the screen: "Main controls", "Parameters 9-16".
fn page_name(pt: &PluginTarget) -> String {
    if pt.mapped && pt.page == 0 {
        return "Main controls".into();
    }
    let first = (pt.page - usize::from(pt.mapped)) * 8 + 1;
    let n = pt.slots.iter().filter(|s| s.is_some()).count().max(1);
    format!("Parameters {first}-{}", first + n - 1)
}

/// The plugin's text for its parameter's value, or the value in percent.
/// The value texts of Plugin mode's eight slots, the plugin held once for all of them.
/// `None` when the instance gave no moment between two of its blocks this tick (`plugins::lock_between_blocks`): the
/// texts shown stay, read again at the next tick.
fn plugin_texts(s: &State, pt: &PluginTarget, meter: Option<&os_host::Meter>) -> Option<[String; 8]> {
    let list = s.params.lists.get(&pt.key);
    let plugin = s.params.plugin(pt.key);
    let p = match &plugin {
        Some(m) => Some(crate::plugins::lock_between_blocks(m, meter, crate::plugins::TICK_WAIT)?),
        None => None,
    };
    let t0 = Instant::now();
    let texts = std::array::from_fn(|i| {
        let Some((index, _, _, regions)) = &pt.slots[i] else { return String::new() };
        let Some(e) = list.and_then(|l| l.params.get(*index)) else { return String::new() };
        let v = p.as_ref().and_then(|p| p.param_value(e.id)).unwrap_or(0.0);
        if let Some((_, name)) = regions.iter().rev().find(|(lo, _)| *lo <= v + 1e-6) {
            return name.to_string();
        }
        let text = p.as_ref().and_then(|p| p.param_text(e.id, v));
        text.filter(|t| !t.is_empty()).unwrap_or_else(|| format!("{:.0}%", v * 100.0))
    });
    if let Some(p) = &p {
        crate::dropout_log::held(&p.desc().name, "the keyboard's screen", t0.elapsed());
    }
    Some(texts)
}

impl App {
    /// The control port: opened when a recognised keyboard is listed, closed (the keyboard taken out of DAW mode first)
    /// when it goes. From `midi_refresh`.
    pub(crate) fn surface_refresh(&mut self) {
        if self.surface.as_ref().is_some_and(|d| !d.port.listed(&self.midi.inputs)) {
            self.surface = None;
        }
        if self.surface.is_none() && cfg!(not(test)) {
            if let Some(port) = Port::open(crate::wake_ui) {
                let mut d = Drive::new(port);
                d.start();
                self.surface = Some(d);
                self.surface_first_connect();
            }
        }
    }

    /// The first time the keyboard is ever connected, loop recording goes on and the keyboard gets the clock.
    fn surface_first_connect(&mut self) {
        const KEY: &str = "launchkey_mk4_connected";
        if crate::settings_file::get(KEY).is_some() {
            return;
        }
        crate::settings_file::write(KEY, "1");
        if !Opt::Loop.on() {
            self.toggle_record_option(Opt::Loop);
        }
        if let Some(d) = &self.surface {
            let name = d.port.name.clone();
            crate::midi_cfg::set_output(&name, crate::midi_cfg::OutCfg { sync: true, ..crate::midi_cfg::output(&name) });
        }
    }

    /// The keyboard back to its own mode: Lemon Studio quits.
    pub(crate) fn surface_exit(&mut self) {
        if let Some(d) = &mut self.surface {
            d.stop();
        }
    }

    /// A project opened: the keyboard starts over (banks, pages, button mode).
    pub(crate) fn surface_reload(&mut self) {
        if let Some(d) = &mut self.surface {
            d.start();
        }
    }

    /// The port's messages, the held buttons' repeats, then the lights and displays. From `midi_tick`.
    pub(crate) fn surface_tick(&mut self) {
        let Some(d) = &mut self.surface else { return };
        let msgs = d.port.poll();
        for m in msgs {
            if let Some(ev) = lk::parse(&m) {
                self.surface_in(ev);
            }
        }
        self.surface_repeat();
        if self.drive().encoders == Some(Encoders::Plugin) {
            self.plugin_page_follow();
        }
        self.surface_paint();
    }

    /// Something to do soon: a held arrow's repeat, the master fader's second display.
    pub(crate) fn surface_busy(&self) -> bool {
        self.surface.as_ref().is_some_and(|d| !d.held.is_empty() || d.master_again.is_some())
    }

    fn drive(&mut self) -> &mut Drive {
        self.surface.as_mut().expect("a surface")
    }

    fn surface_in(&mut self, ev: In) {
        match ev {
            In::Daw(on) => {
                let d = self.drive();
                d.daw = on;
                if on {
                    d.lights.forget();
                    d.screen.forget();
                }
            }
            In::Shift(held) => self.surface_shift(held),
            In::PadLayout(p) => self.pad_layout(p),
            In::EncoderLayout(e) => self.encoder_layout(e),
            In::FaderLayout(f) => self.fader_layout(f),
            In::Button { cc, down } => self.surface_button(cc, down),
            In::Pad { pad, vel } => self.surface_pad(pad, vel),
            In::Drum { note, vel } => self.surface_drum(note, vel),
            In::Encoder { i, steps } => self.surface_encoder(usize::from(i), steps),
            In::Fader { i, pos } => self.surface_fader(usize::from(i), pos),
        }
    }

    fn surface_shift(&mut self, held: bool) {
        let d = self.drive();
        d.shift = held;
        d.lights.shift(held);
        // Buttons still held are let go.
        d.held.clear();
        if held {
            // The Transport layout's encoders show their names again.
            for (i, n) in [(0, "Scrub"), (1, "Zoom"), (4, "Marker"), (7, "Tempo")] {
                if d.encoders == Some(Encoders::Transport) {
                    let m = d.model();
                    for msg in d.screen.write(m, display::ENCODER_1 + i, Text::three("", n, ""), Instant::now()) {
                        d.send(&msg);
                    }
                }
            }
        }
        self.surface_highlight(held);
    }

    /// While Shift is held, the mixer tracks the faders take and the channels the pads play are marked in the windows.
    fn surface_highlight(&mut self, on: bool) {
        let Some(d) = &self.surface else { return };
        let (bank, cbank, daw) = (d.mixer_bank, d.channel_bank, d.pads == Some(Pads::Daw));
        let until = if on { None } else { Some(Instant::now()) };
        let n = track_count(&self.state);
        self.state.surface_marks.mixer = Some((bank..(bank + BANK).min(n), until));
        if daw {
            let rows = rows(&self.state);
            let shown: Vec<usize> = rows.iter().skip(cbank * BANK).take(2 * BANK).copied().collect();
            self.state.surface_marks.rack = Some((shown, until));
        }
        self.invalidate();
    }

    fn pad_layout(&mut self, p: Pads) {
        let d = self.drive();
        // The last layout's pads go dark.
        for i in 0..16 {
            d.lights.set(Led::Pad(i), lk::OFF, false);
        }
        for n in lk::DRUM_NOTES {
            d.lights.set(Led::Drum(n), lk::OFF, false);
        }
        d.pads = Some(p);
        d.pads_held.clear();
        if p == Pads::Daw {
            self.state.raise(Win::ChannelRack);
            self.invalidate();
        }
    }

    fn encoder_layout(&mut self, e: Encoders) {
        let d = self.drive();
        d.encoders = Some(e);
        d.sticky = [None; 8];
        d.carry = [0.0; 8];
        if e != Encoders::Other {
            d.send(&lk::RELATIVE);
        }
        if e == Encoders::Mixer {
            self.close_opened();
            self.mixer_page_window();
        }
    }

    fn fader_layout(&mut self, f: Faders) {
        let d = self.drive();
        d.faders = Some(f);
        d.rearm = [true; 9];
        self.close_opened();
        if f == Faders::Volume {
            self.state.raise(Win::Mixer);
            self.invalidate();
        }
    }

    /// Mixer pages 0–1 (channels) show the channel rack; 2–4 (mixer tracks) the mixer.
    fn mixer_page_window(&mut self) {
        let page = self.drive().mixer_page;
        self.state.raise(if page < 2 { Win::ChannelRack } else { Win::Mixer });
        self.invalidate();
    }

    /// The plugin window the encoders opened closes.
    fn close_opened(&mut self) {
        if let Some(row) = self.drive().opened.take() {
            self.plugins.close_if_open(row);
            self.invalidate();
        }
    }

    fn notify(&mut self, a: impl Into<String>, b: impl Into<String>) {
        let d = self.drive();
        let m = d.model();
        for msg in d.screen.write(m, display::TEMPORARY, Text::two(a, b), Instant::now()) {
            d.send(&msg);
        }
    }

    fn surface_button(&mut self, cc: u8, down: bool) {
        let shift = self.drive().shift;
        // While Shift is held, only the Shift layer's buttons do anything.
        let shift_layer = matches!(cc, button::UNDO | button::SHIFT_TRACK_LEFT | button::SHIFT_TRACK_RIGHT);
        if shift && !shift_layer {
            return;
        }
        let d = self.drive();
        if !down {
            d.held.retain(|h| h.0 != cc);
            return;
        }
        let now = Instant::now();
        d.held.retain(|h| h.0 != cc);
        d.held.push((cc, now + REPEAT_DELAY));
        self.surface_press(cc, shift);
    }

    /// A button press (or an arrow's repeat).
    fn surface_press(&mut self, cc: u8, shift: bool) {
        match cc {
            button::PLAY => self.act(crate::app::Action::Play),
            button::STOP => self.act(crate::app::Action::Stop),
            button::RECORD => {
                // From the keyboard the Record button arms at once (no "What to record?" question).
                if self.state.rec.armed || !Opt::Ask.on() {
                    self.act(crate::app::Action::Record);
                } else {
                    let msg = self.rec_cmd(RecCmd::Arm { notes: Opt::Notes.on(), automation: Opt::Automation.on() });
                    if let Some(m) = msg {
                        self.say(m);
                    }
                }
            }
            button::LOOP => self.toggle_record_option(Opt::Loop),
            button::METRONOME => self.toggle_metronome(),
            button::CAPTURE => {
                if let Some(m) = self.rec_cmd(RecCmd::DumpAs(5, false)) {
                    self.say(m);
                }
            }
            button::QUANTISE => {
                if let Some(m) = crate::piano_tools::run(self, crate::piano_tools::NCmd::Quantize { strength: 100, starts_only: true }) {
                    self.say(m);
                }
            }
            button::UNDO => {
                let step = if shift { crate::history::redo(&mut self.state) } else { crate::history::undo(&mut self.state) };
                if let Some(step) = step {
                    self.say(format!("{}: {step}", if shift { "Redo" } else { "Undo" }));
                }
                self.invalidate();
            }
            button::TRACK_LEFT | button::TRACK_RIGHT => self.mixer_bank_step(if cc == button::TRACK_LEFT { -1 } else { 1 }),
            button::SHIFT_TRACK_LEFT | button::SHIFT_TRACK_RIGHT => self.mixer_select_step(if cc == button::SHIFT_TRACK_LEFT { -1 } else { 1 }),
            button::PADS_UP | button::PADS_DOWN => self.channel_bank_step(if cc == button::PADS_UP { -1 } else { 1 }),
            button::ENCODERS_UP | button::ENCODERS_DOWN => self.mixer_page_step(if cc == button::ENCODERS_UP { -1 } else { 1 }),
            button::ARM_SELECT => {
                let d = self.drive();
                d.arm = !d.arm;
                let arm = d.arm;
                self.close_opened();
                self.state.raise(Win::Mixer);
                self.invalidate();
                self.notify("Button Mode", if arm { "Arm" } else { "Mute" });
            }
            _ if (button::FADER_1..button::FADER_1 + 8).contains(&cc) => self.fader_button(usize::from(cc - button::FADER_1)),
            _ => {}
        }
    }

    /// Held arrows repeat.
    fn surface_repeat(&mut self) {
        let now = Instant::now();
        let shift = self.drive().shift;
        let due: Vec<u8> = self.drive().held.iter().filter(|h| h.1 <= now).map(|h| h.0).collect();
        for cc in due {
            let every = match cc {
                button::TRACK_LEFT | button::TRACK_RIGHT | button::PADS_UP | button::PADS_DOWN => REPEAT_SLOW,
                button::SHIFT_TRACK_LEFT | button::SHIFT_TRACK_RIGHT => REPEAT_FAST,
                _ => {
                    self.drive().held.retain(|h| h.0 != cc);
                    continue;
                }
            };
            if let Some(h) = self.drive().held.iter_mut().find(|h| h.0 == cc) {
                h.1 = now + every;
            }
            self.surface_press(cc, shift);
        }
    }

    fn mixer_bank_step(&mut self, by: isize) {
        let n = track_count(&self.state);
        let d = self.drive();
        let banks = n.div_ceil(BANK).max(1);
        let old = d.mixer_bank / BANK;
        let new = (old as isize + by).clamp(0, banks as isize - 1) as usize;
        d.mixer_bank = new * BANK;
        let shift = d.shift;
        let first = new * BANK;
        let last = (first + BANK).min(n) - 1;
        // The bank is marked in the mixer and scrolled into view, every press.
        let until = if shift { None } else { Some(Instant::now() + Duration::from_secs(3)) };
        self.state.surface_marks.mixer = Some((first..last + 1, until));
        self.mixer_show_track(first as u16);
        self.state.raise(Win::Mixer);
        self.invalidate();
        if new != old {
            let d = self.drive();
            d.rearm = [true; 9];
            self.notify("Mixer Tracks", format!("{first}-{last}"));
        }
    }

    /// The mixer scrolls to show track `t`.
    fn mixer_show_track(&mut self, t: u16) {
        let w = crate::views::mixer::STRIP_W;
        let x = f32::from(t.saturating_sub(1)) * w;
        let s = &mut self.state;
        if x < s.mixer_scroll || x > s.mixer_scroll + w * (BANK as f32 - 1.0) {
            s.mixer_scroll = x;
        }
    }

    fn mixer_select_step(&mut self, by: isize) {
        let n = track_count(&self.state) as isize;
        let cur = self.state.selected_mixer as isize;
        let t = (cur + by).rem_euclid(n) as u16;
        crate::mixer::select(&mut self.state, t, false);
        self.invalidate();
        let name = crate::mixer::name(&self.state, t);
        self.notify(if t == 0 { "Track".to_string() } else { format!("Track {t}") }, name);
    }

    fn channel_bank_step(&mut self, by: isize) {
        if self.drive().pads != Some(Pads::Daw) {
            return;
        }
        let rows = rows(&self.state);
        let d = self.drive();
        let banks = rows.len().div_ceil(BANK).max(1);
        let old = d.channel_bank;
        let new = (old as isize + by).clamp(0, banks as isize - 1) as usize;
        d.channel_bank = new;
        let shift = d.shift;
        let until = if shift { None } else { Some(Instant::now() + Duration::from_secs(3)) };
        let shown: Vec<usize> = rows.iter().skip(new * BANK).take(2 * BANK).copied().collect();
        if let Some(&first) = shown.first() {
            self.rack_show_row(first);
        }
        self.state.surface_marks.rack = Some((shown, until));
        self.state.raise(Win::ChannelRack);
        self.invalidate();
        if new != old {
            let a = new * BANK + 1;
            let b = (new * BANK + BANK).min(rows.len());
            self.notify("Channel Rack", format!("{a}-{b}"));
        }
    }

    /// The channel rack scrolls to show row `r`.
    fn rack_show_row(&mut self, r: usize) {
        let Some(y) = crate::views::rack::layout(&self.state).into_iter().find(|p| p.row == r).map(|p| p.top) else { return };
        let s = &mut self.state;
        if y < s.rack_scroll || y > s.rack_scroll + crate::views::rack::ROW_H * 8.0 {
            s.rack_scroll = y.max(0.0);
        }
    }

    fn mixer_page_step(&mut self, by: isize) {
        if self.drive().encoders == Some(Encoders::Plugin) {
            return self.plugin_page_step(by);
        }
        if self.drive().encoders != Some(Encoders::Mixer) {
            return;
        }
        let d = self.drive();
        let new = (d.mixer_page as isize + by).clamp(0, MIXER_PAGES.len() as isize - 1) as usize;
        if new == d.mixer_page {
            return;
        }
        d.mixer_page = new;
        d.sticky = [None; 8];
        self.close_opened();
        self.mixer_page_window();
        self.notify("Mixer", MIXER_PAGES[new]);
    }

    /// Plugin mode's pages: the encoder arrows step through the plugin's parameters, eight at a time (a Lemon Studio
    /// addition; spec: controllers/launchkey-mk4.md "Lemon Studio (cutting-edge)").
    fn plugin_page_step(&mut self, by: isize) {
        let page = self.drive().plugin_page;
        let Some(pt) = plugin_target(&self.state, page) else { return };
        let new = (pt.page as isize + by).clamp(0, pt.pages as isize - 1) as usize;
        if new == pt.page {
            return;
        }
        let d = self.drive();
        (d.plugin_page, d.carry, d.sticky) = (new, [0.0; 8], [None; 8]);
        if let Some(pt) = plugin_target(&self.state, new) {
            self.notify(pt.title.clone(), page_name(&pt));
        }
    }

    /// Another instance in Plugin mode starts on its first page.
    fn plugin_page_follow(&mut self) {
        let key = plugin_target(&self.state, 0).map(|pt| pt.key);
        let d = self.drive();
        if d.plugin_key != key {
            (d.plugin_key, d.plugin_page) = (key, 0);
        }
    }

    fn fader_button(&mut self, i: usize) {
        let d = self.drive();
        let t = d.mixer_bank + i;
        let arm = d.arm;
        if t >= track_count(&self.state) {
            return;
        }
        let t = t as u16;
        if arm {
            crate::mixer::hit(self, crate::mixer::MHit::Arm(t));
        } else if let Some(m) = crate::mixer::track_mut(&mut self.state, t) {
            m.enabled = !m.enabled;
            self.state.piano.dirty = true;
        }
        self.invalidate();
    }

    fn surface_pad(&mut self, pad: u8, vel: u8) {
        if self.drive().pads != Some(Pads::Daw) {
            return;
        }
        let rows = rows(&self.state);
        let bank = self.drive().channel_bank;
        // The bottom row plays the bank's first eight channels, the top row the next eight.
        let i = bank * BANK + if pad >= 8 { usize::from(pad) - 8 } else { usize::from(pad) + 8 };
        let src = Src::Omni(0x60 + pad);
        if vel == 0 {
            self.drive().pads_held.retain(|h| h.0 != pad);
            self.note_off_from(src, 64);
            return;
        }
        let Some(&row) = rows.get(i) else { return };
        let others = !self.drive().pads_held.is_empty();
        self.drive().pads_held.push((pad, row));
        let extra = Extra { channel: Some(self.state.project.channels[row].index), ..Extra::default() };
        self.midi_play_note(src, 60, row, 60, vel, extra);
        if !others && self.state.selected_channel != row {
            self.state.rack.select_only(row);
            self.focus_channel(row);
            self.state.raise(Win::ChannelRack);
            if self.drive().opened.is_some_and(|o| o != row) {
                self.close_opened();
            }
            self.drive().last_turn = [None; 8];
            self.invalidate();
        }
    }

    fn surface_drum(&mut self, note: u8, vel: u8) {
        if self.drive().pads != Some(Pads::Drum) {
            return;
        }
        let src = Src::Omni(note);
        if vel == 0 {
            return self.note_off_from(src, 64);
        }
        let row = self.state.selected_channel;
        let Some(ch) = self.state.project.channels.get(row) else { return };
        let extra = Extra { channel: Some(ch.index), ..Extra::default() };
        self.midi_play_note(src, note, row, u16::from(note), vel, extra);
    }

    /// Value `v` of `t` as it is now.
    fn target_value(&self, t: Target) -> f64 {
        let s = &self.state;
        match t {
            Target::Knob(k) => f64::from(s.knob(k)),
            Target::Fader(m) => crate::mixer::track(s, m).and_then(|m| m.volume).unwrap_or(0.8),
            Target::Param(key, i) => crate::params::param_value(s, key, i).unwrap_or(0.0),
            Target::Tempo => self.transport.tempo().unwrap_or(s.tempo),
        }
    }

    /// Sets `t` to `v` as the mouse would: undoable (`first`: the gesture's first change makes the step), recorded while
    /// recording automation.
    fn target_set(&mut self, t: Target, v: f64, first: bool) {
        let v = v.clamp(0.0, 1.0);
        let s = &mut self.state;
        match t {
            Target::Knob(k) => {
                crate::history::knob(s, k, !first);
                s.set_knob(k, v as f32);
                self.record_knob(k);
            }
            Target::Fader(m) => {
                let run = crate::history::Run::Fader(m);
                if first {
                    crate::history::settle(s);
                }
                crate::control::run_edit(s, crate::history::MIXER_VOLUME, Region::Track(m), run, true, |s| {
                    if let Some(tr) = crate::mixer::track_mut(s, m) {
                        tr.volume = Some(v);
                        s.piano.dirty = true;
                    }
                });
            }
            Target::Param(key, i) => {
                let before = crate::params::param_value(s, key, i);
                if crate::params::set_param_value(s, key, i, v) {
                    crate::control::param_set(s, key, i, if first { before } else { None }, v);
                }
            }
            Target::Tempo => {}
        }
        self.invalidate();
    }

    /// Whether this turn of encoder `i` on `t` starts a gesture (an undo step of its own).
    fn gesture(&mut self, i: usize, t: Target) -> bool {
        let now = Instant::now();
        let d = self.drive();
        let first = !d.last_turn[i].is_some_and(|(at, was)| was == t && now.duration_since(at) < GESTURE);
        d.last_turn[i] = Some((now, t));
        first
    }

    /// Turns `t` by `steps` detents (and `carry`, turning a stepped parameter has not shown yet); returns the value set.
    fn turn(&mut self, i: usize, t: Target, steps: i8, centre: Option<(f64, f64)>, carry: f64) -> f64 {
        let d = f64::from(steps) * lk::STEP;
        let v = self.target_value(t);
        let to = match centre {
            Some((c, w)) => {
                let drv = self.drive();
                let st = drv.sticky[i].get_or_insert(Sticky::new(c, w));
                if st.centre != c {
                    *st = Sticky::new(c, w);
                }
                st.turn(v, d)
            }
            None => (v + d).clamp(0.0, 1.0),
        } + carry;
        let to = to.clamp(0.0, 1.0);
        let first = self.gesture(i, t);
        self.target_set(t, to, first);
        to
    }

    fn surface_encoder(&mut self, i: usize, steps: i8) {
        let Some(mode) = self.drive().encoders else { return };
        match mode {
            Encoders::Plugin => self.plugin_encoder(i, steps),
            Encoders::Mixer => self.mixer_encoder(i, steps),
            Encoders::Transport => self.transport_encoder(i, steps),
            Encoders::Sends | Encoders::Other => {}
        }
    }

    fn plugin_encoder(&mut self, i: usize, steps: i8) {
        let page = self.drive().plugin_page;
        let Some(pt) = plugin_target(&self.state, page) else { return };
        let Some((index, _, centre, _)) = pt.slots[i].clone() else { return };
        let t = Target::Param(pt.key, index);
        // A parameter of few steps (a switch, a choice) moves a step per detent.
        let steps_of = self.state.params.lists.get(&pt.key).and_then(|l| l.params.get(index)).map_or(0, |e| e.steps);
        if (1..=MAX_STEPPED).contains(&steps_of) {
            let n = f64::from(steps_of);
            let at = (self.target_value(t) * n).round() + f64::from(steps);
            let first = self.gesture(i, t);
            self.target_set(t, at.clamp(0.0, n) / n, first);
            return self.open_target_window(pt.key);
        }
        // A stepped parameter takes the turns it could not show yet into the next.
        let carry = self.drive().carry[i];
        let wanted = self.turn(i, t, steps, centre, carry);
        let got = self.target_value(t);
        self.drive().carry[i] = if (got - wanted).abs() > 1e-9 { carry + wanted - got } else { 0.0 };
        self.open_target_window(pt.key);
    }

    /// The selected channel's plugin window opens as its parameters turn, unless an effect's window is in front.
    fn open_target_window(&mut self, key: usize) {
        if crate::plugins::fx_of(key).is_none() {
            let row = key;
            let open = self.plugins.window_of(row).is_some();
            if !open {
                if self.state.hosted_rows.contains(&row) {
                    self.state.pending_open = Some(row);
                } else {
                    self.open_channel_window(row);
                }
                self.drive().opened = Some(row);
            }
        }
    }

    fn mixer_encoder(&mut self, i: usize, steps: i8) {
        let page = self.drive().mixer_page;
        match page {
            0 | 1 => {
                let rows = rows(&self.state);
                let bank = self.drive().channel_bank;
                let Some(&row) = rows.get(bank * BANK + i) else { return };
                let (k, centre) = if page == 0 { (KnobId::ChanVol(row), None) } else { (KnobId::ChanPan(row), Some((0.5, 0.1))) };
                self.turn(i, Target::Knob(k), steps, centre, 0.0);
                self.state.raise(Win::ChannelRack);
            }
            2 | 3 => {
                let t = self.drive().mixer_bank + i;
                if t >= track_count(&self.state) {
                    return;
                }
                let t = t as u16;
                if page == 2 {
                    self.turn(i, Target::Fader(t), steps, Some((0.8, 0.05)), 0.0);
                } else {
                    self.turn(i, Target::Knob(KnobId::MixPan(t)), steps, Some((0.5, 0.1)), 0.0);
                }
            }
            _ => {
                if i >= 6 {
                    return;
                }
                let t = self.state.selected_mixer;
                let (band, gain) = ((i / 2) as u8, i % 2 == 1);
                let k = KnobId::Eq(t, band, if gain { 0 } else { 1 });
                self.turn(i, Target::Knob(k), steps, gain.then_some((0.5, 0.05)), 0.0);
            }
        }
    }

    fn transport_encoder(&mut self, i: usize, steps: i8) {
        if steps == 0 {
            return;
        }
        let (name, value) = match i {
            0 => {
                // Off a beat: right goes to the next beat, left back to this one's start. On a beat: a beat either way.
                let beat = self.state.project.beat_ticks().max(1);
                let pos = self.song_pos_now();
                let on = pos % beat == 0;
                let to = match (steps > 0, on) {
                    (true, _) => (pos / beat + 1) * beat,
                    (false, true) => pos.saturating_sub(beat),
                    (false, false) => pos / beat * beat,
                };
                self.set_song_pos(to, true);
                self.invalidate();
                let shown = self.song_pos_now();
                let hint = crate::song_pos::hint(&self.state, shown);
                ("Scrub", value_part(hint))
            }
            1 => {
                let into = steps > 0;
                self.zoom_focused(into);
                ("Zoom", if into { "In".into() } else { "Out".into() })
            }
            4 => {
                if !self.state.song_mode {
                    ("Marker", "Pattern mode".into())
                } else {
                    self.state.raise(Win::Playlist);
                    let d = self.drive();
                    d.marker_turns += i32::from(steps.signum());
                    let jump = if d.marker_turns > 3 {
                        1
                    } else if d.marker_turns < -3 {
                        -1
                    } else {
                        0
                    };
                    if jump != 0 {
                        d.marker_turns = 0;
                        self.marker_step(jump);
                    }
                    ("Marker", if steps > 0 { "Next".into() } else { "Previous".into() })
                }
            }
            7 => {
                let k = (f64::from(steps) * 25.0 / 127.0).abs().ceil() * f64::from(steps.signum());
                let bpm = self.target_value(Target::Tempo) + k;
                let first = self.gesture(i, Target::Tempo);
                if first {
                    crate::control::edit_press(&mut self.state, crate::control::command::TEMPO, Region::Settings);
                }
                let bpm = self.set_tempo(bpm);
                self.invalidate();
                self.drive().tempo_shown = Some(bpm);
                ("Tempo", format!("{bpm:.3} BPM"))
            }
            _ => return,
        };
        let d = self.drive();
        let m = d.model();
        for msg in d.screen.write(m, display::ENCODER_1 + i as u8, Text::two(name, value), Instant::now()) {
            d.send(&msg);
        }
    }

    /// The next (`by` 1) or previous playlist marker.
    fn marker_step(&mut self, by: i32) {
        let pos = self.song_pos_now();
        let mut at: Vec<u32> = crate::playlist::markers(&self.state)
            .iter()
            .filter(|m| m.kind != os_model::TimeMarker::SIGNATURE)
            .map(|m| m.position)
            .collect();
        at.sort_unstable();
        let to = if by > 0 { at.into_iter().find(|&t| t > pos) } else { at.into_iter().rev().find(|&t| t < pos) };
        if let Some(t) = to {
            self.set_song_pos(t, true);
            self.invalidate();
        }
    }

    /// The focused editor zooms in or out in time (the playlist, or the piano roll).
    fn zoom_focused(&mut self, into: bool) {
        match self.state.focused() {
            Some(Win::PianoRoll) => {
                crate::piano_tools::zoom_key(self, if into { 1.0 } else { -1.0 });
            }
            _ => {
                let cmd = if into { crate::playlist::Cmd::ZoomIn } else { crate::playlist::Cmd::ZoomOut };
                crate::playlist::run(self, cmd);
            }
        }
    }

    fn surface_fader(&mut self, i: usize, pos: f64) {
        let d = self.drive();
        let master = i == 8;
        if !master && d.faders != Some(Faders::Volume) {
            return;
        }
        let t = if master { 0 } else { d.mixer_bank + i };
        if t >= track_count(&self.state) {
            return;
        }
        let t = t as u16;
        let v = lk::fader_volume(pos);
        let now_v = self.target_value(Target::Fader(t));
        let d = self.drive();
        if std::mem::take(&mut d.rearm[i]) {
            d.takeover[i] = (None, false);
        }
        // Soft takeover: the fader moves the volume once it has met it.
        let (last, caught) = d.takeover[i];
        let caught = caught
            || !crate::prefs::check(&crate::prefs::PICKUP)
            || (v - now_v).abs() < 0.01
            || last.is_some_and(|l| (l - now_v) * (v - now_v) <= 0.0);
        d.takeover[i] = (Some(v), caught);
        if caught {
            let first = self.gesture_fader(i);
            self.target_set(Target::Fader(t), v, first);
        }
        let name = crate::mixer::name(&self.state, t);
        let level = db_text(os_engine::laws::fader_gain(self.target_value(Target::Fader(t))));
        let d = self.drive();
        let m = d.model();
        for msg in d.screen.write(m, display::FADER_1 + i as u8, Text::three(name, "Level", level), Instant::now()) {
            d.send(&msg);
        }
        if master {
            d.master_again = Some(Instant::now() + MASTER_AGAIN);
        }
    }

    fn gesture_fader(&mut self, i: usize) -> bool {
        // The faders share the encoders' gesture timing, on their own slots.
        let now = Instant::now();
        let d = self.drive();
        let slot = i % 8;
        let first = !d.last_turn[slot].is_some_and(|(at, was)| matches!(was, Target::Fader(_)) && now.duration_since(at) < GESTURE);
        d.last_turn[slot] = Some((now, Target::Fader(i as u16)));
        first
    }

    /// The lights and the displays as the project is now.
    fn surface_paint(&mut self) {
        let Some(d) = &self.surface else { return };
        if !d.daw {
            return;
        }
        let s = &self.state;
        let (pads, enc, faders, arm, mbank, cbank, page) = (d.pads, d.encoders, d.faders, d.arm, d.mixer_bank, d.channel_bank, d.mixer_page);
        let plugin = if enc == Some(Encoders::Plugin) { plugin_target(s, d.plugin_page) } else { None };
        // The plugin's texts: read again only after a while, on another instance or page, or after a turn.
        let now = Instant::now();
        let reread = plugin.as_ref().map(|pt| match &d.plugin_shown {
            Some((at, key, pg, texts)) if *key == pt.key && *pg == pt.page && now.duration_since(*at) < PLUGIN_REREAD
                && !d.last_turn.iter().flatten().any(|(t, _)| t > at) => (false, texts.clone()),
            shown => match plugin_texts(s, pt, self.plugins.hosted.get(&pt.key).and_then(|h| h.meter.as_deref())) {
                Some(texts) => (true, texts),
                None => (false, shown.as_ref().filter(|(_, key, pg, _)| *key == pt.key && *pg == pt.page).map(|x| x.3.clone()).unwrap_or_default()),
            },
        });
        let held: Vec<u8> = d.held.iter().map(|h| h.0).collect();
        let pads_held: Vec<u8> = d.pads_held.iter().map(|h| h.0).collect();
        let n = track_count(s);
        let rows = rows(s);
        let playing = self.transport.playing() && !self.transport.paused();
        let mut lights: Vec<(Led, Colour, bool)> = Vec::new();
        let lit = |on: bool| if on { lk::WHITE } else { lk::OFF };
        let momentary = |cc: u8| lit(held.contains(&cc));
        // An arrow: dim when it can go further, white while held, dark at the end.
        let arrow = |cc: u8, can: bool| if !can { lk::OFF } else if held.contains(&cc) { lk::WHITE } else { lk::DIM };
        lights.push((Led::Button(button::PLAY), lit(playing), false));
        lights.push((Led::Button(button::RECORD), lit(s.rec.armed), false));
        lights.push((Led::Button(button::METRONOME), lit(crate::metronome::on()), false));
        lights.push((Led::Button(button::LOOP), lit(Opt::Loop.on()), false));
        for cc in [button::STOP, button::CAPTURE, button::QUANTISE, button::UNDO] {
            lights.push((Led::Button(cc), momentary(cc), false));
        }
        lights.push((Led::Button(button::UNDO), momentary(button::UNDO), true));
        let banks = n.div_ceil(BANK);
        lights.push((Led::Button(button::TRACK_LEFT), arrow(button::TRACK_LEFT, mbank > 0), false));
        lights.push((Led::Button(button::TRACK_RIGHT), arrow(button::TRACK_RIGHT, mbank / BANK + 1 < banks), false));
        lights.push((Led::Button(button::SHIFT_TRACK_LEFT), arrow(button::SHIFT_TRACK_LEFT, true), true));
        lights.push((Led::Button(button::SHIFT_TRACK_RIGHT), arrow(button::SHIFT_TRACK_RIGHT, true), true));
        let daw_pads = pads == Some(Pads::Daw);
        let cbanks = rows.len().div_ceil(BANK);
        lights.push((Led::Button(button::PADS_UP), arrow(button::PADS_UP, daw_pads && cbank > 0), false));
        lights.push((Led::Button(button::PADS_DOWN), arrow(button::PADS_DOWN, daw_pads && cbank + 1 < cbanks), false));
        let mixer = enc == Some(Encoders::Mixer);
        let (up, down) = match &plugin {
            Some(pt) => (pt.page > 0, pt.page + 1 < pt.pages),
            None => (mixer && page > 0, mixer && page + 1 < MIXER_PAGES.len()),
        };
        lights.push((Led::Button(button::ENCODERS_UP), arrow(button::ENCODERS_UP, up), false));
        lights.push((Led::Button(button::ENCODERS_DOWN), arrow(button::ENCODERS_DOWN, down), false));
        // Fader buttons: armed red, not armed dark red; or the track's colour unless muted.
        if faders == Some(Faders::Volume) {
            for i in 0..8u8 {
                let t = mbank + usize::from(i);
                let c = match crate::mixer::track(s, t as u16).filter(|_| t < n) {
                    None => lk::OFF,
                    Some(m) if arm => if m.armed { lk::RED } else { lk::DARK_RED },
                    Some(m) if !m.enabled => lk::OFF,
                    Some(m) => Colour::of(m.color.unwrap_or(DEFAULT_COLOUR)),
                };
                lights.push((Led::Button(button::FADER_1 + i), c, false));
            }
            lights.push((Led::Button(button::ARM_SELECT), if arm { lk::RED } else { lk::WHITE }, false));
        }
        match pads {
            Some(Pads::Daw) => {
                for pad in 0..16u8 {
                    let i = cbank * BANK + if pad >= 8 { usize::from(pad) - 8 } else { usize::from(pad) + 8 };
                    let c = match rows.get(i) {
                        None => lk::OFF,
                        Some(_) if pads_held.contains(&pad) => lk::GREEN,
                        Some(&r) if r == s.selected_channel => lk::WHITE,
                        Some(&r) => Colour::of(s.project.channels[r].color.unwrap_or(DEFAULT_COLOUR)),
                    };
                    lights.push((Led::Pad(pad), c, false));
                }
            }
            Some(Pads::Drum) => {
                let c = s.project.channels.get(s.selected_channel).map_or(lk::OFF, |ch| Colour::of(ch.color.unwrap_or(DEFAULT_COLOUR)));
                for note in lk::DRUM_NOTES {
                    lights.push((Led::Drum(note), c, false));
                }
            }
            _ => {}
        }
        // Displays: the stationary one, and what each encoder shows when touched.
        let stationary = if enc == Some(Encoders::Transport) {
            Text::Grid("Transport".into(), ["Scrb", "Zoom", "-", "-", "Mark", "-", "-", "BPM"].map(String::from))
        } else {
            Text::three("", NAME, "")
        };
        let mut texts: Vec<(u8, Text)> = vec![(display::STATIONARY, stationary)];
        let unused = |title: String| Text::three(title, "Not Used", "");
        match enc {
            Some(Encoders::Plugin) => match &plugin {
                Some(pt) => {
                    let shown = &reread.as_ref().expect("read with the target").1;
                    for (i, slot) in pt.slots.iter().enumerate() {
                        let t = match slot {
                            Some((_, name, _, _)) => Text::three(pt.title.clone(), name.clone(), shown[i].clone()),
                            None => unused(pt.title.clone()),
                        };
                        texts.push((display::ENCODER_1 + i as u8, t));
                    }
                }
                None => {
                    if let Some(row) = (s.selected_channel < s.project.channels.len()).then_some(s.selected_channel) {
                        let title = crate::rack::channel_name(s, row);
                        for i in 0..8 {
                            texts.push((display::ENCODER_1 + i, unused(title.clone())));
                        }
                    }
                }
            },
            Some(Encoders::Mixer) => {
                for i in 0..8usize {
                    let t = match page {
                        0 | 1 => match rows.get(cbank * BANK + i) {
                            Some(&r) => {
                                let name = crate::rack::channel_name(s, r);
                                if page == 0 {
                                    let v = f64::from(s.knob(KnobId::ChanVol(r)));
                                    Text::three(name, "Level", db_text(os_engine::laws::channel_gain(v)))
                                } else {
                                    Text::three(name, "Pan", pan_text(f64::from(s.knob(KnobId::ChanPan(r)))))
                                }
                            }
                            None => unused(String::new()),
                        },
                        2 | 3 => {
                            let t = mbank + i;
                            if t >= n {
                                unused(String::new())
                            } else if page == 2 {
                                let v = crate::mixer::track(s, t as u16).and_then(|m| m.volume).unwrap_or(0.8);
                                Text::three(crate::mixer::name(s, t as u16), "Level", db_text(os_engine::laws::fader_gain(v)))
                            } else {
                                Text::three(crate::mixer::name(s, t as u16), "Pan", pan_text(f64::from(s.knob(KnobId::MixPan(t as u16)))))
                            }
                        }
                        _ if i < 6 => {
                            let t = s.selected_mixer;
                            let (band, gain) = ((i / 2) as u8, i % 2 == 1);
                            let k = KnobId::Eq(t, band, if gain { 0 } else { 1 });
                            let label = ["Low Shelf", "Peaking", "High Shelf"][usize::from(band)];
                            Text::three(crate::mixer::name(s, t), label, value_part(s.knob_text(k)))
                        }
                        _ => unused(String::new()),
                    };
                    texts.push((display::ENCODER_1 + i as u8, t));
                }
            }
            Some(Encoders::Sends) => {
                for i in 0..8 {
                    texts.push((display::ENCODER_1 + i, unused(String::new())));
                }
            }
            _ => {}
        }
        let tempo = self.transport.tempo().unwrap_or(s.tempo);
        let master_again = d.master_again.filter(|t| *t <= Instant::now()).is_some();
        let master = (crate::mixer::name(s, 0), crate::mixer::track(s, 0).and_then(|m| m.volume).unwrap_or(0.8));
        let shown = plugin.as_ref().zip(reread).filter(|(_, (read, _))| *read).map(|(pt, (_, texts))| (now, pt.key, pt.page, texts));
        let d = self.drive();
        if shown.is_some() {
            d.plugin_shown = shown;
        }
        for (led, c, shift) in lights {
            d.lights.set(led, c, shift);
        }
        let m = d.model();
        let now = Instant::now();
        if let Some(msg) = d.lights.flush(m) {
            d.send(&msg);
        }
        let mut out = Vec::new();
        for (at, t) in texts {
            out.extend(d.screen.write(m, at, t, now));
        }
        // The tempo encoder's display follows tempo changes made elsewhere.
        if enc == Some(Encoders::Transport) && d.tempo_shown.is_some_and(|b| (b - tempo).abs() > 1e-6) {
            d.tempo_shown = Some(tempo);
            out.extend(d.screen.write(m, display::ENCODER_1 + 7, Text::two("Tempo", format!("{tempo:.3} BPM")), now));
        }
        if master_again {
            d.master_again = None;
            out.extend(d.screen.write(m, display::FADER_1 + 8, Text::three(master.0, "Level", db_text(os_engine::laws::fader_gain(master.1))), now));
        }
        for msg in out {
            d.send(&msg);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texts() {
        assert_eq!(pan_text(0.5), "C");
        assert_eq!(pan_text(0.0), "100L");
        assert_eq!(pan_text(0.625), "25R");
        assert_eq!(db_text(1.0), "0.0 dB");
        assert_eq!(db_text(0.0), "-Inf dB");
        assert_eq!(value_part("EQ band 1 (low shelf) level: 0.0dB".into()), "0.0dB");
    }

    #[test]
    fn maps_find_lemon_recreations() {
        assert!(map_of("Lemon Delay").is_some_and(|m| m[1].as_ref().is_some_and(|s| s.index == 4)));
        assert!(map_of("Fruity Delay 3").is_some());
        assert!(map_of("fruity  REEVERB 2").is_some());
        assert!(map_of(crate::osc3::LEMON_TRIO).is_some());
        assert!(map_of("Some VST").is_none());
    }
}
