// Copyright (C) 2026 ItsAlphaNeon
// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Pattern selector and PATTERNS menu commands: find empty, rename / colour, clone, insert, delete, move,
//! transpose, plus the name box and the delete confirmation.
// spec: ui/pattern-menu.md

use os_model::{ClipSource, Rgb};
#[cfg(test)]
use os_model::Pattern;
pub use crate::edit::patterns::*;

use crate::app::State;

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
    /// Quick render as audio clip and Render and replace: run by `App::pattern_cmd` (spec: ui/pattern-render.md).
    QuickRender,
    RenderReplace,
    /// Render as audio clip...: the Export form in its render mode first.
    RenderAsClip,
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

/// Numbers of the listed patterns, ascending: the used ones, those placed in the playlist, and the playlist's clip
/// source (a new pattern, empty until notes are added).
pub fn listed(s: &State) -> Vec<u16> {
    let mut v: Vec<u16> = s.session.project.patterns.iter().filter(|p| used(p)).map(|p| p.number).collect();
    v.extend(crate::edit::playlist::clips(&s.session).iter().filter_map(|c| if let ClipSource::Pattern(n) = c.source { Some(n) } else { None }));
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

/// Renumber patterns (`None` removes one): playlist clips follow (clips of removed patterns go). Numbers that
/// end up without a pattern keep an empty entry so saving clears their blocks.
pub fn renumber(s: &mut State, map: impl Fn(u16) -> Option<u16>) {
    crate::follow::around(s, |s| renumber_in(&mut s.session, map));
}

/// Open the name box for pattern `n` at `at`.
pub fn open_name_box(s: &mut State, target: BoxTarget, at: (f32, f32)) {
    let text = match target {
        BoxTarget::Name(n) => name_of(&s.session, n),
        BoxTarget::Transpose(_) => String::new(),
        BoxTarget::Rack(b) => crate::rack::box_text(s, b),
    };
    s.pat_ui.name_box = Some(NameBox { target, text, fresh: true, at, suggestions: false, palette: false, created: false });
}

/// Where a box opened from a menu / key goes: the mouse over its colour tab (§4).
pub fn box_at_mouse(mouse: (f32, f32)) -> (f32, f32) {
    ((mouse.0 - 241.0).max(0.0), (mouse.1 - 41.0).max(0.0))
}

/// Pattern `n` picked in the pattern selector (its menu, a drag or the wheel): it becomes the current pattern and the
/// playlist's clip source, so Draw and Paint place it, as FL does (issue #94).
pub fn select(s: &mut State, n: u16) {
    s.set_pattern(n);
    s.playlist.source = Some(ClipSource::Pattern(n));
}

/// Box position for "+" (§4).
pub const PLUS_BOX_AT: (f32, f32) = (949.0, 46.0);

/// Run a command; `at` is where a name box opens. Returns a status message. The commands that change the project are
/// commands of the control plane, one undo step each.
pub fn run(s: &mut State, cmd: Cmd, at: (f32, f32)) -> Option<String> {
    run_now(s, cmd, at)
}

use crate::control::command::Command;

/// The patterns have their ids, for commands (a pattern just made gets its own on the next tick).
fn pattern_ids(s: &mut State) {
    if s.session.project.patterns.iter().any(|p| p.id.is_none()) {
        crate::control_ui::settle_ids(s);
    }
}

/// Pattern `n`'s id, when the project has it.
fn pat_id(s: &State, n: u16) -> Option<u32> {
    s.session.project.pattern(n).map(|p| p.id.0)
}

/// Pattern `n` named `name` and / or coloured `color` (as the commands take them): its `set_pattern`, or an
/// `add_pattern` at `n` when the project has no pattern there yet.
fn set_pattern(s: &State, n: u16, name: Option<String>, color: Option<String>) -> Command {
    match pat_id(s, n) {
        Some(pattern) => Command::SetPattern { pattern, name, color, length: None },
        None => Command::AddPattern { name, color, number: Some(n) },
    }
}

/// The interface's pattern edit made of `commands`: one undo step named `name`; the error as the message.
fn apply(s: &mut State, name: &'static str, commands: Vec<Command>) -> Option<String> {
    pattern_ids(s);
    crate::control_ui::command(s, name, None, commands).err()
}

/// The undo step of a pattern's deletion.
const DELETE: &str = "Delete pattern";

fn run_now(s: &mut State, cmd: Cmd, at: (f32, f32)) -> Option<String> {
    let cur = s.pattern().max(1);
    match cmd {
        Cmd::FindFirst => {
            s.set_pattern(first_empty(&s.session));
            open_name_box(s, BoxTarget::Name(s.pattern()), at);
            created_box(s);
        }
        Cmd::FindNext => {
            s.set_pattern(next_empty(&s.session, cur));
            open_name_box(s, BoxTarget::Name(s.pattern()), at);
            created_box(s);
        }
        Cmd::FindNextNoName => {
            s.set_pattern(next_empty(&s.session, cur));
            s.playlist.source = Some(ClipSource::Pattern(s.pattern()));
        }
        Cmd::SelectInPlaylist => {
            let sel: Vec<usize> = crate::edit::playlist::clips(&s.session)
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
            return Some(format!("Selected {n} clip(s) of {}", name_of(&s.session, cur)));
        }
        Cmd::Rename => open_name_box(s, BoxTarget::Name(cur), at),
        Cmd::ChangeColor => {
            open_name_box(s, BoxTarget::Name(cur), at);
            if let Some(b) = s.pat_ui.name_box.as_mut() {
                b.palette = true;
            }
        }
        Cmd::RandomColor => {
            let seed = crate::clip_colors::clock();
            pattern_ids(s);
            let c = set_pattern(s, cur, None, crate::control::command::color_text(Some(random_color(seed ^ u64::from(cur)))));
            return apply(s, "Pattern color", vec![c]);
        }
        Cmd::Transpose => open_name_box(s, BoxTarget::Transpose(cur), at),
        Cmd::InsertOne => {
            if let Some(e) = apply(s, "Insert pattern", vec![Command::ShiftPatterns { from: cur, by: 1 }]) {
                return Some(e);
            }
            open_name_box(s, BoxTarget::Name(cur), at);
            created_box(s);
        }
        Cmd::Clone => {
            if cur >= MAX {
                return None;
            }
            pattern_ids(s);
            let c = match pat_id(s, cur) {
                Some(pattern) => vec![Command::ClonePattern { pattern }],
                // An empty number: its copy is an empty pattern with the clone's name.
                None => vec![Command::ShiftPatterns { from: cur + 1, by: 1 }, Command::AddPattern { name: Some(clone_name(&s.session, cur)), color: None, number: Some(cur + 1) }],
            };
            if let Some(e) = apply(s, "Clone pattern", c) {
                return Some(e);
            }
            s.set_pattern(cur + 1);
        }
        // Its Manage messages entry off: as if Ok was pressed.
        Cmd::Delete if !crate::messages::on(&crate::messages::DELETE_PATTERN) => return delete_pattern(s),
        Cmd::Delete => open_confirm(s),
        Cmd::MoveUp if cur > 1 => return move_pattern(s, cur, cur - 1),
        Cmd::MoveDown if cur < MAX => return move_pattern(s, cur, cur + 1),
        Cmd::MoveUp | Cmd::MoveDown => {}
        Cmd::SplitByChannel => return split_by_channel(s),
        // Dialogs, the browser and the renders: `App::pattern_cmd` runs them.
        Cmd::TimeSignature | Cmd::OpenInBrowser | Cmd::QuickRender | Cmd::RenderReplace | Cmd::RenderAsClip => {}
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
pub fn split_by_channel(s: &mut State) -> Option<String> {
    pattern_ids(s);
    let pattern = pat_id(s, s.pattern().max(1))?;
    apply(s, "Split by instrument", vec![Command::SplitPattern { pattern }])
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
    s.message = delete_pattern(s).or(s.message.take());
}

/// Cancel in the delete confirmation: nothing is deleted and Remember my choice is not kept.
fn cancel_confirm(s: &mut State) {
    s.pat_ui.confirm = false;
    s.pat_ui.remember = false;
    s.rack.pending_delete = None;
}

/// Deletes the current pattern: later ones move down a number, its clips go. One undo step.
fn delete_pattern(s: &mut State) -> Option<String> {
    let cur = s.pattern().max(1);
    pattern_ids(s);
    let c = match pat_id(s, cur) {
        Some(pattern) => Command::DeletePattern { pattern },
        None => Command::ShiftPatterns { from: cur + 1, by: -1 },
    };
    apply(s, DELETE, vec![c])
}

/// Move up / down: the current pattern takes number `to`, the one there its number; `to` becomes current.
fn move_pattern(s: &mut State, cur: u16, to: u16) -> Option<String> {
    if let Some(e) = apply(s, "Move pattern", vec![Command::MovePattern { from: cur, to }]) {
        return Some(e);
    }
    s.set_pattern(to);
    None
}

/// Accept the name box.
pub fn accept(s: &mut State) {
    let Some(b) = s.pat_ui.name_box.as_ref() else { return };
    if let BoxTarget::Name(n) | BoxTarget::Transpose(n) = b.target {
        return accept_pattern(s, n);
    }
    // The rack's boxes apply their commands (`rack::box_accept`); filter groups (Group selected, Add and Rename filter
    // group) are no undo step, as in FL (spec: ui/undo-steps.md).
    accept_now(s);
}

/// The name box of pattern `n` accepted: its name, or its transposition.
fn accept_pattern(s: &mut State, n: u16) {
    let Some(b) = s.pat_ui.name_box.take() else { return };
    pattern_ids(s);
    match b.target {
        BoxTarget::Name(_) => {
            let text = b.text.trim().to_string();
            let named = !text.is_empty() && text != format!("Pattern {n}");
            if named || pat_id(s, n).is_some() {
                let c = set_pattern(s, n, Some(if named { text.clone() } else { String::new() }), None);
                if let Some(e) = apply(s, "Pattern name", vec![c]) {
                    s.message = Some(e);
                }
            }
            if b.created {
                s.playlist.source = Some(ClipSource::Pattern(n));
            }
            if !text.is_empty() {
                s.pat_ui.recent.retain(|r| *r != text);
                s.pat_ui.recent.insert(0, text);
                s.pat_ui.recent.truncate(RECENT);
            }
        }
        _ => {
            if let (Ok(v), Some(pattern)) = (b.text.trim().trim_start_matches('+').parse::<i32>(), pat_id(s, n)) {
                if let Some(e) = apply(s, "Transpose", vec![Command::TransposePattern { pattern, semitones: v, channels: Vec::new() }]) {
                    s.message = Some(e);
                }
            }
        }
    }
}

fn accept_now(s: &mut State) {
    let Some(b) = s.pat_ui.name_box.take() else { return };
    match b.target {
        BoxTarget::Name(n) => {
            let text = b.text.trim().to_string();
            let name = (!text.is_empty() && text != format!("Pattern {n}")).then_some(text.clone());
            if name.is_some() || s.session.project.pattern(n).is_some() {
                pat_mut(&mut s.session, n).name = name;
            }
            if b.created {
                s.playlist.source = Some(ClipSource::Pattern(n));
            }
            if !text.is_empty() {
                s.pat_ui.recent.retain(|r| *r != text);
                s.pat_ui.recent.insert(0, text);
                s.pat_ui.recent.truncate(RECENT);
            }
            s.session.dirty = true;
        }
        BoxTarget::Transpose(n) => {
            if let Ok(v) = b.text.trim().trim_start_matches('+').parse::<i32>() {
                transpose(&mut s.session, n, v, &[]);
            }
        }
        BoxTarget::Rack(rb) => crate::rack::box_accept(s, rb, &b.text),
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
    let col = SWATCHES.get(i).map(|&c| Rgb::new((c >> 16) as u8, (c >> 8) as u8, c as u8));
    match s.pat_ui.name_box.as_ref().map(|b| b.target) {
        Some(BoxTarget::Name(n)) => {
            if let Some(col) = col {
                pattern_ids(s);
                let cmd = set_pattern(s, n, None, crate::control::command::color_text(Some(col)));
                if let Some(e) = apply(s, "Pattern color", vec![cmd]) {
                    s.message = Some(e);
                }
            }
        }
        Some(BoxTarget::Rack(rb @ (crate::rack::RBox::Channel(_) | crate::rack::RBox::ColorSel))) => {
            if let Some(col) = col {
                if let Some(e) = crate::rack::box_color(s, rb, col.for_channel()) {
                    s.message = Some(e);
                }
            }
        }
        _ => return,
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
pub fn key(s: &mut State, key: &crate::keys::Key, ctrl: bool) -> bool {
    use crate::keys::{Key, NamedKey};
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
        app.state.session.project = crate::locked::Locked::new(crate::projects::test_project());
        let s = &mut app.state;
        s.session.project.get_mut().patterns = vec![
            Pattern { number: 1, name: Some("Kick".into()), notes: vec![Note::step(0, 0)], ..Pattern::default() },
            Pattern { number: 2, name: Some("Kick #2".into()), notes: vec![Note::step(0, 48)], ..Pattern::default() },
            Pattern { number: 3, notes: vec![Note::step(1, 0)], ..Pattern::default() },
            Pattern { number: 5, name: Some("Lead".into()), ..Pattern::default() },
        ];
        crate::edit::playlist::arr_mut(&mut s.session).clips = vec![Clip::new(0, 384, 0, ClipSource::Pattern(1)), Clip::new(0, 384, 1, ClipSource::Pattern(3))];
        app
    }

    fn clip_pats(s: &State) -> Vec<u16> {
        crate::edit::playlist::clips(&s.session).iter().filter_map(|c| if let ClipSource::Pattern(n) = c.source { Some(n) } else { None }).collect()
    }

    #[test]
    fn empty_numbers_and_listing() {
        let mut app = app();
        let s = &mut app.state;
        assert_eq!(listed(s), vec![1, 2, 3, 5]);
        assert_eq!(first_empty(&s.session), 4);
        s.set_pattern(5);
        assert_eq!(next_empty(&s.session, 5), 6);
        run(s, Cmd::FindFirst, (0.0, 0.0));
        assert_eq!(s.pattern(), 4);
        // Cancel keeps the number current but unlisted; accepting a name lists it.
        s.pat_ui.name_box = None;
        assert_eq!(listed(s), vec![1, 2, 3, 5]);
        run(s, Cmd::Rename, (0.0, 0.0));
        key(s, &crate::keys::Key::Character("Probe".into()), false);
        accept(s);
        assert_eq!(listed(s), vec![1, 2, 3, 4, 5]);
        assert_eq!(name_of(&s.session, 4), "Probe");
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
        assert_eq!((s.pattern(), s.playlist.source), (4, Some(ClipSource::Pattern(4))));
        assert_eq!(listed(s), vec![1, 2, 3, 4, 5]);
        assert_eq!(crate::playlist::source(s), ClipSource::Pattern(4));
        // Without a name box (Ctrl+F4) alike.
        run(s, Cmd::FindNextNoName, (0.0, 0.0));
        assert_eq!((s.pattern(), s.playlist.source), (6, Some(ClipSource::Pattern(6))));
        assert_eq!(listed(s), vec![1, 2, 3, 5, 6]);
    }

    #[test]
    fn clone_insert_delete_move_renumber_clips() {
        let mut app = app();
        let s = &mut app.state;
        s.set_pattern(1);
        run(s, Cmd::Clone, (0.0, 0.0));
        assert_eq!(s.pattern(), 2);
        assert_eq!(name_of(&s.session, 2), "Kick #3");
        assert_eq!(name_of(&s.session, 3), "Kick #2");
        assert_eq!(clip_pats(s), vec![1, 4]);
        // Insert one at 2: everything from 2 moves up.
        run(s, Cmd::InsertOne, (0.0, 0.0));
        s.pat_ui.name_box = None;
        assert_eq!(name_of(&s.session, 3), "Kick #3");
        assert!(!s.session.project.pattern(2).is_some_and(used));
        assert_eq!(clip_pats(s), vec![1, 5]);
        // Delete the empty 2: later ones move down; the clip follows.
        run(s, Cmd::Delete, (0.0, 0.0));
        assert!(s.pat_ui.confirm);
        delete_current(s);
        assert_eq!(name_of(&s.session, 2), "Kick #3");
        assert_eq!(clip_pats(s), vec![1, 4]);
        // Move down swaps with the next number.
        run(s, Cmd::MoveDown, (0.0, 0.0));
        assert_eq!((s.pattern(), name_of(&s.session, 3), name_of(&s.session, 2)), (3, "Kick #3".into(), "Kick #2".into()));
        // Deleting a pattern removes its clips.
        s.set_pattern(1);
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
        use crate::keys::{Key, NamedKey};
        let mut app = app();
        let s = &mut app.state;
        s.set_pattern(2);
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
        s.set_pattern(1);
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
        s.set_pattern(40);
        assert_eq!(crate::views::playlist::pick_items(s), before);
        assert_eq!(s.session.project.patterns.len(), 4);
        // The footer "+" = find first empty, which lists the pattern once it is named.
        run(s, Cmd::FindFirst, (0.0, 0.0));
        key(s, &crate::keys::Key::Character("Bass".into()), false);
        accept(s);
        assert_eq!(crate::views::playlist::pick_items(s).len(), before.len() + 1);
    }

    #[test]
    fn transpose_and_suggestions() {
        let mut app = app();
        let s = &mut app.state;
        s.set_pattern(1);
        let k0 = s.session.project.pattern(1).unwrap().notes[0].key;
        run(s, Cmd::Transpose, (0.0, 0.0));
        key(s, &crate::keys::Key::Character("-".into()), false);
        key(s, &crate::keys::Key::Character("3".into()), false);
        accept(s);
        assert_eq!(s.session.project.pattern(1).unwrap().notes[0].key, k0 - 3);
        run(s, Cmd::Rename, (0.0, 0.0));
        suggest(s, 4, 3);
        assert_eq!(name_of(&s.session, 1), "Chorus");
        assert!(s.pat_ui.name_box.is_none());
    }
}
