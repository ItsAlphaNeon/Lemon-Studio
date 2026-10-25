// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The piano roll's Flam (Alt+F), Claw machine (Alt+W), Flip (Alt+Y), Scale levels (Alt+X), Limit (Alt+K), LFO
//! (Alt+O), Quantize (Alt+Q), Chop (Alt+U) and Randomize (Alt+R) dialogs: one tool window shows whichever is open. As
//! with Strum, its session previews every change on the notes (and LFO's on a channel lane's control points), Accept
//! keeps the result as one undo step and × puts the notes back. The tools are `flam.rs`, `claw.rs`, `flip.rs`,
//! `levels.rs`, `limit.rs`, `lfo.rs`, `quantize.rs`, `chop.rs` and `randomize.rs`; the window is `views/piano_dlg.rs`.
// spec: ui/piano-flam.md, ui/piano-claw.md, ui/piano-flip.md, ui/piano-level-scaling.md, ui/piano-limit.md, ui/piano-lfo.md,
// ui/piano-quantize.md, ui/piano-chop.md, ui/piano-randomize.md

use os_model::{ControlPoint, Note};

use crate::app::State;
use crate::piano::{self, Lane};
use crate::playlist::{Cmd, Mark, PItem};
use crate::{chop, chords, claw, flam, flip, levels, lfo, limit, quantize, randomize};

/// Which dialog the window shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tool {
    #[default]
    Flam,
    Flip,
    Levels,
    Limit,
    Claw,
    Lfo,
    Quantize,
    Chop,
    Randomize,
}

impl Tool {
    /// How many there are (`Ui::pos`).
    pub const COUNT: usize = 9;

    pub fn title(self) -> &'static str {
        match self {
            Tool::Flam => "Piano roll - flamer",
            Tool::Flip => "Piano roll - score flipper",
            Tool::Levels => "Piano roll - level scaling",
            Tool::Limit => "Piano roll - limiter / transposer",
            Tool::Claw => "Piano roll - claw machine",
            Tool::Lfo => "Piano roll - LFO",
            Tool::Quantize => "Piano roll - quantizer",
            Tool::Chop => "Piano roll - chopper",
            Tool::Randomize => "Piano roll - randomizer",
        }
    }

    /// The title over the Control lane `lane`: LFO's says "Events" on a channel lane (piano-lfo.md §1).
    pub fn title_on(self, lane: Lane) -> &'static str {
        if self == Tool::Lfo && lane.is_channel() { "Events - LFO" } else { self.title() }
    }

    /// The undo step an Accept makes.
    fn step(self) -> &'static str {
        match self {
            Tool::Flam => "Piano roll flam",
            Tool::Flip => "Piano roll flip",
            Tool::Levels => "Piano roll level scaling",
            Tool::Limit => "Piano roll limit",
            Tool::Claw => "Piano roll claw machine",
            Tool::Lfo => "Piano roll LFO",
            Tool::Quantize => "Piano roll quantize",
            Tool::Chop => "Piano roll chop",
            Tool::Randomize => "Piano roll randomize",
        }
    }

    fn done(self) -> &'static str {
        match self {
            Tool::Flam => "flammed",
            Tool::Flip => "flipped",
            Tool::Levels => "levels scaled",
            Tool::Limit => "limited",
            Tool::Claw => "claw machine applied",
            Tool::Lfo => "LFO applied",
            Tool::Quantize => "quantized",
            Tool::Chop => "chopped",
            Tool::Randomize => "randomized",
        }
    }

    pub fn index(self) -> usize {
        self as usize
    }
}

/// A knob or fader of the dialogs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Knob {
    FlamTime,
    FlamVel,
    Center,
    Mult,
    Offset,
    Tension,
    ClawTime,
    /// LFO's Start and End columns (Value, Range, Speed) and its Phase.
    LfoValue,
    LfoRange,
    LfoSpeed,
    LfoEndValue,
    LfoEndRange,
    LfoEndSpeed,
    LfoPhase,
    /// Quantize's Start time, Sensitivity and Duration, and its Levels (an index into `quantize::LEVELS`).
    QStart,
    QSens,
    QDur,
    QLevel(u8),
    /// Chop's Time mul and its Levels (an index into `quantize::LEVELS`).
    ChopTimeMul,
    ChopLevel(u8),
    /// Randomize's Octave, Range and Stack boxes (they turn like knobs), its Length, Variation and Population knobs
    /// and its Levels (an index into `quantize::LEVELS`).
    RandOctave,
    RandRange,
    RandStack,
    RandLength,
    RandVariation,
    RandPopulation,
    RandLevel(u8),
}

impl Knob {
    /// Change of the 0..1 position per wheel notch (piano-flam.md §1, piano-level-scaling.md §1, piano-claw.md §1,
    /// piano-lfo.md §1, piano-quantize.md §1, piano-chop.md §1, piano-randomize.md §1).
    pub fn notch(self) -> f32 {
        match self {
            Knob::FlamTime => flam::TIME_NOTCH,
            Knob::FlamVel => flam::VEL_NOTCH,
            Knob::Center => 1.0 / 32.0,
            Knob::Mult | Knob::Offset | Knob::Tension => 1.0 / 64.0,
            Knob::ClawTime => claw::TIME_NOTCH,
            Knob::LfoValue | Knob::LfoEndValue => lfo::VALUE_NOTCH,
            Knob::LfoRange | Knob::LfoEndRange => lfo::RANGE_NOTCH,
            Knob::LfoSpeed | Knob::LfoEndSpeed => lfo::SPEED_NOTCH,
            Knob::LfoPhase => lfo::PHASE_NOTCH,
            Knob::QStart | Knob::QDur => quantize::STRENGTH_NOTCH,
            Knob::QSens => quantize::SENS_NOTCH,
            Knob::QLevel(_) => quantize::LEVEL_NOTCH,
            Knob::ChopTimeMul => chop::TIME_MUL_NOTCH,
            Knob::ChopLevel(_) => chop::LEVEL_NOTCH,
            Knob::RandOctave => randomize::OCTAVE_NOTCH,
            Knob::RandRange | Knob::RandStack => randomize::COUNT_NOTCH,
            Knob::RandLength => randomize::LENGTH_NOTCH,
            Knob::RandVariation => randomize::VARIATION_NOTCH,
            Knob::RandPopulation => randomize::POPULATION_NOTCH,
            Knob::RandLevel(_) => randomize::LEVEL_NOTCH,
        }
    }

