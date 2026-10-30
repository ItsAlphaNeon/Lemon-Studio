// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The Novation Launchkey MK4's DAW port: what it sends, and the messages that light its buttons and pads and write
//! its screen (Novation's Launchkey MK4 Programmer's Reference Guide, v2). What the controls do in Lemon Studio is
//! `super::drive`.
// spec: controllers/launchkey-mk4.md

use std::collections::HashMap;
use std::time::{Duration, Instant};

use os_model::Rgb;

/// Enter DAW mode (the keyboard answers with the same message, then reports its layouts).
pub const DAW_ON: [u8; 3] = [0x9f, 0x0c, 0x7f];
pub const DAW_OFF: [u8; 3] = [0x9f, 0x0c, 0x00];
/// The DAW owns the drum layout's pads (their notes come to the DAW port, and the DAW lights them).
pub const DRUMS_OWNED: [u8; 3] = [0xb6, 0x54, 0x7f];
pub const DRUMS_RELEASED: [u8; 3] = [0xb6, 0x54, 0x00];
/// The encoders send relative values.
pub const RELATIVE: [u8; 3] = [0xb6, 0x45, 0x7f];
/// Layouts chosen on connecting: encoders Plugin, pads DAW, faders Volume.
pub const START_LAYOUTS: [[u8; 3]; 3] = [[0xb6, 0x1e, 0x02], [0xb6, 0x1d, 0x02], [0xb6, 0x1f, 0x01]];

/// The buttons, by the controller number they send on MIDI channel 1 (and light by).
pub mod button {
    pub const PLAY: u8 = 0x73;
    pub const STOP: u8 = 0x74;
    pub const RECORD: u8 = 0x75;
    pub const LOOP: u8 = 0x76;
    pub const CAPTURE: u8 = 0x4a;
    pub const QUANTISE: u8 = 0x4b;
    pub const METRONOME: u8 = 0x4c;
    /// Undo; with Shift, Redo.
    pub const UNDO: u8 = 0x4d;
    pub const TRACK_LEFT: u8 = 0x67;
    pub const TRACK_RIGHT: u8 = 0x66;
    /// What Track ◀ / ▶ send while Shift is held.
    pub const SHIFT_TRACK_LEFT: u8 = 0x6d;
    pub const SHIFT_TRACK_RIGHT: u8 = 0x6c;
    pub const PADS_UP: u8 = 0x6a;
    pub const PADS_DOWN: u8 = 0x6b;
    pub const ENCODERS_UP: u8 = 0x33;
    pub const ENCODERS_DOWN: u8 = 0x34;
    /// Fader buttons 1–8 are `FADER_1 + i`.
    pub const FADER_1: u8 = 0x25;
    pub const ARM_SELECT: u8 = 0x2d;
}

/// Pad layouts (reported on channel 7, controller 29).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pads {
    Daw,
    Drum,
    /// Any of the keyboard's own (chords, custom modes): the DAW leaves the pads alone.
    Other,
}

/// Encoder layouts (controller 30).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoders {
    Mixer,
    Plugin,
    Sends,
    Transport,
    Other,
}

/// Fader layouts (controller 31; Launchkey 49 and 61).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Faders {
    Volume,
    Other,
}

/// A message from the DAW port.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum In {
    /// The keyboard confirmed DAW mode (on) or left it.
    Daw(bool),
    Button { cc: u8, down: bool },
    Shift(bool),
    PadLayout(Pads),
    EncoderLayout(Encoders),
    FaderLayout(Faders),
    /// Pad 0..16 (0–7 the top row, 8–15 the bottom row, left to right), velocity (0: released).
    Pad { pad: u8, vel: u8 },
    /// Drum layout: the pad's note (36–51), velocity (0: released).
    Drum { note: u8, vel: u8 },
    /// Encoder 0..8 turned by `steps` detents (relative output).
    Encoder { i: u8, steps: i8 },
    /// Fader 0..8 (8: the master fader), position 0..=1.
    Fader { i: u8, pos: f64 },
}

/// The top row's notes start at 0x60, the bottom row's at 0x70.
fn pad_of(note: u8) -> Option<u8> {
    match note {
        0x60..=0x67 => Some(note - 0x60),
        0x70..=0x77 => Some(note - 0x70 + 8),
        _ => None,
    }
}

/// The note pad `pad` sends in the DAW layout.
pub fn pad_note(pad: u8) -> u8 {
    if pad < 8 { 0x60 + pad } else { 0x70 + pad - 8 }
}

