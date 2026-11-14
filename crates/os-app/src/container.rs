// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Containers, Lemon Studio's Patcher (slices 1 and 3): a mixer slot holding a serial chain of effects, or a channel
//! whose instrument is such a chain led by instruments, each member with its own wet and dry levels. The slot menu's
//! Wrap in container moves the slot's effect into a new container in the same slot; Wrap all in container moves the
//! track's effects, in slot order, into one container in this slot and empties the others; the channel menu's Wrap in
//! container moves the channel's instrument into a container on the channel, which plays the channel's notes. The
//! container's window (`ContainerUi`, `views::container`) lists the members with their wet and dry knobs, removes,
//! moves, adds and replaces them and opens their windows. Each edit is one undo step.
//!
//! The project keeps a container as a slot or channel plugin of its own name whose state lists the members (`Member`):
//! each plugin's identity, kind, name, levels and saved state. The chain is built from that list
//! (`App::sync_containers`) and plays as the slot's effect or the channel's instrument (`os_engine::chain::Chain`). A
//! container is named by its place, a track and a slot, the slot [`CHANNEL`] for a channel's (the track then being its
//! rack row). The members' instances are hosted under keys of their own (`plugins::MEMBER_KEY`), for their windows,
//! parameters and presets. A member's parameters and its wet and dry levels are automated and linked as parameters of
//! the container's slot or channel: the member's id and the item packed in the parameter index
//! (`os_model::target::member_item`), so that clips and links follow the member when it moves. Their windows' edits and
//! presets are undo steps by an owner id of their own (`control::command::member_owner`). A wrap points the clips,
//! recorded pattern events and links on the wrapped plugins' parameters at their members (`Moved`). A member has its
//! own name, colour, icon and CPU switches, as a slot's plugin has, saved with the container.
//!
//! An opened project's mixer slots holding the original's Patcher open as containers when their patch is one plain
//! serial chain of effects (slice 5's serial part, [`open_patchers`]); a Shift+drop of the browser's container entry on
//! a slot or channel wraps its plugin as the menu does (`App::drop_container_on_slot`, `App::drop_container_on_rack`).
// spec: plugins/patcher/README.md §3.1, §3.3, §4 slices 1, 3 and 5, §4.1, §4.2, §4.3, §4.4, §4.5

#![allow(deprecated)] // writes the project directly (`crate::locked`; control-plane step 14 removes this)
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
pub use crate::edit::container::*;

use os_engine::chain::{self, Chain};
use os_model::target::{self, MEMBER_IDS, MEMBER_WET};
use os_model::{Id, Project};

use crate::plugins::{fx_key, fx_of, Hosted, Plugins, FX_KEY, MEMBER_KEY};

/// The slot plugin name a container is kept under (Lemon Studio's own; the original's Patcher slots whose patch is a
/// plain serial chain open as containers, [`open_patchers`]).
pub const CONTAINER: &str = "Lemon Container";

/// The state's first bytes and version (1: without the members' ids; 2: without their kinds; 3: without their CPU
/// switches, names, colours and icons).
pub(crate) const MAGIC: &[u8; 4] = b"LCNT";
pub(crate) const VERSION: u32 = 4;

/// A member's kind in the state (version 3): bit 0 set for an instrument.
const INSTRUMENT: u32 = 1;

/// A member's CPU switches in the state (version 4), the bits of a plugin window record's flags (spec: ui/plugin-
/// window.md "CPU"): Smart disable (Auto-bypass when silent) and Allow threaded processing.
const SMART_DISABLE: u32 = 0x20;
const THREADED: u32 = 0x40;

/// A member's colour in the state (version 4): this bit over 0xRRGGBB, 0 for none.
const HAS_COLOR: u32 = 0x0100_0000;

/// The slot a channel's container is named by: a container's place is (rack row, `CHANNEL`) on a channel, (track,
/// slot) in the mixer.
pub const CHANNEL: u8 = 0xFF;

