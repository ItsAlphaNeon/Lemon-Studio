// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! What an automation clip drives (spec: ui/automation-targets.md): Audio clip ▸ Automate ▸ (§1), the clip window's
//! Target links list with Add, Delete and Locate (§3), and the Remote control settings box's Internal controller, an
//! existing clip the control joins (§4). The double-click that opens a clip's window is in `crate::playlist`.

use os_model::target::{self, Target as T};
use os_model::ChannelKind;
use os_ui::{Font, Rect};

use crate::app::{Action, App, State, Ui, Win};
use crate::control::command::Command;
use crate::playlist::{Cmd, PItem};

/// The undo step of a change to a clip's targets.
pub const TARGETS: &str = "Automation clip targets";
/// The Target links part of a clip window: its height.
pub const LINKS_H: f32 = 116.0;
const ROW_H: f32 = 18.0;
const ROWS: usize = 4;

/// The Target links list's state.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Links {
    /// The clip (its channel index) whose Add is on: every control tweaked joins its targets (§3).
    pub adding: Option<u16>,
    /// The selected row of clip `of`'s list (`sel_row`: every other clip's list starts at its first row).
    pub sel: usize,
    pub of: Option<u16>,
}

/// The selected row of clip `ch`'s list of `len` targets: the row chosen in it, kept inside the list.
fn sel_row(s: &State, ch: u16, len: usize) -> usize {
    if s.auto_links.of == Some(ch) { s.auto_links.sel.min(len.saturating_sub(1)) } else { 0 }
}

/// A part of the Target links list (`Action::Targets`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum THit {
    Add,
    Delete,
    Locate,
    Row(usize),
}

/// Audio clip ▸ Automate ▸ (§1).
pub fn automate_item(i: usize) -> PItem {
    PItem::sub(
        "Automate",
        vec![PItem::new("Volume", Cmd::ClipAutomate(i, false)), PItem::new("Panning", Cmd::ClipAutomate(i, true))],
    )
}

/// Audio clip `i` ▸ Automate ▸ Volume / Panning (§1): a clip on its channel's volume or panning at the value it has,
/// over the audio clip's time, on the first free track below it. One undo step, as Create automation clip.
pub fn automate_clip(s: &mut State, i: usize, pan: bool) -> Option<String> {
    let c = *crate::playlist::clips(&s.session).get(i)?;
    let os_model::ClipSource::Channel(ch) = c.source else { return None };
    let chan = s.session.project.channel(ch).filter(|k| k.kind == ChannelKind::AudioClip)?;
    let (tgt, v) = if pan {
        (target::channel(ch, target::CHANNEL_PAN), 0.5 + chan.pan / 2.0)
    } else {
        (target::channel(ch, target::CHANNEL_VOLUME), chan.volume)
    };
    let name = crate::init_ui::name(s, tgt);
    let below = (c.track + 1).min(crate::edit::playlist::TRACKS - 1);
    crate::playlist::create_automation_over(s, tgt, &name, v, Some((c.position, c.end(), below)));
    s.raise(Win::Playlist);
    Some(format!("Created automation clip \"{name}\""))
}

/// Clip `ch`'s targets, in order.
pub fn targets(s: &State, ch: u16) -> Vec<u32> {
    s.session.project.automation(ch).map(|a| a.targets.clone()).unwrap_or_default()
}

fn set_targets(s: &mut State, ch: u16, targets: Vec<u32>) -> Option<String> {
    let id = s.session.project.channel(ch)?.id.0;
    crate::control_ui::command(s, TARGETS, None, vec![Command::SetTargets { channel: id, targets }]).err()
}

/// `tgt` joins clip `ch`'s targets, last (§3, §4); a message when it can't.
pub fn link(s: &mut State, ch: u16, tgt: u32) -> Option<String> {
    let mut t = targets(s, ch);
    if t.contains(&tgt) {
        return Some(format!("{} is already a target of this clip", crate::init_ui::name(s, tgt)));
    }
    t.push(tgt);
    set_targets(s, ch, t)
}

/// The project's automation clips, the internal controllers a control can be linked to (§4): channel indexes in
/// rack order.
pub fn clips(s: &State) -> Vec<u16> {
    s.session
        .project
        .channels
        .iter()
        .filter(|c| c.kind == ChannelKind::Automation && s.session.project.automation(c.index).is_some())
        .map(|c| c.index)
        .collect()
}

