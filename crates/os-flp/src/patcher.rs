// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The original's Patcher state: event 213 of a channel or mixer slot whose plugin is [`PATCHER`]. A `u32` version (4),
//! then `(u32 id, u64 size, bytes)` chunks ending with chunk 1001; a chunk of id 0 is a group of chunks. The groups are
//! the nodes (each plugin with its own state stream), the original's own ports (Audio input, Audio output, a sidechain,
//! Notes) and the links between ports. What else the state holds (the map's view, node positions, surface entries) is
//! not read. [`Patch::serial_chain`] finds the patches that are one plain serial chain of effects, which Lemon Studio
//! opens as its own containers (`README.md` slice 5).
//!
// spec: plugins/patcher/state.md §1 to §7; plugins/patcher/README.md §4 slice 5

/// The Patcher's internal name (event 201).
pub const PATCHER: &str = "Patcher";

/// Chunk ids (spec: plugins/patcher/state.md §1, §3 to §5).
mod id {
    pub const GROUP: u32 = 0;
    pub const END: u32 = 1001;
    pub const NODE: u32 = 5000;
    pub const NODE_PLUGIN: u32 = 5004;
    pub const NODE_DISPLAY: u32 = 5008;
    pub const NODE_NAME: u32 = 5010;
    pub const NODE_STATE: u32 = 5005;
    pub const NODE_PORT: u32 = 5009;
    pub const PORT: u32 = 1400;
    pub const PORT_NAME: u32 = 1402;
    pub const LINK: u32 = 1300;
}

/// Port types of a node's ports (spec: plugins/patcher/state.md §3): audio in and out, a sidechain input. The others
/// (0, 10, 11) are not established; 10 and 11 are common on controllers.
pub const AUDIO_IN: u32 = 3;
pub const AUDIO_OUT: u32 = 2;
pub const SIDECHAIN_IN: u32 = 7;

/// A node header's kind for an instrument (spec: plugins/patcher/state.md §3); effects and controllers are 2.
pub const INSTRUMENT_NODE: u32 = 3;

/// The original's own ports, by name (spec: plugins/patcher/state.md §4).
pub const FL_AUDIO_IN: &str = "Audio input";
pub const FL_AUDIO_OUT: &str = "Audio output";

/// A plugin in the patch.
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub id: u32,
    /// The header's kind: [`INSTRUMENT_NODE`] for an instrument, 2 for an effect or a controller.
    pub kind: u32,
    /// The plugin's internal name ("Fruity Limiter"), as event 201 gives a slot's.
    pub plugin: String,
    /// The plugin's display name and the node's own name in the map (the user may have renamed it).
    pub display: String,
    pub name: String,
    /// The plugin's own state: the stream the plugin saves in a mixer slot or channel (event 213).
    pub state: Vec<u8>,
    /// Its ports: (port id, type).
    pub ports: Vec<(u32, u32)>,
}

/// A link: the signal from port `from` goes to port `to` at `level` (1.0 = as it is).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Link {
    pub to: u32,
    pub from: u32,
    pub level: f32,
}

/// What Lemon Studio reads of a Patcher state.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Patch {
    pub nodes: Vec<Node>,
    /// The original's own ports: (port id, name).
    pub fl_ports: Vec<(u32, String)>,
    /// The links that play, in the state's order. The original ignores a second link between the same two ports (the
    /// first one plays) and a link that closes a cycle (spec: plugins/patcher/state.md §5), so neither is kept.
    pub links: Vec<Link>,
}

/// Why a patch is not one plain serial chain of effects ([`Patch::serial_chain`]), as an opened project's hint names
/// it. Each kind needs a later slice of `README.md` §4.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotChain {
    /// No Audio input (a channel's patch has Notes in its place), or a linked instrument node: slice 3.
    Instrument,
    /// A link on a node port that is neither an audio input nor an audio output (controllers, a linked Control
    /// Surface, events): slice 4.
    Controls,
    /// A link from one of the original's sidechain ports or into a node's sidechain input.
    Sidechain,
    /// A link level other than 1.0 (a slot's mix moved into the patch, a level set on the map): slice 2.
    Level,
    /// More links than a chain: a split or a sum (slice 2).
    Parallel,
    /// Audio input does not reach Audio output.
    NoPath,
}