/// One plugin of a container, as the project keeps it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Member {
    /// Its number in the container, kept while it is moved (an edit keeps its instance and window).
    pub id: u32,
    /// An instrument (it plays the channel's notes; a channel's container only), else an effect.
    pub instrument: bool,
    /// The plugin's identity (`os_host::Descriptor::uid`) and name.
    pub uid: String,
    pub name: String,
    /// The output is `wet` times the effect's output plus `dry` times its input.
    pub wet: f32,
    pub dry: f32,
    /// The plugin's own saved state (empty: the plugin's defaults).
    pub state: Vec<u8>,
    /// Its CPU switches, as a slot's plugin has them (spec: plugins/patcher/README.md §4.5): Auto-bypass when silent
    /// passes an effect member by while it is silent (`os_engine::chain`); Allow threaded processing is kept.
    pub cpu: os_model::PluginCpu,
    /// A name typed for it (Rename, color and icon...), shown in place of its plugin's; its colour and icon (§4.5).
    pub label: Option<String>,
    pub color: Option<os_model::Rgb>,
    pub icon: Option<crate::icon::Icon>,
}

impl Member {
    /// What the member is shown as: the name typed for it, else its plugin's.
    pub fn shown(&self) -> &str {
        self.label.as_deref().unwrap_or(&self.name)
    }
}

/// The state of a container slot: the members in order.
pub fn encode(members: &[Member]) -> Vec<u8> {
    let mut b = MAGIC.to_vec();
    b.extend(VERSION.to_le_bytes());
    b.extend((members.len() as u32).to_le_bytes());
    let bytes = |b: &mut Vec<u8>, v: &[u8]| {
        b.extend((v.len() as u32).to_le_bytes());
        b.extend_from_slice(v);
    };
    for m in members {
        b.extend(m.id.to_le_bytes());
        b.extend((if m.instrument { INSTRUMENT } else { 0 }).to_le_bytes());
        bytes(&mut b, m.uid.as_bytes());
        bytes(&mut b, m.name.as_bytes());
        b.extend(m.wet.to_le_bytes());
        b.extend(m.dry.to_le_bytes());
        bytes(&mut b, &m.state);
        // Version 4: the CPU switches, the name typed, the colour and the icon's name (empty: none).
        let cpu = if m.cpu.smart_disable { SMART_DISABLE } else { 0 } | if m.cpu.threaded { THREADED } else { 0 };
        b.extend(cpu.to_le_bytes());
        bytes(&mut b, m.label.as_deref().unwrap_or_default().as_bytes());
        let color = m.color.map_or(0, |c| HAS_COLOR | u32::from(c.r) << 16 | u32::from(c.g) << 8 | u32::from(c.b));
        b.extend(color.to_le_bytes());
        bytes(&mut b, icon_name(m.icon).as_bytes());
    }
    b
}

/// The name an icon is saved by: its name in the Change icon... list (empty: none).
fn icon_name(icon: Option<crate::icon::Icon>) -> &'static str {
    icon.and_then(|i| crate::icon::ICONS.iter().find(|(_, x)| *x == i)).map_or("", |(name, _)| name)
}

/// The icon saved as `name` (a name the list does not have: none).
fn icon_named(name: &str) -> Option<crate::icon::Icon> {
    crate::icon::ICONS.iter().find(|(n, _)| *n == name).map(|(_, i)| *i)
}

/// The members a container slot's state lists; `None` for bytes that are not one.
pub fn decode(b: &[u8]) -> Option<Vec<Member>> {
    if b.get(..4)? != MAGIC {
        return None;
    }
    let mut at = 4;
    let word = |at: &mut usize| -> Option<u32> {
        let w = u32::from_le_bytes(b.get(*at..*at + 4)?.try_into().ok()?);
        *at += 4;
        Some(w)
    };
    let bytes = |at: &mut usize| -> Option<Vec<u8>> {
        let n = word(at)? as usize;
        let s = b.get(*at..at.checked_add(n)?)?.to_vec();
        *at += n;
        Some(s)
    };
    let version = word(&mut at)?;
    if !(1..=VERSION).contains(&version) {
        return None;
    }
    let count = word(&mut at)?;
    let mut v = Vec::new();
    for i in 0..count {
        let id = if version >= 2 { word(&mut at)? } else { i + 1 };
        let instrument = version >= 3 && word(&mut at)? & INSTRUMENT != 0;
        let uid = String::from_utf8(bytes(&mut at)?).ok()?;
        let name = String::from_utf8(bytes(&mut at)?).ok()?;
        let wet = f32::from_bits(word(&mut at)?);
        let dry = f32::from_bits(word(&mut at)?);
        let state = bytes(&mut at)?;
        let mut m = Member { id, instrument, uid, name, wet, dry, state, ..Member::default() };
        if version >= 4 {
            let cpu = word(&mut at)?;
            m.cpu = os_model::PluginCpu { threaded: cpu & THREADED != 0, smart_disable: cpu & SMART_DISABLE != 0 };
            m.label = Some(String::from_utf8(bytes(&mut at)?).ok()?).filter(|l| !l.is_empty());
            let c = word(&mut at)?;
            m.color = (c & HAS_COLOR != 0).then(|| os_model::Rgb::new((c >> 16) as u8, (c >> 8) as u8, c as u8));
            m.icon = icon_named(std::str::from_utf8(&bytes(&mut at)?).ok()?);
        }
        v.push(m);
    }
    Some(v)
}