fn clip_name(s: &State, ch: u16) -> String {
    s.session.project.channel(ch).and_then(|c| c.name.clone()).unwrap_or_else(|| "Automation Clip".into())
}

/// The Internal controller box's text: the chosen clip, or "(none)".
pub fn internal_name(s: &State, ch: Option<u16>) -> String {
    ch.map_or_else(|| "(none)".into(), |ch| clip_name(s, ch))
}

/// The Internal controller box clicked: the next clip, then none again.
pub fn next_internal(s: &State, cur: Option<u16>) -> Option<u16> {
    let all = clips(s);
    match cur.and_then(|c| all.iter().position(|&x| x == c)) {
        None => all.first().copied(),
        Some(i) => all.get(i + 1).copied(),
    }
}

/// The automation target of a control the Remote control settings box links.
pub fn auto_id(app: &App, t: crate::typein::Target) -> Option<u32> {
    use crate::typein::Target as R;
    match t {
        R::Knob(k) => app.automation_target(&Action::Knob(k)).map(|x| x.0),
        R::Fader(n) => app.automation_target(&Action::Fader(n)).map(|x| x.0),
        R::Param(key, index) => crate::params_ui::automation(&app.state, key, index).map(|x| x.0),
        _ => None,
    }
}

/// Accept with an internal controller chosen (§4): the control joins that clip's targets. With Remove conflicts
/// (`conflicts`) its earlier links go: it leaves every other clip's targets, in the same undo step, and its MIDI
/// controller link goes. The message.
pub fn accept_internal(app: &mut App, t: crate::typein::Target, ch: u16, conflicts: bool) -> String {
    let Some(tgt) = auto_id(app, t) else { return "This control can't be driven by an automation clip".into() };
    let s = &mut app.state;
    let mut c = Vec::new();
    if conflicts {
        for other in clips(s).into_iter().filter(|&o| o != ch) {
            let mut list = targets(s, other);
            if list.contains(&tgt) {
                list.retain(|&x| x != tgt);
                c.extend(s.session.project.channel(other).map(|k| Command::SetTargets { channel: k.id.0, targets: list }));
            }
        }
        let st = &s.session.settings;
        let midi = crate::remote::links(st);
        if midi.iter().any(|l| l.target == t) {
            crate::remote::set_links(st, &midi.into_iter().filter(|l| l.target != t).collect::<Vec<_>>());
        }
    }
    let mut mine = targets(s, ch);
    let already = mine.contains(&tgt);
    if !already {
        mine.push(tgt);
        c.extend(s.session.project.channel(ch).map(|k| Command::SetTargets { channel: k.id.0, targets: mine }));
    }
    if already && c.is_empty() {
        return format!("{} is already a target of this clip", crate::init_ui::name(s, tgt));
    }
    if let Err(e) = crate::control_ui::command(s, TARGETS, None, c) {
        return e;
    }
    format!("Linked {} to automation clip \"{}\"", crate::init_ui::name(s, tgt), clip_name(s, ch))
}

/// A control was tweaked (`crate::params_ui`): while a clip's Add is on, its automation target joins that clip (§3).
/// The clip's own range knobs don't.
pub fn tweaked(app: &mut App) {
    let Some(ch) = app.state.auto_links.adding else { return };
    if app.state.session.project.automation(ch).is_none() {
        app.state.auto_links.adding = None;
        return;
    }
    let Some(t) = app.state.tweaks.first().cloned() else { return };
    let Some((tgt, ..)) = app.tweak_automation(&t) else { return };
    if matches!(target::decode(tgt), T::ChannelVolume(c) | T::ChannelPan(c) if c == ch)
        || targets(&app.state, ch).contains(&tgt)
    {
        return;
    }
    let m = link(&mut app.state, ch, tgt)
        .unwrap_or_else(|| format!("Added target {}", crate::init_ui::name(&app.state, tgt)));
    app.state.message = Some(m);
}

