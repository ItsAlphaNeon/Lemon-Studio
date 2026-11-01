// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Studio's own state saved in a `.lemon` file's `LMex` chunk, where an FLP has no place for it (Lemon Studio's
//! design, spec: flp/lemon-format.md "LMex"): the icons chosen with Change icon..., the effect slots' names and colours,
//! and Performance mode's settings. Each kind is one event; its payload starts with a layout version (1), and numbers
//! are little-endian. Save writes all four (empty ones too, so a file saved again holds no stale state); an FLP target
//! drops them with the rest of `LMex`. Open reads them back from a `.lemon` file only.

use crate::app::State;
use crate::playlist::{ICONS, IconOwner};
use os_flp::{Event, FlpFile, Value, lemon};

/// The payload layout written (the first byte of each event). A reader ignores an event with a layout it does not know.
const LAYOUT: u8 = 1;

/// Icon owner kinds in [`lemon::ICONS`].
const CHANNEL: u8 = 0;
const TRACK: u8 = 1;
const SLOT: u8 = 2;

/// Performance flags in [`lemon::PERFORMANCE`].
const CLIP_PROGRESS: u8 = 1;
const TRACK_PROGRESS: u8 = 2;
const PLUS_SCENE: u8 = 4;
const VELOCITY_SUBCLIPS: u8 = 8;

/// The `LMex` events holding `s`'s Lemon Studio state, in ID order.
pub fn events(s: &State) -> Vec<Event> {
    vec![icons(s), slot_names(s), slot_colors(s), performance(s)]
}

/// The bytes [`events`] would write: a change to them is an unsaved change, as an edit of the project is.
pub fn saved_form(s: &State) -> Vec<u8> {
    let mut b = Vec::new();
    for e in events(s) {
        b.push(e.id);
        b.extend_from_slice(e.data().unwrap_or_default());
    }
    b
}

/// Puts the state a `.lemon` file holds into `s` (a project just opened, its ids given). An FLP has none.
pub fn apply(s: &mut State, file: &FlpFile) {
    if !lemon::is_lemon(file) {
        return;
    }
    let read = |id| lemon::data(file, id).filter(|d| d.first() == Some(&LAYOUT));
    if let Some(d) = read(lemon::ICONS) {
        read_icons(s, &d[1..]);
    }
    if let Some(d) = read(lemon::SLOT_NAMES) {
        let mut r = Reader(&d[1..]);
        for _ in 0..r.u32().unwrap_or(0) {
            let (Some(t), Some(k), Some(len)) = (r.u16(), r.u8(), r.u16()) else { break };
            let Some(text) = r.take(usize::from(len)) else { break };
            s.mixer.slot_names.insert((t, k), String::from_utf8_lossy(text).into_owned());
        }
    }
    if let Some(d) = read(lemon::SLOT_COLORS) {
        let mut r = Reader(&d[1..]);
        for _ in 0..r.u32().unwrap_or(0) {
            let (Some(t), Some(k), Some(c)) = (r.u16(), r.u8(), r.take(3)) else { break };
            s.mixer.slot_colors.insert((t, k), os_model::Rgb::new(c[0], c[1], c[2]));
        }
    }
    if let Some(&flags) = read(lemon::PERFORMANCE).as_deref().and_then(|d| d.get(1)) {
        let pl = &mut s.playlist;
        pl.perf_clip_progress = flags & CLIP_PROGRESS != 0;
        pl.perf_track_progress = flags & TRACK_PROGRESS != 0;
        pl.perf_plus_scene = flags & PLUS_SCENE != 0;
        pl.perf_velocity_subclips = flags & VELOCITY_SUBCLIPS != 0;
    }
}

