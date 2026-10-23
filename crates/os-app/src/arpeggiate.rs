// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The piano roll's Arpeggiate dialog ("Piano roll - arpeggiator"): its settings, the pattern files (`.fsc`), the
//! arpeggiator itself and the dialog's preview session. The window is `views/arpeggiate.rs`.
// spec: ui/piano-arpeggiate.md

use std::cmp::Ordering;
use std::path::{Path, PathBuf};

use os_model::Note;

use crate::app::State;
use crate::piano;
use crate::playlist::{Cmd, Mark, PItem};

/// Pattern and Range directions (§5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Order {
    #[default]
    Normal,
    Flip,
    Alternate,
}

impl Order {
    pub const ALL: [Order; 3] = [Order::Normal, Order::Flip, Order::Alternate];

    pub fn name(self) -> &'static str {
        match self {
            Order::Normal => "Normal",
            Order::Flip => "Flip",
            Order::Alternate => "Alternate",
        }
    }
}

/// Where the pattern restarts (§4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Sync {
    #[default]
    Time,
    Block,
    Chord,
}

impl Sync {
    pub const ALL: [Sync; 3] = [Sync::Time, Sync::Block, Sync::Chord];

    pub fn name(self) -> &'static str {
        match self {
            Sync::Time => "Time",
            Sync::Block => "Block",
            Sync::Chord => "Chord",
        }
    }
}

/// Time mul's seven values (§1) as (numerator, denominator); index 3 is ×1.
pub const TIME_MUL: [(u32, u32); 7] = [(1, 8), (1, 4), (1, 2), (1, 1), (2, 1), (3, 1), (4, 1)];
pub(crate) const TIME_MUL_NAMES: [&str; 7] = ["1/8", "1/4", "1/2", "x1", "x2", "x3", "x4"];

/// The highest Range; 0 is `---`.
pub const RANGE_MAX: u8 = 6;

/// The Levels knobs (§7): label, hint, neutral row value, highest value.
pub const LEVELS: [(&str, &str, u8, u8); 6] = [
    ("PAN", "Note panning strength", 64, 128),
    ("VEL", "Note velocity strength", 100, 128),
    ("REL", "Note release strength", 64, 128),
    ("MOD X", "Note filter cutoff frequency strength", 128, 255),
    ("MOD Y", "Note filter resonance (Q) strength", 128, 255),
    ("PITCH", "Note fine pitch strength", 120, 240),
];

/// The dialog's controls (§1), Reset's values by default.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Settings {
    pub order: Order,
    /// Index into `TIME_MUL`.
    pub time_mul: u8,
    /// 0 = `---`, else 1..=6 octaves.
    pub range: u8,
    pub range_order: Order,
    pub sync: Sync,
    /// 0..=1 of the pattern note length.
    pub gate: f32,
    /// 0..=1, in `LEVELS` order.
    pub levels: [f32; 6],
    pub group: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { order: Order::Normal, time_mul: 3, range: 1, range_order: Order::Normal, sync: Sync::Time, gate: 1.0, levels: [1.0; 6], group: true }
    }
}

/// The dialog's knobs (Range is a spin box, driven like a knob).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Knob {
    TimeMul,
    Range,
    Gate,
    /// Index into `LEVELS`.
    Level(u8),
}

impl Knob {
    /// Value change per wheel notch: the stepped controls move one value.
    pub fn notch(self) -> f32 {
        match self {
            Knob::TimeMul => 1.0 / 6.0,
            Knob::Range => 1.0 / f32::from(RANGE_MAX),
            Knob::Gate | Knob::Level(_) => 1.0 / 32.0,
        }
    }
}

impl Settings {
    /// A knob as 0..=1.
    pub fn get(&self, k: Knob) -> f32 {
        match k {
            Knob::TimeMul => f32::from(self.time_mul) / 6.0,
            Knob::Range => f32::from(self.range) / f32::from(RANGE_MAX),
            Knob::Gate => self.gate,
            Knob::Level(i) => self.levels[usize::from(i.min(5))],
        }
    }

    pub fn set(&mut self, k: Knob, v: f32) {
        let v = v.clamp(0.0, 1.0);
        match k {
            Knob::TimeMul => self.time_mul = (v * 6.0).round() as u8,
            Knob::Range => self.range = (v * f32::from(RANGE_MAX)).round() as u8,
            Knob::Gate => self.gate = v,
            Knob::Level(i) => self.levels[usize::from(i.min(5))] = v,
        }
    }

    /// The hint-bar text (FL shows none; §9).
    pub fn text(&self, k: Knob) -> String {
        match k {
            Knob::TimeMul => format!("Time multiplicator: {}", TIME_MUL_NAMES[usize::from(self.time_mul.min(6))]),
            Knob::Range => format!("Range: {}", self.range_text()),
            Knob::Gate => format!("Gate: {:.0}%", self.gate * 100.0),
            Knob::Level(i) => format!("{}: {:.0}%", LEVELS[usize::from(i.min(5))].1, self.levels[usize::from(i.min(5))] * 100.0),
        }
    }

