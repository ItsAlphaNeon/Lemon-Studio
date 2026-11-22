// Copyright (C) 2026 ItsAlphaNeon
// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The undo of a delete as an inverse (`Taken::Removed`): what the delete took out of the project, each piece with
//! the place it had, put back where it was. A channel's delete takes its notes, control points, automation clip and
//! targets, clips, instrument track links, initial values, controller links and layer memberships; a pattern's takes
//! its clips in the arrangement shown; a mixer track's takes the routes and sends into it, the channels and audio
//! track links going to it (they go to the master) and the automation targets on it and its slots. The rest of the project is left as it is, so the undo of a delete leaves later
//! changes to other things in place. Its redo is the delete again (`Taken::Deleted`), giving what the delete makes to
//! fill numbers (a pattern delete's patterns, a track delete's empty track before the hidden last one) the ids they had.
//! A track's plugins come back from the states kept by their ids (`control::host::follow_plan`).
// spec: roadmap/control-plane.md "Undo is made from inverses, not snapshots"

use os_model::{target, Automation, Clip, ClipSource, ControlPoint, ControllerLink, InitControl, MixerTrack, Note, Pattern, Project, TrackLink};

use super::command::{channel_row, pattern_place, track_number, Error, Thing};

/// What a delete took out, with the places it had (`Taken::Removed`).
#[derive(Debug, Clone, PartialEq)]
pub enum Removed {
    Channel(Box<ChannelGone>),
    Pattern(Box<PatternGone>),
    Track(Box<TrackGone>),
}

/// The pieces taken out of `v`, each at its place in `v` before (in place order).
type Places<T> = Vec<(usize, T)>;

/// A channel deleted, at its row, and what went with it.
#[derive(Debug, Clone, PartialEq)]
pub struct ChannelGone {
    pub row: usize,
    pub channel: os_model::Channel,
    /// Other channels' layer children naming it: (channel id, place in its list).
    layer: Vec<(u32, usize)>,
    /// Each pattern's notes and control points of it: (pattern id, ...).
    notes: Vec<(u32, Places<Note>)>,
    controls: Vec<(u32, Places<ControlPoint>)>,
    /// Its automation clip's data (none: it is no automation clip).
    automations: Places<Automation>,
    /// Other automation clips' targets on it: (clip's place, target's place, target).
    targets: Vec<(usize, usize, u32)>,
    /// Each arrangement's clips of it: (arrangement id, ...).
    clips: Vec<(u32, Places<Clip>)>,
    /// Playlist tracks linked to it: (arrangement id, track's place, the link as it was).
    links: Vec<(u32, usize, TrackLink)>,
    init: Places<InitControl>,
    controller_links: Places<ControllerLink>,
}

/// A pattern deleted, and what went with it.
#[derive(Debug, Clone, PartialEq)]
pub struct PatternGone {
    pub pattern: Pattern,
    /// The pattern shown before the delete.
    shown: u16,
    /// The numbers the delete fills with empty patterns (`edit::patterns::renumber_in` keeps every number there was).
    fillers: Vec<u16>,
    /// The arrangement shown's clips of it: (arrangement id, ...).
    clips: Option<(u32, Places<Clip>)>,
}

/// A mixer track deleted, at its number, and what went with it.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackGone {
    pub track: MixerTrack,
    /// Other tracks' routes and sends into it: (track id, place, level).
    routes: Vec<(u32, usize)>,
    sends: Vec<(u32, usize, f64)>,
    /// The channels going to it, by id.
    channels: Vec<u32>,
    /// Playlist tracks linked to it: (arrangement id, track's place, the link as it was).
    links: Vec<(u32, usize, TrackLink)>,
    /// The automation targets on it and its slots (`Project::follow_mixer_places`): automation clips' (clip's place,
    /// target's place, target), each pattern's control points, initial values, controller links.
    targets: Vec<(usize, usize, u32)>,
    controls: Vec<(u32, Places<ControlPoint>)>,
    init: Places<InitControl>,
    controller_links: Places<ControllerLink>,
}