/// The drum layout's notes, top row then bottom row, left to right.
pub const DRUM_NOTES: [u8; 16] = [40, 41, 42, 43, 48, 49, 50, 51, 36, 37, 38, 39, 44, 45, 46, 47];

/// The message in `b` (one short message from the DAW port), if the driver uses it.
pub fn parse(b: &[u8]) -> Option<In> {
    let (&status, rest) = b.split_first()?;
    let d1 = *rest.first()? & 0x7f;
    let d2 = rest.get(1).map_or(0, |v| v & 0x7f);
    let on = |vel: u8| if status & 0xf0 == 0x80 { 0 } else { vel };
    Some(match status {
        0x9f | 0x8f if d1 == 0x0c => In::Daw(status == 0x9f && d2 != 0),
        0x90 | 0x80 => In::Pad { pad: pad_of(d1)?, vel: on(d2) },
        0x99 | 0x89 if DRUM_NOTES.contains(&d1) => In::Drum { note: d1, vel: on(d2) },
        0xb0 => In::Button { cc: d1, down: d2 != 0 },
        0xb6 => match d1 {
            0x3f | 0x48 => In::Shift(d2 != 0),
            0x1d | 0x40 => In::PadLayout(match d2 {
                2 => Pads::Daw,
                1 | 15 => Pads::Drum,
                _ => Pads::Other,
            }),
            0x1e | 0x41 => In::EncoderLayout(match d2 {
                1 => Encoders::Mixer,
                2 => Encoders::Plugin,
                4 => Encoders::Sends,
                5 => Encoders::Transport,
                _ => Encoders::Other,
            }),
            0x1f | 0x42 => In::FaderLayout(if d2 == 1 { Faders::Volume } else { Faders::Other }),
            _ => return None,
        },
        0xbf => match d1 {
            0x55..=0x5c => In::Encoder { i: d1 - 0x55, steps: (i16::from(d2) - 64).clamp(-64, 63) as i8 },
            0x05..=0x0d => In::Fader { i: d1 - 0x05, pos: f64::from(d2) / 127.0 },
            _ => return None,
        },
        _ => return None,
    })
}

/// Which keyboard: the SysEx header's product byte and the controls it has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Model {
    /// Launchkey 25, 37, 49, 61 MK4: whether it has faders (49, 61).
    Mk4 { faders: bool },
    Mini,
}

impl Model {
    /// The model a DAW port's system name belongs to ("Launchkey MK4 61:Launchkey MK4 61 DAW In 24:1" on Linux,
    /// "MIDIIN2 (Launchkey MK4 61)" on Windows).
    pub fn of_port(name: &str) -> Option<Model> {
        let lower = name.to_ascii_lowercase();
        let daw = lower.contains(" daw") || lower.starts_with("midiin2 (") || lower.starts_with("midiout2 (");
        if !daw || !lower.contains("launchkey") || !lower.contains("mk4") {
            return None;
        }
        if lower.contains("mini") {
            return Some(Model::Mini);
        }
        Some(Model::Mk4 { faders: lower.contains("mk4 49") || lower.contains("mk4 61") })
    }

    /// Whether `name` is a Launchkey MK4's keys port (its "MIDI" port, the first on Windows).
    pub fn is_keys_port(name: &str) -> bool {
        let lower = name.to_ascii_lowercase();
        lower.contains("launchkey") && lower.contains("mk4") && Model::of_port(name).is_none()
    }

    fn product(self) -> u8 {
        match self {
            Model::Mk4 { .. } => 0x14,
            Model::Mini => 0x13,
        }
    }

    /// A SysEx message: Novation's header, `body`, the end.
    pub fn sysex(self, body: &[u8]) -> Vec<u8> {
        let mut m = vec![0xf0, 0x00, 0x20, 0x29, 0x02, self.product()];
        m.extend_from_slice(body);
        m.push(0xf7);
        m
    }

    /// The name Settings > MIDI shows as the device's controller type.
    pub fn type_name(self) -> &'static str {
        match self {
            Model::Mk4 { .. } => "Novation Launchkey MK4 DAW",
            Model::Mini => "Novation Launchkey Mini MK4 DAW",
        }
    }
}

/// An LED: a button (by its controller number), a pad of the DAW layout, or a pad of the drum layout (by note).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Led {
    Button(u8),
    Pad(u8),
    Drum(u8),
}