    pub fn range_text(&self) -> String {
        if self.range == 0 { "---".into() } else { self.range.to_string() }
    }
}

/// The dialog's menus (§1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Menu {
    Pattern,
    Order,
    RangeOrder,
    Sync,
}

/// The dialog's buttons and switches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Act {
    Group,
    Reset,
    Accept,
    /// The folder button: a file dialog for any pattern file.
    Folder,
    Menu(Menu),
}

/// A menu entry's command.
#[derive(Debug, Clone, PartialEq)]
pub enum ACmd {
    /// A pattern file; None for the built-in Default.
    Pattern(Option<PathBuf>),
    Order(Order),
    RangeOrder(Order),
    Sync(Sync),
}

// ---------------------------------------------------------------- pattern files (§2)

/// A pattern row (a note of the pattern file); colours in today's numbering.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Row {
    pub pos: u32,
    pub len: u32,
    pub key: u16,
    pub colour: u8,
    pub slide: bool,
    pub fine: u8,
    pub rel: u8,
    pub pan: u8,
    pub vel: u8,
    pub mod_x: u8,
    pub mod_y: u8,
}

/// Yellow rows: the length marker (§2).
const MARKER: u8 = 12;
/// Pink rows: no range octave.
const NO_RANGE: u8 = 8;

impl Row {
    fn plain(pos: u32, len: u32, key: u16) -> Row {
        Row { pos, len, key, colour: 0, slide: false, fine: 120, rel: 64, pan: 64, vel: 100, mod_x: 128, mod_y: 128 }
    }

    pub(crate) fn level(&self, i: usize) -> u8 {
        [self.pan, self.vel, self.rel, self.mod_x, self.mod_y, self.fine][i]
    }
}

/// A loaded pattern.
#[derive(Debug, Clone, PartialEq)]
pub struct Pattern {
    pub name: String,
    /// The file; None for the built-in Default.
    pub path: Option<PathBuf>,
    pub ppq: u32,
    pub rows: Vec<Row>,
}

impl Default for Pattern {
    /// FL's Default pattern (§9): C5, C#5, D5, a 1/16 note each, one after another.
    fn default() -> Self {
        Pattern { name: "Default".into(), path: None, ppq: 96, rows: vec![Row::plain(0, 24, 60), Row::plain(24, 24, 61), Row::plain(48, 24, 62)] }
    }
}

/// The rows and PPQ of a pattern file: 20-byte records for a version string starting with `3.` or `4.`, else 24-byte
/// ones.
pub fn parse(bytes: &[u8]) -> Option<(u32, Vec<Row>)> {
    let f = os_flp::FlpFile::parse(bytes).ok()?;
    let mut ppq = 96;
    let (mut old, mut data) = (false, None);
    for c in &f.chunks {
        match &c.body {
            os_flp::ChunkBody::Header(h) => ppq = u32::from(h.ppq().unwrap_or(96).max(1)),
            os_flp::ChunkBody::Events(evs) => {
                for e in evs {
                    match (e.id, e.data()) {
                        (199, Some(d)) => old = d.starts_with(b"3.") || d.starts_with(b"4."),
                        (224, Some(d)) => data = Some(d.to_vec()),
                        _ => {}
                    }
                }
            }
            os_flp::ChunkBody::Raw(_) => {}
        }
    }
    let d = data?;
    let u32_at = |r: &[u8], o: usize| u32::from_le_bytes(r[o..o + 4].try_into().unwrap());
    let rows = if old {
        d.chunks_exact(20)
            .map(|r| Row {
                pos: u32_at(r, 0),
                len: u32_at(r, 8),
                key: u16::from(r[12]),
                colour: (r[15] & 15) * 4,
                slide: u16::from_le_bytes([r[4], r[5]]) & 0x0008 != 0,
                fine: r[13],
                rel: 64,
                pan: r[16],
                vel: r[17],
                mod_x: r[18],
                mod_y: r[19],
            })
            .collect()
    } else {
        d.chunks_exact(24)
            .map(|r| Row {
                pos: u32_at(r, 0),
                len: u32_at(r, 8),
                key: u16::from_le_bytes([r[12], r[13]]),
                colour: r[19] & 15,
                slide: u16::from_le_bytes([r[4], r[5]]) & 0x0008 != 0,
                fine: r[16],
                rel: r[18],
                pan: r[20],
                vel: r[21],
                mod_x: r[22],
                mod_y: r[23],
            })
            .collect()
    };
    Some((ppq, rows))
}

/// Load a pattern file.
pub fn load(path: &Path) -> Result<Pattern, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let (ppq, rows) = parse(&bytes).ok_or_else(|| format!("{}: not a pattern file", path.display()))?;
    let name = path.file_stem().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    Ok(Pattern { name, path: Some(path.to_path_buf()), ppq, rows })
}