impl NotChain {
    /// The reason in a hint: "Insert 2, slot 1 left as it was (parallel paths)".
    pub fn text(self) -> &'static str {
        match self {
            NotChain::Instrument => "an instrument patch",
            NotChain::Controls => "controllers or a linked surface",
            NotChain::Sidechain => "a sidechain link",
            NotChain::Level => "link levels other than 1.0",
            NotChain::Parallel => "parallel paths",
            NotChain::NoPath => "no path from Audio input to Audio output",
        }
    }
}

fn chunks(b: &[u8]) -> Option<Vec<(u32, &[u8])>> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < b.len() {
        let id = u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?);
        let size = usize::try_from(u64::from_le_bytes(b.get(at + 4..at + 12)?.try_into().ok()?)).ok()?;
        let body = b.get(at + 12..at.checked_add(12)?.checked_add(size)?)?;
        out.push((id, body));
        at += 12 + size;
    }
    Some(out)
}

fn word(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

/// UTF-16 text with its terminator.
fn text(b: &[u8]) -> String {
    let units: Vec<u16> = b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&u| u != 0).collect();
    String::from_utf16_lossy(&units)
}

/// Reads a Patcher state; `None` for bytes that are not one (another version, a chunk running past the end, no end
/// chunk).
pub fn parse(b: &[u8]) -> Option<Patch> {
    if word(b, 0)? != 4 {
        return None;
    }
    let top = chunks(b.get(4..)?)?;
    // Every state ends with chunk 1001, empty (spec §1): bytes cut at a chunk's end are not taken for a whole patch.
    if top.last().is_none_or(|&(cid, body)| cid != id::END || !body.is_empty()) {
        return None;
    }
    let mut patch = Patch::default();
    for (cid, body) in top {
        if cid != id::GROUP || body.is_empty() {
            continue;
        }
        let group = chunks(body)?;
        let get = |want: u32| group.iter().find(|(c, _)| *c == want).map(|(_, d)| *d);
        match group.first()?.0 {
            id::NODE => {
                let head = get(id::NODE)?;
                patch.nodes.push(Node {
                    id: word(head, 4)?,
                    kind: word(head, 0)?,
                    plugin: text(get(id::NODE_PLUGIN).unwrap_or_default()),
                    display: text(get(id::NODE_DISPLAY).unwrap_or_default()),
                    name: text(get(id::NODE_NAME).unwrap_or_default()),
                    state: get(id::NODE_STATE).unwrap_or_default().to_vec(),
                    ports: group.iter().filter(|(c, _)| *c == id::NODE_PORT).map(|(_, d)| Some((word(d, 8)?, word(d, 16)?))).collect::<Option<_>>()?,
                });
            }
            id::PORT => patch.fl_ports.push((word(get(id::PORT)?, 0)?, text(get(id::PORT_NAME).unwrap_or_default()))),
            id::LINK => {
                let d = get(id::LINK)?;
                patch.links.push(Link { to: word(d, 0)?, from: word(d, 8)?, level: f32::from_le_bytes(d.get(16..20)?.try_into().ok()?) });
            }
            _ => {}
        }
    }
    patch.drop_ignored_links();
    Some(patch)
}

impl Patch {
    /// What a port belongs to: its node's id, or the port itself for one of the original's own ports (ids are one space,
    /// spec §2).
    fn owner(&self, port: u32) -> u32 {
        self.nodes.iter().find(|n| n.ports.iter().any(|p| p.0 == port)).map_or(port, |n| n.id)
    }

    /// Keeps the links the original plays (spec §5): a second link between the same two ports goes, and so does a link
    /// whose target already reaches its source through the links kept before it. Which link of a cycle is the one
    /// ignored is Lemon Studio's choice (the later one in the state): the render measured a link from a node back into
    /// itself only.
    fn drop_ignored_links(&mut self) {
        let mut kept: Vec<Link> = Vec::new();
        for l in std::mem::take(&mut self.links) {
            if kept.iter().any(|x| (x.to, x.from) == (l.to, l.from)) {
                continue;
            }
            let (source, target) = (self.owner(l.from), self.owner(l.to));
            let mut seen = vec![target];
            let mut todo = vec![target];
            while let Some(at) = todo.pop() {
                for x in kept.iter().filter(|x| self.owner(x.from) == at) {
                    let next = self.owner(x.to);
                    if !seen.contains(&next) {
                        seen.push(next);
                        todo.push(next);
                    }
                }
            }
            if !seen.contains(&source) {
                kept.push(l);
            }
        }
        self.links = kept;
    }