    /// Track length in px of the faders (`views/piano_dlg.rs`): a drag moves the handle with the mouse.
    pub fn travel(self) -> Option<f32> {
        match self {
            Knob::Center => Some(72.0),
            Knob::Mult | Knob::Offset => Some(144.0),
            _ => None,
        }
    }

    /// Position after Reset; Center's is on the velocity lane.
    pub fn default_pos(self) -> f32 {
        let (f, l, c) = (flam::Settings::default(), levels::Settings::default(), claw::Settings::default());
        Ui::pos_of(self, &f, &l, &c, &lfo::Settings::default(), &quantize::Settings::default(), &chop::Settings::default(), &randomize::Settings::default())
    }
}

/// A switch or button of the dialogs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Act {
    /// Flam's Options ▸ menu of presets.
    Options,
    Absolute,
    Before,
    Group,
    Horizontal,
    Preserve,
    Vertical,
    /// Limit's Key, scale and Snap ▸ menus.
    KeyMenu,
    ScaleMenu,
    SnapMenu,
    Wrap,
    /// Claw machine's Period and Trash every ▸ menus and its two switches.
    PeriodMenu,
    TrashMenu,
    Stretch,
    Short,
    /// LFO's End and log switches and its Shape ▸ menu.
    LfoEnd,
    LfoLog,
    ShapeMenu,
    /// Quantize's template folder button (a file dialog) and ▸ menu, and its duration menu; Chop's pattern folder
    /// button and ▸ menu are the first two.
    TemplateFolder,
    TemplateMenu,
    DurMenu,
    /// Limit's section header arrows: 0 = Key / scale restriction, 1 = Keyboard range.
    Collapse(u8),
    /// Randomize's section switches, its four switches and its seed arrows (`levels`: the Levels section's; `up`: ›).
    RandPattern,
    RandLevels,
    Porta,
    Glue,
    ResetBefore,
    Bipolar,
    Seed { levels: bool, up: bool },
    /// A press on Limit's keys (the green key follows), its name strip (the nearer range end follows) or its scroll
    /// bar's handle.
    Keys,
    Ruler,
    Scroll,
    Reset,
    Accept,
}

/// A pick from one of Claw machine's menus: an index into `claw::PERIODS` or `claw::TRASH`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CCmd {
    Period(usize),
    Trash(usize),
}

/// A pick from one of Limit's menus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LCmd {
    Key(u8),
    Scale(usize),
    Snap(limit::Snap),
}

/// The window's place for each dialog and the dialogs' state: the settings shown, the ones last accepted (an opening
/// starts from them) and the open session.
#[derive(Default)]
pub struct Ui {
    pub tool: Tool,
    /// Top-left corner (logical px) of each dialog, by `Tool::index`.
    pub pos: [(f32, f32); Tool::COUNT],
    pub placed: [bool; Tool::COUNT],
    pub flam: flam::Settings,
    pub flip: flip::Settings,
    pub levels: levels::Settings,
    pub limit: limit::Settings,
    pub claw: claw::Settings,
    pub lfo: lfo::Settings,
    pub quant: quantize::Settings,
    pub quant_tpl: quantize::Template,
    pub chop: chop::Settings,
    pub chop_pat: chop::ChopPattern,
    pub rand: randomize::Settings,
    /// Limit's collapsed sections (`Act::Collapse`); a view setting, kept as it is.
    pub collapsed: [bool; 2],
    /// Limit's keyboard view: px from C0's left edge to the view's.
    pub kb_scroll: f32,
    kept_flam: flam::Settings,
    kept_flip: flip::Settings,
    kept_levels: levels::Settings,
    kept_limit: limit::Settings,
    kept_claw: claw::Settings,
    kept_lfo: lfo::Settings,
    kept_quant: quantize::Settings,
    kept_quant_tpl: quantize::Template,
    kept_chop: chop::Settings,
    kept_chop_pat: chop::ChopPattern,
    kept_rand: randomize::Settings,
    /// The Control lane Scale levels was last accepted on (its Center follows a new lane).
    kept_lane: Option<Lane>,
    session: Option<Session>,
}

/// What an open dialog previews on: the pattern's notes, selection and control points as they were, and what the last
/// preview left (`shown`: once one has run).
struct Session {
    pattern: u16,
    channel: u16,
    base: Vec<Note>,
    base_sel: Vec<usize>,
    base_ctl: Vec<ControlPoint>,
    last: Vec<Note>,
    last_ctl: Vec<ControlPoint>,
    shown: bool,
}

fn pct(v: f32) -> i64 {
    (f64::from(v) * 100.0).round() as i64
}