/// [`lemon::ICONS`]: a u32 count, then per icon its owner's kind (u8: 0 a channel, 1 a mixer track, 2 an effect slot),
/// number (u16: the channel's position in the rack, or the mixer track) and slot (u8, 0 unless an effect slot), and the
/// icon's name (u8 length, UTF-8; the names of the Change icon... menu). A channel is kept by its position, since the
/// file holds the channels in rack order and ids are not saved.
fn icons(s: &State) -> Event {
    let mut v: Vec<(u8, u16, u8, &str)> = Vec::new();
    for (owner, icon) in &s.playlist.icons {
        let Some(&(name, _)) = ICONS.iter().find(|(_, i)| i == icon) else { continue };
        let at = match *owner {
            IconOwner::Channel(id) => match s.project.channels.iter().position(|c| c.id.0 == id) {
                Some(i) => (CHANNEL, i as u16, 0),
                None => continue,
            },
            IconOwner::Track(t) => (TRACK, t, 0),
            IconOwner::Slot(t, k) => (SLOT, t, k),
        };
        v.push((at.0, at.1, at.2, name));
    }
    v.sort();
    let mut b = start(v.len());
    for (kind, n, k, name) in v {
        b.push(kind);
        b.extend_from_slice(&n.to_le_bytes());
        b.push(k);
        b.push(name.len() as u8);
        b.extend_from_slice(name.as_bytes());
    }
    Event::new(lemon::ICONS, Value::var(b))
}

fn read_icons(s: &mut State, d: &[u8]) {
    let mut r = Reader(d);
    for _ in 0..r.u32().unwrap_or(0) {
        let (Some(kind), Some(n), Some(k), Some(len)) = (r.u8(), r.u16(), r.u8(), r.u8()) else { break };
        let Some(name) = r.take(usize::from(len)) else { break };
        // A name this version does not know (one added later) is left out.
        let Some(&(_, icon)) = ICONS.iter().find(|(nm, _)| nm.as_bytes() == name) else { continue };
        let owner = match kind {
            CHANNEL => match s.project.channels.get(usize::from(n)) {
                Some(c) => IconOwner::Channel(c.id.0),
                None => continue,
            },
            TRACK => IconOwner::Track(n),
            SLOT => IconOwner::Slot(n, k),
            _ => continue,
        };
        s.playlist.icons.insert(owner, icon);
    }
}

/// [`lemon::SLOT_NAMES`]: a u32 count, then per slot its mixer track (u16), slot (u8) and name (u16 length, UTF-8).
fn slot_names(s: &State) -> Event {
    let mut v: Vec<_> = s.mixer.slot_names.iter().collect();
    v.sort();
    let mut b = start(v.len());
    for (&(t, k), name) in v {
        let name = &name.as_bytes()[..name.len().min(usize::from(u16::MAX))];
        b.extend_from_slice(&t.to_le_bytes());
        b.push(k);
        b.extend_from_slice(&(name.len() as u16).to_le_bytes());
        b.extend_from_slice(name);
    }
    Event::new(lemon::SLOT_NAMES, Value::var(b))
}

/// [`lemon::SLOT_COLORS`]: a u32 count, then per slot its mixer track (u16), slot (u8) and colour (u8 red, green, blue).
fn slot_colors(s: &State) -> Event {
    let mut v: Vec<_> = s.mixer.slot_colors.iter().collect();
    v.sort_by_key(|(k, _)| **k);
    let mut b = start(v.len());
    for (&(t, k), c) in v {
        b.extend_from_slice(&t.to_le_bytes());
        b.extend_from_slice(&[k, c.r, c.g, c.b]);
    }
    Event::new(lemon::SLOT_COLORS, Value::var(b))
}