/// FL's Arpeggiator folder (read in place when FL Studio is installed; §9).
pub fn folder() -> PathBuf {
    let fl = crate::factory_folder::dir().unwrap_or_else(os_native::default_fl_dir);
    fl.join("Data").join("Patches").join("Scores").join("Arpeggiator")
}

/// Names compared with their digit runs by value ("2 notes" before "10 notes").
fn natural(a: &str, b: &str) -> Ordering {
    let (mut x, mut y) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (x.peek().copied(), y.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, _) => return Ordering::Less,
            (_, None) => return Ordering::Greater,
            (Some(p), Some(q)) if p.is_ascii_digit() && q.is_ascii_digit() => {
                let num = |it: &mut std::iter::Peekable<std::str::Chars>| {
                    let mut s = String::new();
                    while let Some(c) = it.peek().copied().filter(char::is_ascii_digit) {
                        s.push(c);
                        it.next();
                    }
                    s.trim_start_matches('0').to_string()
                };
                let (m, n) = (num(&mut x), num(&mut y));
                let o = m.len().cmp(&n.len()).then_with(|| m.cmp(&n));
                if o != Ordering::Equal {
                    return o;
                }
            }
            (Some(p), Some(q)) => {
                let o = p.to_ascii_lowercase().cmp(&q.to_ascii_lowercase());
                if o != Ordering::Equal {
                    return o;
                }
                x.next();
                y.next();
            }
        }
    }
}

/// The pattern files and sub-folders of `dir`, each sorted naturally.
pub(crate) fn listing(dir: &Path) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let (mut files, mut dirs) = (Vec::new(), Vec::new());
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() {
            dirs.push(p);
        } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("fsc")) {
            files.push(p);
        }
    }
    let key = |p: &PathBuf| p.file_stem().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    files.sort_by(|a, b| natural(&key(a), &key(b)));
    dirs.sort_by(|a, b| natural(&key(a), &key(b)));
    (files, dirs)
}

/// The ▸ menu (§1, §9): the built-in Default, then FL's Arpeggiator folder when installed, its sub-folders as submenus;
/// the current pattern ticked.
pub fn pattern_menu(cur: &Pattern, root: &Path) -> Vec<PItem> {
    let item = |name: String, path: Option<PathBuf>| {
        let on = cur.path == path;
        PItem::new(name, Cmd::Arp(ACmd::Pattern(path))).mark(Mark::Radio(on))
    };
    let stem = |p: &Path| p.file_stem().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let mut items = vec![item("Default".into(), None)];
    let (files, dirs) = listing(root);
    items.extend(files.into_iter().filter(|p| !stem(p).eq_ignore_ascii_case("Default")).map(|p| item(stem(&p), Some(p))));
    for d in dirs {
        let (files, _) = listing(&d);
        if !files.is_empty() {
            let name = d.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
            items.push(PItem::sub(name, files.into_iter().map(|p| item(stem(&p), Some(p))).collect()));
        }
    }
    items
}

/// One of the Order / Range mode / Sync menus, the current value ticked.
pub fn option_menu(set: &Settings, m: Menu) -> Vec<PItem> {
    match m {
        Menu::Order => Order::ALL.iter().map(|&o| PItem::new(o.name(), Cmd::Arp(ACmd::Order(o))).mark(Mark::Radio(set.order == o))).collect(),
        Menu::RangeOrder => Order::ALL.iter().map(|&o| PItem::new(o.name(), Cmd::Arp(ACmd::RangeOrder(o))).mark(Mark::Radio(set.range_order == o))).collect(),
        Menu::Sync => Sync::ALL.iter().map(|&o| PItem::new(o.name(), Cmd::Arp(ACmd::Sync(o))).mark(Mark::Radio(set.sync == o))).collect(),
        Menu::Pattern => Vec::new(),
    }
}

// ---------------------------------------------------------------- the arpeggiator (§3-§8)

/// A produced note before the collision rules (§6).
#[derive(Debug, Clone, Copy)]
struct Cand {
    src: usize,
    row: usize,
    /// The sync run it was made in.
    run: usize,
    s: f64,
    e: f64,
    key: i32,
    /// The pattern note's own span, before clipping and gating.
    us: f64,
    ue: f64,
    sus: bool,
}

/// The octaves Range adds on repetition `j` (§5).
fn range_octave(set: &Settings, j: i64) -> i64 {
    let r = i64::from(set.range.max(1));
    match set.range_order {
        Order::Normal => j.rem_euclid(r),
        Order::Flip => r - 1 - j.rem_euclid(r),
        Order::Alternate => {
            let x = j.rem_euclid(2 * r);
            if x < r { x } else { 2 * r - 1 - x }
        }
    }
}