impl Ui {
    #[allow(clippy::too_many_arguments)]
    fn pos_of(k: Knob, f: &flam::Settings, l: &levels::Settings, c: &claw::Settings, o: &lfo::Settings, q: &quantize::Settings, ch: &chop::Settings, r: &randomize::Settings) -> f32 {
        match k {
            Knob::FlamTime => f.time,
            Knob::FlamVel => f.vel,
            Knob::Center => l.center,
            Knob::Mult => (l.mult + 2.0) / 4.0,
            Knob::Offset => (l.offset + 1.0) / 2.0,
            Knob::Tension => (l.tension + 1.0) / 2.0,
            Knob::ClawTime => c.time,
            Knob::LfoValue => o.start.value,
            Knob::LfoRange => lfo::range_pos(o.start.range),
            Knob::LfoSpeed => lfo::speed_pos(o.start.speed),
            Knob::LfoEndValue => o.end.value,
            Knob::LfoEndRange => lfo::range_pos(o.end.range),
            Knob::LfoEndSpeed => lfo::speed_pos(o.end.speed),
            Knob::LfoPhase => lfo::phase_pos(o.phase),
            Knob::QStart => q.start,
            Knob::QSens => q.sens_pos(),
            Knob::QDur => q.dur,
            Knob::QLevel(i) => q.levels[usize::from(i.min(5))],
            Knob::ChopTimeMul => ch.time_mul_pos(),
            Knob::ChopLevel(i) => ch.levels[usize::from(i.min(5))],
            Knob::RandOctave => f32::from(r.octave.min(9)) / 9.0,
            Knob::RandRange => f32::from(r.range.clamp(1, 6) - 1) / 5.0,
            Knob::RandStack => f32::from(r.stack.clamp(1, 6) - 1) / 5.0,
            Knob::RandLength => r.length_pos(),
            Knob::RandVariation => r.variation_pos(),
            Knob::RandPopulation => r.population,
            Knob::RandLevel(i) => (r.amounts[usize::from(i.min(5))] + 1.0) / 2.0,
        }
    }

    /// The 0..1 position of knob `k`.
    pub fn get(&self, k: Knob) -> f32 {
        Self::pos_of(k, &self.flam, &self.levels, &self.claw, &self.lfo, &self.quant, &self.chop, &self.rand)
    }

    pub fn set(&mut self, k: Knob, v: f32) {
        let v = v.clamp(0.0, 1.0);
        match k {
            Knob::FlamTime => self.flam.time = v,
            Knob::FlamVel => self.flam.vel = v,
            Knob::Center => self.levels.center = v,
            Knob::Mult => self.levels.mult = v * 4.0 - 2.0,
            Knob::Offset => self.levels.offset = v * 2.0 - 1.0,
            Knob::Tension => self.levels.tension = v * 2.0 - 1.0,
            Knob::ClawTime => self.claw.time = v,
            Knob::LfoValue => self.lfo.start.value = v,
            Knob::LfoRange => self.lfo.start.range = lfo::range_of(v),
            Knob::LfoSpeed => self.lfo.start.speed = lfo::speed_of(v),
            Knob::LfoEndValue => self.lfo.end.value = v,
            Knob::LfoEndRange => self.lfo.end.range = lfo::range_of(v),
            Knob::LfoEndSpeed => self.lfo.end.speed = lfo::speed_of(v),
            Knob::LfoPhase => self.lfo.phase = lfo::phase_of(v),
            Knob::QStart => self.quant.start = v,
            Knob::QSens => self.quant.set_sens_pos(v),
            Knob::QDur => self.quant.dur = v,
            Knob::QLevel(i) => self.quant.levels[usize::from(i.min(5))] = v,
            Knob::ChopTimeMul => self.chop.set_time_mul_pos(v),
            Knob::ChopLevel(i) => self.chop.levels[usize::from(i.min(5))] = v,
            Knob::RandOctave => self.rand.octave = (v * 9.0).round() as u8,
            Knob::RandRange => self.rand.range = 1 + (v * 5.0).round() as u8,
            Knob::RandStack => self.rand.stack = 1 + (v * 5.0).round() as u8,
            Knob::RandLength => self.rand.set_length_pos(v),
            Knob::RandVariation => self.rand.set_variation_pos(v),
            Knob::RandPopulation => self.rand.population = v,
            Knob::RandLevel(i) => self.rand.amounts[usize::from(i.min(5))] = v * 2.0 - 1.0,
        }
    }

    /// The hint-bar text of a knob (piano-flam.md §1, piano-level-scaling.md §1, piano-claw.md §5, piano-lfo.md §1,
    /// piano-quantize.md §1, piano-chop.md §1, piano-randomize.md §6).
    pub fn text(&self, k: Knob, ppq: u32) -> String {
        let (l, o, r) = (&self.levels, &self.lfo, &self.rand);
        match k {
            Knob::FlamTime => self.flam.time_text(ppq),
            Knob::FlamVel => self.flam.vel_text(),
            Knob::Center => format!("Center: {}%", pct(l.center)),
            Knob::Mult => format!("Multiplier: {}%", pct(l.mult)),
            Knob::Offset => format!("Offset: {}%", pct(l.offset)),
            Knob::Tension => format!("Tension: {}%", pct(l.tension)),
            Knob::ClawTime => self.claw.time_text(),
            Knob::LfoValue => o.start.value_text(),
            Knob::LfoRange => o.start.range_text(),
            Knob::LfoSpeed => o.start.speed_text(ppq),
            Knob::LfoEndValue => o.end.value_text(),
            Knob::LfoEndRange => o.end.range_text(),
            Knob::LfoEndSpeed => o.end.speed_text(ppq),
            Knob::LfoPhase => o.phase_text(),
            Knob::QStart => quantize::Settings::pct_text(self.quant.start),
            Knob::QSens => self.quant.sens_text(ppq),
            Knob::QDur => quantize::Settings::pct_text(self.quant.dur),
            Knob::QLevel(i) => quantize::Settings::pct_text(self.quant.levels[usize::from(i.min(5))]),
            Knob::ChopTimeMul => self.chop.time_mul_text(),
            Knob::ChopLevel(i) => quantize::Settings::pct_text(self.chop.levels[usize::from(i.min(5))]),
            Knob::RandOctave => format!("Octave: {}", r.octave),
            Knob::RandRange => format!("Range: {} octave{}", r.range, if r.range == 1 { "" } else { "s" }),
            Knob::RandStack => format!("Stack: {}", r.stack),
            Knob::RandLength => r.length_text(),
            Knob::RandVariation => r.variation_text(),
            Knob::RandPopulation => r.population_text(),
            Knob::RandLevel(i) => format!("{}: {}", quantize::LEVELS[usize::from(i.min(5))].1, r.amount_text(usize::from(i))),
        }
    }