/// Whether channel `ch` is a container (its instrument a chain).
pub fn is_container_channel(ch: &os_model::Channel) -> bool {
    ch.kind == os_model::ChannelKind::Plugin && ch.plugin.as_deref() == Some(CONTAINER)
}

/// The state of the container at place (`t`, `k`): slot `k` of track `t`, or rack row `t`'s for `k` = [`CHANNEL`].
pub(crate) fn state_in(p: &Project, t: u16, k: u8) -> Option<&[u8]> {
    if k == CHANNEL {
        let ch = p.channels.get(usize::from(t)).filter(|c| is_container_channel(c))?;
        return ch.plugin_state.as_deref().or(Some(&[][..]));
    }
    let slot = p.mixer.iter().find(|m| m.index == t)?.slots.get(usize::from(k))?.as_ref()?;
    (slot.name == CONTAINER).then_some(slot.state.as_slice())
}

/// `state_in` to write.
pub(crate) fn state_in_mut(p: &mut Project, t: u16, k: u8) -> Option<&mut Vec<u8>> {
    if k == CHANNEL {
        let ch = p.channels.get_mut(usize::from(t)).filter(|c| is_container_channel(c))?;
        return Some(ch.plugin_state.get_or_insert_with(Vec::new));
    }
    let slot = p.mixer.iter_mut().find(|m| m.index == t)?.slots.get_mut(usize::from(k))?.as_mut()?;
    (slot.name == CONTAINER).then_some(&mut slot.state)
}

/// A level knob's hint: "Lemon Limiter wet: 60%".
pub fn level_text(name: &str, wet: bool, v: f32) -> String {
    format!("{name} {}: {:.0}%", if wet { "wet" } else { "dry" }, v * 100.0)
}

/// The `Plugins::containers` key of the container at place (`t`, `k`): its slot's `fx_key`, or its channel's row.
pub fn place_key(t: u16, k: u8) -> usize {
    if k == CHANNEL {
        usize::from(t)
    } else {
        fx_key(t, k)
    }
}

/// The place of the container under `Plugins::containers` key `key` (`place_key`'s other way).
pub fn key_place(key: usize) -> Option<(u16, u8)> {
    if key < FX_KEY {
        return Some((u16::try_from(key).ok()?, CHANNEL));
    }
    fx_of(key)
}

/// The wet and dry levels of a slot at stored mix `mix` (0..1) and switch `on`, as the slot blends them (spec:
/// audio/mixer.md "An effect slot"): an effect switched off keeps only its dry part (the manual's "mix 0").
pub(crate) fn levels(mix: f64, on: bool) -> (f32, f32) {
    if !on {
        return (0.0, 1.0);
    }
    let d = os_dsp::mixer::dry_level((mix.clamp(0.0, 1.0) * 12800.0).round() as i32);
    (1.0 - d, d)
}

/// A container being played: the slot's or channel's id, the state its chain was built from, the members' instances
/// and the chain.
pub struct Live {
    pub id: Id,
    /// The slot's state the chain was built from: compared every tick, decoded only when it changed.
    src: Vec<u8>,
    /// The members' ids and identities by position (another list rebuilds the chain; other levels only set them).
    pub(crate) shape: Vec<(u32, String)>,
    /// Whether it plays a channel (its members' instruments play the channel's notes).
    pub channel: bool,
    /// The members' `hosted` keys by position (`None`: its effect was not found, left out of the chain).
    pub keys: Vec<Option<usize>>,
    pub fx: Arc<Mutex<Chain>>,
}