    fn fl_port(&self, name: &str) -> Option<u32> {
        self.fl_ports.iter().find(|(_, n)| n == name).map(|&(id, _)| id)
    }

    /// The node owning port `port`, and the port's type.
    fn node_port(&self, port: u32) -> Option<(usize, u32)> {
        self.nodes.iter().enumerate().find_map(|(i, n)| n.ports.iter().find(|p| p.0 == port).map(|p| (i, p.1)))
    }

    /// The nodes in the order the signal passes them, when the patch is one plain serial chain of effects: Audio input
    /// into the first node's audio input, each node's audio output into the next one's, the last one's into Audio
    /// output, every link at level 1.0, n linked nodes and n + 1 links. Nodes with no link at all (the unlinked Control
    /// Surface the original adds to a new patch, spec §7) are left out of the chain. For any other patch, why not.
    pub fn serial_chain(&self) -> Result<Vec<usize>, NotChain> {
        let fl_in = self.fl_port(FL_AUDIO_IN).ok_or(NotChain::Instrument)?;
        let fl_out = self.fl_port(FL_AUDIO_OUT).ok_or(NotChain::NoPath)?;
        let mut linked: Vec<usize> = Vec::new();
        for l in &self.links {
            // Each end: a node's port of the type the link's direction needs, or the original's own audio port.
            for (port, want, fl) in [(l.from, AUDIO_OUT, fl_in), (l.to, AUDIO_IN, fl_out)] {
                match self.node_port(port) {
                    Some((i, kind)) => {
                        if self.nodes[i].kind == INSTRUMENT_NODE {
                            return Err(NotChain::Instrument);
                        }
                        if kind == SIDECHAIN_IN {
                            return Err(NotChain::Sidechain);
                        }
                        if kind != want {
                            return Err(NotChain::Controls);
                        }
                        if !linked.contains(&i) {
                            linked.push(i);
                        }
                    }
                    None if port == fl => {}
                    // Another of the original's ports ("7 Sidechain"), or Audio output used as a source.
                    None if self.fl_ports.iter().any(|p| p.0 == port && p.1 != FL_AUDIO_IN && p.1 != FL_AUDIO_OUT) => return Err(NotChain::Sidechain),
                    None => return Err(NotChain::Parallel),
                }
            }
        }
        if self.links.iter().any(|l| l.level != 1.0) {
            return Err(NotChain::Level);
        }
        if self.links.len() != linked.len() + 1 {
            return Err(if self.links.is_empty() { NotChain::NoPath } else { NotChain::Parallel });
        }
        let from = |port: u32| -> Vec<&Link> { self.links.iter().filter(|l| l.from == port).collect() };
        let mut order = Vec::new();
        let mut source = fl_in;
        loop {
            let link = match from(source)[..] {
                [link] => link,
                [] => return Err(NotChain::NoPath),
                _ => return Err(NotChain::Parallel),
            };
            if link.to == fl_out {
                break;
            }
            let (i, _) = self.node_port(link.to).ok_or(NotChain::Parallel)?;
            if order.contains(&i) || self.links.iter().filter(|l| l.to == link.to).count() != 1 {
                return Err(NotChain::Parallel);
            }
            order.push(i);
            // The node's one linked audio output leads on.
            let outs: Vec<u32> = self.nodes[i].ports.iter().filter(|p| p.1 == AUDIO_OUT && !from(p.0).is_empty()).map(|p| p.0).collect();
            let [out] = outs[..] else { return Err(if outs.is_empty() { NotChain::NoPath } else { NotChain::Parallel }) };
            source = out;
        }
        if order.len() != linked.len() {
            return Err(NotChain::Parallel);
        }
        Ok(order)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(id: u32, body: &[u8]) -> Vec<u8> {
        let mut b = id.to_le_bytes().to_vec();
        b.extend((body.len() as u64).to_le_bytes());
        b.extend_from_slice(body);
        b
    }

    fn utf16(s: &str) -> Vec<u8> {
        s.encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect()
    }

    fn words(w: &[u32]) -> Vec<u8> {
        w.iter().flat_map(|x| x.to_le_bytes()).collect()
    }

    /// A node group as the original saves one (spec §3): header (kind `kind`), names, state, ports (in, then out), end.
    fn node_of(kind: u32, id: u32, plugin: &str, name: &str, state: &[u8], ports: &[(u32, u32)]) -> Vec<u8> {
        let mut g = chunk(id::NODE, &words(&[kind, id, 0, 0, 1, 5, 0, 0, 0, 0]));
        g.extend(chunk(id::NODE_PLUGIN, &utf16(plugin)));
        g.extend(chunk(id::NODE_DISPLAY, &utf16(plugin)));
        g.extend(chunk(id::NODE_NAME, &utf16(name)));
        g.extend(chunk(id::NODE_STATE, state));
        g.extend(chunk(5007, &[0; 40]));
        for (k, &(port, kind)) in ports.iter().enumerate() {
            g.extend(chunk(id::NODE_PORT, &words(&[0, k as u32, port, 0, kind, 0, 0])));
        }
        g.extend(chunk(5001, &[]));
        chunk(id::GROUP, &g)
    }

    fn node(id: u32, plugin: &str, name: &str, state: &[u8], ports: &[(u32, u32)]) -> Vec<u8> {
        node_of(2, id, plugin, name, state, ports)
    }

    fn fl_port(id: u32, name: &str) -> Vec<u8> {
        let mut g = chunk(id::PORT, &words(&[id, 0, 0, 7, 2, 0, 0]));
        g.extend(chunk(id::PORT_NAME, &utf16(name)));
        g.extend(chunk(1401, &[]));
        chunk(id::GROUP, &g)
    }

    fn link(to: u32, from: u32, level: f32) -> Vec<u8> {
        let mut d = words(&[to, 0, from, 0]);
        d.extend(level.to_le_bytes());
        d.extend(words(&[2, 0, 0]));
        let mut g = chunk(id::LINK, &d);
        g.extend(chunk(1301, &[]));
        chunk(id::GROUP, &g)
    }

    /// A state of `nodes`, the original's Audio input (1000) and Audio output (1001), then `links` (spec §1's order).
    fn state(nodes: &[Vec<u8>], links: &[Vec<u8>]) -> Vec<u8> {
        let mut b = 4u32.to_le_bytes().to_vec();
        b.extend(chunk(1000, &[0; 64]));
        b.extend(chunk(1002, &[0; 44]));
        nodes.iter().for_each(|n| b.extend_from_slice(n));
        b.extend(fl_port(1000, FL_AUDIO_IN));
        b.extend(fl_port(1001, FL_AUDIO_OUT));
        links.iter().for_each(|l| b.extend_from_slice(l));
        b.extend(chunk(1500, &[0; 40]));
        b.extend(chunk(id::GROUP, &[]));
        b.extend(chunk(1001, &[]));
        b
    }

    /// Audio input → Fruity Limiter (node 1004, ports 1005 in / 1006 out) → Fruity Chorus (1007, ports 1008 / 1009) →
    /// Audio output, with `extra` groups after the links.
    fn serial(levels: [f32; 3], extra: &[Vec<u8>]) -> Vec<u8> {
        let nodes = [
            node(1004, "Fruity Limiter", "Limiter", &[7, 0, 0, 0], &[(1005, AUDIO_IN), (1006, AUDIO_OUT), (1010, 0)]),
            node(1007, "Fruity Chorus", "Fruity Chorus", &[1, 2, 3], &[(1008, AUDIO_IN), (1009, AUDIO_OUT)]),
        ];
        let mut links = vec![link(1005, 1000, levels[0]), link(1008, 1006, levels[1]), link(1001, 1009, levels[2])];
        links.extend_from_slice(extra);
        state(&nodes, &links)
    }

    /// Every Patcher slot state in the corpus reads (spec §1: all 35 of the FL 26 re-saves walk), and those that are a
    /// plain chain give every linked node once.
    #[test]
    fn corpus_patcher_states_read() {
        let Some(list) = std::env::var_os("OPENSTUDIO_FLP_CORPUS") else { return };
        let text = std::fs::read_to_string(list).unwrap();
        let (mut states, mut chains) = (0, 0);
        for path in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
            let file = crate::FlpFile::parse(&std::fs::read(path).unwrap()).unwrap();
            let project = crate::project::load(&file).unwrap();
            for slot in project.mixer.iter().flat_map(|m| m.slots.iter().flatten()).filter(|s| s.name == PATCHER) {
                let p = parse(&slot.state).unwrap_or_else(|| panic!("{path}: a Patcher state reads"));
                states += 1;
                if let Ok(order) = p.serial_chain() {
                    assert!(order.len() <= p.nodes.len(), "{path}");
                    chains += 1;
                }
            }
        }
        eprintln!("{states} Patcher slot states read, {chains} of them plain chains");
    }

