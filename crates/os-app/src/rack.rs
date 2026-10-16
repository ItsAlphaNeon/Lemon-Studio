// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

//! Channel rack behaviour: the selection set, channel commands (fill, rotate, copy / paste, clone, delete, move,
//! sort, filter groups, colours, mute / solo / lock, zip, free mixer tracks), the rack's popup menus, its keyboard,
//! the graph editor values and the per-channel loop lengths.
// spec: ui/channel-rack.md

use std::collections::{BTreeMap, BTreeSet};

use os_model::{target, ChannelKind, ClipSource, Note, Project, Rgb};

use crate::app::{App, State, Win};
use crate::knobs::KnobId;
use crate::playlist::{Cmd, Mark, PItem, Popup};

/// Name button width at 100 % (§17): wide enough for a new project's "Lemon Sampler" at every UI scale with every text
/// engine; Linux's Barlow is the widest (Lemon Studio).
pub const NAME_W: f32 = 94.0;
/// FL's name button width at 100 % (§17): the measured columns right of the name button assume it.
pub const FL_NAME_W: f32 = 82.0;
/// Width of the mixer-box column (§17: Alt+M moves everything right of it ≈ 41 px).
pub const MIX_W: f32 = 41.0;
/// Extra width of loop mode (§15).
pub const LOOP_W: f32 = 40.0;
/// Row pitch of a zipped channel (§17).
pub const ZIP_H: f32 = 12.0;
/// Pixels of vertical drag per step on the pattern length box and the loop boxes (§7, §15).
pub const LEN_DRAG: f32 = 4.0;
/// Pixels of vertical drag per insert on the mixer box (§12).
pub const MIXER_DRAG: f32 = 10.0;
/// Longest pattern the length box sets (32 bars, §7).
pub const MAX_STEPS: u32 = 512;
/// Keys the graph editor's Note tab spans (two octaves around C5, a Lemon Studio choice).
pub const KEY_LO: u16 = 48;
pub const KEY_HI: u16 = 72;

/// Display filter (§21).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Filter {
    #[default]
    All,
    Group(usize),
    Unsorted,
}

/// Options > Auto resize (§17).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AutoResize {
    Off,
    #[default]
    Min,
    Max,
}

/// Graph editor tabs (§14).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    Note,
    #[default]
    Vel,
    Rel,
    Fine,
    Pan,
    X,
    Y,
    Shift,
    Rep,
}

impl Tab {
    pub const ALL: [Tab; 9] = [Tab::Note, Tab::Vel, Tab::Rel, Tab::Fine, Tab::Pan, Tab::X, Tab::Y, Tab::Shift, Tab::Rep];

    pub fn label(self) -> &'static str {
        match self {
            Tab::Note => "Note",
            Tab::Vel => "Vel",
            Tab::Rel => "Rel",
            Tab::Fine => "Fine",
            Tab::Pan => "Pan",
            Tab::X => "X",
            Tab::Y => "Y",
            Tab::Shift => "Shift",
            Tab::Rep => "Rep",
        }
    }

    /// Bar / label colour (§14, approximate).
    pub fn color(self) -> u32 {
        match self {
            Tab::Note => 0x5fd07a,
            Tab::Vel => 0x40c0e0,
            Tab::Rel => 0x30c070,
            Tab::Fine => 0xe05050,
            Tab::Pan => 0xb070e0,
            Tab::X => 0x70d060,
            Tab::Y => 0xe0d050,
            Tab::Shift => 0xf06040,
            Tab::Rep => 0xf0a040,
        }
    }

    /// Drawn as a line from the centre instead of a bar from the bottom.
    pub fn centred(self) -> bool {
        matches!(self, Tab::Fine | Tab::Pan)
    }

    /// The note's value as 0..1.
    pub fn get(self, n: &Note, per_step: u32) -> f32 {
        let f = |v: u8, max: f32| f32::from(v) / max;
        match self {
            Tab::Note => (f32::from(n.key.clamp(KEY_LO, KEY_HI) - KEY_LO) / f32::from(KEY_HI - KEY_LO)).clamp(0.0, 1.0),
            Tab::Vel => f(n.velocity, 128.0),
            Tab::Rel => f(n.release, 128.0),
            Tab::Fine => f(n.fine_pitch, 240.0),
            Tab::Pan => f(n.pan, 128.0),
            Tab::X => f(n.mod_x, 255.0),
            Tab::Y => f(n.mod_y, 255.0),
            Tab::Shift => (n.position % per_step.max(1)) as f32 / per_step.max(1) as f32,
            Tab::Rep => f(n.repeat, f32::from(Note::MAX_REPEAT)),
        }
        .clamp(0.0, 1.0)
    }

    /// Set the note's value from 0..1.
    pub fn set(self, n: &mut Note, v: f32, per_step: u32) {
        let v = v.clamp(0.0, 1.0);
        let b = |max: f32| (v * max).round() as u8;
        match self {
            Tab::Note => n.key = KEY_LO + (v * f32::from(KEY_HI - KEY_LO)).round() as u16,
            Tab::Vel => n.velocity = b(128.0),
            Tab::Rel => n.release = b(128.0),
            Tab::Fine => n.fine_pitch = b(240.0),
            Tab::Pan => n.pan = b(128.0),
            Tab::X => n.mod_x = b(255.0),
            Tab::Y => n.mod_y = b(255.0),
            Tab::Shift => {
                let ps = per_step.max(1);
                n.position = n.position / ps * ps + ((v * (ps - 1) as f32).round() as u32).min(ps - 1);
            }
            Tab::Rep => n.repeat = b(f32::from(Note::MAX_REPEAT)),
        }
    }

    /// Hint text ("Velocity 39%").
    pub fn text(self, n: &Note, per_step: u32) -> String {
        let v = self.get(n, per_step);
        let pct = |v: f32| format!("{:.0}%", v * 100.0);
        match self {
            Tab::Note => format!("Note {}", key_name(n.key)),
            Tab::Vel => format!("Velocity {}", pct(v)),
            Tab::Rel => format!("Release {}", pct(v)),
            Tab::Fine => format!("Fine pitch {:+.0} cents", (f32::from(n.fine_pitch) - 120.0) * 100.0 / 120.0),
            Tab::Pan => format!("Panning {}", crate::knobs::pan_text((f32::from(n.pan) - 64.0) / 64.0)),
            Tab::X => format!("Mod X {}", pct(v)),
            Tab::Y => format!("Mod Y {}", pct(v)),
            Tab::Shift => format!("Time shift {}", pct(v)),
            Tab::Rep => format!("Note repeat {}", n.repeat),
        }
    }
}

/// "C5" for key 60, as the settings choose it (`crate::note_names`).
pub fn key_name(k: u16) -> String {
    crate::note_names::note_name(i32::from(k))
}

/// A named set of channels (§21).
#[derive(Debug, Clone, PartialEq)]
pub struct Group {
    pub name: String,
    pub members: BTreeSet<usize>,
}

/// Sort keys of Options > Sort by (§17).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortBy {
    Color,
    Name,
    Track,
    Type,
}

/// The rack's name boxes (they use the pattern name box, spec: ui/pattern-menu.md §4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RBox {
    /// Rename (and colour) a channel (§13).
    Channel(usize),
    /// Group selected: name of the new filter group (§21).
    GroupSel,
    /// Filter menu "Add filter group".
    NewGroup,
    RenameGroup(usize),
    /// Transpose selected: semitones.
    Transpose,
}

/// Rack UI state. Lock, zip, groups, the filter and loop lengths are saved through [`to_model`] / [`from_model`].
#[derive(Debug, Clone)]
pub struct RackUi {
    /// Selected channels (§8); the focused channel is `State::selected_channel`.
    pub sel: BTreeSet<usize>,
    pub filter: Filter,
    pub groups: Vec<Group>,
    pub zipped: BTreeSet<usize>,
    /// Channels whose mute state solo leaves alone (§10).
    pub locked: BTreeSet<usize>,
    /// The channel the last solo click soloed.
    pub soloed: Option<usize>,
    /// Mute states (`enabled` per row) from before the first solo: what unsolo puts back when "Restore previous
    /// state after solo" is on.
    pub pre_solo: Option<Vec<bool>>,
    /// The current pattern at the last tick (`None` = not yet seen), to notice a switch ([`App::pattern_tick`]).
    pub pattern_seen: Option<u16>,
    /// Name button width in percent (§17).
    pub name_pct: u32,
    pub show_mixer: bool,
    pub colorful: bool,
    pub align_left: bool,
    pub auto_resize: AutoResize,
    pub graph_open: bool,
    pub graph_tab: Tab,
    pub loop_mode: bool,
    /// Per-channel loop length in steps (§15).
    pub loop_len: BTreeMap<usize, u32>,
    pub loop_type: u8,
    /// Position of "Unsorted" in the file's group list (`None` = last; see [`to_model`]).
    pub unsorted_at: Option<usize>,
    /// Steps copied by Cut / Copy (the channel's notes of the pattern).
    pub clipboard: Option<Vec<Note>>,
    /// Copied knob value (knob menu, §19).
    pub value_clip: Option<f32>,
    /// Menu checkboxes that have no effect yet (greyed out): their default ticks.
    pub opts: BTreeSet<&'static str>,
    /// Channels waiting for the delete confirmation's Ok.
    pub pending_delete: Option<Vec<usize>>,
    /// The channels waiting are unused audio clips (Purge unused audio clips, spec: ui/unused-channels.md §4).
    pub purging: bool,
    /// Channels to delete now (the confirmation was accepted; the app removes them).
    pub delete_now: Option<Vec<usize>>,
    /// Advanced fill dialog (§22).
    pub fill: Option<FillDlg>,
}

impl Default for RackUi {
    fn default() -> Self {
        RackUi {
            sel: BTreeSet::from([0]),
            filter: Filter::All,
            groups: Vec::new(),
            zipped: BTreeSet::new(),
            locked: BTreeSet::new(),
            soloed: None,
            pre_solo: None,
            pattern_seen: None,
            name_pct: 100,
            show_mixer: true,
            colorful: false,
            align_left: false,
            auto_resize: AutoResize::Min,
            graph_open: false,
            graph_tab: Tab::Vel,
            loop_mode: false,
            loop_len: BTreeMap::new(),
            loop_type: 0,
            unsorted_at: None,
            clipboard: None,
            value_clip: None,
            opts: ["Show complete piano roll preview", "Show a mute button on the piano roll preview"].into_iter().collect(),
            pending_delete: None,
            purging: false,
            delete_now: None,
            fill: None,
        }
    }
}

impl RackUi {
    pub fn select_only(&mut self, r: usize) {
        self.sel.clear();
        self.sel.insert(r);
    }

    /// Back to the defaults for a new or opened project (view options stay).
    pub fn reset(&mut self) {
        let keep = (self.name_pct, self.show_mixer, self.colorful, self.align_left, self.auto_resize, self.opts.clone());
        *self = RackUi::default();
        (self.name_pct, self.show_mixer, self.colorful, self.align_left, self.auto_resize, self.opts) = keep;
    }

