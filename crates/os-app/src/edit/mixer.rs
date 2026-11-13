// Copyright (C) 2026 ItsAlphaNeon
// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The session's half of `crate::mixer`: what reads and changes the project, apart from the interface (specs
//! `roadmap/control-plane.md`, part 2, step 6).

#![allow(deprecated)] // writes the project directly (`crate::locked`; control-plane step 14 removes this)
#![allow(unused_imports)]

use std::collections::HashMap;
use os_model::{MixerTrack, Rgb};
use crate::app::{App, State};
use crate::control::command::Region;
use crate::control_ui::edit;
use crate::knobs::KnobId;
use crate::playlist::{Cmd, EditTarget, Mark, PItem, Popup};
use crate::mixer::*;

/// Inserts shown: stored tracks minus the master and the last (hidden) one (§1).
pub fn inserts(s: &crate::session::Session) -> u16 {
    s.project.mixer.len().saturating_sub(2) as u16
}

pub fn track(s: &crate::session::Session, n: u16) -> Option<&MixerTrack> {
    s.project.mixer.iter().find(|m| m.index == n)
}

pub(crate) fn track_mut(s: &mut crate::session::Session, n: u16) -> Option<&mut MixerTrack> {
    s.project.get_mut().mixer.iter_mut().find(|m| m.index == n)
}

/// "Master", "Bus 3", or the track's own name.
pub fn name(s: &crate::session::Session, n: u16) -> String {
    track(s, n).and_then(|m| m.name.clone()).filter(|n| !n.is_empty()).unwrap_or_else(|| if n == 0 { "Master".into() } else { format!("Bus {n}") })
}

/// A fader's value (unity when the track has none stored).
pub fn volume(s: &crate::session::Session, t: u16) -> f64 {
    track(s, t).and_then(|m| m.volume).unwrap_or(0.8)
}

/// Whether track `src` sends to `dst` (route present), and at what level (fader scale, 0.8 = unity).
pub fn send(s: &crate::session::Session, src: u16, dst: u16) -> Option<f64> {
    let t = track(s, src)?;
    t.routes.contains(&dst).then(|| t.send_levels.iter().find(|(d, _)| *d == dst).map_or(0.8, |&(_, l)| l))
}

/// Whether any track routes into `n`.
pub fn receives(s: &crate::session::Session, n: u16) -> bool {
    s.project.mixer.iter().any(|m| m.index != n && m.routes.contains(&n))
}

/// The slot's shown name (renamed, else the plugin's name).
pub fn slot_name(s: &crate::session::Session, t: u16, slot: u8) -> Option<String> {
    let p = track(s, t)?.slots.get(usize::from(slot))?.as_ref()?;
    Some(slot_label(s, t, slot).unwrap_or_else(|| crate::project_plugins::slot_title(p)))
}

/// The slot's colour, when it holds a plugin and was given one.
pub fn slot_color(s: &crate::session::Session, t: u16, slot: u8) -> Option<Rgb> {
    let m = track(s, t)?;
    m.slots.get(usize::from(slot))?.as_ref()?;
    m.slot_colors[usize::from(slot)]
}

/// Automation and controllers follow mixer places that moved (`track` and `slot` as `target::moved` takes them; spec:
/// audio/automation.md "Targets follow moves"): the project's targets (`Project::follow_mixer_places`), the event
/// editor's, and the remote control links (`remote::follow`). A link whose place went goes.
pub(crate) fn follow_places(s: &mut crate::session::Session, track: &dyn Fn(u16) -> Option<u16>, slot: &dyn Fn(u16, u8) -> Option<(u16, u8)>) {
    let view = crate::session::View::places(&s.project, track, slot);
    s.touched.views.push(view);
    s.project.get_mut().follow_mixer_places(track, slot);
    crate::remote::follow(s, |l| moved_link(l, track, slot));
}

/// Forgets slot `k` of track `t`'s name, colour and icon (a new or removed plugin).
pub(crate) fn forget_slot_ui(s: &mut crate::session::Session, t: u16, k: u8) {
    s.project.write_if(|p| {
        let Some(m) = p.mixer.iter_mut().find(|m| m.index == t) else { return false };
        let k = usize::from(k);
        let had = m.slot_names[k].is_some() || m.slot_colors[k].is_some() || m.slot_icons[k].is_some();
        (m.slot_names[k], m.slot_colors[k], m.slot_icons[k]) = (None, None, None);
        had
    });
}

/// Lemon Studio's name typed for slot `k` of track `t` (the rename box), if any.
pub fn slot_label(s: &crate::session::Session, t: u16, k: u8) -> Option<String> {
    track(s, t)?.slot_names.get(usize::from(k))?.clone()
}

