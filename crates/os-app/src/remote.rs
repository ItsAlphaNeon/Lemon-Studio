// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Remote control: MIDI controllers linked to knobs. The links (kept in `settings.txt`, key `midi_links`), how a
//! controller value drives its knob (formula, mode, pickup, smoothing), the Remote control settings box ("Link to
//! controller..." on a knob's menu; drawn by `views::remote_box`) and Multilink to controllers.
// spec: ui/remote-control.md

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::app::{App, State};
use crate::knobs::KnobId;
use crate::playlist::{Cmd, PItem};
use crate::typein::Target;

/// How a link treats a button's 127 / 0 (spec §3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Default,
    Hold,
    Latch,
    Inc,
    Stay,
}

impl Mode {
    pub const ALL: [Mode; 5] = [Mode::Default, Mode::Hold, Mode::Latch, Mode::Inc, Mode::Stay];

    pub fn name(self) -> &'static str {
        match self {
            Mode::Default => "Default",
            Mode::Hold => "Hold",
            Mode::Latch => "Latch",
            Mode::Inc => "Inc",
            Mode::Stay => "Stay",
        }
    }
}

/// The mapping formula (spec §3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Formula {
    #[default]
    Input,
    Inverted,
    Square,
    Root,
}

impl Formula {
    pub const ALL: [Formula; 4] = [Formula::Input, Formula::Inverted, Formula::Square, Formula::Root];

    pub fn name(self) -> &'static str {
        match self {
            Formula::Input => "Input",
            Formula::Inverted => "1 - Input",
            Formula::Square => "Input ^ 2",
            Formula::Root => "Sqrt(Input)",
        }
    }

    /// The note beside the combo.
    pub fn note(self) -> &'static str {
        match self {
            Formula::Input => "(default 1:1 mapping)",
            Formula::Inverted => "(inverted)",
            Formula::Square => "(slow start)",
            Formula::Root => "(fast start)",
        }
    }

    pub fn map(self, x: f32) -> f32 {
        let x = x.clamp(0.0, 1.0);
        match self {
            Formula::Input => x,
            Formula::Inverted => 1.0 - x,
            Formula::Square => x * x,
            Formula::Root => x.sqrt(),
        }
    }
}

/// Smoothing time's range and default, in milliseconds (spec §3).
pub const SMOOTH_MIN: u16 = 10;
pub const SMOOTH_MAX: u16 = 1000;
pub const SMOOTH_DEFAULT: u16 = 100;

/// A controller linked to a knob.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Link {
    /// The device's port (Settings > MIDI); `None` = any device.
    pub port: Option<u8>,
    /// MIDI channel 0..15.
    pub ch: u8,
    pub cc: u8,
    pub mode: Mode,
    pub formula: Formula,
    /// Smoothing time in milliseconds, when on.
    pub smooth: Option<u16>,
    /// Drives the selected channel's knob of the same kind (channel volume / pan targets).
    pub omni: bool,
    pub target: Target,
}

impl Link {
    pub fn new(ch: u8, cc: u8, target: Target) -> Link {
        Link { port: None, ch, cc, mode: Mode::Default, formula: Formula::Input, smooth: None, omni: false, target }
    }

    /// Whether a controller message from a device of port `port` drives it.
    fn hears(&self, port: Option<u8>, ch: u8, cc: u8) -> bool {
        self.ch == ch && self.cc == cc && (self.port.is_none() || self.port == port)
    }

    /// The same controller as `o` (Remove conflicts).
    fn same_controller(&self, o: &Link) -> bool {
        self.ch == o.ch && self.cc == o.cc && self.port == o.port
    }
}

/// Whether `t` can be linked: the Strum and Arpeggiate dialogs' knobs live only while their dialog is open.
fn linkable(t: Target) -> bool {
    !matches!(t, Target::Knob(KnobId::Strum(_) | KnobId::Arp(_)))
}

