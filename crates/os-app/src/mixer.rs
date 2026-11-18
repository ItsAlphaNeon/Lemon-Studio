// Copyright (C) 2026 ItsAlphaNeon
// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Mixer behaviour: track selection, mute / solo, routing and send levels, the track / options / layout menus,
//! adding / moving / deleting tracks, the effect slots that host plugins, and the tracks' audio device inputs and
//! outputs (the input / output combos, the record menu, arming).
//!
// spec: ui/mixer.md (every interaction), flp/events/mixer.md (what is stored), audio/gain-laws.md (fader law),
// audio/track-io.md (audio inputs and outputs)

#![allow(deprecated)] // writes the project directly (`crate::locked`; control-plane step 14 removes this)
#[cfg(test)]
use std::collections::HashMap;
pub use crate::edit::mixer::*;

use os_model::Rgb;

use crate::app::{App, State};
use crate::control::command::Region;

/// Undo step names of the mixer's edits.
const ROUTING: &str = "Mixer routing";
const INSERT: &str = "Insert mixer track";
const SLOT: &str = "Effect slot";

use crate::control::command::{color_text, Command};

/// The mixer tracks and channels have their ids, for commands (a track just added gets its own on the next tick). Only then: a
/// settle gives ids to everything new, and something about to go would take one.
pub fn track_ids(s: &mut State) {
    let p = &s.session.project;
    if p.mixer.iter().any(|m| m.id.is_none()) || p.channels.iter().any(|c| c.id.is_none()) {
        crate::control_ui::settle_ids(s);
    }
}

/// Track `t`'s id, for a command.
fn id_of(s: &State, t: u16) -> Option<u32> {
    track(&s.session, t).map(|m| m.id.0)
}

/// `Command::SetTrack` for track `t` with the settings given (`set_track!(s, t, color: c)`); `None` without the track.
macro_rules! set_track {
    ($s:expr, $t:expr, $($f:ident: $v:expr),+ $(,)?) => {{
        // The values first: the fields' names may be the caller's variables.
        let values = ($(Some($v),)+);
        id_of($s, $t).map(move |id| {
            let mut c = Command::set_track(id);
            if let Command::SetTrack { $($f,)+ .. } = &mut c {
                ($(*$f,)+) = values;
            }
            c
        })
    }};
}

/// The interface's edit made of `commands`: one undo step named `name` (none when nothing changed); the error as the
/// message when they can't be applied.
fn apply(s: &mut State, name: &'static str, commands: Vec<Command>) -> Option<String> {
    crate::control_ui::command(s, name, None, commands).err()
}

/// Changes that make no undo step (a mute light, a solo).
fn apply_quietly(s: &mut State, commands: Vec<Command>) {
    let _ = crate::control_ui::run(s, commands);
}

/// The commands that route track `from` to `to` only: its other routes and sends go.
fn unroute_all(s: &State, from: u16, keep: Option<u16>) -> Vec<Command> {
    let Some(m) = track(&s.session, from) else { return Vec::new() };
    let mut dests: Vec<u16> = m.routes.iter().copied().chain(m.send_levels.iter().map(|l| l.0)).filter(|&d| Some(d) != keep).collect();
    dests.sort_unstable();
    dests.dedup();
    dests.into_iter().filter_map(|d| Some(Command::SetSend { from: id_of(s, from)?, to: id_of(s, d)?, level: None })).collect()
}
use crate::knobs::KnobId;
use crate::playlist::{Cmd, EditTarget, Mark, PItem, Popup};

/// Mixer controls (the view registers them as `Action::Mix`). Controls that are not available yet (title-bar view
/// buttons, input pin, latency clocks, track equalizer) are greyed out with `Ui::off` instead.
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
    /// The title bar's View waveforms and Extra volume / stereo properties buttons (Options ▸ View ▸).
    Waveforms,
    Extra,
    /// Effect slot `n` of the current track: name, ▸ arrow, LED.
    SlotName(u8),
    SlotArrow(u8),
    SlotLed(u8),
    /// The level scale column.
    Scale,
    /// The current track's equaliser graph: a press grabs the band nearest the cursor's frequency.
    EqGraph,
    /// The current track's record-source icon, input combo and output combo (spec: audio/track-io.md §1.1).
    RecSource,
    Input,
    Output,
    /// A strip's disk-record button: arms / disarms the track.
    Arm(u16),
    /// A strip's track latency clock (selects; right click: the latency menu), and the current track's input clock and
    /// track latency combo in the right panel (spec: audio/track-io.md §2.6).
    Latency(u16),
    InLatency,
    OutLatency,
}

/// Menu commands (`Cmd::Mixer`).
#[derive(Debug, Clone, PartialEq)]
pub enum MCmd {
    Rename(u16),
    Color(u16, Option<Rgb>),
    RandomColor(u16),
    /// Gradient color...: the selected tracks, in track order, from the first one's colour to the last one's.
    GradientColor,
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
    /// Select ▸ Channels and playlist tracks routed to this track: as `SelectChannels`, and the playlist selects the
    /// first audio or instrument track linked to the tracks (spec: ui/mixer-track-commands.md §3).
    SelectChannelsTracks(u16),
    /// Assign to new Audio track(s) / Unassign from Audio track(s) on the track (the selection when it is in it).
    AssignAudio(u16),
    UnassignAudio(u16),
    SelectDest(u16),
    SelectSources(u16),
    SelectAll,
    /// Trim unused tracks (§4): delete the inserts no channel uses (`non_empty`: also those holding effects).
    Trim { non_empty: bool },
    MoveLeft(u16),
    MoveRight(u16),
    Delete(u16),
    InsertOne(u16),
    /// Insert multiple...: a box for how many tracks to insert in front of this one (`insert_many`).
    InsertMultiple(u16),
    /// File ▸ Render to wave file...: the Export form for the selected track(s)' outputs (`App::open_track_render`).
    RenderWave(u16),
    /// Route the selected tracks to `dst`: only = drop their other routes, side = sidechain (level 0).
    Route { dst: u16, only: bool, side: bool },
    /// Layout menu: Wide (the only layout drawn; the others are greyed out).
    Layout(u8),
    /// Options ▸ Cable physics: open / close the cables' debug panel (Lemon Studio's own).
    CablePhysics,
    /// General settings ▸ Animations: the level every animation reads (`crate::anim`); the cables swing from Polished
    /// up and hang still below. Kept for the next start.
    Animations(crate::anim::Level),
    /// Open audio editor: Tesla on this track (Tesla's own, as FL's Edison).
    AudioEditor(u16),
    /// Open audio editor and record: a Tesla on this track set up as an audio logger.
    AudioEditorRecord(u16),
    /// Remove all (audio editor) instances: asks first, then `RemoveTeslasNow` removes every Tesla.
    RemoveTeslas,
    RemoveTeslasNow,
    /// Effect slots of the current track.
    AddEffect(u8, usize),
    /// An empty container in place of the slot's plugin (the browser's container entry dropped without Shift,
    /// `App::drop_container_on_slot`).
    AddContainer(u8),
    SlotDelete(u8),
    SlotMove(u8, i8),
    SlotRename(u8),
    /// The slot menu's Change color... (`None`: Default) and Random color, on the current track's slot.
    SlotColor(u8, Option<Rgb>),
    SlotRandomColor(u8),
    SlotView(u8),
    /// Sidechain input ▸ (Lemon Studio's own): the input of the current track a hosted plugin's sidechain bus hears,
    /// counted from 1, 0 for none (spec: audio/sidechain.md "Hosted plugins' input buses").
    SlotSidechain(u8, usize),
    /// A track's audio input and output as stored (-1: none), its record source and its monitor mode (spec:
    /// audio/track-io.md §1.6).
    Input(u16, i32),
    Output(u16, i32),
    RecSource(u16, u8),
    Monitor(u16, u8),
    /// Arm selected track(s) (true) / Disarm selected track(s) for disk recording (spec: audio/track-io.md §2.5).
    Arm(u16, bool),
    /// Arm recording / Disarm recording in an audio track's options: that mixer track alone (spec: ui/playlist.md §6c).
    ArmTrack(u16, bool),
    /// Disk recording ▸ Auto-create audio clips (a program setting).
    AutoClips,
    /// A track's input (true) or output latency setting: Reset (`None`), or Set in ms / samples / beats (the value box).
    Latency(u16, bool, Option<crate::typein::LatUnit>),
    /// The track menu's Group entries (spec: ui/mixer.md §6.1 "Group, Layout and CPU entries"): Separator on or off for
    /// the target tracks, Create group... (its name box, then `create_group`), Auto color group, Select ▸ Group.
    Separator(u16),
    CreateGroup(u16),
    AutoColorGroup(u16),
    SelectGroup(u16),
    /// Dock to ▸ Left / Middle / Right (`os_model::DOCK_*`) and Allow threaded processing on or off, for the target
    /// tracks.
    Dock(u16, u8),
    Threaded(u16),
    /// Switch auto-bypass for all plugins (Options, TOOLS and the instrument rack's options menu): every plugin's Auto-
    /// bypass when silent on when one is off, all off when all are on (`switch_auto_bypass`).
    AutoBypassAll,
    /// Options ▸ Plugin delay compensation ▸ Automatic (`Project::pdc_off`; spec: audio/delay-compensation.md
    /// "Automatic").
    PdcAuto,
    /// The slot menu's Help: the slot plugin's help page (`slot_help`).
    SlotHelp(u8),
    /// Wrap in container (false) and Wrap all in container (true) on the current track's slot (`App::wrap_slot`).
    Wrap(u8, bool),
    /// The container list's entries on a member of the container in the current track's slot (`App::edit_member`), its
    /// Add effect ▸ (`App::add_member`: the effect's index in `Plugins::effects`), a member window's Replace ▸
    /// (`App::replace_member`: the member's place, the effect's index), and the list's window. A channel's container
    /// takes them as slot `container::CHANNEL` of its row in the track's place (`Cmd::SlotWin`), and instruments too:
    /// `MemberAddInst` and `MemberReplaceInst` with the instrument's index in `Plugins::available`.
    Member(u8, usize, crate::container::Op),
    MemberAdd(u8, usize),
    MemberAddInst(u8, usize),
    MemberReplace(u8, usize, usize),
    MemberReplaceInst(u8, usize, usize),
    ContainerList(u8),
    /// A member window's Rename, color and icon..., Change color... ▸ and Random color on member `.1` (its place) of the
    /// container in the current track's slot `.0`, as `Member` addresses it: the member's own name and colour, one undo
    /// step each (`crate::container_ui::edit_own`).
    MemberRename(u8, usize),
    MemberColor(u8, usize, Option<Rgb>),
    MemberRandomColor(u8, usize),
    /// The slot menu's Scaling ▸: the hosted plugin's window at this percent, `None` = Main (`Plugins::editor_scales`).
    SlotScale(u8, Option<u16>),
    /// Options ▸ View ▸ Waveforms and Extra properties (`MixerUi::waveforms`, `MixerUi::extra`).
    ViewWaveforms,
    ViewExtra,
    /// File ▸ Save mixer track state, File ▸ Open mixer track state ▸ and Insert from state ▸ (Lemon Studio's design,
    /// `crate::mixer_state`): save track `n`'s state under its name; put a saved one on track `n` (its name and colour
    /// stay); insert a new track in front of `n` from one (with the state's name and colour).
    SaveState(u16),
    OpenState(u16, std::path::PathBuf),
    InsertState(u16, std::path::PathBuf),
}

pub const LAYOUTS: [&str; 8] = ["Compact", "Compact 2", "Compact alt", "Wide", "Wide 2", "Wide 3", "Wide alt", "Extra large"];
/// Index of "Wide" (the only layout drawn).
pub const WIDE: u8 = 3;

/// Mixer state that is not project data.
pub struct MixerUi {
    /// Selected tracks besides the current one (Ctrl+click, §2).
    pub sel: Vec<u16>,
    pub layout: u8,
    /// Meter state per track (`meters::Meter`), fed by `feed_meters`.
    pub meters: Vec<crate::meters::Meter>,
    /// Meter ballistics and look.
    pub meter_cfg: crate::meters::Settings,
    /// When the meters last advanced.
    pub meter_at: Option<std::time::Instant>,
    /// The last solo: the tracks it kept on and the states (track, enabled) from before the first solo, put back by
    /// unsolo when "Restore previous state after solo" is on.
    pub solo: Option<(Vec<u16>, Vec<(u16, bool)>)>,
    /// The track and effect slot the wheel last moved, and where the mouse was then (`slot_wheel`).
    pub wheel_grab: Option<(u16, u8, (f32, f32))>,
    /// View ▸ Waveforms (Lemon Studio's design, saved in `.lemon` files, `lemon_state`): each strip's meter shows its recent peaks as a waveform
    /// scrolling up, newest at the bottom; `waves` holds them per track (L, R, linear), newest last.
    pub waveforms: bool,
    pub waves: Vec<std::collections::VecDeque<[f32; 2]>>,
    /// View ▸ Extra properties (Lemon Studio's design, saved in `.lemon` files, on by default): the strips show the reverse
    /// polarity and swap buttons and the stereo separation knob.
    pub extra: bool,
    /// What drags on the selected faders keep while the selection lasts.
    pub faders: FaderGroup,
    /// The track Shift+wheel last moved, and where the mouse was then (`wheel_move`).
    pub move_grab: Option<(u16, (f32, f32))>,
}

/// What drags on selected faders keep while the selection lasts (spec: ui/mixer.md §4): each selected track's level at
/// the press that started them, which is what every fader moves from, and the pressed fader's last move. So a fader
/// stopped at the top stays there until the others have come down, and a group taken to -INF comes back as it was.
#[derive(Default)]
pub struct FaderGroup {
    levels: Vec<(u16, f64)>,
    pub moved: Option<(u16, f64)>,
}

impl FaderGroup {
    /// Whether these are the levels of the tracks `sel` (sorted), with every fader where the last drag left it.
    fn holds(&self, s: &crate::session::Session, sel: &[u16]) -> bool {
        let level_of = |t: u16| self.levels.iter().find(|l| l.0 == t).map_or(0.0, |l| l.1);
        self.levels.iter().map(|l| l.0).eq(sel.iter().copied())
            && self.levels.iter().all(|&(t, level)| {
                let left = match self.moved {
                    Some((p, v)) if p == t => v,
                    Some((p, v)) => group_fader(level, level_of(p), v),
                    None => level,
                };
                volume(s, t) == left
            })
    }
}

/// How many meter readings a waveform keeps: more than the tallest meter has pixels.
pub const WAVE_LEN: usize = 600;

impl Default for MixerUi {
    fn default() -> Self {
        MixerUi { sel: Vec::new(), layout: WIDE, meters: Vec::new(),
            meter_cfg: Default::default(), meter_at: None, solo: None, wheel_grab: None, waveforms: false, waves: Vec::new(), extra: true,
            faders: FaderGroup::default(), move_grab: None }
    }
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

/// Whether track `n` is one of several selected: a press on its fader keeps the selection (spec: ui/mixer.md §2, §4).
pub fn in_group(s: &State, n: u16) -> bool {
    is_selected(s, n) && selected(s).len() > 1
}

/// The selected tracks' levels for a press on a selected fader: the ones kept since the selection was made, or the
/// faders as they are when something else has moved one since the last drag (the wheel, Ctrl+Z, automation).
pub fn group_levels(s: &mut State) -> Vec<(u16, f64)> {
    let mut sel = selected(s);
    sel.sort_unstable();
    if !s.mixer.faders.holds(&s.session, &sel) {
        let levels = sel.iter().map(|&t| (t, volume(&s.session, t))).collect();
        s.mixer.faders = FaderGroup { levels, moved: None };
    }
    s.mixer.faders.levels.clone()
}

/// Where a selected fader goes in a group drag: from `other`, its kept level, by the same number of dB as the pressed
/// fader is from its own kept level `level` at `v`, held to the fader's range. At `v` = `level` it is exactly `other`. A
/// fader at -INF stays there. Moved up from a `level` of -INF, which is no number of dB, the others go to the top, as FL's
/// do (spec: ui/mixer.md §4).
pub fn group_fader(other: f64, level: f64, v: f64) -> f64 {
    use os_engine::laws::{fader_gain, fader_of_gain};
    if v == level || other <= 0.0 {
        return other;
    }
    if level <= 0.0 {
        return 1.0;
    }
    fader_of_gain(fader_gain(other) * fader_gain(v) / fader_gain(level))
}


/// Every slot's name typed in Lemon Studio, by (track, slot).
#[cfg(test)]
pub fn slot_names_of(p: &os_model::Project) -> HashMap<(u16, u8), String> {
    p.mixer.iter().flat_map(|m| m.slot_names.iter().enumerate().filter_map(move |(k, n)| Some(((m.index, k as u8), n.clone()?)))).collect()
}

/// Every slot's colour, by (track, slot).
#[cfg(test)]
pub fn slot_colors_of(p: &os_model::Project) -> HashMap<(u16, u8), Rgb> {
    p.mixer.iter().flat_map(|m| m.slot_colors.iter().enumerate().filter_map(move |(k, c)| Some(((m.index, k as u8), (*c)?)))).collect()
}

/// Tesla's "For" index of 5 minutes (Edison's menu: Forever, 1, 3, 5 minutes, ...), the audio logger's length.
const AUDIO_LOG_5_MINUTES: u8 = 3;

/// The question before Remove all (audio editor) instances.
fn remove_question(n: usize) -> String {
    match n {
        1 => "Remove 1 audio editor? Its audio is lost.".into(),
        n => format!("Remove {n} audio editors? Their audio is lost."),
    }
}

/// "Volume: 100%  0.0dB  1.00" (§4); `label` = "Volume" or "To Bus 3".
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

/// The help page of the plugin in slot `slot` of the current track (spec: ui/plugin-window.md "Help", Lemon Studio's
/// design): a hosted plugin's own page (`os_host::Plugin::help_url`); for Lemon Studio's own plugins, the effects the
/// engine plays itself and a container, its page in the wiki. `None`: an empty slot, or a hosted plugin that names no
/// page.
pub fn slot_help(app: &App, slot: u8) -> Option<String> {
    let s = &app.state;
    let t = s.selected_mixer;
    let p = track(&s.session, t)?.slots.get(usize::from(slot))?.as_ref()?;
    match app.state.session.plugins.hosted.get(&crate::plugins::fx_key(t, slot)) {
        Some(h) => {
            let p = h.plugin.lock().ok()?;
            let own = crate::plugins::builtin_effects().iter().chain(&crate::plugins::builtin()).any(|a| a.desc.id == p.desc().id);
            p.help_url().or_else(|| own.then(|| crate::help_links::plugin_page(&p.desc().name)))
        }
        None => {
            let own = p.name == crate::container::CONTAINER || os_engine::effects::from_slot(&p.name, &p.state, 44100).is_some();
            own.then(|| crate::help_links::plugin_page(crate::plugins::slot_label(&p.name)))
        }
    }
}

/// Scaling ▸ choices (Lemon Studio's design): Main, then these percents.
pub const SCALES: [u16; 5] = [100, 125, 150, 175, 200];

/// The slot menu's Scaling ▸ (Lemon Studio's design, saved in `.lemon` files): Main (the program's scale) or a fixed scale for
/// the hosted plugin's window and editor, radio items; greyed for an effect the engine plays itself (no editor).
fn scaling_menu(app: &App, slot: u8) -> PItem {
    let key = crate::plugins::fx_key(app.state.selected_mixer, slot);
    let now = app.state.session.plugins.editor_scales.get(&key).map(|k| (k * 100.0).round() as u16);
    let mut items = vec![PItem::new("Main", Cmd::Mixer(MCmd::SlotScale(slot, None))).mark(Mark::Radio(now.is_none()))];
    items.extend(SCALES.iter().map(|&p| PItem::new(format!("{p}%"), Cmd::Mixer(MCmd::SlotScale(slot, Some(p)))).mark(Mark::Radio(now == Some(p)))));
    let it = PItem::sub("Scaling", items);
    if app.state.session.plugins.hosted.contains_key(&key) { it } else { it.off() }
}

/// View ▸ Waveforms on or off; the waveforms start empty.
fn view_waveforms(s: &mut State) {
    s.mixer.waveforms = !s.mixer.waveforms;
    s.mixer.waves.clear();
}

/// The label of Switch auto-bypass for all plugins (mixer Options, TOOLS, the instrument rack's options menu).
pub const AUTO_BYPASS_ALL_LABEL: &str = "Switch auto-bypass for all plugins";

/// Selects `n` as a click with these modifiers does (§2): plain = only it, Ctrl = add it.
pub fn select(s: &mut State, n: u16, ctrl: bool) {
    if !(ctrl && is_selected(s, n)) {
        // A selection made again, even of the same tracks, starts from the faders as they are (spec: ui/mixer.md §4).
        s.mixer.faders = FaderGroup::default();
    }
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
            if shift && !ctrl {
                start_rename(s, n, at);
            }
            None
        }
        MHit::Led(n) => {
            select(s, n, ctrl);
            track_ids(s);
            let on = !track(&s.session, n)?.enabled;
            apply_quietly(s, vec![set_track!(s, n, enabled: on)?]);
            None
        }
        MHit::Polarity(n) => {
            select(s, n, ctrl);
            track_ids(s);
            let on = !track(&s.session, n)?.polarity;
            apply(s, "Mixer track polarity", vec![set_track!(s, n, polarity: on)?])
        }
        MHit::Swap(n) => {
            select(s, n, ctrl);
            track_ids(s);
            let on = !track(&s.session, n)?.swap;
            apply(s, "Mixer track swap", vec![set_track!(s, n, swap: on)?])
        }
        MHit::Route(dst) => route(s, dst, ctrl, shift),
        MHit::Unroute(dst) => {
            let cur = s.selected_mixer;
            track_ids(s);
            let c = Command::SetSend { from: id_of(s, cur)?, to: id_of(s, dst)?, level: None };
            apply(s, ROUTING, vec![c]).or_else(|| Some(format!("{} no longer sends to {}", name(&s.session, cur), name(&s.session, dst))))
        }
        MHit::Add => Some(insert_tracks(s, None, 1).map_or_else(|e| e, |n| format!("Added {}", name(&s.session, n)))),
        MHit::AddAfter => Some(insert_tracks(s, Some(s.selected_mixer + 1), 1).map_or_else(|e| e, |n| format!("Added {}", name(&s.session, n)))),
        MHit::Options => {
            s.playlist.popup = Some(options_menu(s, at.0, at.1 + 10.0));
            None
        }
        MHit::Layout => {
            s.playlist.popup = Some(layout_menu(s, at.0, at.1 + 10.0));
            None
        }
        MHit::Waveforms => {
            view_waveforms(s);
            None
        }
        MHit::Extra => {
            s.mixer.extra = !s.mixer.extra;
            None
        }
        MHit::SlotName(i) => {
            let t = s.selected_mixer;
            if slot_name(&s.session, t, i).is_some() {
                s.session.fx_requests.push(FxReq::Toggle { track: t, slot: i });
            } else {
                effect_palette(app, i, at);
            }
            None
        }
        MHit::SlotArrow(i) => {
            let t = s.selected_mixer;
            if slot_name(&s.session, t, i).is_some() {
                app.state.playlist.popup = Some(slot_menu(app, i, at.0, at.1));
            } else {
                effect_palette(app, i, at);
            }
            None
        }
        MHit::SlotLed(i) => {
            let t = s.selected_mixer;
            track_ids(s);
            let on = !track(&s.session, t)?.slot_on[usize::from(i)];
            apply(s, SLOT, vec![Command::SetSlot { track: id_of(s, t)?, slot: i, enabled: Some(on), mix: None, name: None, color: None, sidechain: None, auto_bypass: None, threaded: None }])
        }
        MHit::Input => {
            s.playlist.popup = input_menu(s, at.0, at.1 + 10.0);
            None
        }
        MHit::Output => {
            s.playlist.popup = Some(output_menu(s, at.0, at.1 + 10.0));
            None
        }
        MHit::RecSource => {
            s.playlist.popup = Some(record_menu(s, at.0, at.1 + 10.0));
            None
        }
        MHit::Arm(n) => {
            let on = !track(&s.session, n)?.armed;
            app.arm_tracks(&[n], on)
        }
        MHit::Latency(n) => {
            select(s, n, ctrl);
            None
        }
        MHit::InLatency => {
            s.playlist.popup = Some(latency_menu(&s.session, s.selected_mixer, true, at.0, at.1 + 10.0));
            None
        }
        MHit::OutLatency => {
            s.playlist.popup = Some(latency_menu(&s.session, s.selected_mixer, false, at.0, at.1 + 10.0));
            None
        }
        MHit::Scale | MHit::EqGraph => None,
    }
}