/// Names slot `k` of track `t` (`None`: its plugin's name again).
pub fn set_slot_label(s: &mut crate::session::Session, t: u16, k: u8, name: Option<String>) {
    s.project.write_if(|p| {
        p.mixer.iter_mut().find(|m| m.index == t).and_then(|m| m.slot_names.get_mut(usize::from(k))).is_some_and(|v| std::mem::replace(v, name) != *v)
    });
}

/// Colours slot `k` of track `t` (`None`: none).
pub fn set_slot_color(s: &mut crate::session::Session, t: u16, k: u8, color: Option<Rgb>) {
    s.project.write_if(|p| {
        p.mixer.iter_mut().find(|m| m.index == t).and_then(|m| m.slot_colors.get_mut(usize::from(k))).is_some_and(|v| std::mem::replace(v, color) != *v)
    });
}

/// Whether slot `k` of track `t` holds Tesla (a project's Edison slots too; a name typed for the slot doesn't count).
pub(crate) fn is_tesla(s: &crate::session::Session, t: u16, k: u8) -> bool {
    track(s, t).and_then(|m| m.slots.get(usize::from(k))?.as_ref()).is_some_and(|p| crate::project_plugins::slot_title(p) == crate::plugins::TESLA)
}

/// Track `t`'s first Tesla slot.
pub(crate) fn first_tesla(s: &crate::session::Session, t: u16) -> Option<u8> {
    let n = track(s, t)?.slots.len() as u8;
    (0..n).find(|&k| is_tesla(s, t, k))
}

/// Every Tesla in the mixer, as (track, slot).
pub(crate) fn teslas(s: &crate::session::Session) -> Vec<(u16, u8)> {
    s.project.mixer.iter().flat_map(|m| (0..m.slots.len() as u8).map(move |k| (m.index, k))).filter(|&(t, k)| is_tesla(s, t, k)).collect()
}

/// Empties slot `k` of track `t` and drops its instance (the slot menu's Delete).
pub(crate) fn clear_slot(s: &mut crate::session::Session, t: u16, k: u8) {
    if let Some(m) = track_mut(s, t) {
        m.slots[usize::from(k)] = None;
        m.slot_on[usize::from(k)] = true;
        m.slot_mix[usize::from(k)] = 1.0;
        m.slot_cpu[usize::from(k)] = os_model::PluginCpu::default();
    }
    forget_slot_ui(s, t, k);
    s.fx_requests.push(FxReq::Delete { track: t, slot: k });
}