/// A click on the Target links list of the clip window shown.
pub fn hit(app: &mut App, h: THit) -> Option<String> {
    let s = &mut app.state;
    let ch = s.session.project.channels.get(s.plugin_channel)?.index;
    let list = targets(s, ch);
    let sel = sel_row(s, ch, list.len());
    match h {
        THit::Row(i) => {
            (s.auto_links.sel, s.auto_links.of) = (i, Some(ch));
            None
        }
        THit::Add if s.auto_links.adding == Some(ch) => {
            s.auto_links.adding = None;
            Some("Adding targets: off".into())
        }
        THit::Add => {
            s.auto_links.adding = Some(ch);
            Some("Adding targets: move the controls this clip should drive, then click + again".into())
        }
        THit::Delete => {
            if sel >= list.len() {
                return Some("No target selected".into());
            }
            let mut t = list;
            t.remove(sel);
            (s.auto_links.sel, s.auto_links.of) = (sel.min(t.len().saturating_sub(1)), Some(ch));
            set_targets(s, ch, t)
        }
        THit::Locate => {
            let t = *list.get(sel)?;
            locate(app, t)
        }
    }
}

/// Locate (§3): the window that holds target `t`'s control: a channel's settings, or the mixer on its track.
fn locate(app: &mut App, t: u32) -> Option<String> {
    let s = &mut app.state;
    let channel = match target::decode(t) {
        T::ChannelVolume(c)
        | T::ChannelPan(c)
        | T::ChannelPitch(c)
        | T::ChannelLevel(c, _)
        | T::ChannelOn(c)
        | T::ChannelMixerTrack(c)
        | T::ChannelParam(c, _) => Some(c),
        T::MixerVolume(m)
        | T::MixerPan(m)
        | T::MixerSeparation(m)
        | T::MixerEq(m, ..)
        | T::MixerSend(m, _)
        | T::SlotOn(m, _)
        | T::SlotMix(m, _)
        | T::SlotParam(m, ..) => {
            s.raise(Win::Mixer);
            crate::mixer::select(s, m, false);
            return None;
        }
        _ => None,
    };
    let Some(row) = channel.and_then(|c| s.session.project.channels.iter().position(|k| k.index == c)) else {
        return Some(format!("{} has no window", crate::init_ui::name(s, t)));
    };
    s.rack.select_only(row);
    app.focus_channel(row);
    app.open_channel_window(row);
    None
}

