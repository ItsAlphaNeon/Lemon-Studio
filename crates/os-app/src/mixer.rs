// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Mixer behaviour: track selection, mute / solo, routing and send levels, the track / options / layout menus,
//! adding / moving / deleting tracks, and the effect slots that host plugins.
//!
// spec: ui/mixer.md (every interaction), flp/events/mixer.md (what is stored), audio/gain-laws.md (fader law)

use std::collections::HashMap;

use os_model::{MixerTrack, Rgb};

use crate::app::{App, State};
use crate::knobs::KnobId;
use crate::playlist::{Cmd, EditTarget, Mark, PItem, Popup};

/// Mixer controls (the view registers them as `Action::Mix`). Controls that are not available yet (title-bar view
/// buttons, input pin, latency clock, disk recording, input / output combos, track equalizer) are greyed out with
/// `Ui::off` instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MHit {
    /// Anywhere on a strip that is not a control.
    Strip(u16),
    Led(u16),
    Polarity(u16),
    Swap(u16),
    /// The ▲ "route to this track" button.
    Route(u16),
    /// The small green "^" above a send knob (removes that route).
    Unroute(u16),
    /// "+" after the last strip; title-bar "+" (after the selected track).
    Add,
    AddAfter,
    Options,
    Layout,
    /// Effect slot `n` of the current track: name, ▸ arrow, LED.
    SlotName(u8),
    SlotArrow(u8),
    SlotLed(u8),
    /// The level scale column.
    Scale,
    /// The current track's equaliser graph: a press grabs the band nearest the cursor's frequency.
    EqGraph,
}

/// Menu commands (`Cmd::Mixer`).
#[derive(Debug, Clone, PartialEq)]
pub enum MCmd {
    Rename(u16),
    Color(u16, Option<Rgb>),
    RandomColor(u16),
    Reset(u16),
    ResetRouting(u16),
    UnrouteTo(u16),
    Solo(u16),
    /// Solo with the tracks routed into it too (spec: ui/mixer-track-commands.md §5).
    AltSolo(u16),
    /// The rack's selected channels to `dst`, or (`spread`) to `dst`, `dst` + 1, … (§2).
    RouteChannels { dst: u16, spread: bool },
    /// Select ▸ (§3): the channels using the tracks, the tracks they send to, the tracks sending to them, all.
    SelectChannels(u16),
    SelectDest(u16),
    SelectSources(u16),
    SelectAll,
    /// Trim unused tracks (§4): delete the inserts no channel uses (`non_empty`: also those holding effects).
    Trim { non_empty: bool },
    MoveLeft(u16),
    MoveRight(u16),
    Delete(u16),
    InsertOne(u16),
    /// Route the selected tracks to `dst`: only = drop their other routes, side = sidechain (level 0).
    Route { dst: u16, only: bool, side: bool },
    /// Layout menu: Wide (the only layout drawn; the others are greyed out).
    Layout(u8),
    /// Options ▸ Cable physics: open / close the cables' debug panel (Lemon Studio's own).
    CablePhysics,
    /// General settings ▸ Animations: simulate the cables (true) or hang them still; kept for the next start.
    Animations(bool),
    /// Effect slots of the current track.
    AddEffect(u8, usize),
    SlotDelete(u8),
    SlotMove(u8, i8),
    SlotRename(u8),
    SlotView(u8),
}

/// What the plugin host does next tick for a mixer slot (`App::mixer_tick`).
#[derive(Debug, Clone, PartialEq)]
pub enum FxReq {
    Create { track: u16, slot: u8, index: usize },
    Toggle { track: u16, slot: u8 },
    Show { track: u16, slot: u8 },
    Delete { track: u16, slot: u8 },
    /// Slots moved inside a track or tracks moved: `map[(t, s)] = (t2, s2)`; unlisted instances are dropped.
    Remap(Vec<((u16, u8), (u16, u8))>),
}

pub const LAYOUTS: [&str; 8] = ["Compact", "Compact 2", "Compact alt", "Wide", "Wide 2", "Wide 3", "Wide alt", "Extra large"];
/// Index of "Wide" (the only layout drawn).
pub const WIDE: u8 = 3;

/// Mixer state that is not project data.
pub struct MixerUi {
    /// Selected tracks besides the current one (Ctrl+click, §2).
    pub sel: Vec<u16>,
    pub layout: u8,
    /// Slot names typed in the rename box (session only).
    pub slot_names: HashMap<(u16, u8), String>,
    pub pending: Vec<FxReq>,
    /// Meter state per track (`meters::Meter`), fed by `feed_meters`.
    pub meters: Vec<crate::meters::Meter>,
    /// Meter ballistics and look.
    pub meter_cfg: crate::meters::Settings,
    /// When the meters last advanced.
    pub meter_at: Option<std::time::Instant>,
    /// The last solo: the tracks it kept on and the states (track, enabled) from before the first solo, put back by
    /// unsolo when "Restore previous state after solo" is on.
    pub solo: Option<(Vec<u16>, Vec<(u16, bool)>)>,
}

impl Default for MixerUi {
    fn default() -> Self {
        MixerUi { sel: Vec::new(), layout: WIDE, slot_names: HashMap::new(), pending: Vec::new(), meters: Vec::new(),
            meter_cfg: Default::default(), meter_at: None, solo: None }
    }
}

/// Highest insert a channel can target (at least 1).
pub fn max_insert(p: &os_model::Project) -> u16 {
    max_insert_of(&p.mixer)
}

pub fn max_insert_of(mixer: &[MixerTrack]) -> u16 {
    (mixer.len().saturating_sub(2) as u16).max(1)
}

/// Inserts shown: stored tracks minus the master and the last (hidden) one (§1).
pub fn inserts(s: &State) -> u16 {
    s.project.mixer.len().saturating_sub(2) as u16
}

pub fn track(s: &State, n: u16) -> Option<&MixerTrack> {
    s.project.mixer.iter().find(|m| m.index == n)
}

pub(crate) fn track_mut(s: &mut State, n: u16) -> Option<&mut MixerTrack> {
    s.project.mixer.iter_mut().find(|m| m.index == n)
}

/// "Master", "Insert 3", or the track's own name.
pub fn name(s: &State, n: u16) -> String {
    track(s, n).and_then(|m| m.name.clone()).filter(|n| !n.is_empty()).unwrap_or_else(|| if n == 0 { "Master".into() } else { format!("Insert {n}") })
}

/// The current track first, then the other selected ones.
pub fn selected(s: &State) -> Vec<u16> {
    let mut v = vec![s.selected_mixer];
    v.extend(s.mixer.sel.iter().copied().filter(|&t| t != s.selected_mixer));
    v
}

pub fn is_selected(s: &State, n: u16) -> bool {
    s.selected_mixer == n || s.mixer.sel.contains(&n)
}

/// Whether track `src` sends to `dst` (route present), and at what level (fader scale, 0.8 = unity).
pub fn send(s: &State, src: u16, dst: u16) -> Option<f64> {
    let t = track(s, src)?;
    t.routes.contains(&dst).then(|| t.send_levels.iter().find(|(d, _)| *d == dst).map_or(0.8, |&(_, l)| l))
}

/// Whether any track routes into `n`.
pub fn receives(s: &State, n: u16) -> bool {
    s.project.mixer.iter().any(|m| m.index != n && m.routes.contains(&n))
}

/// The slot's shown name (renamed, else the plugin's name).
pub fn slot_name(s: &State, t: u16, slot: u8) -> Option<String> {
    let p = track(s, t)?.slots.get(usize::from(slot))?.as_ref()?;
    Some(s.mixer.slot_names.get(&(t, slot)).cloned().unwrap_or_else(|| crate::project_plugins::slot_title(p)))
}

/// "Volume: 100%  0.0dB  1.00" (§4); `label` = "Volume" or "To Insert 3".
pub fn level_text(label: &str, v: f64) -> String {
    let g = os_engine::laws::fader_gain(v);
    let db = if g > 0.0 { format!("{:.1}dB", 20.0 * g.log10()) } else { "-INFdB".into() };
    format!("{label}: {:.0}%  {db}  {g:.2}", v / 0.8 * 100.0)
}

/// "Stereo separation: Original" / "40% separated" / "40% merged" (§4). A positive stored value merges towards mono
/// (spec: audio/mixer.md "Fader matrix"); which way FL's knob turns for it is not checked.
pub fn sep_text(sep: f64) -> String {
    let pct = (sep.abs() * 100.0).round();
    if pct == 0.0 {
        "Stereo separation: Original".into()
    } else if sep > 0.0 {
        format!("Stereo separation: {pct:.0}% merged")
    } else {
        format!("Stereo separation: {pct:.0}% separated")
    }
}

/// Would a route `src` -> `dst` make a loop (dst already reaches src)?
fn makes_loop(s: &State, src: u16, dst: u16) -> bool {
    let mut seen = vec![dst];
    let mut i = 0;
    while i < seen.len() {
        let t = seen[i];
        if t == src {
            return true;
        }
        if let Some(m) = track(s, t) {
            for &r in &m.routes {
                if !seen.contains(&r) {
                    seen.push(r);
                }
            }
        }
        i += 1;
    }
    false
}

fn changed(s: &mut State) {
    s.piano.dirty = true;
}