// ---------------------------------------------------------------------------------------------------------
// Audio inputs and outputs (spec: audio/track-io.md)

/// The record sources (event 165) with their menu names and the label under the record-source icon (§2.3).
pub const REC_SOURCES: [(&str, &str); 6] = [
    ("External input only", "EXT"),
    ("External and mixer input", "MIX"),
    ("Post effects", "FX"),
    ("Post EQ", "EQ"),
    ("Post level and panning", "LVL"),
    ("Post track", "TRK"),
];

/// The monitor modes (event 166) with their menu names.
pub const MONITOR_MODES: [(u8, &str); 3] = [(os_model::MONITOR_OFF, "Off"), (os_model::MONITOR_ARMED, "When armed"), (os_model::MONITOR_ON, "On")];

/// The input menu of the current track (§1.2): `<driver> - stereo` and its pairs, `<driver> - mono` and the inputs,
/// then "(none)". `None` when the device has no inputs.
fn input_menu(s: &State, x: f32, y: f32) -> Option<Popup> {
    let items = input_items(&s.session, s.selected_mixer, false)?;
    Some(Popup { x, y, items, sub: None, scroll: [0; 2] })
}

/// The output menu of the current track (§1.2): an ASIO driver's `<driver> - stereo` and its pairs (a Windows device:
/// its name), then "(none)".
fn output_menu(s: &State, x: f32, y: f32) -> Popup {
    let d = crate::device::last();
    let t = s.selected_mixer;
    let mut items = Vec::new();
    if let Some(d) = d.as_ref().filter(|d| d.host == crate::device::ASIO && d.channels > 1) {
        items.push(PItem::head(format!("{} - stereo", d.name)));
    }
    items.extend(output_choices(d.as_ref()).into_iter().map(|(v, name)| PItem::new(name, Cmd::Mixer(MCmd::Output(t, v)))));
    items.extend([PItem::sep(), PItem::new("(none)", Cmd::Mixer(MCmd::Output(t, -1)))]);
    Popup { x, y, items, sub: None, scroll: [0; 2] }
}

/// The record-source icon's menu (§1.3): the record source, then the monitor mode, each choice marked.
fn record_menu(s: &State, x: f32, y: f32) -> Popup {
    let t = s.selected_mixer;
    let (source, monitor) = track(&s.session, t).map_or((os_model::REC_POST_EQ, os_model::MONITOR_ARMED), |m| (m.rec_source, m.monitor));
    let mut items = vec![PItem::head("Record")];
    for (k, (label, _)) in REC_SOURCES.iter().enumerate() {
        items.push(PItem::new(*label, Cmd::Mixer(MCmd::RecSource(t, k as u8))).mark(Mark::Radio(usize::from(source) == k)));
    }
    items.push(PItem::head("Monitor external input"));
    for (v, label) in MONITOR_MODES {
        items.push(PItem::new(label, Cmd::Mixer(MCmd::Monitor(t, v))).mark(Mark::Radio(monitor == v)));
    }
    Popup { x, y, items, sub: None, scroll: [0; 2] }
}

/// A latency setting as the right panel's combo and the hints show it: "(none)", "12 ms", "-3.5 ms".
pub fn latency_text(ms: f32) -> String {
    if ms == 0.0 {
        return "(none)".into();
    }
    let t = format!("{ms:.2}");
    format!("{} ms", t.trim_end_matches('0').trim_end_matches('.'))
}

/// Sets track `t`'s input (`input`) or output latency setting, in ms: one undo step, "Track latency"; playback takes it
/// at once (delay compensation works it out again).
pub fn set_latency(s: &mut State, t: u16, input: bool, ms: f32) {
    track_ids(s);
    if track(&s.session, t).is_none_or(|m| (if input { m.latency_in } else { m.latency_out }) == ms) {
        return;
    }
    let c = if input { set_track!(s, t, latency_in: f64::from(ms)) } else { set_track!(s, t, latency_out: f64::from(ms)) };
    if let Some(e) = c.and_then(|c| apply(s, LATENCY, vec![c])) {
        s.message = Some(e);
        return;
    }
    s.message = Some(format!("{} - {} latency: {}", name(&s.session, t), if input { "Input" } else { "Track" }, latency_text(ms)));
}

/// The undo step of a latency setting.
pub const LATENCY: &str = "Track latency";

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
            s.playlist.popup = Some(Popup { x: at.0, y: at.1, items, sub: None, scroll: [0; 2] });
        }
        // The slot's plugin menu, like a left click (spec: ui/plugin-menu.md §1).
        MHit::SlotArrow(i) => {
            let t = s.selected_mixer;
            if slot_name(&s.session, t, i).is_some() {
                app.state.playlist.popup = Some(slot_menu(app, i, at.0, at.1));
            } else {
                effect_palette(app, i, at);
            }
        }
        MHit::SlotName(i) => {
            let t = s.selected_mixer;
            if slot_name(&s.session, t, i).is_some() {
                s.session.fx_requests.push(FxReq::Show { track: t, slot: i });
            } else {
                effect_palette(app, i, at);
            }
        }
        MHit::SlotLed(i) => {
            // Solo the slot: the track's other slots are bypassed (§7).
            let t = s.selected_mixer;
            track_ids(s);
            if let (Some(m), Some(id)) = (track(&s.session, t), id_of(s, t)) {
                let only = (0..10).all(|k| m.slot_on[k] == (k == usize::from(i)));
                let c = (0..10u8).map(|k| Command::SetSlot { track: id, slot: k, enabled: Some(only || k == i), mix: None, name: None, color: None, sidechain: None, auto_bypass: None, threaded: None }).collect();
                if let Some(e) = apply(s, SLOT, c) {
                    s.message = Some(e);
                }
            }
        }
        MHit::Polarity(n) | MHit::Swap(n) => select(s, n, false),
        MHit::Latency(n) => {
            select(s, n, false);
            s.playlist.popup = Some(latency_menu(&s.session, n, false, at.0, at.1));
        }
        MHit::InLatency => s.playlist.popup = Some(latency_menu(&s.session, s.selected_mixer, true, at.0, at.1)),
        MHit::OutLatency => s.playlist.popup = Some(latency_menu(&s.session, s.selected_mixer, false, at.0, at.1)),
        _ => return false,
    }
    true
}

/// Middle click: rename a filled slot (§7).
pub fn middle(app: &mut App, h: MHit) -> bool {
    if let MHit::SlotName(i) = h {
        let t = app.state.selected_mixer;
        if slot_name(&app.state.session, t, i).is_some() {
            let at = app.mouse;
            crate::playlist::start_edit(&mut app.state, EditTarget::SlotName(t, i), at);
            return true;
        }
    }
    false
}

/// The wheel over effect slot `slot` of the current track (§7): what the slot holds, an effect or nothing, trades places
/// with its neighbour, one slot per notch, up for `notches` > 0. While the mouse stays still, more notches move the
/// same slot, which has left the cursor.
pub fn slot_wheel(app: &mut App, slot: u8, notches: i32) {
    let at = app.mouse;
    let t = app.state.selected_mixer;
    let mut slot = match app.state.mixer.wheel_grab {
        Some((gt, g, m)) if gt == t && m == at => g,
        _ => slot,
    };
    let d: i8 = if notches > 0 { -1 } else { 1 };
    for _ in 0..notches.unsigned_abs() {
        let to = i32::from(slot) + i32::from(d);
        if !(0..10).contains(&to) {
            break;
        }
        run(app, MCmd::SlotMove(slot, d));
        slot = to as u8;
    }
    app.state.mixer.wheel_grab = Some((t, slot, at));
}

fn start_rename(s: &mut State, n: u16, at: (f32, f32)) {
    crate::playlist::start_edit(s, EditTarget::MixerName(n), at);
}

/// Create group..., Enter in its name box: names track `n` (an empty box keeps its name) and turns Separator on
/// for the first target track and for the track after the last one, as one undo step (spec: ui/mixer.md §6.1).
pub fn create_group(s: &mut State, n: u16, text: &str) {
    track_ids(s);
    let tg: Vec<u16> = targets(s, n).into_iter().filter(|&t| t > 0).collect();
    let (Some(&a), Some(&b)) = (tg.iter().min(), tg.iter().max()) else { return };
    let last = inserts(&s.session);
    let def = format!("Bus {n}");
    let name = text.trim().to_string();
    let mut c = Vec::new();
    if !name.is_empty() {
        c.extend(set_track!(s, n, name: if name == def { String::new() } else { name }));
    }
    c.extend([a, b + 1].into_iter().filter(|&t| t <= last).filter_map(|t| set_track!(s, t, separator: true)));
    if let Some(e) = apply(s, "Create group", c) {
        s.message = Some(e);
    }
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
    let mut keep = reach(&s.session, n, true);
    if alt {
        keep.extend(reach(&s.session, n, false));
    }
    keep.push(0);
    let soloed = audible_set(&s.session, &keep);
    // A solo still in effect (the last solo's set, untouched since) keeps the states saved when it started.
    let in_solo = s.mixer.solo.as_ref().is_some_and(|(k, _)| audible_set(&s.session, k));
    let before = match s.mixer.solo.take() {
        Some((_, b)) if in_solo => b,
        _ => s.session.project.mixer.iter().map(|m| (m.index, m.enabled)).collect(),
    };
    let back = soloed && in_solo && crate::prefs::check(&crate::prefs::RESTORE_AFTER_SOLO);
    for m in s.session.project.get_mut().mixer.iter_mut() {
        m.enabled = if back {
            before.iter().find(|b| b.0 == m.index).is_none_or(|b| b.1)
        } else {
            soloed || keep.contains(&m.index)
        };
    }
    if !soloed {
        s.mixer.solo = Some((keep, before));
    }
    changed(&mut s.session);
}

/// The rack's selected channels (the piano roll's channel when none is selected), in rack order.
fn selected_channels(s: &State) -> Vec<usize> {
    let n = s.session.project.channels.len();
    let rows: Vec<usize> = s.rack.sel.iter().copied().filter(|&r| r < n).collect();
    if rows.is_empty() && s.selected_channel < n { vec![s.selected_channel] } else { rows }
}

/// Channel routing ▸ (spec: ui/mixer-track-commands.md §2): the selected channels go to `dst` (named after the first),
/// or one per track from `dst` on (`spread`; channels past the last insert stay). `dst` becomes the current track.
pub fn route_channels(s: &mut State, dst: u16, spread: bool) -> Option<String> {
    let rows = selected_channels(s);
    let first = *rows.first()?;
    let max = inserts(&s.session);
    let plan: Vec<(usize, u16, bool)> = rows
        .iter()
        .enumerate()
        .map(|(k, &r)| (r, if spread { dst + k as u16 } else { dst }, spread || k == 0))
        .take_while(|&(_, t, _)| t <= max)
        .collect();
    let moved = plan.len();
    route_plan(s, ROUTING, plan, 0)?;
    s.mixer.sel.clear();
    s.selected_mixer = dst;
    Some(if moved == 1 {
        format!("Routed {} to {}", crate::edit::rack::channel_name(&s.session, first), name(&s.session, dst))
    } else {
        format!("Routed {moved} channels")
    })
}

/// Route to free mixer track (spec: ui/mixer-track-commands.md §1): a channel alone on its insert keeps it, the others
/// move to the lowest insert no channel uses (a new track when none is free); the track is named after the channel
/// and becomes the current one.
pub fn route_free(s: &mut State, rows: &[usize]) -> Option<String> {
    // Where each channel goes, worked out before: a channel routed takes its track for the next ones.
    let mut on: Vec<u16> = s.session.project.channels.iter().map(|c| c.mixer_track).collect();
    let mut last = inserts(&s.session);
    let mut adds = 0;
    let mut plan = Vec::new();
    for &r in rows {
        let Some(&cur) = on.get(r) else { continue };
        let alone = cur != 0 && cur <= last && !on.iter().enumerate().any(|(i, &c)| i != r && c == cur);
        let t = if alone {
            cur
        } else if let Some(t) = (1..=last).find(|t| !on.contains(t)) {
            t
        } else {
            // A new track after the last one in use.
            last += 1;
            adds += 1;
            last
        };
        on[r] = t;
        plan.push((r, t, true));
    }
    let &(r, t, _) = plan.last()?;
    route_plan(s, crate::rack::MIXER_TRACK, plan, adds)?;
    s.mixer.sel.clear();
    s.selected_mixer = t;
    Some(if rows.len() == 1 {
        format!("Routed {} to {}", crate::edit::rack::channel_name(&s.session, r), name(&s.session, t))
    } else {
        format!("Routed {} channels to free tracks", rows.len())
    })
}

/// Channel rows routed to mixer tracks (`plan`: row, track, whether the track is named after the channel), after `adds`
/// new tracks at the end, as one undo step named `step`. `None` (with the message set) when it can't be applied.
fn route_plan(s: &mut State, step: &'static str, plan: Vec<(usize, u16, bool)>, adds: u16) -> Option<()> {
    track_ids(s);
    let commands = move |s: &State, _: &[serde_json::Value]| -> Vec<Command> {
        let mut c = Vec::new();
        for &(r, t, named) in &plan {
            let (Some(ch), Some(track)) = (s.session.project.channels.get(r), id_of(s, t)) else { continue };
            let mut set = Command::set_channel(ch.id.0);
            if let Command::SetChannel { track: t, .. } = &mut set {
                *t = Some(track);
            }
            c.push(set);
            // Named after the channel, and coloured when it has a colour (`edit::mixer::name_after`).
            if named && t != 0 {
                let nm = crate::edit::rack::channel_name(&s.session, r);
                c.extend(match ch.color {
                    Some(col) => set_track!(s, t, name: nm, color: color_text(Some(col)).unwrap_or_default()),
                    None => set_track!(s, t, name: nm),
                });
            }
        }
        c
    };
    let r = if adds == 0 {
        let c = commands(s, &[]);
        crate::control_ui::command(s, step, None, c)
    } else {
        crate::control_ui::command_then(s, step, vec![Command::AddTrack { at: None }; usize::from(adds)], commands)
    };
    match r {
        Ok(_) => Some(()),
        Err(e) => {
            s.message = Some(e);
            None
        }
    }
}

/// Select ▸ Tracks this one is routed to / Tracks routed to this one (spec: ui/mixer-track-commands.md §3), for every
/// track the command acts on. The first found becomes the current track.
fn select_linked(s: &mut State, n: u16, down: bool) -> Option<String> {
    let tg = targets(s, n);
    let mut found: Vec<u16> = Vec::new();
    for m in &s.session.project.mixer {
        let hit = if down { tg.iter().any(|&t| track(&s.session, t).is_some_and(|x| x.routes.contains(&m.index))) } else { m.routes.iter().any(|r| tg.contains(r)) };
        if hit && !tg.contains(&m.index) {
            found.push(m.index);
        }
    }
    let Some(&first) = found.first() else { return Some("No tracks to select".into()) };
    s.selected_mixer = first;
    s.mixer.sel = found;
    None
}

/// Deletes the tracks in `gone`: the others close up in order, channels and sends follow them, the mixer gets
/// shorter (spec: ui/mixer-track-commands.md §4).
fn remove_tracks(s: &mut State, gone: &[u16]) -> Option<String> {
    if gone.is_empty() {
        return Some("No unused mixer tracks".into());
    }
    track_ids(s);
    let tracks = gone.iter().filter_map(|&t| id_of(s, t)).collect();
    // The numbers the kept tracks get, for the selection.
    let mut k = 0u16;
    let mut new_of = std::collections::HashMap::new();
    for m in &s.session.project.mixer {
        if !gone.contains(&m.index) {
            new_of.insert(m.index, k);
            k += 1;
        }
    }
    if let Some(e) = apply(s, "Delete mixer track", vec![Command::RemoveTracks { tracks }]) {
        return Some(e);
    }
    let m = |t: u16| new_of.get(&t).copied();
    s.selected_mixer = m(s.selected_mixer).unwrap_or(1).min(inserts(&s.session));
    s.mixer.sel = s.mixer.sel.iter().filter_map(|&t| m(t)).collect();
    Some(format!("Removed {} unused mixer track(s)", gone.len()))
}

/// Routes the selected tracks to `dst` (§5): a normal route at unity, a sidechain at level 0, `only` drops their
/// other routes.
pub fn route(s: &mut State, dst: u16, only: bool, side: bool) -> Option<String> {
    track_ids(s);
    let mut msg = None;
    let mut c = Vec::new();
    for src in selected(s) {
        if src == dst || src == 0 {
            continue;
        }
        if makes_loop(&s.session, src, dst) {
            msg = Some(format!("Can't route {} to {}: it would make a loop", name(&s.session, src), name(&s.session, dst)));
            continue;
        }
        if only {
            c.extend(unroute_all(s, src, Some(dst)));
        }
        c.push(Command::SetSend { from: id_of(s, src)?, to: id_of(s, dst)?, level: Some(if side { 0.0 } else { 0.8 }) });
    }
    apply(s, ROUTING, c).or(msg)
}

/// Appends a track (`at` = None) or inserts one before `at`, shifting the later ones; selects it. Returns its number.
pub fn add_track(s: &mut State, at: Option<u16>) -> u16 {
    crate::follow::around(s, |s| add_track_in(&mut s.session, at))
}

/// The most tracks one Insert multiple... adds (Lemon Studio's choice).
pub const INSERT_MAX: u16 = 100;