/// The Target links part of a clip window, in `r` (§3): its title with +, Delete and Locate, then a row per target.
pub fn paint_links(ui: &mut Ui, s: &State, ch: u16, r: Rect) {
    let t = ui.t;
    ui.c.fill_rect(Rect::new(r.x, r.y, r.w, 1.0), t.rack_inset_border);
    let f = Font::new(15);
    ui.c.text(f, r.x + 4.0, r.y + 8.0, t.text_dim, "Target links");
    let adding = s.auto_links.adding == Some(ch);
    let mut bx = r.right() - 4.0;
    for (label, h, hint, on) in [
        ("Locate", THit::Locate, "Open the window that holds the selected target", false),
        ("Delete", THit::Delete, "Remove the selected target", false),
        ("+", THit::Add, "Add: move controls to make them targets, click again to stop", adding),
    ] {
        let w = ui.c.text_width(Font::new(13), label) + 14.0;
        bx -= w;
        let b = Rect::new(bx, r.y + 6.0, w, 20.0);
        bx -= 4.0;
        let hov = ui.hit(b, Action::Targets(h), hint);
        ui.c.fill_rect(
            b,
            if on {
                t.led(0xa0dd48)
            } else if hov {
                t.s(0x4a4f53)
            } else {
                t.s(0x34383b)
            },
        );
        ui.c.text_center(Font::new(13), b, if on { t.s(0x1c1e21) } else { t.text }, label);
    }
    let list = Rect::new(r.x + 4.0, r.y + 32.0, r.w - 8.0, ROW_H * ROWS as f32 + 4.0);
    ui.c.fill_rect(list, t.s(0x1c1e21));
    let targets = targets(s, ch);
    if targets.is_empty() {
        ui.c.text(
            Font::new(13),
            list.x + 6.0,
            list.y + 4.0,
            t.text_dim,
            if adding { "Move a control to add it" } else { "(none)" },
        );
    }
    let sel = sel_row(s, ch, targets.len());
    let first = sel.saturating_sub(ROWS - 1);
    for (i, &tgt) in targets.iter().enumerate().skip(first).take(ROWS) {
        let row = Rect::new(list.x, list.y + 2.0 + ROW_H * (i - first) as f32, list.w, ROW_H);
        ui.hit(row, Action::Targets(THit::Row(i)), "");
        if i == sel {
            ui.c.fill_rect(row, t.s(0x3d4f5c));
        }
        ui.c.text_clip(Font::new(13), row.x + 6.0, row.y + 1.0, row.w - 12.0, t.text, &crate::init_ui::name(s, tgt));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::knobs::KnobId;
    use crate::playlist as pl;
    use os_ui::Canvas;

    /// The test project, the playlist shown, and an audio clip on playlist track 2 from bar 2; its index.
    fn app(c: &mut Canvas) -> (App, usize) {
        let mut a = App::new(None);
        a.state.session.project = crate::locked::Locked::new(crate::projects::test_project());
        a.state.playlist.reset();
        a.state.z = vec![Win::Playlist];
        a.paint(c, None);
        let wav = std::env::temp_dir().join(format!(
            "os-app-automate-{}-{:?}.wav",
            std::process::id(),
            std::thread::current().id()
        ));
        os_engine::wav::write_f32_stereo(&mut std::fs::File::create(&wav).unwrap(), 48000, &[0.25; 96000]).unwrap();
        let bar = 4 * u32::from(a.state.session.project.ppq);
        let i = pl::add_audio_clip(&mut a.state, &wav.to_string_lossy(), 1, bar).expect("audio clip added");
        (a, i)
    }

    fn rows(a: &App) -> usize {
        a.state.session.project.channels.len()
    }

    /// spec: ui/automation-targets.md §1: Automate ▸ Volume and Panning on an audio clip make a clip on its channel's
    /// volume or panning, at its value, over the audio clip, below it; one undo step each.
    #[test]
    fn audio_clip_automate_volume_and_panning() {
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let (mut a, i) = app(&mut c);
        let audio = pl::clips(&a.state.session)[i];
        let os_model::ClipSource::Channel(ch) = audio.source else { panic!("an audio clip") };
        let menu = pl::clip_menu(&a.state.session, i, 0.0, 0.0);
        let auto = menu.items.iter().find(|it| it.label == "Automate").expect("Automate ▸");
        let labels: Vec<_> = auto.children.iter().map(|it| it.label.to_string()).collect();
        assert_eq!(labels, ["Volume", "Panning"]);
        assert_eq!(auto.children[0].cmd, Cmd::ClipAutomate(i, false));

        let n = rows(&a);
        let m = pl::run(&mut a, Cmd::ClipAutomate(i, false)).unwrap_or_default();
        assert!(m.starts_with("Created automation clip"), "{m}");
        assert_eq!(rows(&a), n + 1);
        let made = a.state.session.project.channels.last().unwrap().index;
        assert_eq!(targets(&a.state, made), [target::channel(ch, target::CHANNEL_VOLUME)]);
        let clip =
            *pl::clips(&a.state.session).iter().find(|c| c.source == os_model::ClipSource::Channel(made)).unwrap();
        assert_eq!((clip.position, clip.length), (audio.position, audio.length), "over the audio clip");
        assert!(clip.track > audio.track, "below it");
        let vol = a.state.session.project.channel(ch).unwrap().volume;
        let pts = &a.state.session.project.automation(made).unwrap().points;
        assert!(pts.iter().all(|p| (p.value - vol).abs() < 1e-6), "flat at the channel's volume");
        crate::history_ui::undo(&mut a.state);
        assert_eq!(rows(&a), n, "one undo step");

        pl::run(&mut a, Cmd::ClipAutomate(i, true));
        let made = a.state.session.project.channels.last().unwrap().index;
        assert_eq!(targets(&a.state, made), [target::channel(ch, target::CHANNEL_PAN)]);
    }

    /// The clip window of automation clip `made`, painted; the Target links hits.
    fn clip_window(a: &mut App, c: &mut Canvas, made: u16) -> Vec<(THit, Rect)> {
        a.state.plugin_channel = a.state.session.project.channels.iter().position(|k| k.index == made).unwrap();
        a.state.raise(Win::Plugin);
        a.invalidate();
        a.paint(c, None);
        a.hits.iter().filter_map(|h| if let Action::Targets(t) = h.action { Some((t, h.rect)) } else { None }).collect()
    }

    /// spec: ui/automation-targets.md §3: + switches adding on, every control moved then joins the clip (not its own
    /// range knobs), + again stops; Delete removes the selected row; Locate opens the target's window. Painted: the
    /// buttons and a row per target.
    #[test]
    fn target_links_add_delete_locate() {
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let (mut a, i) = app(&mut c);
        pl::run(&mut a, Cmd::ClipAutomate(i, false));
        let made = a.state.session.project.channels.last().unwrap().index;
        let own = a.state.session.project.channels.len() - 1;
        let hits = clip_window(&mut a, &mut c, made);
        for h in [THit::Add, THit::Delete, THit::Locate, THit::Row(0)] {
            assert!(hits.iter().any(|x| x.0 == h), "{h:?} drawn");
        }
        assert!(!hits.iter().any(|x| x.0 == THit::Row(1)), "one target, one row");

        let tweak = |a: &mut App, k: KnobId| {
            crate::params_ui::note(&mut a.state.tweaks, crate::params_ui::Tweak::Host(Action::Knob(k)));
            tweaked(a);
        };
        tweak(&mut a, KnobId::ChanPan(0));
        assert_eq!(targets(&a.state, made).len(), 1, "Add is off: nothing joins");
        hit(&mut a, THit::Add);
        tweak(&mut a, KnobId::ChanPan(0));
        tweak(&mut a, KnobId::ChanVol(own));
        let first = a.state.session.project.channels[0].index;
        assert_eq!(targets(&a.state, made).len(), 2, "the clip's own range knob does not join");
        assert_eq!(targets(&a.state, made)[1], target::channel(first, target::CHANNEL_PAN));
        hit(&mut a, THit::Add);
        tweak(&mut a, KnobId::ChanVol(0));
        assert_eq!(targets(&a.state, made).len(), 2, "+ again stops adding");
        let hits = clip_window(&mut a, &mut c, made);
        assert!(hits.iter().any(|x| x.0 == THit::Row(1)));
        crate::history_ui::undo(&mut a.state);
        assert_eq!(targets(&a.state, made).len(), 1, "an added target is one undo step");
        crate::history_ui::redo(&mut a.state);

        hit(&mut a, THit::Row(0));
        hit(&mut a, THit::Delete);
        assert_eq!(targets(&a.state, made), [target::channel(first, target::CHANNEL_PAN)]);
        a.state.hide(Win::Plugin);
        a.state.session.pending_open = None;
        hit(&mut a, THit::Locate);
        // A sampler's window is its hosted Lemon Sampler, opened once hosted; any other channel's opens at once.
        let opened = if a.state.session.project.channels[0].kind.is_sampler() {
            a.state.session.pending_open == Some(0)
        } else {
            a.state.shown(Win::Plugin) && a.state.plugin_channel == 0
        };
        assert!(opened, "Locate opens the first channel's window");
    }

    /// spec: ui/automation-targets.md §4: the Remote control settings box's Internal controller cycles through the
    /// project's clips; Accept makes the control one of the chosen clip's targets.
    #[test]
    fn link_to_controller_with_an_existing_clip() {
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let (mut a, i) = app(&mut c);
        pl::run(&mut a, Cmd::ClipAutomate(i, false));
        let made = a.state.session.project.channels.last().unwrap().index;
        a.remote_cmd(crate::remote::RCmd::Link(crate::typein::Target::Knob(KnobId::ChanPan(0))));
        assert_eq!(a.state.link_box.as_ref().unwrap().internal, None);
        a.link_box_act(crate::remote::RAct::Internal);
        assert_eq!(a.state.link_box.as_ref().unwrap().internal, Some(made));
        a.invalidate();
        a.paint(&mut c, None);
        assert!(
            a.hits.iter().any(|h| h.action == Action::Remote(crate::remote::RAct::Internal)),
            "the Internal controller box is drawn"
        );
        a.link_box_act(crate::remote::RAct::Accept);
        assert!(a.state.link_box.is_none());
        let first = a.state.session.project.channels[0].index;
        assert_eq!(targets(&a.state, made).last(), Some(&target::channel(first, target::CHANNEL_PAN)));
        assert_eq!(next_internal(&a.state, Some(made)), None, "after the last clip: none again");
    }

    /// Two automation clips over the audio clip (volume, then panning): their channel indexes and rack rows.
    fn two_clips(a: &mut App, i: usize) -> ((u16, usize), (u16, usize)) {
        let last = |a: &App| (a.state.session.project.channels.last().unwrap().index, a.state.session.project.channels.len() - 1);
        pl::run(a, Cmd::ClipAutomate(i, false));
        let one = last(a);
        pl::run(a, Cmd::ClipAutomate(i, true));
        (one, last(a))
    }

    /// spec: ui/automation-targets.md §3: the row chosen in one clip's list is that list's own: another clip's window
    /// shows its rows from the first, and its buttons act on its own row.
    #[test]
    fn target_links_selection_is_each_clips_own() {
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let (mut a, i) = app(&mut c);
        let ((one, _), (two, _)) = two_clips(&mut a, i);
        for t in 1..=5 {
            assert_eq!(link(&mut a.state, one, target::mixer(t, target::MIXER_VOLUME)), None);
        }
        clip_window(&mut a, &mut c, one);
        hit(&mut a, THit::Row(5));
        let hits = clip_window(&mut a, &mut c, two);
        assert!(hits.iter().any(|x| x.0 == THit::Row(0)), "the other clip's one target is drawn");
        a.state.session.pending_open = None;
        assert_eq!(hit(&mut a, THit::Locate), None, "Locate acts on its row");
        clip_window(&mut a, &mut c, two);
        assert_eq!(hit(&mut a, THit::Delete), None);
        assert!(targets(&a.state, two).is_empty(), "Delete takes its row");
        assert_eq!(targets(&a.state, one).len(), 6, "the first clip keeps its targets");
    }

    /// spec: ui/automation-targets.md §3: Locate opens the window that holds the target: a channel's own window (here
    /// another automation clip's), or the mixer on the target's track.
    #[test]
    fn locate_shows_a_channel_window_and_the_mixer() {
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let (mut a, i) = app(&mut c);
        let ((one, _), (two, row_two)) = two_clips(&mut a, i);
        assert_eq!(link(&mut a.state, one, target::channel(two, target::CHANNEL_VOLUME)), None);
        assert_eq!(link(&mut a.state, one, target::mixer(5, target::MIXER_VOLUME)), None);
        clip_window(&mut a, &mut c, one);
        a.state.hide(Win::Mixer);
        hit(&mut a, THit::Row(1));
        assert_eq!(hit(&mut a, THit::Locate), None);
        assert!(!a.state.session.project.channels[row_two].kind.is_sampler());
        assert!(a.state.shown(Win::Plugin) && a.state.plugin_channel == row_two, "the other clip's window");
        a.invalidate();
        a.paint(&mut c, None);
        assert!(a.hits.iter().any(|h| matches!(h.action, Action::Targets(_))), "painted: a clip window");

        clip_window(&mut a, &mut c, one);
        hit(&mut a, THit::Row(2));
        assert_eq!(hit(&mut a, THit::Locate), None);
        assert_eq!(a.state.z.last(), Some(&Win::Mixer), "the mixer in front");
        assert_eq!(a.state.selected_mixer, 5);
        a.invalidate();
        a.paint(&mut c, None);
        assert!(a.hits.iter().any(|h| h.action == Action::Mix(crate::mixer::MHit::Strip(5))), "painted: the mixer");
    }

    /// spec: ui/automation-targets.md §3: with + on, a control pressed with the mouse joins the clip.
    #[test]
    fn target_links_add_takes_a_pressed_control() {
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let (mut a, i) = app(&mut c);
        pl::run(&mut a, Cmd::ClipAutomate(i, false));
        let made = a.state.session.project.channels.last().unwrap().index;
        clip_window(&mut a, &mut c, made);
        hit(&mut a, THit::Add);
        a.state.z = vec![Win::Mixer];
        a.invalidate();
        a.paint(&mut c, None);
        let (t, r) = (1..=16)
            .find_map(|t| {
                let r = a.hits.iter().rev().find(|h| h.action == Action::Fader(t))?.rect;
                (a.top_hit(r.cx(), r.cy())?.action == Action::Fader(t)).then_some((t, r))
            })
            .expect("a fader to press");
        a.mouse_move(r.cx(), r.cy());
        a.mouse_down();
        a.mouse_up();
        assert_eq!(targets(&a.state, made).last(), Some(&target::mixer(t, target::MIXER_VOLUME)));
    }

    /// spec: ui/automation-targets.md §4: Remove conflicts ticked, the control leaves the other clips' targets and its
    /// MIDI link goes; unticked, both stay.
    #[test]
    fn link_to_controller_remove_conflicts() {
        use crate::remote::{RAct, RCmd};
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let (mut a, i) = app(&mut c);
        let ((one, _), (two, _)) = two_clips(&mut a, i);
        let knob = crate::typein::Target::Knob(KnobId::ChanPan(0));
        let pan = target::channel(a.state.session.project.channels[0].index, target::CHANNEL_PAN);
        let link_to = |a: &mut App, clip: u16, conflicts: bool| {
            a.remote_cmd(RCmd::Link(knob));
            for _ in 0..8 {
                if a.state.link_box.as_ref().unwrap().internal == Some(clip) {
                    break;
                }
                a.link_box_act(RAct::Internal);
            }
            a.state.link_box.as_mut().unwrap().conflicts = conflicts;
            a.link_box_act(RAct::Accept);
        };
        link_to(&mut a, one, false);
        link_to(&mut a, two, false);
        assert!(targets(&a.state, one).contains(&pan) && targets(&a.state, two).contains(&pan), "unticked: both");
        // A MIDI controller link of the same control.
        a.remote_cmd(RCmd::Link(knob));
        a.state.link_box.as_mut().unwrap().cc = Some(20);
        a.link_box_act(RAct::Accept);
        assert!(crate::remote::link_of(&a.state.session.settings, knob).is_some());

        let steps = a.state.session.history.at();
        link_to(&mut a, one, true);
        assert!(targets(&a.state, one).contains(&pan));
        assert!(!targets(&a.state, two).contains(&pan), "it leaves the other clip");
        assert!(crate::remote::link_of(&a.state.session.settings, knob).is_none(), "its MIDI link goes");
        assert_eq!(a.state.session.history.at(), steps + 1, "one step");
        crate::history_ui::undo(&mut a.state);
        assert!(targets(&a.state, two).contains(&pan), "undone together");
    }

    /// The Remote control settings box opened at the bottom right stays on the screen with its Internal controller
    /// group.
    #[test]
    fn link_box_with_internal_controller_stays_on_screen() {
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let (mut a, i) = app(&mut c);
        pl::run(&mut a, Cmd::ClipAutomate(i, false));
        a.state.screen = (1920.0, 1080.0);
        a.mouse = (1915.0, 1075.0);
        a.remote_cmd(crate::remote::RCmd::Link(crate::typein::Target::Knob(KnobId::ChanPan(0))));
        let b = a.state.link_box.as_ref().unwrap();
        assert!(crate::views::remote_box::internal_shown(&a.state, b));
        let (w, h) = crate::views::remote_box::SIZE;
        assert!(b.at.1 + h + crate::views::remote_box::INTERNAL_H <= 1080.0, "{:?}", b.at);
        assert!(b.at.0 + w <= 1920.0);
    }

    /// spec: ui/automation-targets.md §2: a double-click on an automation clip opens its window; one click does not.
    #[test]
    fn automation_clip_double_click_opens_its_window() {
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let (mut a, i) = app(&mut c);
        pl::run(&mut a, Cmd::ClipAutomate(i, false));
        let made = a.state.session.project.channels.last().unwrap().index;
        let row = a.state.session.project.channels.len() - 1;
        a.state.z = vec![Win::Playlist];
        a.state.session.pending_open = None;
        a.invalidate();
        a.paint(&mut c, None);
        let g = pl::PGeom::new(&a.state, a.pl_grid().expect("playlist grid"));
        let clip =
            *pl::clips(&a.state.session).iter().find(|c| c.source == os_model::ClipSource::Channel(made)).unwrap();
        let (x, y) = (g.x_of((clip.position + clip.length / 2) as f32), g.y_of(clip.track) + pl::TITLE_H + 3.0);
        let tap = |a: &mut App, c: &mut Canvas| {
            a.mouse_move(x, y);
            a.mouse_down();
            a.mouse_up();
            a.paint(c, None);
        };
        let open = |a: &App| a.state.shown(Win::Plugin) && a.state.plugin_channel == row;
        tap(&mut a, &mut c);
        assert!(!open(&a), "one click only");
        tap(&mut a, &mut c);
        assert!(open(&a), "a double click opens the clip's window");
    }
}