/// Selects `n` as a click with these modifiers does (§2): plain = only it, Ctrl = add it.
pub fn select(s: &mut State, n: u16, ctrl: bool) {
    if ctrl {
        let cur = s.selected_mixer;
        if !s.mixer.sel.contains(&cur) {
            s.mixer.sel.push(cur);
        }
        if !s.mixer.sel.contains(&n) {
            s.mixer.sel.push(n);
        }
    } else {
        s.mixer.sel.clear();
    }
    s.selected_mixer = n;
}

/// Left click (or press) on a mixer control.
pub fn hit(app: &mut App, h: MHit) -> Option<String> {
    let (ctrl, shift) = (app.ctrl, app.shift);
    let at = app.mouse;
    let s = &mut app.state;
    match h {
        MHit::Strip(n) => {
            select(s, n, ctrl);
            if shift {
                start_rename(s, n, at);
            }
            None
        }
        MHit::Led(n) => {
            select(s, n, ctrl);
            let m = track_mut(s, n)?;
            m.enabled = !m.enabled;
            changed(s);
            None
        }
        MHit::Polarity(n) => {
            select(s, n, ctrl);
            let m = track_mut(s, n)?;
            m.polarity = !m.polarity;
            changed(s);
            None
        }
        MHit::Swap(n) => {
            select(s, n, ctrl);
            let m = track_mut(s, n)?;
            m.swap = !m.swap;
            changed(s);
            None
        }
        MHit::Route(dst) => route(s, dst, ctrl, shift),
        MHit::Unroute(dst) => {
            let cur = s.selected_mixer;
            let m = track_mut(s, cur)?;
            m.routes.retain(|&r| r != dst);
            m.send_levels.retain(|&(d, _)| d != dst);
            changed(s);
            Some(format!("{} no longer sends to {}", name(s, cur), name(s, dst)))
        }
        MHit::Add => {
            let n = add_track(s, None);
            Some(format!("Added {}", name(s, n)))
        }
        MHit::AddAfter => {
            let n = add_track(s, Some(s.selected_mixer + 1));
            Some(format!("Added {}", name(s, n)))
        }
        MHit::Options => {
            s.playlist.popup = Some(options_menu(s, at.0, at.1 + 10.0));
            None
        }
        MHit::Layout => {
            s.playlist.popup = Some(layout_menu(s, at.0, at.1 + 10.0));
            None
        }
        MHit::SlotName(i) => {
            let t = s.selected_mixer;
            if slot_name(s, t, i).is_some() {
                s.mixer.pending.push(FxReq::Toggle { track: t, slot: i });
            } else {
                s.playlist.popup = Some(effect_menu(s, i, at.0, at.1));
            }
            None
        }
        MHit::SlotArrow(i) => {
            let t = s.selected_mixer;
            s.playlist.popup = Some(if slot_name(s, t, i).is_some() { slot_menu(s, i, at.0, at.1) } else { effect_menu(s, i, at.0, at.1) });
            None
        }
        MHit::SlotLed(i) => {
            let t = s.selected_mixer;
            let m = track_mut(s, t)?;
            let k = usize::from(i);
            m.slot_on[k] = !m.slot_on[k];
            changed(s);
            None
        }
        MHit::Scale | MHit::EqGraph => None,
    }
}

// The equaliser graph `g` spans the frequency knob's range left to right (10 Hz .. 16 kHz) and ±18 dB over its
// half-height less 4 px (spec: ui/mixer.md §8).

/// The frequency position 0..1 at `x`.
pub fn eq_graph_pos(g: os_ui::Rect, x: f32) -> f32 {
    ((x - g.x - 1.0) / (g.w - 2.0)).clamp(0.0, 1.0)
}

/// The gain at `y`, in dB.
pub fn eq_graph_db(g: os_ui::Rect, y: f32) -> f32 {
    ((g.cy() - y) / (g.h / 2.0 - 4.0) * 18.0).clamp(-18.0, 18.0)
}

/// The band of the current track a press at `x` tweaks: the one whose frequency is nearest.
pub fn eq_band_at(s: &State, g: os_ui::Rect, x: f32) -> u8 {
    let (t, p) = (s.selected_mixer, eq_graph_pos(g, x));
    let dist = |b: u8| (s.knob(KnobId::Eq(t, b, 1)) - p).abs();
    (0..3).min_by(|&a, &b| dist(a).total_cmp(&dist(b))).unwrap_or(0)
}

/// The graph's hint at `x`: "1278Hz  Tweak band 2" (spec: ui/mixer.md §8).
pub fn eq_graph_hint(s: &State, g: os_ui::Rect, x: f32) -> String {
    let hz = os_dsp::mixer::eq_frequency((eq_graph_pos(g, x) * 65536.0).round() as i32);
    format!("{hz:.0}Hz  Tweak band {}", eq_band_at(s, g, x) + 1)
}

/// Dragging on the graph: band `band` of track `t` moves to the cursor, its frequency across and its gain up and down.
pub fn eq_graph_drag(s: &mut State, t: u16, band: u8, g: os_ui::Rect, (x, y): (f32, f32)) {
    s.set_knob(KnobId::Eq(t, band, 1), eq_graph_pos(g, x));
    s.set_knob(KnobId::Eq(t, band, 0), (eq_graph_db(g, y) / 18.0 + 1.0) / 2.0);
}

/// Right click on a mixer control. Returns false when it is not handled here (knob menus).
pub fn right(app: &mut App, h: MHit) -> bool {
    let at = app.mouse;
    let s = &mut app.state;
    match h {
        MHit::Strip(n) => {
            if !is_selected(s, n) {
                select(s, n, false);
            }
            s.playlist.popup = Some(track_menu(s, n, at.0, at.1));
        }
        MHit::Add => s.playlist.popup = Some(track_menu(s, s.selected_mixer, at.0, at.1)),
        MHit::Led(n) => solo(s, n),
        MHit::Route(dst) => {
            let items = vec![
                PItem::new("Route to this track", Cmd::Mixer(MCmd::Route { dst, only: false, side: false })),
                PItem::new("Route to this track only", Cmd::Mixer(MCmd::Route { dst, only: true, side: false })),
                PItem::sep(),
                PItem::new("Sidechain to this track", Cmd::Mixer(MCmd::Route { dst, only: false, side: true })),
                PItem::new("Sidechain to this track only", Cmd::Mixer(MCmd::Route { dst, only: true, side: true })),
            ];
            s.playlist.popup = Some(Popup { x: at.0, y: at.1, items, sub: None });
        }
        // The slot's plugin menu, like a left click (spec: ui/plugin-menu.md §1).
        MHit::SlotArrow(i) => {
            let t = s.selected_mixer;
            s.playlist.popup = Some(if slot_name(s, t, i).is_some() { slot_menu(s, i, at.0, at.1) } else { effect_menu(s, i, at.0, at.1) });
        }
        MHit::SlotName(i) => {
            let t = s.selected_mixer;
            if slot_name(s, t, i).is_some() {
                s.mixer.pending.push(FxReq::Show { track: t, slot: i });
            } else {
                s.playlist.popup = Some(effect_menu(s, i, at.0, at.1));
            }
        }
        MHit::SlotLed(i) => {
            // Solo the slot: the track's other slots are bypassed (§7).
            let t = s.selected_mixer;
            if let Some(m) = track_mut(s, t) {
                let only = (0..10).all(|k| m.slot_on[k] == (k == usize::from(i)));
                for k in 0..10 {
                    m.slot_on[k] = only || k == usize::from(i);
                }
                changed(s);
            }
        }
        MHit::Polarity(n) | MHit::Swap(n) => select(s, n, false),
        _ => return false,
    }
    true
}

/// Middle click: rename a filled slot (§7).
pub fn middle(app: &mut App, h: MHit) -> bool {
    if let MHit::SlotName(i) = h {
        let t = app.state.selected_mixer;
        if slot_name(&app.state, t, i).is_some() {
            let at = app.mouse;
            crate::playlist::start_edit(&mut app.state, EditTarget::SlotName(t, i), at);
            return true;
        }
    }
    false
}

fn start_rename(s: &mut State, n: u16, at: (f32, f32)) {
    crate::playlist::start_edit(s, EditTarget::MixerName(n), at);
}

/// Right click on a LED (§3): solo, or back to all on when it already is the only one audible.
pub fn solo(s: &mut State, n: u16) {
    solo_with(s, n, false);
}

/// Solo `n` with the tracks it sends to (down to the master) and, for `alt`, the tracks sending into it; back to
/// all on when exactly that set is already the audible one (spec: ui/mixer-track-commands.md §5), or, with "Restore
/// previous state after solo" on, to the states from before the first solo (a solo moved to another track keeps them;
/// spec: ui/settings-choices.md "Miscellaneous").
pub fn solo_with(s: &mut State, n: u16, alt: bool) {
    let mut keep = reach(s, n, true);
    if alt {
        keep.extend(reach(s, n, false));
    }
    keep.push(0);
    let soloed = audible_set(s, &keep);
    // A solo still in effect (the last solo's set, untouched since) keeps the states saved when it started.
    let in_solo = s.mixer.solo.as_ref().is_some_and(|(k, _)| audible_set(s, k));
    let before = match s.mixer.solo.take() {
        Some((_, b)) if in_solo => b,
        _ => s.project.mixer.iter().map(|m| (m.index, m.enabled)).collect(),
    };
    let back = soloed && in_solo && crate::prefs::check(&crate::prefs::RESTORE_AFTER_SOLO);
    for m in s.project.mixer.iter_mut() {
        m.enabled = if back {
            before.iter().find(|b| b.0 == m.index).is_none_or(|b| b.1)
        } else {
            soloed || keep.contains(&m.index)
        };
    }
    if !soloed {
        s.mixer.solo = Some((keep, before));
    }
    changed(s);
}