/// An LED's colour: from the keyboard's 128-colour palette, or RGB (7 bits each).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Colour {
    Palette(u8),
    Rgb(u8, u8, u8),
}

pub const OFF: Colour = Colour::Palette(0);
/// Dim white: an arrow that can go further.
pub const DIM: Colour = Colour::Palette(1);
/// White: on, held.
pub const WHITE: Colour = Colour::Palette(3);
pub const RED: Colour = Colour::Palette(5);
pub const DARK_RED: Colour = Colour::Palette(7);
pub const GREEN: Colour = Colour::Palette(21);

impl Colour {
    /// A Lemon Studio colour on the LEDs: halved to 7 bits, then given a 2.25 gamma that keeps the brightest
    /// component's level.
    pub fn of(c: Rgb) -> Colour {
        let v = [c.r / 2, c.g / 2, c.b / 2];
        let max = *v.iter().max().unwrap_or(&0);
        if max == 0 {
            return Colour::Rgb(0, 0, 0);
        }
        let g = v.map(|x| 128.0 * (f64::from(x) / 128.0).powf(2.25));
        let gmax = g.iter().copied().fold(0.0, f64::max);
        let k = f64::from(max) / gmax;
        let [r, g, b] = g.map(|x| (x * k).round_ties_even().clamp(0.0, 127.0) as u8);
        Colour::Rgb(r, g, b)
    }

    fn entry(self, led: Led, out: &mut Vec<u8>) {
        let (target, n) = match led {
            Led::Button(cc) => (0x50, cc),
            Led::Pad(p) => (0x40, pad_note(p)),
            Led::Drum(note) => (0x60, note),
        };
        match self {
            Colour::Palette(i) => out.extend_from_slice(&[target, n, i & 0x7f]),
            Colour::Rgb(r, g, b) => out.extend_from_slice(&[target + 3, n, r & 0x7f, g & 0x7f, b & 0x7f]),
        }
    }
}

/// The LEDs: what they should show (two images, the normal one and the one shown while Shift is held) and what the
/// keyboard was last sent. `flush` sends the differences in one message.
#[derive(Debug, Default)]
pub struct Lights {
    normal: HashMap<Led, Colour>,
    shifted: HashMap<Led, Colour>,
    sent: HashMap<Led, Colour>,
    /// Changed since the last flush, in order.
    order: Vec<Led>,
    shift: bool,
}

impl Lights {
    /// Sets `led` in the normal image (`shift`: in the Shift image).
    pub fn set(&mut self, led: Led, c: Colour, shift: bool) {
        let image = if shift { &mut self.shifted } else { &mut self.normal };
        if image.insert(led, c) != Some(c) && !self.order.contains(&led) {
            self.order.push(led);
        }
    }

    /// Shift was pressed or released: the other image shows.
    pub fn shift(&mut self, held: bool) {
        if self.shift != held {
            self.shift = held;
            let mut all: Vec<Led> = self.normal.keys().chain(self.shifted.keys()).copied().collect();
            all.sort();
            all.dedup();
            for led in all {
                if !self.order.contains(&led) {
                    self.order.push(led);
                }
            }
        }
    }

    fn shown(&self, led: Led) -> Colour {
        let image = if self.shift { &self.shifted } else { &self.normal };
        image.get(&led).copied().unwrap_or(OFF)
    }

    /// The message that brings the keyboard up to date, if anything changed.
    pub fn flush(&mut self, model: Model) -> Option<Vec<u8>> {
        let mut body = vec![0x01];
        for led in std::mem::take(&mut self.order) {
            let c = self.shown(led);
            if self.sent.get(&led) != Some(&c) {
                c.entry(led, &mut body);
                self.sent.insert(led, c);
            }
        }
        (body.len() > 1).then(|| model.sysex(&body))
    }

    /// Everything off (leaving DAW mode).
    pub fn all_off(&mut self) {
        for image in [&mut self.normal, &mut self.shifted] {
            for c in image.values_mut() {
                *c = OFF;
            }
        }
        let mut leds: Vec<Led> = self.sent.keys().copied().collect();
        leds.sort();
        self.order = leds;
    }

    /// The keyboard was (re)connected: nothing is known to be lit.
    pub fn forget(&mut self) {
        *self = Lights::default();
    }
}