/// A target as `settings.txt` keeps it (`none` for one that is not linkable, which reads back as nothing).
fn target_text(t: Target) -> String {
    match t {
        Target::Knob(k) => match k {
            KnobId::MasterVol => "mainvol".into(),
            KnobId::MasterPitch => "mainpitch".into(),
            KnobId::MixPan(m) => format!("mixpan:{m}"),
            KnobId::MixSep(m) => format!("mixsep:{m}"),
            KnobId::SlotMix(m, s) => format!("slotmix:{m}:{s}"),
            KnobId::Send(a, b) => format!("send:{a}:{b}"),
            KnobId::ChanVol(r) => format!("chanvol:{r}"),
            KnobId::ChanPan(r) => format!("chanpan:{r}"),
            KnobId::Swing => "swing".into(),
            KnobId::Eq(m, b, f) => format!("eq:{m}:{b}:{f}"),
            KnobId::Strum(_) | KnobId::Arp(_) => "none".into(),
        },
        Target::Fader(m) => format!("fader:{m}"),
        Target::Param(key, i) => format!("param:{key}:{i}"),
    }
}

fn parse_target(s: &str) -> Option<Target> {
    let mut f = s.split(':');
    let kind = f.next()?;
    let mut n = || f.next().and_then(|v| v.parse::<usize>().ok());
    Some(match kind {
        "mainvol" => Target::Knob(KnobId::MasterVol),
        "mainpitch" => Target::Knob(KnobId::MasterPitch),
        "swing" => Target::Knob(KnobId::Swing),
        "mixpan" => Target::Knob(KnobId::MixPan(n()? as u16)),
        "mixsep" => Target::Knob(KnobId::MixSep(n()? as u16)),
        "slotmix" => Target::Knob(KnobId::SlotMix(n()? as u16, n()? as u8)),
        "send" => Target::Knob(KnobId::Send(n()? as u16, n()? as u16)),
        "chanvol" => Target::Knob(KnobId::ChanVol(n()?)),
        "chanpan" => Target::Knob(KnobId::ChanPan(n()?)),
        "eq" => Target::Knob(KnobId::Eq(n()? as u16, n()? as u8, n()? as u8)),
        "fader" => Target::Fader(n()? as u16),
        "param" => Target::Param(n()?, n()?),
        _ => return None,
    })
}

/// `port|channel|controller|mode|formula|smoothing ms (0 = off)|omni|target`.
fn link_text(l: &Link) -> String {
    let mode = Mode::ALL.iter().position(|&m| m == l.mode).unwrap_or(0);
    let formula = Formula::ALL.iter().position(|&f| f == l.formula).unwrap_or(0);
    let port = l.port.map_or_else(|| "---".into(), |p| p.to_string());
    format!("{port}|{}|{}|{mode}|{formula}|{}|{}|{}", l.ch, l.cc, l.smooth.unwrap_or(0), u8::from(l.omni), target_text(l.target))
}

fn parse_link(s: &str) -> Option<Link> {
    let f: Vec<&str> = s.split('|').map(str::trim).collect();
    if f.len() != 8 {
        return None;
    }
    let smooth = f[5].parse::<u16>().ok()?;
    Some(Link {
        port: f[0].parse().ok(),
        ch: f[1].parse::<u8>().ok()?.min(15),
        cc: f[2].parse::<u8>().ok()?.min(127),
        mode: *Mode::ALL.get(f[3].parse::<usize>().ok()?)?,
        formula: *Formula::ALL.get(f[4].parse::<usize>().ok()?)?,
        smooth: (smooth > 0).then(|| smooth.clamp(SMOOTH_MIN, SMOOTH_MAX)),
        omni: f[6] == "1",
        target: parse_target(f[7])?,
    })
}

const KEY: &str = "midi_links";

/// The links kept (spec §5).
pub fn links() -> Vec<Link> {
    crate::settings_file::get(KEY).map(|v| v.split(';').filter_map(parse_link).collect()).unwrap_or_default()
}

pub fn set_links(l: &[Link]) {
    crate::settings_file::write(KEY, &l.iter().map(link_text).collect::<Vec<_>>().join(";"));
}

/// The link of target `t`, if it has one.
pub fn link_of(t: Target) -> Option<Link> {
    links().into_iter().find(|l| l.target == t)
}

/// Makes link `l`: it replaces the target's earlier link; with `remove_conflicts`, links of the same controller go too.
pub fn add(l: Link, remove_conflicts: bool) {
    let mut all = links();
    all.retain(|o| o.target != l.target && !(remove_conflicts && o.same_controller(&l)));
    all.push(l);
    set_links(&all);
}

