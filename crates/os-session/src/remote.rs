// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Remote control: MIDI controllers linked to knobs. The links (kept in `settings.txt`, key `midi_links`), how a
//! controller value drives its knob (formula, mode, pickup, smoothing), the Remote control settings box ("Link to
//! controller..." on a knob's menu; drawn by `views::remote_box`) and Multilink to controllers.
// spec: ui/remote-control.md

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::knob_id::KnobId;
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
    pub fn hears(&self, port: Option<u8>, ch: u8, cc: u8) -> bool {
        self.ch == ch && self.cc == cc && (self.port.is_none() || self.port == port)
    }

    /// The same controller as `o` (Remove conflicts).
    fn same_controller(&self, o: &Link) -> bool {
        self.ch == o.ch && self.cc == o.cc && self.port == o.port
    }
}

/// Whether `t` can be linked: the piano roll tool dialogs' knobs live only while their dialog is open. A rack row's
/// mixer track box and LED can be (Lemon Studio's design): the controller's travel spans Master to the last insert,
/// and turns the channel on from its middle. So can the effects inside a container, their parameters and their wet
/// and dry levels (Lemon Studio's design: spec plugins/patcher/README.md §4.2).
pub fn linkable(t: Target) -> bool {
    // A member's level knob links as its level (`KnobId::target`).
    !matches!(t, Target::Knob(KnobId::Strum(_) | KnobId::Arp(_) | KnobId::PianoDlg(_) | KnobId::AutoLfo(..) | KnobId::MidiOut(..) | KnobId::Member(..)) | Target::Latency(..))
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
            KnobId::Strum(_) | KnobId::Arp(_) | KnobId::PianoDlg(_) | KnobId::AutoLfo(..) | KnobId::MidiOut(..) | KnobId::Member(..) => "none".into(),
        },
        Target::Fader(m) => format!("fader:{m}"),
        Target::Param(key, i) => format!("param:{key}:{i}"),
        Target::RackTrack(r) => format!("racktrack:{r}"),
        Target::RackLed(r) => format!("rackled:{r}"),
        Target::Member(m, k, id, wet) => format!("{}:{m}:{k}:{id}", if wet { "memberwet" } else { "memberdry" }),
        Target::MemberParam(m, k, id, i) => format!("memberparam:{m}:{k}:{id}:{i}"),
        Target::Latency(..) => "none".into(),
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
        "racktrack" => Target::RackTrack(n()?),
        "rackled" => Target::RackLed(n()?),
        "memberwet" | "memberdry" => Target::Member(n()? as u16, n()? as u8, n()? as u32, kind == "memberwet"),
        "memberparam" => Target::MemberParam(n()? as u16, n()? as u8, n()? as u32, n()?),
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

/// Each link on the first target of a pair goes onto the second (an undo or redo of a wrap into a container, which
/// moved the links of the plugins it wrapped: `crate::container`).
pub fn retarget(pairs: &[(Target, Target)]) {
    let mut all = links();
    let mut moved = false;
    for l in &mut all {
        if let Some(&(_, to)) = pairs.iter().find(|p| p.0 == l.target) {
            l.target = to;
            moved = true;
        }
    }
    if moved {
        set_links(&all);
    }
}

/// The links follow what they drive as it moves (mixer places, rack rows): `f` gives a target's new one, `None` when
/// what it drove went, and the link with it. The moves join the edit's (`State::link_moves`), so its undo step puts
/// them back; a link that went does not come back.
pub fn follow(s: &mut crate::session::Session, f: impl Fn(Target) -> Option<Target>) {
    let old = links();
    let mut all = old.clone();
    let before = std::mem::take(&mut s.link_moves);
    let mut moves = Vec::new();
    all.retain_mut(|l| {
        let Some(to) = f(l.target) else { return false };
        let from = before.iter().find(|m| m.1 == l.target).map_or(l.target, |m| m.0);
        if from != to {
            moves.push((from, to));
        }
        l.target = to;
        true
    });
    if all != old {
        set_links(&all);
    }
    s.link_moves = moves;
}

pub fn remove(t: Target) {
    let mut all = links();
    all.retain(|o| o.target != t);
    set_links(&all);
}