    /// Whether `a` is a switch that is on.
    pub fn on(&self, a: Act) -> bool {
        let chop = self.tool == Tool::Chop;
        let r = &self.rand;
        match a {
            Act::RandPattern => r.pattern,
            Act::RandLevels => r.levels,
            Act::Porta => r.porta,
            Act::Glue => r.glue,
            Act::ResetBefore => r.reset_before,
            Act::Bipolar => r.bipolar,
            Act::Absolute if chop => self.chop.absolute,
            Act::Group if chop => self.chop.group,
            Act::Absolute => self.flam.absolute,
            Act::Before => self.flam.before,
            Act::Group => self.flam.group,
            Act::Horizontal => self.flip.horizontal,
            Act::Preserve => self.flip.preserve,
            Act::Vertical => self.flip.vertical,
            Act::Wrap => self.limit.wrap,
            Act::Stretch => self.claw.stretch,
            Act::Short => self.claw.short,
            Act::LfoEnd => self.lfo.end_on,
            Act::LfoLog => self.lfo.log,
            _ => false,
        }
    }
}

/// Options ▸ (piano-flam.md §5).
pub fn options_menu() -> Vec<PItem> {
    flam::Preset::ALL.iter().map(|&p| PItem::new(p.name(), Cmd::FlamPreset(p))).collect()
}

/// Start `tool`'s session on the piano roll's channel and preview the settings last accepted; another dialog's
/// preview is put back first. Scale levels' Center takes the lane's Reset value when the Control lane changed since
/// its last Accept (piano-level-scaling.md §4); Limit's range is the targets' own until one is kept (piano-limit.md
/// §5).
pub fn open(s: &mut State, tool: Tool) {
    cancel(s);
    let span = limit::span(&target_notes(s));
    let d = &mut s.piano_dlg;
    d.tool = tool;
    match tool {
        Tool::Flam => d.flam = d.kept_flam,
        Tool::Flip => d.flip = d.kept_flip,
        Tool::Levels => {
            d.levels = d.kept_levels;
            if d.kept_lane != Some(s.piano.lane) {
                d.levels.center = levels::reset_center(s.piano.lane);
            }
        }
        Tool::Limit => {
            d.limit = d.kept_limit;
            if d.limit.range.is_none() {
                d.limit.range = span;
            }
            show_range(s);
        }
        Tool::Claw => d.claw = d.kept_claw,
        Tool::Lfo => d.lfo = d.kept_lfo,
        Tool::Quantize => (d.quant, d.quant_tpl) = (d.kept_quant, d.kept_quant_tpl.clone()),
        Tool::Chop => (d.chop, d.chop_pat) = (d.kept_chop, d.kept_chop_pat.clone()),
        // The seeds go on from where they are (piano-randomize.md §2).
        Tool::Randomize => d.rand = randomize::Settings { pattern_seed: d.rand.pattern_seed, levels_seed: d.rand.levels_seed, ..d.kept_rand },
    }
    preview(s);
}

/// The notes a tool would change now (`piano::targets`).
fn target_notes(s: &State) -> Vec<Note> {
    let all = piano::notes(s);
    piano::targets(s).iter().filter_map(|&i| all.get(i).copied()).collect()
}

/// The targets as the open session found them: its selection, or every note of its channel.
fn base_targets(s: &State) -> Vec<Note> {
    match &s.piano_dlg.session {
        Some(se) if !se.base_sel.is_empty() => se.base_sel.iter().filter_map(|&i| se.base.get(i).copied()).collect(),
        Some(se) => se.base.iter().filter(|n| n.channel == se.channel).copied().collect(),
        None => target_notes(s),
    }
}

/// Scroll Limit's keyboard so its range shows, centred, when it would not (a Lemon Studio choice, piano-limit.md §5).
fn show_range(s: &mut State) {
    let d = &mut s.piano_dlg;
    let Some((lo, hi)) = d.limit.range else { return };
    let (a, b) = (white_x(i32::from(lo)), white_x(i32::from(hi)) + KEY_W);
    if a < d.kb_scroll || b > d.kb_scroll + KB_W {
        d.kb_scroll = ((a + b - KB_W) / 2.0).round();
    }
    d.kb_scroll = d.kb_scroll.clamp(0.0, kb_max());
}

/// Limit's keyboard (piano-limit.md §1): white keys 13 px wide, a view 416 px wide over C0…B10.
pub const KEY_W: f32 = 13.0;
pub const KB_W: f32 = 416.0;
/// The view FL opens on: C5's left edge 202 px into it.
pub const KB_HOME: f32 = 35.0 * KEY_W - 202.0;

