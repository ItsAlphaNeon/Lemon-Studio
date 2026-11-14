// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The interface's side of `crate::remote` (step 9: the session's side stays there).

#![allow(unused_imports)]
use std::collections::VecDeque;
use std::time::{Duration, Instant};
use crate::app::{App, State};
use crate::knobs::KnobId;
use crate::playlist::{Cmd, PItem};
use crate::typein::Target;
#[allow(unused_imports)]
use crate::remote::*;

/// A link's row in the browser's Remote control section (Lemon Studio's design: spec ui/browser.md "Current project
/// children"): what it drives, named with its owner as automation clips are ("Kick - Channel volume", "Bus 1 - To
/// Bus 2", "Lemon Reverb - Bus 3 - Mix"), then its controller as the Remote control settings box sets it ("channel 1,
/// ctrl 7"), and "omni" for one that drives the selected channel's knob.
pub fn label(s: &State, l: &Link) -> String {
    let channel = |r: usize| crate::views::browser::channel_label(s, r).unwrap_or_else(|| format!("Channel {}", r + 1));
    let name = crate::typein_ui::target_name(s, l.target);
    let what = match l.target {
        Target::Knob(KnobId::ChanVol(r) | KnobId::ChanPan(r)) if !l.omni => format!("{} - {name}", channel(r)),
        Target::RackTrack(r) | Target::RackLed(r) => format!("{} - {name}", channel(r)),
        Target::Knob(KnobId::Send(a, _)) => format!("{} - {name}", crate::edit::mixer::name(&s.session, a)),
        Target::Param(key, index) => crate::params_ui::clip_name(s, key, index),
        Target::MemberParam(m, k, id, index) => match crate::edit::container::member_key(&s.session, crate::container::place_key(m, k), id) {
            Some(key) => crate::params_ui::clip_name(s, key, index),
            None => format!("{} - {name}", crate::edit::container::place_name(&s.session, m, k)),
        },
        _ => name,
    };
    let omni = if l.omni { ", omni" } else { "" };
    format!("{what}: channel {}, ctrl {}{omni}", l.ch + 1, l.cc)
}

/// The menu's Remote control part (spec §1): Link to controller..., Override global link... (greyed), and Remove link
/// for a linked target.
pub fn menu_items(t: Option<Target>) -> Vec<PItem> {
    let t = t.filter(|&t| linkable(t));
    let mut v = vec![
        PItem::head("Remote control"),
        match t {
            Some(t) => PItem::new("MIDI learn...", Cmd::Remote(RCmd::Link(t))),
            None => PItem::not_yet("MIDI learn..."),
        },
        PItem::new("Override global mapping...", Cmd::Nothing).off(),
    ];
    if let Some(t) = t.filter(|&t| link_of(t).is_some()) {
        v.push(PItem::new("Remove link", Cmd::Remote(RCmd::Unlink(t))));
    }
    v
}

impl App {
    /// The knob a link drives now (Omni: the selected channel's).
    fn remote_target(&self, l: &Link) -> Target {
        match l.target {
            Target::Knob(KnobId::ChanVol(_)) if l.omni => Target::Knob(KnobId::ChanVol(self.state.selected_channel)),
            Target::Knob(KnobId::ChanPan(_)) if l.omni => Target::Knob(KnobId::ChanPan(self.state.selected_channel)),
            t => t,
        }
    }

    /// A controller message from device `dev` (spec §2-§5): the open box detects it, Multilink links it, the links it
    /// drives take it. Returns whether something took it (otherwise it goes to the instrument).
    pub(crate) fn remote_cc(&mut self, dev: &str, ch: u8, cc: u8, v: u8) -> bool {
        let port = crate::midi_cfg::input(dev).port;
        if let Some(b) = self.state.link_box.as_mut().filter(|b| b.auto) {
            b.port = port;
            b.ch = ch;
            b.cc = Some(cc);
            self.state.message = Some(format!("Detected: channel {}, controller {cc}", ch + 1));
            if crate::prefs::check(&crate::prefs::AUTO_ACCEPT) {
                self.link_accept();
            }
            self.invalidate();
            return true;
        }
        self.remote.sync();
        let hit: Vec<usize> = (0..self.remote.links.len()).filter(|&i| self.remote.links[i].hears(port, ch, cc)).collect();
        if hit.is_empty() && self.state.multilink.on {
            if let Some(t) = self.state.multilink.queue.pop_front() {
                let mut l = Link::new(ch, cc, t);
                l.port = port;
                add(l, true);
                self.remote.sync();
                let name = crate::typein_ui::caption(&self.state, t);
                self.state.message = Some(format!("Linked channel {}, controller {cc} to {}", ch + 1, name.trim_start_matches("Value for ")));
                if self.state.multilink.queue.is_empty() {
                    self.state.multilink.on = false;
                }
                self.invalidate();
                return true;
            }
        }
        for i in hit.iter().copied() {
            self.remote_drive(i, v);
        }
        !hit.is_empty()
    }