/// The menu commands.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RCmd {
    Link(Target),
    Unlink(Target),
    /// A plugin window menu's Link all parameters...: hosted instance key (spec §6).
    LinkAll(usize),
    /// Mixer options ▸ Link all parameters...: the mixer track's own controls (`track_targets`).
    LinkTrack(u16),
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
    /// Link all parameters... (spec §6): the instance's parameters, or a mixer track's controls (the mixer's Link all
    /// parameters..., Lemon Studio's design), to link in order to the controller set and the ones after it (`target`
    /// is the first); empty for a single link.
    pub all: Vec<Target>,
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
            all: Vec::new(),
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
        *self = LinkBox { target: self.target, at: self.at, all: std::mem::take(&mut self.all), ..LinkBox::new_blank(self.target) };
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
            all: Vec::new(),
        }
    }

    /// Link all parameters... of hosted instance `key` (spec §6): its visible parameters `params`, the box blank.
    pub fn all(key: usize, params: Vec<usize>, at: (f32, f32)) -> LinkBox {
        LinkBox::group(params.into_iter().map(|i| Target::Param(key, i)).collect(), at)
    }

    /// Link all parameters... over `targets` (at least one), the box blank.
    pub fn group(targets: Vec<Target>, at: (f32, f32)) -> LinkBox {
        let first = targets.first().copied().unwrap_or(Target::Param(0, 0));
        LinkBox { at, all: targets, ..LinkBox::new_blank(first) }
    }

    /// The links Accept makes: the one link, or for Link all parameters... one per parameter on the controller set and
    /// the ones after it, as far as controller 127 goes.
    pub fn links(&self) -> Vec<Link> {
        let Some(l) = self.link() else { return Vec::new() };
        if self.all.is_empty() {
            return vec![l];
        }
        self.all.iter().zip(l.cc..=127).map(|(&target, cc)| Link { cc, target, ..l }).collect()
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
pub struct Live {
    /// The value this link last gave its knob.
    pub set: Option<f32>,
    /// Its last mapped input.
    pub last_in: Option<f32>,
    /// Pickup: the controller has met the knob.
    pub engaged: bool,
    /// Smoothing: from, to, since, over.
    pub glide: Option<(f32, f32, Instant, Duration)>,
}

/// The links read from `settings.txt` and their live state.
#[derive(Debug, Default)]
pub struct Remote {
    pub links: Vec<Link>,
    pub live: Vec<Live>,
    /// The `midi_links` text `links` was read from.
    read: Option<String>,
}

impl Remote {
    /// The kept links, read again when `settings.txt`'s text changed.
    pub fn sync(&mut self) {
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
pub const NEAR: f32 = 1.0 / 127.0;

/// The mixer's Link all parameters... (Lemon Studio's design): track `t`'s own controls in the strip's order: volume,
/// pan, stereo separation, the equaliser's three bands (gain, frequency, width each), the send levels of its routes,
/// then the mix levels of its filled slots.
pub fn track_targets(s: &crate::session::Session, t: u16) -> Vec<Target> {
    let Some(track) = crate::edit::mixer::track(s, t) else { return Vec::new() };
    let mut v = vec![Target::Fader(t), Target::Knob(KnobId::MixPan(t)), Target::Knob(KnobId::MixSep(t))];
    v.extend((0..3).flat_map(|b| (0..3).map(move |f| Target::Knob(KnobId::Eq(t, b, f)))));
    let mut routes: Vec<u16> = track.routes.iter().copied().filter(|&d| d != t).collect();
    routes.sort_unstable();
    routes.dedup();
    v.extend(routes.into_iter().map(|d| Target::Knob(KnobId::Send(t, d))));
    v.extend((0..10u8).filter(|&k| crate::edit::mixer::slot_name(s, t, k).is_some()).map(|k| Target::Knob(KnobId::SlotMix(t, k))));
    v
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
        for t in [Target::Knob(KnobId::Eq(1, 2, 0)), Target::Knob(KnobId::Send(1, 0)), Target::Knob(KnobId::MasterPitch), Target::RackTrack(4), Target::RackLed(2)] {
            assert_eq!(parse_target(&target_text(t)), Some(t));
        }
        // The effects inside a container, by place and id (their hosted keys are the session's).
        for t in [Target::Member(3, 9, 15, true), Target::Member(0, 0, 1, false), Target::MemberParam(2, 4, 7, 1999)] {
            assert!(linkable(t));
            assert_eq!(parse_target(&target_text(t)), Some(t));
        }
        assert_eq!(target_text(Target::MemberParam(2, 4, 7, 19)), "memberparam:2:4:7:19");
    }

    #[test]
    fn formulas() {
        assert_eq!(Formula::Inverted.map(0.25), 0.75);
        assert_eq!(Formula::Square.map(0.5), 0.25);
        assert_eq!(Formula::Root.map(0.25), 0.5);
    }

    /// Link all parameters... (spec §6): one link per parameter, on the controller set and the ones after it, as far as
    /// controller 127; Reset keeps the parameters.
    #[test]
    fn the_box_links_all_parameters() {
        let mut b = LinkBox::all(7, vec![0, 2, 3, 5], (0.0, 0.0));
        assert_eq!(b.target, Target::Param(7, 0));
        assert!(b.links().is_empty(), "no controller yet");
        b.cc = Some(20);
        b.ch = 1;
        let l = b.links();
        assert_eq!(l.iter().map(|l| (l.cc, l.target)).collect::<Vec<_>>(), [(20, Target::Param(7, 0)), (21, Target::Param(7, 2)), (22, Target::Param(7, 3)), (23, Target::Param(7, 5))]);
        assert!(l.iter().all(|l| l.ch == 1 && !l.omni));
        b.cc = Some(126);
        assert_eq!(b.links().len(), 2, "up to controller 127");
        b.reset();
        assert_eq!((b.cc, b.all.len()), (None, 4));
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