/// Where white key `k`'s left edge is (for a black key: the white key under it), px from C0's.
pub fn white_x(k: i32) -> f32 {
    const WHITE: [i32; 12] = [0, 0, 1, 1, 2, 3, 3, 4, 4, 5, 5, 6];
    (7 * k.div_euclid(12) + WHITE[k.rem_euclid(12) as usize]) as f32 * KEY_W
}

/// The largest keyboard scroll.
pub fn kb_max() -> f32 {
    white_x(crate::views::piano::TOP_KEY) + KEY_W - KB_W
}

/// Apply the open dialog to the targets; `step` names the undo step (none for a preview).
fn run(s: &mut State, step: Option<&'static str>) -> bool {
    let d = &s.piano_dlg;
    match d.tool {
        Tool::Flam => {
            let set = d.flam;
            let off = set.offset(u32::from(s.project.ppq.max(4)), s.tempo);
            let mut used: Vec<u16> = piano::notes(s).iter().map(|n| n.unk14).filter(|&g| g != 0).collect();
            used.sort_unstable();
            used.dedup();
            crate::piano_tools::apply_as(s, |_, ns| flam::flam(ns, &set, off, &used), step)
        }
        Tool::Flip => {
            let set = d.flip;
            crate::piano_tools::apply_as(s, |_, ns| flip::flip(ns, &set), step)
        }
        Tool::Levels => {
            let (set, lane) = (d.levels, s.piano.lane);
            crate::piano_tools::apply_as(s, |_, ns| levels::scale(ns, lane, &set), step)
        }
        Tool::Limit => {
            let set = d.limit;
            crate::piano_tools::apply_as(
                s,
                |_, ns| {
                    let range = set.range.or_else(|| limit::span(&ns)).unwrap_or((0, 131));
                    limit::limit(ns, &set, range)
                },
                step,
            )
        }
        // The periods count from tick 0 with a selection too (piano-claw.md §2).
        Tool::Claw => {
            let (set, ppq) = (d.claw, u32::from(s.project.ppq.max(4)));
            crate::piano_tools::apply_as(s, |_, ns| claw::claw(ns, &set, ppq), step)
        }
        Tool::Lfo => {
            let (set, lane, ppq) = (d.lfo, s.piano.lane, u32::from(s.project.ppq.max(4)));
            let len = piano::pattern_length(s, s.pattern);
            if lane.is_channel() {
                return lfo_points(s, &set, lane, ppq, len, step);
            }
            // With a selection the time starts at its first note and End is reached a tick before its end
            // (piano-lfo.md §3).
            let sel = piano::channel(s).is_some_and(|ch| !s.piano.selection(s.pattern, Some(ch)).is_empty());
            crate::piano_tools::apply_as(
                s,
                |_, ns| {
                    let (origin, l) = match (sel, ns.iter().map(|n| n.position).min()) {
                        (true, Some(a)) => {
                            let b = ns.iter().map(|n| n.position + n.length.max(1)).max().unwrap_or(a + 1);
                            (a, (b - a).saturating_sub(1))
                        }
                        _ => (0, len),
                    };
                    lfo::lfo(ns, lane, &set, ppq, origin, l)
                },
                step,
            )
        }
        Tool::Quantize => {
            let (set, tpl, ppq) = (d.quant, d.quant_tpl.clone(), u32::from(s.project.ppq.max(4)));
            crate::piano_tools::apply_as(s, |_, ns| quantize::quantize(ns, &tpl.0, &set, ppq), step)
        }
        // The new groups skip the numbers in use (piano-chop.md §6).
        Tool::Chop => {
            let (set, pat, ppq) = (d.chop, d.chop_pat.clone(), u32::from(s.project.ppq.max(4)));
            let mut used: Vec<u16> = piano::notes(s).iter().map(|n| n.unk14).filter(|&g| g != 0).collect();
            used.sort_unstable();
            used.dedup();
            crate::piano_tools::apply_as(s, |_, ns| chop::chop(ns, &pat.0, &set, ppq, &used), step)
        }
        Tool::Randomize => randomize_notes(s, step),
    }
}

/// Randomize (piano-randomize.md §3, §4): with Pattern on every note of the channel gives way to new ones over the
/// pattern's whole bars, on the piano roll's snap grid (one tick without a unit), and they end up selected, as FL leaves
/// them; with Pattern off the Levels section works on the targets.
fn randomize_notes(s: &mut State, step: Option<&'static str>) -> bool {
    let set = s.piano_dlg.rand;
    if !set.pattern {
        return crate::piano_tools::apply_as(s, |_, ns| randomize::levels(ns, &set), step);
    }
    let Some(ch) = piano::channel(s) else { return false };
    let (grid, end) = (crate::piano_snap::unit(s).unwrap_or(1), piano::pattern_end(s));
    let mine: Vec<Note> = piano::notes(s).iter().filter(|n| n.channel == ch).copied().collect();
    let out = randomize::randomize(mine.clone(), &set, ch, grid, end);
    if out == mine {
        return false;
    }
    if let Some(name) = step {
        piano::snapshot_as(s, name);
    }
    let p = piano::pattern_mut(s);
    let mut notes: Vec<Note> = p.notes.iter().filter(|n| n.channel != ch).copied().collect();
    let start = notes.len();
    notes.extend(out);
    let end = notes.len();
    p.notes = notes;
    s.piano.select(s.pattern, ch, (start..end).collect());
    piano::finish_edit(s);
    s.piano.dirty = true;
    true
}