    /// Link `i` gets controller value `v` (spec §5).
    fn remote_drive(&mut self, i: usize, v: u8) {
        let l = self.remote.links[i];
        let t = self.remote_target(&l);
        // A rack box's position over its travel (its track over the last insert, the LED 0 / 1), a knob's value.
        let now = crate::typein_ui::position(&self.state, t).or_else(|| crate::typein_ui::get(&self.state, t));
        let Some(now) = now.map(|x| x as f32) else { return };
        let x = l.formula.map(f32::from(v) / 127.0);
        let pressed = v > 0;
        let val = match l.mode {
            Mode::Default | Mode::Hold => x,
            Mode::Stay if !pressed => return,
            Mode::Stay => x,
            Mode::Latch if !pressed => return,
            Mode::Latch => {
                if now >= 0.5 { 0.0 } else { 1.0 }
            }
            Mode::Inc if !pressed => return,
            Mode::Inc => {
                let n = now + 1.0 / 16.0;
                if n > 1.0 + 1e-4 { 0.0 } else { n.min(1.0) }
            }
        };
        let live = &mut self.remote.live[i];
        // Pickup (takeover mode): after another change, nothing until the controller meets the knob.
        if crate::prefs::check(&crate::prefs::PICKUP) && matches!(l.mode, Mode::Default | Mode::Hold | Mode::Stay) {
            if live.set.is_none_or(|s| (s - now).abs() > 1e-4) {
                live.engaged = false;
            }
            if !live.engaged {
                let met = (val - now).abs() <= NEAR || live.last_in.is_some_and(|p| (p - now) * (val - now) <= 0.0);
                live.last_in = Some(val);
                if !met {
                    return;
                }
                live.engaged = true;
            }
        }
        live.last_in = Some(val);
        match l.smooth {
            Some(ms) => {
                live.glide = Some((now, val, crate::anim::now(), Duration::from_millis(u64::from(ms))));
            }
            None => {
                live.set = Some(val);
                self.remote_set(t, val);
            }
        }
    }

    /// The glides move on (spec §3 Smoothing).
    pub(crate) fn remote_tick(&mut self) {
        if !self.remote.busy() {
            return;
        }
        for i in 0..self.remote.live.len() {
            let Some((from, to, since, over)) = self.remote.live[i].glide else { continue };
            let k = (crate::anim::now().duration_since(since).as_secs_f32() / over.as_secs_f32().max(1e-3)).min(1.0);
            let v = from + (to - from) * k;
            if k >= 1.0 {
                self.remote.live[i].glide = None;
            }
            self.remote.live[i].set = Some(v);
            let t = self.remote_target(&self.remote.links[i]);
            self.remote_set(t, v);
        }
    }