/// Arpeggiate `notes` (the targets) with pattern `pat`. `used` are note group numbers to leave alone. Returns the notes
/// that replace the targets; the targets unchanged when the pattern has nothing to play.
pub fn arpeggiate(notes: Vec<Note>, pat: &Pattern, set: &Settings, ppq: u32, used: &[u16]) -> Vec<Note> {
    let (num, den) = TIME_MUL[usize::from(set.time_mul.min(6))];
    let scale = f64::from(ppq) / f64::from(pat.ppq.max(1)) * f64::from(num) / f64::from(den);
    let marks: Vec<&Row> = pat.rows.iter().filter(|r| r.colour == MARKER).collect();
    let end_of = |r: &&Row| f64::from(r.pos + r.len) * scale;
    let len = if marks.is_empty() { pat.rows.iter().map(|r| end_of(&r)).fold(0.0, f64::max) } else { marks.iter().map(end_of).fold(0.0, f64::max) };
    let body: Vec<(usize, &Row)> = pat.rows.iter().enumerate().filter(|(_, r)| r.colour != MARKER).collect();
    if notes.is_empty() || body.is_empty() || len <= 0.0 {
        return notes;
    }
    let floor = f64::from((ppq / 24).max(1));
    // A step note (length 0) counts as one step.
    let span = |n: &Note| {
        let l = if n.length == 0 { ppq / 4 } else { n.length };
        (f64::from(n.position), f64::from(n.position + l))
    };
    let mut pts: Vec<f64> = notes.iter().flat_map(|n| {
        let (a, b) = span(n);
        [a, b]
    }).collect();
    pts.sort_by(f64::total_cmp);
    pts.dedup();
    let sounding = |t: f64| {
        let mut v: Vec<usize> = (0..notes.len()).filter(|&i| {
            let (a, b) = span(&notes[i]);
            a <= t && t < b
        }).collect();
        v.sort_by_key(|&i| notes[i].key);
        v
    };
    let lo = notes.iter().map(|n| span(n).0).fold(f64::MAX, f64::min);
    let hi = notes.iter().map(|n| span(n).1).fold(0.0, f64::max);
    // §4: the origins of the sync runs.
    let origins: Vec<f64> = match set.sync {
        Sync::Time => vec![0.0],
        Sync::Chord => pts.clone(),
        Sync::Block => {
            let mut order: Vec<&Note> = notes.iter().collect();
            order.sort_by_key(|n| n.position);
            let (mut v, mut end) = (Vec::new(), -1.0);
            for n in order {
                let (a, b) = span(n);
                if a >= end {
                    v.push(a);
                }
                end = f64::max(end, b);
            }
            v
        }
    };
    let mut cands: Vec<Cand> = Vec::new();
    for (oi, &o) in origins.iter().enumerate() {
        let stop = origins.get(oi + 1).copied().unwrap_or(hi);
        let mut j = if set.sync == Sync::Time { ((lo.max(o) - o) / len).floor() as i64 } else { 0 };
        while o + j as f64 * len < stop {
            let base = o + j as f64 * len;
            let flip = set.order == Order::Flip || set.order == Order::Alternate && j % 2 == 1;
            for &(ri, p) in &body {
                let s = base + f64::from(p.pos) * scale;
                let gated = (f64::from(p.len) * scale * f64::from(set.gate)).round().max(floor);
                let e = s + gated;
                let (s2, e2) = (s.max(o).max(lo), e.min(stop));
                let sus = p.pos == 0 && f64::from(p.len) * scale >= len;
                let k = i32::from(p.key) - 60;
                let oct = if p.colour == NO_RANGE {
                    0
                } else if set.range == 0 {
                    i64::from(k)
                } else {
                    range_octave(set, j)
                };
                // §6: the chord at the first tick inside where something sounds; kept until the pattern note or the
                // picked note ends; a picked note ending early picks again from there.
                let mut t = s2;
                while t < e2 {
                    let Some(a0) = std::iter::once(t).chain(pts.iter().copied().filter(|&x| t < x && x < e2)).find(|&x| !sounding(x).is_empty()) else { break };
                    let mut ch = sounding(a0);
                    if flip {
                        ch.reverse();
                    }
                    let src = ch[k.rem_euclid(12) as usize % ch.len()];
                    let end = e2.min(span(&notes[src]).1);
                    let key = i64::from(notes[src].key) + 12 * (oct + i64::from(k.div_euclid(12)));
                    cands.push(Cand { src, row: ri, run: oi, s: a0, e: end, key: key as i32, us: s, ue: s + f64::from(p.len) * scale, sus });
                    t = end;
                }
            }
            j += 1;
        }
    }
    // §6 collisions, per picked note and key.
    let mut by: Vec<Vec<(usize, Cand)>> = Vec::new();
    let mut slot: std::collections::HashMap<(usize, i32), usize> = std::collections::HashMap::new();
    for (i, c) in cands.into_iter().enumerate() {
        let at = *slot.entry((c.src, c.key)).or_insert_with(|| {
            by.push(Vec::new());
            by.len() - 1
        });
        by[at].push((i, c));
    }
    let mut kept: Vec<Cand> = Vec::new();
    for mut list in by {
        list.sort_by(|(i, a), (j, b)| a.s.total_cmp(&b.s).then(b.sus.cmp(&a.sus)).then(i.cmp(j)));
        let first = kept.len();
        let mut prev: Option<Cand> = None;
        // Whether `prev` is the last kept note.
        let mut prev_kept = false;
        for (_, c) in list {
            if let Some(p) = prev {
                if (p.us < c.us && c.us < p.ue) || (p.sus && p.us == c.us && !c.sus) {
                    prev = Some(c);
                    prev_kept = false;
                    continue;
                }
                if c.sus && p.sus && prev_kept && kept.len() > first && p.e == c.s && p.run == c.run {
                    let last = kept.last_mut().unwrap();
                    last.e = c.e;
                    last.ue = c.ue;
                    prev = Some(*last);
                    continue;
                }
            }
            kept.push(c);
            prev = Some(c);
            prev_kept = true;
        }
    }
    // §8: a group per source note, in source order.
    let mut srcs: Vec<usize> = kept.iter().map(|c| c.src).collect();
    srcs.sort_by_key(|&i| (notes[i].position, notes[i].key, i));
    srcs.dedup();
    let mut groups = vec![0u16; notes.len()];
    let mut next = 2u16;
    for i in srcs {
        while used.contains(&next) {
            next += 1;
        }
        groups[i] = next;
        next += 1;
    }
    let mut out: Vec<Note> = kept
        .iter()
        .filter(|c| (0..=131).contains(&c.key))
        .map(|c| {
            let (n, r) = (notes[c.src], pat.rows[c.row]);
            let pos = c.s.round();
            let mut o = Note {
                position: pos as u32,
                length: ((c.e - c.s).round() as u32).max(1),
                key: c.key as u16,
                flags: 0x4000 | if r.slide { 0x0008 } else { 0 },
                unk14: if set.group { groups[c.src] } else { 0 },
                ..n
            };
            levels(&mut o, &r, set);
            o
        })
        .collect();
    out.sort_by_key(|n| (n.position, n.key));
    out
}