pub(crate) fn shape(members: &[Member]) -> Vec<(u32, String)> {
    members.iter().map(|m| (m.id, m.uid.clone())).collect()
}

/// The undo step names of the menu entries.
/// The slot menu's Wrap in container and the channel menu's (the original's Patcherize) are named alike.
pub const WRAP: &str = "Wrap in container";
pub const WRAP_ALL: &str = "Wrap all in container";
pub const REMOVE: &str = "Remove effect from container";
pub const MOVE: &str = "Move effect in container";
pub const ADD: &str = "Add effect to container";
pub const ADD_INSTRUMENT: &str = "Add instrument to container";
pub const REPLACE: &str = "Replace effect in container";
pub const LEVEL: &str = "Effect level in container";

/// The undo step names of a member's own name, colour and icon (its CPU switches' are a slot's: `playlist::
/// flip_plugin_cpu`).
pub const NAME: &str = "Effect name in container";
pub const COLOR: &str = "Effect color in container";
pub const ICON: &str = "Effect icon in container";

/// A new instrument member's levels: its sound added to what the members before it put out (a layer).
pub const INSTRUMENT_LEVELS: (f32, f32) = (1.0, 1.0);

/// The most effects a container holds (the ids its members' automation targets have room for).
pub const MAX_MEMBERS: usize = MEMBER_IDS as usize;

/// The container's window (Lemon Studio's design: spec plugins/patcher/README.md §4.3): where it is, the container it
/// shows, and the levels that container's chain plays, read every tick (`App::mirror_levels`) so that an automated
/// level turns its knob.
#[derive(Debug, Clone)]
pub struct ContainerUi {
    pub pos: (f32, f32),
    /// The place of the container shown: track and slot, or rack row and [`CHANNEL`].
    pub at: (u16, u8),
    /// The chain's wet and dry by member id, as last read.
    pub live: BTreeMap<u32, (f32, f32)>,
}

impl Default for ContainerUi {
    fn default() -> Self {
        ContainerUi { pos: (420.0, 160.0), at: (0, 0), live: BTreeMap::new() }
    }
}

/// What the window's controls do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CAct {
    /// Member `.0`'s name (its place in the list): opens its window; a right click, its menu.
    Open(usize),
    /// Member `.0`'s ▸ button: its menu.
    Menu(usize),
    /// Add effect: the effect list of an empty slot (a channel's container: the instruments too).
    Add,
}

/// What the container's list does to a member.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Op {
    /// Opens its plugin window (no undo step).
    Open,
    Up,
    Down,
    Remove,
}

pub(crate) fn pct(v: f32) -> i32 {
    (v * 100.0).round() as i32
}

/// Every container member's CPU switches in project `p`, for `f` to read or change (Switch auto-bypass for all plugins,
/// `mixer::switch_auto_bypass`); a container whose members `f` changed is written again.
pub fn each_member_cpu(p: &mut Project, mut f: impl FnMut(&mut os_model::PluginCpu)) {
    let slots = p.mixer.iter_mut().flat_map(|m| m.slots.iter_mut().flatten()).filter(|s| s.name == CONTAINER).map(|s| &mut s.state);
    let channels = p.channels.iter_mut().filter(|c| is_container_channel(c)).filter_map(|c| c.plugin_state.as_mut());
    for b in slots.chain(channels) {
        let Some(mut list) = decode(b) else { continue };
        let before = list.clone();
        list.iter_mut().for_each(|m| f(&mut m.cpu));
        if list != before {
            *b = encode(&list);
        }
    }
}

/// What an edit of the container at place (`t`, `k`) changes, for its undo step: its track, or its channel.
pub(crate) fn region(t: u16, k: u8) -> crate::control::command::Region {
    use crate::control::command::Region;
    if k == CHANNEL {
        Region::Channel(usize::from(t))
    } else {
        Region::Track(t)
    }
}