fn take<T: Clone>(v: &[T], f: impl Fn(&T) -> bool) -> Places<T> {
    v.iter().enumerate().filter(|(_, x)| f(x)).map(|(i, x)| (i, x.clone())).collect()
}

/// Puts `items` back into `v` at their places (taken in place order from `v` as it was).
fn put<T>(v: &mut Vec<T>, items: Places<T>) {
    for (i, x) in items {
        let i = i.min(v.len());
        v.insert(i, x);
    }
}

/// Whether target `t` names channel `c` (`edit::rack::remap_project`'s automation targets).
fn on_channel(t: u32, c: u16) -> bool {
    t != target::TEMPO && (t >> 16) as u16 == c
}

impl Removed {
    /// What deleting `thing` would take out of `p` (`s` for the pattern shown).
    pub fn of(s: &crate::session::Session, thing: Thing) -> Result<Removed, Error> {
        let p = &s.project;
        match thing {
            Thing::Channel(id) => {
                let row = channel_row(p, id)?;
                if p.channels.len() < 2 {
                    return Err("a project keeps at least one channel".into());
                }
                Ok(Removed::Channel(Box::new(channel(p, row))))
            }
            Thing::Pattern(id) => {
                let pattern = p.patterns[pattern_place(p, id)?].clone();
                let n = pattern.number;
                let old: std::collections::BTreeSet<u16> = p.patterns.iter().map(|q| q.number).collect();
                let kept: std::collections::BTreeSet<u16> = old.iter().filter(|&&m| m != n).map(|&m| if m > n { m - 1 } else { m }).collect();
                let fillers = old.difference(&kept).copied().collect();
                let clips = p.arrangement().map(|a| (a.id.0, take(&a.clips, |c| c.source == ClipSource::Pattern(n))));
                Ok(Removed::Pattern(Box::new(PatternGone { pattern, shown: s.pattern(), fillers, clips })))
            }
            Thing::Track(id) => {
                let n = track_number(p, id)?;
                if n == 0 {
                    return Err("the master track can't be deleted".into());
                }
                Ok(Removed::Track(Box::new(track(p, n))))
            }
        }
    }

    /// The thing this puts back.
    pub fn thing(&self) -> Thing {
        match self {
            Removed::Channel(g) => Thing::Channel(g.channel.id.0),
            Removed::Pattern(g) => Thing::Pattern(g.pattern.id.0),
            Removed::Track(g) => Thing::Track(g.track.id.0),
        }
    }

    /// The ids of the things the delete made, as they are now: the patterns filling a pattern delete's numbers. Its
    /// redo gives them again.
    pub fn made(&self, p: &Project) -> Vec<u32> {
        match self {
            Removed::Channel(_) => Vec::new(),
            Removed::Pattern(g) => g.fillers.iter().filter_map(|&n| p.pattern(n).map(|q| q.id.0)).collect(),
            // The track before the hidden last one.
            Removed::Track(_) => p.mixer.iter().find(|m| usize::from(m.index) + 2 == p.mixer.len()).map(|m| m.id.0).into_iter().collect(),
        }
    }

    /// The timebase changed (`Command::rescale`).
    pub fn rescale(&mut self, from: u16, to: u16) {
        match self {
            Removed::Channel(g) => {
                g.notes.iter_mut().flat_map(|(_, v)| v.iter_mut()).for_each(|(_, n)| n.rescale(from, to));
                g.controls.iter_mut().flat_map(|(_, v)| v.iter_mut()).for_each(|(_, c)| c.rescale(from, to));
                g.clips.iter_mut().flat_map(|(_, v)| v.iter_mut()).for_each(|(_, c)| c.rescale(from, to));
            }
            Removed::Pattern(g) => {
                super::command::rescale_pattern(&mut g.pattern, from, to);
                g.clips.iter_mut().flat_map(|(_, v)| v.iter_mut()).for_each(|(_, c)| c.rescale(from, to));
            }
            Removed::Track(g) => g.controls.iter_mut().flat_map(|(_, v)| v.iter_mut()).for_each(|(_, c)| c.rescale(from, to)),
        }
    }