/// §7: the row's values weighted against the source note's by the Levels knobs.
fn levels(n: &mut Note, r: &Row, set: &Settings) {
    let src = [n.pan, n.velocity, n.release, n.mod_x, n.mod_y, n.fine_pitch];
    let mut v = [0u8; 6];
    for i in 0..6 {
        let (_, _, neutral, max) = LEVELS[i];
        let (x, w, row) = (f64::from(src[i]), f64::from(set.levels[i]), f64::from(r.level(i)));
        let y = if i == 1 { x * (1.0 + (row / 100.0 - 1.0) * w) } else { x + (row - f64::from(neutral)) * w };
        v[i] = y.round().clamp(0.0, f64::from(max)) as u8;
    }
    [n.pan, n.velocity, n.release, n.mod_x, n.mod_y, n.fine_pitch] = v;
}

// ---------------------------------------------------------------- the dialog

/// The window's place and the dialog's state: the settings and pattern shown, the ones last accepted (an opening
/// starts from them) and the open session.
#[derive(Default)]
pub struct ArpUi {
    /// Top-left corner (logical px).
    pub pos: (f32, f32),
    pub placed: bool,
    pub set: Settings,
    pub kept: Settings,
    pub pattern: Pattern,
    pub kept_pattern: Pattern,
    session: Option<Session>,
}

/// What an open dialog previews on: the pattern's notes and selection as they were, and what the last preview left.
struct Session {
    pattern: u16,
    channel: u16,
    base: Vec<Note>,
    base_sel: Vec<usize>,
    last: Vec<Note>,
}

/// Start a session on the piano roll's channel and preview the settings last accepted.
pub fn open(s: &mut State) {
    s.arp.set = s.arp.kept;
    s.arp.pattern = s.arp.kept_pattern.clone();
    s.arp.session = None;
    preview(s);
}

/// Arpeggiate the targets with the dialog's settings; `step` names the undo step (none for a preview).
fn run(s: &mut State, step: Option<&'static str>) -> bool {
    let (set, pat, ppq) = (s.arp.set, s.arp.pattern.clone(), u32::from(s.project.ppq.max(4)));
    let mut used: Vec<u16> = piano::notes(s).iter().map(|n| n.unk14).filter(|&g| g != 0).collect();
    used.sort_unstable();
    used.dedup();
    crate::piano_tools::apply_as(s, |_, ns| arpeggiate(ns, &pat, &set, ppq, &used), step)
}

