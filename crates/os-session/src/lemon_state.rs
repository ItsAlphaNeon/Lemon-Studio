// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Studio's own state saved in a `.lemon` file's `LMex` chunk, where an FLP has no place for it (Lemon Studio's
//! design, spec: flp/lemon-format.md "LMex"): the icons chosen with Change icon..., the effect slots' names and colours,
//! Performance mode's settings, the mixer's Plugin delay compensation ▸ Automatic and View ▸ checks, the plugin windows'
//! Scaling ▸, the channels' Show waveform preview and MIDI channel through checks, Set keyboard focus of all plugins
//! to ▸, the playlist tracks' locks to their clips, the Receive notes from ▸ locks, and the features the project uses
//! that a version without them cannot play (`FEATURES`: a version opening a file that names one it does not know
//! says which parts it does not play). Each kind is one event; its
//! payload starts with a layout version (1), and numbers are little-endian. Save writes them all (empty ones too, so a
//! file saved again holds no stale state); an FLP target drops them with the rest of `LMex`. Open reads them back from
//! a `.lemon` file only; an FLP's icons come from its icon numbers instead.

#![allow(deprecated)] // writes the project directly (`crate::locked`; control-plane step 14 removes this)
use std::collections::BTreeSet;

use crate::plugins::Plugins;
use crate::icon::ICONS;
use os_flp::{Event, FlpFile, Value, lemon};

/// The payload layout written (the first byte of each event). A reader ignores an event with a layout it does not know.
pub const LAYOUT: u8 = 1;

/// Icon owner kinds in [`lemon::ICONS`].
const CHANNEL: u8 = 0;
const TRACK: u8 = 1;
const SLOT: u8 = 2;

/// Performance flags in [`lemon::PERFORMANCE`].
pub const CLIP_PROGRESS: u8 = 1;
pub const TRACK_PROGRESS: u8 = 2;
pub const PLUS_SCENE: u8 = 4;
pub const VELOCITY_SUBCLIPS: u8 = 8;

/// Mixer flags in [`lemon::MIXER`].
const PDC_AUTO: u8 = 1;
pub const WAVEFORMS: u8 = 2;
pub const EXTRA_PROPS: u8 = 4;

/// Channel flags in [`lemon::CHANNEL_CHECKS`].
pub const WAVE_PREVIEW: u8 = 1;
pub const MIDI_THROUGH: u8 = 2;

/// Clip sources in [`lemon::TRACK_LOCKS`].
const PATTERN: u8 = 0;
const CLIP_CHANNEL: u8 = 1;

/// The features a project can use that a Lemon Studio without them cannot play: one is named here when it is added,
/// and [`used`] names it for a project that uses it. A build that opens a file naming one it does not know says so
/// (`unknown_features`). None yet.
pub const FEATURES: &[&str] = &[];

/// The features of [`FEATURES`] that `s` uses.
fn used(_s: &crate::session::Session) -> Vec<&'static str> {
    Vec::new()
}

/// The features `file` names that this version does not know: the parts of the project it does not play.
pub fn unknown_features(file: &os_flp::FlpFile) -> Vec<String> {
    let Some(d) = lemon::data(file, lemon::FEATURES).filter(|d| d.first() == Some(&LAYOUT)) else { return Vec::new() };
    let mut r = Reader(&d[1..]);
    let mut out = Vec::new();
    for _ in 0..r.u32().unwrap_or(0) {
        let Some(len) = r.u16() else { break };
        let Some(name) = r.take(usize::from(len)) else { break };
        let name = String::from_utf8_lossy(name).into_owned();
        if !FEATURES.contains(&name.as_str()) && !out.contains(&name) {
            out.push(name);
        }
    }
    out
}

/// [`lemon::FEATURES`]: a u32 count, then each name (u16 length, UTF-8). The features `s` uses, and those its file
/// names that this version does not know: their parts are kept through a save, so the names stay too.
fn features(s: &crate::session::Session) -> Event {
    let mut names: Vec<String> = used(s).into_iter().map(String::from).collect();
    if let Some(f) = &s.file {
        names.extend(unknown_features(f));
    }
    let mut b = start(names.len());
    for name in &names {
        let name = &name.as_bytes()[..name.len().min(usize::from(u16::MAX))];
        b.extend_from_slice(&(name.len() as u16).to_le_bytes());
        b.extend_from_slice(name);
    }
    Event::new(lemon::FEATURES, Value::var(b))
}