/// The tracks on besides the master are exactly `keep`.
fn audible_set(s: &State, keep: &[u16]) -> bool {
    s.project.mixer.iter().all(|m| m.index == 0 || keep.contains(&m.index) == m.enabled)
}

/// `n` and every track it reaches through sends (`down`), or every track reaching it (`!down`).
fn reach(s: &State, n: u16, down: bool) -> Vec<u16> {
    let mut seen = vec![n];
    let mut i = 0;
    while i < seen.len() {
        let t = seen[i];
        let next: Vec<u16> = if down {
            track(s, t).map(|m| m.routes.clone()).unwrap_or_default()
        } else {
            s.project.mixer.iter().filter(|m| m.routes.contains(&t)).map(|m| m.index).collect()
        };
        for r in next {
            if !seen.contains(&r) {
                seen.push(r);
            }
        }
        i += 1;
    }
    seen
}

/// Names track `t` after channel row `r` and gives it the channel's colour (spec: ui/mixer-track-commands.md §1,
/// §2). The master keeps its name.
fn name_after(s: &mut State, t: u16, r: usize) {
    if t == 0 {
        return;
    }
    let nm = crate::rack::channel_name(s, r);
    let col = s.project.channels.get(r).and_then(|c| c.color);
    if let Some(m) = track_mut(s, t) {
        m.name = Some(nm);
        if col.is_some() {
            m.color = col;
        }
    }
}

/// The rack's selected channels (the piano roll's channel when none is selected), in rack order.
fn selected_channels(s: &State) -> Vec<usize> {
    let n = s.project.channels.len();
    let rows: Vec<usize> = s.rack.sel.iter().copied().filter(|&r| r < n).collect();
    if rows.is_empty() && s.selected_channel < n { vec![s.selected_channel] } else { rows }
}

/// Channel routing ▸ (spec: ui/mixer-track-commands.md §2): the selected channels go to `dst` (named after the first),
/// or one per track from `dst` on (`spread`; channels past the last insert stay). `dst` becomes the current track.
pub fn route_channels(s: &mut State, dst: u16, spread: bool) -> Option<String> {
    let rows = selected_channels(s);
    let first = *rows.first()?;
    let max = inserts(s);
    let mut moved = 0;
    for (k, &r) in rows.iter().enumerate() {
        let t = if spread { dst + k as u16 } else { dst };
        if t > max {
            break;
        }
        s.project.channels[r].mixer_track = t;
        if spread || k == 0 {
            name_after(s, t, r);
        }
        moved += 1;
    }
    s.mixer.sel.clear();
    s.selected_mixer = dst;
    changed(s);
    Some(if moved == 1 {
        format!("Routed {} to {}", crate::rack::channel_name(s, first), name(s, dst))
    } else {
        format!("Routed {moved} channels")
    })
}

/// Route to free mixer track (spec: ui/mixer-track-commands.md §1): a channel alone on its insert keeps it, the others
/// move to the lowest insert no channel uses (a new track when none is free); the track is named after the channel
/// and becomes the current one.
pub fn route_free(s: &mut State, rows: &[usize]) -> Option<String> {
    let mut last = None;
    for &r in rows {
        let Some(cur) = s.project.channels.get(r).map(|c| c.mixer_track) else { continue };
        let alone = cur != 0 && cur <= inserts(s) && !s.project.channels.iter().enumerate().any(|(i, c)| i != r && c.mixer_track == cur);
        let t = if alone {
            cur
        } else {
            match (1..=inserts(s)).find(|t| !s.project.channels.iter().any(|c| c.mixer_track == *t)) {
                Some(t) => t,
                None => add_track(s, None),
            }
        };
        s.project.channels[r].mixer_track = t;
        name_after(s, t, r);
        last = Some((r, t));
    }
    let (r, t) = last?;
    s.mixer.sel.clear();
    s.selected_mixer = t;
    changed(s);
    Some(if rows.len() == 1 {
        format!("Routed {} to {}", crate::rack::channel_name(s, r), name(s, t))
    } else {
        format!("Routed {} channels to free tracks", rows.len())
    })
}

/// Select ▸ Tracks this one is routed to / Tracks routed to this one (spec: ui/mixer-track-commands.md §3), for every
/// track the command acts on. The first found becomes the current track.
fn select_linked(s: &mut State, n: u16, down: bool) -> Option<String> {
    let tg = targets(s, n);
    let mut found: Vec<u16> = Vec::new();
    for m in &s.project.mixer {
        let hit = if down { tg.iter().any(|&t| track(s, t).is_some_and(|x| x.routes.contains(&m.index))) } else { m.routes.iter().any(|r| tg.contains(r)) };
        if hit && !tg.contains(&m.index) {
            found.push(m.index);
        }
    }
    let Some(&first) = found.first() else { return Some("No tracks to select".into()) };
    s.selected_mixer = first;
    s.mixer.sel = found;
    None
}

/// The inserts Trim unused tracks deletes (spec: ui/mixer-track-commands.md §4, §6): no channel uses them and (unless
/// `non_empty`) they hold no effect; a track a kept track sends to stays, and so does one insert at least.
pub fn trim_list(s: &State, non_empty: bool) -> Vec<u16> {
    let n = inserts(s);
    let mut gone: Vec<u16> = (1..=n)
        .filter(|&t| !s.project.channels.iter().any(|c| c.mixer_track == t))
        .filter(|&t| non_empty || track(s, t).is_none_or(|m| m.slots.iter().all(Option::is_none)))
        .collect();
    loop {
        let fed: Vec<u16> = gone
            .iter()
            .copied()
            .filter(|&t| s.project.mixer.iter().any(|m| !gone.contains(&m.index) && m.index != t && m.routes.contains(&t)))
            .collect();
        if fed.is_empty() {
            break;
        }
        gone.retain(|t| !fed.contains(t));
    }
    if gone.len() == usize::from(n) {
        gone.remove(0);
    }
    gone
}

/// Deletes the tracks in `gone`: the others close up in order, channels and sends follow them, the mixer gets
/// shorter (spec: ui/mixer-track-commands.md §4).
fn remove_tracks(s: &mut State, gone: &[u16]) -> Option<String> {
    if gone.is_empty() {
        return Some("No unused mixer tracks".into());
    }
    let mut new_of = std::collections::HashMap::new();
    let mut k = 0u16;
    for m in &s.project.mixer {
        if !gone.contains(&m.index) {
            new_of.insert(m.index, k);
            k += 1;
        }
    }
    let m = |t: u16| new_of.get(&t).copied();
    for &t in gone {
        for slot in 0..10 {
            s.mixer.pending.push(FxReq::Delete { track: t, slot });
        }
    }
    let mut map = Vec::new();
    for (&o, &n) in &new_of {
        if o != n {
            map.extend((0..10u8).map(|slot| ((o, slot), (n, slot))));
        }
    }
    s.mixer.pending.push(FxReq::Remap(map));
    let old = std::mem::take(&mut s.project.mixer);
    s.project.mixer = old
        .into_iter()
        .filter_map(|t| {
            let index = m(t.index)?;
            let routes = t.routes.iter().filter_map(|&r| m(r)).collect();
            let send_levels = t.send_levels.iter().filter_map(|&(d, l)| m(d).map(|d| (d, l))).collect();
            Some(MixerTrack { index, routes, send_levels, ..t })
        })
        .collect();
    s.project.mixer.sort_by_key(|t| t.index);
    for c in s.project.channels.iter_mut() {
        c.mixer_track = m(c.mixer_track).unwrap_or(0);
    }
    s.selected_mixer = m(s.selected_mixer).unwrap_or(1).min(inserts(s));
    s.mixer.sel = s.mixer.sel.iter().filter_map(|&t| m(t)).collect();
    s.mixer.slot_names = std::mem::take(&mut s.mixer.slot_names).into_iter().filter_map(|((t, k), v)| Some(((m(t)?, k), v))).collect();
    changed(s);
    Some(format!("Removed {} unused mixer track(s)", gone.len()))
}

/// Routes the selected tracks to `dst` (§5): a normal route at unity, a sidechain at level 0, `only` drops their
/// other routes.
pub fn route(s: &mut State, dst: u16, only: bool, side: bool) -> Option<String> {
    let mut msg = None;
    for src in selected(s) {
        if src == dst || src == 0 {
            continue;
        }
        if makes_loop(s, src, dst) {
            msg = Some(format!("Can't route {} to {}: it would make a loop", name(s, src), name(s, dst)));
            continue;
        }
        let m = track_mut(s, src)?;
        if only {
            m.routes.clear();
            m.send_levels.clear();
        }
        if !m.routes.contains(&dst) {
            m.routes.push(dst);
            m.routes.sort_unstable();
        }
        m.send_levels.retain(|&(d, _)| d != dst);
        if side {
            m.send_levels.push((dst, 0.0));
        }
    }
    changed(s);
    msg
}

/// Appends a track (`at` = None) or inserts one before `at`, shifting the later ones; selects it. Returns its number.
pub fn add_track(s: &mut State, at: Option<u16>) -> u16 {
    let last = inserts(s) + 1;
    let n = at.unwrap_or(last).clamp(1, last);
    // The new track takes number n; n.. move one up (the hidden last track stays last).
    let len = s.project.mixer.len() as u16;
    s.project.mixer.push(MixerTrack::new(len));
    // The hidden last track moves too, keeping what it had (spec: flp/events/mixer.md "Adding tracks").
    shift_tracks(s, n, len, 1);
    let fresh = MixerTrack::new(n);
    if let Some(m) = track_mut(s, n) {
        *m = fresh;
    }
    s.mixer.sel.clear();
    s.selected_mixer = n;
    changed(s);
    n
}