    // spec: plugins/patcher/state.md "1. Framing", "3. A node", "4. FL's own ports", "5. A link"
    #[test]
    fn a_serial_patch_reads_in_chain_order() {
        let p = parse(&serial([1.0; 3], &[])).expect("a patch");
        assert_eq!(p.nodes.len(), 2);
        let n = &p.nodes[0];
        assert_eq!((n.id, n.kind, n.plugin.as_str(), n.name.as_str(), n.state.as_slice()), (1004, 2, "Fruity Limiter", "Limiter", &[7, 0, 0, 0][..]));
        assert_eq!(p.nodes[1].ports, [(1008, AUDIO_IN), (1009, AUDIO_OUT)]);
        assert_eq!(p.fl_ports, [(1000, FL_AUDIO_IN.to_string()), (1001, FL_AUDIO_OUT.to_string())]);
        assert_eq!(p.links[1], Link { to: 1008, from: 1006, level: 1.0 });
        assert_eq!(p.serial_chain(), Ok(vec![0, 1]));
        // The links in another order give the same chain.
        let nodes = [node(1004, "A", "A", &[], &[(1005, AUDIO_IN), (1006, AUDIO_OUT)]), node(1007, "B", "B", &[], &[(1008, AUDIO_IN), (1009, AUDIO_OUT)])];
        let p = parse(&state(&nodes, &[link(1001, 1006, 1.0), link(1005, 1009, 1.0), link(1008, 1000, 1.0)])).unwrap();
        assert_eq!(p.serial_chain(), Ok(vec![1, 0]), "B, then A");
    }