pub fn remove(t: Target) {
    let mut all = links();
    all.retain(|o| o.target != t);
    set_links(&all);
}

/// The menu's Remote control part (spec §1): Link to controller..., Override global link... (greyed), and Remove link
/// for a linked target.
pub fn menu_items(t: Option<Target>) -> Vec<PItem> {
    let t = t.filter(|&t| linkable(t));
    let mut v = vec![
        PItem::head("Remote control"),
        match t {
            Some(t) => PItem::new("Link to controller...", Cmd::Remote(RCmd::Link(t))),
            None => PItem::not_yet("Link to controller..."),
        },
        PItem::new("Override global link...", Cmd::Nothing).off(),
    ];
    if let Some(t) = t.filter(|&t| link_of(t).is_some()) {
        v.push(PItem::new("Remove link", Cmd::Remote(RCmd::Unlink(t))));
    }
    v
}

/// The menu commands.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RCmd {
    Link(Target),
    Unlink(Target),
}

/// The Remote control settings box (spec §2, §3), while open.
#[derive(Debug, Clone, PartialEq)]
pub struct LinkBox {
    pub target: Target,
    /// Its top left.
    pub at: (f32, f32),
    pub port: Option<u8>,
    /// 0..15 (shown 1..16).
    pub ch: u8,
    pub cc: Option<u8>,
    pub auto: bool,
    pub omni: bool,
    pub mode: Mode,
    pub formula: Formula,
    pub smooth: bool,
    pub time: u16,
    pub conflicts: bool,
}

impl LinkBox {
    pub fn new(target: Target, at: (f32, f32)) -> LinkBox {
        let mut b = LinkBox {
            target,
            at,
            port: None,
            ch: 0,
            cc: None,
            auto: true,
            omni: false,
            mode: Mode::Default,
            formula: Formula::Input,
            smooth: false,
            time: SMOOTH_DEFAULT,
            conflicts: true,
        };
        // An existing link opens with its settings.
        if let Some(l) = link_of(target) {
            b.port = l.port;
            b.ch = l.ch;
            b.cc = Some(l.cc);
            b.omni = l.omni;
            b.mode = l.mode;
            b.formula = l.formula;
            b.smooth = l.smooth.is_some();
            b.time = l.smooth.unwrap_or(SMOOTH_DEFAULT);
        }
        b
    }

    /// Reset: every field back to its default (spec §3).
    pub fn reset(&mut self) {
        *self = LinkBox { target: self.target, at: self.at, ..LinkBox::new_blank(self.target) };
    }

    fn new_blank(target: Target) -> LinkBox {
        LinkBox {
            target,
            at: (0.0, 0.0),
            port: None,
            ch: 0,
            cc: None,
            auto: true,
            omni: false,
            mode: Mode::Default,
            formula: Formula::Input,
            smooth: false,
            time: SMOOTH_DEFAULT,
            conflicts: true,
        }
    }

    /// The link it makes on Accept: none while Ctrl is "---".
    pub fn link(&self) -> Option<Link> {
        Some(Link {
            port: self.port,
            ch: self.ch,
            cc: self.cc?,
            mode: self.mode,
            formula: self.formula,
            smooth: self.smooth.then_some(self.time),
            omni: self.omni && omni_kind(self.target),
            target: self.target,
        })
    }
}

/// Targets Omni works for: a rack channel's knobs.
pub fn omni_kind(t: Target) -> bool {
    matches!(t, Target::Knob(KnobId::ChanVol(_) | KnobId::ChanPan(_)))
}

/// The box's parts (`Action::Remote`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RAct {
    /// The panel (swallows clicks).
    Body,
    Close,
    Port,
    Channel,
    Ctrl,
    Auto,
    Omni,
    Mode,
    Formula,
    Smooth,
    Time,
    Conflicts,
    Reset,
    Accept,
}

impl RAct {
    /// A value box (the wheel and a vertical drag step it).
    pub fn is_value(self) -> bool {
        matches!(self, RAct::Port | RAct::Channel | RAct::Ctrl | RAct::Time)
    }
}

/// Multilink to controllers (spec §4): on, and the knobs changed since, in order.
#[derive(Debug, Default, Clone)]
pub struct Multi {
    pub on: bool,
    pub queue: VecDeque<Target>,
}