/// Moves tracks `from..to` (exclusive) by `d` places (+1 = right): contents, routes into them, channel targets and
/// hosted slot instances. The track at the destination end is overwritten; callers make room first.
fn shift_tracks(s: &mut State, from: u16, to: u16, d: i32) {
    let m = |t: u16| if t >= from && t < to { (i32::from(t) + d) as u16 } else { t };
    renumber(s, &m, from, to, d);
}

/// Renumbers tracks with `m` (a permutation on the touched range).
fn renumber(s: &mut State, m: &dyn Fn(u16) -> u16, from: u16, to: u16, d: i32) {
    let old: Vec<MixerTrack> = s.project.mixer.clone();
    let _ = d;
    for t in &old {
        if t.index >= from && t.index < to {
            let n = m(t.index);
            if let Some(dst) = s.project.mixer.iter_mut().find(|x| x.index == n) {
                *dst = MixerTrack { index: n, ..t.clone() };
            }
        }
    }
    for t in s.project.mixer.iter_mut() {
        for r in t.routes.iter_mut() {
            *r = m(*r);
        }
        for (d, _) in t.send_levels.iter_mut() {
            *d = m(*d);
        }
        t.routes.retain(|&r| r != t.index);
    }
    for c in s.project.channels.iter_mut() {
        c.mixer_track = m(c.mixer_track);
    }
    s.selected_mixer = m(s.selected_mixer);
    s.mixer.sel = s.mixer.sel.iter().map(|&t| m(t)).collect();
    s.mixer.slot_names = std::mem::take(&mut s.mixer.slot_names).into_iter().map(|((t, k), v)| ((m(t), k), v)).collect();
    let mut map = Vec::new();
    for t in &old {
        for k in 0..10u8 {
            map.push(((t.index, k), (m(t.index), k)));
        }
    }
    s.mixer.pending.push(FxReq::Remap(map));
    changed(s);
}

/// Swaps track `n` with its neighbour (`d` = -1 left, +1 right), §6.1 Move left / right.
fn move_track(s: &mut State, n: u16, d: i32) -> Option<String> {
    let o = i32::from(n) + d;
    if n == 0 || o < 1 || o > i32::from(inserts(s)) {
        return None;
    }
    let o = o as u16;
    let m = move |t: u16| if t == n { o } else if t == o { n } else { t };
    let (a, b) = (n.min(o), n.max(o) + 1);
    renumber(s, &m, a, b, 0);
    None
}

/// Deletes track `n`: later tracks move one left, a default track is added at the end (§6.1 Delete).
fn delete_track(s: &mut State, n: u16) -> Option<String> {
    if n == 0 || n > inserts(s) {
        return Some("The master track can't be deleted".into());
    }
    let label = name(s, n);
    let len = s.project.mixer.len() as u16;
    // Drop the track's slots and routes into it, then close the gap.
    for t in s.project.mixer.iter_mut() {
        t.routes.retain(|&r| r != n);
        t.send_levels.retain(|&(d, _)| d != n);
    }
    for c in s.project.channels.iter_mut().filter(|c| c.mixer_track == n) {
        c.mixer_track = 0;
    }
    for k in 0..10 {
        s.mixer.pending.push(FxReq::Delete { track: n, slot: k });
    }
    s.mixer.slot_names.retain(|(t, _), _| *t != n);
    // The hidden last track stays last; the default track comes before it (spec: flp/events/mixer.md "The last track").
    let hidden = len - 1;
    let m = move |t: u16| if t > n && t < hidden { t - 1 } else { t };
    let old = s.project.mixer.clone();
    for t in &old {
        if t.index > n && t.index < hidden {
            if let Some(dst) = s.project.mixer.iter_mut().find(|x| x.index == t.index - 1) {
                *dst = MixerTrack { index: t.index - 1, ..t.clone() };
            }
        }
    }
    if let Some(last) = s.project.mixer.iter_mut().find(|x| x.index == hidden - 1) {
        *last = MixerTrack::new(hidden - 1);
    }
    for t in s.project.mixer.iter_mut() {
        for r in t.routes.iter_mut() {
            *r = m(*r);
        }
        for (d, _) in t.send_levels.iter_mut() {
            *d = m(*d);
        }
    }
    for c in s.project.channels.iter_mut() {
        c.mixer_track = m(c.mixer_track);
    }
    let mut map = Vec::new();
    for t in n + 1..hidden {
        for k in 0..10u8 {
            map.push(((t, k), (t - 1, k)));
        }
    }
    s.mixer.pending.push(FxReq::Remap(map));
    s.mixer.slot_names = std::mem::take(&mut s.mixer.slot_names).into_iter().map(|((t, k), v)| ((m(t), k), v)).collect();
    s.mixer.sel.clear();
    s.selected_mixer = n.min(inserts(s)).max(1);
    changed(s);
    Some(format!("Deleted {label}"))
}

/// Back to a fresh track (name, colour, levels, slots kept out), routed to the master (§6.1 Reset).
fn reset_track(s: &mut State, n: u16) {
    if let Some(m) = track_mut(s, n) {
        let slots = m.slots.clone();
        *m = MixerTrack { slots, ..MixerTrack::new(n) };
    }
    changed(s);
}

// ---------------------------------------------------------------------------------------------------------
// Menus

const COLORS: [(&str, u32); 8] = [
    ("Red", 0xc8504a),
    ("Orange", 0xd0803c),
    ("Yellow", 0xc8b44a),
    ("Green", 0x5a9c4a),
    ("Teal", 0x3c8c88),
    ("Blue", 0x4a78b4),
    ("Purple", 0x8a5aa0),
    ("Grey", 0x7a8288),
];

/// Track menu (right click on a strip), §6.1.
pub fn track_menu(s: &State, n: u16, x: f32, y: f32) -> Popup {
    let mut items = vec![PItem::head(Box::leak(format!("Mixer track ({})", name(s, n)).into_boxed_str()))];
    items.extend(track_items(n));
    Popup { x, y, items, sub: None }
}

fn track_items(n: u16) -> Vec<PItem> {
    let mx = |c: MCmd| Cmd::Mixer(c);
    let mut colors: Vec<PItem> = vec![PItem::new("Default", mx(MCmd::Color(n, None)))];
    colors.extend(COLORS.iter().map(|&(l, c)| PItem::new(l, mx(MCmd::Color(n, Some(Rgb::new((c >> 16) as u8, (c >> 8) as u8, c as u8)))))));
    vec![
        PItem::new("Rename, color and icon...", mx(MCmd::Rename(n))).key("F2"),
        PItem::sub("Change color...", colors),
        PItem::new("Random color", mx(MCmd::RandomColor(n))),
        PItem::not_yet("Gradient color..."),
        PItem::not_yet("Change icon..."),
        PItem::not_yet("Assign to new Audio track(s)"),
        PItem::not_yet("Unassign from Audio track(s)"),
        PItem::sep(),
        PItem::sub("File", vec![PItem::not_yet("Render to wave file...")]),
        PItem::not_yet("Open audio editor").key("Ctrl+E"),
        PItem::not_yet("Open audio editor and record").key("Shift+E"),
        PItem::sep(),
        PItem::new("Reset selected track(s) to default", mx(MCmd::Reset(n))),
        PItem::head("Routing"),
        PItem::sub(
            "Channel routing",
            vec![
                PItem::new("Route selected channels to this track", mx(MCmd::RouteChannels { dst: n, spread: false })).key("Ctrl+L"),
                PItem::new("Route selected channels starting from this track", mx(MCmd::RouteChannels { dst: n, spread: true })).key("Shift+Ctrl+L"),
            ],
        ),
        PItem::not_yet("Track routing"),
        PItem::sub(
            "Select",
            vec![
                PItem::new("Channels routed to this track", mx(MCmd::SelectChannels(n))).key("Alt+L"),
                PItem::not_yet("Channels and playlist tracks routed to this track"),
                PItem::sep(),
                PItem::new("Tracks this one is routed to", mx(MCmd::SelectDest(n))),
                PItem::new("Tracks routed to this one", mx(MCmd::SelectSources(n))).key("Ctrl+Alt+L"),
                PItem::sep(),
                PItem::not_yet("Group"),
                PItem::new("All", mx(MCmd::SelectAll)).key("Ctrl+A"),
            ],
        ),
        PItem::new("Reset selected track(s) routing", mx(MCmd::ResetRouting(n))),
        PItem::new("Unroute tracks routed to the selected track(s)", mx(MCmd::UnrouteTo(n))),
        PItem::sep(),
        PItem::new("Solo selected track(s)", mx(MCmd::Solo(n))).key("S"),
        PItem::new("Alt solo selected track(s)", mx(MCmd::AltSolo(n))).key("Alt+S"),
        // Disk recording is not available yet.
        PItem::not_yet("Arm selected track(s)"),
        PItem::not_yet("Disarm selected track(s)"),
        PItem::sep(),
        PItem::not_yet("Compensate for master latency").mark(Mark::Check(true)),
        PItem::head("Group"),
        PItem::not_yet("Separator").mark(Mark::Check(true)),
        PItem::not_yet("Create group..."),
        PItem::not_yet("Auto color group"),
        PItem::head("Layout"),
        PItem::sub("Dock to", vec![PItem::not_yet("Left"), PItem::not_yet("Middle"), PItem::not_yet("Right")]),
        PItem::new("Move left", mx(MCmd::MoveLeft(n))).key("Alt+Left"),
        PItem::new("Move right", mx(MCmd::MoveRight(n))).key("Alt+Right"),
        PItem::sep(),
        PItem::new("Delete", mx(MCmd::Delete(n))).key("Del"),
        PItem::new("Insert one", mx(MCmd::InsertOne(n))),
        PItem::not_yet("Insert multiple..."),
        PItem::sub("Insert from state", vec![PItem::not_yet("(no saved states)")]),
        PItem::head("CPU"),
        PItem::not_yet("Allow threaded processing").mark(Mark::Check(true)),
    ]
}