    // spec: plugins/patcher/state.md "7. What slice 5 can do with this", "8. What Patcherize writes"
    /// The Control Surface the original adds to a new patch, unlinked, is left out of the chain; a lone link Audio input
    /// → Audio output is an empty chain.
    #[test]
    fn unlinked_nodes_are_left_out() {
        let surface = node(1002, "Control Surface", "Surface", &[], &[(1003, 11)]);
        let limiter = node(1004, "Fruity Limiter", "Limiter", &[1], &[(1005, AUDIO_IN), (1006, AUDIO_OUT)]);
        let p = parse(&state(&[surface.clone(), limiter], &[link(1005, 1000, 1.0), link(1001, 1006, 1.0)])).unwrap();
        assert_eq!(p.serial_chain(), Ok(vec![1]));
        let p = parse(&state(&[surface], &[link(1001, 1000, 1.0)])).unwrap();
        assert_eq!(p.serial_chain(), Ok(vec![]));
    }

    // spec: plugins/patcher/state.md "5. A link" (a second link between the same two ports, a link closing a cycle)
    /// The links the original ignores are not read, so the chain under them opens.
    #[test]
    fn ignored_links_are_dropped() {
        let p = parse(&serial([1.0; 3], &[link(1008, 1006, 0.25)])).unwrap();
        assert_eq!(p.links.len(), 3);
        assert_eq!(p.links[1].level, 1.0, "the first one is kept");
        assert_eq!(p.serial_chain(), Ok(vec![0, 1]));
        // The limiter's output back into its own input (the render of §5), and the chorus's output back into the limiter.
        for back in [link(1005, 1006, 0.5), link(1005, 1009, 1.0)] {
            let p = parse(&serial([1.0; 3], &[back])).unwrap();
            assert_eq!(p.links.len(), 3);
            assert_eq!(p.serial_chain(), Ok(vec![0, 1]));
        }
    }

