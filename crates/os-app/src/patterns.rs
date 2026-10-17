// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Pattern selector and PATTERNS menu commands: find empty, rename / colour, clone, insert, delete, move,
//! transpose, plus the name box and the delete confirmation.
// spec: ui/pattern-menu.md

use os_model::{ClipSource, Pattern, Rgb};

use crate::app::State;

/// Highest pattern number (§1).
pub const MAX: u16 = 999;
/// Pixels of vertical drag per pattern step on the name box (§1, OS: linear).
pub const DRAG_STEP: f32 = 8.0;
/// Name box size (§4).
pub const BOX_W: f32 = 251.0;
pub const BOX_H: f32 = 51.0;
/// Names remembered for the suggestions column (OS).
const RECENT: usize = 5;

/// Generic name suggestions (OS word list, §4).
pub const WORDS: [&[&str]; 4] = [
    &["Drums", "Kick", "Snare", "Clap", "Hats", "Toms", "Perc", "Cymbal", "Shaker", "FX"],
    &["Bass", "Sub", "Lead", "Pad", "Keys", "Piano", "Pluck", "Chords", "Arp", "Strings"],
    &["Vocal", "Choir", "Guitar", "Brass", "Bells", "Organ", "Synth", "Sample", "Loop", "Fill"],
    &["Intro", "Verse", "Pre-chorus", "Chorus", "Hook", "Bridge", "Drop", "Breakdown", "Build", "Outro"],
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cmd {
    FindFirst,
    FindNext,
    FindNextNoName,
    SelectInPlaylist,
    Rename,
    ChangeColor,
    RandomColor,
    Transpose,
    InsertOne,
    Clone,
    Delete,
    MoveUp,
    MoveDown,
    /// Set time signature...: a dialog, run by `App::pattern_cmd` (spec: ui/pattern-time-signature.md).
    TimeSignature,
    /// Open in project browser: the browser's Patterns section, run by `App::pattern_cmd`
    /// (spec: ui/browser-commands.md §4).
    OpenInBrowser,
    /// Split by channel: one pattern per channel with notes (spec: ui/split-by-channel.md §2).
    SplitByChannel,
}

impl Cmd {
    /// The command behind a PATTERNS menu label.
    pub fn from_label(label: &str) -> Option<Cmd> {
        Some(match label {
            "Find first empty..." => Cmd::FindFirst,
            "Find next empty..." => Cmd::FindNext,
            "Find next empty (no naming)" => Cmd::FindNextNoName,
            "Select in playlist" => Cmd::SelectInPlaylist,
            "Rename and color..." => Cmd::Rename,
            "Change color..." => Cmd::ChangeColor,
            "Random color" => Cmd::RandomColor,
            "Transpose..." => Cmd::Transpose,
            "Insert one" => Cmd::InsertOne,
            "Clone" => Cmd::Clone,
            "Delete" => Cmd::Delete,
            "Move up" => Cmd::MoveUp,
            "Move down" => Cmd::MoveDown,
            "Set time signature..." => Cmd::TimeSignature,
            "Open in project browser" => Cmd::OpenInBrowser,
            "Split by channel" => Cmd::SplitByChannel,
            _ => return None,
        })
    }
}

/// Clicks inside the name box, its popups and the confirm dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    Accept,
    Cancel,
    Suggestions,
    /// Column, row of a suggestion (column 0 = recent names).
    Suggest(usize, usize),
    Color,
    Swatch(usize), // This code is a part of L e m o n S t u d i o.
    ConfirmOk,
    ConfirmCancel,
    /// The confirmation's Remember my choice check.
    ConfirmRemember,
    Inside,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoxTarget {
    Name(u16),
    Transpose(u16),
    /// The channel rack's boxes (spec: ui/channel-rack.md §13, §17, §21).
    Rack(crate::rack::RBox),
}

#[derive(Debug, Clone, PartialEq)]
pub struct NameBox {
    pub target: BoxTarget,
    pub text: String,
    /// The text is selected: the first typed character replaces it.
    pub fresh: bool,
    /// Top-left on screen.
    pub at: (f32, f32),
    pub suggestions: bool,
    pub palette: bool,
    /// Opened for a new pattern (find empty, insert one): accepting it makes the pattern the playlist's clip
    /// source, listed in the picker even while empty, so it can be placed at once.
    pub created: bool,
}

#[derive(Debug, Default)]
pub struct PatternUi {
    pub name_box: Option<NameBox>,
    /// "The selected pattern is about to be deleted." is showing.
    pub confirm: bool,
    /// The confirmation's "Remember my choice" check: off each time it opens; Ok with it ticked turns the
    /// confirmation's Manage messages entry off, Cancel never does (spec: ui/settings-choices.md "Hints and warnings:
    /// Manage messages and Project data size warning (2026-10-16)").
    pub remember: bool,
    pub recent: Vec<String>,
    /// Where the PATTERNS menu opens (`None` = under its menu-bar title).
    pub menu_at: Option<(f32, f32)>,
}

/// Palette for the colour tab and "Change color..." (OS swatches, §4).
pub const SWATCHES: [u32; 20] = [
    0x925755, 0xa65d3f, 0xa8813f, 0x9a9a45, 0x6f9446, 0x45946a, 0x3f9493, 0x4679a2, 0x4b4977, 0x6d4f91,
    0x905887, 0x764955, 0x5f6f75, 0xc06060, 0xd08850, 0xd0c060, 0x80c070, 0x60b0c0, 0x7090e0, 0xb080d0,
];

// ---------------------------------------------------------------------------------------------------------

/// A pattern with notes, control points or its own name (listed in menus, §2).
pub fn used(p: &Pattern) -> bool {
    !p.notes.is_empty() || !p.controls.is_empty() || p.name.is_some()
}

/// Numbers of the listed patterns, ascending: the used ones, those placed in the playlist, and the playlist's clip
/// source (a new pattern, empty until notes are added).
pub fn listed(s: &State) -> Vec<u16> {
    let mut v: Vec<u16> = s.project.patterns.iter().filter(|p| used(p)).map(|p| p.number).collect();
    v.extend(crate::playlist::clips(s).iter().filter_map(|c| if let ClipSource::Pattern(n) = c.source { Some(n) } else { None }));
    if let Some(ClipSource::Pattern(n)) = s.playlist.source {
        v.push(n);
    }
    v.sort_unstable();
    v.dedup();
    v
}

/// A new pattern's name box: accepting it lists the pattern and makes it the playlist's clip source.
fn created_box(s: &mut State) {
    if let Some(b) = s.pat_ui.name_box.as_mut() {
        b.created = true;
    }
}

pub fn name_of(s: &State, n: u16) -> String {
    s.project.pattern(n).and_then(|p| p.name.clone()).unwrap_or_else(|| format!("Pattern {n}"))
}

fn is_used(s: &State, n: u16) -> bool {
    s.project.pattern(n).is_some_and(used)
}

/// Lowest unused number.
pub fn first_empty(s: &State) -> u16 {
    (1..=MAX).find(|&n| !is_used(s, n)).unwrap_or(MAX)
}

/// Lowest unused number above `after`.
pub fn next_empty(s: &State, after: u16) -> u16 {
    (after.saturating_add(1)..=MAX).find(|&n| !is_used(s, n)).unwrap_or(MAX)
}

/// The pattern `n`, added when the project has none with that number.
pub fn pat_mut(s: &mut State, n: u16) -> &mut Pattern {
    let ps = &mut s.project.patterns;
    let i = match ps.iter().position(|p| p.number == n) {
        Some(i) => i,
        None => {
            let at = ps.partition_point(|p| p.number < n);
            ps.insert(at, Pattern { number: n, ..Pattern::default() });
            at
        }
    };
    &mut ps[i]
}

/// Renumber patterns (`None` removes one): playlist clips follow (clips of removed patterns go). Numbers that
/// end up without a pattern keep an empty entry so saving clears their blocks.
fn renumber(s: &mut State, map: impl Fn(u16) -> Option<u16>) {
    let old: Vec<u16> = s.project.patterns.iter().map(|p| p.number).collect();
    let mut pats: Vec<Pattern> = std::mem::take(&mut s.project.patterns)
        .into_iter()
        .filter_map(|mut p| {
            let n = map(p.number)?;
            p.number = n;
            Some(p)
        })
        .collect();
    for n in old {
        if !pats.iter().any(|p| p.number == n) {
            pats.push(Pattern { number: n, ..Pattern::default() });
        }
    }
    pats.sort_by_key(|p| p.number);
    s.project.patterns = pats;
    if let Some(a) = s.project.arrangement_mut() {
        a.clips.retain_mut(|c| match c.source {
            ClipSource::Pattern(n) => match map(n) {
                Some(m) => {
                    c.source = ClipSource::Pattern(m);
                    true
                }
                None => false,
            },
            _ => true,
        });
    }
    s.playlist.sel.clear();
    s.playlist.hover_clip = None;
    s.piano.clear_selection();
    s.piano.dirty = true;
}

/// "<name> #k" with the lowest free k >= 2 (§3 Clone).
pub fn clone_name(s: &State, n: u16) -> String {
    let name = name_of(s, n);
    let base = match name.rsplit_once(" #") {
        Some((b, k)) if k.parse::<u32>().is_ok() => b.to_string(),
        _ => name,
    };
    let taken = |cand: &str| s.project.patterns.iter().any(|p| p.name.as_deref() == Some(cand));
    (2..).map(|k| format!("{base} #{k}")).find(|c| !taken(c)).expect("unbounded")
}

/// Swap patterns `a` and `b`.
fn swap(s: &mut State, a: u16, b: u16) {
    renumber(s, |n| Some(if n == a { b } else if n == b { a } else { n }));
}

/// Transpose every note of pattern `n` by `semis`.
pub fn transpose(s: &mut State, n: u16, semis: i32) {
    if let Some(p) = s.project.patterns.iter_mut().find(|p| p.number == n) {
        for note in &mut p.notes {
            note.key = (i32::from(note.key) + semis).clamp(0, 131) as u16;
        }
        s.piano.dirty = true;
    }
}

pub(crate) fn random_color(seed: u64) -> Rgb {
    // A random hue at a muted saturation / lightness like the default pattern colours.
    let mut x = seed ^ 0x9e37_79b9_7f4a_7c15;
    x ^= x >> 33;
    x = x.wrapping_mul(0xff51_afd7_ed55_8ccd);
    x ^= x >> 33;
    let h = (x % 360) as f32;
    let (sat, l) = (0.35f32, 0.45f32);
    let c = (1.0 - (2.0 * l - 1.0).abs()) * sat;
    let hp = h / 60.0;
    let xx = c * (1.0 - (hp % 2.0 - 1.0).abs());
    let (r, g, b) = match hp as u32 {
        0 => (c, xx, 0.0),
        1 => (xx, c, 0.0),
        2 => (0.0, c, xx),
        3 => (0.0, xx, c),
        4 => (xx, 0.0, c),
        _ => (c, 0.0, xx),
    };
    let m = l - c / 2.0;
    let u = |v: f32| ((v + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    Rgb::new(u(r), u(g), u(b))
}

/// Open the name box for pattern `n` at `at`.
pub fn open_name_box(s: &mut State, target: BoxTarget, at: (f32, f32)) {
    let text = match target {
        BoxTarget::Name(n) => name_of(s, n),
        BoxTarget::Transpose(_) => String::new(),
        BoxTarget::Rack(b) => crate::rack::box_text(s, b),
    };
    s.pat_ui.name_box = Some(NameBox { target, text, fresh: true, at, suggestions: false, palette: false, created: false });
}

/// Where a box opened from a menu / key goes: the mouse over its colour tab (§4).
pub fn box_at_mouse(mouse: (f32, f32)) -> (f32, f32) {
    ((mouse.0 - 241.0).max(0.0), (mouse.1 - 41.0).max(0.0))
}

/// Box position for "+" (§4).
pub const PLUS_BOX_AT: (f32, f32) = (949.0, 46.0);

/// Run a command; `at` is where a name box opens. Returns a status message.
pub fn run(s: &mut State, cmd: Cmd, at: (f32, f32)) -> Option<String> {
    let cur = s.pattern.max(1);
    match cmd {
        Cmd::FindFirst => {
            s.pattern = first_empty(s);
            open_name_box(s, BoxTarget::Name(s.pattern), at);
            created_box(s);
        }
        Cmd::FindNext => {
            s.pattern = next_empty(s, cur);
            open_name_box(s, BoxTarget::Name(s.pattern), at);
            created_box(s);
        }
        Cmd::FindNextNoName => {
            s.pattern = next_empty(s, cur);
            s.playlist.source = Some(ClipSource::Pattern(s.pattern));
        }
        Cmd::SelectInPlaylist => {
            let sel: Vec<usize> = crate::playlist::clips(s)
                .iter()
                .enumerate()
                .filter(|(_, c)| c.source == ClipSource::Pattern(cur))
                .map(|(i, _)| i)
                .collect();
            let n = sel.len();
            s.playlist.sel = sel;
            if !s.shown(crate::app::Win::Playlist) || !s.in_front(crate::app::Win::Playlist) {
                s.raise(crate::app::Win::Playlist);
            }
            return Some(format!("Selected {n} clip(s) of {}", name_of(s, cur)));
        }
        Cmd::Rename => open_name_box(s, BoxTarget::Name(cur), at),
        Cmd::ChangeColor => {
            open_name_box(s, BoxTarget::Name(cur), at);
            if let Some(b) = s.pat_ui.name_box.as_mut() {
                b.palette = true;
            }
        }
        Cmd::RandomColor => {
            let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64);
            pat_mut(s, cur).color = Some(random_color(seed ^ u64::from(cur)));
            s.piano.dirty = true;
        }
        Cmd::Transpose => open_name_box(s, BoxTarget::Transpose(cur), at),
        Cmd::InsertOne => {
            renumber(s, |n| Some(if n >= cur { (n + 1).min(MAX) } else { n }));
            open_name_box(s, BoxTarget::Name(cur), at);
            created_box(s);
        }
        Cmd::Clone => {
            if cur >= MAX {
                return None;
            }
            let mut copy = s.project.pattern(cur).cloned().unwrap_or(Pattern { number: cur, ..Pattern::default() });
            copy.name = Some(clone_name(s, cur));
            renumber(s, |n| Some(if n > cur { (n + 1).min(MAX) } else { n }));
            copy.number = cur + 1;
            *pat_mut(s, cur + 1) = copy;
            s.pattern = cur + 1;
        }
        // Its Manage messages entry off: as if Ok was pressed.
        Cmd::Delete if !crate::messages::on(&crate::messages::DELETE_PATTERN) => delete_pattern(s),
        Cmd::Delete => open_confirm(s),
        Cmd::MoveUp if cur > 1 => {
            swap(s, cur, cur - 1);
            s.pattern = cur - 1;
        }
        Cmd::MoveDown if cur < MAX => {
            swap(s, cur, cur + 1);
            s.pattern = cur + 1;
        }
        Cmd::MoveUp | Cmd::MoveDown => {}
        Cmd::SplitByChannel => split_by_channel(s),
        // Dialogs and the browser: `App::pattern_cmd` runs them.
        Cmd::TimeSignature | Cmd::OpenInBrowser => {}
    }
    None
}

/// Shows the delete confirmation (the pattern's, or the channels' waiting in `rack.pending_delete`), its Remember my
/// choice off.
pub fn open_confirm(s: &mut State) {
    s.pat_ui.confirm = true;
    s.pat_ui.remember = false;
}

/// The Manage messages entry of the delete confirmation showing: the channels' or the pattern's.
fn confirming(s: &State) -> &'static crate::messages::Message {
    if s.rack.pending_delete.is_some() {
        &crate::messages::DELETE_CHANNEL
    } else {
        &crate::messages::DELETE_PATTERN
    }
}

/// PATTERNS > Split by channel (spec: ui/split-by-channel.md §2). Channels with notes in the current pattern, in rack
/// order: the pattern keeps the first one's notes and takes its name; each further one gets a pattern of its own named
/// after it, right after the current one in channel order (later patterns move up with their clips, as Insert one),
/// with the original's colour, markers and other settings but only that channel's notes. Every playlist clip of the
/// original gets one clip per new pattern at the same place, under it. No notes: nothing happens. One undo step.
pub fn split_by_channel(s: &mut State) {
    let cur = s.pattern.max(1);
    let Some(orig) = s.project.pattern(cur).cloned() else { return };
    let chans: Vec<(u16, String)> = s
        .project
        .channels
        .iter()
        .enumerate()
        .filter(|(_, c)| orig.notes.iter().any(|n| n.channel == c.index))
        .map(|(r, c)| (c.index, crate::rack::channel_name(s, r)))
        .collect();
    let Some(((first, name), rest)) = chans.split_first() else { return };
    let k = rest.len() as u16;
    // Room for the new numbers: no used pattern may move past the last one.
    let last = s.project.patterns.iter().filter(|p| used(p)).map(|p| p.number).max().unwrap_or(cur).max(cur);
    if usize::from(last) + rest.len() > usize::from(MAX) {
        return;
    }
    let undo = crate::history::Snap::Patterns { patterns: s.project.patterns.clone(), current: cur };
    crate::playlist::snapshot_with(s, "Split by channel", undo);
    // Notes of a channel no longer in the rack stay with the original.
    let p = pat_mut(s, cur);
    p.notes.retain(|n| n.channel == *first || !chans.iter().any(|c| c.0 == n.channel));
    p.name = Some(name.clone());
    s.piano.dirty = true;
    if k == 0 {
        return;
    }
    renumber(s, |n| Some(if n > cur { n + k } else { n }));
    for (j, (ch, name)) in (1..).zip(rest) {
        // Control points stay with the original (only notes are divided).
        let notes = orig.notes.iter().filter(|n| n.channel == *ch).copied().collect();
        *pat_mut(s, cur + j) = Pattern { number: cur + j, name: Some(name.clone()), notes, controls: Vec::new(), ..orig.clone() };
    }
    if let Some(a) = s.project.arrangement_mut() {
        // Clips are drawn in record order: the new ones go just before their original, which stays on top.
        let mut clips = Vec::with_capacity(a.clips.len());
        for c in std::mem::take(&mut a.clips) {
            if c.source == ClipSource::Pattern(cur) {
                clips.extend((1..=k).map(|j| os_model::Clip { source: ClipSource::Pattern(cur + j), ..c }));
            }
            clips.push(c);
        }
        a.clips = clips;
    }
}

/// Ok in the delete confirmation (channels waiting in the rack go to the app, which removes them). With Remember my
/// choice ticked, the confirmation's Manage messages entry turns off.
pub fn delete_current(s: &mut State) {
    if s.pat_ui.remember {
        crate::messages::set(confirming(s), false);
    }
    s.pat_ui.confirm = false;
    s.pat_ui.remember = false;
    if let Some(rows) = s.rack.pending_delete.take() {
        s.rack.delete_now = Some(rows);
        return;
    }
    delete_pattern(s);
}

/// Cancel in the delete confirmation: nothing is deleted and Remember my choice is not kept.
fn cancel_confirm(s: &mut State) {
    s.pat_ui.confirm = false;
    s.pat_ui.remember = false;
    s.rack.pending_delete = None;
}

/// Deletes the current pattern: later ones move down a number, its clips go.
fn delete_pattern(s: &mut State) {
    let cur = s.pattern.max(1);
    renumber(s, |n| if n == cur { None } else if n > cur { Some(n - 1) } else { Some(n) });
}

/// Accept the name box.
pub fn accept(s: &mut State) {
    let Some(b) = s.pat_ui.name_box.take() else { return };
    match b.target {
        BoxTarget::Name(n) => {
            let text = b.text.trim().to_string();
            let name = (!text.is_empty() && text != format!("Pattern {n}")).then_some(text.clone());
            if name.is_some() || s.project.pattern(n).is_some() {
                pat_mut(s, n).name = name;
            }
            if b.created {
                s.playlist.source = Some(ClipSource::Pattern(n));
            }
            if !text.is_empty() {
                s.pat_ui.recent.retain(|r| *r != text);
                s.pat_ui.recent.insert(0, text);
                s.pat_ui.recent.truncate(RECENT);
            }
            s.piano.dirty = true;
        }
        BoxTarget::Transpose(n) => {
            if let Ok(v) = b.text.trim().trim_start_matches('+').parse::<i32>() {
                transpose(s, n, v);
            }
        }
        BoxTarget::Rack(rb) => {
            crate::rack::box_accept(s, rb, &b.text);
            s.piano.dirty = true;
        }
    }
}

/// A suggestion picked: rename and close at once (§4).
pub fn suggest(s: &mut State, col: usize, row: usize) {
    let word = if col == 0 { s.pat_ui.recent.get(row).cloned() } else { WORDS.get(col - 1).and_then(|c| c.get(row)).map(|w| w.to_string()) };
    if let (Some(w), Some(b)) = (word, s.pat_ui.name_box.as_mut()) {
        b.text = w;
        accept(s);
    }
}

pub fn set_color(s: &mut State, i: usize) {
    let target = s.pat_ui.name_box.as_ref().map(|b| b.target);
    if let Some(&c) = SWATCHES.get(i) {
        let col = Some(Rgb::new((c >> 16) as u8, (c >> 8) as u8, c as u8));
        match target {
            Some(BoxTarget::Name(n)) => pat_mut(s, n).color = col,
            Some(BoxTarget::Rack(crate::rack::RBox::Channel(r))) => {
                if let Some(ch) = s.project.channels.get_mut(r) {
                    ch.color = col;
                }
            }
            _ => return,
        }
        s.piano.dirty = true;
    }
    if let Some(b) = s.pat_ui.name_box.as_mut() {
        b.palette = false;
    }
}

/// A click in the name box / its popups / the confirmation.
pub fn hit(s: &mut State, h: Hit) {
    match h {
        Hit::Accept => accept(s),
        Hit::Cancel => s.pat_ui.name_box = None,
        Hit::Suggestions => {
            if let Some(b) = s.pat_ui.name_box.as_mut() {
                b.suggestions = !b.suggestions;
                b.palette = false;
            }
        }
        Hit::Suggest(c, r) => suggest(s, c, r),
        Hit::Color => {
            if let Some(b) = s.pat_ui.name_box.as_mut() {
                b.palette = !b.palette;
                b.suggestions = false;
            }
        }
        Hit::Swatch(i) => set_color(s, i),
        Hit::ConfirmOk => delete_current(s),
        Hit::ConfirmCancel => cancel_confirm(s),
        Hit::ConfirmRemember => s.pat_ui.remember = !s.pat_ui.remember,
        Hit::Inside => {}
    }
}

/// Typing into the name box. Returns whether the key was taken.
pub fn key(s: &mut State, key: &winit::keyboard::Key, ctrl: bool) -> bool {
    use winit::keyboard::{Key, NamedKey};
    if s.pat_ui.confirm {
        match key {
            Key::Named(NamedKey::Enter) => delete_current(s),
            Key::Named(NamedKey::Escape) => cancel_confirm(s),
            _ => {}
        }
        return true;
    }
    let Some(b) = s.pat_ui.name_box.as_mut() else { return false };
    let typed = |b: &mut NameBox, t: &str| {
        if b.fresh {
            b.text.clear();
            b.fresh = false;
        }
        b.text.push_str(t);
    };
    match key {
        Key::Named(NamedKey::Enter) => accept(s),
        Key::Named(NamedKey::Escape) => s.pat_ui.name_box = None,
        Key::Named(NamedKey::Backspace) => {
            if b.fresh {
                b.text.clear();
                b.fresh = false;
            } else {
                b.text.pop();
            }
        }
        Key::Named(NamedKey::Space) => typed(b, " "),
        Key::Character(c) if !ctrl => typed(b, c),
        _ => {}
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use os_model::{Clip, Note};

    fn app() -> App {
        let mut app = App::new(None);
        app.state.project = crate::app::test_project();
        let s = &mut app.state;
        s.project.patterns = vec![
            Pattern { number: 1, name: Some("Kick".into()), notes: vec![Note::step(0, 0)], ..Pattern::default() },
            Pattern { number: 2, name: Some("Kick #2".into()), notes: vec![Note::step(0, 48)], ..Pattern::default() },
            Pattern { number: 3, notes: vec![Note::step(1, 0)], ..Pattern::default() },
            Pattern { number: 5, name: Some("Lead".into()), ..Pattern::default() },
        ];
        crate::playlist::arr_mut(s).clips = vec![Clip::new(0, 384, 0, ClipSource::Pattern(1)), Clip::new(0, 384, 1, ClipSource::Pattern(3))];
        app
    }

    fn clip_pats(s: &State) -> Vec<u16> {
        crate::playlist::clips(s).iter().filter_map(|c| if let ClipSource::Pattern(n) = c.source { Some(n) } else { None }).collect()
    }

    #[test]
    fn empty_numbers_and_listing() {
        let mut app = app();
        let s = &mut app.state;
        assert_eq!(listed(s), vec![1, 2, 3, 5]);
        assert_eq!(first_empty(s), 4);
        s.pattern = 5;
        assert_eq!(next_empty(s, 5), 6);
        run(s, Cmd::FindFirst, (0.0, 0.0));
        assert_eq!(s.pattern, 4);
        // Cancel keeps the number current but unlisted; accepting a name lists it.
        s.pat_ui.name_box = None;
        assert_eq!(listed(s), vec![1, 2, 3, 5]);
        run(s, Cmd::Rename, (0.0, 0.0));
        key(s, &winit::keyboard::Key::Character("Probe".into()), false);
        accept(s);
        assert_eq!(listed(s), vec![1, 2, 3, 4, 5]);
        assert_eq!(name_of(s, 4), "Probe");
    }

    /// A new pattern is placeable at once: accepting its name box, default name included, lists it in the picker
    /// and makes it the playlist's clip source while it is still empty; placed, it stays listed.
    #[test]
    fn a_new_empty_pattern_is_listed_and_picked() {
        let mut app = app();
        let s = &mut app.state;
        s.playlist.source = Some(ClipSource::Pattern(1));
        run(s, Cmd::FindFirst, (0.0, 0.0));
        accept(s);
        assert_eq!((s.pattern, s.playlist.source), (4, Some(ClipSource::Pattern(4))));
        assert_eq!(listed(s), vec![1, 2, 3, 4, 5]);
        assert_eq!(crate::playlist::source(s), ClipSource::Pattern(4));
        // Without a name box (Ctrl+F4) alike.
        run(s, Cmd::FindNextNoName, (0.0, 0.0));
        assert_eq!((s.pattern, s.playlist.source), (6, Some(ClipSource::Pattern(6))));
        assert_eq!(listed(s), vec![1, 2, 3, 5, 6]);
    }

    #[test]
    fn clone_insert_delete_move_renumber_clips() {
        let mut app = app();
        let s = &mut app.state;
        s.pattern = 1;
        run(s, Cmd::Clone, (0.0, 0.0));
        assert_eq!(s.pattern, 2);
        assert_eq!(name_of(s, 2), "Kick #3");
        assert_eq!(name_of(s, 3), "Kick #2");
        assert_eq!(clip_pats(s), vec![1, 4]);
        // Insert one at 2: everything from 2 moves up.
        run(s, Cmd::InsertOne, (0.0, 0.0));
        s.pat_ui.name_box = None;
        assert_eq!(name_of(s, 3), "Kick #3");
        assert!(!s.project.pattern(2).is_some_and(used));
        assert_eq!(clip_pats(s), vec![1, 5]);
        // Delete the empty 2: later ones move down; the clip follows.
        run(s, Cmd::Delete, (0.0, 0.0));
        assert!(s.pat_ui.confirm);
        delete_current(s);
        assert_eq!(name_of(s, 2), "Kick #3");
        assert_eq!(clip_pats(s), vec![1, 4]);
        // Move down swaps with the next number.
        run(s, Cmd::MoveDown, (0.0, 0.0));
        assert_eq!((s.pattern, name_of(s, 3), name_of(s, 2)), (3, "Kick #3".into(), "Kick #2".into()));
        // Deleting a pattern removes its clips.
        s.pattern = 1;
        delete_current(s);
        assert_eq!(clip_pats(s), vec![3]);
    }

    // spec: ui/settings-choices.md "Hints and warnings: Manage messages and Project data size warning (2026-10-16)"
    /// Delete's confirmation has Remember my choice, off each time it opens. Ticked with Cancel nothing goes and the
    /// "Delete pattern" entry stays on; ticked with Ok the pattern goes and the entry turns off; while it is off,
    /// Delete deletes at once with no box.
    #[test]
    fn delete_confirmation_remembers_ok_only() {
        use crate::messages::{on, set, DELETE_PATTERN};
        use winit::keyboard::{Key, NamedKey};
        let mut app = app();
        let s = &mut app.state;
        s.pattern = 2;
        run(s, Cmd::Delete, (0.0, 0.0));
        assert!(s.pat_ui.confirm && !s.pat_ui.remember);
        hit(s, Hit::ConfirmRemember);
        assert!(s.pat_ui.remember);
        key(s, &Key::Named(NamedKey::Escape), false);
        assert_eq!((listed(s), on(&DELETE_PATTERN), s.pat_ui.confirm), (vec![1, 2, 3, 5], true, false), "Cancel");
        run(s, Cmd::Delete, (0.0, 0.0));
        assert!(s.pat_ui.confirm && !s.pat_ui.remember, "Remember my choice is off again");
        hit(s, Hit::ConfirmRemember);
        hit(s, Hit::ConfirmOk);
        assert_eq!((listed(s), on(&DELETE_PATTERN), s.pat_ui.confirm), (vec![1, 2, 4], false, false), "Ok");
        // Off: as if Ok was pressed.
        s.pattern = 1;
        run(s, Cmd::Delete, (0.0, 0.0));
        assert!(!s.pat_ui.confirm);
        assert_eq!((listed(s), clip_pats(s)), (vec![1, 3], vec![1]));
        set(&DELETE_PATTERN, true);
    }

    #[test]
    fn stepping_the_selector_adds_no_picker_rows() {
        let mut app = app();
        let s = &mut app.state;
        let before = crate::views::playlist::pick_items(s);
        s.pattern = 40;
        assert_eq!(crate::views::playlist::pick_items(s), before);
        assert_eq!(s.project.patterns.len(), 4);
        // The footer "+" = find first empty, which lists the pattern once it is named.
        run(s, Cmd::FindFirst, (0.0, 0.0));
        key(s, &winit::keyboard::Key::Character("Bass".into()), false);
        accept(s);
        assert_eq!(crate::views::playlist::pick_items(s).len(), before.len() + 1);
    }

    #[test]
    fn transpose_and_suggestions() {
        let mut app = app();
        let s = &mut app.state;
        s.pattern = 1;
        let k0 = s.project.pattern(1).unwrap().notes[0].key;
        run(s, Cmd::Transpose, (0.0, 0.0));
        key(s, &winit::keyboard::Key::Character("-".into()), false);
        key(s, &winit::keyboard::Key::Character("3".into()), false);
        accept(s);
        assert_eq!(s.project.pattern(1).unwrap().notes[0].key, k0 - 3);
        run(s, Cmd::Rename, (0.0, 0.0));
        suggest(s, 4, 3);
        assert_eq!(name_of(s, 1), "Chorus");
        assert!(s.pat_ui.name_box.is_none());
    }
}
