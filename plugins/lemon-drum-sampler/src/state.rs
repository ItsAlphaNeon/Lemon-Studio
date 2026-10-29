// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The original's saved state (an FPC channel's plugin state, event 213): a 4-byte head, then chunks of a u32 id, a
//! u64 size and the payload. A pad (chunk 2) and a layer (chunk 0x69) hold chunks of their own; every other chunk is
//! kept as its bytes. Writing the tree back gives the bytes it was read from, so what Lemon Drum Sampler does not change
//! stays as the file had it.
//!
// spec: plugins/fpc.md §1

/// The head of every state seen.
pub const HEAD: [u8; 4] = [0x03, 0xe4, 0x84, 0x1e];

/// Top-level chunks.
pub const PAD: u32 = 2;
pub const PATTERN: u32 = 1;
/// A pad's chunks.
pub const P_SETTINGS: u32 = 0x64;
pub const P_NAME: u32 = 0x65;
pub const P_FLAGS: u32 = 0x66;
pub const P_ENV_NO: u32 = 0x67;
pub const P_ENV: u32 = 0x68;
pub const P_LAYER: u32 = 0x69;
pub const P_END: u32 = 0x6a;
/// A layer's chunks.
pub const L_PATH: u32 = 0x12c;
pub const L_SETTINGS: u32 = 0x12d;
pub const L_END: u32 = 0x12e;

/// Fields of a pad's settings (chunk 0x64, i32 at these offsets).
pub const PAD_VOLUME: usize = 0;
pub const PAD_PAN: usize = 4;
/// The pad's switches: bit 0 mute, bit 1 solo, bit 3 layers take turns (measured); other bits kept.
pub const PAD_SWITCHES: usize = 8;
pub const PAD_KEY: usize = 12;
pub const PAD_OUTPUT: usize = 16;
pub const PAD_CUT: usize = 20;
pub const PAD_CUT_BY: usize = 24;
pub const PAD_TUNE: usize = 28;
pub const PAD_COLOR: usize = 32;
pub const PAD_ICON: usize = 36;
/// Fields of a layer's settings (chunk 0x12d).
pub const L_VOLUME: usize = 0;
pub const L_PAN: usize = 4;
pub const L_VEL_LO: usize = 8;
pub const L_VEL_HI: usize = 12;
pub const L_TUNE: usize = 20;
/// Pad flags (chunk 0x66's first i32).
/// The first word of a new pad's flags chunk (0x66); its meaning was not found (spec fpc.md §2).
pub const NEW_FLAGS: i32 = 1;
pub const SW_MUTE: i32 = 1;
pub const SW_SOLO: i32 = 2;
pub const SW_TAKE_TURNS: i32 = 8;

/// Pads in a state: banks A and B of 16.
pub const PADS: usize = 32;
/// The keys a new instance's pads play, bank A from the bottom left (pad 1) to the top right, then bank B.
pub const NEW_KEYS: [i32; PADS] = [
    37, 36, 42, 54, 40, 38, 46, 44, 48, 47, 45, 43, 49, 55, 51, 53, 60, 61, 62, 63, 64, 65, 66, 67, 68, 69, 70, 71, 72, 73, 74, 75,
];
/// A new pad's volume and its layer's.
pub const NEW_PAD_VOLUME: i32 = 99;
pub const NEW_LAYER_VOLUME: i32 = 127;

/// A chunk: bytes, or chunks of its own (pads and layers).
#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    Leaf(u32, Vec<u8>),
    Tree(u32, Vec<Node>),
}

impl Node {
    pub fn id(&self) -> u32 {
        match self {
            Node::Leaf(i, _) | Node::Tree(i, _) => *i,
        }
    }

    fn write(&self, out: &mut Vec<u8>) {
        out.extend(self.id().to_le_bytes());
        match self {
            Node::Leaf(_, b) => {
                out.extend((b.len() as u64).to_le_bytes());
                out.extend(b);
            }
            Node::Tree(_, kids) => {
                let at = out.len();
                out.extend(0u64.to_le_bytes());
                for k in kids {
                    k.write(out);
                }
                let n = (out.len() - at - 8) as u64;
                out[at..at + 8].copy_from_slice(&n.to_le_bytes());
            }
        }
    }
}