/// The smallest id no member of `list` has, up to the targets' limit (`None`: the container is full).
pub(crate) fn free_id(list: &[Member]) -> Option<u32> {
    (1..=MEMBER_IDS).find(|id| list.iter().all(|m| m.id != *id))
}

pub(crate) fn label(i: usize, m: &Member) -> String {
    format!("{}. {} (wet {}%, dry {}%)", i + 1, m.shown(), pct(m.wet), pct(m.dry))
}

/// Before an edit of the containers' lists: each member's state as its instance holds it now, so that the edit (and its
/// undo) keeps what was set in the members' windows. A save stores them the same way (`project_plugins::store_hosted`).
pub fn store_members(proj: &mut Project, plugins: &Plugins) {
    for (&key, live) in &plugins.containers {
        let Some((t, k)) = key_place(key) else { continue };
        let Some(p) = state_in_mut(proj, t, k) else { continue };
        let Some(mut list) = decode(p) else { continue };
        let mut changed = false;
        for m in &mut list {
            let Some(at) = live.shape.iter().position(|s| (s.0, s.1.as_str()) == (m.id, m.uid.as_str())) else { continue };
            let Some(h) = live.keys[at].and_then(|k| plugins.hosted.get(&k)) else { continue };
            if let Some(state) = h.plugin.lock().ok().and_then(|p| p.save_state()).filter(|st| *st != m.state) {
                m.state = state;
                changed = true;
            }
        }
        if changed {
            *p = encode(&list);
        }
    }
}

/// A plugin a wrap moved into a container (Lemon Studio's design, spec: plugins/patcher/README.md §4.5): where it was,
/// and which member it became.
#[derive(Debug, Clone)]
pub(crate) struct Move {
    /// Its place before: slot `.1` of mixer track `.0`, or rack row `.0`'s channel for [`CHANNEL`].
    pub(crate) from: (u16, u8),
    pub(crate) ids: Ids,
}

/// The member a moved plugin became: one member's id, or for a container wrapped again (Wrap all) its members' ids, old
/// and new.
#[derive(Debug, Clone)]
pub(crate) enum Ids {
    Plugin(u32),
    Members(Vec<(u32, u32)>),
}

impl Ids {
    /// The container item that parameter `i` of the moved plugin (or item `i` of the moved container) is now; `None`
    /// for a plugin parameter past a member's room (index 2046 and up, spec §4.2) or a member that is not there.
    pub(crate) fn item(&self, i: u16) -> Option<u16> {
        match self {
            Ids::Plugin(id) => target::member_item(*id, i).filter(|_| i < MEMBER_WET),
            Ids::Members(ids) => {
                let (old, item) = target::member_of(i)?;
                target::member_item(ids.iter().find(|p| p.0 == old)?.1, item)
            }
        }
    }
}

/// What a wrap moved into the container at place `to` (a slot, or a rack row's channel of index `index`): the
/// automation targets and remote control links naming the moved plugins' parameters follow them onto the members.
pub(crate) struct Moved {
    pub(crate) to: (u16, u8),
    /// The channel's index (automation names a channel by it), for a channel's container.
    pub(crate) index: u16,
    pub(crate) moves: Vec<Move>,
}

/// The hint bar's note on the targets a wrap could not move ("" for none).
pub(crate) fn stuck_text(n: usize) -> String {
    match n {
        0 => String::new(),
        1 => "; 1 automation target past parameter 2046 left as it was".into(),
        n => format!("; {n} automation targets past parameter 2046 left as they were"),
    }
}