    /// Puts it all back.
    pub fn put(self, s: &mut crate::session::Session) {
        match self {
            Removed::Channel(g) => put_channel(s, *g),
            Removed::Pattern(g) => put_pattern(s, *g),
            Removed::Track(g) => put_track(s, *g),
        }
        s.dirty = true;
    }
}

fn channel(p: &Project, row: usize) -> ChannelGone {
    let channel = p.channels[row].clone();
    let c = channel.index;
    let layer = p
        .channels
        .iter()
        .filter(|o| o.id.0 != channel.id.0)
        .flat_map(|o| o.layer_children.iter().enumerate().filter(|&(_, &k)| k == c).map(|(i, _)| (o.id.0, i)))
        .collect();
    let notes = p.patterns.iter().map(|q| (q.id.0, take(&q.notes, |n| n.channel == c))).filter(|(_, v)| !v.is_empty()).collect();
    let controls = p
        .patterns
        .iter()
        .map(|q| (q.id.0, take(&q.controls, |x| x.channel() < 0x4000 && x.channel() == c)))
        .filter(|(_, v)| !v.is_empty())
        .collect();
    let automations = take(&p.automations, |a| a.channel == c);
    let targets = p
        .automations
        .iter()
        .enumerate()
        .filter(|(_, a)| a.channel != c)
        .flat_map(|(ai, a)| a.targets.iter().enumerate().filter(|&(_, &t)| on_channel(t, c)).map(move |(ti, &t)| (ai, ti, t)))
        .collect();
    let clips = p
        .arrangements
        .iter()
        .map(|a| (a.id.0, take(&a.clips, |x| x.source == ClipSource::Channel(c))))
        .filter(|(_, v)| !v.is_empty())
        .collect();
    let links = p
        .arrangements
        .iter()
        .flat_map(|a| a.tracks.iter().enumerate().filter(|(_, t)| t.link.channel == Some(c)).map(|(i, t)| (a.id.0, i, t.link)))
        .collect();
    let init = take(&p.init_controls, |x| (x.target >> 16) as u16 == c);
    let controller_links = take(&p.controller_links, |l| {
        let (hi, source) = ((l.target() >> 16) as u16, l.source());
        hi == c || source == c
    });
    ChannelGone { row, channel, layer, notes, controls, automations, targets, clips, links, init, controller_links }
}

fn put_channel(s: &mut crate::session::Session, g: ChannelGone) {
    let ChannelGone { row, channel, layer, notes, controls, automations, targets, clips, links, init, controller_links } = g;
    let id = channel.id;
    // The rows from its own down, and every index after them, move one down again.
    super::command::put_channel_back(s, row, channel.clone());
    let p = s.project.get_mut();
    let Some(r) = p.channels.iter().position(|c| c.id.0 == id.0) else { return };
    p.channels[r] = os_model::Channel { index: r as u16, ..channel };
    for (owner, i) in layer {
        if let Some(o) = p.channels.iter_mut().find(|o| o.id.0 == owner) {
            let i = i.min(o.layer_children.len());
            o.layer_children.insert(i, r as u16);
        }
    }
    for (pat, v) in notes {
        if let Some(q) = p.patterns.iter_mut().find(|q| q.id.0 == pat) {
            put(&mut q.notes, v);
        }
    }
    for (pat, v) in controls {
        if let Some(q) = p.patterns.iter_mut().find(|q| q.id.0 == pat) {
            put(&mut q.controls, v);
        }
    }
    put(&mut p.automations, automations);
    for (ai, ti, t) in targets {
        if let Some(a) = p.automations.get_mut(ai) {
            let ti = ti.min(a.targets.len());
            a.targets.insert(ti, t);
        }
    }
    for (arr, v) in clips {
        if let Some(a) = p.arrangements.iter_mut().find(|a| a.id.0 == arr) {
            put(&mut a.clips, v);
        }
    }
    for (arr, i, link) in links {
        if let Some(t) = p.arrangements.iter_mut().find(|a| a.id.0 == arr).and_then(|a| a.tracks.get_mut(i)) {
            t.link = link;
        }
    }
    put(&mut p.init_controls, init);
    put(&mut p.controller_links, controller_links);
    s.touched.clips = true;
    s.touched.notes = true;
}