/// A display on the screen.
pub mod display {
    /// Fader 1–8's are `FADER_1 + i`; the master fader's is `FADER_1 + 8`.
    pub const FADER_1: u8 = 0x05;
    /// Encoder 1–8's are `ENCODER_1 + i`.
    pub const ENCODER_1: u8 = 0x15;
    /// Shown when nothing else is.
    pub const STATIONARY: u8 = 0x20;
    /// Notifications.
    pub const TEMPORARY: u8 = 0x21;
}

/// How a display is laid out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Text {
    /// Name, value.
    Two(String, String),
    /// Title, name, value.
    Three(String, String, String),
    /// A title and 8 fields.
    Grid(String, [String; 8]),
}

impl Text {
    pub fn two(a: impl Into<String>, b: impl Into<String>) -> Text {
        Text::Two(a.into(), b.into())
    }

    pub fn three(a: impl Into<String>, b: impl Into<String>, c: impl Into<String>) -> Text {
        Text::Three(a.into(), b.into(), c.into())
    }

    fn arrangement(&self) -> u8 {
        match self {
            Text::Two(..) => 1,
            Text::Three(..) => 2,
            Text::Grid(..) => 3,
        }
    }

    /// The fields, written last to first (the first carries the trigger).
    fn fields(&self) -> Vec<(u8, &str)> {
        match self {
            Text::Two(a, b) => vec![(1, b), (0, a)],
            Text::Three(a, b, c) => vec![(2, c), (1, b), (0, a)],
            Text::Grid(t, f) => {
                let mut v: Vec<(u8, &str)> = f.iter().enumerate().map(|(i, s)| (i as u8 + 1, s.as_str())).collect();
                v.push((0, t));
                v
            }
        }
    }
}

/// The screen's quiet periods: after the stationary display shows, other displays wait half a second before popping
/// up; after a notification, a tenth.
const QUIET_STATIONARY: Duration = Duration::from_millis(500);
const QUIET_TEMPORARY: Duration = Duration::from_millis(100);

/// The screen: the text and arrangement last sent to each display (identical writes are not sent again) and when a
/// display last popped up.
#[derive(Debug, Default)]
pub struct Screen {
    configs: HashMap<u8, u8>,
    texts: HashMap<u8, Text>,
    /// The last time the stationary display or a notification showed, and the quiet period after it.
    shown: Option<(Instant, Duration)>,
}

impl Screen {
    /// The messages that write `text` to display `at`: the arrangement when it changed, then the fields; the first
    /// field pops the display up when `at` shows by itself (the encoders' show when touched or turned).
    pub fn write(&mut self, model: Model, at: u8, text: Text, now: Instant) -> Vec<Vec<u8>> {
        if self.texts.get(&at) == Some(&text) {
            return Vec::new();
        }
        let encoder = (display::ENCODER_1..display::ENCODER_1 + 8).contains(&at);
        // Encoders show their display on touch and on change by themselves.
        let config = text.arrangement() | if encoder { 0x60 } else { 0 };
        let mut out = Vec::new();
        if self.configs.insert(at, config) != Some(config) {
            out.push(model.sysex(&[0x04, at, config]));
        }
        let quiet = self.shown.is_none_or(|(t, q)| now.duration_since(t) > q);
        let trigger = match at {
            _ if encoder => false,
            display::STATIONARY => {
                self.shown = Some((now, QUIET_STATIONARY));
                true
            }
            display::TEMPORARY => {
                let t = self.shown.is_some() && quiet;
                if t {
                    self.shown = Some((now, QUIET_TEMPORARY));
                }
                t
            }
            _ => quiet,
        };
        for (field, s) in text.fields() {
            let mut body = vec![0x06, at, field | if field == 0 && trigger { 0x40 } else { 0 }];
            body.extend(s.chars().map(|c| if c.is_ascii() && !c.is_ascii_control() { c as u8 } else { b'?' }));
            out.push(model.sysex(&body));
        }
        self.texts.insert(at, text);
        out
    }

    /// The keyboard was (re)connected: nothing is known to be on the screen.
    pub fn forget(&mut self) {
        *self = Screen::default();
    }
}

/// A fader's position as a volume (0..=1, 0.8 = 0 dB): a little dead zone holds 0 dB.
pub fn fader_volume(p: f64) -> f64 {
    if p >= 0.825 {
        0.8 + (p - 0.825) * (0.2 / 0.175)
    } else if p > 0.775 {
        0.8
    } else {
        p * (0.8 / 0.775)
    }
}