/// The member a node of the original's Patcher opens as, its plugin found among `effects` the way a plain mixer slot
/// holding that plugin is played on opening (spec: plugins/patcher/state.md §7):
/// 1. with the "installed native plugins first" setting, the installed native plugin (`App::host_native`);
/// 2. an FL effect a Lemon effect recreates: that effect, with the state its reader makes of the node's stream
///    (`App::sync_effects`);
/// 3. a wrapped plugin (the wrapper's stream, `flp/events/plugins.md` §4): the plugin `project_plugins::find` gives,
///    from its defaults when it is a copy in another format (`App::sync_wrapped`);
/// 4. else the installed native plugin of that name.
///
/// A plugin not found is kept with its stream, and the chain passes it by, as a container's member that is not found
/// always is (a plain slot keeps its bytes and plays nothing). Lemon Studio's own: the effects the engine plays for a
/// plain slot without a plugin instance (`os_engine::effects::from_slot`, Fruity Stereo Shaper) are not found here, as
/// a container's members are plugin instances.
fn member_of(n: &os_flp::patcher::Node, effects: &[crate::plugins::Available]) -> Member {
    use crate::plugins::{recreation_of, recreation_state_from_fl, EDISON_FILE_NAME, EQ_FILE_NAME, LEMON_EQ, LOVE_PHILTER_FILE_NAME, MAXIMUS_FILE_NAME, PRESS, PULP_FILTER, TESLA};
    use crate::project_plugins::{find, find_native, native_first, switched_format};
    let (fl, file) = (n.state.as_slice(), n.plugin.as_str());
    let found = |i: usize, state: Vec<u8>| (effects[i].desc.uid(), effects[i].desc.name.clone(), state);
    let lemon = match file {
        EQ_FILE_NAME => Some(LEMON_EQ),
        LOVE_PHILTER_FILE_NAME => Some(PULP_FILTER),
        MAXIMUS_FILE_NAME => Some(PRESS),
        EDISON_FILE_NAME => Some(TESLA),
        _ => recreation_of(file),
    };
    let wrapped = (file == os_flp::wrapper::WRAPPER).then(|| os_flp::wrapper::decode(fl)).flatten();
    let (uid, name, state) = if let Some(i) = find_native(effects, file).filter(|_| native_first()) {
        found(i, fl.to_vec())
    } else if let Some((i, lemon)) = lemon.and_then(|l| Some((effects.iter().position(|a| a.desc.name == l)?, l))) {
        let state = match file {
            EQ_FILE_NAME => Some(lemon_parametric_eq_2::params::plugin_state_from_fl(fl)),
            LOVE_PHILTER_FILE_NAME => Some(pulp_filter::plugin_state_from_fl(fl)),
            MAXIMUS_FILE_NAME => Some(press::plugin_state_from_fl(fl)),
            EDISON_FILE_NAME => tesla::plugin_state_from_edison(fl),
            _ => recreation_state_from_fl(lemon, fl),
        };
        // No state: the effect's defaults, as for a slot.
        found(i, state.unwrap_or_default())
    } else if let Some(r) = wrapped {
        match find(effects, &r) {
            Some(i) => {
                let other = switched_format(&effects[i].desc, &r);
                found(i, if other.is_none() { r.state } else { Vec::new() })
            }
            None => {
                let format = os_host::Format::parse(&r.format).map_or_else(|| r.format.to_ascii_lowercase(), |f| f.as_str().to_string());
                (format!("{format}:{}", r.id), r.name, r.state)
            }
        }
    } else if let Some(i) = find_native(effects, file) {
        found(i, fl.to_vec())
    } else {
        // A native plugin's identity is its file id (`Descriptor::uid` of a native plugin).
        (format!("native:{file}"), if n.display.is_empty() { file.to_string() } else { n.display.clone() }, fl.to_vec())
    };
    // The node's own name in the map, when the user gave it one (spec: plugins/patcher/state.md §3).
    let own = n.name.trim();
    let label = (!own.is_empty() && own != name && own != n.plugin && !own.eq_ignore_ascii_case(&n.display)).then(|| own.to_string());
    Member { uid, name, wet: 1.0, dry: 0.0, state, label, ..Member::default() }
}