/// What of a `.lemon` file's own state is the interface's (`events_in`): its views' choices.
pub struct Looks<'a> {
    /// The mixer's View ▸ Waveforms and Extra properties.
    pub waveforms: bool,
    pub extra: bool,
    /// The channels (by id) that show a waveform preview, and those with MIDI channel through off.
    pub wave_preview: &'a BTreeSet<u32>,
    pub through_off: &'a BTreeSet<u32>,
    /// The playlist's Performance flags ([`lemon::PERFORMANCE`]).
    pub performance: u8,
    /// The rack's note locks (`rack::NoteSrc` order).
    pub note_lock: [Option<usize>; 2],
    /// TOOLS > Set keyboard focus of all plugins to ▸ (saved on Windows and macOS, where it applies).
    pub editor_keys: bool,
}

static NO_CHANNELS: BTreeSet<u32> = BTreeSet::new();

impl Looks<'static> {
    /// Without an interface (`--serve`): the choices an interface starts with.
    pub fn none() -> Looks<'static> {
        Looks { waveforms: false, extra: true, wave_preview: &NO_CHANNELS, through_off: &NO_CHANNELS, performance: CLIP_PROGRESS | VELOCITY_SUBCLIPS, note_lock: [None; 2], editor_keys: true }
    }
}

/// `events` on the session, with the interface's choices `looks`.
pub fn events_in(s: &crate::session::Session, plugins: &Plugins, looks: &Looks) -> Vec<Event> {
    let mut v = vec![icons(s), slot_names(s), slot_colors(s), performance(looks), mixer(s, looks), scaling(s, plugins), channel_checks(s, looks)];
    #[cfg(any(windows, target_os = "macos"))]
    v.push(Event::new(lemon::KEY_FOCUS, Value::var([LAYOUT, u8::from(looks.editor_keys)])));
    v.push(track_locks(s));
    v.push(crate::clip_reverse::event(&s.project));
    v.push(note_locks(looks));
    v.push(features(s));
    v
}

/// `saved_form` on the session, with the interface's choices `looks`.
pub fn saved_form_in(s: &crate::session::Session, plugins: &Plugins, looks: &Looks) -> Vec<u8> {
    let mut b = Vec::new();
    for e in events_in(s, plugins, looks).into_iter().filter(|e| e.id != lemon::KEY_FOCUS) {
        b.push(e.id);
        b.extend_from_slice(e.data().unwrap_or_default());
    }
    b
}

/// `apply`, the session's half: what of a `.lemon` file's own state belongs to the project and its plugins (plugin
/// delay compensation, the editors' scaling, the playlist tracks' locks, icons, slot names and colours), or for an FLP
/// the icons its numbers stand for.
pub fn apply_session(s: &mut crate::session::Session, file: &FlpFile) {
    if !lemon::is_lemon(file) {
        from_flp(s);
        return;
    }
    let read = |id| lemon::data(file, id).filter(|d| d.first() == Some(&LAYOUT));
    if let Some(&flags) = read(lemon::MIXER).as_deref().and_then(|d| d.get(1)) {
        s.project.get_mut().pdc_off = flags & PDC_AUTO == 0;
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
                s.plugins.editor_scales.insert(key, f32::from(pct) / 100.0);
            }
        }
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
            if let Some(tr) = s.project.get_mut().arrangements.get_mut(usize::from(a)).and_then(|a| a.tracks.get_mut(usize::from(t))) {
                tr.clip_lock = Some(src);
            }
        }
    }
    if let Some(d) = read(lemon::CLIP_PROPS) {
        crate::clip_reverse::read(s.project.get_mut(), &d[1..]);
    }
    if let Some(d) = read(lemon::ICONS) {
        read_icons(s, &d[1..]);
    }
    if let Some(d) = read(lemon::SLOT_NAMES) {
        let mut r = Reader(&d[1..]);
        for _ in 0..r.u32().unwrap_or(0) {
            let (Some(t), Some(k), Some(len)) = (r.u16(), r.u8(), r.u16()) else { break };
            let Some(text) = r.take(usize::from(len)) else { break };
            if let Some(v) = s.project.get_mut().mixer.iter_mut().find(|m| m.index == t).and_then(|m| m.slot_names.get_mut(usize::from(k))) {
                *v = Some(String::from_utf8_lossy(text).into_owned());
            }
        }
    }
    if let Some(d) = read(lemon::SLOT_COLORS) {
        let mut r = Reader(&d[1..]);
        for _ in 0..r.u32().unwrap_or(0) {
            let (Some(t), Some(k), Some(c)) = (r.u16(), r.u8(), r.take(3)) else { break };
            if let Some(v) = s.project.get_mut().mixer.iter_mut().find(|m| m.index == t).and_then(|m| m.slot_colors.get_mut(usize::from(k))) {
                *v = Some(os_model::Rgb::new(c[0], c[1], c[2]));
            }
        }
    }
}