/// Trim unused tracks ▸ (spec: ui/mixer-track-commands.md §6): each choice with the number of tracks it deletes.
fn trim_items(s: &State) -> Vec<PItem> {
    [(false, "Unlinked tracks only"), (true, "Unlinked and non-empty tracks")]
        .into_iter()
        .map(|(non_empty, label)| {
            let k = trim_list(s, non_empty).len();
            let it = PItem::new(format!("{label} ({k})"), Cmd::Mixer(MCmd::Trim { non_empty }));
            if k == 0 { it.off() } else { it }
        })
        .collect()
}

/// Mixer options (title-bar ▸), §6.2: its own items, then the track menu's.
pub fn options_menu(s: &State, x: f32, y: f32) -> Popup {
    let mut items = vec![
        PItem::head("Options"),
        PItem::sub("Disk recording", vec![PItem::not_yet("Auto-create audio clips")]),
        PItem::sub("Plugin delay compensation", vec![PItem::not_yet("Automatic")]),
        PItem::not_yet("Link all parameters..."),
        PItem::sub("View", vec![PItem::not_yet("Waveforms"), PItem::not_yet("Extra properties")]),
        PItem::not_yet("Detached").mark(Mark::Check(false)),
        PItem::sep(),
        PItem::sub("Trim unused tracks", trim_items(s)),
        PItem::not_yet("Switch smart disable for all plugins"),
        PItem::not_yet("Remove all (audio editor) instances"),
        // The cable physics debug panel, hidden from users; uncomment to tune the cables again.
        // PItem::new("Cable physics (debug)...", Cmd::Mixer(MCmd::CablePhysics)).mark(Mark::Check(s.cables.borrow().debug)),
        PItem::head(Box::leak(format!("Mixer track ({})", name(s, s.selected_mixer)).into_boxed_str())),
    ];
    items.extend(track_items(s.selected_mixer));
    Popup { x, y, items, sub: None }
}

/// Layout combo menu (§6.4): only Wide is drawn, the other layouts are greyed out.
pub fn layout_menu(s: &State, x: f32, y: f32) -> Popup {
    let mut items = Vec::new();
    for (i, &l) in LAYOUTS.iter().enumerate() {
        if i == 7 {
            items.push(PItem::sep());
        }
        let on = Mark::Radio(s.mixer.layout == i as u8);
        items.push(if i as u8 == WIDE { PItem::new(l, Cmd::Mixer(MCmd::Layout(i as u8))) } else { PItem::not_yet(l) }.mark(on));
    }
    Popup { x, y, items, sub: None }
}

/// Empty slot: the effect list (§7.1), the plugin menu for this slot (spec: ui/plugin-menu.md).
pub fn effect_menu(s: &State, slot: u8, x: f32, y: f32) -> Popup {
    let mut items = vec![PItem::not_yet("Plugin database").key(crate::shortcuts::menu_key("Plugin database", "Shift+F8")), PItem::sep()];
    items.extend(crate::plugin_menu::items(s, crate::plugin_menu::Target::Fx(Some(slot)), false));
    Popup { x, y, items, sub: None }
}

/// Filled slot ▸ (§7.2).
pub fn slot_menu(s: &State, slot: u8, x: f32, y: f32) -> Popup {
    let mx = |c: MCmd| Cmd::Mixer(c);
    let replace = crate::plugin_menu::items(s, crate::plugin_menu::Target::Fx(Some(slot)), true);
    let items = vec![
        PItem::sub("Replace", replace),
        PItem::new("Delete...", mx(MCmd::SlotDelete(slot))),
        PItem::sep(),
        PItem::not_yet("Patcherize"),
        PItem::not_yet("Patcherize all"),
        PItem::head("Presets"),
        PItem::sub("Presets", vec![PItem::not_yet("(none)")]),
        PItem::not_yet("Browse presets"),
        PItem::not_yet("Save preset as..."),
        PItem::not_yet("Add to plugin database (flag as favorite)"),
        PItem::not_yet("Display preset name in preset selector").mark(Mark::Check(true)),
        PItem::head("Spare state"),
        PItem::not_yet("Store in spare state"),
        PItem::not_yet("Flip with spare state"),
        PItem::head("Parameters"),
        PItem::not_yet("Link all parameters..."),
        PItem::new("Browse parameters", Cmd::Param(crate::params::PCmd::Browse(crate::plugins::fx_key(s.selected_mixer, slot)))),
        PItem::head("CPU"),
        PItem::not_yet("Allow threaded processing").mark(Mark::Check(true)),
        PItem::not_yet("Smart disable"),
        PItem::sep(),
        PItem::new("Move up", mx(MCmd::SlotMove(slot, -1))),
        PItem::new("Move down", mx(MCmd::SlotMove(slot, 1))),
        PItem::sep(),
        PItem::new("View editor", mx(MCmd::SlotView(slot))).mark(Mark::Check(false)),
        PItem::new("Rename, color and icon...", mx(MCmd::SlotRename(slot))).key("F2"),
        PItem::not_yet("Change color..."),
        PItem::not_yet("Random color"),
        PItem::not_yet("Change icon..."),
        PItem::sep(),
        PItem::sub("Scaling", vec![PItem::not_yet("100%")]),
        PItem::not_yet("Help"),
    ];
    Popup { x, y, items, sub: None }
}

// ---------------------------------------------------------------------------------------------------------
// Commands

pub fn run(app: &mut App, c: MCmd) -> Option<String> {
    let at = app.mouse;
    let s = &mut app.state;
    let msg = match c {
        MCmd::Rename(n) => {
            start_rename(s, n, at);
            None
        }
        MCmd::Color(n, col) => {
            for t in targets(s, n) {
                if let Some(m) = track_mut(s, t) {
                    m.color = col;
                }
            }
            None
        }
        MCmd::RandomColor(n) => {
            for t in targets(s, n) {
                let c = crate::patterns::random_color(u64::from(t).wrapping_mul(0x9e37_79b9) ^ u64::from(rand_seed()));
                if let Some(m) = track_mut(s, t) {
                    m.color = Some(c);
                }
            }
            None
        }
        MCmd::Reset(n) => {
            for t in targets(s, n) {
                reset_track(s, t);
            }
            None
        }
        MCmd::ResetRouting(n) => {
            for t in targets(s, n).into_iter().filter(|&t| t != 0) {
                if let Some(m) = track_mut(s, t) {
                    m.routes = vec![0];
                    m.send_levels.clear();
                }
            }
            changed(s);
            None
        }
        MCmd::UnrouteTo(n) => {
            let tg = targets(s, n);
            for m in s.project.mixer.iter_mut() {
                m.routes.retain(|r| !tg.contains(r));
                m.send_levels.retain(|(d, _)| !tg.contains(d));
            }
            changed(s);
            None
        }
        MCmd::Solo(n) => {
            solo(s, n);
            None
        }
        MCmd::AltSolo(n) => {
            solo_with(s, n, true);
            None
        }
        MCmd::RouteChannels { dst, spread } => route_channels(s, dst, spread),
        MCmd::SelectChannels(n) => {
            let tg = targets(s, n);
            let rows: std::collections::BTreeSet<usize> =
                s.project.channels.iter().enumerate().filter(|(_, c)| tg.contains(&c.mixer_track)).map(|(i, _)| i).collect();
            let k = rows.len();
            s.rack.sel = rows;
            Some(format!("Selected {k} channel(s)"))
        }
        MCmd::SelectDest(n) => select_linked(s, n, true),
        MCmd::SelectSources(n) => select_linked(s, n, false),
        MCmd::SelectAll => {
            s.mixer.sel = s.project.mixer.iter().map(|m| m.index).filter(|&t| t <= inserts(s)).collect();
            None
        }
        MCmd::Trim { non_empty } => {
            let gone = trim_list(s, non_empty);
            remove_tracks(s, &gone)
        }
        MCmd::MoveLeft(n) => move_track(s, n, -1),
        MCmd::MoveRight(n) => move_track(s, n, 1),
        MCmd::Delete(n) => delete_track(s, n),
        MCmd::InsertOne(n) => {
            let n = add_track(s, Some(n.max(1)));
            Some(format!("Inserted {}", name(s, n)))
        }
        MCmd::Route { dst, only, side } => route(s, dst, only, side),
        // Wide is the layout shown; the others are greyed out in the menu.
        MCmd::Layout(_) => None,
        MCmd::CablePhysics => {
            let c = s.cables.get_mut();
            c.debug = !c.debug;
            None
        }
        MCmd::Animations(on) => {
            s.cables.get_mut().animate(on);
            crate::app::save_animations(on);
            None
        }
        MCmd::AddEffect(slot, index) => {
            let t = s.selected_mixer;
            let Some(nm) = app.plugins.effects.get(index).map(|a| a.desc.name.clone()) else { return None };
            if let Some(m) = track_mut(s, t) {
                m.slots[usize::from(slot)] = Some(crate::plugins::new_slot(&nm));
                m.slot_on[usize::from(slot)] = true;
                m.slot_mix[usize::from(slot)] = 1.0;
            }
            s.mixer.slot_names.remove(&(t, slot));
            s.mixer.pending.push(FxReq::Create { track: t, slot, index });
            changed(s);
            None
        }
        MCmd::SlotDelete(slot) => {
            let t = s.selected_mixer;
            if let Some(m) = track_mut(s, t) {
                m.slots[usize::from(slot)] = None;
                m.slot_on[usize::from(slot)] = true;
                m.slot_mix[usize::from(slot)] = 1.0;
            }
            s.mixer.slot_names.remove(&(t, slot));
            s.mixer.pending.push(FxReq::Delete { track: t, slot });
            changed(s);
            None
        }
        MCmd::SlotMove(slot, d) => {
            let t = s.selected_mixer;
            let o = i32::from(slot) + i32::from(d);
            if !(0..10).contains(&o) {
                return None;
            }
            let (a, b) = (usize::from(slot), o as usize);
            if let Some(m) = track_mut(s, t) {
                m.slots.swap(a, b);
                m.slot_on.swap(a, b);
                m.slot_mix.swap(a, b);
            }
            let (na, nb) = (s.mixer.slot_names.remove(&(t, a as u8)), s.mixer.slot_names.remove(&(t, b as u8)));
            if let Some(v) = na {
                s.mixer.slot_names.insert((t, b as u8), v);
            }
            if let Some(v) = nb {
                s.mixer.slot_names.insert((t, a as u8), v);
            }
            s.mixer.pending.push(FxReq::Remap(vec![((t, a as u8), (t, b as u8)), ((t, b as u8), (t, a as u8))]));
            changed(s);
            None
        }
        MCmd::SlotRename(slot) => {
            let t = s.selected_mixer;
            crate::playlist::start_edit(s, EditTarget::SlotName(t, slot), at);
            None
        }
        MCmd::SlotView(slot) => {
            let t = s.selected_mixer;
            s.mixer.pending.push(FxReq::Toggle { track: t, slot });
            None
        }
    };
    app.invalidate();
    msg
}