/// LFO on a channel lane: the lane's control points in the pattern become the wave's (piano-lfo.md §5); the other
/// lanes' stay.
fn lfo_points(s: &mut State, set: &lfo::Settings, lane: Lane, ppq: u32, len: u32, step: Option<&'static str>) -> bool {
    let (Some(ch), Some(param)) = (piano::channel(s), lane.param()) else { return false };
    let kind = ControlPoint::new(0, ch, param, 0).kind;
    let mut ctl: Vec<ControlPoint> = piano::controls(s).iter().filter(|c| c.kind != kind).copied().collect();
    ctl.extend(lfo::points(set, lane, ppq, len).into_iter().map(|(t, v)| ControlPoint::new(t, ch, param, v)));
    ctl.sort_by_key(|c| c.position);
    if ctl == piano::controls(s) {
        return false;
    }
    if let Some(name) = step {
        piano::snapshot_as(s, name);
    }
    piano::pattern_mut(s).controls = ctl;
    s.piano.dirty = true;
    true
}

/// Put the notes back to the session's start and apply the dialog to them, without an undo step. Notes or control
/// points changed by something else since the last preview (another pattern or channel, an edit, an undo) start the
/// session again from them.
pub fn preview(s: &mut State) {
    let Some(ch) = piano::channel(s) else { return };
    let (now, now_ctl) = (piano::notes(s).to_vec(), piano::controls(s).to_vec());
    let fresh = match &s.piano_dlg.session {
        Some(se) => se.pattern != s.pattern || se.channel != ch || se.last != now || se.last_ctl != now_ctl,
        None => true,
    };
    if fresh {
        let base_sel = s.piano.selection(s.pattern, Some(ch));
        let se = Session { pattern: s.pattern, channel: ch, base: now, base_sel, base_ctl: now_ctl, last: Vec::new(), last_ctl: Vec::new(), shown: false };
        s.piano_dlg.session = Some(se);
    }
    restore(s);
    run(s, None);
    let (last, last_ctl) = (piano::notes(s).to_vec(), piano::controls(s).to_vec());
    if let Some(se) = &mut s.piano_dlg.session {
        (se.last, se.last_ctl, se.shown) = (last, last_ctl, true);
    }
}

/// The session's notes, selection and control points back, when they are still what its last preview left.
fn restore(s: &mut State) -> bool {
    let Some(se) = &s.piano_dlg.session else { return false };
    if se.pattern != s.pattern || se.shown && (piano::notes(s) != se.last.as_slice() || piano::controls(s) != se.last_ctl.as_slice()) {
        return false;
    }
    let (base, base_ctl, sel, ch) = (se.base.clone(), se.base_ctl.clone(), se.base_sel.clone(), se.channel);
    let p = piano::pattern_mut(s);
    (p.notes, p.controls) = (base, base_ctl);
    if sel.is_empty() {
        s.piano.clear_selection();
    } else {
        s.piano.select(s.pattern, ch, sel);
    }
    s.piano.dirty = true;
    true
}

/// Accept: the result becomes one undo step, the settings are kept for the next opening. Returns the status text.
pub fn accept(s: &mut State) -> String {
    let tool = s.piano_dlg.tool;
    if !restore(s) {
        s.piano_dlg.session = None;
        return "Piano roll: nothing to change".into();
    }
    let d = &mut s.piano_dlg;
    d.session = None;
    match tool {
        Tool::Flam => d.kept_flam = d.flam,
        Tool::Flip => d.kept_flip = d.flip,
        Tool::Levels => (d.kept_levels, d.kept_lane) = (d.levels, Some(s.piano.lane)),
        Tool::Limit => d.kept_limit = d.limit,
        Tool::Claw => d.kept_claw = d.claw,
        Tool::Lfo => d.kept_lfo = d.lfo,
        Tool::Quantize => (d.kept_quant, d.kept_quant_tpl) = (d.quant, d.quant_tpl.clone()),
        Tool::Chop => (d.kept_chop, d.kept_chop_pat) = (d.chop, d.chop_pat.clone()),
        Tool::Randomize => d.kept_rand = d.rand,
    }
    if run(s, Some(tool.step())) {
        format!("Piano roll: {}", tool.done())
    } else {
        format!("Piano roll: {}: nothing to change", tool.done())
    }
}

/// The × button: the notes as they were before the dialog opened.
pub fn cancel(s: &mut State) {
    restore(s);
    s.piano_dlg.session = None;
}