/// [`lemon::PERFORMANCE`]: one u8 of flags: 1 Performance clip progress, 2 Performance track progress, 4 Ctrl+click
/// triggers +Scene, 8 Velocity picks sub-clips.
fn performance(s: &State) -> Event {
    let pl = &s.playlist;
    let flags = [
        (pl.perf_clip_progress, CLIP_PROGRESS),
        (pl.perf_track_progress, TRACK_PROGRESS),
        (pl.perf_plus_scene, PLUS_SCENE),
        (pl.perf_velocity_subclips, VELOCITY_SUBCLIPS),
    ]
    .iter()
    .filter(|(on, _)| *on)
    .fold(0, |f, (_, bit)| f | bit);
    Event::new(lemon::PERFORMANCE, Value::var([LAYOUT, flags]))
}

/// A payload's layout version and entry count.
fn start(count: usize) -> Vec<u8> {
    let mut b = vec![LAYOUT];
    b.extend_from_slice(&(count as u32).to_le_bytes());
    b
}

/// Reads a payload front to back; `None` once it runs out (a cut entry and the ones after it are left out).
struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let (head, rest) = self.0.split_at_checked(n)?;
        self.0 = rest;
        Some(head)
    }

    fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|b| b[0])
    }

    fn u16(&mut self) -> Option<u16> {
        self.take(2).map(|b| u16::from_le_bytes([b[0], b[1]]))
    }

    fn u32(&mut self) -> Option<u32> {
        self.take(4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::icons::Icon;

    fn dir(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("os_lemon_state_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// An app with every kind of state set: icons on the second channel, mixer track 2 and its slot 1; slot 1's name
    /// and slot 0's colour on track 2; Performance settings other than the defaults.
    fn with_state() -> App {
        let mut app = App::new(None);
        app.state.project = crate::app::test_project();
        app.state.control.settle(&mut app.state.project);
        let ch = app.state.project.channels[1].id.0;
        let s = &mut app.state;
        s.playlist.icons.insert(IconOwner::Channel(ch), Icon::Mic);
        s.playlist.icons.insert(IconOwner::Track(2), Icon::Star);
        s.playlist.icons.insert(IconOwner::Slot(2, 1), Icon::Plug);
        s.mixer.slot_names.insert((2, 1), "Crunch ✓".into());
        s.mixer.slot_colors.insert((2, 0), os_model::Rgb::new(10, 200, 30));
        s.playlist.perf_clip_progress = false;
        s.playlist.perf_track_progress = true;
        s.playlist.perf_plus_scene = true;
        s.playlist.perf_velocity_subclips = false;
        app
    }

    /// Saved to a .lemon file and opened again, each kind of state comes back: the channel's icon on the channel at
    /// the same rack position (ids are new), the track's and the slot's, the slot's name and colour, the Performance
    /// settings. Changing one of them is an unsaved change.
    #[test]
    fn state_round_trips_through_a_lemon_file() {
        let d = dir("round");
        let path = d.join("State.lemon");
        let mut app = with_state();
        app.save_to(path.clone());
        assert!(!app.unsaved());
        let file = os_flp::FlpFile::parse(&std::fs::read(&path).unwrap()).unwrap();
        let ids: Vec<u8> = lemon::events(&file).unwrap().iter().map(|e| e.id).collect();
        assert_eq!(ids, vec![lemon::SAVED_BY, lemon::ICONS, lemon::SLOT_NAMES, lemon::SLOT_COLORS, lemon::PERFORMANCE]);
        let mut back = App::new(None);
        back.open(path.clone());
        assert!(!back.unsaved(), "the state read back is the saved state");
        let s = &back.state;
        let ch = s.project.channels[1].id.0;
        assert_eq!(s.playlist.icons.get(&IconOwner::Channel(ch)), Some(&Icon::Mic));
        assert_eq!(s.playlist.icons.get(&IconOwner::Track(2)), Some(&Icon::Star));
        assert_eq!(s.playlist.icons.get(&IconOwner::Slot(2, 1)), Some(&Icon::Plug));
        assert_eq!(s.playlist.icons.len(), 3);
        assert_eq!(s.mixer.slot_names.get(&(2, 1)).map(String::as_str), Some("Crunch ✓"));
        assert_eq!(s.mixer.slot_colors.get(&(2, 0)), Some(&os_model::Rgb::new(10, 200, 30)));
        let pl = &s.playlist;
        assert_eq!((pl.perf_clip_progress, pl.perf_track_progress, pl.perf_plus_scene, pl.perf_velocity_subclips), (false, true, true, false));
        back.state.playlist.icons.remove(&IconOwner::Track(2));
        assert!(back.unsaved(), "an icon taken off is an unsaved change");
        // Saved again, what was taken off stays off (the events are replaced, not kept from the opened file).
        back.save_to(path.clone());
        let mut again = App::new(None);
        again.open(path);
        assert_eq!(again.state.playlist.icons.len(), 2);
        assert!(!again.state.playlist.icons.contains_key(&IconOwner::Track(2)));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A target named .flp has no place for the state: the save succeeds, the file is a plain FLP, and opening it gives
    /// the defaults.
    #[test]
    fn an_flp_target_drops_the_state() {
        let d = dir("flp");
        let path = d.join("State.flp");
        let mut app = with_state();
        app.save_to(path.clone());
        assert!(app.state.message.as_deref().is_some_and(|m| m.starts_with("Saved")), "{:?}", app.state.message);
        let file = os_flp::FlpFile::parse(&std::fs::read(&path).unwrap()).unwrap();
        assert!(!lemon::is_lemon(&file) && lemon::events(&file).unwrap().is_empty());
        let mut back = App::new(None);
        back.open(path);
        let s = &back.state;
        assert!(s.playlist.icons.is_empty() && s.mixer.slot_names.is_empty() && s.mixer.slot_colors.is_empty());
        let fresh = crate::playlist::PlaylistState::default();
        assert_eq!((s.playlist.perf_clip_progress, s.playlist.perf_velocity_subclips), (fresh.perf_clip_progress, fresh.perf_velocity_subclips));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// An `LMex` event this version does not know is kept through open and save; one of ours with a later layout is
    /// left unread, and a payload cut short keeps the entries before the cut.
    #[test]
    fn unknown_events_and_layouts() {
        let d = dir("unknown");
        let path = d.join("State.lemon");
        let mut app = with_state();
        app.save_to(path.clone());
        let mut file = os_flp::FlpFile::parse(&std::fs::read(&path).unwrap()).unwrap();
        let future = Event::new(240, Value::var([7, 7]));
        let later = Event::new(lemon::PERFORMANCE, Value::var([2, 0]));
        lemon::to_lemon(&mut file, &[future.clone(), later]).unwrap();
        std::fs::write(&path, file.to_bytes().unwrap()).unwrap();
        let mut back = App::new(None);
        back.open(path.clone());
        assert!(back.state.playlist.perf_clip_progress, "a layout 2 Performance event is not read: the defaults stay");
        assert_eq!(back.state.playlist.icons.len(), 3);
        back.save_to(path.clone());
        let file = os_flp::FlpFile::parse(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(lemon::data(&file, 240), Some(vec![7, 7]), "the unknown event is kept");
        // Two colours, the second cut short: the first is read.
        let mut s = with_state();
        s.state.mixer.slot_colors.insert((3, 4), os_model::Rgb::new(1, 2, 3));
        let mut b = slot_colors(&s.state).data().unwrap().to_vec();
        b.truncate(b.len() - 1);
        s.state.mixer.slot_colors.clear();
        let mut f = file.clone();
        lemon::to_lemon(&mut f, &[Event::new(lemon::SLOT_COLORS, Value::var(b))]).unwrap();
        apply(&mut s.state, &f);
        assert_eq!(s.state.mixer.slot_colors.len(), 1);
        assert_eq!(s.state.mixer.slot_colors.get(&(2, 0)), Some(&os_model::Rgb::new(10, 200, 30)));
        let _ = std::fs::remove_dir_all(&d);
    }
}