/// A link's state while the program runs.
#[derive(Debug, Clone, Copy, Default)]
struct Live {
    /// The value this link last gave its knob.
    set: Option<f32>,
    /// Its last mapped input.
    last_in: Option<f32>,
    /// Pickup: the controller has met the knob.
    engaged: bool,
    /// Smoothing: from, to, since, over.
    glide: Option<(f32, f32, Instant, Duration)>,
}

/// The links read from `settings.txt` and their live state.
#[derive(Debug, Default)]
pub struct Remote {
    links: Vec<Link>,
    live: Vec<Live>,
    /// The `midi_links` text `links` was read from.
    read: Option<String>,
}

impl Remote {
    /// The kept links, read again when `settings.txt`'s text changed.
    fn sync(&mut self) {
        let now = crate::settings_file::get(KEY);
        if self.read != now {
            self.links = links();
            self.live = vec![Live::default(); self.links.len()];
            self.read = now;
        }
    }

    /// A glide is running: the shell keeps ticking.
    pub fn busy(&self) -> bool {
        self.live.iter().any(|l| l.glide.is_some())
    }
}

/// A value close enough to another to count as it.
const NEAR: f32 = 1.0 / 127.0;

impl App {
    /// The knob a link drives now (Omni: the selected channel's).
    fn remote_target(&self, l: &Link) -> Target {
        match l.target {
            Target::Knob(KnobId::ChanVol(_)) if l.omni => Target::Knob(KnobId::ChanVol(self.state.selected_channel)),
            Target::Knob(KnobId::ChanPan(_)) if l.omni => Target::Knob(KnobId::ChanPan(self.state.selected_channel)),
            t => t,
        }
    }

    /// A controller message from device `dev` (spec §2-§5): the open box detects it, Multilink links it, the links it
    /// drives take it. Returns whether something took it (otherwise it goes to the instrument).
    pub(crate) fn remote_cc(&mut self, dev: &str, ch: u8, cc: u8, v: u8) -> bool {
        let port = crate::midi_cfg::input(dev).port;
        if let Some(b) = self.state.link_box.as_mut().filter(|b| b.auto) {
            b.port = port;
            b.ch = ch;
            b.cc = Some(cc);
            self.state.message = Some(format!("Detected: channel {}, controller {cc}", ch + 1));
            if crate::prefs::check(&crate::prefs::AUTO_ACCEPT) {
                self.link_accept();
            }
            self.invalidate();
            return true;
        }
        self.remote.sync();
        let hit: Vec<usize> = (0..self.remote.links.len()).filter(|&i| self.remote.links[i].hears(port, ch, cc)).collect();
        if hit.is_empty() && self.state.multilink.on {
            if let Some(t) = self.state.multilink.queue.pop_front() {
                let mut l = Link::new(ch, cc, t);
                l.port = port;
                add(l, true);
                self.remote.sync();
                let name = crate::typein::caption(&self.state, t);
                self.state.message = Some(format!("Linked channel {}, controller {cc} to {}", ch + 1, name.trim_start_matches("Value for ")));
                if self.state.multilink.queue.is_empty() {
                    self.state.multilink.on = false;
                }
                self.invalidate();
                return true;
            }
        }
        for i in hit.iter().copied() {
            self.remote_drive(i, v);
        }
        !hit.is_empty()
    }