    // spec: plugins/patcher/state.md "7. What slice 5 can do with this", "8. What Patcherize writes"
    /// Link levels, side paths, sidechains, controllers, instruments and patches that do not reach Audio output are not a
    /// plain chain, each with its reason; broken bytes are not a patch.
    #[test]
    fn other_patches_are_not_a_chain() {
        let not = |b: Vec<u8>| parse(&b).unwrap().serial_chain().unwrap_err();
        assert_eq!(not(serial([1.0, 0.5, 1.0], &[])), NotChain::Level, "a link level");
        // Patcherize of a slot at mix 50 %: a wet and a dry link (spec §8).
        let lim = || node(1004, "Fruity Limiter", "Limiter", &[], &[(1005, AUDIO_IN), (1006, AUDIO_OUT), (1007, SIDECHAIN_IN)]);
        assert_eq!(not(state(&[lim()], &[link(1005, 1000, 1.0), link(1001, 1006, 0.5), link(1001, 1000, 0.5)])), NotChain::Level, "a mix");
        // The limiter's output also goes straight to Audio output: a parallel path.
        assert_eq!(not(serial([1.0; 3], &[link(1001, 1006, 1.0)])), NotChain::Parallel, "a second path");
        // Audio input also feeds the chorus.
        assert_eq!(not(serial([1.0; 3], &[link(1008, 1000, 1.0)])), NotChain::Parallel, "two links into one input");
        // The original's "7 Sidechain" into the limiter's sidechain input (spec §4).
        let mut sc = state(&[lim()], &[link(1005, 1000, 1.0), link(1001, 1006, 1.0), link(1007, 1018, 1.0)]);
        let at = sc.len() - 12 * 3 - 40;
        sc.splice(at..at, fl_port(1018, "7 Sidechain"));
        assert_eq!(not(sc), NotChain::Sidechain, "a sidechain");
        // A controller's parameter output linked to the limiter.
        let ctrl = node(1010, "Fruity Formula Controller", "Formula", &[], &[(1011, 10)]);
        assert_eq!(not(state(&[lim(), ctrl], &[link(1005, 1000, 1.0), link(1001, 1006, 1.0), link(1007, 1011, 1.0)])), NotChain::Controls, "a controller");
        // An instrument node in an effect patch, and a channel's patch (Notes, no Audio input).
        let synth = node_of(INSTRUMENT_NODE, 1004, "3x Osc", "3x Osc", &[], &[(1005, AUDIO_IN), (1006, AUDIO_OUT)]);
        assert_eq!(not(state(&[synth], &[link(1005, 1000, 1.0), link(1001, 1006, 1.0)])), NotChain::Instrument, "an instrument node");
        let mut inst = serial([1.0; 3], &[]);
        let at = inst.windows(utf16(FL_AUDIO_IN).len()).position(|w| w == utf16(FL_AUDIO_IN)).unwrap();
        inst.splice(at..at + utf16(FL_AUDIO_IN).len(), utf16("Notes______"));
        assert_eq!(not(inst), NotChain::Instrument, "no Audio input");
        // A patch with no links at all.
        assert_eq!(not(state(&[lim()], &[])), NotChain::NoPath, "no link");
    }

    // spec: plugins/patcher/state.md "1. Framing"
    #[test]
    fn broken_bytes_are_not_a_patch() {
        let b = serial([1.0; 3], &[]);
        assert!(parse(&b).is_some());
        assert_eq!(parse(&b[..b.len() - 5]), None, "cut short");
        assert_eq!(parse(&b[..b.len() - 12]), None, "cut at a chunk's end, without the end chunk");
        let mut v3 = b.clone();
        v3[0] = 3;
        assert_eq!(parse(&v3), None, "another version");
        assert_eq!(parse(&[]), None);
    }
}