/// A dialog control: a switch flips (Preserve start times only while Flip horizontally is on), Reset puts the open
/// dialog's controls back to their defaults; the notes follow.
pub fn act(s: &mut State, a: Act) {
    let span = limit::span(&base_targets(s));
    let (d, lane) = (&mut s.piano_dlg, s.piano.lane);
    match a {
        Act::Reset => match d.tool {
            Tool::Flam => d.flam = flam::Settings::default(),
            Tool::Flip => d.flip = flip::Settings::default(),
            Tool::Levels => d.levels = levels::Settings::reset(lane),
            Tool::Limit => {
                d.limit = limit::Settings { range: span, ..limit::Settings::default() };
                show_range(s);
            }
            Tool::Claw => d.claw = claw::Settings::default(),
            Tool::Lfo => d.lfo = lfo::Settings::default(),
            Tool::Quantize => (d.quant, d.quant_tpl) = (quantize::Settings::default(), quantize::Template::default()),
            Tool::Chop => (d.chop, d.chop_pat) = (chop::Settings::default(), chop::ChopPattern::default()),
            Tool::Randomize => d.rand = d.rand.reset(),
        },
        Act::RandPattern => d.rand.pattern = !d.rand.pattern,
        Act::RandLevels => d.rand.levels = !d.rand.levels,
        Act::Porta => d.rand.porta = !d.rand.porta,
        Act::Glue => d.rand.glue = !d.rand.glue,
        Act::ResetBefore => d.rand.reset_before = !d.rand.reset_before,
        Act::Bipolar => d.rand.bipolar = !d.rand.bipolar,
        Act::Seed { levels, up } => {
            let seed = if levels { &mut d.rand.levels_seed } else { &mut d.rand.pattern_seed };
            *seed = if up { seed.wrapping_add(1) } else { seed.wrapping_sub(1) };
        }
        Act::Absolute if d.tool == Tool::Chop => d.chop.absolute = !d.chop.absolute,
        Act::Group if d.tool == Tool::Chop => d.chop.group = !d.chop.group,
        Act::Absolute => d.flam.absolute = !d.flam.absolute,
        Act::Before => d.flam.before = !d.flam.before,
        Act::Group => d.flam.group = !d.flam.group,
        Act::Horizontal => d.flip.horizontal = !d.flip.horizontal,
        Act::Preserve if d.flip.horizontal => d.flip.preserve = !d.flip.preserve,
        Act::Vertical => d.flip.vertical = !d.flip.vertical,
        Act::Wrap => d.limit.wrap = !d.limit.wrap,
        Act::Stretch => d.claw.stretch = !d.claw.stretch,
        Act::Short => d.claw.short = !d.claw.short,
        Act::LfoEnd => d.lfo.toggle_end(),
        // log only bends End's speed: it does nothing while End is off.
        Act::LfoLog if d.lfo.end_on => d.lfo.log = !d.lfo.log,
        Act::Collapse(i) => {
            d.collapsed[usize::from(i & 1)] ^= true;
            return;
        }
        _ => return,
    }
    preview(s);
}

/// The open dialog's key and scale: Randomize's, else Limit's.
pub fn key_scale(d: &Ui) -> (u8, usize) {
    if d.tool == Tool::Randomize { (d.rand.key, d.rand.scale) } else { (d.limit.key, d.limit.scale) }
}

/// Limit's Key ▸ menu (piano-limit.md §1), also Randomize's (piano-randomize.md §1), `key` ticked.
pub fn key_menu(key: u8) -> Vec<PItem> {
    limit::KEYS.iter().enumerate().map(|(i, &n)| PItem::new(n, Cmd::Limit(LCmd::Key(i as u8))).mark(Mark::Radio(usize::from(key) == i))).collect()
}

/// Limit's scale ▸ menu, also Randomize's: "(none)", then the chords file in two columns with its headings and
/// separators (§1), `scale` ticked.
pub fn scale_menu(scale: usize) -> Vec<PItem> {
    let pick = |k: usize, label: &'static str| PItem::new(label, Cmd::Limit(LCmd::Scale(k))).mark(Mark::Radio(scale == k));
    let mut items = vec![pick(0, "(none)"), PItem::head("Chords")];
    for k in 1..=chords::ENTRIES.len() {
        if chords::SEPARATED.contains(&k) {
            items.push(PItem::sep());
        }
        if k == chords::FIRST_SCALE {
            items.push(PItem::head("Scales"));
        }
        items.push(pick(k, chords::label(k)));
        if k == 50 {
            items.last_mut().expect("just pushed").col = true;
        }
    }
    items
}

/// Limit's Snap ▸ menu.
pub fn snap_menu(set: &limit::Settings) -> Vec<PItem> {
    limit::Snap::ALL.iter().map(|&m| PItem::new(m.name(), Cmd::Limit(LCmd::Snap(m))).mark(Mark::Radio(set.snap == m))).collect()
}

/// A pick from Limit's menus, or Randomize's Key and scale: it previews at once.
pub fn limit_cmd(s: &mut State, c: LCmd) {
    let d = &mut s.piano_dlg;
    if d.tool == Tool::Randomize {
        match c {
            LCmd::Key(k) => d.rand.key = k % 12,
            LCmd::Scale(k) => d.rand.scale = k.min(chords::ENTRIES.len()),
            LCmd::Snap(_) => return,
        }
        preview(s);
        return;
    }
    let set = &mut d.limit;
    match c {
        LCmd::Key(k) => set.key = k % 12,
        LCmd::Scale(k) => set.scale = k.min(chords::ENTRIES.len()),
        LCmd::Snap(m) => set.snap = m,
    }
    preview(s);
}

/// Limit's keys: key `k` becomes the green key, the transposition `60 − k` (§1 "Mouse on the keyboard").
pub fn set_green(s: &mut State, k: i32) {
    let t = 60 - k.clamp(0, crate::views::piano::TOP_KEY);
    if s.piano_dlg.limit.transpose != t {
        s.piano_dlg.limit.transpose = t;
        preview(s);
    }
}

/// Which range end a press on the name strip at key `k` takes: the nearer one (true = the highest key); the lower
/// one when both are as near.
pub fn nearer_end(s: &State, k: i32) -> bool {
    let (lo, hi) = s.piano_dlg.limit.shown_range();
    (k - i32::from(lo)).abs() > (k - i32::from(hi)).abs()
}

/// Move a range end to key `k`; it stops at the other end (a Lemon Studio choice, piano-limit.md §5).
pub fn set_end(s: &mut State, high: bool, k: i32) {
    let (lo, hi) = s.piano_dlg.limit.shown_range();
    let k = k.clamp(0, crate::views::piano::TOP_KEY) as u16;
    let range = if high { (lo, k.max(lo)) } else { (k.min(hi), hi) };
    if s.piano_dlg.limit.range != Some(range) {
        s.piano_dlg.limit.range = Some(range);
        preview(s);
    }
}