    /// Link `i` gets controller value `v` (spec §5).
    fn remote_drive(&mut self, i: usize, v: u8) {
        let l = self.remote.links[i];
        let t = self.remote_target(&l);
        let Some(now) = crate::typein::get(&self.state, t).map(|x| x as f32) else { return };
        let x = l.formula.map(f32::from(v) / 127.0);
        let pressed = v > 0;
        let val = match l.mode {
            Mode::Default | Mode::Hold => x,
            Mode::Stay if !pressed => return,
            Mode::Stay => x,
            Mode::Latch if !pressed => return,
            Mode::Latch => {
                if now >= 0.5 { 0.0 } else { 1.0 }
            }
            Mode::Inc if !pressed => return,
            Mode::Inc => {
                let n = now + 1.0 / 16.0;
                if n > 1.0 + 1e-4 { 0.0 } else { n.min(1.0) }
            }
        };
        let live = &mut self.remote.live[i];
        // Pickup (takeover mode): after another change, nothing until the controller meets the knob.
        if crate::prefs::check(&crate::prefs::PICKUP) && matches!(l.mode, Mode::Default | Mode::Hold | Mode::Stay) {
            if live.set.is_none_or(|s| (s - now).abs() > 1e-4) {
                live.engaged = false;
            }
            if !live.engaged {
                let met = (val - now).abs() <= NEAR || live.last_in.is_some_and(|p| (p - now) * (val - now) <= 0.0);
                live.last_in = Some(val);
                if !met {
                    return;
                }
                live.engaged = true;
            }
        }
        live.last_in = Some(val);
        match l.smooth {
            Some(ms) => {
                live.glide = Some((now, val, Instant::now(), Duration::from_millis(u64::from(ms))));
            }
            None => {
                live.set = Some(val);
                self.remote_set(t, val);
            }
        }
    }

    /// The glides move on (spec §3 Smoothing).
    pub(crate) fn remote_tick(&mut self) {
        if !self.remote.busy() {
            return;
        }
        for i in 0..self.remote.live.len() {
            let Some((from, to, since, over)) = self.remote.live[i].glide else { continue };
            let k = (since.elapsed().as_secs_f32() / over.as_secs_f32().max(1e-3)).min(1.0);
            let v = from + (to - from) * k;
            if k >= 1.0 {
                self.remote.live[i].glide = None;
            }
            self.remote.live[i].set = Some(v);
            let t = self.remote_target(&self.remote.links[i]);
            self.remote_set(t, v);
        }
    }

    /// Sets the knob as a change of it with the mouse would: undoable (one step while the same knob keeps changing),
    /// recorded while recording automation.
    fn remote_set(&mut self, t: Target, v: f32) {
        let v = v.clamp(0.0, 1.0);
        match t {
            Target::Knob(k) => {
                crate::history::knob(&mut self.state, k, true);
                self.state.set_knob(k, v);
                self.record_knob(k);
            }
            Target::Fader(m) => {
                if let Some(tr) = crate::mixer::track_mut(&mut self.state, m) {
                    tr.volume = Some(f64::from(v));
                    self.state.piano.dirty = true;
                }
            }
            Target::Param(key, index) => {
                crate::params::set_param_value(&self.state, key, index, f64::from(v));
            }
        }
        self.invalidate();
    }

    /// A menu's Remote control command.
    pub(crate) fn remote_cmd(&mut self, c: RCmd) {
        match c {
            RCmd::Link(t) => {
                let (w, h) = crate::views::remote_box::SIZE;
                let (sw, sh) = self.state.screen;
                let at = ((self.mouse.0 - 20.0).clamp(0.0, (sw - w).max(0.0)), (self.mouse.1 - 20.0).clamp(0.0, (sh - h).max(0.0)));
                self.state.link_box = Some(LinkBox::new(t, at));
                self.state.message = Some("Just move your MIDI controller".into());
            }
            RCmd::Unlink(t) => {
                remove(t);
                self.state.message = Some("Link removed".into());
            }
        }
        self.invalidate();
    }

    /// Accept: makes the link and closes the box (spec §3).
    pub(crate) fn link_accept(&mut self) {
        let Some(b) = self.state.link_box.take() else { return };
        match b.link() {
            Some(l) => {
                add(l, b.conflicts);
                let name = crate::typein::caption(&self.state, b.target);
                self.state.message = Some(format!("Linked channel {}, controller {} to {}", l.ch + 1, l.cc, name.trim_start_matches("Value for ")));
            }
            None => self.state.message = Some("No controller: nothing linked".into()),
        }
        self.invalidate();
    }