    /// Sets the knob as a change of it with the mouse would: undoable (one step while the same knob keeps changing),
    /// recorded while recording automation.
    fn remote_set(&mut self, t: Target, v: f32) {
        let v = v.clamp(0.0, 1.0);
        match t {
            Target::Knob(k) => {
                crate::history::knob(&mut self.state.session, k, true);
                self.state.set_knob(k, v);
                self.record_knob(k);
            }
            Target::Fader(m) => {
                if let Some(tr) = crate::edit::mixer::track_mut(&mut self.state.session, m) {
                    tr.volume = Some(f64::from(v));
                    self.state.session.dirty = true;
                }
                self.record_control(&crate::app::Action::Fader(m));
            }
            Target::Param(key, index) => {
                crate::params_ui::set_param_value(&mut self.state, key, index, f64::from(v));
                self.record_plugin_param(key, index, f64::from(v));
            }
            // An effect inside a container: its instance while it is loaded.
            Target::MemberParam(m, k, id, index) => {
                if let Some(key) = crate::edit::container::member_key(&self.state.session, crate::container::place_key(m, k), id) {
                    crate::params_ui::set_param_value(&mut self.state, key, index, f64::from(v));
                    self.record_plugin_param(key, index, f64::from(v));
                }
            }
            // A container member's level: one undo step while it keeps moving; recorded as its automation target.
            Target::Member(m, k, id, wet) => {
                crate::container_ui::set_level_run(&mut self.state, m, k, id, wet, v);
                if let Some(target) = crate::edit::container::level_target(&self.state.session, m, k, id, wet) {
                    self.record_param(target, (0.0, 1.0), f64::from(v));
                }
            }
            // One undo step for each track the box moves to, and each switch of the LED (`typein::set_value`).
            Target::RackTrack(_) | Target::RackLed(_) => {
                let to = crate::typein_ui::at_position(&self.state, t, f64::from(v));
                crate::typein_ui::set_value(&mut self.state, t, to);
            }
            // Never linked (no remote control).
            Target::Latency(..) => {}
        }
        self.invalidate();
    }

    /// A menu's Remote control command.
    pub(crate) fn remote_cmd(&mut self, c: RCmd) {
        match c {
            RCmd::Link(t) => {
                let (w, h) = crate::views::remote_box::SIZE;
                let (sw, sh) = self.state.screen;
                let at = ((self.mouse.0 - 20.0).clamp(0.0, (sw - w).max(0.0)), (self.mouse.1 - 20.0).clamp(0.0, (sh - h).max(0.0)));
                self.state.link_box = Some(LinkBox::new(t, at));
                self.state.message = Some("Just move your MIDI controller".into());
            }
            RCmd::Unlink(t) => {
                remove(t);
                self.state.message = Some("Link removed".into());
            }
            RCmd::LinkAll(key) => {
                self.params_tick();
                let params: Vec<usize> = self.state.session.params.visible(key).map(|(i, _)| i).collect();
                if params.is_empty() {
                    self.state.message = Some("This plugin has no parameters to link".into());
                    return self.invalidate();
                }
                let (w, h) = crate::views::remote_box::SIZE;
                let (sw, sh) = self.state.screen;
                let at = ((self.mouse.0 - 20.0).clamp(0.0, (sw - w).max(0.0)), (self.mouse.1 - 20.0).clamp(0.0, (sh - h).max(0.0)));
                // An effect inside a container links by its place and id (`params::link_target`).
                let targets = params.into_iter().map(|i| crate::params::link_target(&self.state.session, key, i)).collect();
                self.state.link_box = Some(LinkBox::group(targets, at));
                self.state.message = Some("Move the MIDI controller for the first parameter".into());
            }
            RCmd::LinkTrack(t) => {
                let targets = track_targets(&self.state.session, t);
                if targets.is_empty() {
                    return self.invalidate();
                }
                let (w, h) = crate::views::remote_box::SIZE;
                let (sw, sh) = self.state.screen;
                let at = ((self.mouse.0 - 20.0).clamp(0.0, (sw - w).max(0.0)), (self.mouse.1 - 20.0).clamp(0.0, (sh - h).max(0.0)));
                self.state.link_box = Some(LinkBox::group(targets, at));
                self.state.message = Some("Move the MIDI controller for the track's volume".into());
            }
        }
        self.invalidate();
    }

    /// Accept: makes the link and closes the box (spec §3).
    pub(crate) fn link_accept(&mut self) {
        let Some(b) = self.state.link_box.take() else { return };
        if !b.all.is_empty() {
            let made = b.links();
            for l in &made {
                add(*l, b.conflicts);
            }
            self.state.message = Some(match (made.first(), made.last()) {
                (Some(a), Some(z)) => format!(
                    "Linked {} parameters of {} to channel {}, controllers {} to {}",
                    made.len(),
                    group_name(&self.state, &b),
                    a.ch + 1,
                    a.cc,
                    z.cc
                ),
                _ => "No controller: nothing linked".into(),
            });
            return self.invalidate();
        }
        match b.link() {
            Some(l) => {
                add(l, b.conflicts);
                let name = crate::typein_ui::caption(&self.state, b.target);
                self.state.message = Some(format!("Linked channel {}, controller {} to {}", l.ch + 1, l.cc, name.trim_start_matches("Value for ")));
            }
            None => self.state.message = Some("No controller: nothing linked".into()),
        }
        self.invalidate();
    }