/// Put the notes back to the session's start and arpeggiate them with the current settings, without an undo step.
/// Notes changed by something else since the last preview (another pattern or channel, an edit, an undo) start the
/// session again from them.
pub fn preview(s: &mut State) {
    let Some(ch) = piano::channel(s) else { return };
    let now = piano::notes(s).to_vec();
    let fresh = match &s.arp.session {
        Some(se) => se.pattern != s.pattern || se.channel != ch || se.last != now,
        None => true,
    };
    if fresh {
        let base_sel = s.piano.selection(s.pattern, Some(ch));
        s.arp.session = Some(Session { pattern: s.pattern, channel: ch, base: now, base_sel, last: Vec::new() });
    }
    restore(s);
    run(s, None);
    let last = piano::notes(s).to_vec();
    if let Some(se) = &mut s.arp.session {
        se.last = last;
    }
}

/// The session's notes and selection back, when the notes are still what its last preview left.
fn restore(s: &mut State) -> bool {
    let Some(se) = &s.arp.session else { return false };
    if se.pattern != s.pattern || piano::notes(s) != se.last.as_slice() && !se.last.is_empty() {
        return false;
    }
    let (base, sel, ch) = (se.base.clone(), se.base_sel.clone(), se.channel);
    piano::pattern_mut(s).notes = base;
    if sel.is_empty() {
        s.piano.clear_selection();
    } else {
        s.piano.select(s.pattern, ch, sel);
    }
    s.piano.dirty = true;
    true
}

/// Accept (§1): the arpeggio becomes one undo step, the settings are kept for the next opening. Returns the status text.
pub fn accept(s: &mut State) -> String {
    if !restore(s) {
        s.arp.session = None;
        return "Piano roll: nothing to arpeggiate".into();
    }
    s.arp.session = None;
    s.arp.kept = s.arp.set;
    s.arp.kept_pattern = s.arp.pattern.clone();
    if run(s, Some("Piano roll arpeggiate")) {
        "Piano roll: arpeggiated".into()
    } else {
        "Piano roll: arpeggiated: nothing to change".into()
    }
}

/// The × button and Esc (§1): the notes as they were before the dialog opened.
pub fn cancel(s: &mut State) {
    restore(s);
    s.arp.session = None;
}

/// Group notes and Reset; the notes follow. The menus, the folder button and Accept are the app's (`App::arp_act`).
pub fn act(s: &mut State, a: Act) {
    match a {
        Act::Group => s.arp.set.group = !s.arp.set.group,
        Act::Reset => {
            s.arp.set = Settings::default();
            s.arp.pattern = Pattern::default();
        }
        Act::Accept | Act::Folder | Act::Menu(_) => return,
    }
    preview(s);
}