fn put_pattern(s: &mut crate::session::Session, g: PatternGone) {
    let PatternGone { pattern, shown, fillers, clips } = g;
    let n = pattern.number;
    let p = s.project.get_mut();
    p.patterns.retain(|q| !fillers.contains(&q.number));
    for q in &mut p.patterns {
        if q.number >= n {
            q.number += 1;
        }
    }
    let at = p.patterns.iter().position(|q| q.number > n).unwrap_or(p.patterns.len());
    p.patterns.insert(at, pattern);
    if let Some((arr, v)) = clips {
        if let Some(a) = p.arrangements.iter_mut().find(|a| a.id.0 == arr) {
            for c in &mut a.clips {
                if let ClipSource::Pattern(m) = c.source {
                    if m >= n {
                        c.source = ClipSource::Pattern(m + 1);
                    }
                }
            }
            put(&mut a.clips, v);
        }
    }
    s.set_pattern(shown);
    s.touched.views.push(crate::session::View::PatternsRenumbered);
    s.touched.clips = true;
}

/// Whether target `t` names track `n` or one of its slots.
fn on_track(t: u32, n: u16) -> bool {
    target::moved(t, &|x| (x != n).then_some(x), &|x, k| (x != n).then_some((x, k))).is_none()
}

fn track(p: &Project, n: u16) -> TrackGone {
    let track = p.mixer.iter().find(|m| m.index == n).cloned().unwrap_or_else(|| MixerTrack::new(n));
    let others = || p.mixer.iter().filter(move |m| m.index != n);
    let routes = others().flat_map(|m| m.routes.iter().enumerate().filter(|&(_, &r)| r == n).map(|(i, _)| (m.id.0, i))).collect();
    let sends = others().flat_map(|m| m.send_levels.iter().enumerate().filter(|&(_, &(d, _))| d == n).map(|(i, &(_, v))| (m.id.0, i, v))).collect();
    let channels = p.channels.iter().filter(|c| c.mixer_track == n).map(|c| c.id.0).collect();
    let links = p
        .arrangements
        .iter()
        .flat_map(|a| a.tracks.iter().enumerate().filter(|(_, t)| t.link.mixer == Some(n)).map(|(i, t)| (a.id.0, i, t.link)))
        .collect();
    let targets = p
        .automations
        .iter()
        .enumerate()
        .flat_map(|(ai, a)| a.targets.iter().enumerate().filter(|&(_, &t)| on_track(t, n)).map(move |(ti, &t)| (ai, ti, t)))
        .collect();
    let controls = p.patterns.iter().map(|q| (q.id.0, take(&q.controls, |c| on_track(c.kind, n)))).filter(|(_, v)| !v.is_empty()).collect();
    let init = take(&p.init_controls, |c| on_track(c.target, n));
    let controller_links = take(&p.controller_links, |l| on_track(l.target(), n) || l.source_slot().is_some_and(|(t, _)| t == n));
    TrackGone { track, routes, sends, channels, links, targets, controls, init, controller_links }
}

