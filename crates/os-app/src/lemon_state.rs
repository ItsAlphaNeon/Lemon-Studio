// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Studio's own state saved in a `.lemon` file's `LMex` chunk, where an FLP has no place for it (Lemon Studio's
//! design, spec: flp/lemon-format.md "LMex"): the icons chosen with Change icon..., the effect slots' names and colours,
//! Performance mode's settings, the mixer's Plugin delay compensation ▸ Automatic and View ▸ checks, the plugin windows'
//! Scaling ▸, the channels' Show waveform preview and MIDI channel through checks, Set keyboard focus of all plugins
//! to ▸ and the playlist tracks' locks to their clips. Each kind is one event; its payload starts with a layout version
//! (1), and numbers are little-endian. Save writes them all (empty ones too, so a file saved again holds no stale
//! state); an FLP target drops them with the rest of `LMex`. Open reads them back from a `.lemon` file only; an FLP's
//! icons come from its icon numbers instead.

use crate::app::State;
use crate::playlist::{ICONS, IconOwner};
use crate::plugins::Plugins;
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

/// Mixer flags in [`lemon::MIXER`].
const PDC_AUTO: u8 = 1;
const WAVEFORMS: u8 = 2;
const EXTRA_PROPS: u8 = 4;

/// Channel flags in [`lemon::CHANNEL_CHECKS`].
const WAVE_PREVIEW: u8 = 1;
const MIDI_THROUGH: u8 = 2;

/// Clip sources in [`lemon::TRACK_LOCKS`].
const PATTERN: u8 = 0;
const CLIP_CHANNEL: u8 = 1;

/// The `LMex` events holding the Lemon Studio state of `s` and `plugins`, in ID order. Set keyboard focus of all
/// plugins to ▸ is written only where it works (Windows and macOS); elsewhere the file's own event is kept.
pub fn events(s: &State, plugins: &Plugins) -> Vec<Event> {
    let mut v = vec![icons(s), slot_names(s), slot_colors(s), performance(s), mixer(s), scaling(s, plugins), channel_checks(s)];
    #[cfg(any(windows, target_os = "macos"))]
    v.push(Event::new(lemon::KEY_FOCUS, Value::var([LAYOUT, u8::from(editor_keys())])));
    v.push(track_locks(s));
    v
}

/// The bytes [`events`] would write: a change to them is an unsaved change, as an edit of the project is. Set keyboard
/// focus of all plugins to ▸ is left out: it is a switch for the whole program, with no undo step, so it marks no
/// project changed.
pub fn saved_form(s: &State, plugins: &Plugins) -> Vec<u8> {
    let mut b = Vec::new();
    for e in events(s, plugins).into_iter().filter(|e| e.id != lemon::KEY_FOCUS) {
        b.push(e.id);
        b.extend_from_slice(e.data().unwrap_or_default());
    }
    b
}