/// Insert multiple... (the track menu): `text` read as a count (1 to `INSERT_MAX`) of new tracks inserted in front of
/// track `n`, as Insert one inserts one, in one undo step; the first new one becomes current. Spec: the original's
/// manual says its insert entry asks how many tracks to insert in front of the selected one; the box is Lemon
/// Studio's (ui/mixer-track-commands.md §6).
pub fn insert_many(s: &mut State, n: u16, text: &str) -> Option<String> {
    let Ok(k) = text.trim().parse::<u16>() else { return Some(format!("Insert multiple: \"{}\" is not a number", text.trim())) };
    let k = k.clamp(1, INSERT_MAX);
    if let Err(e) = insert_tracks(s, Some(n), k) {
        return Some(e);
    }
    Some(format!("Inserted {k} mixer track{}", if k == 1 { "" } else { "s" }))
}

/// Swaps track `n` with its neighbour (`d` = -1 left, +1 right), §6.1 Move left / right.
fn move_track(s: &mut State, n: u16, d: i32) -> Option<String> {
    let o = i32::from(n) + d;
    if n == 0 || o < 1 || o > i32::from(inserts(&s.session)) {
        return None;
    }
    track_ids(s);
    apply(s, "Move mixer track", vec![Command::MoveTrack { track: id_of(s, n)?, to: o as u16 }])
}

/// `k` new tracks inserted before track `at` (else after the last one in use), as one undo step. Returns the last new
/// track's number.
fn insert_tracks(s: &mut State, at: Option<u16>, k: u16) -> Result<u16, String> {
    let at = at.map(|n| n.clamp(1, inserts(&s.session) + 1));
    let r = crate::control_ui::command(s, INSERT, None, vec![Command::AddTrack { at }; usize::from(k)])?;
    Ok(number_of(s, r.last()))
}

/// The number of the track a command's result names (`{"track": id}`); the master when none.
fn number_of(s: &State, result: Option<&serde_json::Value>) -> u16 {
    let id = result.and_then(|v| v["track"].as_u64());
    s.session.project.mixer.iter().find(|m| Some(u64::from(m.id.0)) == id).map_or(0, |m| m.index)
}

/// Open mixer track state: the saved state in `path` onto track `n`, its old effects going; one undo step.
fn load_state(s: &mut State, n: u16, path: &std::path::Path) -> Option<String> {
    if crate::mixer_state::load(path).is_none() {
        return Some(unreadable(path));
    }
    track_ids(s);
    let c = Command::LoadTrackState { track: id_of(s, n)?, path: path.to_string_lossy().into_owned(), named: false };
    apply(s, "Open mixer track state", vec![c]).or_else(|| Some(state_taken(s, n, path)))
}

/// Insert mixer track state: a new track before track `n` takes the saved state in `path` with its name and colour;
/// one undo step.
fn insert_state(s: &mut State, n: u16, path: &std::path::Path) -> Option<String> {
    if crate::mixer_state::load(path).is_none() {
        return Some(unreadable(path));
    }
    let p = path.to_string_lossy().into_owned();
    let then = |_: &State, r: &[serde_json::Value]| -> Vec<Command> {
        let Some(track) = r.first().and_then(|v| v["track"].as_u64()) else { return Vec::new() };
        vec![Command::LoadTrackState { track: track as u32, path: p, named: true }]
    };
    match crate::control_ui::command_then(s, INSERT, vec![Command::AddTrack { at: Some(n.max(1)) }], then) {
        Ok(r) => {
            let t = number_of(s, r.first());
            Some(state_taken(s, t, path))
        }
        Err(e) => Some(e),
    }
}

fn unreadable(path: &std::path::Path) -> String {
    format!("Couldn't read the mixer track state {}", path.display())
}

/// What a track taking a saved state says.
fn state_taken(s: &State, n: u16, path: &std::path::Path) -> String {
    let file = path.file_stem().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
    format!("{} takes the mixer track state \"{file}\"", name(&s.session, n))
}

/// Shift+wheel over track `n`'s strip: it becomes the only selected track and moves one place per notch, left for
/// `notches` > 0, as Move left / right do. While the mouse stays still, more notches move the same track, which has left
/// the cursor.
pub fn wheel_move(app: &mut App, n: u16, notches: i32) {
    let at = app.mouse;
    let n = match app.state.mixer.move_grab {
        Some((g, m)) if m == at => g,
        _ => n,
    };
    if n == 0 {
        return;
    }
    select(&mut app.state, n, false);
    for _ in 0..notches.unsigned_abs() {
        let now = app.state.selected_mixer;
        app.state.message = run(app, if notches > 0 { MCmd::MoveLeft(now) } else { MCmd::MoveRight(now) });
    }
    app.state.mixer.move_grab = Some((app.state.selected_mixer, at));
}