/// Opening a project (README.md slice 5, serial part): each mixer slot holding the original's Patcher whose patch is
/// one plain serial chain of effects (`os_flp::patcher::Patch::serial_chain`) becomes a container in the same slot,
/// its nodes the members in chain order at wet 100 %, dry 0 % (every link level is 1.0), each with its plugin and state
/// ([`member_of`]). The slot keeps its id, switch, mix, name, colour and icon. Any other Patcher slot, and every Patcher
/// channel (an instrument patch, slice 3), stays as the project had it. What was opened and what was not, and why, is
/// returned for the hint bar. The project is opened that way, not changed: a save writes the containers (decision #7).
// spec: plugins/patcher/state.md §1 to §7; plugins/patcher/README.md §3.1, §4 slice 5
pub fn open_patchers(s: &mut crate::session::Session, effects: &[crate::plugins::Available]) -> Option<String> {
    use os_flp::patcher::{self, NotChain, PATCHER};
    let mut opened = Vec::new();
    let mut left = Vec::new();
    let places: Vec<(u16, u8)> = s
        .project
        .mixer
        .iter()
        .flat_map(|m| m.slots.iter().enumerate().filter(|(_, p)| p.as_ref().is_some_and(|p| p.name == PATCHER)).map(move |(k, _)| (m.index, k as u8)))
        .collect();
    for (t, k) in places {
        let place = format!("{}, slot {}", crate::edit::mixer::name(s, t), k + 1);
        let Some(slot) = crate::edit::mixer::track_mut(s, t).and_then(|m| m.slots[usize::from(k)].as_mut()) else { continue };
        let Some(patch) = patcher::parse(&slot.state) else {
            left.push(format!("{place} (a state Lemon Studio does not read)"));
            continue;
        };
        let order = match patch.serial_chain() {
            Ok(order) if order.len() <= MAX_MEMBERS => order,
            Ok(_) => {
                left.push(format!("{place} (more than {MAX_MEMBERS} effects)"));
                continue;
            }
            Err(why) => {
                left.push(format!("{place} ({})", why.text()));
                continue;
            }
        };
        let list: Vec<Member> = order.iter().zip(1..).map(|(&i, id)| Member { id, ..member_of(&patch.nodes[i], effects) }).collect();
        slot.name = CONTAINER.into();
        slot.state = encode(&list);
        opened.push(place);
    }
    for (r, ch) in s.project.channels.iter().enumerate() {
        if ch.plugin.as_deref() == Some(PATCHER) {
            let name = ch.name.clone().filter(|n| !n.is_empty()).unwrap_or_else(|| format!("channel {}", r + 1));
            left.push(format!("{name} ({})", NotChain::Instrument.text()));
        }
    }
    let mut parts = Vec::new();
    if !opened.is_empty() {
        parts.push(format!("{} opened as {}", opened.join(", "), if opened.len() == 1 { "a container" } else { "containers" }));
    }
    if !left.is_empty() {
        parts.push(format!("{}: {}", if left.len() == 1 { "left as it was" } else { "left as they were" }, left.join(", ")));
    }
    (!parts.is_empty()).then(|| format!("Patcher: {}", parts.join("; ")))
}