/// Splits `b` into chunks; `None` when it does not split exactly. Chunks with id `tree(id)` are read as trees (kept
/// as bytes when their payload does not split).
fn split(b: &[u8], tree: &dyn Fn(u32) -> bool) -> Option<Vec<Node>> {
    let mut out = Vec::new();
    let mut p = 0usize;
    while p < b.len() {
        let id = u32::from_le_bytes(b.get(p..p + 4)?.try_into().ok()?);
        let n = usize::try_from(u64::from_le_bytes(b.get(p + 4..p + 12)?.try_into().ok()?)).ok()?;
        let body = b.get(p + 12..(p + 12).checked_add(n)?)?;
        out.push(match tree(id).then(|| split(body, &|i| i == P_LAYER && id == PAD)).flatten() {
            Some(kids) => Node::Tree(id, kids),
            None => Node::Leaf(id, body.to_vec()),
        });
        p += 12 + n;
    }
    Some(out)
}

fn i32_at(b: &[u8], at: usize) -> i32 {
    b.get(at..at + 4).map_or(0, |x| i32::from_le_bytes(x.try_into().unwrap()))
}

fn set_i32_at(b: &mut Vec<u8>, at: usize, v: i32) {
    if b.len() < at + 4 {
        b.resize(at + 4, 0);
    }
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

/// UTF-16 text of a chunk.
pub fn text(b: &[u8]) -> String {
    let w: Vec<u16> = b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    String::from_utf16_lossy(&w).trim_end_matches('\0').to_string()
}

fn utf16(s: &str) -> Vec<u8> {
    s.encode_utf16().flat_map(|c| c.to_le_bytes()).collect()
}

/// One layer as the voices play it.
#[derive(Clone, Debug, PartialEq)]
pub struct Layer {
    /// The sample as stored (FL's placeholders kept); "" = no sample.
    pub path: String,
    pub volume: i32,
    pub pan: i32,
    pub vel_lo: i32,
    pub vel_hi: i32,
    pub tune: i32,
}

/// One pad as the voices play it.
#[derive(Clone, Debug, PartialEq)]
pub struct Pad {
    pub name: String,
    pub key: i32,
    pub volume: i32,
    pub pan: i32,
    pub tune: i32,
    pub output: i32,
    /// Cut group (−1 = none) and the group that cuts this pad (−1 = none).
    pub cut: i32,
    pub cut_by: i32,
    pub color: u32,
    pub flags: i32,
    /// Mute, solo, layers taking turns (`SW_*`).
    pub switches: i32,
    pub layers: Vec<Layer>,
}

impl Pad {
    /// The layers that hold a sample.
    pub fn sounding(&self) -> impl Iterator<Item = (usize, &Layer)> {
        self.layers.iter().enumerate().filter(|(_, l)| !l.path.is_empty())
    }
}

/// The saved state as a chunk tree.
#[derive(Clone, Debug, PartialEq)]
pub struct State {
    pub head: [u8; 4],
    pub nodes: Vec<Node>,
}

impl Default for State {
    /// A new instance: 32 empty pads on the General MIDI drum keys (bank B on C5 and up), then the original's
    /// closing chunks (spec: plugins/lemon-drum-sampler.md §1).
    fn default() -> Self {
        let mut nodes: Vec<Node> = NEW_KEYS.iter().map(|&k| new_pad(k)).collect();
        nodes.push(Node::Leaf(PATTERN, Vec::new()));
        nodes.push(Node::Leaf(3, vec![1]));
        nodes.push(Node::Leaf(5, vec![1]));
        nodes.push(Node::Leaf(4, vec![0; 32]));
        State { head: HEAD, nodes }
    }
}

/// A new pad's chunks on key `key`.
pub fn new_pad(key: i32) -> Node {
    let mut s = vec![0u8; 64];
    for (at, v) in [(PAD_VOLUME, NEW_PAD_VOLUME), (PAD_KEY, key), (PAD_CUT, -1), (PAD_CUT_BY, -1)] {
        set_i32_at(&mut s, at, v);
    }
    let mut flags = vec![0u8; 32];
    set_i32_at(&mut flags, 0, NEW_FLAGS);
    Node::Tree(
        PAD,
        vec![
            Node::Leaf(P_SETTINGS, s),
            Node::Leaf(P_NAME, Vec::new()),
            Node::Leaf(P_FLAGS, flags),
            new_layer(""),
            Node::Leaf(P_ENV_NO, [0u32, 0].iter().flat_map(|v| v.to_le_bytes()).collect()),
            Node::Leaf(P_ENV, env_bytes(&VOLUME_ENV)),
            Node::Leaf(P_ENV_NO, [1u32, 0].iter().flat_map(|v| v.to_le_bytes()).collect()),
            Node::Leaf(P_ENV, env_bytes(&PAN_ENV)),
            Node::Leaf(P_END, Vec::new()),
        ],
    )
}

/// A layer playing `path` over all velocities at full volume.
pub fn new_layer(path: &str) -> Node {
    let mut s = vec![0u8; 28];
    for (at, v) in [(L_VOLUME, NEW_LAYER_VOLUME), (L_VEL_HI, 127), (16, 1), (24, -1)] {
        set_i32_at(&mut s, at, v);
    }
    Node::Tree(P_LAYER, vec![Node::Leaf(L_PATH, utf16(path)), Node::Leaf(L_SETTINGS, s), Node::Leaf(L_END, Vec::new())])
}

/// An envelope record as a new pad stores it (spec: plugins/fpc.md §1): three head ints (the last is the number of
/// points), 24 bytes per point (f64 time, f64 level, f32 tension, u32 flags), then nine tail ints.
struct Env {
    head: [i32; 3],
    points: &'static [(f64, f64, f32, u32)],
    tail: [i32; 9],
}

const VOLUME_ENV: Env = Env {
    head: [1, 3, 4],
    points: &[(0.0, 0.0, 0.0, 0), (0.75, 1.0, 0.2, 1 << 24), (1.5, 0.5, 0.2, 1 << 24), (0.25, 0.0, 0.2, 1 << 24)],
    tail: [0, 0, 1, -1, 2, 128, 128, 0, 128],
};
const PAN_ENV: Env = Env { head: [1, 3, 1], points: &[(0.0, 0.5, 0.2, 0)], tail: [0, 0, -1, -1, -1, 128, 128, 0, 128] };

fn env_bytes(e: &Env) -> Vec<u8> {
    let mut b = Vec::new();
    for v in e.head {
        b.extend(v.to_le_bytes());
    }
    for &(x, y, t, f) in e.points {
        b.extend(x.to_le_bytes());
        b.extend(y.to_le_bytes());
        b.extend(t.to_le_bytes());
        b.extend(f.to_le_bytes());
    }
    for v in e.tail {
        b.extend(v.to_le_bytes());
    }
    b
}

impl State {
    /// Reads a saved state; `None` when it is not one.
    pub fn parse(b: &[u8]) -> Option<State> {
        let head: [u8; 4] = b.get(..4)?.try_into().ok()?;
        if head != HEAD {
            return None;
        }
        let nodes = split(&b[4..], &|id| id == PAD)?;
        Some(State { head, nodes })
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = self.head.to_vec();
        for n in &self.nodes {
            n.write(&mut out);
        }
        out
    }

    fn pad_nodes(&self) -> impl Iterator<Item = &Vec<Node>> {
        self.nodes.iter().filter_map(|n| match n {
            Node::Tree(PAD, kids) => Some(kids),
            _ => None,
        })
    }

    fn pad_nodes_mut(&mut self) -> impl Iterator<Item = &mut Vec<Node>> {
        self.nodes.iter_mut().filter_map(|n| match n {
            Node::Tree(PAD, kids) => Some(kids),
            _ => None,
        })
    }

    pub fn pad_count(&self) -> usize {
        self.pad_nodes().count()
    }

    /// The pads, bank A first.
    pub fn pads(&self) -> Vec<Pad> {
        self.pad_nodes().map(|k| read_pad(k)).collect()
    }

    pub fn pad(&self, i: usize) -> Option<Pad> {
        self.pad_nodes().nth(i).map(|k| read_pad(k))
    }

    /// The Pattern Manager's file the state names (kept, not used).
    pub fn pattern(&self) -> String {
        self.nodes.iter().find_map(|n| match n {
            Node::Leaf(PATTERN, b) => Some(text(b)),
            _ => None,
        })
        .unwrap_or_default()
    }

    /// Sets field `at` of pad `i`'s settings.
    pub fn set_pad(&mut self, i: usize, at: usize, v: i32) {
        if let Some(b) = self.pad_nodes_mut().nth(i).and_then(|k| leaf_mut(k, P_SETTINGS)) {
            set_i32_at(b, at, v);
        }
    }

    /// Sets pad `i`'s flags.
    pub fn set_flags(&mut self, i: usize, v: i32) {
        if let Some(b) = self.pad_nodes_mut().nth(i).and_then(|k| leaf_mut(k, P_FLAGS)) {
            set_i32_at(b, 0, v);
        }
    }

    pub fn set_name(&mut self, i: usize, name: &str) {
        if let Some(b) = self.pad_nodes_mut().nth(i).and_then(|k| leaf_mut(k, P_NAME)) {
            *b = utf16(name);
        }
    }

    /// Sets field `at` of layer `l` of pad `i`.
    pub fn set_layer(&mut self, i: usize, l: usize, at: usize, v: i32) {
        if let Some(b) = self.pad_nodes_mut().nth(i).and_then(|k| layer_mut(k, l)).and_then(|k| leaf_mut(k, L_SETTINGS)) {
            set_i32_at(b, at, v);
        }
    }

    /// Puts sample `path` on pad `i`: into its first layer, which then plays every velocity; the other layers go. A
    /// pad named after nothing takes the file's name.
    pub fn set_sample(&mut self, i: usize, path: &str) {
        let Some(kids) = self.pad_nodes_mut().nth(i) else { return };
        let first = kids.iter().position(|n| n.id() == P_LAYER).unwrap_or_else(|| kids.iter().position(|n| n.id() == P_ENV_NO).unwrap_or(kids.len()));
        kids.retain(|n| n.id() != P_LAYER);
        kids.insert(first.min(kids.len()), new_layer(path));
        let named = kids.iter().any(|n| matches!(n, Node::Leaf(P_NAME, b) if !b.is_empty()));
        if !named {
            let stem = path.rsplit(['\\', '/']).next().unwrap_or("");
            let stem = stem.rsplit_once('.').map_or(stem, |(a, _)| a);
            if let Some(b) = leaf_mut(kids, P_NAME) {
                *b = utf16(stem);
            }
        }
    }
}

fn leaf<'a>(kids: &'a [Node], id: u32) -> Option<&'a [u8]> {
    kids.iter().find_map(|n| match n {
        Node::Leaf(i, b) if *i == id => Some(b.as_slice()),
        _ => None,
    })
}