/// Claw machine's Period ▸ menu (piano-claw.md §1).
pub fn period_menu(set: &claw::Settings) -> Vec<PItem> {
    claw::PERIODS.iter().enumerate().map(|(i, &(_, name))| PItem::new(name, Cmd::Claw(CCmd::Period(i))).mark(Mark::Radio(set.period == i))).collect()
}

/// Claw machine's Trash every ▸ menu, its groups apart.
pub fn trash_menu(set: &claw::Settings) -> Vec<PItem> {
    let mut items = Vec::new();
    for (i, &(_, name)) in claw::TRASH.iter().enumerate() {
        if claw::TRASH_SEPARATED.contains(&i) {
            items.push(PItem::sep());
        }
        items.push(PItem::new(name, Cmd::Claw(CCmd::Trash(i))).mark(Mark::Radio(set.trash == i)));
    }
    items
}

/// A pick from Claw machine's menus: it previews at once.
pub fn claw_cmd(s: &mut State, c: CCmd) {
    let set = &mut s.piano_dlg.claw;
    match c {
        CCmd::Period(i) => set.period = i.min(claw::PERIODS.len() - 1),
        CCmd::Trash(i) => set.trash = i.min(claw::TRASH.len() - 1),
    }
    preview(s);
}

/// LFO's Shape ▸ menu (piano-lfo.md §1).
pub fn shape_menu(set: &lfo::Settings) -> Vec<PItem> {
    lfo::Shape::ALL.iter().map(|&w| PItem::new(w.name(), Cmd::LfoShape(w)).mark(Mark::Radio(set.shape == w))).collect()
}

/// A shape picked: it previews at once.
pub fn lfo_shape(s: &mut State, w: lfo::Shape) {
    s.piano_dlg.lfo.shape = w;
    preview(s);
}

/// A pick from Quantize's menus or its folder button's file dialog: it previews at once. Returns a message for a
/// template file that could not be read.
pub fn quant_cmd(s: &mut State, c: quantize::QCmd) -> Option<String> {
    let mut msg = None;
    match c {
        quantize::QCmd::Template(None) => s.piano_dlg.quant_tpl = quantize::Template::default(),
        quantize::QCmd::Template(Some(p)) => match crate::arpeggiate::load(&p) {
            Ok(pat) => s.piano_dlg.quant_tpl = quantize::Template(pat),
            Err(e) => msg = Some(format!("Quantizer: {e}")),
        },
        quantize::QCmd::Snap(i) => s.piano_dlg.quant_tpl = quantize::Template::snap(usize::from(i)),
        quantize::QCmd::Mode(m) => s.piano_dlg.quant.mode = m,
    }
    preview(s);
    msg
}

/// A pattern picked from Chop's ▸ menu or its folder button's file dialog (None: the built-in Default): it previews at
/// once. Returns a message for a file that could not be read.
pub fn chop_cmd(s: &mut State, p: Option<std::path::PathBuf>) -> Option<String> {
    let mut msg = None;
    match p {
        None => s.piano_dlg.chop_pat = chop::ChopPattern::default(),
        Some(p) => match crate::arpeggiate::load(&p) {
            Ok(pat) => s.piano_dlg.chop_pat = chop::ChopPattern(pat),
            Err(e) => msg = Some(format!("Chopper: {e}")),
        },
    }
    preview(s);
    msg
}

/// An Options ▸ preset picked: it previews at once (piano-flam.md §5).
pub fn preset(s: &mut State, p: flam::Preset) {
    s.piano_dlg.flam.preset(p);
    preview(s);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn knob_positions_round_trip() {
        let mut d = Ui::default();
        assert_eq!([Knob::FlamTime, Knob::FlamVel, Knob::Center, Knob::Mult, Knob::Offset, Knob::Tension, Knob::ClawTime].map(|k| d.get(k)), [1.0 / 3.0, 85.0 / 128.0, 0.0, 0.75, 0.5, 0.5, 0.5]);
        d.set(Knob::Mult, 0.75 - 5.0 / 64.0);
        assert_eq!(d.levels.mult, 0.6875);
        assert_eq!(d.text(Knob::Mult, 96), "Multiplier: 69%");
        d.set(Knob::Offset, 0.0);
        assert_eq!(d.text(Knob::Offset, 96), "Offset: -100%");
        d.set(Knob::Tension, 0.5 + 5.0 / 64.0);
        assert_eq!(d.levels.tension, 0.15625);
        assert_eq!(d.text(Knob::Center, 96), "Center: 0%");
        assert_eq!(Knob::Center.default_pos(), 0.0);
        assert!(d.on(Act::Horizontal) && d.on(Act::Preserve) && !d.on(Act::Vertical) && d.on(Act::Absolute) && d.on(Act::Group));
        assert!(d.on(Act::Stretch) && d.on(Act::Short));
        d.set(Knob::ClawTime, 0.5 - 16.0 / 64.0);
        assert_eq!(d.text(Knob::ClawTime, 96), "Time distribution: -50%");
        let names = |items: Vec<PItem>| items.iter().map(|i| i.label.to_string()).collect::<Vec<_>>().join(",");
        assert_eq!(names(period_menu(&d.claw)), "Beat,2 beats,4 beats,8 beats,16 beats");
        assert_eq!(names(trash_menu(&d.claw)), "1/4,2/4,3/4,4/4,,1/3,2/3,3/3,,1/2,2/2,,(none)");
    }
}