/// Deletes track `n`: later tracks move one left, a default track is added at the end (§6.1 Delete).
pub fn delete_track(s: &mut State, n: u16) -> Option<String> {
    if n == 0 || n > inserts(&s.session) {
        return Some("The master track can't be deleted".into());
    }
    track_ids(s);
    let label = name(&s.session, n);
    apply(s, "Delete mixer track", vec![Command::DeleteTrack { track: id_of(s, n)? }]).or_else(|| Some(format!("Deleted {label}")))
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

/// Change color... ▸: Default, then the palette.
pub fn color_items(cmd: impl Fn(Option<Rgb>) -> Cmd) -> Vec<PItem> {
    let mut items = vec![PItem::new("Default", cmd(None))];
    items.extend(COLORS.iter().map(|&(l, c)| PItem::new(l, cmd(Some(Rgb::new((c >> 16) as u8, (c >> 8) as u8, c as u8))))));
    items
}

/// Track menu (right click on a strip), §6.1.
pub fn track_menu(s: &State, n: u16, x: f32, y: f32) -> Popup {
    let mut items = vec![PItem::head(format!("Mixer track ({})", name(&s.session, n)))];
    items.extend(track_items(s, n));
    Popup { x, y, items, sub: None, scroll: [0; 2] }
}

fn track_items(s: &State, n: u16) -> Vec<PItem> {
    let mx = |c: MCmd| Cmd::Mixer(c);
    let colors = color_items(|c| mx(MCmd::Color(n, c)));
    let t = track(&s.session, n);
    let dock = t.map_or(os_model::DOCK_MIDDLE, |m| m.dock);
    let dock_items = [(os_model::DOCK_LEFT, "Left"), (os_model::DOCK_MIDDLE, "Middle"), (os_model::DOCK_RIGHT, "Right")]
        .into_iter()
        .map(|(d, label)| PItem::new(label, mx(MCmd::Dock(n, d))).mark(Mark::Radio(dock == d)))
        .collect();
    // Greyed outside a group, as the original's Auto color group.
    let grouped = group(&s.session, n).is_some();
    let in_group = |it: PItem| if grouped { it } else { it.off() };
    vec![
        PItem::new("Rename, color and icon...", mx(MCmd::Rename(n))).key("F2"),
        PItem::sub("Change color...", colors),
        PItem::new("Random color", mx(MCmd::RandomColor(n))),
        // Greyed with one track selected, as in the original's menu (§6.1); with several it is Lemon Studio's version of
        // the channel rack's gradient (no dialog).
        if is_selected(s, n) && selected(s).len() > 1 { PItem::new("Gradient color...", mx(MCmd::GradientColor)) } else { PItem::not_yet("Gradient color...") },
        // Lemon Studio's design (session only, as the rack's): drawn under the track's name.
        PItem::sub("Change icon...", crate::playlist::icon_items(&s.session, crate::playlist::TRACK_ICON_KEY + usize::from(n))),
        // Greyed on tracks that have an audio track already (Assign) or none (Unassign), as in the original.
        {
            let tg = targets(s, n);
            let it = PItem::new("Assign to new Audio track(s)", mx(MCmd::AssignAudio(n)));
            if crate::track_links::can_assign_audio(&s.session, &tg) { it } else { it.off() }
        },
        {
            let tg = targets(s, n);
            let it = PItem::new("Unassign from Audio track(s)", mx(MCmd::UnassignAudio(n)));
            if crate::track_links::can_unassign_audio(&s.session, &tg) { it } else { it.off() }
        },
        PItem::sep(),
        PItem::sub(
            "File",
            vec![
                PItem::sub("Open mixer track state", state_items(|p| mx(MCmd::OpenState(n, p)))),
                PItem::new("Save mixer track state", mx(MCmd::SaveState(n))),
                PItem::sep(),
                PItem::new("Render to wave file...", mx(MCmd::RenderWave(n))),
            ],
        ),
        PItem::new("Open audio editor", mx(MCmd::AudioEditor(n))).key("Ctrl+E"),
        PItem::new("Open audio editor and record", mx(MCmd::AudioEditorRecord(n))).key("Shift+E"),
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
        track_routing(s, n),
        PItem::sub(
            "Select",
            vec![
                PItem::new("Channels routed to this track", mx(MCmd::SelectChannels(n))).key("Alt+L"),
                PItem::new("Channels and playlist tracks routed to this track", mx(MCmd::SelectChannelsTracks(n))),
                PItem::sep(),
                PItem::new("Tracks this one is routed to", mx(MCmd::SelectDest(n))),
                PItem::new("Tracks routed to this one", mx(MCmd::SelectSources(n))).key("Ctrl+Alt+L"),
                PItem::sep(),
                in_group(PItem::new("Group", mx(MCmd::SelectGroup(n)))),
                PItem::new("All", mx(MCmd::SelectAll)).key("Ctrl+A"),
            ],
        ),
        PItem::new("Reset selected track(s) routing", mx(MCmd::ResetRouting(n))),
        PItem::new("Unroute tracks routed to the selected track(s)", mx(MCmd::UnrouteTo(n))),
        PItem::sep(),
        PItem::new("Solo selected track(s)", mx(MCmd::Solo(n))).key("S"),
        PItem::new("Alt solo selected track(s)", mx(MCmd::AltSolo(n))).key("Alt+S"),
        PItem::new("Arm selected track(s)", mx(MCmd::Arm(n, true))),
        PItem::new("Disarm selected track(s)", mx(MCmd::Arm(n, false))),
        PItem::sep(),
        // Greyed in every capture of the original's menu (a track without an external output), and off.
        PItem::not_yet("Compensate for master latency").mark(Mark::Check(false)),
        PItem::head("Group"),
        insert_only(n, PItem::new("Separator", mx(MCmd::Separator(n))).mark(Mark::Check(t.is_some_and(|m| m.separator)))),
        insert_only(n, PItem::new("Create group...", mx(MCmd::CreateGroup(n)))),
        in_group(PItem::new("Auto color group", mx(MCmd::AutoColorGroup(n)))),
        PItem::head("Layout"),
        insert_only(n, PItem::sub("Dock to", dock_items)),
        PItem::new("Move left", mx(MCmd::MoveLeft(n))).key("Alt+Left"),
        PItem::new("Move right", mx(MCmd::MoveRight(n))).key("Alt+Right"),
        PItem::sep(),
        PItem::new("Delete", mx(MCmd::Delete(n))).key("Del"),
        PItem::new("Insert one", mx(MCmd::InsertOne(n))),
        PItem::new("Insert multiple...", mx(MCmd::InsertMultiple(n))),
        PItem::sub("Insert from state", state_items(|p| mx(MCmd::InsertState(n, p)))),
        PItem::head("CPU"),
        PItem::new("Allow threaded processing", mx(MCmd::Threaded(n))).mark(Mark::Check(t.is_none_or(|m| m.threaded))),
    ]
}

/// The saved mixer track states, one entry each running `cmd` with its file; a greyed "(no saved states)" without any.
fn state_items(cmd: impl Fn(std::path::PathBuf) -> Cmd) -> Vec<PItem> {
    let v: Vec<PItem> = crate::mixer_state::list().into_iter().map(|(name, p)| PItem::new(name, cmd(p))).collect();
    if v.is_empty() { vec![PItem::not_yet("(no saved states)")] } else { v }
}

/// Entries that do not apply to the master (its strip is fixed and starts no group).
fn insert_only(n: u16, it: PItem) -> PItem {
    if n == 0 { it.off() } else { it }
}

/// Track routing ▸ (Lemon Studio's design; greyed in the original's menu as captured, its entries not seen): the ▲ menu's
/// four entries (spec: ui/mixer.md §5) for the other selected tracks into track `n`. Greyed when no other track is
/// selected (the master never sends).
fn track_routing(s: &State, n: u16) -> PItem {
    let mx = |only, side| Cmd::Mixer(MCmd::Route { dst: n, only, side });
    let items = vec![
        PItem::new("Route selected tracks to this track", mx(false, false)),
        PItem::new("Route selected tracks to this track only", mx(true, false)),
        PItem::sep(),
        PItem::new("Sidechain selected tracks to this track", mx(false, true)),
        PItem::new("Sidechain selected tracks to this track only", mx(true, true)),
    ];
    let it = PItem::sub("Track routing", items);
    if is_selected(s, n) && selected(s).iter().any(|&t| t != n && t != 0) { it } else { it.off() }
}

/// Mixer options (title-bar ▸), §6.2: its own items, then the track menu's.
pub fn options_menu(s: &State, x: f32, y: f32) -> Popup {
    let mut items = vec![
        PItem::head("Options"),
        PItem::sub("Disk recording", vec![PItem::new("Auto-create audio clips", Cmd::Mixer(MCmd::AutoClips)).mark(Mark::Check(crate::prefs::check(&crate::prefs::AUTO_AUDIO_CLIPS)))]),
        PItem::sub("Plugin delay compensation", vec![PItem::new("Automatic", Cmd::Mixer(MCmd::PdcAuto)).mark(Mark::Check(!s.session.project.pdc_off))]),
        // The current track's own controls (Lemon Studio's design, ui/mixer.md §6.1; spec: ui/remote-control.md §6).
        PItem::new("Link all parameters...", Cmd::Remote(crate::remote::RCmd::LinkTrack(s.selected_mixer))),
        PItem::sub(
            "View",
            vec![
                PItem::new("Waveforms", Cmd::Mixer(MCmd::ViewWaveforms)).key("Alt+W").mark(Mark::Check(s.mixer.waveforms)),
                PItem::new("Extra properties", Cmd::Mixer(MCmd::ViewExtra)).mark(Mark::Check(s.mixer.extra)),
            ],
        ),
        // In a native window of its own (Lemon Studio's design: ui/mixer.md §6.2).
        PItem::new("Detached", Cmd::Detach(crate::app::Win::Mixer, !s.is_detached(crate::app::Win::Mixer))).mark(Mark::Check(s.is_detached(crate::app::Win::Mixer))),
        PItem::sep(),
        PItem::sub("Trim unused tracks", trim_items(&s.session)),
        PItem::new(AUTO_BYPASS_ALL_LABEL, Cmd::Mixer(MCmd::AutoBypassAll)),
        PItem::new("Remove all (audio editor) instances", Cmd::Mixer(MCmd::RemoveTeslas)),
        // The cable physics debug panel, hidden from users; uncomment to tune the cables again.
        // PItem::new("Cable physics (debug)...", Cmd::Mixer(MCmd::CablePhysics)).mark(Mark::Check(s.cables.borrow().debug)),
        PItem::head(format!("Mixer track ({})", name(&s.session, s.selected_mixer))),
    ];
    items.extend(track_items(s, s.selected_mixer));
    Popup { x, y, items, sub: None, scroll: [0; 2] }
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
    Popup { x, y, items, sub: None, scroll: [0; 2] }
}

/// Empty slot: the effect list (§7.1), the plugin palette for this slot at the click (spec: ui/plugin-menu.md §1, §6;
/// its footer has the Plugin library).
fn effect_palette(app: &mut App, slot: u8, at: (f32, f32)) {
    app.open_plugin_palette(crate::plugin_menu::Target::Fx(Some(slot)), at, None);
}

/// Filled slot ▸ (§7.2). Its Presets section works as in the plugin's window menu.
pub fn slot_menu(app: &App, slot: u8, x: f32, y: f32) -> Popup {
    let s = &app.state;
    let mx = |c: MCmd| Cmd::Mixer(c);
    let replace = crate::plugin_menu::items(s, crate::plugin_menu::Target::Fx(Some(slot)), true);
    // CPU: the plugin's switches, as in its window menu (spec: ui/plugin-window.md "CPU", audio/auto-bypass.md).
    let key = crate::plugins::fx_key(s.selected_mixer, slot);
    let cpu = crate::edit::playlist::plugin_cpu(&s.session, key).unwrap_or_default();
    let items = vec![
        PItem::sub("Replace", replace),
        PItem::new("Delete...", mx(MCmd::SlotDelete(slot))),
        PItem::sep(),
        // Lemon Studio's Patcher, slice 1 (`crate::container`).
        PItem { enabled: crate::container_ui::can_wrap(app, slot, false), ..PItem::new(crate::container::WRAP, mx(MCmd::Wrap(slot, false))) },
        PItem { enabled: crate::container_ui::can_wrap(app, slot, true), ..PItem::new(crate::container::WRAP_ALL, mx(MCmd::Wrap(slot, true))) },
        PItem::head("Presets"),
        PItem::sub("Presets", vec![PItem::not_yet("(none)")]),
        PItem::not_yet("Browse presets"),
        PItem::not_yet("Save preset as..."),
        PItem::new("Add to favourites", Cmd::Browser(crate::browser::BCmd::Star(crate::plugins::fx_key(s.selected_mixer, slot)))),
        PItem::not_yet("Display preset name in preset selector").mark(Mark::Check(true)),
        PItem::head("A/B compare"),
        PItem::not_yet("Copy to B"),
        PItem::not_yet("Swap A / B"),
        PItem::head("Parameters"),
        // As the plugin window menu's: the link box over the plugin's parameters.
        PItem::new("Link all parameters...", Cmd::Remote(crate::remote::RCmd::LinkAll(crate::plugins::fx_key(s.selected_mixer, slot)))),
        PItem::new("Show all parameters", Cmd::Param(crate::params::PCmd::Browse(crate::plugins::fx_key(s.selected_mixer, slot)))),
        PItem::sub("Sidechain input", sidechain_items(s, slot)),
        PItem::head("CPU"),
        PItem::new("Allow threaded processing", Cmd::PluginCpu(key, false)).mark(Mark::Check(cpu.threaded)),
        PItem::new("Auto-bypass when silent", Cmd::PluginCpu(key, true)).mark(Mark::Check(cpu.smart_disable)),
        PItem::sep(),
        PItem::new("Move up", mx(MCmd::SlotMove(slot, -1))),
        PItem::new("Move down", mx(MCmd::SlotMove(slot, 1))),
        PItem::sep(),
        PItem::new("View editor", mx(MCmd::SlotView(slot))).mark(Mark::Check(false)),
        PItem::new("Rename, color and icon...", mx(MCmd::SlotRename(slot))).key("F2"),
        // Lemon Studio's design (session only, as the slot's name): the track menu's colours, for the slot.
        PItem::sub("Change color...", color_items(|c| mx(MCmd::SlotColor(slot, c)))),
        PItem::new("Random color", mx(MCmd::SlotRandomColor(slot))),
        // Lemon Studio's design (session only): drawn before the slot's name and on its plugin window.
        PItem::sub("Change icon...", crate::playlist::icon_items(&s.session, crate::plugins::fx_key(s.selected_mixer, slot))),
        PItem::sep(),
        scaling_menu(app, slot),
        PItem::new("Help", mx(MCmd::SlotHelp(slot))),
    ];
    let mut items = items;
    // A container's members, listed (slice 1 gives them no windows of their own).
    if let Some(list) = crate::container_ui::list_items(s, slot) {
        items.insert(3, PItem::sub("Container", list));
    }
    let mut menu = Popup { x, y, items, sub: None, scroll: [0; 2] };
    crate::presets_ui::fill_window_menu(app, crate::plugins::fx_key(s.selected_mixer, slot), &mut menu);
    menu
}

/// Sidechain input ▸: "None" and the current track's inputs by name, the slot's choice marked.
fn sidechain_items(s: &State, slot: u8) -> Vec<PItem> {
    let t = s.selected_mixer;
    let now = track(&s.session, t).and_then(|m| m.slots.get(usize::from(slot))?.as_ref()).filter(|p| p.name == os_flp::wrapper::WRAPPER).map_or(0, |p| {
        os_flp::wrapper::input_ports(&p.state).get(1).map_or(0, |&p| p.max(0) as usize)
    });
    let mut items = vec![PItem::new("None", Cmd::Mixer(MCmd::SlotSidechain(slot, 0))).mark(Mark::Radio(now == 0))];
    for (i, n) in inputs(&s.session, t).into_iter().enumerate() {
        items.push(PItem::new(format!("{}. {}", i + 1, name(&s.session, n)), Cmd::Mixer(MCmd::SlotSidechain(slot, i + 1))).mark(Mark::Radio(now == i + 1)));
    }
    items
}

// ---------------------------------------------------------------------------------------------------------
// Commands

pub fn run(app: &mut App, c: MCmd) -> Option<String> {
    if let MCmd::SlotScale(slot, pct) = c {
        let key = crate::plugins::fx_key(app.state.selected_mixer, slot);
        app.state.editor_host.set_editor_scale(&mut app.state.session.plugins, key, pct.map(|p| f32::from(p) / 100.0));
        return None;
    }
    // A track's state takes its effects' settings from their instances.
    if let MCmd::SaveState(n) = c {
        let mut proj = app.state.session.project.clone();
        crate::project_plugins::store_hosted(&mut proj, &app.state.session.plugins);
        let m = proj.mixer.iter().find(|m| m.index == n)?;
        return Some(match crate::mixer_state::save(&crate::mixer_state::TrackState::of(m), &name(&app.state.session, n)) {
            Ok(p) => format!("Saved the mixer track state \"{}\"", p.file_stem().unwrap_or_default().to_string_lossy()),
            Err(e) => format!("Couldn't save the mixer track state: {e}"),
        });
    }
    if let MCmd::SlotHelp(slot) = c {
        let name = slot_name(&app.state.session, app.state.selected_mixer, slot).unwrap_or_default();
        return Some(match slot_help(app, slot) {
            Some(url) => {
                crate::help_links::open(&url);
                format!("Opening {url}")
            }
            None => format!("{name} has no help page"),
        });
    }
    // Wrap in container and Wrap all in container: one step, with the remote control links they move (`App::wrap_slot`).
    if let MCmd::Wrap(slot, all) = c {
        crate::container::store_members(app.state.session.project.get_mut(), &app.state.session.plugins);
        return app.wrap_slot(slot, all);
    }
    // An effect slot's colour: one step on its track, as FL's "Name/color/icon effect" (spec: ui/undo-steps.md).
    // The track settings: the commands of the control plane, one undo step each.
    if commands_of(&app.state, &c).is_some() {
        track_ids(&mut app.state);
    }
    if let Some((name, commands)) = commands_of(&app.state, &c) {
        let e = apply(&mut app.state, name, commands);
        app.invalidate();
        return e.or_else(|| match c {
            MCmd::PdcAuto => Some(format!("Automatic plugin delay compensation {}", if app.state.session.project.pdc_off { "off" } else { "on" })),
            _ => None,
        });
    }
    if let MCmd::Route { dst, only, side } = c {
        let m = route(&mut app.state, dst, only, side);
        app.invalidate();
        return m;
    }
    if let MCmd::AddEffect(slot, index) = c {
        let m = add_effect(&mut app.state, slot, index);
        app.invalidate();
        return m;
    }
    if let MCmd::RemoveTeslasNow = c {
        let gone = teslas(&app.state.session);
        track_ids(&mut app.state);
        let s = &app.state;
        let c = gone.iter().filter_map(|&(t, k)| Some(Command::RemoveEffect { track: id_of(s, t)?, slot: k })).collect();
        let e = apply(&mut app.state, "Remove audio editors", c);
        app.invalidate();
        return e.or_else(|| {
            Some(match gone.len() {
                1 => "Removed 1 audio editor".into(),
                n => format!("Removed {n} audio editors"),
            })
        });
    }
    // The commands that change the project are one undo step each (spec: roadmap/control-plane.md).
    let step = match &c {
        // The original's History names (spec: ui/one-click-recording.md §1).
        MCmd::AssignAudio(_) => Some(crate::track_links::ASSIGN_AUDIO),
        MCmd::UnassignAudio(_) => Some(crate::track_links::UNASSIGN_AUDIO),
        _ => None,
    };
    // The member edits apply their commands (`container_ui`), the containers' members as their windows left them first:
    // what the step keeps and an undo brings back.
    if matches!(c, MCmd::Member(..) | MCmd::MemberAdd(..) | MCmd::MemberAddInst(..) | MCmd::MemberReplace(..) | MCmd::MemberReplaceInst(..)) {
        crate::container::store_members(app.state.session.project.get_mut(), &app.state.session.plugins);
        return run_now(app, c);
    }
    if let Some(name) = step {
        return crate::control_ui::edit_app(app, name, Region::Project, |app| run_now(app, c));
    }
    run_now(app, c)
}

/// The menu commands that are commands of the control plane: their step's name and the commands.
fn commands_of(s: &State, c: &MCmd) -> Option<(&'static str, Vec<Command>)> {
    let each = |n: u16, f: &dyn Fn(u16) -> Option<Command>| targets(s, n).into_iter().filter_map(f).collect::<Vec<_>>();
    let color = |t: u16, c: Option<Rgb>| set_track!(s, t, color: color_text(c).unwrap_or_default());
    Some(match *c {
        MCmd::Color(n, col) => ("Mixer track color", each(n, &|t| color(t, col))),
        MCmd::RandomColor(n) => ("Mixer track color", each(n, &|t| color(t, Some(crate::patterns::random_color(u64::from(t).wrapping_mul(0x9e37_79b9) ^ u64::from(rand_seed())))))),
        MCmd::GradientColor => {
            let mut t = selected(s);
            t.sort_unstable();
            let mut c = Vec::new();
            if let (Some(&a), Some(&b)) = (t.first(), t.last()) {
                let (a, b) = (track(&s.session, a).and_then(|m| m.color), track(&s.session, b).and_then(|m| m.color));
                c = crate::rack::gradient_colors(a, b, t.len()).into_iter().zip(&t).filter_map(|(c, &n)| color(n, Some(c))).collect();
            }
            ("Mixer track color", c)
        }
        MCmd::AutoColorGroup(n) => {
            let col = group(&s.session, n).and_then(|g| Some((g.clone(), track(&s.session, *g.start())?.color?)));
            ("Mixer track color", col.map(|(g, col)| g.filter_map(|t| color(t, Some(col))).collect()).unwrap_or_default())
        }
        MCmd::Separator(n) => {
            let on = !track(&s.session, n).is_some_and(|m| m.separator);
            ("Mixer track separator", each(n, &|t| (t > 0).then(|| set_track!(s, t, separator: on)).flatten()))
        }
        MCmd::Dock(n, d) => {
            let d = ["left", "middle", "right"][usize::from(d.min(os_model::DOCK_RIGHT))];
            ("Mixer track docking", each(n, &|t| (t > 0).then(|| set_track!(s, t, dock: d.to_string())).flatten()))
        }
        MCmd::Threaded(n) => {
            let on = !track(&s.session, n).is_none_or(|m| m.threaded);
            ("Mixer track threading", each(n, &|t| set_track!(s, t, threaded: on)))
        }
        MCmd::Reset(n) => ("Mixer track reset", each(n, &|t| Some(Command::ResetTrack { track: id_of(s, t)? }))),
        MCmd::ResetRouting(n) => {
            let mut c = Vec::new();
            for t in targets(s, n).into_iter().filter(|&t| t != 0) {
                c.extend(unroute_all(s, t, None));
                c.extend(id_of(s, t).zip(id_of(s, 0)).map(|(from, to)| Command::SetSend { from, to, level: Some(0.8) }));
            }
            (ROUTING, c)
        }
        MCmd::UnrouteTo(n) => {
            let tg = targets(s, n);
            let last = inserts(&s.session);
            let mut c = Vec::new();
            for m in s.session.project.mixer.iter().filter(|m| m.index <= last) {
                let mut dests: Vec<u16> = m.routes.iter().copied().chain(m.send_levels.iter().map(|l| l.0)).filter(|d| tg.contains(d)).collect();
                dests.sort_unstable();
                dests.dedup();
                c.extend(dests.into_iter().filter_map(|d| Some(Command::SetSend { from: m.id.0, to: id_of(s, d)?, level: None })));
            }
            (ROUTING, c)
        }
        MCmd::SlotDelete(k) => ("Remove effect", vec![Command::RemoveEffect { track: id_of(s, s.selected_mixer)?, slot: k }]),
        MCmd::SlotMove(k, d) => {
            let o = i32::from(k) + i32::from(d);
            let c = (0..10).contains(&o).then(|| id_of(s, s.selected_mixer).map(|track| Command::MoveEffect { track, slot: k, to: o as u8 })).flatten();
            ("Move effect", c.into_iter().collect())
        }
        MCmd::SlotColor(k, col) => ("Effect slot color", vec![slot_color_cmd(s, k, col)?]),
        MCmd::SlotRandomColor(k) => {
            let t = s.selected_mixer;
            let col = crate::patterns::random_color((u64::from(t) << 8 | u64::from(k)).wrapping_mul(0x9e37_79b9) ^ u64::from(rand_seed()));
            ("Effect slot color", vec![slot_color_cmd(s, k, Some(col))?])
        }
        MCmd::AutoBypassAll => ("Auto-bypass for all plugins", crate::edit::mixer::auto_bypass_switch(&s.session).map(|on| Command::SetAutoBypass { on }).into_iter().collect()),
        MCmd::PdcAuto => {
            let mut c = Command::set_project();
            if let Command::SetProject { pdc, .. } = &mut c {
                *pdc = Some(s.session.project.pdc_off);
            }
            ("Plugin delay compensation", vec![c])
        }
        _ => return None,
    })
}

/// Slot `k` of the current track coloured `col`.
fn slot_color_cmd(s: &State, k: u8, col: Option<Rgb>) -> Option<Command> {
    Some(Command::SetSlot { track: id_of(s, s.selected_mixer)?, slot: k, enabled: None, mix: None, name: None, color: Some(color_text(col).unwrap_or_default()), sidechain: None, auto_bypass: None, threaded: None })
}

/// Effect `index` (in the plugin list) into slot `k` of the current track: one undo step, "Add effect". The plugin is
/// hosted as the interface hosts it (its window opens), not as a client's command is.
fn add_effect(s: &mut State, k: u8, index: usize) -> Option<String> {
    add_effect_on(s, s.selected_mixer, k, index, None)
}

/// Effect `index` into slot `k` of track `t`, from `state` when given: its command, one step "Add effect", the
/// interface hosting it its own way so its window opens.
fn add_effect_on(s: &mut State, t: u16, k: u8, index: usize, state: Option<Vec<u8>>) -> Option<String> {
    let nm = s.session.plugins.effects.get(index).map(|a| a.desc.name.clone())?;
    track_ids(s);
    let c = Command::AddEffect { track: id_of(s, t)?, slot: Some(k), plugin: nm };
    if let Err(e) = crate::control_ui::command(s, "Add effect", None, vec![c]) {
        return Some(e);
    }
    let id = track(&s.session, t).and_then(|m| m.slots[usize::from(k)].as_ref()).map(|p| p.id);
    s.session.control.host.retain(|r| !matches!(r, crate::control::HostReq::Effect { slot, .. } if Some(*slot) == id));
    s.session.fx_requests.push(FxReq::Create { track: t, slot: k, index, state });
    None
}

fn run_now(app: &mut App, c: MCmd) -> Option<String> {
    let at = app.mouse;
    let s = &mut app.state;
    let msg = match c {
        MCmd::Rename(n) => {
            start_rename(s, n, at);
            None
        }
        MCmd::AudioEditor(n) => {
            app.audio_editor(n, false);
            return None;
        }
        MCmd::AudioEditorRecord(n) => {
            app.audio_logger(n);
            return None;
        }
        MCmd::RemoveTeslas => return app.ask_remove_teslas(),
        MCmd::Input(t, v) => {
            // An input chosen where there was none asks whether to record it alone (§1.4).
            let ask = track(&s.session, t).is_some_and(|m| m.audio_in < 0 && v >= 0 && m.rec_source != os_model::REC_EXTERNAL);
            if set_io(&mut s.session, t, |m| m.audio_in = v) && ask {
                app.ask_external(t);
            }
            return None;
        }
        MCmd::Output(t, v) => {
            set_io(&mut s.session, t, |m| m.audio_out = v);
            None
        }
        MCmd::RecSource(t, v) => {
            set_io(&mut s.session, t, |m| m.rec_source = v);
            None
        }
        MCmd::Monitor(t, v) => {
            set_io(&mut s.session, t, |m| m.monitor = v);
            None
        }
        MCmd::Solo(n) => {
            solo(s, n);
            None
        }
        MCmd::Arm(n, on) => {
            let tg = targets(s, n);
            app.arm_tracks(&tg, on)
        }
        MCmd::ArmTrack(t, on) => app.arm_tracks(&[t], on),
        MCmd::Latency(t, input, None) => {
            set_latency(s, t, input, 0.0);
            None
        }
        MCmd::Latency(t, input, Some(u)) => {
            crate::typein_ui::open(s, crate::typein::Target::Latency(t, input, u), at);
            None
        }
        MCmd::CreateGroup(n) => {
            crate::playlist::start_edit(s, EditTarget::MixerGroup(n), at);
            None
        }
        MCmd::SelectGroup(n) => {
            let Some(g) = group(&s.session, n) else { return Some("Not in a group".into()) };
            s.selected_mixer = n;
            s.mixer.sel = g.collect();
            None
        }
        // Answered in `run`.
        MCmd::SlotHelp(_) | MCmd::SlotScale(..) | MCmd::SaveState(_) => None,
        MCmd::AddEffect(..) | MCmd::RemoveTeslasNow | MCmd::SlotDelete(_) | MCmd::SlotMove(..) | MCmd::SlotColor(..) | MCmd::SlotRandomColor(_) => None,
        MCmd::Color(..)
        | MCmd::RandomColor(_)
        | MCmd::GradientColor
        | MCmd::AutoColorGroup(_)
        | MCmd::Separator(_)
        | MCmd::Dock(..)
        | MCmd::Threaded(_)
        | MCmd::AutoBypassAll
        | MCmd::PdcAuto
        | MCmd::Reset(_)
        | MCmd::ResetRouting(_)
        | MCmd::UnrouteTo(_)
        | MCmd::Route { .. } => None,
        MCmd::OpenState(n, p) => load_state(s, n, &p),
        MCmd::InsertState(n, p) => insert_state(s, n, &p),
        MCmd::ViewWaveforms => {
            view_waveforms(s);
            None
        }
        MCmd::ViewExtra => {
            s.mixer.extra = !s.mixer.extra;
            None
        }
        MCmd::AutoClips => {
            let on = crate::prefs::toggle(&crate::prefs::AUTO_AUDIO_CLIPS);
            Some(format!("Auto-create audio clips {}", if on { "on" } else { "off" }))
        }
        MCmd::AltSolo(n) => {
            solo_with(s, n, true);
            None
        }
        MCmd::RouteChannels { dst, spread } => route_channels(s, dst, spread),
        MCmd::SelectChannels(n) => {
            let tg = targets(s, n);
            let rows: std::collections::BTreeSet<usize> =
                s.session.project.channels.iter().enumerate().filter(|(_, c)| tg.contains(&c.mixer_track)).map(|(i, _)| i).collect();
            let k = rows.len();
            s.rack.sel = rows;
            Some(format!("Selected {k} channel(s)"))
        }
        MCmd::SelectChannelsTracks(n) => {
            let tg = targets(s, n);
            let rows: std::collections::BTreeSet<usize> =
                s.session.project.channels.iter().enumerate().filter(|(_, c)| tg.contains(&c.mixer_track)).map(|(i, _)| i).collect();
            let k = rows.len();
            s.rack.sel = rows;
            let tracks = crate::track_links::tracks_routed_to(&s.session, &tg);
            if let Some(&t) = tracks.first() {
                s.playlist.track = t;
            }
            Some(format!("Selected {k} channel(s) and {} playlist track(s)", tracks.len()))
        }
        MCmd::AssignAudio(n) => {
            let tg = targets(s, n);
            crate::track_links_ui::assign_audio(s, &tg)
        }
        MCmd::UnassignAudio(n) => {
            let tg = targets(s, n);
            crate::track_links::unassign_audio(&mut s.session, &tg)
        }
        MCmd::SelectDest(n) => select_linked(s, n, true),
        MCmd::SelectSources(n) => select_linked(s, n, false),
        MCmd::SelectAll => {
            s.mixer.sel = s.session.project.mixer.iter().map(|m| m.index).filter(|&t| t <= inserts(&s.session)).collect();
            s.mixer.faders = FaderGroup::default();
            None
        }
        MCmd::Trim { non_empty } => {
            let gone = trim_list(&s.session, non_empty);
            remove_tracks(s, &gone)
        }
        MCmd::MoveLeft(n) => move_track(s, n, -1),
        MCmd::MoveRight(n) => move_track(s, n, 1),
        MCmd::Delete(n) => delete_track(s, n),
        MCmd::InsertOne(n) => Some(insert_tracks(s, Some(n.max(1)), 1).map_or_else(|e| e, |n| format!("Inserted {}", name(&s.session, n)))),
        MCmd::InsertMultiple(n) => {
            crate::playlist::start_edit(s, EditTarget::MixerInsert(n.max(1)), at);
            if let Some(e) = s.playlist.edit.as_mut() {
                e.sel = true;
            }
            Some("How many tracks to insert: type a number, Enter inserts them".into())
        }
        MCmd::RenderWave(n) => {
            let tracks = targets(s, n);
            app.open_track_render(tracks);
            return None;
        }
        // Wide is the layout shown; the others are greyed out in the menu.
        MCmd::Layout(_) => None,
        MCmd::CablePhysics => {
            let c = s.cables.get_mut();
            c.debug = !c.debug;
            None
        }
        MCmd::Animations(l) => {
            crate::anim::set_level(l);
            crate::anim::save(l);
            s.cables.get_mut().animate(crate::anim::cables_swing(l));
            // Everything restyles at once (menus' borders, what animates): the whole window repaints.
            app.invalidate();
            None
        }
        // The slot's instance goes as a replaced effect's does; the container plays from the next tick
        // (`App::sync_containers`).
        MCmd::AddContainer(slot) => {
            track_ids(s);
            let t = s.selected_mixer;
            let c = id_of(s, t).map(|track| Command::AddEffect { track, slot: Some(slot), plugin: crate::container::CONTAINER.into() });
            c.and_then(|c| apply(s, "Add effect", vec![c]))
        }
        // `run` makes it a step of its own.
        MCmd::Wrap(..) => None,
        MCmd::Member(slot, i, op) => {
            let msg = app.edit_member(slot, i, op);
            changed(&mut app.state.session);
            msg
        }
        MCmd::MemberAdd(slot, index) | MCmd::MemberAddInst(slot, index) => {
            let msg = app.add_member(slot, index, matches!(c, MCmd::MemberAddInst(..)));
            changed(&mut app.state.session);
            msg
        }
        MCmd::MemberReplace(slot, i, index) | MCmd::MemberReplaceInst(slot, i, index) => {
            let msg = app.replace_member(slot, i, index, matches!(c, MCmd::MemberReplaceInst(..)));
            changed(&mut app.state.session);
            msg
        }
        MCmd::MemberRename(slot, i) => {
            let t = s.selected_mixer;
            if let Some(m) = crate::edit::container::members(&s.session, t, slot).and_then(|l| l.get(i).cloned()) {
                crate::playlist::start_edit(s, EditTarget::MemberName(t, slot, m.id), at);
            }
            None
        }
        MCmd::MemberColor(slot, i, c) => {
            let t = s.selected_mixer;
            if let Some(m) = crate::edit::container::members(&s.session, t, slot).and_then(|l| l.get(i).cloned()) {
                crate::container_ui::edit_own(s, t, slot, m.id, crate::container::COLOR, |m| m.color = c);
            }
            None
        }
        MCmd::MemberRandomColor(slot, i) => {
            let t = s.selected_mixer;
            if let Some(m) = crate::edit::container::members(&s.session, t, slot).and_then(|l| l.get(i).cloned()) {
                let seed = (u64::from(t) << 16 | u64::from(slot) << 8 | u64::from(m.id)).wrapping_mul(0x9e37_79b9) ^ u64::from(rand_seed());
                let c = crate::patterns::random_color(seed);
                crate::container_ui::edit_own(s, t, slot, m.id, crate::container::COLOR, |m| m.color = Some(c));
            }
            None
        }
        MCmd::ContainerList(slot) => {
            crate::container_ui::show(s, s.selected_mixer, slot);
            None
        }
        MCmd::SlotRename(slot) => {
            let t = s.selected_mixer;
            crate::playlist::start_edit(s, EditTarget::SlotName(t, slot), at);
            None
        }
        MCmd::SlotView(slot) => {
            let t = s.selected_mixer;
            s.session.fx_requests.push(FxReq::Toggle { track: t, slot });
            None
        }
        // A slot's sidechain input is the track's own (its slot's saved state).
        MCmd::SlotSidechain(slot, port) => {
            track_ids(s);
            let t = s.selected_mixer;
            let c = id_of(s, t).map(|track| Command::SetSlot { track, slot, enabled: None, mix: None, name: None, color: None, sidechain: Some(port as u32), auto_bypass: None, threaded: None });
            c.and_then(|c| apply(s, "Sidechain input", vec![c]))
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
    crate::clip_colors::clock() as u32
}

/// Keys while the mixer is in front (§2): F2 rename, Del delete, Alt+Left / Right move, S solo, Alt+S alt solo,
/// Ctrl+L / Shift+Ctrl+L route the selected channels, Alt+L / Ctrl+Alt+L / Ctrl+A select (spec:
/// ui/mixer-track-commands.md).
pub fn key(app: &mut App, key: &crate::keys::Key, ctrl: bool, alt: bool) -> bool {
    use crate::keys::{Key, NamedKey};
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
        Key::Character(c) if alt && letter(c, "w") => run(app, MCmd::ViewWaveforms),
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
            if n < inserts(&app.state.session) {
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
    // View ▸ Waveforms: one reading per meter step; the waveforms keep scrolling until they have gone quiet.
    if s.mixer.waveforms {
        let w = &mut s.mixer.waves;
        w.resize_with(peaks.len().max(w.len()), Default::default);
        for (i, wave) in w.iter_mut().enumerate() {
            let p = peaks.get(i).copied().unwrap_or([0.0; 2]);
            wave.push_back(p.map(f32::abs));
            if wave.len() > WAVE_LEN {
                wave.pop_front();
            }
        }
        moving |= w.iter().any(|wave| wave.iter().any(|p| p[0] > 1e-5 || p[1] > 1e-5));
    }
    if !moving {
        s.mixer.meter_at = None;
    }
    moving
}

impl App {
    /// Mixer slot work on the UI thread (`plugin_tick`): create / open / close / drop hosted effect instances.
    pub fn mixer_tick(&mut self) {
        self.mixer_requests();
        self.sync_containers();
    }

    fn mixer_requests(&mut self) {
        if self.state.session.fx_requests.is_empty() {
            return;
        }
        let done = self.host(fx_requests_in);
        if let Some(index) = done.trying {
            let name = self.state.session.plugins.effects.get(index).map(|a| a.desc.name.clone()).unwrap_or_default();
            let msg = format!("Trying {name} before its first load...");
            if self.state.message.as_deref() != Some(msg.as_str()) {
                self.state.message = Some(msg);
                self.invalidate();
            }
        }
        for (track, slot, toggle) in done.windows {
            if !(toggle && self.state.editor_host.close_if_open(&mut self.state.session.plugins, crate::plugins::fx_key(track, slot))) {
                self.open_fx(track, slot);
            }
        }
    }

    /// Tesla on track `t`: the toolbar's audio editor button puts it on the master, as FL opens Edison there (spec:
    /// plugins/tesla/requests.md #5); the track menu's "Open audio editor" on that track. It shows the track's first
    /// Tesla, adding one when it has none; `new` (the button's right click) always adds one, in the first free slot.
    /// An added Tesla is an "Add effect" step, as FL's Edison is one ("insert effect plugin").
    pub fn audio_editor(&mut self, t: u16, new: bool) {
        if !new {
            if let Some(k) = first_tesla(&self.state.session, t) {
                self.state.session.fx_requests.push(FxReq::Show { track: t, slot: k });
                return self.invalidate();
            }
        }
        self.add_tesla(t, None);
    }

    /// Open audio editor and record: a Tesla on track `t` set up as an audio logger, as FL's quick load sets Edison
    /// (spec: plugins/tesla/README.md "Recording into Tesla from Lemon Studio"): records on the host's play, for 5
    /// minutes, appended, slaved to the transport, armed.
    pub fn audio_logger(&mut self, t: u16) {
        self.tesla_for_recording(t, |s| {
            s.rec.when = tesla::state::WHEN_ON_PLAY;
            s.rec.max = AUDIO_LOG_5_MINUTES;
            s.rec.append = true;
            s.rec.slave = true;
            s.rec.armed = true;
        });
    }

    /// A Tesla on track `t` to record into (spec: plugins/tesla/README.md "Recording into Tesla from Lemon Studio"):
    /// the track's first Tesla when it holds no sample, else a new one in the first free slot, so no audio or undo
    /// history is touched. `set` changes its state (the empty one is reloaded with it, a new one starts from it) and
    /// its window is shown. False when there is no Tesla to use (the message says why).
    pub fn tesla_for_recording(&mut self, t: u16, set: impl FnOnce(&mut tesla::state::Saved)) -> bool {
        use tesla::state::{from_plugin_state, plugin_state, Saved};
        let empty = first_tesla(&self.state.session, t).and_then(|k| {
            let key = crate::plugins::fx_key(t, k);
            let saved = self.state.session.plugins.state(key).and_then(|b| from_plugin_state(&b)).filter(|s| s.sample.is_empty())?;
            Some((k, key, saved))
        });
        let saved = match empty {
            Some((k, key, mut saved)) => {
                set(&mut saved);
                if self.state.session.plugins.set_state(key, &plugin_state(&saved)) {
                    self.state.session.plugins.mark_edited(key);
                    self.state.session.fx_requests.push(FxReq::Show { track: t, slot: k });
                    changed(&mut self.state.session);
                    self.invalidate();
                    return true;
                }
                saved
            }
            None => {
                let mut saved = Saved::default();
                set(&mut saved);
                saved
            }
        };
        self.add_tesla(t, Some(plugin_state(&saved)))
    }

    /// A new Tesla in track `t`'s first free slot, starting from `state` when given; false (with a message) when
    /// Tesla is missing or the track is full.
    fn add_tesla(&mut self, t: u16, state: Option<Vec<u8>>) -> bool {
        use crate::plugins::TESLA;
        let s = &mut self.state;
        let Some(index) = s.session.plugins.effect_index(TESLA) else {
            s.message = Some(format!("{TESLA} is not available"));
            self.invalidate();
            return false;
        };
        let Some(k) = track(&s.session, t).and_then(|m| m.slots.iter().position(Option::is_none)) else {
            s.message = Some("No free effect slot on this track".into());
            self.invalidate();
            return false;
        };
        forget_slot_ui(&mut s.session, t, k as u8);
        // One step, "Add effect", as FL's Edison is one ("insert effect plugin").
        let e = add_effect_on(s, t, k as u8, index, state);
        let ok = e.is_none();
        if let Some(e) = e {
            s.message = Some(e);
        }
        self.invalidate();
        ok
    }

    /// Remove all (audio editor) instances (spec: plugins/tesla/README.md "Recording into Tesla from Lemon Studio"):
    /// asks first, since removed slots can't be brought back yet; with none, says so.
    /// The External input question for track `t` (spec: audio/track-io.md §1.4), unless Remember my choice answered it.
    pub fn ask_external(&mut self, t: u16) {
        use crate::dialog::{spans, Answer, Kind, Msg, Then};
        // Settings ▸ Recording's External input question: Ask, Switch to External input only, Keep the record source.
        match crate::prefs::index(&crate::prefs::EXTERNAL_INPUT) {
            1 => external_only(&mut self.state.session, t),
            2 => {}
            _ => {
                let text = "You've selected an external input. Do you want to switch to External input only to avoid recording internal audio on this track?";
                let mut m = Msg::new(Kind::Question, "External input", spans(&[text]), &[Answer::Yes, Answer::No], Then::ExtInput(t));
                m.remember = Some((&crate::messages::EXT_INPUT, false));
                self.show_msg(m);
            }
        }
    }

    pub fn ask_remove_teslas(&mut self) -> Option<String> {
        let n = teslas(&self.state.session).len();
        if n == 0 {
            return Some("No audio editors in the mixer".into());
        }
        let (x, y) = self.mouse;
        let items = vec![
            PItem::head(remove_question(n)),
            PItem::new("Remove", Cmd::Mixer(MCmd::RemoveTeslasNow)),
            PItem::new("Cancel", Cmd::Nothing),
        ];
        self.state.playlist.popup = Some(Popup { x, y, items, sub: None, scroll: [0; 2] });
        self.invalidate();
        None
    }

    pub fn open_fx(&mut self, track: u16, slot: u8) {
        let key = crate::plugins::fx_key(track, slot);
        // A container: its list's window.
        if crate::edit::container::members(&self.state.session, track, slot).is_some() {
            crate::container_ui::show(&mut self.state, track, slot);
            return;
        }
        if !self.state.session.plugins.hosted.contains_key(&key) {
            self.state.message = Some("This slot's plugin is not hosted (no Lemon Studio version of it yet)".into());
            return;
        }
        #[cfg(not(plugin_windows))]
        {
            self.state.message = Some(crate::app::plugin_windows_off());
        }
        #[cfg(plugin_windows)]
        {
            let info = fx_info(&self.state.session.project, key);
            match info.map(|i| self.state.editor_host.open_window(&mut self.state.session.plugins, key, i)) {
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
    pub fn select_window_track(&mut self, key: usize) {
        if !crate::prefs::check(&crate::prefs::AUTO_SELECT_LINKED) {
            return;
        }
        let s = &self.state;
        let track = match crate::plugins::fx_of(key) {
            Some((t, _)) => Some(t),
            None => s.session.project.channels.get(key).map(|c| c.mixer_track),
        };
        let Some(t) = track.filter(|t| s.session.project.mixer.iter().any(|m| m.index == *t)) else { return };
        select(&mut self.state, t, false);
        // The master and the docked strips are fixed; a middle one scrolls into the mixer's last painted view.
        let place = crate::views::mixer::docks(&self.state).place(t).filter(|&(p, _)| p == 1);
        if let (Some(bar), Some((_, left))) = (self.bar(crate::app::Scroll::MixerX), place) {
            let w = crate::views::mixer::STRIP_W;
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
pub fn fx_info(p: &os_model::Project, key: usize) -> Option<crate::plugin_window::ChannelInfo> {
    let (t, k) = crate::plugins::fx_of(key)?;
    let m = p.mixer.iter().find(|m| m.index == t)?;
    let name = m.slot_names.get(usize::from(k)).cloned().flatten().or_else(|| m.slots.get(usize::from(k))?.as_ref().map(crate::project_plugins::slot_title))?;
    Some(crate::plugin_window::ChannelInfo {
        misc: None,
        icon: None,
        name,
        target: m.name.clone().filter(|n| !n.is_empty()).unwrap_or_else(|| if t == 0 { "Master".into() } else { format!("Bus {t}") }),
        enabled: m.slot_on[usize::from(k)],
        volume: 0.8,
        pan: 0.0,
        pitch_cents: 0,
        range: 2,
        track: t,
        max_track: t,
    })
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
pub fn latency_menu(s: &crate::session::Session, t: u16, input: bool, x: f32, y: f32) -> Popup {
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

/// Trim unused tracks ▸ (spec: ui/mixer-track-commands.md §6): each choice with the number of tracks it deletes.
pub fn trim_items(s: &crate::session::Session) -> Vec<PItem> {
    [(false, "Unlinked tracks only"), (true, "Unlinked and non-empty tracks")]
        .into_iter()
        .map(|(non_empty, label)| {
            let k = trim_list(s, non_empty).len();
            let it = PItem::new(format!("{label} ({k})"), Cmd::Mixer(MCmd::Trim { non_empty }));
            if k == 0 { it.off() } else { it }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The track menu entry `label` (searched in the submenus too): whether it is enabled, its command and its mark.
    fn track_entry(a: &App, n: u16, label: &str) -> (bool, Cmd, Mark) {
        let items = track_items(&a.state, n);
        let all = items.iter().chain(items.iter().flat_map(|i| i.children.iter()));
        let found = all.filter(|i| i.label == label && !i.head).last().unwrap_or_else(|| panic!("no entry {label}"));
        (found.enabled, found.cmd.clone(), found.mark)
    }

    /// spec: ui/mixer.md §6.1 "Group, Layout and CPU entries": Separator toggles on the targets, one undo step; a group
    /// runs from a separator to the next; Create group... names the track and puts separators around the selection;
    /// Auto color group gives the group its first track's colour; Select ▸ Group selects it.
    #[test]
    fn group_entries_set_separators_and_act_on_the_group() {
        let mut a = app();
        assert_eq!(track_entry(&a, 3, "Separator"), (true, Cmd::Mixer(MCmd::Separator(3)), Mark::Check(false)));
        assert!(!track_entry(&a, 0, "Separator").0, "not on the master");
        assert!(!track_entry(&a, 3, "Auto color group").0, "no group yet");
        assert!(!track_entry(&a, 3, "Group").0, "Select ▸ Group: no group yet");
        assert_eq!(track_entry(&a, 3, "Compensate for master latency"), (false, Cmd::NotYet("Compensate for master latency"), Mark::Check(false)));
        let steps = a.state.session.history.at();
        run(&mut a, MCmd::Separator(3));
        assert!(track(&a.state.session, 3).unwrap().separator);
        assert_eq!(a.state.session.history.at(), steps + 1);
        assert_eq!(track_entry(&a, 3, "Separator").2, Mark::Check(true));
        assert_eq!((group(&a.state.session, 2), group(&a.state.session, 3), group(&a.state.session, 16)), (None, Some(3..=16), Some(3..=16)));
        crate::history_ui::undo(&mut a.state);
        assert!(!track(&a.state.session, 3).unwrap().separator);

        // Create group... on 5 with 5 to 7 selected: the name box, then Enter.
        select(&mut a.state, 5, false);
        select(&mut a.state, 6, true);
        select(&mut a.state, 7, true);
        select(&mut a.state, 5, true);
        run(&mut a, MCmd::CreateGroup(5));
        let e = a.state.playlist.edit.as_ref().expect("the name box");
        assert_eq!((e.target, e.text.as_str()), (EditTarget::MixerGroup(5), ""));
        a.state.playlist.edit.as_mut().unwrap().text = "Drums".into();
        let steps = a.state.session.history.at();
        crate::playlist::commit_edit(&mut a.state);
        assert_eq!(name(&a.state.session, 5), "Drums");
        let seps: Vec<u16> = (1..=inserts(&a.state.session)).filter(|&t| track(&a.state.session, t).unwrap().separator).collect();
        assert_eq!(seps, vec![5, 8]);
        assert_eq!(group(&a.state.session, 6), Some(5..=7));
        assert_eq!(a.state.session.history.at(), steps + 1, "one step");

        // Auto color group: the first track's colour over the group; Select ▸ Group.
        let red = Rgb::new(200, 0, 0);
        track_mut(&mut a.state.session, 5).unwrap().color = Some(red);
        select(&mut a.state, 6, false);
        assert!(track_entry(&a, 6, "Auto color group").0);
        run(&mut a, MCmd::AutoColorGroup(6));
        let col = |a: &App, n: u16| track(&a.state.session, n).unwrap().color;
        assert_eq!((col(&a, 5), col(&a, 6), col(&a, 7), col(&a, 8)), (Some(red), Some(red), Some(red), None));
        assert_eq!(track_entry(&a, 6, "Group").1, Cmd::Mixer(MCmd::SelectGroup(6)));
        run(&mut a, MCmd::SelectGroup(6));
        assert_eq!((a.state.selected_mixer, a.state.mixer.sel.clone()), (6, vec![5, 6, 7]));
        // Esc in the name box changes nothing.
        run(&mut a, MCmd::CreateGroup(12));
        a.state.playlist.edit = None;
        assert!(!track(&a.state.session, 12).unwrap().separator);
    }

    /// Dock to ▸ (radio) and Allow threaded processing (check), saved in the track; docked strips are drawn after the
    /// master (left) or at the right edge (right) and do not scroll; a separator's gap is 6 px (spec: ui/mixer.md §6.1).
    #[test]
    fn docking_and_threading_entries() {
        use crate::views::mixer::{docks, SEP_W, STRIP_W};
        let mut a = app();
        assert_eq!(track_entry(&a, 4, "Middle"), (true, Cmd::Mixer(MCmd::Dock(4, os_model::DOCK_MIDDLE)), Mark::Radio(true)));
        assert_eq!(track_entry(&a, 4, "Right").2, Mark::Radio(false));
        assert!(!track_entry(&a, 0, "Dock to").0, "the master stays put");
        assert_eq!(track_entry(&a, 4, "Allow threaded processing"), (true, Cmd::Mixer(MCmd::Threaded(4)), Mark::Check(true)));
        run(&mut a, MCmd::Dock(4, os_model::DOCK_RIGHT));
        run(&mut a, MCmd::Dock(2, os_model::DOCK_LEFT));
        run(&mut a, MCmd::Separator(7));
        run(&mut a, MCmd::Threaded(4));
        assert_eq!(track_entry(&a, 4, "Right").2, Mark::Radio(true));
        assert!(!track(&a.state.session, 4).unwrap().threaded);
        assert_eq!(track_entry(&a, 4, "Allow threaded processing").2, Mark::Check(false));
        let d = docks(&a.state);
        let ids = |p: usize| d.parts[p].iter().map(|e| e.0).collect::<Vec<_>>();
        assert_eq!((ids(0), ids(2)), (vec![2], vec![4]));
        assert_eq!(ids(1), vec![1, 3, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]);
        // 1, 3, 5, 6 before 7: its gap.
        assert_eq!(d.place(7), Some((1, 4.0 * STRIP_W + SEP_W)));
        assert_eq!(d.widths, [STRIP_W, 14.0 * STRIP_W + SEP_W, STRIP_W]);
        // Painted: the right-docked strip at the right end of the strips, the left-docked one after the master.
        let Ok(mut c) = os_ui::Canvas::new(1920, 1080, 1.0) else { return };
        let ws = &mut a.state.wins[crate::app::Win::Mixer.slot().unwrap()];
        (ws.maxed, ws.rect) = (false, os_ui::Rect::new(100.0, 100.0, 900.0, 500.0));
        a.state.z = vec![crate::app::Win::Mixer];
        a.paint(&mut c, None);
        let at = |a: &App, n: u16| a.hits.iter().rev().find(|h| h.action == crate::app::Action::Mix(MHit::Strip(n))).map(|h| h.rect).unwrap();
        let (master, left, first, right) = (at(&a, 0), at(&a, 2), at(&a, 1), at(&a, 4));
        assert_eq!(left.x, master.right(), "left-docked right after the master");
        assert_eq!(first.x, left.right() + 9.0, "then the gap and the middle strips");
        assert!(right.x > at(&a, 3).x && right.right() > 700.0, "right-docked at the right end: {right:?}");
        // Scrolling moves the middle strips only.
        a.state.mixer_scroll = 2.0 * STRIP_W;
        a.paint(&mut c, None);
        assert_eq!((at(&a, 2), at(&a, 4)), (left, right));
        assert_eq!(at(&a, 5).x, first.x, "Bus 5 where Bus 1 was");
    }

    /// The slot menu's Presets section works as in the plugin's window menu: the list, Browse presets and Save preset
    /// as... on the slot's plugin (§7.2).
    #[test]
    fn slot_menu_presets_act_on_the_slot() {
        let mut app = App::new(None);
        app.state.selected_mixer = 2;
        let key = crate::plugins::fx_key(2, 3);
        let m = slot_menu(&app, 3, 0.0, 0.0);
        let cmd = |label: &str| m.items.iter().find(|i| i.label == label && !i.head).map(|i| (i.enabled, i.cmd.clone())).unwrap();
        assert_eq!(cmd("Browse presets"), (true, Cmd::Preset(crate::presets::PCmd::Browse(key))));
        assert_eq!(cmd("Save preset as..."), (true, Cmd::Preset(crate::presets::PCmd::SaveAs(key))));
        let p = m.items.iter().find(|i| i.label == "Presets" && !i.head).unwrap();
        assert!(p.enabled, "a submenu of the plugin's presets (empty here: no plugin is hosted)");
    }

    fn app() -> App {
        let mut app = App::new(None);
        app.state.session.project = crate::locked::Locked::new(crate::projects::test_project());
        app
    }

    fn asio(inputs: u16, outputs: u16) -> crate::device::DeviceInfo {
        crate::device::DeviceInfo {
            host: crate::device::ASIO,
            name: "Card".into(),
            rate: 48000,
            fixed: Some(256),
            buffers: None,
            channels: outputs,
            inputs,
            out_names: (1..=outputs).map(|c| format!("Out {c}")).collect(),
            in_names: vec!["Mic".into(), "Guitar".into(), "In 3".into()].into_iter().take(usize::from(inputs)).collect(),
            input_latency: None,
            rates: Vec::new(),
            sizes: Vec::new(),
            latency: None,
            instead: None,
        }
    }

    /// spec: audio/track-io.md §2.1: pairs first, then each input alone; outputs in pairs, else one entry.
    #[test]
    fn device_inputs_and_outputs_are_numbered_as_stored() {
        let d = asio(3, 5);
        let (pairs, monos) = input_choices(crate::device::InputSource::of_asio(&d).as_ref());
        assert_eq!(pairs, [(0, "Mic - Guitar".to_string())]);
        assert_eq!(monos, [(1, "Mic".to_string()), (2, "Guitar".to_string()), (3, "In 3".to_string())]);
        assert_eq!(output_choices(Some(&d)), [(0, "Out 1 - Out 2".to_string()), (1, "Out 3 - Out 4".to_string())], "a last odd output is not offered");
        assert_eq!(output_choices(Some(&asio(0, 1))), [(0, "Out 1".to_string())]);
        let wasapi = crate::device::DeviceInfo { host: "WASAPI", name: "Speakers".into(), ..asio(0, 2) };
        assert_eq!(crate::device::InputSource::of_asio(&wasapi), None, "an ASIO driver's inputs only under ASIO");
        let mic = crate::device::InputSource::of_system("Microphone", 2);
        assert_eq!(input_choices(Some(&mic)), (vec![(0, "Left - Right".to_string())], vec![(1, "Left".to_string()), (2, "Right".to_string())]), "the system's input device");
        assert_eq!(input_choices(Some(&crate::device::InputSource::of_system("Headset", 1))), (vec![], vec![(0, "In 1".to_string())]));
        assert_eq!(output_choices(Some(&wasapi)), [(0, "Speakers".to_string())]);
        assert_eq!(choice_label(-1, &pairs), "(none)");
        assert_eq!(choice_label(4, &monos), "(not available)");
    }

    /// spec: audio/track-io.md §2.6: the latency menus; values typed in ms, samples or beats; one undo step each.
    #[test]
    fn latency_settings_are_typed_in_and_undo() {
        use crate::typein::{LatUnit, Target};
        use crate::typein_ui::parse;
        use crate::typein_ui::accept;
        let mut a = app();
        a.state.set_tempo(120.0);
        // The app's tick gives the tracks their ids, which the undo steps name them by.
        a.state.session.control.settle(a.state.session.project.get_mut());
        let labels = |p: &Popup| p.items.iter().map(|i| (i.label.to_string(), i.enabled)).collect::<Vec<_>>();
        let out = labels(&latency_menu(&a.state.session, 2, false, 0.0, 0.0));
        assert_eq!(out[0].0, "Track latency: (none)");
        assert_eq!(out[1], ("Reset".to_string(), false), "nothing to reset");
        assert!(out[2..5].iter().all(|(_, on)| *on));
        assert!(out.iter().any(|(l, on)| l == "Delay audio output" && !on));
        let input = labels(&latency_menu(&a.state.session, 2, true, 0.0, 0.0));
        assert_eq!(input[0].0, "Input latency: (none)");
        assert_eq!(input.last().unwrap().0, "Adjust position of recorded audio");
        // ms, beats (500 ms at 120 BPM), with or without the unit; text that is no number changes nothing.
        let t = Target::Latency(2, false, LatUnit::Ms);
        assert_eq!(parse(t, false, "12.5 ms"), Some(12.5));
        assert_eq!(parse(t, false, "12.5%"), None);
        accept(&mut a.state, t, false, "12.5");
        assert_eq!(track(&a.state.session, 2).unwrap().latency_out, 12.5);
        assert_eq!(crate::typein_ui::text(&a.state, Target::Latency(2, false, LatUnit::Beats), false), "0.025");
        accept(&mut a.state, Target::Latency(2, true, LatUnit::Beats), false, "0.5 beats");
        assert_eq!(track(&a.state.session, 2).unwrap().latency_in, 250.0);
        accept(&mut a.state, Target::Latency(2, true, LatUnit::Beats), false, "x");
        accept(&mut a.state, t, false, "5000");
        assert_eq!(track(&a.state.session, 2).unwrap().latency_out, 1000.0, "kept within a second");
        assert!(labels(&latency_menu(&a.state.session, 2, false, 0.0, 0.0))[1].1, "Reset");
        run(&mut a, MCmd::Latency(2, false, None));
        assert_eq!(track(&a.state.session, 2).unwrap().latency_out, 0.0);
        assert_eq!(latency_text(250.0), "250 ms");
        assert_eq!(latency_text(-1.25), "-1.25 ms");
        // Each change is one step, undone in turn.
        crate::history_ui::undo(&mut a.state);
        assert_eq!(track(&a.state.session, 2).unwrap().latency_out, 1000.0);
        crate::history_ui::undo(&mut a.state);
        crate::history_ui::undo(&mut a.state);
        assert_eq!(track(&a.state.session, 2).unwrap().latency_in, 0.0);
        assert_eq!(track(&a.state.session, 2).unwrap().latency_out, 12.5);
    }

    /// spec: audio/track-io.md §1.3, §1.4, §2.3: the record menu; choosing an input asks once; the edits undo, arming
    /// is no edit.
    #[test]
    fn choosing_an_input_asks_to_record_it_alone() {
        use crate::dialog::{Body, DHit};
        let mut a = app();
        let menu = record_menu(&a.state, 0.0, 0.0);
        let marked: Vec<&str> = menu.items.iter().filter(|i| i.mark == Mark::Radio(true)).map(|i| i.label.as_ref()).collect();
        assert_eq!(marked, ["Post EQ", "When armed"]);
        run(&mut a, MCmd::Input(1, 0));
        assert_eq!(track(&a.state.session, 1).unwrap().audio_in, 0);
        assert!(matches!(a.state.dialogs.last().map(|d| &d.body), Some(Body::Msg(m)) if m.title == "External input"));
        a.dialog_click(DHit::Button(0));
        assert_eq!(track(&a.state.session, 1).unwrap().rec_source, os_model::REC_EXTERNAL, "Yes");
        // Another input, or one chosen while the source is External input only: no question.
        run(&mut a, MCmd::Input(1, 2));
        assert!(a.state.dialogs.is_empty());
        // No, remembered: the next track is not asked and keeps its source.
        run(&mut a, MCmd::Input(2, 1));
        a.dialog_click(DHit::Remember);
        a.dialog_click(DHit::Button(1));
        assert_eq!(crate::prefs::choice(&crate::prefs::EXTERNAL_INPUT), "Keep the record source");
        run(&mut a, MCmd::Input(3, 1));
        assert!(a.state.dialogs.is_empty());
        assert_eq!(track(&a.state.session, 3).unwrap().rec_source, os_model::REC_POST_EQ);
        crate::prefs::set_choice(&crate::prefs::EXTERNAL_INPUT, 0);
        // The edits undo; arming is kept through them.
        hit(&mut a, MHit::Arm(3));
        run(&mut a, MCmd::Monitor(3, os_model::MONITOR_ON));
        run(&mut a, MCmd::Output(3, 1));
        crate::history_ui::undo(&mut a.state);
        crate::history_ui::undo(&mut a.state);
        let m = track(&a.state.session, 3).unwrap();
        assert_eq!((m.audio_out, m.monitor, m.armed), (-1, os_model::MONITOR_ARMED, true));
        assert!(m.monitored(), "armed with an input");
        crate::history_ui::undo(&mut a.state);
        assert!(!track(&a.state.session, 3).unwrap().monitored(), "no input");
    }

    #[test]
    fn select_mute_solo() {
        let mut a = app();
        let n = inserts(&a.state.session);
        assert_eq!(n, 16, "default mixer shows Bus 1-16");
        hit(&mut a, MHit::Strip(3));
        assert_eq!(selected(&a.state), vec![3]);
        a.ctrl = true;
        hit(&mut a, MHit::Strip(5));
        a.ctrl = false;
        assert_eq!(selected(&a.state), vec![5, 3]);
        hit(&mut a, MHit::Strip(2));
        assert_eq!(selected(&a.state), vec![2]);
        hit(&mut a, MHit::Led(4));
        assert!(!track(&a.state.session, 4).unwrap().enabled);
        hit(&mut a, MHit::Led(4));
        assert!(track(&a.state.session, 4).unwrap().enabled);
        right(&mut a, MHit::Led(6));
        assert!(a.state.session.project.mixer.iter().all(|m| m.enabled == (m.index == 0 || m.index == 6)), "solo");
        right(&mut a, MHit::Led(6));
        assert!(a.state.session.project.mixer.iter().all(|m| m.enabled), "unsolo");
        a.shift = true;
        hit(&mut a, MHit::Strip(7));
        a.shift = false;
        assert_eq!(a.state.playlist.edit.as_ref().map(|e| e.target), Some(EditTarget::MixerName(7)));
        a.state.playlist.edit.as_mut().unwrap().text = "Drums".into();
        crate::playlist::commit_edit(&mut a.state);
        assert_eq!(name(&a.state.session, 7), "Drums");
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
        a.state.session.project.get_mut().channels[0].mixer_track = 0;
        a.select_window_track(0);
        assert_eq!(a.state.selected_mixer, 0, "a channel on the master selects the master");
        a.select_window_track(crate::plugins::fx_key(5, 2));
        assert_eq!(a.state.selected_mixer, 5, "an effect selects its slot's track");
        a.state.session.project.get_mut().channels[1].mixer_track = 40;
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
        a.state.session.project.get_mut().channels[4].mixer_track = 16;
        a.select_window_track(4);
        assert_eq!(a.state.selected_mixer, 16);
        assert!((a.state.mixer_scroll + view - 16.0 * w).abs() < 0.5, "Bus 16 at the right edge: {}", a.state.mixer_scroll);
        a.select_window_track(2);
        assert_eq!((a.state.selected_mixer, a.state.mixer_scroll), (3, 2.0 * w), "Bus 3 at the left edge");
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
        assert_eq!(track(&a.state.session, 1).unwrap().routes, vec![0, 5]);
        assert_eq!(send(&a.state.session, 1, 5), Some(0.8));
        a.ctrl = true;
        hit(&mut a, MHit::Route(6));
        a.ctrl = false;
        assert_eq!(track(&a.state.session, 1).unwrap().routes, vec![6], "route only");
        a.shift = true;
        hit(&mut a, MHit::Route(7));
        a.shift = false;
        assert_eq!(send(&a.state.session, 1, 7), Some(0.0), "sidechain");
        hit(&mut a, MHit::Unroute(7));
        assert_eq!(send(&a.state.session, 1, 7), None);
        assert!(receives(&a.state.session, 6));
        // 6 -> 1 would loop.
        select(&mut a.state, 6, false);
        assert!(hit(&mut a, MHit::Route(1)).is_some_and(|m| m.contains("loop")));
        assert!(!track(&a.state.session, 6).unwrap().routes.contains(&1));
        assert_eq!(level_text("Volume", 0.8), "Volume: 100%  0.0dB  1.00");
        assert_eq!(level_text("To Bus 11", 0.0), "To Bus 11: 0%  -INFdB  0.00");
    }

    #[test]
    fn undoing_a_track_delete_puts_back_its_slots_colours_and_icons() {
        use crate::playlist::IconOwner;
        let mut a = app();
        let red = Rgb { r: 200, g: 0, b: 0 };
        crate::edit::mixer::set_slot_color(&mut a.state.session, 3, 0, Some(red));
        crate::edit::playlist::put_icon(&mut a.state.session, IconOwner::Track(3), Some(crate::icons::Icon::Star));
        run(&mut a, MCmd::Delete(2));
        assert_eq!(crate::mixer::slot_colors_of(&a.state.session.project).get(&(2, 0)), Some(&red), "the colour follows its track left");
        crate::history_ui::undo(&mut a.state);
        assert_eq!(crate::mixer::slot_colors_of(&a.state.session.project).get(&(3, 0)), Some(&red));
        assert_eq!(crate::mixer::slot_colors_of(&a.state.session.project).get(&(2, 0)), None);
        assert_eq!(crate::playlist::icons(&a.state.session.project).get(&IconOwner::Track(3)), Some(&crate::icons::Icon::Star));
        assert_eq!(crate::playlist::icons(&a.state.session.project).get(&IconOwner::Track(2)), None);
    }

    #[test]
    fn add_move_delete_tracks() {
        let mut a = app();
        let len = a.state.session.project.mixer.len();
        hit(&mut a, MHit::Add);
        assert_eq!(a.state.session.project.mixer.len(), len + 1);
        assert_eq!(a.state.selected_mixer, 17);
        track_mut(&mut a.state.session, 2).unwrap().name = Some("Two".into());
        a.state.session.project.get_mut().channels[0].mixer_track = 2;
        run(&mut a, MCmd::MoveRight(2));
        assert_eq!(name(&a.state.session, 3), "Two");
        assert_eq!(a.state.session.project.channels[0].mixer_track, 3, "channels follow their track");
        run(&mut a, MCmd::Delete(1));
        assert_eq!(name(&a.state.session, 2), "Two", "later tracks move left");
        assert_eq!(a.state.session.project.channels[0].mixer_track, 2);
        assert_eq!(a.state.session.project.mixer.len(), len + 1, "a fresh track fills the end");
        run(&mut a, MCmd::InsertOne(2));
        assert_eq!(name(&a.state.session, 3), "Two");
        assert!(a.state.session.project.mixer.iter().enumerate().all(|(i, m)| usize::from(m.index) == i), "indices stay in order");
    }

    #[test]
    fn slot_commands_without_plugins() {
        let mut a = app();
        a.state.session.plugins.effects = vec![crate::plugin_db::fake("Test FX", os_host::Kind::Effect)];
        select(&mut a.state, 2, false);
        run(&mut a, MCmd::AddEffect(0, 0));
        assert_eq!(slot_name(&a.state.session, 2, 0).as_deref(), Some("Test FX"));
        run(&mut a, MCmd::SlotMove(0, 1));
        assert_eq!(slot_name(&a.state.session, 2, 1).as_deref(), Some("Test FX"));
        hit(&mut a, MHit::SlotLed(1));
        assert!(!track(&a.state.session, 2).unwrap().slot_on[1]);
        run(&mut a, MCmd::SlotDelete(1));
        assert_eq!(slot_name(&a.state.session, 2, 1), None);
        assert!(a.state.session.fx_requests.iter().any(|r| matches!(r, FxReq::Create { track: 2, slot: 0, index: 0, .. })));
    }

    /// spec: audio/automation.md "Targets follow moves": an automation clip, a recorded pattern event, an initial value,
    /// a controller link (and its controller in a slot) and the remote control links on a slot's or a track's settings
    /// follow the slot moved, the track moved, tracks inserted before it and a track deleted before it; the undo of each
    /// puts them back, and redo moves them again. A deleted track's targets go.
    #[test]
    fn automation_follows_moved_slots_and_tracks() {
        use crate::remote::Link;
        use crate::typein::Target as T;
        use os_model::target::{self, MIXER_PAN, MIXER_VOLUME, SLOT_MIX};
        let mut a = app();
        a.state.session.plugins.effects = vec![crate::plugin_db::fake("Test FX", os_host::Kind::Effect)];
        select(&mut a.state, 3, false);
        run(&mut a, MCmd::AddEffect(0, 0));
        crate::playlist::automation_channel(&mut a.state, target::slot_param(3, 0, 5), "FX", Vec::new());
        let p = a.state.session.project.get_mut();
        p.automations.last_mut().unwrap().targets.push(target::mixer(4, MIXER_VOLUME));
        p.patterns[0].controls.push(os_model::ControlPoint { id: os_model::Id::NONE, position: 0, kind: target::send(4, 5), value: 0 });
        p.init_controls.push(os_model::InitControl { head: 0, target: target::slot(3, 0, SLOT_MIX), value: 1 });
        p.controller_links.push(os_model::ControllerLink::new(0x7000 + 3 * 64 + 1, 0, target::mixer(4, MIXER_PAN)));
        crate::remote::set_links(&crate::settings_file::store(), &[
            Link::new(0, 20, T::Param(crate::plugins::fx_key(3, 0), 5)),
            Link::new(0, 21, T::Fader(4)),
            Link::new(0, 22, T::Knob(KnobId::SlotMix(3, 1))),
            Link::new(0, 23, T::Knob(KnobId::Send(4, 5))),
        ]);
        a.state.session.history.reset();
        // (clip targets, pattern event, initial value, controller link's source and target, remote links' targets)
        type Places = (Vec<u32>, Vec<u32>, Vec<u32>, Vec<(u16, u32)>, Vec<T>);
        let places = |a: &App| -> Places {
            let p = &a.state.session.project;
            (
                p.automations.last().unwrap().targets.clone(),
                p.patterns[0].controls.iter().map(|c| c.kind).collect(),
                p.init_controls.iter().map(|c| c.target).collect(),
                p.controller_links.iter().map(|l| (l.source(), l.target())).collect(),
                crate::remote::links(&crate::settings_file::store()).iter().map(|l| l.target).collect(),
            )
        };
        // Where everything is with tracks 3 and 4 (and 5, the send's destination) at `t3`, `t4`, `t5`, and track 3's
        // slots 0 and 1 at `k0`, `k1`.
        let at = |t3: u16, t4: u16, t5: u16, k0: u8, k1: u8| -> Places {
            (
                vec![target::slot_param(t3, k0, 5), target::mixer(t4, MIXER_VOLUME)],
                vec![target::send(t4, t5)],
                vec![target::slot(t3, k0, SLOT_MIX)],
                vec![(0x7000 + t3 * 64 + u16::from(k1), target::mixer(t4, MIXER_PAN))],
                vec![T::Param(crate::plugins::fx_key(t3, k0), 5), T::Fader(t4), T::Knob(KnobId::SlotMix(t3, k1)), T::Knob(KnobId::Send(t4, t5))],
            )
        };
        let start = at(3, 4, 5, 0, 1);
        assert_eq!(places(&a), start);

        // Move effect: slots 0 and 1 of track 3 swap.
        run(&mut a, MCmd::SlotMove(0, 1));
        assert_eq!(slot_name(&a.state.session, 3, 1).as_deref(), Some("Test FX"));
        assert_eq!(places(&a), at(3, 4, 5, 1, 0));
        crate::history_ui::undo(&mut a.state);
        assert_eq!(places(&a), start, "undo puts them back");
        crate::history_ui::redo(&mut a.state);
        assert_eq!(places(&a), at(3, 4, 5, 1, 0), "redo moves them again");
        crate::history_ui::undo(&mut a.state);

        // Move right: tracks 3 and 4 swap.
        run(&mut a, MCmd::MoveRight(3));
        assert_eq!(places(&a), at(4, 3, 5, 0, 1));
        crate::history_ui::undo(&mut a.state);
        assert_eq!(places(&a), start);

        // Insert multiple... before track 2: three tracks, one step.
        insert_many(&mut a.state, 2, "3");
        assert_eq!(places(&a), at(6, 7, 8, 0, 1));
        crate::history_ui::undo(&mut a.state);
        assert_eq!(places(&a), start, "one step puts back three moves");

        // Delete track 2: the later ones move one left.
        run(&mut a, MCmd::Delete(2));
        assert_eq!(places(&a), at(2, 3, 4, 0, 1));
        crate::history_ui::undo(&mut a.state);
        assert_eq!(places(&a), start);

        // Delete track 4: its targets go; the clip stays with track 3's.
        run(&mut a, MCmd::Delete(4));
        let (clip, events, init, ctl, links) = places(&a);
        assert_eq!(clip, [target::slot_param(3, 0, 5)]);
        assert_eq!((events, init, ctl), (vec![], vec![target::slot(3, 0, SLOT_MIX)], vec![]));
        assert_eq!(links, [T::Param(crate::plugins::fx_key(3, 0), 5), T::Knob(KnobId::SlotMix(3, 1))]);
        crate::history_ui::undo(&mut a.state);
        let (clip, events, init, ctl, _) = places(&a);
        assert_eq!((clip, events, init, ctl), (start.0, start.1, start.2, start.3), "the project's come back");
        crate::remote::set_links(&crate::settings_file::store(), &[]);
    }

    /// spec: audio/sidechain.md "Hosted plugins' input buses": Sidechain input ▸ lists None and the tracks routed into
    /// the track, in track order, and writes the pick into the slot's wrapper state as FL does; an effect that is not
    /// a wrapped plugin says so.
    #[test]
    fn sidechain_input_submenu() {
        use os_flp::wrapper;
        let mut a = app();
        select(&mut a.state, 1, false);
        track_mut(&mut a.state.session, 3).unwrap().routes = vec![1];
        track_mut(&mut a.state.session, 2).unwrap().routes = vec![1, 0];
        track_mut(&mut a.state.session, 2).unwrap().name = Some("Kick".into());
        assert_eq!(inputs(&a.state.session, 1), [2, 3]);
        let plugin = os_model::PluginRef { format: "clap".into(), name: "Comp".into(), path: "C:/Comp.clap".into(), ..Default::default() };
        track_mut(&mut a.state.session, 1).unwrap().slots[0] = Some(os_model::SlotPlugin { id: os_model::Id::NONE, name: wrapper::WRAPPER.into(), state: wrapper::encode(None, &plugin) });
        let labels = |a: &App| sidechain_items(&a.state, 0).iter().map(|i| (i.label.to_string(), i.mark == Mark::Radio(true))).collect::<Vec<_>>();
        assert_eq!(labels(&a), [("None".into(), true), ("1. Kick".into(), false), ("2. Bus 3".into(), false)]);
        assert_eq!(run(&mut a, MCmd::SlotSidechain(0, 2)), None);
        let state = |a: &App| track(&a.state.session, 1).unwrap().slots[0].as_ref().unwrap().state.clone();
        assert_eq!(wrapper::input_ports(&state(&a)), [0, 2]);
        assert_eq!(wrapper::decode(&state(&a)), Some(plugin));
        assert_eq!(labels(&a)[2].1, true);
        assert_eq!(crate::effect_ports::slot_ports(&a.state.session.project, 1, 0), (0, 2));
        track_mut(&mut a.state.session, 1).unwrap().slots[1] = Some(os_model::SlotPlugin { id: os_model::Id::NONE, name: "Fruity Limiter".into(), state: vec![] });
        assert!(run(&mut a, MCmd::SlotSidechain(1, 1)).is_some());
    }

    // spec: ui/mixer-track-commands.md §1
    #[test]
    fn route_to_free_track() {
        let mut a = app();
        // Kick 1, Clap 2, Hat 3, Snare 4, Pluck 5; Snare joins Kick on 1, Pluck uses the master.
        a.state.session.project.get_mut().channels[3].mixer_track = 1;
        a.state.session.project.get_mut().channels[4].mixer_track = 0;
        a.state.session.project.get_mut().channels[2].color = Some(Rgb::new(10, 20, 30));
        crate::rack::run(&mut a, crate::rack::RCmd::AssignFree(2));
        assert_eq!(a.state.session.project.channels[2].mixer_track, 3, "alone on its track: stays");
        assert_eq!(name(&a.state.session, 3), "Hat");
        assert_eq!(track(&a.state.session, 3).unwrap().color, Some(Rgb::new(10, 20, 30)));
        assert_eq!(a.state.selected_mixer, 3);
        crate::rack::run(&mut a, crate::rack::RCmd::AssignFree(3));
        assert_eq!(a.state.session.project.channels[3].mixer_track, 4, "shared: the lowest free insert");
        assert_eq!(name(&a.state.session, 4), "Snare");
        crate::rack::run(&mut a, crate::rack::RCmd::AssignFree(4));
        assert_eq!(a.state.session.project.channels[4].mixer_track, 5, "on the master: a free insert");
        assert_eq!(a.state.selected_mixer, 5);
        // No free insert left: a new track at the end.
        for (i, c) in a.state.session.project.get_mut().channels.iter_mut().enumerate() {
            c.mixer_track = if i == 0 { 1 } else { 1 + i as u16 };
        }
        // Channels on every insert, one more sharing Insert 1.
        let more: Vec<_> = (6..=16).chain([1]).map(|t| os_model::Channel { mixer_track: t, ..a.state.session.project.channels[0].clone() }).collect();
        a.state.session.project.get_mut().channels.extend(more);
        let len = a.state.session.project.mixer.len();
        crate::rack::run(&mut a, crate::rack::RCmd::AssignFree(16));
        assert_eq!(a.state.session.project.mixer.len(), len + 1);
        assert_eq!(a.state.session.project.channels[16].mixer_track, 17);
    }

    // spec: ui/mixer-track-commands.md §2
    #[test]
    fn channel_routing() {
        let mut a = app();
        a.state.rack.sel = [0, 2].into_iter().collect();
        run(&mut a, MCmd::RouteChannels { dst: 14, spread: false });
        assert_eq!(a.state.session.project.channels[0].mixer_track, 14);
        assert_eq!(a.state.session.project.channels[2].mixer_track, 14);
        assert_eq!(name(&a.state.session, 14), "Kick", "named after the first channel");
        assert_eq!(name(&a.state.session, 3), "Bus 3", "the track left keeps its name");
        assert_eq!(a.state.selected_mixer, 14);
        a.state.rack.sel = [1, 3, 4].into_iter().collect();
        run(&mut a, MCmd::RouteChannels { dst: 15, spread: true });
        let tracks: Vec<u16> = a.state.session.project.channels.iter().map(|c| c.mixer_track).collect();
        assert_eq!(tracks, vec![14, 15, 14, 16, 5], "past the last insert the channel stays");
        assert_eq!((name(&a.state.session, 15), name(&a.state.session, 16)), ("Clap".to_string(), "Snare".to_string()));
        // Nothing selected in the rack: the piano roll's channel.
        a.state.rack.sel.clear();
        a.state.selected_channel = 4;
        run(&mut a, MCmd::RouteChannels { dst: 7, spread: false });
        assert_eq!(a.state.session.project.channels[4].mixer_track, 7);
    }

    // spec: ui/mixer-track-commands.md §3
    #[test]
    fn select_by_routing() {
        let mut a = app();
        a.state.session.project.get_mut().channels[3].mixer_track = 2;
        run(&mut a, MCmd::SelectChannels(2));
        assert_eq!(a.state.rack.sel.iter().copied().collect::<Vec<_>>(), vec![1, 3]);
        track_mut(&mut a.state.session, 5).unwrap().routes = vec![3];
        track_mut(&mut a.state.session, 6).unwrap().routes = vec![3];
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
        a.state.session.project.get_mut().channels[3].mixer_track = 5;
        a.state.session.project.get_mut().channels[4].mixer_track = 9;
        track_mut(&mut a.state.session, 9).unwrap().name = Some("Nine".into());
        // 12 holds an effect, 11 feeds nothing but 9 sends to 13.
        track_mut(&mut a.state.session, 12).unwrap().slots[0] = Some(os_model::SlotPlugin { id: os_model::Id::NONE, name: "FX".into(), state: Vec::new() });
        track_mut(&mut a.state.session, 9).unwrap().routes = vec![13];
        assert_eq!(trim_list(&a.state.session, false), vec![4, 6, 7, 8, 10, 11, 14, 15, 16]);
        assert_eq!(trim_list(&a.state.session, true), vec![4, 6, 7, 8, 10, 11, 12, 14, 15, 16]);
        let len = a.state.session.project.mixer.len();
        run(&mut a, MCmd::Trim { non_empty: true });
        assert_eq!(a.state.session.project.mixer.len(), len - 10);
        assert!(a.state.session.project.mixer.iter().enumerate().all(|(i, m)| usize::from(m.index) == i));
        let tracks: Vec<u16> = a.state.session.project.channels.iter().map(|c| c.mixer_track).collect();
        assert_eq!(tracks, vec![1, 2, 3, 4, 5]);
        assert_eq!(name(&a.state.session, 5), "Nine");
        assert_eq!(track(&a.state.session, 5).unwrap().routes, vec![6], "its send follows 13 to 6");
        assert!(a.state.session.project.mixer.last().unwrap().routes.is_empty(), "the hidden last track stays");
        assert!(trim_list(&a.state.session, true).is_empty());
        // Nothing uses any insert: one stays.
        for c in a.state.session.project.get_mut().channels.iter_mut() {
            c.mixer_track = 0;
        }
        track_mut(&mut a.state.session, 5).unwrap().routes = vec![0];
        assert_eq!(trim_list(&a.state.session, true), vec![2, 3, 4, 5, 6]);
    }

    // spec: ui/mixer-track-commands.md §5
    #[test]
    fn solo_and_alt_solo() {
        let mut a = app();
        track_mut(&mut a.state.session, 5).unwrap().routes = vec![3];
        let on = |a: &App| a.state.session.project.mixer.iter().filter(|m| m.enabled).map(|m| m.index).collect::<Vec<_>>();
        run(&mut a, MCmd::Solo(3));
        assert_eq!(on(&a), vec![0, 3], "Solo mutes the track sending into 3");
        run(&mut a, MCmd::Solo(3));
        assert_eq!(on(&a).len(), a.state.session.project.mixer.len());
        run(&mut a, MCmd::AltSolo(3));
        assert_eq!(on(&a), vec![0, 3, 5]);
        run(&mut a, MCmd::AltSolo(3));
        assert_eq!(on(&a).len(), a.state.session.project.mixer.len());
        run(&mut a, MCmd::Solo(5));
        assert_eq!(on(&a), vec![0, 3, 5], "Solo keeps where 5 sends");
    }

    #[test]
    fn mixer_keys() {
        use crate::keys::Key;
        let mut a = app();
        select(&mut a.state, 10, false);
        a.state.rack.sel = [1].into_iter().collect();
        assert!(key(&mut a, &Key::Character("l".into()), true, false));
        assert_eq!(a.state.session.project.channels[1].mixer_track, 10, "Ctrl+L");
        assert!(key(&mut a, &Key::Character("a".into()), true, false));
        assert_eq!(a.state.mixer.sel.len(), 17, "Ctrl+A");
        select(&mut a.state, 4, false);
        assert!(key(&mut a, &Key::Character("s".into()), false, true));
        assert!(!track(&a.state.session, 7).unwrap().enabled, "Alt+S");
        assert!(!key(&mut a, &Key::Character("q".into()), true, false));
    }

    /// Moved and deleted tracks take their effects to their new numbers through saves (spec: flp/events/mixer.md
    /// "Removing tracks"): each slot's window record (event 212), marked here in the saved file with the track it was
    /// saved on, follows its track, and the slots read back where the tracks went.
    #[test]
    fn moved_tracks_keep_their_effects_in_the_file() {
        use os_flp::{ChunkBody, FlpFile, Value};
        use os_model::SlotPlugin;
        let mut a = app();
        (a.state.session.file, a.state.session.path) = (None, None);
        let fx = |name: &str, s: u8| Some(SlotPlugin { id: os_model::Id::NONE, name: name.into(), state: vec![s; 4] });
        track_mut(&mut a.state.session, 2).unwrap().slots[0] = fx("Test FX", 2);
        track_mut(&mut a.state.session, 3).unwrap().slots[0] = fx("Test FX", 3);
        track_mut(&mut a.state.session, 4).unwrap().slots[1] = fx("Other FX", 4);
        let path = std::env::temp_dir().join(format!("lemon-mixer-moves-{}.flp", std::process::id()));
        let saved = |a: &mut App| {
            a.save_to(path.clone());
            assert!(a.state.message.as_deref().is_some_and(|m| m.starts_with("Saved")), "{:?}", a.state.message);
        };
        // The mixer's slot records (after the first track record, event 236): track, slot and mark (byte 40, which
        // Lemon Studio does not read).
        let records = || {
            let file = FlpFile::parse(&std::fs::read(&path).unwrap()).unwrap();
            let ev = file.chunks.into_iter().find_map(|c| match c.body {
                ChunkBody::Events(e) => Some(e),
                _ => None,
            });
            let ev = ev.unwrap();
            let first = ev.iter().position(|e| e.id == 236).unwrap();
            let le = |d: &[u8], at: usize| u32::from_le_bytes(d[at..at + 4].try_into().unwrap());
            ev[first..].iter().filter(|e| e.id == 212).filter_map(|e| e.data()).map(|d| (le(d, 0), le(d, 4), d[40])).collect::<Vec<_>>()
        };
        saved(&mut a);
        let mut file = FlpFile::parse(&std::fs::read(&path).unwrap()).unwrap();
        for c in &mut file.chunks {
            if let ChunkBody::Events(ev) = &mut c.body {
                for e in ev.iter_mut().filter(|e| e.id == 212) {
                    if let Value::Var { data, .. } = &mut e.value {
                        data[40] = data[0];
                    }
                }
            }
        }
        std::fs::write(&path, file.to_bytes().unwrap()).unwrap();
        assert_eq!(records(), vec![(2, 0, 2), (3, 0, 3), (4, 1, 4)]);
        let mut a = App::new(None);
        a.open(path.clone());
        run(&mut a, MCmd::MoveRight(2));
        saved(&mut a);
        assert_eq!(records(), vec![(2, 0, 3), (3, 0, 2), (4, 1, 4)], "2 and 3 swap places with their effects");
        // Saved over again (the file holds the tracks in their new order now): deleting 2 moves the later ones left.
        run(&mut a, MCmd::Delete(2));
        saved(&mut a);
        assert_eq!(records(), vec![(2, 0, 2), (3, 1, 4)]);
        let mut opened = App::new(None);
        opened.open(path.clone());
        let _ = std::fs::remove_file(&path);
        let slots = |t: u16| track(&opened.state.session, t).unwrap().slots.clone();
        assert_eq!((slots(2)[0].clone(), slots(3)[1].clone()), (fx("Test FX", 2), fx("Other FX", 4)));
        assert!(slots(4).iter().all(Option::is_none));
    }

    /// Open audio editor and record: a Tesla set up as an audio logger (On play, 5 minutes, Append, slaved, armed). An
    /// empty Tesla on the track is used; one holding a sample is left alone for a new one (spec:
    /// plugins/tesla/README.md "Recording into Tesla from Lemon Studio").
    #[test]
    fn open_audio_editor_and_record_sets_up_a_logger() {
        use tesla::state::{from_plugin_state, plugin_state, Saved, WHEN_ON_PLAY};
        let mut a = app();
        a.state.session.plugins.effects = crate::plugins::builtin_effects();
        let logger = |b: &[u8]| from_plugin_state(b).map(|s| (s.rec.when, s.rec.max, s.rec.append, s.rec.slave, s.rec.armed));
        let want = Some((WHEN_ON_PLAY, 3, true, true, true));
        assert!(track_items(&a.state, 2).iter().any(|i| i.label == "Open audio editor and record" && i.enabled));
        run(&mut a, MCmd::AudioEditorRecord(2));
        let Some(FxReq::Create { track: 2, slot: 0, index, state: Some(st) }) = a.state.session.fx_requests.first().cloned() else {
            panic!("{:?}", a.state.session.fx_requests)
        };
        assert_eq!(logger(&st), want);
        // An empty Tesla already there (hosted with its defaults) is set up in place and shown.
        a.state.session.fx_requests.clear();
        a.state.session.plugins.create_effect(2, 0, index, None).unwrap();
        let key = crate::plugins::fx_key(2, 0);
        assert_eq!(logger(&a.state.session.plugins.state(key).unwrap()).map(|r| r.4), Some(false));
        run(&mut a, MCmd::AudioEditorRecord(2));
        assert_eq!(a.state.session.fx_requests, vec![FxReq::Show { track: 2, slot: 0 }]);
        assert_eq!(logger(&a.state.session.plugins.state(key).unwrap()), want);
        // One holding a sample: a new Tesla in the next free slot.
        a.state.session.fx_requests.clear();
        let sample = tesla::sample::Sample::from_channels(44100, vec![vec![0.5; 100]], tesla::sample::Format::Int16);
        assert!(a.state.session.plugins.set_state(key, &plugin_state(&Saved { sample, ..Saved::default() })));
        run(&mut a, MCmd::AudioEditorRecord(2));
        assert!(matches!(a.state.session.fx_requests.as_slice(), [FxReq::Create { track: 2, slot: 1, state: Some(_), .. }]));
        assert_eq!(from_plugin_state(&a.state.session.plugins.state(key).unwrap()).map(|s| s.sample.frames()), Some(100), "left alone");
    }

    /// Open audio editor adds a Tesla as one step that undo takes away, as FL's adds Edison ("insert effect plugin",
    /// measured on FL 24.1.1); showing the track's Tesla again is no step.
    #[test]
    fn an_added_audio_editor_is_an_undo_step() {
        let mut a = app();
        a.state.session.plugins.effects = crate::plugins::builtin_effects();
        let steps = a.state.session.history.at();
        run(&mut a, MCmd::AudioEditor(2));
        assert_eq!(teslas(&a.state.session), [(2, 0)]);
        assert_eq!(a.state.session.history.at(), steps + 1);
        run(&mut a, MCmd::AudioEditor(2));
        assert_eq!(a.state.session.history.at(), steps + 1, "shown again: no step");
        run(&mut a, MCmd::AudioEditorRecord(3));
        assert_eq!(a.state.session.history.at(), steps + 2);
        crate::history_ui::undo(&mut a.state);
        crate::history_ui::undo(&mut a.state);
        assert!(teslas(&a.state.session).is_empty());
    }

    /// Remove all (audio editor) instances, in Mixer options and TOOLS > Macros: every Tesla (a project's Edison slots
    /// too) goes after a question; with none, a message (spec: plugins/tesla/README.md "Recording into Tesla from
    /// Lemon Studio").
    #[test]
    fn remove_all_audio_editors_asks_first() {
        let mut a = app();
        a.state.session.plugins.effects = crate::plugins::builtin_effects();
        let label = "Remove all (audio editor) instances";
        assert!(options_menu(&a.state, 0.0, 0.0).items.iter().any(|i| i.label == label && i.enabled));
        assert_eq!(run(&mut a, MCmd::RemoveTeslas).as_deref(), Some("No audio editors in the mixer"));
        a.audio_editor(0, false);
        a.audio_editor(3, false);
        track_mut(&mut a.state.session, 4).unwrap().slots[2] = Some(os_model::SlotPlugin { id: os_model::Id::NONE, name: crate::plugins::EDISON_FILE_NAME.into(), state: Vec::new() });
        a.state.session.fx_requests.clear();
        assert_eq!(run(&mut a, MCmd::RemoveTeslas), None);
        let Some(Popup { items, .. }) = a.state.playlist.popup.clone() else { panic!("no question") };
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_ref()).collect();
        assert_eq!(labels, ["Remove 3 audio editors? Their audio is lost.", "Remove", "Cancel"]);
        assert_eq!(teslas(&a.state.session).len(), 3, "nothing goes before the answer");
        assert_eq!(run(&mut a, MCmd::RemoveTeslasNow).as_deref(), Some("Removed 3 audio editors"));
        assert!(teslas(&a.state.session).is_empty());
        assert_eq!(a.state.session.fx_requests.iter().filter(|r| matches!(r, FxReq::Delete { .. })).count(), 3);
        use crate::views::menus::{item_at, REMOVE_TESLAS_LABEL, TOOLS_MENU};
        let i = (0..200).find(|&i| item_at(TOOLS_MENU, i).is_some_and(|it| it.label == REMOVE_TESLAS_LABEL)).expect("in TOOLS");
        a.menu_item(TOOLS_MENU, i);
        assert_eq!(a.state.message.as_deref(), Some("No audio editors in the mixer"));
    }

    /// Insert multiple... asks how many (a box opening on 2, selected) and inserts that many fresh tracks in front of
    /// the track, one undo step; text that is not a number inserts none.
    #[test]
    fn insert_multiple_tracks() {
        let mut a = app();
        let len = a.state.session.project.mixer.len();
        let it = track_items(&a.state, 3).into_iter().find(|i| i.label == "Insert multiple...").unwrap();
        assert_eq!((it.enabled, it.cmd), (true, Cmd::Mixer(MCmd::InsertMultiple(3))));
        track_mut(&mut a.state.session, 3).unwrap().name = Some("Three".into());
        a.state.session.project.get_mut().channels[0].mixer_track = 3;
        run(&mut a, MCmd::InsertMultiple(3));
        let e = a.state.playlist.edit.clone().expect("the box");
        assert_eq!((e.target, e.text.as_str(), e.sel), (EditTarget::MixerInsert(3), "2", true));
        a.state.playlist.edit.as_mut().unwrap().text = "3".into();
        let steps = a.state.session.history.at();
        crate::playlist::commit_edit(&mut a.state);
        assert_eq!(a.state.session.project.mixer.len(), len + 3);
        assert_eq!(name(&a.state.session, 6), "Three");
        assert_eq!(a.state.session.project.channels[0].mixer_track, 6, "channels follow their track");
        assert_eq!((name(&a.state.session, 3), a.state.selected_mixer), ("Bus 3".to_string(), 3));
        assert_eq!(a.state.message.as_deref(), Some("Inserted 3 mixer tracks"));
        assert_eq!((a.state.session.history.at(), a.state.session.history.newest_done()), (steps + 1, Some(INSERT)));
        crate::history_ui::undo(&mut a.state);
        assert_eq!((a.state.session.project.mixer.len(), name(&a.state.session, 3)), (len, "Three".to_string()));
        assert_eq!(insert_many(&mut a.state, 3, "lots").as_deref(), Some("Insert multiple: \"lots\" is not a number"));
        assert_eq!(a.state.session.project.mixer.len(), len);
        insert_many(&mut a.state, 1, "1000");
        assert_eq!(a.state.session.project.mixer.len(), len + usize::from(INSERT_MAX), "at most INSERT_MAX at once");
    }

    /// Gradient color...: greyed with one track selected; with several, their colours run from the first one's to the
    /// last one's in track order (a hue sweep when either has none), one undo step.
    #[test]
    fn gradient_color_over_the_selected_tracks() {
        let mut a = app();
        let entry = |a: &App, n: u16| track_items(&a.state, n).into_iter().find(|i| i.label == "Gradient color...").unwrap();
        select(&mut a.state, 2, false);
        assert!(!entry(&a, 2).enabled, "one track");
        select(&mut a.state, 6, true);
        select(&mut a.state, 4, true);
        assert_eq!((entry(&a, 4).enabled, entry(&a, 4).cmd), (true, Cmd::Mixer(MCmd::GradientColor)));
        assert!(!entry(&a, 9).enabled, "a track outside the selection");
        let (red, blue) = (Rgb::new(200, 0, 0), Rgb::new(0, 0, 100));
        track_mut(&mut a.state.session, 2).unwrap().color = Some(red);
        track_mut(&mut a.state.session, 6).unwrap().color = Some(blue);
        let steps = a.state.session.history.at();
        run(&mut a, MCmd::GradientColor);
        let col = |a: &App, n: u16| track(&a.state.session, n).unwrap().color;
        assert_eq!((col(&a, 2), col(&a, 4), col(&a, 6)), (Some(red), Some(Rgb::new(100, 0, 50)), Some(blue)));
        assert_eq!(col(&a, 3), None, "unselected tracks keep theirs");
        assert_eq!(a.state.session.history.at(), steps + 1);
        crate::history_ui::undo(&mut a.state);
        assert_eq!(col(&a, 4), None);
    }

    /// File ▸ Save mixer track state writes the track's state under its name; Open mixer track state ▸ puts it on another
    /// track (its old effects go, its name stays) and Insert from state ▸ inserts a track from it, each one undo step.
    #[test]
    fn mixer_track_states_save_open_and_insert() {
        let _ = std::fs::remove_dir_all(crate::mixer_state::dir().unwrap());
        let mut a = app();
        let file = |a: &App, n: u16| track_items(&a.state, n).into_iter().find(|i| i.label == "File").unwrap();
        let ins = |a: &App, n: u16| track_items(&a.state, n).into_iter().find(|i| i.label == "Insert from state").unwrap();
        assert_eq!((file(&a, 2).children[0].children[0].label.as_ref(), file(&a, 2).children[0].children[0].enabled), ("(no saved states)", false));
        assert!(!ins(&a, 2).children[0].enabled);
        let balance = os_engine::effects::balance::Settings { v: [0, 128] }.to_state();
        {
            let m = track_mut(&mut a.state.session, 2).unwrap();
            (m.name, m.color, m.volume) = (Some("Lead".into()), Some(Rgb::new(10, 20, 30)), Some(0.5));
            m.slots[1] = Some(os_model::SlotPlugin { id: os_model::Id::NONE, name: "Fruity Balance".into(), state: balance.clone() });
            m.slot_mix[1] = 0.6;
        }
        assert_eq!(run(&mut a, MCmd::SaveState(2)).as_deref(), Some("Saved the mixer track state \"Lead\""));
        let path = crate::mixer_state::list()[0].1.clone();
        assert_eq!(file(&a, 2).children[0].children[0].cmd, Cmd::Mixer(MCmd::OpenState(2, path.clone())));
        assert_eq!(ins(&a, 5).children[0].cmd, Cmd::Mixer(MCmd::InsertState(5, path.clone())));
        // Open on track 3: its own effect goes, its name stays.
        track_mut(&mut a.state.session, 3).unwrap().slots[0] = Some(os_model::SlotPlugin { id: os_model::Id::NONE, name: "Fruity Balance".into(), state: balance.clone() });
        let steps = a.state.session.history.at();
        a.state.session.fx_requests.clear();
        run(&mut a, MCmd::OpenState(3, path.clone()));
        let t3 = track(&a.state.session, 3).unwrap();
        assert_eq!((t3.name.as_deref(), t3.volume, t3.slots[0].is_none(), t3.slot_mix[1]), (None, Some(0.5), true, 0.6));
        assert_eq!(t3.slots[1].as_ref().map(|p| p.state.clone()), Some(balance.clone()));
        assert!(a.state.session.fx_requests.contains(&FxReq::Delete { track: 3, slot: 0 }));
        assert_eq!(a.state.session.history.at(), steps + 1);
        // Insert in front of track 5: a new track 5 named after the state; the old 5 is now 6.
        track_mut(&mut a.state.session, 5).unwrap().name = Some("Old five".into());
        run(&mut a, MCmd::InsertState(5, path));
        assert_eq!((name(&a.state.session, 5), name(&a.state.session, 6)), ("Lead".to_string(), "Old five".to_string()));
        assert_eq!(track(&a.state.session, 5).unwrap().color, Some(Rgb::new(10, 20, 30)));
        assert_eq!(a.state.session.history.at(), steps + 2);
        crate::history_ui::undo(&mut a.state);
        assert_eq!(name(&a.state.session, 5), "Old five");
        let _ = std::fs::remove_dir_all(crate::mixer_state::dir().unwrap());
    }

    /// File ▸ Render to wave file...: the Export form for the selected tracks, or the one clicked, named after it and
    /// with WAV ticked; its export writes those tracks' outputs (`export::Job::tracks`).
    #[test]
    fn render_to_wave_file_opens_the_form_for_the_tracks() {
        let mut a = app();
        a.state.session.project.get_mut().patterns[0].notes.push(os_model::Note::step(0, 0));
        a.state.set_song_mode(false);
        let file = track_items(&a.state, 2).into_iter().find(|i| i.label == "File").unwrap();
        assert_eq!(file.children[3].cmd, Cmd::Mixer(MCmd::RenderWave(2)));
        track_mut(&mut a.state.session, 2).unwrap().name = Some("Bass: sub".into());
        run(&mut a, MCmd::RenderWave(2));
        let form = |a: &App| match &a.state.dialogs.last().expect("the form").body {
            crate::dialog::Body::Project(crate::project_forms::Form::Export(f)) => (**f).clone(),
            _ => panic!("not the Export form"),
        };
        let f = form(&a);
        assert_eq!((f.tracks.clone(), f.name.text.as_str(), f.o.formats), (vec![2], "Bass_ sub", [true, false, false, false]));
        assert!(!f.check(2).1 && !f.check(3).1, "Split mixer tracks does not apply");
        let job = a.export_job(&f);
        assert_eq!((job.tracks, job.options.split), (vec![2], true));
        // The selection, when the track clicked is in it.
        a.state.dialogs.clear();
        select(&mut a.state, 2, false);
        select(&mut a.state, 4, true);
        run(&mut a, MCmd::RenderWave(4));
        let mut t = form(&a).tracks;
        t.sort_unstable();
        assert_eq!(t, [2, 4]);
    }

    /// Track routing ▸ (Lemon Studio's design): greyed until another track is selected; then the ▲ menu's four entries
    /// route or sidechain the other selected tracks into this one, one undo step each.
    #[test]
    fn track_routing_routes_the_selected_tracks_here() {
        let mut a = app();
        let entry = |a: &App, n: u16| track_items(&a.state, n).into_iter().find(|i| i.label == "Track routing").unwrap();
        select(&mut a.state, 5, false);
        assert!(!entry(&a, 5).enabled, "only this track selected");
        select(&mut a.state, 2, true);
        select(&mut a.state, 3, true);
        select(&mut a.state, 5, true);
        let e = entry(&a, 5);
        assert!(e.enabled);
        let labels: Vec<&str> = e.children.iter().filter(|i| !i.sep).map(|i| i.label.as_ref()).collect();
        assert_eq!(labels, ["Route selected tracks to this track", "Route selected tracks to this track only", "Sidechain selected tracks to this track", "Sidechain selected tracks to this track only"]);
        assert!(!entry(&a, 9).enabled, "a track outside the selection");
        let steps = a.state.session.history.at();
        crate::playlist::run(&mut a, e.children[0].cmd.clone());
        assert_eq!((send(&a.state.session, 2, 5), send(&a.state.session, 3, 5), send(&a.state.session, 2, 0)), (Some(0.8), Some(0.8), Some(0.8)));
        assert_eq!(a.state.session.history.at(), steps + 1);
        crate::playlist::run(&mut a, e.children[4].cmd.clone());
        assert_eq!((send(&a.state.session, 2, 5), send(&a.state.session, 2, 0)), (Some(0.0), None), "sidechained only: the master route goes");
        assert_eq!(send(&a.state.session, 5, 5), None, "this track does not route to itself");
    }

    /// The slot menu's Change color... and Random color (Lemon Studio's design): the slot gets a colour (Default clears
    /// it), which moves with the slot and goes with its plugin.
    #[test]
    fn slot_colors_follow_the_slot() {
        let mut a = app();
        a.state.session.plugins.effects = vec![crate::plugin_db::fake("Test FX", os_host::Kind::Effect)];
        select(&mut a.state, 2, false);
        run(&mut a, MCmd::AddEffect(0, 0));
        let m = slot_menu(&a, 0, 0.0, 0.0);
        let entry = |label: &str| m.items.iter().find(|i| i.label == label).unwrap().clone();
        let change = entry("Change color...");
        assert!(change.enabled && entry("Random color").enabled);
        assert_eq!(change.children[0].cmd, Cmd::Mixer(MCmd::SlotColor(0, None)));
        let red = Rgb::new(0xc8, 0x50, 0x4a);
        assert_eq!((change.children[1].label.as_ref(), &change.children[1].cmd), ("Red", &Cmd::Mixer(MCmd::SlotColor(0, Some(red)))));
        run(&mut a, MCmd::SlotColor(0, Some(red)));
        assert_eq!(slot_color(&a.state.session, 2, 0), Some(red));
        run(&mut a, MCmd::SlotMove(0, 1));
        assert_eq!((slot_color(&a.state.session, 2, 0), slot_color(&a.state.session, 2, 1)), (None, Some(red)), "moves with the slot");
        run(&mut a, MCmd::MoveRight(2));
        assert_eq!(slot_color(&a.state.session, 3, 1), Some(red), "and with its track");
        select(&mut a.state, 3, false);
        run(&mut a, MCmd::SlotColor(1, None));
        assert_eq!(slot_color(&a.state.session, 3, 1), None, "Default");
        run(&mut a, MCmd::SlotRandomColor(1));
        assert!(slot_color(&a.state.session, 3, 1).is_some());
        run(&mut a, MCmd::SlotDelete(1));
        assert_eq!(slot_color(&a.state.session, 3, 1), None, "goes with the plugin");
        assert!(crate::mixer::slot_colors_of(&a.state.session.project).is_empty());
    }

    /// Change icon... on the track menu and the slot menu (Lemon Studio's design, session only): the icon moves with its
    /// track and slot and goes with a deleted track; the slot menu's Link all parameters... opens the link box.
    #[test]
    fn icons_follow_tracks_and_slots() {
        use crate::icons::Icon;
        use crate::playlist::TRACK_ICON_KEY;
        let mut a = app();
        a.state.session.plugins.effects = vec![crate::plugin_db::fake("Test FX", os_host::Kind::Effect)];
        select(&mut a.state, 2, false);
        run(&mut a, MCmd::AddEffect(0, 0));
        let pick = |m: &Popup, label: &str| {
            let e = m.items.iter().find(|i| i.label == "Change icon...").unwrap();
            assert!(e.enabled);
            e.children.iter().find(|i| i.label == label).unwrap().cmd.clone()
        };
        let icon = |a: &App, key: usize| crate::playlist::window_icon(&a.state.session.project, key);
        let star = pick(&track_menu(&a.state, 2, 0.0, 0.0), "Star");
        crate::playlist::run(&mut a, star);
        let keys = pick(&slot_menu(&a, 0, 0.0, 0.0), "Keys");
        crate::playlist::run(&mut a, keys);
        assert_eq!((icon(&a, TRACK_ICON_KEY + 2), icon(&a, crate::plugins::fx_key(2, 0))), (Some(Icon::Star), Some(Icon::PianoRoll)));
        let link = slot_menu(&a, 0, 0.0, 0.0).items.into_iter().find(|i| i.label == "Link all parameters...").unwrap();
        assert_eq!((link.enabled, link.cmd), (true, Cmd::Remote(crate::remote::RCmd::LinkAll(crate::plugins::fx_key(2, 0)))));
        run(&mut a, MCmd::SlotMove(0, 1));
        assert_eq!((icon(&a, crate::plugins::fx_key(2, 0)), icon(&a, crate::plugins::fx_key(2, 1))), (None, Some(Icon::PianoRoll)), "moves with the slot");
        run(&mut a, MCmd::MoveRight(2));
        assert_eq!((icon(&a, TRACK_ICON_KEY + 2), icon(&a, TRACK_ICON_KEY + 3)), (None, Some(Icon::Star)), "and with its track");
        assert_eq!(icon(&a, crate::plugins::fx_key(3, 1)), Some(Icon::PianoRoll));
        run(&mut a, MCmd::Delete(3));
        assert!(crate::playlist::icons(&a.state.session.project).is_empty(), "a deleted track takes its icons");
    }

    /// The slot menu's CPU entries are the plugin's switches, as in its window menu, one undo step each (spec:
    /// audio/auto-bypass.md); Switch auto-bypass for all plugins (Options, TOOLS, the instrument rack's options menu)
    /// turns every plugin's Auto-bypass when silent on when one is off, and all of them off when all are on.
    #[test]
    fn auto_bypass_switches_per_slot_and_for_all() {
        let mut a = app();
        a.state.session.plugins.effects = vec![crate::plugin_db::fake("Test FX", os_host::Kind::Effect)];
        select(&mut a.state, 2, false);
        run(&mut a, MCmd::AddEffect(0, 0));
        run(&mut a, MCmd::AddEffect(1, 0));
        let key = crate::plugins::fx_key(2, 0);
        let entry = |a: &App, label: &str| {
            let m = slot_menu(a, 0, 0.0, 0.0);
            m.items.iter().find(|i| i.label == label && !i.head).map(|i| (i.enabled, i.mark.clone(), i.cmd.clone())).unwrap()
        };
        assert_eq!(entry(&a, "Auto-bypass when silent"), (true, Mark::Check(false), Cmd::PluginCpu(key, true)), "new slots start off");
        assert_eq!(entry(&a, "Allow threaded processing"), (true, Mark::Check(true), Cmd::PluginCpu(key, false)));
        crate::playlist::run(&mut a, Cmd::PluginCpu(key, true));
        assert_eq!(entry(&a, "Auto-bypass when silent").1, Mark::Check(true));
        let smart = |a: &App, k: usize| track(&a.state.session, 2).unwrap().slot_cpu[k].smart_disable;
        assert!(smart(&a, 0) && !smart(&a, 1), "only that slot");
        crate::history_ui::undo(&mut a.state);
        assert!(!smart(&a, 0), "one undo step");
        crate::playlist::run(&mut a, Cmd::PluginCpu(key, false));
        assert_eq!(entry(&a, "Allow threaded processing").1, Mark::Check(false));
        // Options > Switch auto-bypass for all plugins: the slots and the plugin channels.
        let find = |m: Popup| m.items.iter().find(|i| i.label == AUTO_BYPASS_ALL_LABEL).map(|i| (i.enabled, i.cmd.clone())).unwrap();
        assert_eq!(find(options_menu(&a.state, 0.0, 0.0)), (true, Cmd::Mixer(MCmd::AutoBypassAll)));
        a.state.session.project.get_mut().channels[0].kind = os_model::ChannelKind::Plugin;
        crate::playlist::run(&mut a, Cmd::PluginCpu(key, true));
        run(&mut a, MCmd::AutoBypassAll);
        assert!(smart(&a, 0) && smart(&a, 1) && a.state.session.project.channels[0].misc.cpu.smart_disable, "one off: all on");
        assert!(!track(&a.state.session, 2).unwrap().slot_cpu[2].smart_disable, "an empty slot stays as it is");
        run(&mut a, MCmd::AutoBypassAll);
        assert!(!smart(&a, 0) && !smart(&a, 1) && !a.state.session.project.channels[0].misc.cpu.smart_disable, "all on: all off");
        crate::history_ui::undo(&mut a.state);
        assert!(smart(&a, 0) && smart(&a, 1), "one undo step");
        // The instrument rack's options menu runs the same command.
        assert_eq!(find(crate::rack::options_menu(&a.state, 0.0, 0.0)), (true, Cmd::Mixer(MCmd::AutoBypassAll)));
        // TOOLS > Switch auto-bypass for all plugins.
        use crate::views::menus::{self, MENUS, TOOLS_MENU};
        let items: Vec<_> = MENUS[TOOLS_MENU].cols.iter().flat_map(|(_, items)| items.iter()).collect();
        let i = items.iter().position(|it| it.label == AUTO_BYPASS_ALL_LABEL).unwrap();
        assert!(menus::works(TOOLS_MENU, items[i]));
        a.menu_item(TOOLS_MENU, i);
        assert!(!smart(&a, 0) && !smart(&a, 1), "TOOLS: all on, so all off");
    }

    /// Options ▸ Plugin delay compensation ▸ Automatic (spec: audio/delay-compensation.md "Automatic"): ticked on a new
    /// project; a pick flips it as one undo step and the engine follows it.
    #[test]
    fn automatic_delay_compensation_is_a_project_switch() {
        let mut a = app();
        let auto = |a: &App| {
            let m = options_menu(&a.state, 0.0, 0.0);
            let pdc = m.items.iter().find(|i| i.label == "Plugin delay compensation").unwrap();
            pdc.children.iter().find(|i| i.label == "Automatic").map(|i| (i.enabled, i.mark.clone(), i.cmd.clone())).unwrap()
        };
        assert_eq!(auto(&a), (true, Mark::Check(true), Cmd::Mixer(MCmd::PdcAuto)));
        assert_eq!(run(&mut a, MCmd::PdcAuto).as_deref(), Some("Automatic plugin delay compensation off"));
        assert!(a.state.session.project.pdc_off);
        assert_eq!(auto(&a).1, Mark::Check(false));
        crate::history_ui::undo(&mut a.state);
        assert!(!a.state.session.project.pdc_off, "one undo step");
    }

    /// Options ▸ View ▸ Waveforms (Alt+W, the title bar's View waveforms) and Extra properties (Lemon Studio's design):
    /// switches of the window; with Waveforms on each track keeps its recent peaks, newest last, and the meters keep
    /// moving until the waveforms are quiet.
    #[test]
    fn view_waveforms_and_extra_properties() {
        use crate::keys::Key;
        let mut a = app();
        let view = |a: &App| {
            let m = options_menu(&a.state, 0.0, 0.0);
            let v = m.items.iter().find(|i| i.label == "View").unwrap();
            v.children.iter().map(|i| (i.label.to_string(), i.enabled, i.mark.clone())).collect::<Vec<_>>()
        };
        assert_eq!(view(&a), [("Waveforms".into(), true, Mark::Check(false)), ("Extra properties".into(), true, Mark::Check(true))]);
        run(&mut a, MCmd::ViewExtra);
        assert!(!a.state.mixer.extra);
        assert!(key(&mut a, &Key::Character("w".into()), false, true), "Alt+W");
        assert_eq!(view(&a)[0].2, Mark::Check(true));
        assert!(feed_meters(&mut a.state, &[[0.5, -0.25], [0.0, 0.0]]));
        assert_eq!(a.state.mixer.waves[0].back(), Some(&[0.5, 0.25]));
        assert_eq!(a.state.mixer.waves[1].back(), Some(&[0.0, 0.0]));
        for _ in 0..WAVE_LEN {
            feed_meters(&mut a.state, &[[0.0, 0.0], [0.0, 0.0]]);
        }
        assert_eq!(a.state.mixer.waves[0].len(), WAVE_LEN);
        assert!(!a.state.mixer.waves[0].iter().any(|p| p[0] > 0.0), "scrolled out");
        a.state.mixer.meters.clear();
        assert!(!feed_meters(&mut a.state, &[[0.0, 0.0], [0.0, 0.0]]), "quiet: still");
        run(&mut a, MCmd::ViewWaveforms);
        assert!(!a.state.mixer.waveforms && a.state.mixer.waves.is_empty());
    }

    /// The slot menu's Help (Lemon Studio's design): an effect the engine plays itself opens its page in the wiki, under
    /// its Lemon Studio name, and so does a container; a plugin naming no page says so.
    #[test]
    fn slot_help_opens_the_plugins_page() {
        let mut a = app();
        a.state.session.plugins.effects = vec![crate::plugin_db::fake("Test FX", os_host::Kind::Effect)];
        select(&mut a.state, 2, false);
        run(&mut a, MCmd::AddEffect(0, 0));
        track_mut(&mut a.state.session, 2).unwrap().slots[1] = Some(os_model::SlotPlugin { id: os_model::Id::NONE, name: "Fruity Balance".into(), state: os_engine::effects::balance::Settings { v: [0, 128] }.to_state() });
        let help = slot_menu(&a, 0, 0.0, 0.0).items.iter().find(|i| i.label == "Help").map(|i| (i.enabled, i.cmd.clone())).unwrap();
        assert_eq!(help, (true, Cmd::Mixer(MCmd::SlotHelp(0))));
        assert_eq!(run(&mut a, MCmd::SlotHelp(0)).as_deref(), Some("Test FX has no help page"));
        assert_eq!(run(&mut a, MCmd::SlotHelp(1)), Some(format!("Opening {}", crate::help_links::plugin_page("Lemon Balance"))));
        track_mut(&mut a.state.session, 2).unwrap().slots[1] = Some(os_model::SlotPlugin { id: os_model::Id::NONE, name: crate::container::CONTAINER.into(), state: crate::container::encode(&[]) });
        assert_eq!(run(&mut a, MCmd::SlotHelp(1)), Some(format!("Opening {}", crate::help_links::plugin_page(crate::container::CONTAINER))));
    }

    /// FL's measured group drags, in the percentages its hint shows (spec: ui/mixer.md §4): `kept` levels, the pressed
    /// one's kept level and where it went, and where the others landed. The hint's whole percents go in, so 1 % either
    /// way is as near as they say.
    #[test]
    fn group_fader_lands_where_fl_does() {
        for (kept, level, to, want) in [
            // Same dB each: Inserts 3 to 7, 3 down 40 px.
            (&[85.0, 70.0, 58.0, 40.0][..], 100.0, 94.0, &[79, 65, 53, 36][..]),
            // Each stops at the top on its own.
            (&[94.0, 65.0, 53.0, 36.0], 79.0, 125.0, &[125, 108, 94, 71]),
            // Three more drags from the same kept levels: the one at the top stays until the others have come down.
            (&[89.0, 60.0], 74.0, 125.0, &[125, 108]),
            (&[89.0, 60.0], 74.0, 116.0, &[125, 100]),
            (&[89.0, 60.0], 74.0, 108.0, &[123, 91]),
            (&[89.0, 60.0], 74.0, 100.0, &[115, 84]),
            // A group at -INF comes back from its kept levels.
            (&[116.0, 116.0], 11.0, 9.0, &[107, 107]),
            // Pressed at -INF and moved up: the others go to the top, and one at -INF stays.
            (&[94.0, 79.0, 65.0, 0.0], 0.0, 14.0, &[125, 125, 125, 0]),
        ] {
            let got: Vec<i64> = kept.iter().map(|&k| (group_fader(k * 0.008, level * 0.008, to * 0.008) * 125.0).round() as i64).collect();
            let near = got.iter().zip(want).all(|(g, w)| (g - w).abs() <= 1 && (*w > 0 || *g == 0));
            assert!(near, "{kept:?} with the pressed one from {level} to {to}: {got:?}, FL {want:?}");
        }
        assert_eq!(group_fader(0.5, 0.6, 0.0), 0.0, "the pressed one at -INF takes them there");
        assert_eq!(group_fader(0.5, 0.6, 0.6), 0.5, "back at the press point, exactly");
    }
}