    /// A click on the box.
    pub(crate) fn link_box_act(&mut self, a: RAct) {
        let Some(b) = self.state.link_box.as_mut() else { return };
        match a {
            RAct::Body | RAct::Port | RAct::Channel | RAct::Ctrl | RAct::Time => {}
            RAct::Close => self.state.link_box = None,
            RAct::Auto => b.auto = !b.auto,
            RAct::Omni => b.omni = !b.omni,
            RAct::Smooth => b.smooth = !b.smooth,
            RAct::Conflicts => b.conflicts = !b.conflicts,
            RAct::Reset => b.reset(),
            RAct::Accept => return self.link_accept(),
            RAct::Mode => {
                let i = Mode::ALL.iter().position(|&m| m == b.mode).unwrap_or(0);
                b.mode = Mode::ALL[(i + 1) % Mode::ALL.len()];
            }
            RAct::Formula => {
                let i = Formula::ALL.iter().position(|&f| f == b.formula).unwrap_or(0);
                b.formula = Formula::ALL[(i + 1) % Formula::ALL.len()];
            }
        }
        self.invalidate();
    }

    /// The wheel (or a drag) on one of the box's value boxes: `steps` up or down.
    pub(crate) fn link_box_step(&mut self, a: RAct, steps: i32) {
        let Some(b) = self.state.link_box.as_mut() else { return };
        let step = |v: Option<u8>, max: i32| -> Option<u8> {
            // "---" sits below 0.
            let n = v.map_or(-1, i32::from) + steps;
            (n >= 0).then(|| n.min(max) as u8)
        };
        match a {
            RAct::Port => b.port = step(b.port, 255),
            RAct::Ctrl => b.cc = step(b.cc, 127),
            RAct::Channel => b.ch = (i32::from(b.ch) + steps).clamp(0, 15) as u8,
            RAct::Time => b.time = (i32::from(b.time) + steps * 10).clamp(i32::from(SMOOTH_MIN), i32::from(SMOOTH_MAX)) as u16,
            _ => return,
        }
        self.invalidate();
    }

    /// Multilink to controllers switched (OPTIONS, Ctrl+J, the toolbar; spec §4).
    pub(crate) fn toggle_multilink(&mut self) {
        let m = &mut self.state.multilink;
        m.on = !m.on;
        m.queue.clear();
        self.state.message = Some(if m.on {
            "Multi MIDI learn: change the knobs to link, then move a controller for each, in the same order".into()
        } else {
            "Multi MIDI learn: off".into()
        });
        self.invalidate();
    }

    /// A knob changed with the mouse: Multilink queues it (once).
    pub(crate) fn multilink_knob(&mut self, t: Target) {
        let m = &mut self.state.multilink;
        if m.on && !m.queue.contains(&t) {
            m.queue.push_back(t);
            let name = crate::typein_ui::caption(&self.state, t);
            let n = self.state.multilink.queue.len();
            self.state.message = Some(format!("MIDI learn {n}: {}", name.trim_start_matches("Value for ")));
        }
    }
}

/// What Link all parameters... links (the box's field, its hint): the plugin's name, or the mixer track's.
pub fn group_name(s: &State, b: &LinkBox) -> String {
    match b.target {
        Target::Param(key, _) => crate::params::owner_name(&s.session, key),
        Target::MemberParam(m, k, id, _) => match crate::edit::container::member_key(&s.session, crate::container::place_key(m, k), id) {
            Some(key) => crate::params::owner_name(&s.session, key),
            None => crate::edit::container::place_name(&s.session, m, k),
        },
        Target::Fader(t) => crate::edit::mixer::name(&s.session, t),
        t => crate::typein_ui::caption(s, t).trim_start_matches("Value for ").to_string(),
    }
}