    /// Follow a channel renumbering (`m(old row) = new row`, `None` = removed).
    fn remap(&mut self, m: &dyn Fn(usize) -> Option<usize>) {
        let set = |s: &BTreeSet<usize>| s.iter().filter_map(|&r| m(r)).collect::<BTreeSet<usize>>();
        self.sel = set(&self.sel);
        self.zipped = set(&self.zipped);
        self.locked = set(&self.locked);
        for g in &mut self.groups {
            g.members = set(&g.members);
        }
        self.soloed = self.soloed.and_then(m);
        self.pre_solo = None; // rows renumbered: unsolo turns everything on
        self.loop_len = self.loop_len.iter().filter_map(|(&r, &l)| m(r).map(|n| (n, l))).collect();
        if let Some(f) = self.fill.as_mut() {
            match m(f.row) {
                Some(r) => f.row = r,
                None => self.fill = None,
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------------------
// Advanced fill (§22)

/// Fill type of the Advanced fill dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FillType {
    Manual,
    Every,
    Euclid,
}

impl FillType {
    pub fn label(self) -> &'static str {
        match self {
            FillType::Manual => "Manual",
            FillType::Every => "Fill every",
            FillType::Euclid => "Euclidean",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct FillDlg {
    pub row: usize,
    pub kind: FillType,
    /// Sequence length in steps.
    pub len: u32,
    /// Interval (Fill every) or step count (Euclidean).
    pub amount: u32,
    /// Rotation in steps (centre knob).
    pub rotate: u32,
    /// The sequence (one flag per step).
    pub steps: Vec<bool>,
    /// Top-left on screen.
    pub at: (f32, f32),
}

impl FillDlg {
    /// Recompute the sequence from the fill type (Manual keeps the edited steps).
    pub fn rebuild(&mut self) {
        let n = self.len.max(1) as usize;
        let base: Vec<bool> = match self.kind {
            FillType::Manual => {
                let mut v = self.steps.clone();
                v.resize(n, false);
                return self.steps = v;
            }
            FillType::Every => (0..n).map(|i| i % self.amount.max(1) as usize == 0).collect(),
            FillType::Euclid => euclid(n, (self.amount as usize).min(n)),
        };
        let r = self.rotate as usize % n;
        self.steps = (0..n).map(|i| base[(i + n - r) % n]).collect();
    }
}

/// `k` hits spread as evenly as possible over `n` steps, the first on step 0.
pub fn euclid(n: usize, k: usize) -> Vec<bool> {
    (0..n).map(|i| k > 0 && (i * k) % n < k).collect()
}

// ---------------------------------------------------------------------------------------------------------
// Rows, steps and lengths

/// Whether row `r` passes the display filter.
pub fn in_filter(s: &State, r: usize) -> bool {
    match s.rack.filter {
        Filter::All => true,
        Filter::Group(g) => s.rack.groups.get(g).is_some_and(|g| g.members.contains(&r)),
        Filter::Unsorted => !s.rack.groups.iter().any(|g| g.members.contains(&r)),
    }
}

/// The rows the rack shows, top to bottom.
pub fn shown(s: &State) -> Vec<usize> {
    (0..s.project.channels.len()).filter(|&r| in_filter(s, r)).collect()
}

/// Ticks per step.
pub fn per_step(s: &State) -> u32 {
    (u32::from(s.project.ppq.max(4)) / 4).max(1)
}

/// The current pattern's explicit length in steps (`None` = Auto).
pub fn explicit_len(s: &State) -> Option<u32> {
    let ps = per_step(s);
    s.project.pattern(s.pattern).and_then(|p| p.length).filter(|&l| l > 0).map(|l| (l / ps).max(1))
}

/// Pattern length in steps: the explicit length, else whole bars past the last used step, at least one bar (§3, §7).
pub fn pattern_steps(s: &State) -> u32 {
    if let Some(l) = explicit_len(s) {
        return l;
    }
    let ps = per_step(s);
    let last = s.project.pattern(s.pattern).and_then(|p| p.notes.iter().map(|n| n.position / ps).max());
    last.map_or(16, |l| (l / 16 + 1) * 16)
}

/// Set the current pattern's length (`None` = Auto).
pub fn set_len(s: &mut State, steps: Option<u32>) {
    let ps = per_step(s);
    let n = s.pattern.max(1);
    crate::patterns::pat_mut(s, n).length = steps.filter(|&v| v > 0).map(|v| v.min(MAX_STEPS) * ps);
    s.piano.dirty = true;
}

/// Length box value: 0 = Auto.
pub fn len_value(s: &State) -> u32 {
    explicit_len(s).unwrap_or(0)
}

/// A row's own length in steps: its loop length in loop mode, else the pattern length (§15).
pub fn row_len(s: &State, r: usize) -> u32 {
    let pat = pattern_steps(s);
    if s.rack.loop_mode { s.rack.loop_len.get(&r).copied().unwrap_or(pat).max(1) } else { pat }
}

/// Channel index (note `channel`) of row `r`.
fn ch_index(s: &State, r: usize) -> Option<u16> {
    s.project.channels.get(r).map(|c| c.index)
}

/// Whether step `k` of channel `ch` has a note (a note anywhere inside the step counts).
pub fn step_on(notes: &[Note], ch: u16, k: u32, ps: u32) -> bool {
    notes.iter().any(|n| n.channel == ch && n.position / ps == k)
}

/// The current pattern's notes of row `r`.
pub fn row_notes(s: &State, r: usize) -> Vec<Note> {
    let Some(ch) = ch_index(s, r) else { return Vec::new() };
    s.project.pattern(s.pattern).map(|p| p.notes.iter().filter(|n| n.channel == ch).copied().collect()).unwrap_or_default()
}

/// Replace row `r`'s notes in the current pattern.
fn set_row_notes(s: &mut State, r: usize, notes: Vec<Note>) {
    let Some(ch) = ch_index(s, r) else { return };
    let n = s.pattern.max(1);
    let p = crate::patterns::pat_mut(s, n);
    p.notes.retain(|x| x.channel != ch);
    p.notes.extend(notes.into_iter().map(|x| Note { channel: ch, ..x }));
    p.notes.sort_by_key(|x| (x.position, x.channel));
    s.piano.dirty = true;
}

/// Turn step `k` of row `r` on or off.
pub fn set_step(s: &mut State, r: usize, k: u32, on: bool) {
    let Some(ch) = ch_index(s, r) else { return };
    let ps = per_step(s);
    let n = s.pattern.max(1);
    let p = crate::patterns::pat_mut(s, n);
    let has = step_on(&p.notes, ch, k, ps);
    if on && !has {
        p.notes.push(Note::step(ch, k * ps));
        p.notes.sort_by_key(|x| (x.position, x.channel));
    } else if !on && has {
        p.notes.retain(|x| !(x.channel == ch && x.position / ps == k));
    }
    s.piano.dirty = true;
}

/// Toggle step `k` of row `r`; returns its new state (§9).
pub fn toggle_step(s: &mut State, r: usize, k: u32) -> bool {
    let on = !ch_index(s, r).is_some_and(|ch| s.project.pattern(s.pattern).is_some_and(|p| step_on(&p.notes, ch, k, per_step(s))));
    set_step(s, r, k, on);
    on
}

/// Fill each N steps (§18): the row's notes inside the pattern length become steps 1, 1+N, 1+2N, …
pub fn fill_each(s: &mut State, r: usize, every: u32) {
    let (len, ps) = (row_len(s, r), per_step(s));
    let mut notes: Vec<Note> = row_notes(s, r).into_iter().filter(|n| n.position >= len * ps).collect();
    notes.extend((0..len).step_by(every.max(1) as usize).map(|k| Note::step(0, k * ps)));
    set_row_notes(s, r, notes);
}

/// Write a step sequence (Advanced fill's Accept) over the row's notes inside the sequence length.
pub fn write_sequence(s: &mut State, r: usize, steps: &[bool]) {
    let ps = per_step(s);
    let len = steps.len() as u32;
    let mut notes: Vec<Note> = row_notes(s, r).into_iter().filter(|n| n.position >= len * ps).collect();
    notes.extend(steps.iter().enumerate().filter(|(_, on)| **on).map(|(k, _)| Note::step(0, k as u32 * ps)));
    set_row_notes(s, r, notes);
}

/// Rotate the row's notes by one step, wrapping inside the pattern length (§18).
pub fn rotate(s: &mut State, r: usize, dir: i32) {
    let (len, ps) = (row_len(s, r), per_step(s));
    let span = i64::from(len * ps);
    let notes = row_notes(s, r)
        .into_iter()
        .map(|mut n| {
            if i64::from(n.position) < span {
                n.position = (i64::from(n.position) + i64::from(dir) * i64::from(ps)).rem_euclid(span) as u32;
            }
            n
        })
        .collect();
    set_row_notes(s, r, notes);
}

/// The steps the row shows: on/off for steps `0..len` (used by the Advanced fill dialog).
pub fn row_steps(s: &State, r: usize, len: u32) -> Vec<bool> {
    let ps = per_step(s);
    let notes = row_notes(s, r);
    (0..len).map(|k| notes.iter().any(|n| n.position / ps == k)).collect()
}

// ---------------------------------------------------------------------------------------------------------
// Channel edits

pub(crate) fn channel_name(s: &State, r: usize) -> String {
    s.project.channels.get(r).map(|c| c.name.clone().unwrap_or_else(|| format!("Channel {}", c.index + 1))).unwrap_or_default()
}

/// "<name> #k" with the lowest free k >= 2 (§18 Clone).
pub fn clone_name(s: &State, r: usize) -> String {
    let name = channel_name(s, r);
    let base = match name.rsplit_once(" #") {
        Some((b, k)) if k.parse::<u32>().is_ok() => b.to_string(),
        _ => name,
    };
    let taken = |cand: &str| s.project.channels.iter().any(|c| c.name.as_deref() == Some(cand));
    (2..).map(|k| format!("{base} #{k}")).find(|c| !taken(c)).expect("unbounded")
}

fn seed() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64)
}

pub fn random_color(s: &mut State, r: usize, salt: u64) {
    if let Some(c) = s.project.channels.get_mut(r) {
        c.color = Some(crate::patterns::random_color(seed() ^ salt.wrapping_mul(0x9e37_79b9)));
    }
}

/// Color selected > Gradient: from the first selected channel's colour to the last one's (a hue sweep when they
/// match or have none; Lemon Studio's version of FL's gradient dialog).
pub fn gradient(s: &mut State) {
    let rows: Vec<usize> = s.rack.sel.iter().copied().filter(|&r| r < s.project.channels.len()).collect();
    if rows.len() < 2 {
        return;
    }
    let (a, b) = (s.project.channels[rows[0]].color, s.project.channels[*rows.last().expect("len >= 2")].color);
    let n = (rows.len() - 1) as f32;
    for (i, &r) in rows.iter().enumerate() {
        let t = i as f32 / n;
        let c = match (a, b) {
            (Some(a), Some(b)) if a != b => {
                let l = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
                Rgb::new(l(a.r, b.r), l(a.g, b.g), l(a.b, b.b))
            }
            _ => crate::playlist::hsl_rgb(300.0 * t, 0.35, 0.45),
        };
        s.project.channels[r].color = Some(c);
        s.project.channels[r].rack.stored_color = Some(c);
    }
}

/// Ctrl / Alt + click on a LED (§10): solo this channel; the same click again on the soloed channel unmutes all, or,
/// with "Restore previous state after solo" on, puts back the states from before the first solo (a solo moved to
/// another channel keeps them; spec: ui/settings-choices.md "Miscellaneous"). Locked channels keep their state.
pub fn solo(s: &mut State, r: usize) {
    let locked = s.rack.locked.clone();
    let soloed_now = solo_of(s, r);
    // A solo still in effect (the last solo click's shape, untouched since) keeps the states saved when it started.
    let in_solo = s.rack.soloed.is_some_and(|a| solo_of(s, a));
    if !soloed_now && !in_solo {
        s.rack.pre_solo = Some(s.project.channels.iter().map(|c| c.enabled).collect());
    }
    let back = if soloed_now { s.rack.pre_solo.take().filter(|_| in_solo && crate::prefs::check(&crate::prefs::RESTORE_AFTER_SOLO)) } else { None };
    for (i, c) in s.project.channels.iter_mut().enumerate() {
        if locked.contains(&i) {
            if i == r {
                c.enabled = true;
            }
        } else if let Some(b) = &back {
            c.enabled = b.get(i).copied().unwrap_or(true);
        } else {
            c.enabled = i == r || soloed_now;
        }
    }
    s.rack.soloed = (!soloed_now).then_some(r);
    s.piano.dirty = true;
}

/// What follows a pattern switch. Run on every tick and before each frame (`App::follow_settings`), so every way of
/// switching pattern counts (numpad - / +, the pattern picker and box, a new pattern, the playlist...); a new or opened
/// project is not a switch. Two General ▸ Miscellaneous checks (spec: ui/settings-choices.md "Miscellaneous"):
/// - "Auto zip empty channels": the channels with notes or steps in the new pattern are unzipped and every other one
///   is zipped; a pattern without any unzips every channel. Zipping by hand works as ever between switches.
/// - "Select first channel with notes when the pattern changes": the lowest-numbered channel with notes or steps in
///   the new pattern becomes the only selected channel and the focused one; a pattern without any (also one just made
///   with "+") leaves the selection alone.
impl App {
    pub(crate) fn pattern_tick(&mut self) {
        let now = self.state.pattern;
        let seen = self.state.rack.pattern_seen.replace(now);
        if seen.is_none_or(|p| p == now) {
            return;
        }
        // Per rack row: the channel has notes or steps in the new pattern.
        let s = &self.state;
        let used: Vec<bool> = match s.project.pattern(now) {
            Some(p) => s.project.channels.iter().map(|c| p.notes.iter().any(|n| n.channel == c.index)).collect(),
            None => vec![false; s.project.channels.len()],
        };
        if crate::prefs::check(&crate::prefs::AUTO_ZIP_EMPTY) {
            let s = &mut self.state;
            let zipped: BTreeSet<usize> = used.iter().enumerate().filter(|&(_, &u)| !u).map(|(r, _)| r).collect();
            s.rack.zipped = if zipped.len() == used.len() { BTreeSet::new() } else { zipped };
            autosize(s);
            self.invalidate();
        }
        if crate::prefs::check(&crate::prefs::SELECT_FIRST_CHANNEL) {
            if let Some(r) = used.iter().position(|&u| u) {
                self.state.rack.select_only(r);
                self.focus_channel(r);
                self.invalidate();
            }
        }
    }
}

/// Channel `r` is the only unlocked channel on.
fn solo_of(s: &State, r: usize) -> bool {
    let locked = &s.rack.locked;
    s.project.channels.get(r).is_some_and(|c| c.enabled)
        && s.project.channels.iter().enumerate().all(|(i, c)| i == r || locked.contains(&i) || !c.enabled)
}

pub fn toggle_mute(s: &mut State, r: usize) {
    if let Some(c) = s.project.channels.get_mut(r) {
        c.enabled = !c.enabled;
        s.piano.dirty = true;
    }
}

pub fn toggle_lock(s: &mut State, r: usize) {
    if !s.rack.locked.remove(&r) {
        s.rack.locked.insert(r);
    }
}

/// Channels with no notes in any pattern that no playlist clip uses (Select unused channels,
/// spec: ui/unused-channels.md §1).
pub fn unused(s: &State) -> BTreeSet<usize> {
    let used_clip = |i: u16| s.project.arrangements.iter().any(|a| a.clips.iter().any(|c| c.source == ClipSource::Channel(i)));
    s.project
        .channels
        .iter()
        .enumerate()
        .filter(|(_, c)| !s.project.patterns.iter().any(|p| p.notes.iter().any(|n| n.channel == c.index)) && !used_clip(c.index))
        .map(|(r, _)| r)
        .collect()
}

/// Transpose the selected channels' notes of the current pattern.
pub fn transpose_selected(s: &mut State, semis: i32) {
    let chans: Vec<u16> = s.rack.sel.iter().filter_map(|&r| ch_index(s, r)).collect();
    let n = s.pattern.max(1);
    for note in crate::patterns::pat_mut(s, n).notes.iter_mut().filter(|x| chans.contains(&x.channel)) {
        note.key = (i32::from(note.key) + semis).clamp(0, 131) as u16;
    }
    s.piano.dirty = true;
}

/// Window height for Auto resize (§17): Minimum = fit the shown rows; Maximum = down to the screen bottom.
pub fn auto_height(s: &State) -> Option<f32> {
    let fit = 96.0 + crate::views::rack::rows_h(s).max(crate::views::rack::ROW_H);
    let room = (s.screen.1 - s.rack_pos.1 - 4.0).max(crate::views::rack::MIN_H);
    match s.rack.auto_resize {
        AutoResize::Off => None,
        AutoResize::Min => Some(fit.min(room)),
        AutoResize::Max => Some(room),
    }
}

/// Apply Auto resize after the shown rows changed.
pub fn autosize(s: &mut State) {
    if let Some(h) = auto_height(s) {
        s.rack_size.1 = h.max(crate::views::rack::MIN_H);
    }
}

/// Keep the step count when a column left of the steps changes width (§17: the window width follows).
pub fn relayout(s: &mut State, f: impl FnOnce(&mut State)) {
    let before = crate::views::rack::base_w(s);
    f(s);
    let after = crate::views::rack::base_w(s);
    s.rack_size.0 = (s.rack_size.0 + after - before).max(after + 3.0 * crate::views::rack::STEP_W);
}

// ---------------------------------------------------------------------------------------------------------
// Renumbering (delete, clone, move, sort)

/// Reorder / remove channels in the project: `map[old row] = new row`. Every reference follows (notes, control
/// points, automation clips and their targets, playlist clips, layer children); indices become positions.
pub fn remap_project(p: &mut Project, map: &[Option<usize>]) {
    let idx: BTreeMap<u16, u16> =
        p.channels.iter().enumerate().filter_map(|(pos, c)| map.get(pos).copied().flatten().map(|n| (c.index, n as u16))).collect();
    let mut chans: Vec<(usize, os_model::Channel)> =
        std::mem::take(&mut p.channels).into_iter().enumerate().filter_map(|(pos, c)| map.get(pos).copied().flatten().map(|n| (n, c))).collect();
    chans.sort_by_key(|(n, _)| *n);
    p.channels = chans
        .into_iter()
        .map(|(n, mut c)| {
            c.index = n as u16;
            c.layer_children = c.layer_children.iter().filter_map(|k| idx.get(k).copied()).collect();
            c
        })
        .collect();
    for pat in &mut p.patterns {
        pat.notes.retain_mut(|n| idx.get(&n.channel).map(|&m| n.channel = m).is_some());
        pat.controls.retain_mut(|c| idx.get(&c.channel()).map(|&m| *c = os_model::ControlPoint::new(c.position, m, c.param(), c.value)).is_some());
    }
    p.automations.retain_mut(|a| {
        let Some(&m) = idx.get(&a.channel) else { return false };
        a.channel = m;
        // Channel targets keep the channel in the high half (spec: flp/events/automation.md "Targets").
        a.targets.retain_mut(|t| {
            let hi = (*t >> 16) as u16;
            if *t == target::TEMPO || hi >= 0x4000 {
                return true;
            }
            idx.get(&hi).map(|&n| *t = target::channel(n, *t as u16)).is_some()
        });
        true
    });
    for arr in &mut p.arrangements {
        arr.clips.retain_mut(|c| match c.source {
            ClipSource::Channel(ch) => idx.get(&ch).map(|&m| c.source = ClipSource::Channel(m)).is_some(),
            ClipSource::Pattern(_) => true,
        });
    }
}

/// Apply a new channel order (`order` = old rows top to bottom; rows left out are deleted) to the project, the
/// app state and the hosted plugins.
pub fn reorder(app: &mut App, order: &[usize]) {
    let n = app.state.project.channels.len();
    let mut map = vec![None; n];
    for (new, &old) in order.iter().enumerate() {
        if old < n {
            map[old] = Some(new);
        }
    }
    let m = |r: usize| map.get(r).copied().flatten();
    let s = &mut app.state;
    remap_project(&mut s.project, &map);
    let len = s.project.channels.len().max(1);
    s.rack.remap(&m);
    let keep = |r: usize| m(r).unwrap_or(r.min(len - 1));
    s.selected_channel = keep(s.selected_channel);
    if m(s.plugin_channel).is_none() {
        s.hide(Win::Plugin);
    }
    s.plugin_channel = keep(s.plugin_channel);
    s.hosted_rows = s.hosted_rows.iter().filter_map(|&r| m(r)).collect();
    s.sample_loads = std::mem::take(&mut s.sample_loads).into_iter().filter_map(|(r, p)| m(r).map(|r| (r, p))).collect();
    s.pending_open = s.pending_open.and_then(m);
    s.pending_plugin = s.pending_plugin.and_then(|(r, i)| m(r).map(|r| (r, i)));
    s.piano.clear_selection();
    s.piano.dirty = true;
    s.playlist.sel.clear();
    s.playlist.hover_clip = None;
    if s.rack.sel.is_empty() && !s.project.channels.is_empty() {
        s.rack.select_only(s.selected_channel);
    }
    autosize(s);
    app.remap_rows(&m);
    app.invalidate();
}

/// Clone rows (§18): each copy goes right below its channel, named "<name> #2", same settings and mixer track,
/// no steps; the hosted instrument is copied with its state. The clones become the selection.
pub fn clone_rows(app: &mut App, rows: &[usize]) {
    let n = app.state.project.channels.len();
    let rows: BTreeSet<usize> = rows.iter().copied().filter(|&r| r < n).collect();
    if rows.is_empty() {
        return;
    }
    let mut order = Vec::new();
    let mut sources = Vec::new();
    for r in 0..n {
        order.push(r);
        if rows.contains(&r) {
            let name = clone_name(&app.state, r);
            let mut c = app.state.project.channels[r].clone();
            c.index = app.state.project.channels.len() as u16;
            c.name = Some(name);
            app.state.project.channels.push(c);
            order.push(app.state.project.channels.len() - 1);
            sources.push(r);
        }
    }
    let states: Vec<Option<(usize, Vec<u8>)>> = sources.iter().map(|&r| app.plugin_copy(r)).collect();
    reorder(app, &order);
    let new_rows: Vec<usize> = order.iter().enumerate().filter(|(_, o)| **o >= n).map(|(i, _)| i).collect();
    for (row, st) in new_rows.iter().zip(states) {
        if let Some((index, blob)) = st {
            app.plugin_paste(*row, index, &blob);
        }
    }
    let s = &mut app.state;
    s.rack.sel = new_rows.iter().copied().collect();
    if let Some(&last) = new_rows.last() {
        s.selected_channel = last;
    }
    s.message = Some(if new_rows.len() == 1 { format!("Cloned as {}", channel_name(s, new_rows[0])) } else { format!("Cloned {} channels", new_rows.len()) });
}

/// Ask before deleting rows (§18: "The selected channel is about to be deleted.").
pub fn ask_delete(s: &mut State, rows: Vec<usize>) {
    s.rack.purging = false;
    if rows.is_empty() {
        return;
    }
    if rows.len() >= s.project.channels.len() {
        s.message = Some("A project keeps at least one channel".into());
        return;
    }
    s.rack.pending_delete = Some(rows);
    s.pat_ui.confirm = true;
}

/// Purge unused audio clips: the unused audio-clip channels go, after the delete confirmation (Lemon Studio cannot
/// undo a channel deletion yet; spec: ui/unused-channels.md §4).
pub fn ask_purge(s: &mut State) -> Option<String> {
    let rows: Vec<usize> = unused(s).into_iter().filter(|&r| s.project.channels[r].kind == ChannelKind::AudioClip).collect();
    if rows.is_empty() {
        return Some("No unused audio clips".into());
    }
    ask_delete(s, rows);
    s.rack.purging = s.rack.pending_delete.is_some();
    None
}

/// Delete rows now: the channels, their notes and clips go.
pub fn delete_rows(app: &mut App, rows: &[usize]) {
    let n = app.state.project.channels.len();
    let order: Vec<usize> = (0..n).filter(|r| !rows.contains(r)).collect();
    let first = rows.iter().copied().min().unwrap_or(0);
    reorder(app, &order);
    let s = &mut app.state;
    let r = first.min(s.project.channels.len().saturating_sub(1));
    s.rack.select_only(r);
    s.selected_channel = r;
    s.message = Some(if rows.len() == 1 { "Deleted the channel".into() } else { format!("Deleted {} channels", rows.len()) });
}

/// Move the selected channels one row up (`dir` -1) or down (+1).
pub fn move_selected(app: &mut App, dir: i32) {
    let n = app.state.project.channels.len();
    let sel = app.state.rack.sel.clone();
    if sel.is_empty() || (dir < 0 && sel.contains(&0)) || (dir > 0 && sel.contains(&(n - 1))) {
        return;
    }
    let mut order: Vec<usize> = (0..n).collect();
    if dir < 0 {
        for i in 1..n {
            if sel.contains(&order[i]) && !sel.contains(&order[i - 1]) {
                order.swap(i, i - 1);
            }
        }
    } else {
        for i in (0..n - 1).rev() {
            if sel.contains(&order[i]) && !sel.contains(&order[i + 1]) {
                order.swap(i, i + 1);
            }
        }
    }
    reorder(app, &order);
}

pub fn sort(app: &mut App, by: SortBy) {
    let s = &app.state;
    let mut order: Vec<usize> = (0..s.project.channels.len()).collect();
    let ch = |r: usize| &s.project.channels[r];
    match by {
        SortBy::Color => order.sort_by_key(|&r| ch(r).color.map_or(u32::MAX, |c| u32::from(c.r) << 16 | u32::from(c.g) << 8 | u32::from(c.b))),
        SortBy::Name => order.sort_by_key(|&r| channel_name(s, r).to_lowercase()),
        SortBy::Track => order.sort_by_key(|&r| ch(r).mixer_track),
        SortBy::Type => order.sort_by_key(|&r| match ch(r).kind {
            ChannelKind::Sampler => 0,
            ChannelKind::Plugin => 1,
            ChannelKind::Layer => 2,
            ChannelKind::AudioClip => 3,
            ChannelKind::Automation => 4,
            ChannelKind::Other(k) => 5 + u32::from(k),
        }),
    }
    reorder(app, &order);
}

// ---------------------------------------------------------------------------------------------------------
// Name boxes

pub fn box_text(s: &State, b: RBox) -> String {
    match b {
        RBox::Channel(r) => channel_name(s, r),
        RBox::GroupSel | RBox::NewGroup => format!("Group {}", s.rack.groups.len() + 1),
        RBox::RenameGroup(g) => s.rack.groups.get(g).map(|g| g.name.clone()).unwrap_or_default(),
        RBox::Transpose => String::new(),
    }
}

pub fn box_caption(s: &State, b: RBox) -> String {
    match b {
        RBox::Channel(r) => format!("Channel #{} name", s.project.channels.get(r).map_or(r + 1, |c| usize::from(c.index) + 1)),
        RBox::GroupSel | RBox::NewGroup | RBox::RenameGroup(_) => "Filter group name".into(),
        RBox::Transpose => "Semitones".into(),
    }
}

pub fn box_accept(s: &mut State, b: RBox, text: &str) {
    let text = text.trim();
    match b {
        RBox::Channel(r) => {
            if let Some(c) = s.project.channels.get_mut(r) {
                c.name = (!text.is_empty()).then(|| text.to_string());
            }
        }
        RBox::GroupSel | RBox::NewGroup if !text.is_empty() => {
            let members = if b == RBox::GroupSel { s.rack.sel.clone() } else { BTreeSet::new() };
            s.rack.groups.push(Group { name: text.to_string(), members });
            s.rack.filter = Filter::Group(s.rack.groups.len() - 1);
            autosize(s);
        }
        RBox::RenameGroup(g) if !text.is_empty() => {
            if let Some(g) = s.rack.groups.get_mut(g) {
                g.name = text.to_string();
            }
        }
        RBox::Transpose => {
            if let Ok(v) = text.trim_start_matches('+').parse::<i32>() {
                transpose_selected(s, v);
            }
        }
        _ => {}
    }
}

/// Open a rack name box at `at`.
pub fn open_box(s: &mut State, b: RBox, at: (f32, f32)) {
    crate::patterns::open_name_box(s, crate::patterns::BoxTarget::Rack(b), at);
}

// ---------------------------------------------------------------------------------------------------------
// Popup menus

/// Commands of the rack's popup menus.
#[derive(Debug, Clone, PartialEq)]
pub enum RCmd {
    // Channel menu (§18), on a row.
    PianoRoll(usize),
    Graph(usize),
    Rename(usize),
    ChangeColor(usize),
    RandomColor(usize),
    LoadSample(usize),
    Clone(usize),
    Delete(usize),
    Cut(usize),
    Copy(usize),
    Paste(usize),
    Fill(usize, u32),
    AdvancedFill(usize),
    Rotate(usize, i32),
    // LED menu (§10).
    Solo(usize),
    Lock(usize),
    // Knob menu (§19) and the mixer box.
    Reset(KnobId),
    CopyValue(KnobId),
    PasteValue(KnobId),
    MixerReset(usize),
    AssignFree(usize),
    // Display filter (§21).
    SetFilter(Filter),
    AddGroup,
    DeleteGroup,
    RenameGroup,
    // Pattern length (§7).
    SetLen(Option<u32>),
    // Loop button (§7).
    LoopType(u8),
    Burn(bool),
    // Options menu (§17).
    SelectUnused,
    Purge,
    CloneSel,
    DeleteSel,
    MoveSel(i32),
    Sort(SortBy),
    GroupSel,
    ColorGradient,
    ColorRandom,
    MuteSel(bool),
    TransposeSel,
    AssignFreeSel,
    SelectMixerTracks,
    ZipSel,
    UnzipAll,
    AutoResize(AutoResize),
    ResizeNow,
    NameWidth(u32),
    SizeToFit,
    AlignLeft(bool),
    ShowMixer,
    Colorful,
}

fn it(label: impl Into<std::borrow::Cow<'static, str>>, c: RCmd) -> PItem {
    PItem::new(label, Cmd::Rack(c))
}

/// A menu option that is not available yet: greyed out, showing its default tick.
fn opt(s: &State, label: &'static str) -> PItem {
    PItem::not_yet(label).mark(Mark::Check(s.rack.opts.contains(label)))
}

/// Options menu (title ▸, §17).
pub fn options_menu(s: &State, x: f32, y: f32) -> Popup {
    let r = &s.rack;
    let none = r.sel.is_empty();
    let off = |i: PItem, cond: bool| if cond { i.off() } else { i };
    let widths = [100, 200, 300, 400];
    let mut width: Vec<PItem> = widths.iter().map(|&w| it(format!("{w}%"), RCmd::NameWidth(w)).mark(Mark::Radio(r.name_pct == w))).collect();
    let custom = !widths.contains(&r.name_pct);
    width.push(PItem::new(if custom { format!("Custom ({}%)", r.name_pct) } else { "Custom".into() }, Cmd::Nothing).mark(Mark::Radio(custom)).off());
    width.push(PItem::sep());
    width.push(it("Size to fit", RCmd::SizeToFit));
    let items = vec![
        PItem::sub("Add one", add_one_items(s)),
        PItem::not_yet("Loop starter").mark(Mark::Check(false)),
        it("Purge unused audio clips", RCmd::Purge),
        PItem::head("Channels"),
        it("Select unused channels", RCmd::SelectUnused),
        off(it("Clone selected", RCmd::CloneSel).key("Alt+C"), none),
        off(it("Delete selected...", RCmd::DeleteSel).key("Alt+Del"), none),
        PItem::not_yet("Create layer with selected channels"),
        PItem::sep(),
        off(it("Move selected up", RCmd::MoveSel(-1)).key("Alt+Up"), none),
        off(it("Move selected down", RCmd::MoveSel(1)).key("Alt+Down"), none),
        PItem::sub(
            "Sort by",
            vec![it("Color", RCmd::Sort(SortBy::Color)), it("Name", RCmd::Sort(SortBy::Name)), it("Track number", RCmd::Sort(SortBy::Track)), it("Type", RCmd::Sort(SortBy::Type))],
        ),
        PItem::sep(),
        off(it("Group selected...", RCmd::GroupSel).key("Alt+G"), none),
        PItem::sub(
            "Color selected",
            vec![it("Gradient...", RCmd::ColorGradient).key("Shift+Alt+C"), it("Random", RCmd::ColorRandom).key("Ctrl+Alt+C"), PItem::not_yet("All...")],
        ),
        off(it("Mute selected", RCmd::MuteSel(true)), none),
        off(it("Unmute selected", RCmd::MuteSel(false)), none),
        off(it("Transpose selected...", RCmd::TransposeSel), none),
        PItem::sub("Set swing mix for selected", ["0%", "25%", "50%", "75%", "100%", "Custom..."].into_iter().map(PItem::not_yet).collect()),
        PItem::sub("Set truncate swing notes for selected", vec![PItem::not_yet("Turn on"), PItem::not_yet("Turn off")]),
        PItem::not_yet("Burn selected loops to pattern"),
        off(it("Assign selected to free mixer track(s)", RCmd::AssignFreeSel).key("Ctrl+L"), none),
        off(it("Select mixer tracks for the selected channels", RCmd::SelectMixerTracks), none),
        PItem::sub(
            "Change declicking for selected channels to",
            ["Out only (no bleeding)", "Transient (no bleeding)", "Transient (bleeding)", "Generic (bleeding)", "Smooth (bleeding)", "Crossfade (bleeding)"]
                .into_iter()
                .map(PItem::not_yet)
                .collect(),
        ),
        PItem::sep(),
        off(it("Zip selected", RCmd::ZipSel).key("Alt+Z"), none),
        it("Unzip all", RCmd::UnzipAll).key("Alt+U"),
        PItem::sep(),
        PItem::not_yet("Restretch all").key("Ctrl+Alt+R"),
        PItem::head("Options"),
        PItem::sub(
            "Group playlist tracks created by Loop starter",
            vec![PItem::not_yet("Don't group tracks"), PItem::not_yet("Group tracks"), PItem::not_yet("Always create a track for the pattern clip")],
        ),
        PItem::sub(
            "Auto resize",
            vec![
                it("Off", RCmd::AutoResize(AutoResize::Off)).mark(Mark::Radio(r.auto_resize == AutoResize::Off)),
                it("Minimum height", RCmd::AutoResize(AutoResize::Min)).mark(Mark::Radio(r.auto_resize == AutoResize::Min)),
                it("Maximum height", RCmd::AutoResize(AutoResize::Max)).mark(Mark::Radio(r.auto_resize == AutoResize::Max)),
                PItem::sep(),
                it("Resize now", RCmd::ResizeNow),
            ],
        ),
        opt(s, "Auto switch display filter"),
        PItem::sub("Channel button width", width),
        PItem::sub(
            "Channel name alignment",
            vec![it("Left", RCmd::AlignLeft(true)).mark(Mark::Radio(r.align_left)), it("Center", RCmd::AlignLeft(false)).mark(Mark::Radio(!r.align_left))],
        ),
        it("Colorful channel controls", RCmd::Colorful).mark(Mark::Check(r.colorful)),
        opt(s, "Focus selected channel on playback"),
        opt(s, "Mute removed steps"),
        it("Show mixer track selectors", RCmd::ShowMixer).key("Alt+M").mark(Mark::Check(r.show_mixer)),
        opt(s, "Show complete piano roll preview"),
        opt(s, "Show a mute button on the piano roll preview"),
        opt(s, "Detached"),
        PItem::head("Switch"),
        PItem::not_yet("Switch smart disable for all plugins"),
        PItem::sub("Switch all audio clip channels to", vec![PItem::not_yet("Resample"), PItem::not_yet("Stretch")]),
        PItem::sub("Switch selected audio clip channels to", vec![PItem::not_yet("Resample"), PItem::not_yet("Stretch")]),
        PItem::sub("Switch all plugin editors to", vec![PItem::not_yet("Attached"), PItem::not_yet("Detached")]),
    ];
    Popup { x, y, items, sub: None }
}

/// Add one ▸: the generator menu, a pick goes to the bottom of the rack (spec: ui/plugin-menu.md §1).
pub(crate) fn add_one_items(s: &State) -> Vec<PItem> {
    crate::plugin_menu::items(s, crate::plugin_menu::Target::Gen(None), true)
}

/// Right click on a name button (§18): two columns.
pub fn channel_menu(s: &State, r: usize, x: f32, y: f32) -> Popup {
    let sampler = s.project.channels.get(r).is_some_and(|c| c.kind.is_sampler());
    let piano_on = s.shown(Win::PianoRoll) && s.selected_channel == r;
    let mut items = vec![
        it("Piano roll", RCmd::PianoRoll(r)).key("F7").mark(Mark::Check(piano_on)),
        it("Graph editor", RCmd::Graph(r)),
        PItem::sep(),
        it("Rename, color and icon...", RCmd::Rename(r)),
        it("Change color...", RCmd::ChangeColor(r)),
        it("Random color", RCmd::RandomColor(r)),
        PItem::not_yet("Change icon..."),
        PItem::sep(),
        if sampler { it("Load sample...", RCmd::LoadSample(r)) } else { PItem::new("Load sample...", Cmd::Nothing).off() },
        PItem::not_yet("Cut itself").mark(Mark::Check(false)),
        PItem::new("Show waveform preview", Cmd::Nothing).off(),
        PItem::sep(),
        PItem::sub("Insert", crate::plugin_menu::items(s, crate::plugin_menu::Target::Gen(Some(r)), true)),
        PItem::not_yet("Replace"),
        PItem::new("Patcherize", Cmd::Nothing).off(),
        it("Clone", RCmd::Clone(r)),
        it("Delete...", RCmd::Delete(r)),
        PItem::sep(),
        PItem::not_yet("Assign to new instrument track"),
        PItem::col(),
        it("Cut", RCmd::Cut(r)),
        it("Copy", RCmd::Copy(r)),
    ];
    let paste = it("Paste", RCmd::Paste(r));
    items.push(if s.rack.clipboard.is_some() { paste } else { paste.off() });
    items.extend([
        PItem::sep(),
        it("Fill each 2 steps", RCmd::Fill(r, 2)),
        it("Fill each 4 steps", RCmd::Fill(r, 4)),
        it("Fill each 8 steps", RCmd::Fill(r, 8)),
        it("Advanced fill...", RCmd::AdvancedFill(r)),
        PItem::new("Auto fill", Cmd::Nothing).off(),
        PItem::sep(),
        it("Rotate left", RCmd::Rotate(r, -1)).key("Shift+Ctrl+Left"),
        it("Rotate right", RCmd::Rotate(r, 1)).key("Shift+Ctrl+Right"),
        PItem::sep(),
        PItem::not_yet("MIDI channel through").mark(Mark::Check(false)),
        PItem::sub("Receive notes from", vec![PItem::new("Unlock", Cmd::Nothing).off(), PItem::not_yet("Typing keyboard").mark(Mark::Check(true))]),
        PItem::not_yet("Create instrument..."),
        PItem::sub("Burn MIDI to", vec![PItem::not_yet("Current pattern"), PItem::not_yet("New pattern")]),
    ]);
    Popup { x, y, items, sub: None }
}

/// The Automation / Remote control / Value groups shared by knob and LED menus (§19).
fn value_groups(s: &State, knob: Option<KnobId>, auto: Option<(u32, String, f64)>) -> Vec<PItem> {
    let mut v = vec![
        PItem::head("Automation"),
        PItem::not_yet("Edit events"),
        PItem::new("Edit events in new window", Cmd::Nothing).off(),
        PItem::new("Edit events in piano roll", Cmd::Nothing).off(),
        PItem::not_yet("Init song with this position"),
        PItem::new("Delete initial value", Cmd::Nothing).off(),
        PItem::sep(),
    ];
    v.push(match auto {
        Some((tgt, name, val)) => PItem::new("Create automation clip", Cmd::CreateAutomation(tgt, name, val)),
        None => PItem::new("Create automation clip", Cmd::Nothing).off(),
    });
    v.extend([
        PItem::new("Edit automation clip...", Cmd::Nothing).off(),
        PItem::head("Remote control"),
        PItem::not_yet("Link to controller..."),
        PItem::not_yet("Override global link..."),
        PItem::head("Value"),
    ]);
    match knob {
        Some(k) => {
            v.push(it("Copy value", RCmd::CopyValue(k)));
            let p = it("Paste value", RCmd::PasteValue(k));
            v.push(if s.rack.value_clip.is_some() { p } else { p.off() });
        }
        None => v.extend([PItem::not_yet("Copy value"), PItem::new("Paste value", Cmd::Nothing).off()]),
    }
    // spec: ui/type-in-value.md §8
    v.push(match knob {
        Some(k) => PItem::new("Type in value...", Cmd::TypeIn(crate::typein::Target::Knob(k))),
        None => PItem::not_yet("Type in value..."),
    });
    v.push(PItem::not_yet("Set..."));
    v
}

/// Right click on a knob (§19): Reset, then the shared groups.
pub fn knob_menu(s: &State, k: KnobId, auto: Option<(u32, String, f64)>, x: f32, y: f32) -> Popup {
    let mut items = vec![it("Reset", RCmd::Reset(k))];
    items.extend(value_groups(s, Some(k), auto));
    Popup { x, y, items, sub: None }
}

/// Right click on the mixer box (§12).
pub fn mixer_menu(s: &State, r: usize, x: f32, y: f32) -> Popup {
    let mut items = vec![it("Reset", RCmd::MixerReset(r))];
    items.extend(value_groups(s, None, None));
    items.push(PItem::sep());
    items.push(it("Assign to free mixer track", RCmd::AssignFree(r)));
    Popup { x, y, items, sub: None }
}

/// Right click on a LED (§10).
pub fn led_menu(s: &State, r: usize, x: f32, y: f32) -> Popup {
    let soloed = s.rack.soloed == Some(r);
    let mut items = vec![
        it("Solo", RCmd::Solo(r)).mark(Mark::Check(soloed)),
        it("Lock active/mute state", RCmd::Lock(r)).mark(Mark::Check(s.rack.locked.contains(&r))),
    ];
    items.extend(value_groups(s, None, None));
    Popup { x, y, items, sub: None }
}

/// Group combo (§7): left click lists the filters.
pub fn filter_menu(s: &State, x: f32, y: f32) -> Popup {
    let f = s.rack.filter;
    let mut items = vec![it("All", RCmd::SetFilter(Filter::All)).mark(Mark::Radio(f == Filter::All))];
    items.extend(s.rack.groups.iter().enumerate().map(|(i, g)| it(g.name.clone(), RCmd::SetFilter(Filter::Group(i))).mark(Mark::Radio(f == Filter::Group(i)))));
    items.push(it("Unsorted", RCmd::SetFilter(Filter::Unsorted)).mark(Mark::Radio(f == Filter::Unsorted)));
    Popup { x, y, items, sub: None }
}

/// Group combo right click (§7).
pub fn filter_edit_menu(s: &State, x: f32, y: f32) -> Popup {
    let group = matches!(s.rack.filter, Filter::Group(_));
    let del = it("Delete filter group", RCmd::DeleteGroup);
    let ren = it("Rename filter group", RCmd::RenameGroup);
    Popup { x, y, items: vec![it("Add filter group", RCmd::AddGroup), if group { del } else { del.off() }, if group { ren } else { ren.off() }], sub: None }
}

/// Pattern length box right click (§7).
pub fn length_menu(s: &State, x: f32, y: f32) -> Popup {
    let cur = explicit_len(s);
    let mut items = vec![PItem::head("Set to"), it("Auto", RCmd::SetLen(None)).mark(Mark::Radio(cur.is_none()))];
    for bars in [1u32, 2, 4, 8, 16, 32] {
        let label = if bars == 1 { "1 bar".to_string() } else { format!("{bars} bars") };
        items.push(it(label, RCmd::SetLen(Some(bars * 16))).mark(Mark::Radio(cur == Some(bars * 16))));
    }
    Popup { x, y, items, sub: None }
}

/// Loop button right click (§7).
pub fn loop_menu(s: &State, x: f32, y: f32) -> Popup {
    let t = s.rack.loop_type;
    let on = s.rack.loop_mode;
    let burn = |label: &'static str, all: bool| if on { it(label, RCmd::Burn(all)) } else { it(label, RCmd::Burn(all)).off() };
    let items = vec![
        PItem::head("Loop type"),
        it("Loop step channels", RCmd::LoopType(0)).mark(Mark::Radio(t == 0)),
        it("Loop all channels", RCmd::LoopType(1)).mark(Mark::Radio(t == 1)),
        PItem::not_yet("Advanced looping").mark(Mark::Radio(t == 2)),
        PItem::head("Burn to pattern"),
        burn("Burn selected channels", false),
        burn("Burn all looping channels", true),
        PItem::head("Options"),
        opt(s, "Always show advanced controls"),
    ];
    Popup { x, y, items, sub: None }
}

// ---------------------------------------------------------------------------------------------------------
// Commands

/// Where the rack's name boxes open: over a row's name button, else near the mouse.
fn box_at_row(s: &State, r: usize, mouse: (f32, f32)) -> (f32, f32) {
    crate::views::rack::name_rect(s, r).map_or(crate::patterns::box_at_mouse(mouse), |nb| (nb.x, nb.y))
}

/// A sample or channel preset at `path` went into the existing channel `ch`: while "Auto name channels" is on, the
/// channel takes the file's name without its extension; off, it keeps its name (spec: ui/settings-choices.md
/// "Miscellaneous: naming, detached plugins and startup"). A channel made new for a file is named after it either way.
pub fn auto_name(ch: &mut os_model::Channel, path: &std::path::Path) {
    if crate::prefs::check(&crate::prefs::AUTO_NAME_CHANNELS) {
        ch.name = path.file_stem().map(|n| n.to_string_lossy().into_owned());
    }
}

/// The sample chosen in Load sample...: row `r`'s channel takes it (and its name, `auto_name`).
pub fn sample_chosen(app: &mut App, r: usize, p: std::path::PathBuf) {
    let s = &mut app.state;
    let path = p.to_string_lossy().into_owned();
    if let Some(ch) = s.project.channels.get_mut(r) {
        auto_name(ch, &p);
        ch.sample_path = Some(path.clone());
    }
    s.sample_loads.push((r, path));
    s.piano.dirty = true;
}

/// Run a rack menu command. Returns a status message.
pub fn run(app: &mut App, c: RCmd) -> Option<String> {
    let mouse = app.mouse;
    let sel: Vec<usize> = app.state.rack.sel.iter().copied().collect();
    match c {
        RCmd::Clone(r) => clone_rows(app, &[r]),
        RCmd::CloneSel => clone_rows(app, &sel),
        RCmd::MoveSel(d) => move_selected(app, d),
        RCmd::Sort(by) => sort(app, by),
        RCmd::LoadSample(r) => {
            // Starts in the folder of the channel's sample, when it has one.
            let dir = app.state.project.channels.get(r).and_then(|c| c.sample_path.as_deref()).map(crate::plugins::resolve_sample).map(std::path::PathBuf::from);
            let dlg = crate::filedlg::FileDlg::new(crate::filedlg::Mode::Open, "Load sample", "audio", crate::dialog::Then::RackSample(r))
                .filter("Audio", &["wav", "mp3", "flac", "ogg", "aif", "aiff"])
                .start(dir.as_deref().and_then(std::path::Path::parent));
            app.file_dialog(dlg);
        }
        RCmd::PianoRoll(r) => {
            app.focus_channel(r);
            app.state.raise(Win::PianoRoll);
        }
        _ => {}
    }
    let s = &mut app.state;
    let msg = match c {
        RCmd::Graph(r) => {
            s.selected_channel = r;
            s.rack.select_only(r);
            s.rack.graph_open = true;
            None
        }
        RCmd::Rename(r) => {
            let at = box_at_row(s, r, mouse);
            open_box(s, RBox::Channel(r), at);
            None
        }
        RCmd::ChangeColor(r) => {
            let at = box_at_row(s, r, mouse);
            open_box(s, RBox::Channel(r), at);
            if let Some(b) = s.pat_ui.name_box.as_mut() {
                b.palette = true;
            }
            None
        }
        RCmd::RandomColor(r) => {
            random_color(s, r, r as u64);
            None
        }
        RCmd::Delete(r) => {
            ask_delete(s, vec![r]);
            None
        }
        RCmd::DeleteSel => {
            ask_delete(s, sel.clone());
            None
        }
        RCmd::Cut(r) => {
            s.rack.clipboard = Some(row_notes(s, r));
            set_row_notes(s, r, Vec::new());
            None
        }
        RCmd::Copy(r) => {
            s.rack.clipboard = Some(row_notes(s, r));
            None
        }
        RCmd::Paste(r) => {
            if let Some(n) = s.rack.clipboard.clone() {
                set_row_notes(s, r, n);
            }
            None
        }
        RCmd::Fill(r, n) => {
            fill_each(s, r, n);
            None
        }
        RCmd::AdvancedFill(r) => {
            open_fill(s, r);
            None
        }
        RCmd::Rotate(r, d) => {
            rotate(s, r, d);
            None
        }
        RCmd::Solo(r) => {
            solo(s, r);
            None
        }
        RCmd::Lock(r) => {
            toggle_lock(s, r);
            None
        }
        RCmd::Reset(k) => {
            s.set_knob(k, k.default_value());
            None
        }
        RCmd::CopyValue(k) => {
            s.rack.value_clip = Some(s.knob(k));
            None
        }
        RCmd::PasteValue(k) => {
            if let Some(v) = s.rack.value_clip {
                s.set_knob(k, v);
            }
            None
        }
        RCmd::MixerReset(r) => {
            // The default target: the insert the channel was made with (its own number).
            if let Some(c) = s.project.channels.get_mut(r) {
                c.mixer_track = (c.index + 1).min(crate::mixer::max_insert_of(&s.project.mixer));
                s.piano.dirty = true;
            }
            None
        }
        // spec: ui/mixer-track-commands.md §1
        RCmd::AssignFree(r) => crate::mixer::route_free(s, &[r]),
        RCmd::AssignFreeSel => crate::mixer::route_free(s, &sel),
        RCmd::SelectMixerTracks => {
            if let Some(t) = sel.first().and_then(|&r| s.project.channels.get(r)).map(|c| c.mixer_track) {
                s.selected_mixer = t;
                s.raise(Win::Mixer);
            }
            None
        }
        RCmd::SetFilter(f) => {
            s.rack.filter = f;
            s.rack_scroll = 0.0;
            autosize(s);
            None
        }
        RCmd::AddGroup => {
            open_box(s, RBox::NewGroup, crate::patterns::box_at_mouse(mouse));
            None
        }
        RCmd::DeleteGroup => {
            if let Filter::Group(g) = s.rack.filter {
                if g < s.rack.groups.len() {
                    s.rack.groups.remove(g);
                }
                s.rack.filter = Filter::All;
                autosize(s);
            }
            None
        }
        RCmd::RenameGroup => {
            if let Filter::Group(g) = s.rack.filter {
                open_box(s, RBox::RenameGroup(g), crate::patterns::box_at_mouse(mouse));
            }
            None
        }
        RCmd::GroupSel => {
            open_box(s, RBox::GroupSel, crate::patterns::box_at_mouse(mouse));
            None
        }
        RCmd::SetLen(v) => {
            set_len(s, v);
            None
        }
        RCmd::LoopType(t) => {
            s.rack.loop_type = t;
            None
        }
        RCmd::Burn(all) => {
            burn(s, all);
            None
        }
        RCmd::SelectUnused => {
            s.rack.sel = unused(s);
            Some(format!("{} unused channel(s) selected", s.rack.sel.len()))
        }
        RCmd::Purge => ask_purge(s),
        RCmd::ColorGradient => {
            gradient(s);
            None
        }
        RCmd::ColorRandom => {
            for &r in &sel {
                random_color(s, r, r as u64 + 1);
            }
            None
        }
        RCmd::MuteSel(mute) => {
            for &r in &sel {
                if let Some(c) = s.project.channels.get_mut(r) {
                    c.enabled = !mute;
                }
            }
            s.piano.dirty = true;
            None
        }
        RCmd::TransposeSel => {
            open_box(s, RBox::Transpose, crate::patterns::box_at_mouse(mouse));
            None
        }
        RCmd::ZipSel => {
            s.rack.zipped.extend(sel.iter().copied());
            autosize(s);
            None
        }
        RCmd::UnzipAll => {
            s.rack.zipped.clear();
            autosize(s);
            None
        }
        RCmd::AutoResize(a) => {
            s.rack.auto_resize = a;
            autosize(s);
            None
        }
        RCmd::ResizeNow => {
            let keep = s.rack.auto_resize;
            if keep == AutoResize::Off {
                s.rack.auto_resize = AutoResize::Min;
            }
            autosize(s);
            s.rack.auto_resize = keep;
            None
        }
        RCmd::NameWidth(p) => {
            relayout(s, |s| s.rack.name_pct = p);
            None
        }
        RCmd::SizeToFit => {
            let w = crate::views::rack::fit_name_w(s);
            relayout(s, |s| s.rack.name_pct = ((w / NAME_W * 100.0).ceil() as u32).clamp(40, 400));
            None
        }
        RCmd::AlignLeft(l) => {
            s.rack.align_left = l;
            None
        }
        RCmd::ShowMixer => {
            relayout(s, |s| s.rack.show_mixer = !s.rack.show_mixer);
            None
        }
        RCmd::Colorful => {
            s.rack.colorful = !s.rack.colorful;
            None
        }
        RCmd::Clone(_)
        | RCmd::CloneSel
        | RCmd::MoveSel(_)
        | RCmd::Sort(_)
        | RCmd::LoadSample(_)
        | RCmd::PianoRoll(_) => None,
    };
    app.invalidate();
    msg
}

// ---------------------------------------------------------------------------------------------------------
// Advanced fill dialog

pub fn open_fill(s: &mut State, r: usize) {
    let len = pattern_steps(s).min(64);
    let steps = row_steps(s, r, len);
    let at = (((s.screen.0 - crate::views::rack::FILL_W) / 2.0).round(), ((s.screen.1 - crate::views::rack::FILL_H) / 2.0).round().max(90.0));
    s.rack.fill = Some(FillDlg { row: r, kind: FillType::Manual, len, amount: 4, rotate: 0, steps, at });
}

// ---------------------------------------------------------------------------------------------------------
// Loops (§15)

/// The project as the transport plays it: in loop mode, channels with their own loop length repeat their first
/// steps across the pattern (polyrhythms).
pub fn play_project(s: &State) -> Project {
    let mut p = s.project.clone();
    p.tempo = Some(s.tempo);
    if s.rack.loop_mode && !s.rack.loop_len.is_empty() {
        let rows: Vec<usize> = s.rack.loop_len.keys().copied().collect();
        expand_loops(&mut p, s, &rows);
    }
    p
}

/// Repeat each row's first `loop_len` steps up to the end of every pattern; notes past the loop length go.
fn expand_loops(p: &mut Project, s: &State, rows: &[usize]) {
    let ps = per_step(s);
    let bar = ps * 16;
    for &r in rows {
        let (Some(&len), Some(ch)) = (s.rack.loop_len.get(&r), p.channels.get(r).map(|c| c.index)) else { continue };
        let span = len.max(1) * ps;
        for pat in &mut p.patterns {
            let end = pat.length.filter(|&l| l > 0).unwrap_or_else(|| {
                let last = pat.notes.iter().map(|n| n.position + n.length).max().unwrap_or(0);
                (last / bar + 1) * bar
            });
            let src: Vec<Note> = pat.notes.iter().filter(|n| n.channel == ch && n.position < span).copied().collect();
            if src.is_empty() {
                continue;
            }
            pat.notes.retain(|n| n.channel != ch);
            let mut off = 0;
            while off < end {
                pat.notes.extend(src.iter().filter(|n| n.position + off < end).map(|n| Note { position: n.position + off, ..*n }));
                off += span;
            }
            pat.notes.sort_by_key(|n| (n.position, n.channel));
        }
    }
}

/// Loop button > Burn: write the loops into the current pattern's notes and drop the loop lengths.
fn burn(s: &mut State, all: bool) {
    let rows: Vec<usize> = if all { s.rack.loop_len.keys().copied().collect() } else { s.rack.sel.iter().copied().filter(|r| s.rack.loop_len.contains_key(r)).collect() };
    let mut p = s.project.clone();
    expand_loops(&mut p, s, &rows);
    if let (Some(src), Some(dst)) = (p.pattern(s.pattern).cloned(), s.project.patterns.iter_mut().find(|x| x.number == s.pattern)) {
        dst.notes = src.notes;
    }
    for r in rows {
        s.rack.loop_len.remove(&r);
    }
    s.piano.dirty = true;
}

// ---------------------------------------------------------------------------------------------------------
// Keyboard (§20)

/// Keys while the rack is the front window. Returns whether the key was taken.
pub fn key(app: &mut App, key: &winit::keyboard::Key, ctrl: bool, alt: bool) -> bool {
    use winit::keyboard::{Key, NamedKey};
    let shift = app.shift;
    let s = &mut app.state;
    if s.playlist.popup.is_some() {
        if matches!(key, Key::Named(NamedKey::Escape)) {
            s.playlist.popup = None;
            app.invalidate();
            return true;
        }
        return false;
    }
    if s.rack.fill.is_some() {
        match key {
            Key::Named(NamedKey::Escape) => s.rack.fill = None,
            Key::Named(NamedKey::Enter) => fill_accept(s),
            _ => return false,
        }
        app.invalidate();
        return true;
    }
    let sel: Vec<usize> = s.rack.sel.iter().copied().collect();
    let focus = s.selected_channel;
    let letter = |c: &str, l: &str| c.eq_ignore_ascii_case(l);
    let cmd = match key {
        Key::Named(NamedKey::ArrowUp) if alt => RCmd::MoveSel(-1),
        Key::Named(NamedKey::ArrowDown) if alt => RCmd::MoveSel(1),
        Key::Named(k @ (NamedKey::ArrowUp | NamedKey::ArrowDown)) if !ctrl => {
            let rows = shown(s);
            let cur = rows.iter().position(|&r| Some(&r) == s.rack.sel.iter().next()).or_else(|| rows.iter().position(|&r| r == focus));
            let next = match (cur, k) {
                (Some(i), NamedKey::ArrowUp) => i.saturating_sub(1),
                (Some(i), _) => (i + 1).min(rows.len().saturating_sub(1)),
                (None, _) => 0,
            };
            if let Some(&r) = rows.get(next) {
                s.rack.select_only(r);
                app.focus_channel(r);
            }
            app.invalidate();
            return true;
        }
        Key::Named(k @ (NamedKey::ArrowLeft | NamedKey::ArrowRight)) if ctrl && shift => RCmd::Rotate(focus, if *k == NamedKey::ArrowLeft { -1 } else { 1 }),
        Key::Named(k @ (NamedKey::PageUp | NamedKey::PageDown)) => {
            let mut all = vec![Filter::All];
            all.extend((0..s.rack.groups.len()).map(Filter::Group));
            all.push(Filter::Unsorted);
            let i = all.iter().position(|&f| f == s.rack.filter).unwrap_or(0);
            let j = if *k == NamedKey::PageUp { (i + all.len() - 1) % all.len() } else { (i + 1) % all.len() };
            RCmd::SetFilter(all[j])
        }
        Key::Named(NamedKey::Delete) if alt => RCmd::DeleteSel,
        Key::Named(NamedKey::Escape) => {
            s.hide(Win::ChannelRack);
            app.invalidate();
            return true;
        }
        Key::Character(c) if alt && shift && letter(c, "c") => RCmd::ColorGradient,
        Key::Character(c) if ctrl && alt && letter(c, "c") => RCmd::ColorRandom,
        Key::Character(c) if alt && !ctrl && letter(c, "c") => RCmd::CloneSel,
        Key::Character(c) if alt && !ctrl && letter(c, "g") => RCmd::GroupSel,
        Key::Character(c) if alt && !ctrl && letter(c, "z") => RCmd::ZipSel,
        Key::Character(c) if alt && !ctrl && letter(c, "u") => RCmd::UnzipAll,
        Key::Character(c) if alt && !ctrl && letter(c, "m") => RCmd::ShowMixer,
        Key::Character(c) if ctrl && !alt && letter(c, "l") => RCmd::AssignFreeSel,
        Key::Character(c) if ctrl && !alt && letter(c, "k") => {
            s.rack.graph_open = !s.rack.graph_open;
            app.invalidate();
            return true;
        }
        // (shift / ctrl +) 0..9: mute / solo channels 1..10 when the typing keyboard does not take the digits (§10).
        Key::Character(c) if !alt && !app.typing_on() && c.len() == 1 && c.chars().all(|d| d.is_ascii_digit()) => {
            let d = c.parse::<usize>().unwrap_or(0);
            let r = if d == 0 { 9 } else { d - 1 };
            let s = &mut app.state;
            if r < s.project.channels.len() {
                if ctrl { solo(s, r) } else { toggle_mute(s, r) }
            }
            app.invalidate();
            return true;
        }
        _ => return false,
    };
    let _ = sel;
    if let Some(m) = run(app, cmd) {
        app.state.message = Some(m);
    }
    true
}

/// Accept in the Advanced fill dialog: write the sequence to the channel.
pub fn fill_accept(s: &mut State) {
    if let Some(f) = s.rack.fill.take() {
        write_sequence(s, f.row, &f.steps);
    }
}

/// Clicks in the Advanced fill dialog (§22).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FillHit {
    Close,
    /// Fill type combo: Manual -> Fill every -> Euclidean.
    Type,
    Len(i32),
    Amount(i32),
    Rotate(i32),
    Dot(usize),
    Random,
    Reset,
    Accept,
    Inside,
}

pub fn fill_hit(s: &mut State, h: FillHit) {
    match h {
        FillHit::Accept => return fill_accept(s),
        FillHit::Close => {
            s.rack.fill = None;
            return;
        }
        _ => {}
    }
    let Some(f) = s.rack.fill.as_mut() else { return };
    match h {
        FillHit::Type => {
            f.kind = match f.kind {
                FillType::Manual => FillType::Every,
                FillType::Every => FillType::Euclid,
                FillType::Euclid => FillType::Manual,
            };
            f.amount = f.amount.clamp(1, f.len.max(1));
        }
        FillHit::Len(d) => {
            f.len = (f.len as i32 + d).clamp(1, 64) as u32;
            f.amount = f.amount.clamp(1, f.len);
        }
        FillHit::Amount(d) => f.amount = (f.amount as i32 + d).clamp(1, f.len.max(1) as i32) as u32,
        FillHit::Rotate(d) => {
            let n = f.len.max(1) as i32;
            if f.kind == FillType::Manual {
                let k = d.rem_euclid(n) as usize;
                { let n = f.steps.len(); f.steps.rotate_right(k.min(n)); }
            } else {
                f.rotate = (f.rotate as i32 + d).rem_euclid(n) as u32;
            }
        }
        FillHit::Dot(k) => {
            if let Some(b) = f.steps.get_mut(k) {
                *b = !*b;
            }
            f.kind = FillType::Manual;
            f.rotate = 0;
        }
        FillHit::Random => {
            let mut x = seed() | 1;
            for b in f.steps.iter_mut() {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                *b = x % 100 < 35;
            }
            f.kind = FillType::Manual;
            f.rotate = 0;
        }
        FillHit::Reset => {
            f.steps.iter_mut().for_each(|b| *b = false);
            f.kind = FillType::Manual;
            f.rotate = 0;
        }
        FillHit::Accept | FillHit::Close | FillHit::Inside => {}
    }
    f.rebuild();
}

/// Graph editor (§14): set step `k`'s value of the focused channel from 0..1; a column without a step edits the
/// nearest step note to its left.
pub fn graph_set(s: &mut State, k: u32, v: f32) -> Option<String> {
    let r = s.selected_channel;
    let ch = ch_index(s, r)?;
    let ps = per_step(s);
    let tab = s.rack.graph_tab;
    let n = s.pattern.max(1);
    let p = crate::patterns::pat_mut(s, n);
    let idx = p
        .notes
        .iter()
        .enumerate()
        .filter(|(_, x)| x.channel == ch && x.position / ps <= k)
        .max_by_key(|(_, x)| x.position)
        .map(|(i, _)| i)?;
    let note = &mut p.notes[idx];
    tab.set(note, v, ps);
    let text = tab.text(note, ps);
    if tab == Tab::Shift {
        p.notes.sort_by_key(|x| (x.position, x.channel));
    }
    s.piano.dirty = true;
    Some(text)
}

// ---------------------------------------------------------------------------------------------------------
// Saving (spec: ui/channel-rack.md "Saving"; flp/events/channels.md, project-info.md, patterns.md)

/// Loop type values of event 162 for the rack's loop types (Loop step channels, Loop all channels, Advanced
/// looping). Advanced looping was not measured; it is written like step channels.
const LOOP_TYPE_CODES: [u32; 3] = [3, 2, 3];

/// The rack state of an opened (or new) project: lock, zip, filter groups and the current filter from the channels,
/// loop lengths and loop mode from the current pattern.
pub fn from_model(s: &mut State) {
    let p = &s.project;
    let rack = &mut s.rack;
    rack.locked = p.channels.iter().enumerate().filter(|(_, c)| c.rack.locked).map(|(r, _)| r).collect();
    rack.zipped = p.channels.iter().enumerate().filter(|(_, c)| c.rack.zipped).map(|(r, _)| r).collect();
    rack.groups.clear();
    rack.unsorted_at = None;
    let mut file_to_ui = Vec::new();
    for (i, name) in p.groups.iter().enumerate() {
        if name == os_model::UNSORTED_GROUP && rack.unsorted_at.is_none() {
            rack.unsorted_at = Some(i);
            file_to_ui.push(None);
            continue;
        }
        let members = p.channels.iter().enumerate().filter(|(_, c)| c.rack.group as usize == i).map(|(r, _)| r).collect();
        file_to_ui.push(Some(rack.groups.len()));
        rack.groups.push(Group { name: name.clone(), members });
    }
    rack.filter = match p.current_group.map(|g| g as usize) {
        None => Filter::All,
        Some(g) => match file_to_ui.get(g) {
            Some(Some(u)) => Filter::Group(*u),
            Some(None) => Filter::Unsorted,
            None => Filter::All,
        },
    };
    let pat = p.pattern(s.pattern);
    let loops = pat.map(|p| p.loops.clone()).unwrap_or_default();
    rack.loop_len = loops
        .iter()
        // 0 = no length (FL writes it for the other channels while loop mode is off).
        .filter(|&&(_, steps)| steps != 0 && steps != os_model::Pattern::LOOP_FOLLOW)
        .filter_map(|&(ch, steps)| p.channels.iter().position(|c| c.index == ch).map(|r| (r, steps)))
        .collect();
    let all = loops.iter().any(|&(_, steps)| steps == os_model::Pattern::LOOP_FOLLOW);
    let off = pat.and_then(|p| p.loop_off);
    rack.loop_type = match off {
        Some(2) => 1,
        Some(_) => 0,
        None if all => 1,
        None => 0,
    };
    rack.loop_mode = !loops.is_empty() && off.is_none();
}

/// Writes the rack state into the project before a save (the inverse of [`from_model`]).
pub fn to_model(s: &mut State) {
    let rack = &s.rack;
    let p = &mut s.project;
    // Groups in file order: the user groups, with "Unsorted" where the file had it (else last, like FL's Alt+G).
    let mut names: Vec<String> = rack.groups.iter().map(|g| g.name.clone()).collect();
    let unsorted = rack.unsorted_at.unwrap_or(names.len()).min(names.len());
    names.insert(unsorted, os_model::UNSORTED_GROUP.to_string());
    let file_index = |g: usize| if g < unsorted { g } else { g + 1 } as u32;
    for (r, c) in p.channels.iter_mut().enumerate() {
        c.rack.locked = rack.locked.contains(&r);
        c.rack.zipped = rack.zipped.contains(&r);
        // A channel is in one group in the file: its first one.
        c.rack.group = rack.groups.iter().position(|g| g.members.contains(&r)).map_or(unsorted as u32, file_index);
    }
    // A file without groups keeps none when nothing was grouped.
    if !(p.groups.is_empty() && rack.groups.is_empty()) {
        p.groups = names;
    }
    p.current_group = match rack.filter {
        Filter::All => None,
        Filter::Group(g) => Some(file_index(g)),
        Filter::Unsorted => Some(unsorted as u32),
    };
    if p.groups.is_empty() {
        p.current_group = None;
    }
    // Loop lengths belong to the current pattern.
    let mut loops: Vec<(u16, u32)> =
        rack.loop_len.iter().filter_map(|(&r, &l)| p.channels.get(r).map(|c| (c.index, l))).collect();
    if rack.loop_type == 1 && !loops.is_empty() {
        let set: BTreeSet<u16> = loops.iter().map(|l| l.0).collect();
        loops.extend(p.channels.iter().filter(|c| !set.contains(&c.index)).map(|c| (c.index, os_model::Pattern::LOOP_FOLLOW)));
    }
    loops.sort_by_key(|l| l.0);
    let loop_off = (!rack.loop_mode && !loops.is_empty()).then(|| LOOP_TYPE_CODES[usize::from(rack.loop_type.min(2))]);
    let n = s.pattern;
    // Pairs with no length (0) carry nothing: a file that has them is unchanged.
    let set = |l: &[(u16, u32)]| l.iter().filter(|x| x.1 != 0).copied().collect::<BTreeSet<_>>();
    let changed = p.pattern(n).map_or(!loops.is_empty(), |pat| set(&pat.loops) != set(&loops) || pat.loop_off != loop_off);
    if changed {
        let pat = crate::patterns::pat_mut(s, n);
        pat.loops = loops;
        pat.loop_off = loop_off;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A new project's channel name, "Lemon Sampler", fits the 100 % name button at every UI scale: the label keeps
    /// 4 px on each side (§17). Runs with the platform's text engine (GDI's Cuprum on Windows, Barlow on Linux).
    #[test]
    fn default_name_button_shows_lemon_sampler_whole() {
        let app = App::new(None);
        let w = crate::views::rack::cols(&app.state).name_w;
        assert_eq!(w, NAME_W);
        for scale in [1.0, 1.25, 1.5, 1.75, 2.0] {
            let mut c = os_ui::Canvas::new(64, 64, scale).expect("canvas");
            let text = c.text_width(os_ui::Font::new(14), crate::plugins::LEMON_SAMPLER);
            assert!(text <= w - 8.0, "{text} px at scale {scale}");
        }
    }

    /// Clone, move and delete renumber the channels' notes so each note keeps its channel (§17-18).
    #[test]
    fn restructuring_keeps_notes_with_their_channel() {
        let mut app = App::new(None);
        let n = app.state.project.channels.len();
        assert!(n >= 3);
        let name = |app: &App, i: usize| app.state.project.channels[i].name.clone();
        let (n1, n2) = (name(&app, 1), name(&app, 2));
        app.state.project.patterns[0].notes = vec![os_model::Note::step(2, 0)];
        clone_rows(&mut app, &[2]);
        assert_eq!(app.state.project.channels.len(), n + 1);
        assert_eq!(app.state.project.channels[3].rack.origin, app.state.project.channels[2].rack.origin, "a clone keeps its source block");
        app.state.rack.select_only(2);
        move_selected(&mut app, -1);
        assert_eq!(name(&app, 1), n2);
        assert!(app.state.project.patterns[0].notes.iter().any(|x| x.channel == 1), "the note moved with its channel");
        delete_rows(&mut app, &[1]);
        assert_eq!(app.state.project.channels.len(), n);
        assert_eq!(name(&app, 1), n1);
        assert!(app.state.project.patterns[0].notes.iter().all(|x| usize::from(x.channel) < n));
    }

    /// TOOLS > Select unused channels and Purge unused audio clips (spec: ui/unused-channels.md): unused = no notes in any
    /// pattern and no playlist clip; purge takes only the unused audio-clip channels, after the delete confirmation.
    #[test]
    fn select_unused_and_purge_audio_clips() {
        use crate::views::menus::{item_at, works, TOOLS_MENU};
        use os_model::Clip;
        use winit::keyboard::{Key, NamedKey};
        let mut app = App::new(None);
        // Rows: 0 sampler with a note, 1 sampler unused, 2 audio clip with a note only, 3 audio clip with a playlist
        // clip, 4 audio clip unused.
        for r in 2..5 {
            app.state.project.channels[r].kind = ChannelKind::AudioClip;
        }
        let idx = |app: &App, r: usize| app.state.project.channels[r].index;
        let (i0, i2, i3) = (idx(&app, 0), idx(&app, 2), idx(&app, 3));
        crate::patterns::pat_mut(&mut app.state, 2).notes = vec![Note::step(i0, 0), Note::step(i2, 3)];
        crate::playlist::arr_mut(&mut app.state).clips.push(Clip::new(0, 384, 0, ClipSource::Channel(i3)));
        let tools = |label: &str| (0..80).find(|&i| item_at(TOOLS_MENU, i).is_some_and(|it| it.label == label)).unwrap();
        for label in ["Select unused channels", "Purge unused audio clips"] {
            assert!(works(TOOLS_MENU, item_at(TOOLS_MENU, tools(label)).unwrap()), "{label} is not greyed out");
        }

        app.menu_item(TOOLS_MENU, tools("Select unused channels"));
        assert_eq!(app.state.rack.sel, BTreeSet::from([1, 4]));
        assert_eq!(app.state.message.as_deref(), Some("2 unused channel(s) selected"));

        // Purge asks first (no undo for channels yet); Cancel keeps everything.
        let names = |app: &App| app.state.project.channels.iter().map(|c| c.name.clone().unwrap_or_default()).collect::<Vec<_>>();
        let before = names(&app);
        app.menu_item(TOOLS_MENU, tools("Purge unused audio clips"));
        assert_eq!(app.state.rack.pending_delete, Some(vec![4]));
        assert!(app.state.rack.purging && app.state.pat_ui.confirm);
        app.key(&Key::Named(NamedKey::Escape), false, false);
        assert_eq!(names(&app), before);
        // Ok deletes only the unused audio-clip channel.
        assert_eq!(run(&mut app, RCmd::Purge), None);
        app.key(&Key::Named(NamedKey::Enter), false, false);
        assert_eq!(names(&app), before[..4].to_vec());
        assert!(!app.state.pat_ui.confirm);
        // Nothing left to purge: said, not asked.
        assert_eq!(run(&mut app, RCmd::Purge).as_deref(), Some("No unused audio clips"));
        assert!(!app.state.pat_ui.confirm);
        // The usual delete confirmation is worded as before.
        ask_delete(&mut app.state, vec![1]);
        assert!(!app.state.rack.purging);
    }

    /// Lock, zip, groups, swing, loop lengths and a restructured rack survive saving into an opened file ("Saving").
    #[test]
    fn rack_settings_save_and_reopen() {
        let path = std::env::temp_dir().join(format!("os_rack_save_{}.flp", std::process::id()));
        let mut app = App::new(None);
        crate::patterns::pat_mut(&mut app.state, 1).notes = vec![os_model::Note::step(2, 0), os_model::Note::step(0, 1)];
        app.save_to(path.clone());
        app.open(path.clone());
        let n = app.state.project.channels.len();
        let names: Vec<Option<String>> = app.state.project.channels.iter().map(|c| c.name.clone()).collect();
        // Clone channel 0, delete channel 1, then set the rack state on the new rows.
        clone_rows(&mut app, &[0]);
        delete_rows(&mut app, &[2]);
        let s = &mut app.state;
        s.project.swing = Some(64);
        s.rack.locked = [1].into();
        s.rack.zipped = [2].into();
        s.rack.groups = vec![Group { name: "Drums".into(), members: [0, 2].into() }];
        s.rack.filter = Filter::Group(0);
        s.rack.loop_len = [(0, 5)].into();
        s.rack.loop_mode = true;
        app.save_to(path.clone());
        assert!(app.state.message.as_deref().is_some_and(|m| m.starts_with("Saved")), "{:?}", app.state.message);
        app.open(path.clone());
        let _ = std::fs::remove_file(&path);
        let s = &app.state;
        let p = &s.project;
        assert_eq!(p.channels.len(), n);
        assert_eq!(p.channels[0].name, names[0]);
        assert_eq!(p.channels[2].name, names[2]);
        assert!(p.channels.iter().enumerate().all(|(i, c)| usize::from(c.index) == i));
        assert_eq!(p.swing, Some(64));
        let mut notes: Vec<u16> = p.patterns[0].notes.iter().map(|x| x.channel).collect();
        notes.sort_unstable();
        assert_eq!(notes, vec![0, 2], "the clone has no notes; channel 2 moved back to row 2");
        assert_eq!(s.rack.locked, [1].into());
        assert_eq!(s.rack.zipped, [2].into());
        assert_eq!(s.rack.groups.len(), 1);
        assert_eq!(s.rack.groups[0].members, [0, 2].into());
        assert_eq!(s.rack.filter, Filter::Group(0));
        assert_eq!(s.rack.loop_len, [(0, 5)].into());
        assert!(s.rack.loop_mode);
    }

    #[test]
    fn euclid_spreads_hits() {
        let e = euclid(16, 4);
        assert_eq!(e.iter().filter(|&&b| b).count(), 4);
        assert!(e[0] && e[4] && e[8] && e[12]);
        let e = euclid(8, 3);
        assert_eq!(e.iter().filter(|&&b| b).count(), 3);
        assert!(e[0]);
    }

    #[test]
    fn graph_values_round_trip() {
        let mut n = Note::step(0, 96);
        for t in Tab::ALL {
            t.set(&mut n, 0.5, 24);
            assert!((t.get(&n, 24) - 0.5).abs() < 0.08, "{t:?}: {}", t.get(&n, 24));
        }
        assert_eq!(n.position / 24, 4, "time shift stays inside its step");
        assert_eq!(Tab::Vel.text(&Note::step(0, 0), 24), "Velocity 78%");
        assert_eq!(key_name(60), "C5");
    }

    /// "Restore previous state after solo" (spec: ui/settings-choices.md "Miscellaneous"), as measured in the channel
    /// rack, the playlist and the mixer: mute B, solo A (the second time moving the solo on to C), unsolo. Off:
    /// everything on; on: the states from before the first solo (B muted). A locked channel keeps its state.
    #[test]
    fn restore_previous_state_after_solo() {
        use crate::prefs::{set_check, RESTORE_AFTER_SOLO};
        for on in [false, true] {
            set_check(&RESTORE_AFTER_SOLO, on);
            for moved in [false, true] {
                let what = format!("check {}, solo {}", if on { "on" } else { "off" }, if moved { "moved" } else { "kept" });
                let (a, b, c) = (0, 1, 2);
                let want = |i: usize| !(on && i == b);
                let mut app = App::new(None);
                app.state.project = crate::app::test_project();
                let s = &mut app.state;

                // Channel rack (Ctrl+click on the LEDs); channel 5 is locked and muted.
                toggle_mute(s, b);
                toggle_mute(s, 4);
                toggle_lock(s, 4);
                solo(s, a);
                assert!(s.project.channels.iter().enumerate().all(|(i, ch)| ch.enabled == (i == a)), "rack solo, {what}");
                if moved {
                    solo(s, c);
                    assert!(s.project.channels.iter().enumerate().all(|(i, ch)| ch.enabled == (i == c)), "rack solo moved, {what}");
                }
                solo(s, if moved { c } else { a });
                let got: Vec<bool> = s.project.channels.iter().map(|ch| ch.enabled).collect();
                assert_eq!(got, (0..5).map(|i| i != 4 && want(i)).collect::<Vec<_>>(), "rack, {what}");

                // Playlist tracks.
                let tracks = |s: &mut State| crate::playlist::arr_mut(s).tracks.iter().map(|t| t.enabled).collect::<Vec<bool>>();
                crate::playlist::toggle_track(s, b as u16);
                crate::playlist::toggle_solo(s, a as u16);
                if moved {
                    crate::playlist::toggle_solo(s, c as u16);
                    assert!(tracks(s).iter().enumerate().all(|(i, &on)| on == (i == c)), "playlist solo moved, {what}");
                }
                crate::playlist::toggle_solo(s, if moved { c } else { a } as u16);
                assert!(tracks(s).iter().enumerate().all(|(i, &on)| on == want(i)), "playlist, {what}");

                // Mixer inserts (right-click on the LEDs): inserts 1, 2 and 3 are A, B and C.
                crate::mixer::track_mut(s, 2).expect("insert 2").enabled = false;
                crate::mixer::solo(s, 1);
                if moved {
                    crate::mixer::solo(s, 3);
                    assert!(s.project.mixer.iter().all(|m| m.enabled == matches!(m.index, 0 | 3)), "mixer solo moved, {what}");
                }
                crate::mixer::solo(s, if moved { 3 } else { 1 });
                assert!(s.project.mixer.iter().all(|m| m.enabled == (!on || m.index != 2)), "mixer, {what}");
            }
        }
        set_check(&RESTORE_AFTER_SOLO, false);
    }

    /// "Select first channel with notes when the pattern changes" (spec: ui/settings-choices.md "Miscellaneous"): on,
    /// switching pattern selects (only) the lowest-numbered channel with notes or steps in it; a pattern without any
    /// leaves the selection alone. Off: the selection stays.
    #[test]
    fn select_first_channel_with_notes_when_the_pattern_changes() {
        use crate::prefs::{set_check, SELECT_FIRST_CHANNEL};
        for on in [false, true] {
            set_check(&SELECT_FIRST_CHANNEL, on);
            let mut app = App::new(None);
            app.state.project = crate::app::test_project();
            // Pattern 2: steps on channels 4 and 2 (indices 3 and 1); pattern 3 has none.
            let notes = vec![Note::step(3, 0), Note::step(1, 96)];
            app.state.project.patterns.push(os_model::Pattern { number: 2, notes, ..Default::default() });
            app.state.project.patterns.push(os_model::Pattern { number: 3, ..Default::default() });
            let select = |app: &mut App, r: usize| {
                app.state.rack.select_only(r);
                app.state.selected_channel = r;
            };
            let selection = |app: &App| (app.state.rack.sel.iter().copied().collect::<Vec<_>>(), app.state.selected_channel);
            select(&mut app, 4);
            app.tick();
            app.state.pattern = 2;
            app.tick();
            assert_eq!(selection(&app), if on { (vec![1], 1) } else { (vec![4], 4) }, "check {on}");
            select(&mut app, 4);
            app.state.pattern = 3;
            app.tick();
            assert_eq!(selection(&app), (vec![4], 4), "no notes: the selection stays (check {on})");
            // The frame's damage brings it up to date too (an input's paint can come before the tick).
            app.state.pattern = 2;
            let _ = app.take_damage();
            assert_eq!(selection(&app).1, if on { 1 } else { 4 }, "before the frame (check {on})");
        }
        set_check(&SELECT_FIRST_CHANNEL, false);
    }

    /// "Auto zip empty channels" (spec: ui/settings-choices.md "Miscellaneous: seven more checks, Default template and
    /// Startup project"), the measured case: steps on Kick and HiHat in pattern 1 and on Clap in pattern 2. On, each
    /// pattern switch unzips the channels with notes or steps in the new pattern and zips the others; an empty pattern
    /// unzips them all; zipping by hand holds until the next switch. Off: the zips stay as they are.
    #[test]
    fn auto_zip_empty_channels() {
        use crate::prefs::{set_check, AUTO_ZIP_EMPTY};
        for on in [false, true] {
            set_check(&AUTO_ZIP_EMPTY, on);
            let mut app = App::new(None);
            app.state.project = crate::app::test_project();
            // Rows: 0 Kick, 1 Clap, 2 Hat, 3 Snare, 4 Pluck.
            let pat = |number: u16, notes: Vec<Note>| os_model::Pattern { number, notes, ..Default::default() };
            app.state.project.patterns = vec![pat(1, vec![Note::step(0, 0), Note::step(2, 48)]), pat(2, vec![Note::step(1, 96)]), pat(3, Vec::new())];
            let zips = |app: &App| app.state.rack.zipped.iter().copied().collect::<Vec<usize>>();
            app.state.rack.zipped = [4].into();
            app.tick();
            assert_eq!(zips(&app), vec![4], "no switch yet, check {on}");
            let mut switch = |app: &mut App, p: u16| {
                app.state.pattern = p;
                app.tick();
                zips(app)
            };
            assert_eq!(switch(&mut app, 2), if on { vec![0, 2, 3, 4] } else { vec![4] }, "pattern 2, check {on}");
            assert_eq!(switch(&mut app, 1), if on { vec![1, 3, 4] } else { vec![4] }, "pattern 1, check {on}");
            // Zipping by hand works between switches.
            app.state.rack.zipped.insert(0);
            app.tick();
            assert_eq!(zips(&app), if on { vec![0, 1, 3, 4] } else { vec![0, 4] }, "zipped by hand, check {on}");
            // A pattern without notes (as one just made with "+") unzips every channel.
            assert_eq!(switch(&mut app, 3), if on { Vec::new() } else { vec![0, 4] }, "empty pattern, check {on}");
            assert_eq!(switch(&mut app, 9), if on { Vec::new() } else { vec![0, 4] }, "a pattern with no data, check {on}");
        }
        set_check(&AUTO_ZIP_EMPTY, false);
    }

    /// The rack shown with row 0's steps `on` (the rest off), painted so its step buttons can be hit.
    fn step_app(c: &mut os_ui::Canvas, on: &[u32]) -> App {
        let mut app = App::new(None);
        app.state.z = vec![crate::app::Win::ChannelRack];
        let ch = ch_index(&app.state, 0).expect("row 0");
        let ps = per_step(&app.state);
        let n = app.state.pattern;
        crate::patterns::pat_mut(&mut app.state, n).notes = on.iter().map(|&k| Note::step(ch, k * ps)).collect();
        app.paint(c, None);
        app
    }

    /// Row 0's steps that are on among the first 16.
    fn steps(app: &App) -> Vec<u32> {
        let (ch, ps) = (ch_index(&app.state, 0).expect("row 0"), per_step(&app.state));
        let notes = app.state.project.pattern(app.state.pattern).map(|p| p.notes.clone()).unwrap_or_default();
        (0..16).filter(|&k| step_on(&notes, ch, k, ps)).collect()
    }

    /// Presses the left or `right` button on row 0's step `k`, moves over to step `to`, and releases.
    fn press(app: &mut App, right: bool, k: u32, to: u32) {
        let at = |app: &App, k: u32| {
            let r = app.hits.iter().rev().find(|h| h.action == crate::app::Action::Step(0, k)).map(|h| h.rect).expect("step shown");
            (r.cx(), r.cy())
        };
        let (x, y) = at(app, k);
        app.mouse_move(x, y);
        if right {
            app.mouse_down_right();
        } else {
            app.mouse_down();
        }
        let (x, y) = at(app, to);
        app.mouse_move(x, y);
        app.mouse_up();
    }

    /// "Use both mouse buttons in step sequencer", off as found (spec: ui/settings-choices.md "Input: Click-and-hold
    /// and both mouse buttons (2026-10-16)", the table's Off column): a left or a right click toggles the step; a left
    /// drag sets each step it crosses on, a right drag sets each one off.
    #[test]
    fn step_buttons_with_both_mouse_buttons_off() {
        use crate::prefs::{check, BOTH_MOUSE_BUTTONS};
        let Ok(mut c) = os_ui::Canvas::new(1920, 1080, 1.0) else { return };
        assert!(!check(&BOTH_MOUSE_BUTTONS), "off by default");
        for (right, start, want, row) in [
            (false, vec![], vec![2], "left click on an off step: on"),
            (false, vec![2], vec![], "left click on an on step: off"),
            (true, vec![2], vec![], "right click on an on step: off"),
            (true, vec![], vec![2], "right click on an off step: on"),
        ] {
            let mut app = step_app(&mut c, &start);
            press(&mut app, right, 2, 2);
            assert_eq!(steps(&app), want, "{row}");
            assert_eq!(app.state.selected_channel, 0, "{row}: the row is selected");
        }
        // Drags, from an off and from an on step: left sets every step crossed, right clears them.
        let mut app = step_app(&mut c, &[5, 9]);
        press(&mut app, false, 3, 7);
        assert_eq!(steps(&app), vec![3, 4, 5, 6, 7, 9], "left drag along the row");
        let mut app = step_app(&mut c, &[2, 3, 5, 6, 7, 9]);
        press(&mut app, true, 2, 6);
        assert_eq!(steps(&app), vec![7, 9], "right drag along the row");
        let mut app = step_app(&mut c, &[4, 9]);
        press(&mut app, false, 4, 6);
        assert_eq!(steps(&app), vec![4, 5, 6, 9], "left drag from an on step");
        let mut app = step_app(&mut c, &[9]);
        press(&mut app, true, 4, 6);
        assert_eq!(steps(&app), vec![9], "right drag from an off step");
    }

    /// The check on (spec, as above, the On column): the left button only sets a step, the right one only clears it;
    /// drags as with it off (the spec's choice for Lemon Studio).
    #[test]
    fn step_buttons_with_both_mouse_buttons_on() {
        use crate::prefs::{set_check, BOTH_MOUSE_BUTTONS};
        let Ok(mut c) = os_ui::Canvas::new(1920, 1080, 1.0) else { return };
        set_check(&BOTH_MOUSE_BUTTONS, true);
        for (right, start, want, row) in [
            (false, vec![], vec![2], "left click on an off step: on"),
            (false, vec![2], vec![2], "left click on an on step: stays on"),
            (true, vec![2], vec![], "right click on an on step: off"),
            (true, vec![], vec![], "right click on an off step: stays off"),
        ] {
            let mut app = step_app(&mut c, &start);
            press(&mut app, right, 2, 2);
            assert_eq!(steps(&app), want, "{row}");
        }
        let mut app = step_app(&mut c, &[5, 9]);
        press(&mut app, false, 3, 7);
        assert_eq!(steps(&app), vec![3, 4, 5, 6, 7, 9], "left drag along the row");
        let mut app = step_app(&mut c, &[2, 3, 5, 6, 7, 9]);
        press(&mut app, true, 2, 6);
        assert_eq!(steps(&app), vec![7, 9], "right drag along the row");
        // Back the other way: a drag to the left covers the same steps.
        let mut app = step_app(&mut c, &[]);
        press(&mut app, false, 7, 4);
        assert_eq!(steps(&app), vec![4, 5, 6, 7], "left drag leftwards");
        set_check(&BOTH_MOUSE_BUTTONS, false);
    }
}