/// Tracks a "selected track(s)" command acts on: the selection when `n` is in it, else `n` alone.
fn targets(s: &State, n: u16) -> Vec<u16> {
    if is_selected(s, n) { selected(s) } else { vec![n] }
}

fn rand_seed() -> u32 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(7, |d| d.subsec_nanos())
}

/// Keys while the mixer is in front (§2): F2 rename, Del delete, Alt+Left / Right move, S solo, Alt+S alt solo,
/// Ctrl+L / Shift+Ctrl+L route the selected channels, Alt+L / Ctrl+Alt+L / Ctrl+A select (spec:
/// ui/mixer-track-commands.md).
pub fn key(app: &mut App, key: &winit::keyboard::Key, ctrl: bool, alt: bool) -> bool {
    use winit::keyboard::{Key, NamedKey};
    let n = app.state.selected_mixer;
    let letter = |c: &str, l: &str| c.eq_ignore_ascii_case(l);
    if ctrl {
        let msg = match key {
            Key::Character(c) if letter(c, "l") && alt => run(app, MCmd::SelectSources(n)),
            Key::Character(c) if letter(c, "l") => {
                let spread = app.shift;
                run(app, MCmd::RouteChannels { dst: n, spread })
            }
            Key::Character(c) if letter(c, "a") && !alt => run(app, MCmd::SelectAll),
            _ => return false,
        };
        if msg.is_some() {
            app.state.message = msg;
        }
        app.invalidate();
        return true;
    }
    let msg = match key {
        Key::Character(c) if alt && letter(c, "s") => run(app, MCmd::AltSolo(n)),
        Key::Character(c) if alt && letter(c, "l") => run(app, MCmd::SelectChannels(n)),
        Key::Named(NamedKey::F2) => run(app, MCmd::Rename(n)),
        Key::Named(NamedKey::Delete) => run(app, MCmd::Delete(n)),
        Key::Named(NamedKey::ArrowLeft) if alt => run(app, MCmd::MoveLeft(n)),
        Key::Named(NamedKey::ArrowRight) if alt => run(app, MCmd::MoveRight(n)),
        Key::Named(NamedKey::ArrowLeft) => {
            if n > 0 {
                select(&mut app.state, n - 1, false);
            }
            None
        }
        Key::Named(NamedKey::ArrowRight) => {
            if n < inserts(&app.state) {
                select(&mut app.state, n + 1, false);
            }
            None
        }
        Key::Character(c) if c.eq_ignore_ascii_case("s") && !alt => run(app, MCmd::Solo(n)),
        _ => return false,
    };
    if msg.is_some() {
        app.state.message = msg;
    }
    app.invalidate();
    true
}

/// New peaks from the audio thread: meters rise, fall and hold by elapsed time (`meters::step`). True while any meter
/// is still moving.
pub fn feed_meters(s: &mut State, peaks: &[[f32; 2]]) -> bool {
    let now = std::time::Instant::now();
    // Long gaps (the app was idle) count as one frame, so a meter does not vanish in a single step.
    let dt = s.mixer.meter_at.map_or(1.0 / 60.0, |t| now.duration_since(t).as_secs_f32().min(0.1));
    s.mixer.meter_at = Some(now);
    let m = &mut s.mixer.meters;
    if m.len() < peaks.len() {
        m.resize(peaks.len(), Default::default());
    }
    let mut moving = false;
    for (i, v) in m.iter_mut().enumerate() {
        let p = peaks.get(i).copied().unwrap_or([0.0; 2]);
        moving |= crate::meters::step(v, p, dt, &s.mixer.meter_cfg);
    }
    if !moving {
        s.mixer.meter_at = None;
    }
    moving
}

impl App {
    /// Mixer slot work on the UI thread (`plugin_tick`): create / open / close / drop hosted effect instances.
    pub(crate) fn mixer_tick(&mut self) {
        let reqs = std::mem::take(&mut self.state.mixer.pending);
        if reqs.is_empty() {
            return;
        }
        let mut relink = false;
        // Creates waiting for a native plugin's first check (`Plugins::effect_ready`); a newer request for the same
        // slot replaces a waiting one.
        let mut waiting: Vec<FxReq> = Vec::new();
        let mut done = false;
        for r in reqs {
            if let FxReq::Create { track, slot, .. } | FxReq::Delete { track, slot } = r {
                waiting.retain(|w| !matches!(*w, FxReq::Create { track: t, slot: s, .. } if (t, s) == (track, slot)));
            }
            if let FxReq::Create { index, .. } = r {
                if !self.plugins.effect_ready(index) {
                    let name = self.plugins.effects.get(index).map(|a| a.desc.name.clone()).unwrap_or_default();
                    let msg = format!("Trying {name} before its first load...");
                    if self.state.message.as_deref() != Some(msg.as_str()) {
                        self.state.message = Some(msg);
                        done = true;
                    }
                    waiting.push(r);
                    continue;
                }
            }
            done = true;
            match r {
                FxReq::Create { track, slot, index } => match self.plugins.create_effect(track, slot, index, None) {
                    Ok(_) => {
                        self.plugins.added(crate::plugins::fx_key(track, slot));
                        relink = true;
                        self.open_fx(track, slot);
                    }
                    Err(e) => {
                        if let Some(m) = track_mut(&mut self.state, track) {
                            m.slots[usize::from(slot)] = None;
                        }
                        self.state.message = Some(format!("Effect: {e}"));
                    }
                },
                FxReq::Toggle { track, slot } => {
                    let key = crate::plugins::fx_key(track, slot);
                    if !self.plugins.close_if_open(key) {
                        self.open_fx(track, slot);
                    }
                }
                FxReq::Show { track, slot } => self.open_fx(track, slot),
                FxReq::Delete { track, slot } => {
                    let key = crate::plugins::fx_key(track, slot);
                    if self.plugins.hosted.contains_key(&key) {
                        self.plugins.drop_key(key);
                        relink = true;
                    }
                }
                FxReq::Remap(map) => {
                    self.plugins.remap_fx(&|t, k| Some(map.iter().find(|(a, _)| *a == (t, k)).map_or((t, k), |&(_, b)| b)));
                    relink = true;
                }
            }
        }
        waiting.append(&mut self.state.mixer.pending);
        self.state.mixer.pending = waiting;
        if relink {
            self.transport.set_effects(self.plugins.effects());
            self.state.piano.dirty = true;
        }
        if done {
            self.invalidate();
        }
    }

    pub(crate) fn open_fx(&mut self, track: u16, slot: u8) {
        let key = crate::plugins::fx_key(track, slot);
        if !self.plugins.hosted.contains_key(&key) {
            self.state.message = Some("This slot's plugin is not hosted (no Lemon Studio version of it yet)".into());
            return;
        }
        #[cfg(not(plugin_windows))]
        {
            self.state.message = Some(crate::app::off_text(crate::app::PLUGIN_WINDOWS));
        }
        #[cfg(plugin_windows)]
        {
            let info = fx_info(&self.state.project, &self.state.mixer, key);
            match info.map(|i| self.plugins.open_window(key, i)) {
                Some(Ok(())) => {
                    self.state.raise(crate::app::Win::Hosted(key));
                    self.select_window_track(key);
                }
                Some(Err(e)) => self.state.message = Some(format!("Plugin: {e}")),
                None => {}
            }
        }
    }