    /// A click on the box.
    pub(crate) fn link_box_act(&mut self, a: RAct) {
        let Some(b) = self.state.link_box.as_mut() else { return };
        match a {
            RAct::Body | RAct::Port | RAct::Channel | RAct::Ctrl | RAct::Time => {}
            RAct::Close => self.state.link_box = None,
            RAct::Auto => b.auto = !b.auto,
            RAct::Omni => b.omni = !b.omni,
            RAct::Smooth => b.smooth = !b.smooth,
            RAct::Conflicts => b.conflicts = !b.conflicts,
            RAct::Reset => b.reset(),
            RAct::Accept => return self.link_accept(),
            RAct::Mode => {
                let i = Mode::ALL.iter().position(|&m| m == b.mode).unwrap_or(0);
                b.mode = Mode::ALL[(i + 1) % Mode::ALL.len()];
            }
            RAct::Formula => {
                let i = Formula::ALL.iter().position(|&f| f == b.formula).unwrap_or(0);
                b.formula = Formula::ALL[(i + 1) % Formula::ALL.len()];
            }
        }
        self.invalidate();
    }

    /// The wheel (or a drag) on one of the box's value boxes: `steps` up or down.
    pub(crate) fn link_box_step(&mut self, a: RAct, steps: i32) {
        let Some(b) = self.state.link_box.as_mut() else { return };
        let step = |v: Option<u8>, max: i32| -> Option<u8> {
            // "---" sits below 0.
            let n = v.map_or(-1, i32::from) + steps;
            (n >= 0).then(|| n.min(max) as u8)
        };
        match a {
            RAct::Port => b.port = step(b.port, 255),
            RAct::Ctrl => b.cc = step(b.cc, 127),
            RAct::Channel => b.ch = (i32::from(b.ch) + steps).clamp(0, 15) as u8,
            RAct::Time => b.time = (i32::from(b.time) + steps * 10).clamp(i32::from(SMOOTH_MIN), i32::from(SMOOTH_MAX)) as u16,
            _ => return,
        }
        self.invalidate();
    }

    /// Multilink to controllers switched (OPTIONS, Ctrl+J, the toolbar; spec §4).
    pub(crate) fn toggle_multilink(&mut self) {
        let m = &mut self.state.multilink;
        m.on = !m.on;
        m.queue.clear();
        self.state.message = Some(if m.on {
            "Multilink to controllers: change the knobs to link, then move a controller for each, in the same order".into()
        } else {
            "Multilink to controllers: off".into()
        });
        self.invalidate();
    }

    /// A knob changed with the mouse: Multilink queues it (once).
    pub(crate) fn multilink_knob(&mut self, t: Target) {
        let m = &mut self.state.multilink;
        if m.on && !m.queue.contains(&t) {
            m.queue.push_back(t);
            let name = crate::typein::caption(&self.state, t);
            let n = self.state.multilink.queue.len();
            self.state.message = Some(format!("Multilink {n}: {}", name.trim_start_matches("Value for ")));
        }
    }
}

/// Whether the box is open (it is modal).
pub fn is_open(s: &State) -> bool {
    s.link_box.is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_are_kept() {
        let mut l = Link::new(2, 21, Target::Knob(KnobId::ChanVol(3)));
        l.port = Some(4);
        l.mode = Mode::Inc;
        l.smooth = Some(250);
        let p = Link::new(0, 7, Target::Param(1, 12));
        add(l, true);
        add(p, true);
        assert_eq!(links(), vec![l, p]);
        // The same controller: the earlier link goes.
        let m = Link { target: Target::Fader(2), ..l };
        add(m, true);
        assert_eq!(links(), vec![p, m]);
        remove(Target::Param(1, 12));
        assert_eq!(links(), vec![m]);
        for t in [Target::Knob(KnobId::Eq(1, 2, 0)), Target::Knob(KnobId::Send(1, 0)), Target::Knob(KnobId::MasterPitch)] {
            assert_eq!(parse_target(&target_text(t)), Some(t));
        }
    }

    #[test]
    fn formulas() {
        assert_eq!(Formula::Inverted.map(0.25), 0.75);
        assert_eq!(Formula::Square.map(0.5), 0.25);
        assert_eq!(Formula::Root.map(0.25), 0.5);
    }

    #[test]
    fn the_box_makes_its_link() {
        let mut b = LinkBox::new(Target::Knob(KnobId::ChanPan(0)), (0.0, 0.0));
        assert_eq!(b.link(), None, "no controller yet");
        b.cc = Some(10);
        b.omni = true;
        let l = b.link().unwrap();
        assert!(l.omni && l.cc == 10 && l.port.is_none());
        b.reset();
        assert_eq!(b.cc, None);
    }
}