/// A menu entry; the notes follow. Returns a message for a pattern file that could not be read.
pub fn command(s: &mut State, c: ACmd) -> Option<String> {
    let mut msg = None;
    match c {
        ACmd::Pattern(None) => s.arp.pattern = Pattern::default(),
        ACmd::Pattern(Some(p)) => match load(&p) {
            Ok(pat) => s.arp.pattern = pat,
            Err(e) => msg = Some(format!("Arpeggiator: {e}")),
        },
        ACmd::Order(o) => s.arp.set.order = o,
        ACmd::RangeOrder(o) => s.arp.set.range_order = o,
        ACmd::Sync(y) => s.arp.set.sync = y,
    }
    preview(s);
    msg
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(pos: u32, len: u32, key: u16) -> Note {
        Note { length: len, key, velocity: 100, ..Note::step(0, pos) }
    }

    fn arp(notes: Vec<Note>, f: impl FnOnce(&mut Settings)) -> Vec<(u32, u32, u16)> {
        arp_with(notes, &Pattern::default(), 96, f)
    }

    fn arp_with(notes: Vec<Note>, pat: &Pattern, ppq: u32, f: impl FnOnce(&mut Settings)) -> Vec<(u32, u32, u16)> {
        let mut set = Settings::default();
        f(&mut set);
        arpeggiate(notes, pat, &set, ppq, &[]).iter().map(|n| (n.position, n.length, n.key)).collect()
    }

    fn keys(v: &[(u32, u32, u16)]) -> Vec<u16> {
        v.iter().map(|n| n.2).collect()
    }

    /// `ui/piano-tools.md` §2 (Reset settings, the Default pattern, PPQ 96).
    #[test]
    fn default_pattern_matches_fl() {
        let c = arp(vec![note(0, 384, 60), note(0, 384, 64), note(0, 384, 67)], |_| {});
        assert_eq!(c.len(), 16);
        assert!(c.iter().all(|n| n.1 == 24));
        assert_eq!(keys(&c[..6]), [60, 64, 67, 60, 64, 67]);
        // D F A C at 384: the pattern repeats from tick 0, so it starts on its second row; C is never reached.
        let d = arp(vec![note(384, 192, 62), note(384, 192, 65), note(384, 192, 69), note(384, 192, 72)], |_| {});
        assert_eq!(keys(&d), [65, 69, 62, 65, 69, 62, 65, 69]);
        let s = arp(vec![note(768, 96, 50)], |_| {});
        assert_eq!(s, [(768, 24, 50), (792, 24, 50), (816, 24, 50), (840, 24, 50)]);
        // C4 96 long + G4 192 long at 960.
        let u = arp(vec![note(960, 96, 48), note(960, 192, 55)], |_| {});
        assert_eq!(keys(&u), [55, 48, 48, 55, 55, 55, 55, 55]);
    }

    /// §5: Flip, Alternate, Range and its modes, `---`.
    #[test]
    fn order_and_range() {
        let ceg = || vec![note(0, 144, 60), note(0, 144, 64), note(0, 144, 67)];
        assert_eq!(keys(&arp(ceg(), |s| s.order = Order::Flip)), [67, 64, 60, 67, 64, 60]);
        assert_eq!(keys(&arp(ceg(), |s| s.order = Order::Alternate)), [60, 64, 67, 67, 64, 60]);
        assert_eq!(keys(&arp(ceg(), |s| s.range = 2)), [60, 64, 67, 72, 76, 79]);
        assert_eq!(keys(&arp(ceg(), |s| (s.range, s.range_order) = (2, Order::Flip))), [72, 76, 79, 60, 64, 67]);
        let long = || vec![note(0, 288, 60), note(0, 288, 64), note(0, 288, 67)];
        let alt = arp(long(), |s| (s.range, s.range_order) = (2, Order::Alternate));
        assert_eq!(keys(&alt).chunks(3).map(|c| c[0]).collect::<Vec<_>>(), [60, 72, 72, 60]);
        // `---`: C, E + 12, G + 24.
        assert_eq!(keys(&arp(ceg(), |s| s.range = 0))[..3], [60, 76, 91]);
        assert_eq!(range_octave(&Settings { range: 3, range_order: Order::Alternate, ..Settings::default() }, 4), 1);
    }

    /// §4: Block and Chord restart the pattern; Time keeps counting from tick 0.
    #[test]
    fn sync() {
        let ns = || vec![note(384, 192, 62), note(384, 192, 65), note(384, 192, 69)];
        assert_eq!(keys(&arp(ns(), |_| {}))[0], 65);
        assert_eq!(keys(&arp(ns(), |s| s.sync = Sync::Block))[..3], [62, 65, 69]);
        // Chord: a new note restarts at its start.
        let chord = arp(vec![note(0, 96, 60), note(48, 48, 64)], |s| s.sync = Sync::Chord);
        assert_eq!(chord, [(0, 24, 60), (24, 24, 60), (48, 24, 60), (72, 24, 64)]);
        // Block: touching notes are separate blocks, overlapping ones share one.
        let block = arp(vec![note(0, 36, 60), note(36, 36, 62)], |s| s.sync = Sync::Block);
        assert_eq!(block, [(0, 24, 60), (24, 12, 60), (36, 24, 62), (60, 12, 62)]);
    }

    /// §7: Gate, its PPQ / 24 floor, Time mul.
    #[test]
    fn gate_and_time_mul() {
        let one = || vec![note(0, 96, 60)];
        for (g, want) in [(0.95, 23), (0.9, 22), (0.8, 19), (0.6, 14), (0.0, 4)] {
            assert_eq!(arp(one(), |s| s.gate = g)[0].1, want, "gate {g}");
        }
        assert_eq!(arp_with(one(), &Pattern::default(), 192, |s| s.gate = 0.0)[0].1, 8);
        let slow = arp(one(), |s| s.time_mul = 4);
        assert_eq!(slow.iter().map(|n| (n.0, n.1)).collect::<Vec<_>>(), [(0, 48), (48, 48)]);
        let fast = arp(one(), |s| s.time_mul = 0);
        assert_eq!(fast[..3].iter().map(|n| (n.0, n.1)).collect::<Vec<_>>(), [(0, 4), (3, 4), (6, 4)]);
        // PPQ 192 doubles the pattern.
        assert_eq!(arp_with(one(), &Pattern::default(), 192, |_| {})[1].0, 48);
    }

    /// §7: the measured Levels case.
    #[test]
    fn levels_match_fl() {
        let src = Note { velocity: 50, pan: 100, release: 20, mod_x: 60, mod_y: 200, fine_pitch: 150, ..note(0, 48, 60) };
        let lo = Row { vel: 20, pan: 0, mod_x: 0, mod_y: 0, fine: 0, ..Row::plain(0, 24, 60) };
        let hi = Row { vel: 127, pan: 128, mod_x: 255, mod_y: 255, fine: 240, pos: 24, ..Row::plain(0, 24, 60) };
        let pat = Pattern { rows: vec![lo, hi], ..Pattern::default() };
        let out = arpeggiate(vec![src], &pat, &Settings::default(), 96, &[]);
        let v = |n: &Note| (n.velocity, n.pan, n.mod_x, n.mod_y, n.fine_pitch);
        assert_eq!(v(&out[0]), (10, 36, 0, 72, 30));
        assert_eq!((out[1].pan, out[1].mod_x, out[1].mod_y, out[1].fine_pitch), (128, 187, 255, 240));
        let off = arpeggiate(vec![src], &pat, &Settings { levels: [0.0; 6], ..Settings::default() }, 96, &[]);
        assert_eq!(v(&off[0]), (50, 100, 60, 200, 150));
    }

    /// §2: yellow rows set the length and are not played; pink rows get no range octave; a sustained row holds.
    #[test]
    fn row_colours() {
        let ceg = || vec![note(0, 192, 60), note(0, 192, 64), note(0, 192, 67)];
        let marked = Pattern { rows: vec![Row::plain(0, 24, 60), Row { colour: MARKER, ..Row::plain(0, 48, 60) }], ..Pattern::default() };
        assert_eq!(arp_with(ceg(), &marked, 96, |_| {}).iter().map(|n| n.0).collect::<Vec<_>>(), [0, 48, 96, 144]);
        let pink = Pattern { rows: vec![Row { colour: NO_RANGE, ..Row::plain(0, 24, 60) }, Row::plain(24, 24, 61)], ..Pattern::default() };
        assert_eq!(keys(&arp_with(ceg(), &pink, 96, |s| s.range = 2))[..4], [60, 64, 60, 76]);
        let held = Pattern { rows: vec![Row::plain(0, 48, 60), Row::plain(0, 24, 62)], ..Pattern::default() };
        assert_eq!(arp_with(ceg(), &held, 96, |_| {}).iter().filter(|n| n.2 == 60).count(), 1);
        // Below C5: the member an octave down.
        let low = Pattern { rows: vec![Row::plain(0, 24, 59)], ..Pattern::default() };
        assert_eq!(keys(&arp_with(ceg(), &low, 96, |_| {}))[0], 55);
    }

    /// §8: a group per source note from 2, skipping used numbers; off: 0.
    #[test]
    fn groups() {
        let ns = vec![note(0, 72, 60), note(0, 72, 64), note(0, 72, 67)];
        let out = arpeggiate(ns.clone(), &Pattern::default(), &Settings::default(), 96, &[3]);
        let g = |k: u16| out.iter().find(|n| n.key == k).unwrap().unk14;
        assert_eq!((g(60), g(64), g(67)), (2, 4, 5));
        let off = arpeggiate(ns, &Pattern::default(), &Settings { group: false, ..Settings::default() }, 96, &[]);
        assert!(off.iter().all(|n| n.unk14 == 0 && n.flags == 0x4000));
    }

    /// §2: both record layouts.
    #[test]
    fn parses_both_layouts() {
        fn file(version: &[u8], recs: &[u8]) -> Vec<u8> {
            let mut out = b"FLhd".to_vec();
            out.extend_from_slice(&6u32.to_le_bytes());
            out.extend_from_slice(&[0, 0, 1, 0, 96, 0]);
            let mut ev = vec![199, version.len() as u8];
            ev.extend_from_slice(version);
            ev.extend_from_slice(&[65, 0, 0, 224, recs.len() as u8]);
            ev.extend_from_slice(recs);
            out.extend_from_slice(b"FLdt");
            out.extend_from_slice(&(ev.len() as u32).to_le_bytes());
            out.extend_from_slice(&ev);
            out
        }
        let mut old = vec![0u8; 20];
        old[4] = 0x08;
        old[8] = 24;
        (old[12], old[13], old[15], old[16], old[17], old[18], old[19]) = (61, 120, 0x13, 64, 90, 128, 128);
        let (ppq, rows) = parse(&file(b"4.5.2\0", &old)).unwrap();
        assert_eq!((ppq, rows.len(), rows[0].key, rows[0].colour, rows[0].vel, rows[0].len), (96, 1, 61, 12, 90, 24));
        assert!(rows[0].slide, "20-byte rows keep the slide flag too");
        let mut new = vec![0u8; 24];
        new[4] = 0x08;
        new[8] = 48;
        new[12] = 72;
        new[19] = 8;
        new[21] = 100;
        let (_, rows) = parse(&file(b"8.1.0\0", &new)).unwrap();
        assert_eq!((rows[0].key, rows[0].colour, rows[0].slide, rows[0].len), (72, 8, true, 48));
    }

    #[test]
    fn natural_order() {
        let mut v = vec!["10 notes", "2 notes", "Riff machine", "1 note"];
        v.sort_by(|a, b| natural(a, b));
        assert_eq!(v, ["1 note", "2 notes", "10 notes", "Riff machine"]);
    }
}