fn put_track(s: &mut crate::session::Session, g: TrackGone) {
    let TrackGone { track, routes, sends, channels, links, targets, controls, init, controller_links } = g;
    let n = track.index;
    let hidden = s.project.mixer.len() as u16 - 1;
    // The other way from `edit::mixer::delete_track_in`: the tracks after it move one up again over the empty one it
    // made before the hidden last track, and every number naming them follows.
    let up = move |t: u16| if t >= n && t + 1 < hidden { t + 1 } else { t };
    let p = s.project.get_mut();
    for t in (n..hidden.saturating_sub(1)).rev() {
        if let Some(from) = p.mixer.iter().find(|m| m.index == t).cloned() {
            if let Some(dst) = p.mixer.iter_mut().find(|m| m.index == t + 1) {
                *dst = MixerTrack { index: t + 1, ..from };
            }
        }
    }
    if let Some(dst) = p.mixer.iter_mut().find(|m| m.index == n) {
        *dst = MixerTrack { index: n, ..track };
    }
    for m in p.mixer.iter_mut().filter(|m| m.index != n) {
        m.routes.iter_mut().for_each(|r| *r = up(*r));
        m.send_levels.iter_mut().for_each(|(d, _)| *d = up(*d));
    }
    for c in &mut p.channels {
        c.mixer_track = up(c.mixer_track);
    }
    p.remap_link_mixers(|t| Some(up(t)));
    let view = crate::session::View::places(p, &|t| Some(up(t)), &|t, k| Some((up(t), k)));
    p.follow_mixer_places(&|t| Some(up(t)), &|t, k| Some((up(t), k)));
    // What went with it, where it was.
    for (id, i) in routes {
        if let Some(m) = p.mixer.iter_mut().find(|m| m.id.0 == id) {
            let i = i.min(m.routes.len());
            m.routes.insert(i, n);
        }
    }
    for (id, i, v) in sends {
        if let Some(m) = p.mixer.iter_mut().find(|m| m.id.0 == id) {
            let i = i.min(m.send_levels.len());
            m.send_levels.insert(i, (n, v));
        }
    }
    for c in p.channels.iter_mut().filter(|c| channels.contains(&c.id.0)) {
        c.mixer_track = n;
    }
    for (arr, i, link) in links {
        if let Some(t) = p.arrangements.iter_mut().find(|a| a.id.0 == arr).and_then(|a| a.tracks.get_mut(i)) {
            t.link = link;
        }
    }
    for (ai, ti, t) in targets {
        if let Some(a) = p.automations.get_mut(ai) {
            let ti = ti.min(a.targets.len());
            a.targets.insert(ti, t);
        }
    }
    for (pat, v) in controls {
        if let Some(q) = p.patterns.iter_mut().find(|q| q.id.0 == pat) {
            put(&mut q.controls, v);
        }
    }
    put(&mut p.init_controls, init);
    put(&mut p.controller_links, controller_links);
    s.touched.views.push(view);
    s.touched.views.push(crate::session::View::MixerSelect(n));
    s.touched.tracks = true;
}

/// The delete of `thing` again (a delete's redo), the patterns it makes to fill numbers given `made`'s ids.
pub fn delete_again(s: &mut crate::session::Session, thing: Thing, made: Vec<u32>) -> Result<(), Error> {
    use super::command::Command;
    match thing {
        Thing::Channel(id) => Command::DeleteChannel { channel: id }.run_change(s),
        Thing::Track(id) => {
            Command::DeleteTrack { track: id }.run_change(s)?;
            let len = s.project.mixer.len();
            if let (Some(m), Some(id)) = (s.project.get_mut().mixer.iter_mut().find(|m| usize::from(m.index) + 2 == len && m.id.is_none()), made.first()) {
                m.id = os_model::Id(*id);
            }
            Ok(())
        }
        Thing::Pattern(id) => {
            Command::DeletePattern { pattern: id }.run_change(s)?;
            let mut made = made.into_iter();
            for q in s.project.get_mut().patterns.iter_mut().filter(|q| q.id.is_none()) {
                match made.next() {
                    Some(m) => q.id = os_model::Id(m),
                    None => break,
                }
            }
            Ok(())
        }
    }
}