/// The icons of an opened FLP: each channel's and mixer track's icon number (events 155 and 95) that has a picture in
/// the Change icon... list ([`crate::edit::playlist::FLP_ICONS`]). A `.lemon` file's icons are its own (event 193), so a
/// choice made in Lemon Studio, None too, wins over the numbers its FLP events keep.
fn from_flp(s: &mut crate::session::Session) {
    let name = |i: crate::icon::Icon| ICONS.iter().find(|(_, x)| *x == i).map(|&(n, _)| n.to_string());
    s.project.write_if(|p| {
        let mut changed = false;
        for c in &mut p.channels {
            if let Some(n) = crate::edit::playlist::flp_icon(c.rack.icon).and_then(name) {
                c.lemon_icon = Some(n);
                changed = true;
            }
        }
        for m in &mut p.mixer {
            if let Some(n) = m.icon.and_then(|i| crate::edit::playlist::flp_icon(u32::from(i))).and_then(name) {
                m.lemon_icon = Some(n);
                changed = true;
            }
        }
        changed
    });
}

/// [`lemon::ICONS`]: a u32 count, then per icon its owner's kind (u8: 0 a channel, 1 a mixer track, 2 an effect slot),
/// number (u16: the channel's position in the rack, or the mixer track) and slot (u8, 0 unless an effect slot), and the
/// icon's name (u8 length, UTF-8; the names of the Change icon... menu). A channel is kept by its position, since the
/// file holds the channels in rack order and ids are not saved.
fn icons(s: &crate::session::Session) -> Event {
    let p = &s.project;
    // Only names this version knows (as the menu offers them).
    fn known(n: &Option<String>) -> Option<&str> {
        n.as_deref().filter(|n| ICONS.iter().any(|(x, _)| x == n))
    }
    let mut v: Vec<(u8, u16, u8, &str)> = Vec::new();
    for (i, c) in p.channels.iter().enumerate() {
        v.extend(known(&c.lemon_icon).map(|n| (CHANNEL, i as u16, 0, n)));
    }
    for m in &p.mixer {
        v.extend(known(&m.lemon_icon).map(|n| (TRACK, m.index, 0, n)));
        for (k, icon) in m.slot_icons.iter().enumerate() {
            v.extend(known(icon).map(|n| (SLOT, m.index, k as u8, n)));
        }
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

fn read_icons(s: &mut crate::session::Session, d: &[u8]) {
    let mut r = Reader(d);
    for _ in 0..r.u32().unwrap_or(0) {
        let (Some(kind), Some(n), Some(k), Some(len)) = (r.u8(), r.u16(), r.u8(), r.u8()) else { break };
        let Some(name) = r.take(usize::from(len)) else { break };
        // A name this version does not know (one added later) is left out.
        let Some(&(name, _)) = ICONS.iter().find(|(nm, _)| nm.as_bytes() == name) else { continue };
        let p = s.project.get_mut();
        let to = match kind {
            CHANNEL => p.channels.get_mut(usize::from(n)).map(|c| &mut c.lemon_icon),
            TRACK => p.mixer.iter_mut().find(|m| m.index == n).map(|m| &mut m.lemon_icon),
            SLOT => p.mixer.iter_mut().find(|m| m.index == n).and_then(|m| m.slot_icons.get_mut(usize::from(k))),
            _ => continue,
        };
        if let Some(to) = to {
            *to = Some(name.to_string());
        }
    }
}

/// [`lemon::SLOT_NAMES`]: a u32 count, then per slot its mixer track (u16), slot (u8) and name (u16 length, UTF-8).
fn slot_names(s: &crate::session::Session) -> Event {
    let mut v: Vec<((u16, u8), &String)> = Vec::new();
    for m in &s.project.mixer {
        v.extend(m.slot_names.iter().enumerate().filter_map(|(k, n)| Some(((m.index, k as u8), n.as_ref()?))));
    }
    v.sort();
    let mut b = start(v.len());
    for ((t, k), name) in v {
        let name = &name.as_bytes()[..name.len().min(usize::from(u16::MAX))];
        b.extend_from_slice(&t.to_le_bytes());
        b.push(k);
        b.extend_from_slice(&(name.len() as u16).to_le_bytes());
        b.extend_from_slice(name);
    }
    Event::new(lemon::SLOT_NAMES, Value::var(b))
}

/// [`lemon::SLOT_COLORS`]: a u32 count, then per slot its mixer track (u16), slot (u8) and colour (u8 red, green, blue).
pub fn slot_colors(s: &crate::session::Session) -> Event {
    let mut v: Vec<((u16, u8), &os_model::Rgb)> = Vec::new();
    for m in &s.project.mixer {
        v.extend(m.slot_colors.iter().enumerate().filter_map(|(k, c)| Some(((m.index, k as u8), c.as_ref()?))));
    }
    v.sort_by_key(|(k, _)| *k);
    let mut b = start(v.len());
    for ((t, k), c) in v {
        b.extend_from_slice(&t.to_le_bytes());
        b.extend_from_slice(&[k, c.r, c.g, c.b]);
    }
    Event::new(lemon::SLOT_COLORS, Value::var(b))
}

/// [`lemon::PERFORMANCE`]: one u8 of flags: 1 Performance clip progress, 2 Performance track progress, 4 Ctrl+click
/// triggers +Scene, 8 Velocity picks sub-clips.
pub fn performance(looks: &Looks) -> Event {
    Event::new(lemon::PERFORMANCE, Value::var([LAYOUT, looks.performance]))
}

/// [`lemon::MIXER`]: one u8 of flags: 1 Plugin delay compensation ▸ Automatic, 2 View ▸ Waveforms, 4 View ▸ Extra
/// properties.
pub fn mixer(s: &crate::session::Session, looks: &Looks) -> Event {
    let flags = [(!s.project.pdc_off, PDC_AUTO), (looks.waveforms, WAVEFORMS), (looks.extra, EXTRA_PROPS)];
    Event::new(lemon::MIXER, Value::var([LAYOUT, flags.iter().filter(|(on, _)| *on).fold(0, |f, (_, bit)| f | bit)]))
}

/// [`lemon::SCALING`]: a u32 count, then per plugin window its owner's kind (u8: 0 a channel, 2 an effect slot, as in
/// [`lemon::ICONS`]), number (u16: the channel's rack position, or the mixer track), slot (u8, 0 for a channel) and
/// scale (u16 percent). Windows at Main are not listed; a container's members are not kept.
fn scaling(s: &crate::session::Session, plugins: &Plugins) -> Event {
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
fn channel_checks(s: &crate::session::Session, looks: &Looks) -> Event {
    let mut b = start(s.project.channels.len());
    for (i, c) in s.project.channels.iter().enumerate() {
        let mut flags = 0;
        if looks.wave_preview.contains(&c.id.0) {
            flags |= WAVE_PREVIEW;
        }
        if !looks.through_off.contains(&c.id.0) {
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
fn track_locks(s: &crate::session::Session) -> Event {
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

/// [`lemon::NOTE_LOCKS`]: a u32 count, then per locked note source its kind (u8: 0 the computer keyboard, 1 the Touch
/// controller, as `rack::NoteSrc` orders them) and the channel's rack position (u16).
fn note_locks(looks: &Looks) -> Event {
    let locks: Vec<(u8, u16)> = looks.note_lock.iter().enumerate().filter_map(|(src, row)| Some((src as u8, (*row)? as u16))).collect();
    let mut b = start(locks.len());
    for (src, row) in locks {
        b.push(src);
        b.extend_from_slice(&row.to_le_bytes());
    }
    Event::new(lemon::NOTE_LOCKS, Value::var(b))
}

/// A payload's layout version and entry count.
pub fn start(count: usize) -> Vec<u8> {
    let mut b = vec![LAYOUT];
    b.extend_from_slice(&(count as u32).to_le_bytes());
    b
}

/// Reads a payload front to back; `None` once it runs out (a cut entry and the ones after it are left out).
pub struct Reader<'a>(pub &'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let (head, rest) = self.0.split_at_checked(n)?;
        self.0 = rest;
        Some(head)
    }

    pub fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|b| b[0])
    }

    pub fn u16(&mut self) -> Option<u16> {
        self.take(2).map(|b| u16::from_le_bytes([b[0], b[1]]))
    }

    pub fn u32(&mut self) -> Option<u32> {
        self.take(4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
}

#[cfg(any(test, feature = "test-util"))]
mod tests {


    fn dir(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("os_lemon_state_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

}