/// Every tick: the containers played follow the project's container slots and channels (a wrap, an edit of the
/// list, an undo, an opened project, a slot or channel moved). A container whose list changed is built again from
/// it: a member still there (by its id and plugin) keeps its instance and window; a new one is an instance of its
/// plugin started from its saved state, or left out of the chain when its plugin is not found (the list keeps it). A
/// list whose levels or Auto-bypass switches alone changed sets them on the chain.
pub(crate) fn sync_containers_in(ses: &mut crate::session::Session) -> Option<String> {
    let mut want: Vec<(usize, Id)> = Vec::new();
    for m in &ses.project.mixer {
        for (k, p) in m.slots.iter().enumerate() {
            if let Some(p) = p.as_ref().filter(|p| p.name == CONTAINER) {
                want.push((fx_key(m.index, k as u8), p.id));
            }
        }
    }
    for (r, ch) in ses.project.channels.iter().enumerate() {
        if is_container_channel(ch) && u16::try_from(r).is_ok() {
            want.push((r, ch.id));
        }
    }
    if want.is_empty() && ses.plugins.containers.is_empty() {
        ses.plugin_members.clear();
        return None;
    }
    let mut changed = false;
    let mut channels = false;
    // Gone, or moved to another slot or row (found by the slot's or channel's id).
    let keys: Vec<usize> = ses.plugins.containers.keys().copied().collect();
    for key in keys {
        if want.iter().any(|w| w.0 == key) {
            continue;
        }
        let live = ses.plugins.containers.remove(&key).unwrap();
        channels |= live.channel;
        let to = want.iter().find(|w| w.1 == live.id && !live.id.is_none() && !ses.plugins.containers.contains_key(&w.0) && (w.0 < FX_KEY) == live.channel).map(|w| w.0);
        match to {
            Some(to) => {
                ses.plugins.containers.insert(to, live);
            }
            None => {
                for k in live.keys.into_iter().flatten() {
                    ses.plugins.drop_key(k);
                }
            }
        }
        changed = true;
    }
    let mut missing = Vec::new();
    for (key, id) in want {
        let Some((t, k)) = key_place(key) else { continue };
        let Some(p) = state_in(&ses.project, t, k) else { continue };
        if let Some(live) = ses.plugins.containers.get_mut(&key) {
            live.id = id;
            if live.src == p {
                continue;
            }
        }
        let list = decode(p).unwrap_or_default();
        let src = p.to_vec();
        if let Some(live) = ses.plugins.containers.get_mut(&key).filter(|l| l.shape == shape(&list)) {
            if let Ok(mut c) = live.fx.lock() {
                let playing = list.iter().zip(&live.keys).filter(|(_, k)| k.is_some());
                for (at, (m, _)) in playing.enumerate() {
                    c.set_levels(at, m.wet, m.dry);
                    c.set_smart(at, m.cpu.smart_disable);
                }
            }
            live.src = src;
            continue;
        }
        // Built again: the instances of the members still there are kept.
        let mut pool: Vec<((u32, String), usize)> = match ses.plugins.containers.remove(&key) {
            Some(old) => old.shape.into_iter().zip(old.keys).filter_map(|(s, k)| Some((s, k?))).collect(),
            None => Vec::new(),
        };
        let mut keys = Vec::new();
        let channel = k == CHANNEL;
        for m in &list {
            if let Some(at) = pool.iter().position(|(s, _)| (s.0, s.1.as_str()) == (m.id, m.uid.as_str())) {
                keys.push(Some(pool.remove(at).1));
                continue;
            }
            let state = (!m.state.is_empty()).then_some(m.state.as_slice());
            // An instrument plays in a channel's container only.
            let made = if !m.instrument {
                ses.plugins.effect_index_of_uid(&m.uid).and_then(|i| ses.plugins.instantiate_effect(i, state).ok())
            } else if channel {
                ses.plugins.index_of_uid(&m.uid).and_then(|i| ses.plugins.instantiate(i, state).ok())
            } else {
                None
            };
            match made {
                Some((plugin, _)) => {
                    let k = MEMBER_KEY + ses.plugins.next_member;
                    ses.plugins.next_member += 1;
                    ses.plugins.hosted.insert(k, Hosted::new(plugin));
                    keys.push(Some(k));
                }
                None => {
                    missing.push(m.name.clone());
                    keys.push(None);
                }
            }
        }
        for (_, k) in pool {
            ses.plugins.drop_key(k);
        }
        let hosted = &ses.plugins.hosted;
        let fx = list
            .iter()
            .zip(&keys)
            .filter_map(|(m, k)| {
                let plugin = hosted.get(&(*k)?)?.plugin.clone();
                Some(if m.instrument {
                    chain::Member::instrument(m.id, plugin as os_engine::SharedInstrument, m.wet, m.dry)
                } else {
                    chain::Member::new(m.id, plugin as os_engine::SharedEffect, m.wet, m.dry).with_smart(m.cpu.smart_disable)
                })
            })
            .collect();
        let live = Live { id, src, shape: shape(&list), channel, keys, fx: Arc::new(Mutex::new(Chain::new(fx))) };
        ses.plugins.containers.insert(key, live);
        channels |= channel;
        changed = true;
    }
    let note = (!missing.is_empty()).then(|| format!("Container: not found, left out of the chain: {}", missing.join(", ")));
    if channels {
        ses.transport.set_instruments(ses.plugins.instruments());
    }
    if changed {
        // Which member each hosted key is, for its parameters' targets and links (`params::target_of`).
        let members = &mut ses.plugin_members;
        members.clear();
        for (&slot_key, live) in &ses.plugins.containers {
            let Some((t, k)) = key_place(slot_key) else { continue };
            for ((id, _), key) in live.shape.iter().zip(&live.keys) {
                if let Some(key) = key {
                    members.insert(*key, (t, k, *id));
                }
            }
        }
        ses.transport.set_effects(ses.plugins.effects());
        ses.dirty = true;
    }
    note
}