/// One detent of a relative encoder, in 0..=1.
pub const STEP: f64 = 1.0 / 127.0;

/// A value turned by an encoder that stops at a centre (0 dB, pan centre): it lands exactly on `centre` and stays
/// there for about `width` more turning the same way. A turn that starts on the centre (a fresh pan) leaves at once.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sticky {
    pub centre: f64,
    pub width: f64,
    /// The last turn landed on the centre.
    landed: bool,
    /// Turning absorbed there since.
    held: f64,
}

impl Sticky {
    pub fn new(centre: f64, width: f64) -> Sticky {
        Sticky { centre, width, landed: false, held: 0.0 }
    }

    /// `v` turned by `d`.
    pub fn turn(&mut self, v: f64, d: f64) -> f64 {
        let c = self.centre;
        if v == c && self.landed {
            self.held = if self.held * d < 0.0 { d } else { self.held + d };
            if self.held.abs() <= self.width {
                return c;
            }
            let out = c + self.held - self.width.copysign(self.held);
            (self.landed, self.held) = (false, 0.0);
            return out.clamp(0.0, 1.0);
        }
        (self.landed, self.held) = (false, 0.0);
        let n = v + d;
        if v != c && ((v < c && n >= c) || (v > c && n <= c)) {
            self.landed = true;
            return c;
        }
        n.clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages() {
        assert_eq!(parse(&[0x9f, 0x0c, 0x7f]), Some(In::Daw(true)));
        assert_eq!(parse(&[0x8f, 0x0c, 0x00]), Some(In::Daw(false)));
        assert_eq!(parse(&[0xb0, button::PLAY, 127]), Some(In::Button { cc: button::PLAY, down: true }));
        assert_eq!(parse(&[0xb6, 0x3f, 0]), Some(In::Shift(false)));
        assert_eq!(parse(&[0xb6, 0x1d, 2]), Some(In::PadLayout(Pads::Daw)));
        assert_eq!(parse(&[0xb6, 0x1e, 5]), Some(In::EncoderLayout(Encoders::Transport)));
        assert_eq!(parse(&[0xb6, 0x1f, 6]), Some(In::FaderLayout(Faders::Other)));
        assert_eq!(parse(&[0x90, 0x61, 90]), Some(In::Pad { pad: 1, vel: 90 }));
        assert_eq!(parse(&[0x90, 0x77, 0]), Some(In::Pad { pad: 15, vel: 0 }));
        assert_eq!(parse(&[0x80, 0x70, 64]), Some(In::Pad { pad: 8, vel: 0 }), "a note off is a release");
        assert_eq!(parse(&[0x99, 36, 100]), Some(In::Drum { note: 36, vel: 100 }));
        assert_eq!(parse(&[0xbf, 0x55, 65]), Some(In::Encoder { i: 0, steps: 1 }));
        assert_eq!(parse(&[0xbf, 0x5c, 61]), Some(In::Encoder { i: 7, steps: -3 }));
        assert_eq!(parse(&[0xbf, 0x0d, 127]), Some(In::Fader { i: 8, pos: 1.0 }));
        assert_eq!(parse(&[0x92, 0x60, 1]), None, "pads come on channel 1");
    }

    #[test]
    fn ports() {
        assert_eq!(Model::of_port("Launchkey MK4 61:Launchkey MK4 61 DAW In 24:1"), Some(Model::Mk4 { faders: true }));
        assert_eq!(Model::of_port("Launchkey MK4 25:Launchkey MK4 25 DAW In 24:1"), Some(Model::Mk4 { faders: false }));
        assert_eq!(Model::of_port("Launchkey MK4 61:Launchkey MK4 61 MIDI In 24:0"), None, "the keys' port");
        assert_eq!(Model::of_port("MIDIIN2 (Launchkey Mini MK4 37)"), Some(Model::Mini));
        assert_eq!(Model::of_port("Launchkey MK3 49 LKMK3 DAW Out"), None);
    }

    #[test]
    fn lights_send_only_changes_and_switch_images_with_shift() {
        let m = Model::Mk4 { faders: true };
        let mut l = Lights::default();
        l.set(Led::Button(button::PLAY), WHITE, false);
        l.set(Led::Pad(9), Colour::Rgb(1, 2, 3), false);
        l.set(Led::Button(button::UNDO), DIM, true);
        assert_eq!(
            l.flush(m),
            Some(vec![0xf0, 0, 0x20, 0x29, 2, 0x14, 1, 0x50, 0x73, 3, 0x43, 0x71, 1, 2, 3, 0x50, 0x4d, 0, 0xf7])
        );
        l.set(Led::Button(button::PLAY), WHITE, false);
        assert_eq!(l.flush(m), None, "nothing changed");
        l.shift(true);
        // Shift shows its own image: Play goes dark, Undo shows Redo's colour.
        assert_eq!(l.flush(m), Some(vec![0xf0, 0, 0x20, 0x29, 2, 0x14, 1, 0x50, 0x4d, 1, 0x50, 0x73, 0, 0x40, 0x71, 0, 0xf7]));
        l.shift(false);
        l.all_off();
        // Only Undo was still lit (the others went dark with the Shift image).
        assert_eq!(l.flush(m), Some(vec![0xf0, 0, 0x20, 0x29, 2, 0x14, 1, 0x50, 0x4d, 0, 0xf7]));
    }

    #[test]
    fn colours() {
        assert_eq!(Colour::of(Rgb::new(0, 0, 0)), Colour::Rgb(0, 0, 0));
        assert_eq!(Colour::of(Rgb::new(255, 255, 255)), Colour::Rgb(127, 127, 127));
        // The brightest component keeps its level; the others darken.
        let Colour::Rgb(r, g, b) = Colour::of(Rgb::new(255, 128, 0)) else { panic!() };
        assert_eq!((r, b), (127, 0));
        assert!(g < 32, "{g}");
    }

    #[test]
    fn screen() {
        let m = Model::Mk4 { faders: true };
        let t0 = Instant::now();
        let mut s = Screen::default();
        // No notification pops up before the stationary display has shown once.
        let first = s.write(m, display::TEMPORARY, Text::two("Mixer", "Channel Pan"), t0);
        assert_eq!(first[0], m.sysex(&[0x04, 0x21, 0x01]));
        assert_eq!(first[2], m.sysex(&[0x06, 0x21, 0x00, b'M', b'i', b'x', b'e', b'r']));
        let st = s.write(m, display::STATIONARY, Text::three("", "Lemon Studio", ""), t0);
        assert_eq!(st.len(), 4);
        assert_eq!(st[3][8], 0x40, "the stationary display pops up");
        assert!(s.write(m, display::STATIONARY, Text::three("", "Lemon Studio", ""), t0).is_empty(), "same text");
        // Within half a second of it, a notification is written but not shown.
        let n = s.write(m, display::TEMPORARY, Text::two("Track 2", "Bass"), t0 + Duration::from_millis(100));
        assert_eq!(n.last().unwrap()[8], 0x00);
        let n = s.write(m, display::TEMPORARY, Text::two("Track 3", "Lead"), t0 + Duration::from_millis(700));
        assert_eq!(n.last().unwrap()[8], 0x40);
        // Encoders show by themselves when touched.
        let e = s.write(m, display::ENCODER_1, Text::three("Kick", "Level", "0.0 dB"), t0);
        assert_eq!(e[0], m.sysex(&[0x04, 0x15, 0x62]));
        assert!(e[1..].iter().all(|x| x[8] & 0x40 == 0));
        let odd = s.write(m, display::STATIONARY, Text::two("é", ""), t0);
        assert_eq!(odd.last().unwrap()[9], b'?');
    }

    #[test]
    fn faders_hold_zero_db() {
        assert_eq!(fader_volume(0.8), 0.8);
        assert_eq!(fader_volume(0.0), 0.0);
        assert!((fader_volume(1.0) - 1.0).abs() < 1e-12);
        assert!((fader_volume(0.775) - 0.8).abs() < 1e-12);
    }

    #[test]
    fn sticky_centre() {
        let mut s = Sticky::new(0.5, 0.1);
        assert_eq!(s.turn(0.45, 0.1), 0.5, "crossing the centre lands on it");
        let mut v = 0.5;
        for _ in 0..9 {
            v = s.turn(v, 0.01);
        }
        assert_eq!(v, 0.5, "held for the width");
        v = s.turn(v, 0.02);
        assert!(v > 0.5 && v < 0.53, "{v}");
        assert_eq!(s.turn(1.0, 0.2), 1.0);
        let mut fresh = Sticky::new(0.5, 0.1);
        assert_eq!(fresh.turn(0.5, 0.05), 0.55, "a turn starting on the centre leaves it at once");
    }
}