fn leaf_mut(kids: &mut [Node], id: u32) -> Option<&mut Vec<u8>> {
    kids.iter_mut().find_map(|n| match n {
        Node::Leaf(i, b) if *i == id => Some(b),
        _ => None,
    })
}

fn layer_mut(kids: &mut [Node], l: usize) -> Option<&mut Vec<Node>> {
    kids.iter_mut()
        .filter_map(|n| match n {
            Node::Tree(P_LAYER, k) => Some(k),
            _ => None,
        })
        .nth(l)
}

fn read_pad(kids: &[Node]) -> Pad {
    let s = leaf(kids, P_SETTINGS).unwrap_or(&[]);
    let layers = kids
        .iter()
        .filter_map(|n| match n {
            Node::Tree(P_LAYER, k) => {
                let ls = leaf(k, L_SETTINGS).unwrap_or(&[]);
                Some(Layer {
                    path: leaf(k, L_PATH).map(text).unwrap_or_default(),
                    volume: i32_at(ls, L_VOLUME),
                    pan: i32_at(ls, L_PAN),
                    vel_lo: i32_at(ls, L_VEL_LO),
                    vel_hi: i32_at(ls, L_VEL_HI),
                    tune: i32_at(ls, L_TUNE),
                })
            }
            _ => None,
        })
        .collect();
    Pad {
        name: leaf(kids, P_NAME).map(text).unwrap_or_default(),
        key: i32_at(s, PAD_KEY),
        volume: i32_at(s, PAD_VOLUME),
        pan: i32_at(s, PAD_PAN),
        tune: i32_at(s, PAD_TUNE),
        output: i32_at(s, PAD_OUTPUT),
        cut: i32_at(s, PAD_CUT),
        cut_by: i32_at(s, PAD_CUT_BY),
        color: i32_at(s, PAD_COLOR) as u32,
        flags: leaf(kids, P_FLAGS).map_or(NEW_FLAGS, |b| i32_at(b, 0)),
        switches: i32_at(s, PAD_SWITCHES),
        layers,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A state with one sample on pad 2 (the kick, key 36) and two velocity layers on pad 1.
    pub(crate) fn kit(kick: &str, soft: &str, hard: &str) -> State {
        let mut st = State::default();
        st.set_sample(1, kick);
        st.set_sample(0, soft);
        if let Some(Node::Tree(_, kids)) = st.nodes.get_mut(0) {
            let at = kids.iter().position(|n| n.id() == P_LAYER).unwrap() + 1;
            kids.insert(at, new_layer(hard));
        }
        st.set_layer(0, 0, L_VEL_HI, 63);
        st.set_layer(0, 1, L_VEL_LO, 64);
        st
    }

    #[test]
    fn a_new_instance_has_32_empty_pads_on_the_drum_keys() {
        let st = State::default();
        let b = st.to_bytes();
        assert_eq!(&b[..4], &HEAD);
        // Each pad's chunk holds 500 bytes, as the original's empty pads do.
        assert_eq!(u64::from_le_bytes(b[8..16].try_into().unwrap()), 500);
        let back = State::parse(&b).unwrap();
        assert_eq!(back, st);
        let pads = back.pads();
        assert_eq!(pads.len(), PADS);
        assert_eq!((pads[0].key, pads[1].key, pads[2].key, pads[16].key), (37, 36, 42, 60));
        assert_eq!((pads[0].volume, pads[0].cut, pads[0].cut_by, pads[0].flags), (99, -1, -1, NEW_FLAGS));
        assert_eq!(pads[0].layers.len(), 1);
        assert_eq!(pads[0].sounding().count(), 0);
        assert_eq!(b.len(), 4 + 32 * 512 + 12 + 13 + 13 + 44);
    }

    #[test]
    fn samples_layers_and_fields_read_back() {
        let st = kit("C:\\Kits\\kick.wav", "soft.wav", "hard.wav");
        let b = st.to_bytes();
        let st = State::parse(&b).unwrap();
        assert_eq!(st.to_bytes(), b);
        let kick = st.pad(1).unwrap();
        assert_eq!((kick.name.as_str(), kick.layers[0].path.as_str()), ("kick", "C:\\Kits\\kick.wav"));
        let p = st.pad(0).unwrap();
        assert_eq!(p.layers.iter().map(|l| (l.vel_lo, l.vel_hi)).collect::<Vec<_>>(), [(0, 63), (64, 127)]);
        // A new sample replaces the pad's layers.
        let mut st2 = st.clone();
        st2.set_sample(0, "one.wav");
        assert_eq!(st2.pad(0).unwrap().layers.len(), 1);
        assert_eq!(st2.pad(0).unwrap().name, "soft");
    }

    #[test]
    fn bytes_that_are_not_a_state_are_refused_and_unknown_chunks_kept() {
        assert!(State::parse(&[1, 2, 3, 4, 5]).is_none());
        let mut b = State::default().to_bytes();
        b.extend(9u32.to_le_bytes());
        b.extend(3u64.to_le_bytes());
        b.extend([7, 8, 9]);
        assert_eq!(State::parse(&b).unwrap().to_bytes(), b);
        // A size past the end: not a state.
        b.truncate(b.len() - 1);
        assert!(State::parse(&b).is_none());
    }

    /// The FL 26 demo projects' FPC states read and write back byte for byte (`OS_FPC_STATES` = a folder of states
    /// saved from them, read locally).
    #[test]
    fn corpus_states_round_trip() {
        let Ok(dir) = std::env::var("OS_FPC_STATES") else { return };
        let mut n = 0;
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let b = std::fs::read(e.path()).unwrap();
            let st = State::parse(&b).unwrap_or_else(|| panic!("{}", e.path().display()));
            assert_eq!(st.to_bytes(), b, "{}", e.path().display());
            assert_eq!(st.pad_count(), PADS);
            n += 1;
        }
        assert!(n > 0);
    }
}