/// Puts the state a `.lemon` file holds into `s` and `plugins` (a project just opened, its ids given). An FLP has
/// none; its channels and mixer tracks show the icons their numbers stand for instead ([`from_flp`]).
pub fn apply(s: &mut State, plugins: &mut Plugins, file: &FlpFile) {
    if !lemon::is_lemon(file) {
        from_flp(s);
        return;
    }
    let read = |id| lemon::data(file, id).filter(|d| d.first() == Some(&LAYOUT));
    if let Some(&flags) = read(lemon::MIXER).as_deref().and_then(|d| d.get(1)) {
        s.project.pdc_off = flags & PDC_AUTO == 0;
        s.mixer.waveforms = flags & WAVEFORMS != 0;
        s.mixer.extra = flags & EXTRA_PROPS != 0;
    }
    if let Some(d) = read(lemon::SCALING) {
        let mut r = Reader(&d[1..]);
        for _ in 0..r.u32().unwrap_or(0) {
            let (Some(kind), Some(n), Some(k), Some(pct)) = (r.u8(), r.u16(), r.u8(), r.u16()) else { break };
            let key = match kind {
                CHANNEL => usize::from(n),
                SLOT => crate::plugins::fx_key(n, k),
                _ => continue,
            };
            if pct > 0 {
                plugins.editor_scales.insert(key, f32::from(pct) / 100.0);
            }
        }
    }
    if let Some(d) = read(lemon::CHANNEL_CHECKS) {
        let mut r = Reader(&d[1..]);
        for _ in 0..r.u32().unwrap_or(0) {
            let (Some(n), Some(flags)) = (r.u16(), r.u8()) else { break };
            let Some(id) = s.project.channels.get(usize::from(n)).map(|c| c.id.0) else { continue };
            if flags & WAVE_PREVIEW != 0 {
                s.rack.wave_preview.insert(id);
            }
            if flags & MIDI_THROUGH == 0 {
                s.rack.through_off.insert(id);
            }
        }
    }
    #[cfg(any(windows, target_os = "macos"))]
    if let Some(&on) = read(lemon::KEY_FOCUS).as_deref().and_then(|d| d.get(1)) {
        set_editor_keys(on != 0);
    }
    if let Some(d) = read(lemon::TRACK_LOCKS) {
        let mut r = Reader(&d[1..]);
        for _ in 0..r.u32().unwrap_or(0) {
            let (Some(a), Some(t), Some(kind), Some(v)) = (r.u16(), r.u16(), r.u8(), r.u16()) else { break };
            let src = match kind {
                PATTERN => os_model::ClipSource::Pattern(v),
                CLIP_CHANNEL => os_model::ClipSource::Channel(v),
                _ => continue,
            };
            if let Some(tr) = s.project.arrangements.get_mut(usize::from(a)).and_then(|a| a.tracks.get_mut(usize::from(t))) {
                tr.clip_lock = Some(src);
            }
        }
    }
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

/// The icons of an opened FLP: each channel's and mixer track's icon number (events 155 and 95) that has a picture in
/// the Change icon... list ([`crate::playlist::FLP_ICONS`]). A `.lemon` file's icons are its own (event 193), so a
/// choice made in Lemon Studio, None too, wins over the numbers its FLP events keep.
fn from_flp(s: &mut State) {
    let p = &s.project;
    let channels = p.channels.iter().filter_map(|c| Some((IconOwner::Channel(c.id.0), crate::playlist::flp_icon(c.rack.icon)?)));
    let tracks = p.mixer.iter().filter_map(|m| Some((IconOwner::Track(m.index), crate::playlist::flp_icon(u32::from(m.icon?))?)));
    let found: Vec<_> = channels.chain(tracks).collect();
    s.playlist.icons.extend(found);
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

/// [`lemon::KEY_FOCUS`]: Set keyboard focus of all plugins to ▸ (`plugin_window::editor_keys`). The switch is one for
/// the whole program, so tests keep their own, one per test thread.
#[cfg(any(windows, target_os = "macos"))]
fn editor_keys() -> bool {
    #[cfg(test)]
    return tests::KEYS.with(|k| k.get());
    #[cfg(not(test))]
    crate::plugin_window::editor_keys()
}

#[cfg(any(windows, target_os = "macos"))]
fn set_editor_keys(on: bool) {
    #[cfg(test)]
    tests::KEYS.with(|k| k.set(on));
    #[cfg(not(test))]
    crate::plugin_window::set_editor_keys(on);
}

/// [`lemon::MIXER`]: one u8 of flags: 1 Plugin delay compensation ▸ Automatic, 2 View ▸ Waveforms, 4 View ▸ Extra
/// properties.
fn mixer(s: &State) -> Event {
    let flags = [(!s.project.pdc_off, PDC_AUTO), (s.mixer.waveforms, WAVEFORMS), (s.mixer.extra, EXTRA_PROPS)];
    Event::new(lemon::MIXER, Value::var([LAYOUT, flags.iter().filter(|(on, _)| *on).fold(0, |f, (_, bit)| f | bit)]))
}

/// [`lemon::SCALING`]: a u32 count, then per plugin window its owner's kind (u8: 0 a channel, 2 an effect slot, as in
/// [`lemon::ICONS`]), number (u16: the channel's rack position, or the mixer track), slot (u8, 0 for a channel) and
/// scale (u16 percent). Windows at Main are not listed; a container's members are not kept.
fn scaling(s: &State, plugins: &Plugins) -> Event {
    let mut v = Vec::new();
    for (&key, &k) in &plugins.editor_scales {
        let pct = (k * 100.0).round() as u16;
        match crate::plugins::fx_of(key) {
            Some((t, slot)) => v.push((SLOT, t, slot, pct)),
            None if key < s.project.channels.len() => v.push((CHANNEL, key as u16, 0, pct)),
            None => {}
        }
    }
    v.sort();
    let mut b = start(v.len());
    for (kind, n, slot, pct) in v {
        b.push(kind);
        b.extend_from_slice(&n.to_le_bytes());
        b.push(slot);
        b.extend_from_slice(&pct.to_le_bytes());
    }
    Event::new(lemon::SCALING, Value::var(b))
}

/// [`lemon::CHANNEL_CHECKS`]: a u32 count, then per channel in rack order its position (u16) and flags (u8: 1 Show
/// waveform preview, 2 MIDI channel through).
fn channel_checks(s: &State) -> Event {
    let mut b = start(s.project.channels.len());
    for (i, c) in s.project.channels.iter().enumerate() {
        let mut flags = 0;
        if s.rack.wave_preview.contains(&c.id.0) {
            flags |= WAVE_PREVIEW;
        }
        if !s.rack.through_off.contains(&c.id.0) {
            flags |= MIDI_THROUGH;
        }
        b.extend_from_slice(&(i as u16).to_le_bytes());
        b.push(flags);
    }
    Event::new(lemon::CHANNEL_CHECKS, Value::var(b))
}

/// [`lemon::TRACK_LOCKS`]: a u32 count, then per locked playlist track its arrangement (u16, 0-based), track (u16,
/// 0-based) and source (u8: 0 a pattern, 1 an audio or automation channel; u16: the 1-based pattern number or the
/// channel's rack position).
fn track_locks(s: &State) -> Event {
    let mut v = Vec::new();
    for (a, arr) in s.project.arrangements.iter().enumerate() {
        for (t, tr) in arr.tracks.iter().enumerate() {
            match tr.clip_lock {
                Some(os_model::ClipSource::Pattern(p)) => v.push((a as u16, t as u16, PATTERN, p)),
                Some(os_model::ClipSource::Channel(c)) => v.push((a as u16, t as u16, CLIP_CHANNEL, c)),
                None => {}
            }
        }
    }
    let mut b = start(v.len());
    for (a, t, kind, n) in v {
        b.extend_from_slice(&a.to_le_bytes());
        b.extend_from_slice(&t.to_le_bytes());
        b.push(kind);
        b.extend_from_slice(&n.to_le_bytes());
    }
    Event::new(lemon::TRACK_LOCKS, Value::var(b))
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

    std::thread_local! {
        /// This test thread's Set keyboard focus of all plugins to ▸ (`editor_keys`).
        pub(super) static KEYS: std::cell::Cell<bool> = const { std::cell::Cell::new(true) };
    }

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
        let mut want = vec![lemon::SAVED_BY, lemon::ICONS, lemon::SLOT_NAMES, lemon::SLOT_COLORS, lemon::PERFORMANCE, lemon::MIXER];
        want.extend([lemon::SCALING, lemon::CHANNEL_CHECKS]);
        #[cfg(any(windows, target_os = "macos"))]
        want.push(lemon::KEY_FOCUS);
        want.push(lemon::TRACK_LOCKS);
        assert_eq!(ids, want);
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

    /// `with_state`, plus the settings of events 197-201: Plugin delay compensation ▸ Automatic off, View ▸ Waveforms
    /// on and Extra properties off; Scaling ▸ 150 % on channel 0's window and 125 % on track 3's slot 2; Show waveform
    /// preview on channel 1 and MIDI channel through off on channel 2; tracks 3 and 4 of the first arrangement locked to
    /// pattern 1 and to channel 0.
    fn with_settings() -> App {
        let mut app = with_state();
        let s = &mut app.state;
        s.project.pdc_off = true;
        s.mixer.waveforms = true;
        s.mixer.extra = false;
        s.rack.wave_preview.insert(s.project.channels[1].id.0);
        s.rack.through_off.insert(s.project.channels[2].id.0);
        if s.project.arrangements.is_empty() {
            s.project.arrangements.push(os_flp::project::default_arrangement());
        }
        let tracks = &mut s.project.arrangements[0].tracks;
        tracks[3].clip_lock = Some(os_model::ClipSource::Pattern(1));
        tracks[4].clip_lock = Some(os_model::ClipSource::Channel(0));
        app.plugins.editor_scales.insert(0, 1.5);
        app.plugins.editor_scales.insert(crate::plugins::fx_key(3, 2), 1.25);
        app
    }

    /// Saved to a .lemon file and opened again, the settings of events 197-201 come back (the checks on the channels
    /// at the same rack positions), Set keyboard focus of all plugins to ▸ too; a project opened from an FLP has the
    /// defaults (and a Scaling ▸ of the project opened before is gone). A changed Scaling ▸ is an unsaved change; the
    /// keyboard focus switch is not.
    #[test]
    fn settings_round_trip_through_a_lemon_file() {
        let d = dir("settings");
        let (lemon_path, flp_path) = (d.join("Settings.lemon"), d.join("Settings.flp"));
        let mut app = with_settings();
        #[cfg(any(windows, target_os = "macos"))]
        set_editor_keys(false);
        app.save_to(lemon_path.clone());
        app.save_to(flp_path.clone());
        assert!(app.state.message.as_deref().is_some_and(|m| m.starts_with("Saved")), "{:?}", app.state.message);
        #[cfg(any(windows, target_os = "macos"))]
        set_editor_keys(true);
        let mut back = App::new(None);
        back.open(lemon_path.clone());
        #[cfg(any(windows, target_os = "macos"))]
        assert!(!editor_keys(), "Disabled comes back");
        assert!(!back.unsaved());
        let s = &back.state;
        assert!(s.project.pdc_off && s.mixer.waveforms && !s.mixer.extra);
        let ids: Vec<u32> = s.project.channels.iter().map(|c| c.id.0).collect();
        assert_eq!(s.rack.wave_preview.iter().copied().collect::<Vec<_>>(), [ids[1]]);
        assert_eq!(s.rack.through_off.iter().copied().collect::<Vec<_>>(), [ids[2]]);
        let locks: Vec<_> = s.project.arrangements[0].tracks.iter().enumerate().filter_map(|(t, tr)| Some((t, tr.clip_lock?))).collect();
        assert_eq!(locks, [(3, os_model::ClipSource::Pattern(1)), (4, os_model::ClipSource::Channel(0))]);
        let scales: Vec<(usize, f32)> = back.plugins.editor_scales.iter().map(|(&k, &v)| (k, v)).collect();
        assert_eq!(scales, [(0, 1.5), (crate::plugins::fx_key(3, 2), 1.25)]);
        back.plugins.editor_scales.remove(&0);
        assert!(back.unsaved(), "a Scaling ▸ change is an unsaved change");
        #[cfg(any(windows, target_os = "macos"))]
        {
            back.plugins.editor_scales.insert(0, 1.5);
            set_editor_keys(true);
            assert!(!back.unsaved(), "the keyboard focus switch is not");
        }
        // The FLP saved from the same state: no LMex, so the defaults (opened over the .lemon project).
        back.open(flp_path);
        let s = &back.state;
        assert!(!s.project.pdc_off && !s.mixer.waveforms && s.mixer.extra);
        assert!(s.rack.wave_preview.is_empty() && s.rack.through_off.is_empty() && back.plugins.editor_scales.is_empty());
        assert!(s.project.arrangements.iter().flat_map(|a| &a.tracks).all(|t| t.clip_lock.is_none()));
        #[cfg(any(windows, target_os = "macos"))]
        assert!(editor_keys());
        let _ = std::fs::remove_dir_all(&d);
    }

    /// An FLP's icon numbers (a channel's event 155, a mixer track's event 95) open as the icons with the same picture
    /// (`playlist::FLP_ICONS`), other numbers as none. Saved as .lemon, the FLP events keep the file's numbers and the
    /// icons come back from event 193, so one taken off in Lemon Studio stays off.
    #[test]
    fn flp_icon_numbers_open_as_icons() {
        let d = dir("flp_icons");
        let path = d.join("Icons.flp");
        let mut made = App::new(None);
        made.state.project = crate::app::test_project();
        made.state.control.settle(&mut made.state.project);
        let m = &mut made.state.project.mixer;
        m[2].icon = Some(75);
        m[3].icon = Some(74); // the automation clip's nodes: no picture of Lemon Studio's
        m[4].icon = Some(80);
        made.save_to(path.clone());
        let mut file = os_flp::FlpFile::parse(&std::fs::read(&path).unwrap()).unwrap();
        // The second channel's icon number, before its colour: 30, the microphone.
        let ev = file.chunks.iter_mut().find_map(|c| match &mut c.body {
            os_flp::ChunkBody::Events(v) => Some(v),
            _ => None,
        });
        let ev = ev.unwrap();
        let second = ev.iter().enumerate().filter(|(_, e)| e.id == 64).nth(1).unwrap().0;
        let color = second + ev[second..].iter().position(|e| e.id == 128).unwrap();
        ev.insert(color, Event::new(155, Value::U32(30)));
        std::fs::write(&path, file.to_bytes().unwrap()).unwrap();
        let mut app = App::new(None);
        app.open(path);
        let s = &app.state;
        let (first, second) = (s.project.channels[0].id.0, s.project.channels[1].id.0);
        assert_eq!(s.project.channels[1].rack.icon, 30);
        assert_eq!(s.playlist.icons.get(&IconOwner::Channel(second)), Some(&Icon::Mic));
        assert_eq!(s.playlist.icons.get(&IconOwner::Channel(first)), None);
        assert_eq!(s.playlist.icons.get(&IconOwner::Track(2)), Some(&Icon::Wave));
        assert_eq!(s.playlist.icons.get(&IconOwner::Track(4)), Some(&Icon::Kick));
        assert_eq!(s.playlist.icons.len(), 3, "74 has no picture here");
        assert_eq!(crate::playlist::window_icon(&s.project, &s.playlist.icons, 1), Some(Icon::Mic));
        assert!(!app.unsaved(), "the icons are the opened state");
        // Saved as .lemon with the track 2 icon taken off: the FLP events keep the original numbers.
        app.state.playlist.icons.remove(&IconOwner::Track(2));
        let lemon_path = d.join("Icons.lemon");
        app.save_to(lemon_path.clone());
        let saved = os_flp::project::load(&os_flp::FlpFile::parse(&std::fs::read(&lemon_path).unwrap()).unwrap()).unwrap();
        assert_eq!((saved.channels[1].rack.icon, saved.mixer[2].icon, saved.mixer[3].icon), (30, Some(75), Some(74)));
        let mut back = App::new(None);
        back.open(lemon_path);
        let s = &back.state;
        assert_eq!(s.playlist.icons.get(&IconOwner::Channel(s.project.channels[1].id.0)), Some(&Icon::Mic));
        assert_eq!(s.playlist.icons.get(&IconOwner::Track(2)), None, "event 193 holds the choice, not event 95");
        assert_eq!(s.playlist.icons.len(), 2);
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
        apply(&mut s.state, &mut s.plugins, &f);
        assert_eq!(s.state.mixer.slot_colors.len(), 1);
        assert_eq!(s.state.mixer.slot_colors.get(&(2, 0)), Some(&os_model::Rgb::new(10, 200, 30)));
        let _ = std::fs::remove_dir_all(&d);
    }
}