/// Would a route `src` -> `dst` make a loop (dst already reaches src)?
pub(crate) fn makes_loop(s: &crate::session::Session, src: u16, dst: u16) -> bool {
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

pub(crate) fn changed(s: &mut crate::session::Session) {
    s.dirty = true;
}

/// Switch auto-bypass for all plugins (spec: ui/channel-switches.md §5): every plugin's Auto-bypass when silent (the
/// mixer slots' and the plugin channels', `os_model::PluginCpu::smart_disable`) on when one is off, as the original
/// does; with all of them on, all off (Lemon Studio's choice: the original's answer there was not observed). The
/// plugins inside containers have switches of their own (`crate::container::Member::cpu`) and are switched with them.
/// No message, as in the original. Returns whether there was a plugin to switch.
pub(crate) fn switch_auto_bypass(s: &mut crate::session::Session) -> bool {
    let (mut members, mut off) = (0, false);
    crate::container::each_member_cpu(s.project.get_mut(), |c| {
        members += 1;
        off |= !c.smart_disable;
    });
    let p = s.project.get_mut();
    let slots = p.mixer.iter_mut().flat_map(|m| m.slot_cpu.iter_mut().zip(&m.slots).filter(|(_, p)| p.is_some()).map(|(c, _)| c));
    let channels = p.channels.iter_mut().filter(|c| c.kind == os_model::ChannelKind::Plugin).map(|c| &mut c.misc.cpu);
    let mut all: Vec<&mut os_model::PluginCpu> = slots.chain(channels).collect();
    if all.is_empty() && members == 0 {
        return false;
    }
    let on = off || all.iter().any(|c| !c.smart_disable);
    for c in &mut all {
        c.smart_disable = on;
    }
    crate::container::each_member_cpu(s.project.get_mut(), |c| c.smart_disable = on);
    changed(s);
    true
}

/// The input combo's text for track `n`, and whether the device has inputs to choose from.
pub fn input_label(s: &crate::session::Session, n: u16) -> (String, bool) {
    let (pairs, monos) = input_choices(crate::device::input_source().as_ref());
    let v = track(s, n).map_or(-1, |m| m.audio_in);
    let all: Vec<(i32, String)> = pairs.into_iter().chain(monos).collect();
    (choice_label(v, &all), !all.is_empty())
}

/// The output combo's text for track `n`.
pub fn output_label(s: &crate::session::Session, n: u16) -> String {
    let v = track(s, n).map_or(-1, |m| m.audio_out);
    choice_label(v, &output_choices(crate::device::last().as_ref()))
}

/// The input choices for track `t`, as its input menu lists them; `marked`: the track's current input marked (an
/// audio track's Select audio input ▸, spec: ui/playlist.md §6c). `None` when the device has no inputs.
pub fn input_items(s: &crate::session::Session, t: u16, marked: bool) -> Option<Vec<PItem>> {
    let d = crate::device::input_source();
    let (pairs, monos) = input_choices(d.as_ref());
    let driver = d.map(|d| d.name).unwrap_or_default();
    if pairs.is_empty() && monos.is_empty() {
        return None;
    }
    let now = track(s, t).map_or(-1, |m| m.audio_in);
    let item = |(v, name): (i32, String)| {
        let it = PItem::new(name, Cmd::Mixer(MCmd::Input(t, v)));
        if marked { it.mark(Mark::Radio(v == now)) } else { it }
    };
    let mut items = Vec::new();
    if !pairs.is_empty() {
        items.push(PItem::head(format!("{driver} - stereo")));
        items.extend(pairs.into_iter().map(item));
    }
    items.push(PItem::head(format!("{driver} - mono")));
    items.extend(monos.into_iter().map(item));
    items.extend([PItem::sep(), item((-1, "(none)".into()))]);
    Some(items)
}

/// The input clock's (`input`) or the track latency combo's menu (spec: audio/track-io.md §1.1, §2.6): Reset, Set in
/// ms / samples / beats; the original's other entries are greyed (its own greyed ones checked as it shows them).
pub(crate) fn latency_menu(s: &crate::session::Session, t: u16, input: bool, x: f32, y: f32) -> Popup {
    use crate::typein::LatUnit;
    let now = track(s, t).map_or(0.0, |m| if input { m.latency_in } else { m.latency_out });
    let set = |label: &'static str, u: LatUnit| PItem::new(label, Cmd::Mixer(MCmd::Latency(t, input, Some(u))));
    let reset = PItem::new("Reset", Cmd::Mixer(MCmd::Latency(t, input, None)));
    let mut items = vec![
        PItem::head(format!("{} latency: {}", if input { "Input" } else { "Track" }, latency_text(now))),
        if now == 0.0 { reset.off() } else { reset },
        set("Set in ms...", LatUnit::Ms),
        set("Set in samples...", LatUnit::Samples),
        set("Set in beats...", LatUnit::Beats),
    ];
    if input {
        items.extend([PItem::sep(), PItem::not_yet("Adjust position of recorded audio").mark(Mark::Check(true))]);
    } else {
        items.extend([
            PItem::sub("Set from", vec![PItem::not_yet("(nothing to set it from)")]),
            PItem::sep(),
            PItem::not_yet("Delay audio output").mark(Mark::Check(true)),
            PItem::head("Bypass track latency compensation"),
            PItem::not_yet("Never").mark(Mark::Radio(true)),
            PItem::not_yet("Always").mark(Mark::Radio(false)),
            PItem::not_yet("When recording or not playing").mark(Mark::Radio(false)),
        ]);
    }
    Popup { x, y, items, sub: None, scroll: [0; 2] }
}

/// Track `t` takes its input alone (the External input question's Yes).
pub(crate) fn external_only(s: &mut crate::session::Session, t: u16) {
    if let Some(m) = track_mut(s, t) {
        m.rec_source = os_model::REC_EXTERNAL;
        changed(s);
    }
}

/// Sets one of track `t`'s input, output, record source or monitor mode (`f`) as an undoable edit, unless it is so
/// already.
pub(crate) fn set_io(s: &mut crate::session::Session, t: u16, f: impl Fn(&mut MixerTrack)) -> bool {
    let Some(m) = track(s, t) else { return false };
    let mut after = m.clone();
    f(&mut after);
    if after == *m {
        return false;
    }
    crate::history::track_io(s, t);
    if let Some(m) = track_mut(s, t) {
        f(m);
    }
    changed(s);
    true
}

/// The group track `n` is in (spec: ui/mixer.md §6.1): the inserts from the nearest one at or before it with Separator
/// on up to the next one with it; `None` before the first separator, and for the master.
pub fn group(s: &crate::session::Session, n: u16) -> Option<std::ops::RangeInclusive<u16>> {
    let last = inserts(s);
    if n == 0 || n > last {
        return None;
    }
    let sep = |t: u16| track(s, t).is_some_and(|m| m.separator);
    let first = (1..=n).rev().find(|&t| sep(t))?;
    let end = (n + 1..=last).find(|&t| sep(t)).map_or(last, |t| t - 1);
    Some(first..=end)
}

/// The tracks on besides the master are exactly `keep`.
pub(crate) fn audible_set(s: &crate::session::Session, keep: &[u16]) -> bool {
    s.project.mixer.iter().all(|m| m.index == 0 || keep.contains(&m.index) == m.enabled)
}

/// `n` and every track it reaches through sends (`down`), or every track reaching it (`!down`).
pub(crate) fn reach(s: &crate::session::Session, n: u16, down: bool) -> Vec<u16> {
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
pub(crate) fn name_after(s: &mut crate::session::Session, t: u16, r: usize) {
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

/// The inserts Trim unused tracks deletes (spec: ui/mixer-track-commands.md §4, §6): no channel uses them and (unless
/// `non_empty`) they hold no effect; a track a kept track sends to stays, and so does one insert at least.
pub fn trim_list(s: &crate::session::Session, non_empty: bool) -> Vec<u16> {
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

/// `add_track` in the session; the views select the new track.
pub(crate) fn add_track_in(s: &mut crate::session::Session, at: Option<u16>) -> u16 {
    let last = inserts(s) + 1;
    let n = at.unwrap_or(last).clamp(1, last);
    // The new track takes number n; n.. move one up (the hidden last track stays last).
    let len = s.project.mixer.len() as u16;
    s.project.get_mut().mixer.push(MixerTrack::new(len));
    // The hidden last track moves too, keeping what it had (spec: flp/events/mixer.md "Adding tracks").
    shift_tracks(s, n, len, 1);
    let fresh = MixerTrack::new(n);
    if let Some(m) = track_mut(s, n) {
        *m = fresh;
    }
    s.touched.views.push(crate::session::View::MixerSelect(n));
    changed(s);
    n
}

/// Moves tracks `from..to` (exclusive) by `d` places (+1 = right): contents, routes into them, channel targets and
/// hosted slot instances. The track at the destination end is overwritten; callers make room first.
pub(crate) fn shift_tracks(s: &mut crate::session::Session, from: u16, to: u16, d: i32) {
    let m = |t: u16| if t >= from && t < to { (i32::from(t) + d) as u16 } else { t };
    renumber(s, &m, from, to, d);
}

/// Renumbers tracks with `m` (a permutation on the touched range).
pub(crate) fn renumber(s: &mut crate::session::Session, m: &dyn Fn(u16) -> u16, from: u16, to: u16, d: i32) {
    let old: Vec<MixerTrack> = s.project.mixer.clone();
    let _ = d;
    for t in &old {
        if t.index >= from && t.index < to {
            let n = m(t.index);
            if let Some(dst) = s.project.get_mut().mixer.iter_mut().find(|x| x.index == n) {
                *dst = MixerTrack { index: n, ..t.clone() };
            }
        }
    }
    for t in s.project.get_mut().mixer.iter_mut() {
        for r in t.routes.iter_mut() {
            *r = m(*r);
        }
        for (d, _) in t.send_levels.iter_mut() {
            *d = m(*d);
        }
        t.routes.retain(|&r| r != t.index);
    }
    for c in s.project.get_mut().channels.iter_mut() {
        c.mixer_track = m(c.mixer_track);
    }
    s.project.get_mut().remap_link_mixers(|t| Some(m(t)));
    follow_places(s, &|t| Some(m(t)), &|t, k| Some((m(t), k)));
    let n = s.project.mixer.len() as u16 + 1;
    s.touched.views.push(crate::session::View::MixerRenumbered((0..n).map(|t| Some(m(t))).collect()));
    let mut map = Vec::new();
    for t in &old {
        for k in 0..10u8 {
            map.push(((t.index, k), (m(t.index), k)));
        }
    }
    s.fx_requests.push(FxReq::Remap(map));
    changed(s);
}

/// `delete_track` in the session; the views select the track now in its place.
pub(crate) fn delete_track_in(s: &mut crate::session::Session, n: u16) -> Option<String> {
    if n == 0 || n > inserts(s) {
        return Some("The master track can't be deleted".into());
    }
    let label = name(s, n);
    let len = s.project.mixer.len() as u16;
    // Drop the track's slots and routes into it, then close the gap.
    for t in s.project.get_mut().mixer.iter_mut() {
        t.routes.retain(|&r| r != n);
        t.send_levels.retain(|&(d, _)| d != n);
    }
    for c in s.project.get_mut().channels.iter_mut().filter(|c| c.mixer_track == n) {
        c.mixer_track = 0;
    }
    s.project.get_mut().remap_link_mixers(|t| (t != n).then_some(t));
    for k in 0..10 {
        s.fx_requests.push(FxReq::Delete { track: n, slot: k });
    }
    // The hidden last track stays last; the default track comes before it (spec: flp/events/mixer.md "The last track").
    let hidden = len - 1;
    let m = move |t: u16| if t > n && t < hidden { t - 1 } else { t };
    let old = s.project.mixer.clone();
    for t in &old {
        if t.index > n && t.index < hidden {
            if let Some(dst) = s.project.get_mut().mixer.iter_mut().find(|x| x.index == t.index - 1) {
                *dst = MixerTrack { index: t.index - 1, ..t.clone() };
            }
        }
    }
    if let Some(last) = s.project.get_mut().mixer.iter_mut().find(|x| x.index == hidden - 1) {
        *last = MixerTrack::new(hidden - 1);
    }
    for t in s.project.get_mut().mixer.iter_mut() {
        for r in t.routes.iter_mut() {
            *r = m(*r);
        }
        for (d, _) in t.send_levels.iter_mut() {
            *d = m(*d);
        }
    }
    for c in s.project.get_mut().channels.iter_mut() {
        c.mixer_track = m(c.mixer_track);
    }
    s.project.get_mut().remap_link_mixers(|t| Some(m(t)));
    follow_places(s, &|t| (t != n).then(|| m(t)), &|t, k| (t != n).then(|| (m(t), k)));
    let mut map = Vec::new();
    for t in n + 1..hidden {
        for k in 0..10u8 {
            map.push(((t, k), (t - 1, k)));
        }
    }
    s.fx_requests.push(FxReq::Remap(map));
    let current = n.min(inserts(s)).max(1);
    s.touched.views.push(crate::session::View::MixerSelect(current));
    changed(s);
    Some(format!("Deleted {label}"))
}

/// Back to a fresh track (name, colour, levels, slots kept out), routed to the master (§6.1 Reset).
pub(crate) fn reset_track(s: &mut crate::session::Session, n: u16) {
    if let Some(m) = track_mut(s, n) {
        let (slots, origin) = (m.slots.clone(), m.origin);
        *m = MixerTrack { slots, origin, ..MixerTrack::new(n) };
    }
    changed(s);
}

/// Puts the saved state in `path` on track `n` (`named`: with its name and colour); the track's old effects go.
pub(crate) fn open_state(s: &mut crate::session::Session, n: u16, path: &std::path::Path, named: bool) -> Option<String> {
    let Some(st) = crate::mixer_state::load(path) else { return Some(format!("Couldn't read the mixer track state {}", path.display())) };
    for k in 0..10u8 {
        if track(s, n).is_some_and(|m| m.slots[usize::from(k)].is_some()) {
            clear_slot(s, n, k);
        }
    }
    if let Some(m) = track_mut(s, n) {
        st.apply(m, named);
    }
    changed(s);
    let file = path.file_stem().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
    Some(format!("{} takes the mixer track state \"{file}\"", name(s, n)))
}

/// Trim unused tracks ▸ (spec: ui/mixer-track-commands.md §6): each choice with the number of tracks it deletes.
pub(crate) fn trim_items(s: &crate::session::Session) -> Vec<PItem> {
    [(false, "Unlinked tracks only"), (true, "Unlinked and non-empty tracks")]
        .into_iter()
        .map(|(non_empty, label)| {
            let k = trim_list(s, non_empty).len();
            let it = PItem::new(format!("{label} ({k})"), Cmd::Mixer(MCmd::Trim { non_empty }));
            if k == 0 { it.off() } else { it }
        })
        .collect()
}

/// The tracks routed into track `n`, in track order: its inputs, numbered from 1 (spec: audio/sidechain.md "Settings").
pub fn inputs(s: &crate::session::Session, n: u16) -> Vec<u16> {
    let mut ins: Vec<u16> = s.project.mixer.iter().filter(|m| m.index != 0 && m.index != n && m.routes.contains(&n)).map(|m| m.index).collect();
    ins.sort_unstable();
    ins
}