/// Whether the box is open (it is modal).
pub fn is_open(s: &State) -> bool {
    s.link_box.is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// VIEW > Remote control works: the browser on the Current project tab with its Remote control section open and
    /// selected, one row per link, named after what it drives and its controller.
    #[test]
    fn view_remote_control_lists_the_links() {
        use crate::views::browser::{rows, section_key, REMOTE_CONTROL};
        use crate::views::menus::{item_at, works, VIEW};
        let mut app = App::new(None);
        app.state.session.project = crate::locked::Locked::new(crate::projects::test_project());
        set_links(&[]);
        add(Link::new(0, 7, Target::Knob(KnobId::ChanVol(1))), true);
        add(Link { omni: true, ..Link::new(1, 10, Target::Knob(KnobId::ChanPan(0))) }, true);
        add(Link::new(15, 74, Target::Fader(2)), true);
        let at = (0..40).find(|&i| item_at(VIEW, i).is_some_and(|it| it.label == "Remote control")).unwrap();
        assert!(works(VIEW, item_at(VIEW, at).unwrap()));
        app.state.show_browser = false;
        app.menu_item(VIEW, at);
        let key = section_key(REMOTE_CONTROL).unwrap();
        assert!(app.state.show_browser && app.state.browser_open.contains(&key) && app.state.browser_sel.as_ref() == Some(&key));
        let listed: Vec<String> = rows(&app.state).into_iter().filter(|r| r.key.starts_with(&format!("{key}/"))).map(|r| r.label).collect();
        let bus2 = crate::edit::mixer::name(&app.state.session, 2);
        assert_eq!(listed, ["Clap - Channel volume: channel 1, ctrl 7".to_string(), "Channel panning: channel 2, ctrl 10, omni".into(), format!("{bus2} - Volume: channel 16, ctrl 74")]);
        set_links(&[]);
    }

    /// The mixer's Link all parameters... (Lemon Studio's design): the current track's volume, pan, separation, nine
    /// equaliser knobs, its sends and its filled slots' mix levels, linked on the first controller and the ones after it.
    #[test]
    fn the_mixer_links_a_tracks_controls() {
        use crate::playlist::Cmd;
        let mut app = App::new(None);
        app.state.session.project = crate::locked::Locked::new(crate::projects::test_project());
        set_links(&[]);
        app.state.selected_mixer = 1;
        if let Some(t) = crate::edit::mixer::track_mut(&mut app.state.session, 1) {
            t.routes = vec![2, 0];
        }
        let menu = crate::mixer::options_menu(&app.state, 0.0, 0.0);
        let item = menu.items.iter().find(|i| i.label == "Link all parameters...").unwrap();
        assert!(item.enabled);
        assert_eq!(item.cmd, Cmd::Remote(RCmd::LinkTrack(1)));
        crate::playlist::run(&mut app, item.cmd.clone());
        let b = app.state.link_box.clone().expect("the box");
        let mut want = vec![Target::Fader(1), Target::Knob(KnobId::MixPan(1)), Target::Knob(KnobId::MixSep(1))];
        want.extend((0..3).flat_map(|b| (0..3).map(move |f| Target::Knob(KnobId::Eq(1, b, f)))));
        want.extend([Target::Knob(KnobId::Send(1, 0)), Target::Knob(KnobId::Send(1, 2))]);
        want.extend((0..10u8).filter(|&k| crate::edit::mixer::slot_name(&app.state.session, 1, k).is_some()).map(|k| Target::Knob(KnobId::SlotMix(1, k))));
        assert_eq!((b.target, &b.all), (Target::Fader(1), &want));
        // The controller moved: every control linked, controllers 20 and on.
        app.remote_cc("Test", 0, 20, 64);
        assert!(app.state.link_box.is_none(), "auto accepted");
        let made = links();
        assert_eq!(made.iter().map(|l| l.target).collect::<Vec<_>>(), want);
        assert_eq!(made.iter().map(|l| l.cc).collect::<Vec<_>>(), (20..20 + want.len() as u8).collect::<Vec<_>>());
        assert!(app.state.message.as_deref().is_some_and(|m| m.contains(&crate::edit::mixer::name(&app.state.session, 1))), "{:?}", app.state.message);
        // The volume follows its controller.
        assert!(app.remote_cc("Test", 0, 20, 127));
        assert_eq!(crate::edit::mixer::track(&app.state.session, 1).and_then(|t| t.volume), Some(1.0));
        set_links(&[]);
    }

}