    /// Auto select linked modules: opening or focusing plugin window `key` (a channel row or an `fx_key`) selects its
    /// mixer track as clicking that insert would, scrolled into view; a channel on the master selects the master
    /// (spec: ui/settings-choices.md "Miscellaneous: seven more checks, Default template and Startup project").
    #[cfg_attr(not(plugin_windows), allow(dead_code))]
    pub(crate) fn select_window_track(&mut self, key: usize) {
        if !crate::prefs::check(&crate::prefs::AUTO_SELECT_LINKED) {
            return;
        }
        let s = &self.state;
        let track = match crate::plugins::fx_of(key) {
            Some((t, _)) => Some(t),
            None => s.project.channels.get(key).map(|c| c.mixer_track),
        };
        let Some(t) = track.filter(|t| s.project.mixer.iter().any(|m| m.index == *t)) else { return };
        select(&mut self.state, t, false);
        // The master strip is fixed; an insert scrolls into the mixer's last painted view.
        if let Some(bar) = self.bar(crate::app::Scroll::MixerX).filter(|_| t > 0) {
            let w = crate::views::mixer::STRIP_W;
            let left = w * f32::from(t - 1);
            let scroll = self.state.mixer_scroll;
            if left < scroll {
                self.scroll_set(crate::app::Scroll::MixerX, left);
            } else if left + w > scroll + bar.view {
                self.scroll_set(crate::app::Scroll::MixerX, left + w - bar.view);
            }
        }
        self.invalidate();
    }
}

/// The plugin frame's title for a mixer slot: "Name (Insert N)" / "(Master)" (§7).
#[cfg(plugin_windows)]
pub fn fx_info(p: &os_model::Project, ui: &MixerUi, key: usize) -> Option<crate::plugin_window::ChannelInfo> {
    let (t, k) = crate::plugins::fx_of(key)?;
    let m = p.mixer.iter().find(|m| m.index == t)?;
    let name = ui.slot_names.get(&(t, k)).cloned().or_else(|| m.slots.get(usize::from(k))?.as_ref().map(crate::project_plugins::slot_title))?;
    Some(crate::plugin_window::ChannelInfo {
        misc: None,
        name,
        target: m.name.clone().filter(|n| !n.is_empty()).unwrap_or_else(|| if t == 0 { "Master".into() } else { format!("Insert {t}") }),
        enabled: m.slot_on[usize::from(k)],
        volume: 0.8,
        pan: 0.0,
        pitch_cents: 0,
        range: 2,
        track: t,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> App {
        let mut app = App::new(None);
        app.state.project = crate::app::test_project();
        app
    }

    #[test]
    fn select_mute_solo() {
        let mut a = app();
        let n = inserts(&a.state);
        assert_eq!(n, 16, "default mixer shows Insert 1-16");
        hit(&mut a, MHit::Strip(3));
        assert_eq!(selected(&a.state), vec![3]);
        a.ctrl = true;
        hit(&mut a, MHit::Strip(5));
        a.ctrl = false;
        assert_eq!(selected(&a.state), vec![5, 3]);
        hit(&mut a, MHit::Strip(2));
        assert_eq!(selected(&a.state), vec![2]);
        hit(&mut a, MHit::Led(4));
        assert!(!track(&a.state, 4).unwrap().enabled);
        hit(&mut a, MHit::Led(4));
        assert!(track(&a.state, 4).unwrap().enabled);
        right(&mut a, MHit::Led(6));
        assert!(a.state.project.mixer.iter().all(|m| m.enabled == (m.index == 0 || m.index == 6)), "solo");
        right(&mut a, MHit::Led(6));
        assert!(a.state.project.mixer.iter().all(|m| m.enabled), "unsolo");
        a.shift = true;
        hit(&mut a, MHit::Strip(7));
        a.shift = false;
        assert_eq!(a.state.playlist.edit.as_ref().map(|e| e.target), Some(EditTarget::MixerName(7)));
        a.state.playlist.edit.as_mut().unwrap().text = "Drums".into();
        crate::playlist::commit_edit(&mut a.state);
        assert_eq!(name(&a.state, 7), "Drums");
    }

    /// Auto select linked modules (spec: ui/settings-choices.md "Miscellaneous: seven more checks, Default template and
    /// Startup project"): a plugin window selects its channel's insert (scrolled into view) or its slot's track.
    #[test]
    fn plugin_windows_select_their_mixer_track() {
        use crate::app::{Scroll, Win};
        let mut a = app();
        assert!(crate::prefs::check(&crate::prefs::AUTO_SELECT_LINKED), "on by default");
        // Snare (channel 3) is on Insert 4, as in the test FL was checked with.
        select(&mut a.state, 2, true);
        a.select_window_track(3);
        assert_eq!((a.state.selected_mixer, a.state.mixer.sel.clone()), (4, vec![]), "as a plain click on the insert");
        a.state.project.channels[0].mixer_track = 0;
        a.select_window_track(0);
        assert_eq!(a.state.selected_mixer, 0, "a channel on the master selects the master");
        a.select_window_track(crate::plugins::fx_key(5, 2));
        assert_eq!(a.state.selected_mixer, 5, "an effect selects its slot's track");
        a.state.project.channels[1].mixer_track = 40;
        a.select_window_track(1);
        assert_eq!(a.state.selected_mixer, 5, "no such track: nothing changes");
        // Scrolled into the mixer's view.
        let Ok(mut c) = os_ui::Canvas::new(1920, 1080, 1.0) else { return };
        let ws = &mut a.state.wins[Win::Mixer.slot().unwrap()];
        (ws.maxed, ws.rect) = (false, os_ui::Rect::new(100.0, 100.0, 560.0, 400.0));
        a.state.z = vec![Win::Mixer];
        a.paint(&mut c, None);
        let view = a.bar(Scroll::MixerX).expect("mixer scroll bar").view;
        let w = crate::views::mixer::STRIP_W;
        assert!(view < 16.0 * w, "the mixer shows part of the inserts");
        a.state.project.channels[4].mixer_track = 16;
        a.select_window_track(4);
        assert_eq!(a.state.selected_mixer, 16);
        assert!((a.state.mixer_scroll + view - 16.0 * w).abs() < 0.5, "Insert 16 at the right edge: {}", a.state.mixer_scroll);
        a.select_window_track(2);
        assert_eq!((a.state.selected_mixer, a.state.mixer_scroll), (3, 2.0 * w), "Insert 3 at the left edge");
        a.select_window_track(0);
        assert_eq!((a.state.selected_mixer, a.state.mixer_scroll), (0, 2.0 * w), "the master does not scroll");
        // Off: the selection is left alone.
        crate::prefs::set_check(&crate::prefs::AUTO_SELECT_LINKED, false);
        a.select_window_track(3);
        assert_eq!(a.state.selected_mixer, 0);
    }

    #[test]
    fn routing() {
        let mut a = app();
        select(&mut a.state, 1, false);
        hit(&mut a, MHit::Route(5));
        assert_eq!(track(&a.state, 1).unwrap().routes, vec![0, 5]);
        assert_eq!(send(&a.state, 1, 5), Some(0.8));
        a.ctrl = true;
        hit(&mut a, MHit::Route(6));
        a.ctrl = false;
        assert_eq!(track(&a.state, 1).unwrap().routes, vec![6], "route only");
        a.shift = true;
        hit(&mut a, MHit::Route(7));
        a.shift = false;
        assert_eq!(send(&a.state, 1, 7), Some(0.0), "sidechain");
        hit(&mut a, MHit::Unroute(7));
        assert_eq!(send(&a.state, 1, 7), None);
        assert!(receives(&a.state, 6));
        // 6 -> 1 would loop.
        select(&mut a.state, 6, false);
        assert!(hit(&mut a, MHit::Route(1)).is_some_and(|m| m.contains("loop")));
        assert!(!track(&a.state, 6).unwrap().routes.contains(&1));
        assert_eq!(level_text("Volume", 0.8), "Volume: 100%  0.0dB  1.00");
        assert_eq!(level_text("To Insert 11", 0.0), "To Insert 11: 0%  -INFdB  0.00");
    }

    #[test]
    fn add_move_delete_tracks() {
        let mut a = app();
        let len = a.state.project.mixer.len();
        hit(&mut a, MHit::Add);
        assert_eq!(a.state.project.mixer.len(), len + 1);
        assert_eq!(a.state.selected_mixer, 17);
        track_mut(&mut a.state, 2).unwrap().name = Some("Two".into());
        a.state.project.channels[0].mixer_track = 2;
        run(&mut a, MCmd::MoveRight(2));
        assert_eq!(name(&a.state, 3), "Two");
        assert_eq!(a.state.project.channels[0].mixer_track, 3, "channels follow their track");
        run(&mut a, MCmd::Delete(1));
        assert_eq!(name(&a.state, 2), "Two", "later tracks move left");
        assert_eq!(a.state.project.channels[0].mixer_track, 2);
        assert_eq!(a.state.project.mixer.len(), len + 1, "a fresh track fills the end");
        run(&mut a, MCmd::InsertOne(2));
        assert_eq!(name(&a.state, 3), "Two");
        assert!(a.state.project.mixer.iter().enumerate().all(|(i, m)| usize::from(m.index) == i), "indices stay in order");
    }

    #[test]
    fn slot_commands_without_plugins() {
        let mut a = app();
        a.plugins.effects = vec![crate::plugin_db::fake("Test FX", os_host::Kind::Effect)];
        select(&mut a.state, 2, false);
        run(&mut a, MCmd::AddEffect(0, 0));
        assert_eq!(slot_name(&a.state, 2, 0).as_deref(), Some("Test FX"));
        run(&mut a, MCmd::SlotMove(0, 1));
        assert_eq!(slot_name(&a.state, 2, 1).as_deref(), Some("Test FX"));
        hit(&mut a, MHit::SlotLed(1));
        assert!(!track(&a.state, 2).unwrap().slot_on[1]);
        run(&mut a, MCmd::SlotDelete(1));
        assert_eq!(slot_name(&a.state, 2, 1), None);
        assert!(a.state.mixer.pending.iter().any(|r| matches!(r, FxReq::Create { track: 2, slot: 0, index: 0 })));
    }

    // spec: ui/mixer-track-commands.md §1
    #[test]
    fn route_to_free_track() {
        let mut a = app();
        // Kick 1, Clap 2, Hat 3, Snare 4, Pluck 5; Snare joins Kick on 1, Pluck uses the master.
        a.state.project.channels[3].mixer_track = 1;
        a.state.project.channels[4].mixer_track = 0;
        a.state.project.channels[2].color = Some(Rgb::new(10, 20, 30));
        crate::rack::run(&mut a, crate::rack::RCmd::AssignFree(2));
        assert_eq!(a.state.project.channels[2].mixer_track, 3, "alone on its track: stays");
        assert_eq!(name(&a.state, 3), "Hat");
        assert_eq!(track(&a.state, 3).unwrap().color, Some(Rgb::new(10, 20, 30)));
        assert_eq!(a.state.selected_mixer, 3);
        crate::rack::run(&mut a, crate::rack::RCmd::AssignFree(3));
        assert_eq!(a.state.project.channels[3].mixer_track, 4, "shared: the lowest free insert");
        assert_eq!(name(&a.state, 4), "Snare");
        crate::rack::run(&mut a, crate::rack::RCmd::AssignFree(4));
        assert_eq!(a.state.project.channels[4].mixer_track, 5, "on the master: a free insert");
        assert_eq!(a.state.selected_mixer, 5);
        // No free insert left: a new track at the end.
        for (i, c) in a.state.project.channels.iter_mut().enumerate() {
            c.mixer_track = if i == 0 { 1 } else { 1 + i as u16 };
        }
        // Channels on every insert, one more sharing Insert 1.
        let more: Vec<_> = (6..=16).chain([1]).map(|t| os_model::Channel { mixer_track: t, ..a.state.project.channels[0].clone() }).collect();
        a.state.project.channels.extend(more);
        let len = a.state.project.mixer.len();
        crate::rack::run(&mut a, crate::rack::RCmd::AssignFree(16));
        assert_eq!(a.state.project.mixer.len(), len + 1);
        assert_eq!(a.state.project.channels[16].mixer_track, 17);
    }

    // spec: ui/mixer-track-commands.md §2
    #[test]
    fn channel_routing() {
        let mut a = app();
        a.state.rack.sel = [0, 2].into_iter().collect();
        run(&mut a, MCmd::RouteChannels { dst: 14, spread: false });
        assert_eq!(a.state.project.channels[0].mixer_track, 14);
        assert_eq!(a.state.project.channels[2].mixer_track, 14);
        assert_eq!(name(&a.state, 14), "Kick", "named after the first channel");
        assert_eq!(name(&a.state, 3), "Insert 3", "the track left keeps its name");
        assert_eq!(a.state.selected_mixer, 14);
        a.state.rack.sel = [1, 3, 4].into_iter().collect();
        run(&mut a, MCmd::RouteChannels { dst: 15, spread: true });
        let tracks: Vec<u16> = a.state.project.channels.iter().map(|c| c.mixer_track).collect();
        assert_eq!(tracks, vec![14, 15, 14, 16, 5], "past the last insert the channel stays");
        assert_eq!((name(&a.state, 15), name(&a.state, 16)), ("Clap".to_string(), "Snare".to_string()));
        // Nothing selected in the rack: the piano roll's channel.
        a.state.rack.sel.clear();
        a.state.selected_channel = 4;
        run(&mut a, MCmd::RouteChannels { dst: 7, spread: false });
        assert_eq!(a.state.project.channels[4].mixer_track, 7);
    }

    // spec: ui/mixer-track-commands.md §3
    #[test]
    fn select_by_routing() {
        let mut a = app();
        a.state.project.channels[3].mixer_track = 2;
        run(&mut a, MCmd::SelectChannels(2));
        assert_eq!(a.state.rack.sel.iter().copied().collect::<Vec<_>>(), vec![1, 3]);
        track_mut(&mut a.state, 5).unwrap().routes = vec![3];
        track_mut(&mut a.state, 6).unwrap().routes = vec![3];
        run(&mut a, MCmd::SelectSources(3));
        assert_eq!(a.state.mixer.sel, vec![5, 6]);
        assert_eq!(a.state.selected_mixer, 5);
        run(&mut a, MCmd::SelectDest(15));
        assert_eq!(a.state.mixer.sel, vec![0]);
        assert!(run(&mut a, MCmd::SelectSources(9)).is_some_and(|m| m.contains("No tracks")));
        run(&mut a, MCmd::SelectAll);
        assert_eq!(a.state.mixer.sel, (0..=16).collect::<Vec<u16>>());
    }

    // spec: ui/mixer-track-commands.md §4, §6
    #[test]
    fn trim_unused_tracks() {
        let mut a = app();
        a.state.project.channels[3].mixer_track = 5;
        a.state.project.channels[4].mixer_track = 9;
        track_mut(&mut a.state, 9).unwrap().name = Some("Nine".into());
        // 12 holds an effect, 11 feeds nothing but 9 sends to 13.
        track_mut(&mut a.state, 12).unwrap().slots[0] = Some(os_model::SlotPlugin { name: "FX".into(), state: Vec::new() });
        track_mut(&mut a.state, 9).unwrap().routes = vec![13];
        assert_eq!(trim_list(&a.state, false), vec![4, 6, 7, 8, 10, 11, 14, 15, 16]);
        assert_eq!(trim_list(&a.state, true), vec![4, 6, 7, 8, 10, 11, 12, 14, 15, 16]);
        let len = a.state.project.mixer.len();
        run(&mut a, MCmd::Trim { non_empty: true });
        assert_eq!(a.state.project.mixer.len(), len - 10);
        assert!(a.state.project.mixer.iter().enumerate().all(|(i, m)| usize::from(m.index) == i));
        let tracks: Vec<u16> = a.state.project.channels.iter().map(|c| c.mixer_track).collect();
        assert_eq!(tracks, vec![1, 2, 3, 4, 5]);
        assert_eq!(name(&a.state, 5), "Nine");
        assert_eq!(track(&a.state, 5).unwrap().routes, vec![6], "its send follows 13 to 6");
        assert!(a.state.project.mixer.last().unwrap().routes.is_empty(), "the hidden last track stays");
        assert!(trim_list(&a.state, true).is_empty());
        // Nothing uses any insert: one stays.
        for c in a.state.project.channels.iter_mut() {
            c.mixer_track = 0;
        }
        track_mut(&mut a.state, 5).unwrap().routes = vec![0];
        assert_eq!(trim_list(&a.state, true), vec![2, 3, 4, 5, 6]);
    }

    // spec: ui/mixer-track-commands.md §5
    #[test]
    fn solo_and_alt_solo() {
        let mut a = app();
        track_mut(&mut a.state, 5).unwrap().routes = vec![3];
        let on = |a: &App| a.state.project.mixer.iter().filter(|m| m.enabled).map(|m| m.index).collect::<Vec<_>>();
        run(&mut a, MCmd::Solo(3));
        assert_eq!(on(&a), vec![0, 3], "Solo mutes the track sending into 3");
        run(&mut a, MCmd::Solo(3));
        assert_eq!(on(&a).len(), a.state.project.mixer.len());
        run(&mut a, MCmd::AltSolo(3));
        assert_eq!(on(&a), vec![0, 3, 5]);
        run(&mut a, MCmd::AltSolo(3));
        assert_eq!(on(&a).len(), a.state.project.mixer.len());
        run(&mut a, MCmd::Solo(5));
        assert_eq!(on(&a), vec![0, 3, 5], "Solo keeps where 5 sends");
    }

    #[test]
    fn mixer_keys() {
        use winit::keyboard::Key;
        let mut a = app();
        select(&mut a.state, 10, false);
        a.state.rack.sel = [1].into_iter().collect();
        assert!(key(&mut a, &Key::Character("l".into()), true, false));
        assert_eq!(a.state.project.channels[1].mixer_track, 10, "Ctrl+L");
        assert!(key(&mut a, &Key::Character("a".into()), true, false));
        assert_eq!(a.state.mixer.sel.len(), 17, "Ctrl+A");
        select(&mut a.state, 4, false);
        assert!(key(&mut a, &Key::Character("s".into()), false, true));
        assert!(!track(&a.state, 7).unwrap().enabled, "Alt+S");
        assert!(!key(&mut a, &Key::Character("q".into()), true, false));
    }
}
