// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The plugin wrapper's state: event 213 of a plugin whose internal name (201) is [`WRAPPER`], which hosts every
//! third-party plugin (VST2, VST3, CLAP). A `u32` type, then `(u32 id, u64 size, bytes)` records. Unknown records
//! are kept verbatim when a state is changed.
//!
// spec: flp/events/plugins.md §4

use os_model::PluginRef;

/// The wrapper's internal name (event 201): a file-format identifier.
pub const WRAPPER: &str = "Fruity Wrapper";

/// Record ids (spec: flp/events/plugins.md §4).
pub mod rec {
    /// MIDI settings: i32 input port, i32 output port, u32 pitch-bend range in semitones, ...
    pub const MIDI: u32 = 1;
    /// Audio ports: u32 number of input buses, u32 number of output buses, two u32 0.
    pub const IO: u32 = 30;
    /// One 12-byte entry per input bus: i32 port, u32 active, u32 0.
    pub const INPUTS: u32 = 31;
    /// The same per output bus.
    pub const OUTPUTS: u32 = 32;
    pub const INFO: u32 = 50;
    pub const FOURCC: u32 = 51;
    pub const GUID: u32 = 52;
    pub const STATE: u32 = 53;
    pub const NAME: u32 = 54;
    pub const PATH: u32 = 55;
    pub const VENDOR: u32 = 56;
    /// A VST2 plugin's VST version (2400 in every VST2 wrapper state FL Studio wrote that has been seen).
    pub const VST_VERSION: u32 = 57;
}

/// What Lemon Studio writes as a new VST2 wrapper state's VST version.
const VST2_VERSION: u32 = 2400;

/// The type word Lemon Studio writes for a new wrapper state.
pub const NEW_TYPE: u32 = 10;

/// A parsed wrapper state: the type word and the records in file order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Records {
    pub kind: u32,
    pub records: Vec<(u32, Vec<u8>)>,
}

impl Records {
    /// Parses a whole wrapper state; `None` when the bytes are not exactly a type word and records.
    pub fn parse(data: &[u8]) -> Option<Records> {
        let kind = u32::from_le_bytes(data.get(0..4)?.try_into().ok()?);
        let mut records = Vec::new();
        let mut p = 4;
        while p < data.len() {
            let id = u32::from_le_bytes(data.get(p..p + 4)?.try_into().ok()?);
            let size = u64::from_le_bytes(data.get(p + 4..p + 12)?.try_into().ok()?);
            let size = usize::try_from(size).ok()?;
            let body = data.get(p + 12..p.checked_add(12)?.checked_add(size)?)?;
            records.push((id, body.to_vec()));
            p += 12 + size;
        }
        Some(Records { kind, records })
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = self.kind.to_le_bytes().to_vec();
        for (id, body) in &self.records {
            out.extend(id.to_le_bytes());
            out.extend((body.len() as u64).to_le_bytes());
            out.extend(body);
        }
        out
    }

    pub fn get(&self, id: u32) -> Option<&[u8]> {
        self.records.iter().find(|(i, _)| *i == id).map(|(_, b)| b.as_slice())
    }

    /// Replaces record `id`'s bytes, or adds the record before the first one with a higher id.
    pub fn set(&mut self, id: u32, body: Vec<u8>) {
        if let Some(r) = self.records.iter_mut().find(|(i, _)| *i == id) {
            r.1 = body;
            return;
        }
        let at = self.records.iter().position(|(i, _)| *i > id).unwrap_or(self.records.len());
        self.records.insert(at, (id, body));
    }

    pub fn remove(&mut self, id: u32) {
        self.records.retain(|(i, _)| *i != id);
    }
}

fn text(b: Option<&[u8]>) -> String {
    let b = b.unwrap_or(&[]);
    String::from_utf8_lossy(b.strip_suffix(&[0]).unwrap_or(b)).into_owned()
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02X}")).collect()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

/// The format of a wrapped plugin: by the file extension, else a VST2 id record = VST2, else a class-id record = VST3,
/// else VST2. FL Studio 2026 keeps a 16-byte class-id record next to some VST2 plugins' id, so the id decides first
/// (spec: plugins.md §4).
fn format_of(r: &Records, path: &str) -> &'static str {
    let lower = path.to_ascii_lowercase();
    if lower.ends_with(".vst3") {
        "vst3"
    } else if lower.ends_with(".clap") {
        "clap"
    } else if r.get(rec::FOURCC).is_some() {
        "vst2"
    } else if r.get(rec::GUID).is_some_and(|g| g.len() == 16) {
        "vst3"
    } else {
        "vst2"
    }
}

/// A VST2 unique id record (a little-endian 32-bit number: `'4FTP'` is stored `PTF4`) as 8 hex digits, `34465450`.
fn vst2_id(b: &[u8]) -> Option<String> {
    Some(format!("{:08X}", u32::from_le_bytes(b.try_into().ok()?)))
}

/// The pitch-bend range (semitones) a wrapper state keeps for its plugin (record 1, the third field). `None` when
/// `data` is not a wrapper state or has no such record.
pub fn bend_range(data: &[u8]) -> Option<u32> {
    let r = Records::parse(data)?;
    Some(u32::from_le_bytes(r.get(rec::MIDI)?.get(8..12)?.try_into().ok()?))
}

/// The plugin a wrapper state names. `None` when `data` is not a wrapper state.
pub fn decode(data: &[u8]) -> Option<PluginRef> {
    let r = Records::parse(data)?;
    let path = text(r.get(rec::PATH));
    let format = format_of(&r, &path);
    let id = match format {
        "vst3" => r.get(rec::GUID).filter(|g| g.len() == 16).map(hex).unwrap_or_default(),
        "vst2" => r.get(rec::FOURCC).and_then(vst2_id).unwrap_or_default(),
        _ => String::new(),
    };
    Some(PluginRef {
        format: format.into(),
        id,
        name: text(r.get(rec::NAME)),
        vendor: text(r.get(rec::VENDOR)),
        path,
        state: r.get(rec::STATE).unwrap_or(&[]).to_vec(),
    })
}

/// A wrapper state for plugin `p`. With a `base` wrapper state (the one the file held), only the identity and state
/// records are replaced and every other record keeps its bytes and place; without one, a new state is built.
pub fn encode(base: Option<&[u8]>, p: &PluginRef) -> Vec<u8> {
    let mut r = base.and_then(Records::parse).unwrap_or_else(|| Records {
        kind: NEW_TYPE,
        records: vec![(rec::INFO, vec![0; 16])],
    });
    if p.format == "vst3" {
        if let Some(g) = unhex(&p.id).filter(|g| g.len() == 16) {
            r.set(rec::GUID, g);
        }
    } else {
        r.remove(rec::GUID);
    }
    if p.format == "vst2" {
        if let Ok(id) = u32::from_str_radix(&p.id, 16) {
            r.set(rec::FOURCC, id.to_le_bytes().to_vec());
        }
        if r.get(rec::VST_VERSION).is_none() {
            r.set(rec::VST_VERSION, VST2_VERSION.to_le_bytes().to_vec());
        }
    } else {
        r.remove(rec::FOURCC);
        r.remove(rec::VST_VERSION);
    }
    r.set(rec::STATE, p.state.clone());
    r.set(rec::NAME, p.name.as_bytes().to_vec());
    r.set(rec::PATH, p.path.as_bytes().to_vec());
    if !p.vendor.is_empty() || r.get(rec::VENDOR).is_some() {
        r.set(rec::VENDOR, p.vendor.as_bytes().to_vec());
    }
    let out = r.to_bytes();
    // An unchanged plugin keeps the exact bytes it had.
    match base {
        Some(b) if decode(b).as_ref() == Some(p) => b.to_vec(),
        _ => out,
    }
}

/// The port of each input bus (record 31) in the plugin's bus order: the number of a track's input counted from 1, 0
/// for none (spec: flp/events/plugins.md "FL 26's own VST3 wrapper"). Empty when the state has no audio ports.
pub fn input_ports(data: &[u8]) -> Vec<i32> {
    let Some(r) = Records::parse(data) else { return Vec::new() };
    r.get(rec::INPUTS).unwrap_or(&[]).chunks_exact(12).map(|e| i32::from_le_bytes([e[0], e[1], e[2], e[3]])).collect()
}

/// `data` with input bus `bus`'s port set to `port`; `None` when `data` is not a wrapper state. A state without audio
/// ports gets them as FL lays them out (the bus counts, then each bus with port 0 and active 1; Lemon Studio writes
/// `bus + 1` input buses and one output bus), unless the port is 0, which every bus has already.
pub fn set_input_port(data: &[u8], bus: usize, port: i32) -> Option<Vec<u8>> {
    let mut r = Records::parse(data)?;
    let entry = |port: i32| [port.to_le_bytes(), 1u32.to_le_bytes(), [0; 4]].concat();
    let Some(ins) = r.get(rec::INPUTS) else {
        if port == 0 {
            return Some(data.to_vec());
        }
        let ins = bus as u32 + 1;
        r.set(rec::IO, [ins.to_le_bytes(), 1u32.to_le_bytes(), [0; 4], [0; 4]].concat());
        r.set(rec::INPUTS, (0..=bus).flat_map(|b| entry(if b == bus { port } else { 0 })).collect());
        r.set(rec::OUTPUTS, entry(0));
        return Some(r.to_bytes());
    };
    let mut ins = ins.to_vec();
    while ins.len() / 12 <= bus {
        ins.extend(entry(0));
    }
    ins[bus * 12..bus * 12 + 4].copy_from_slice(&port.to_le_bytes());
    if let Some(io) = r.get(rec::IO).filter(|io| io.len() >= 4) {
        let mut io = io.to_vec();
        let n = u32::from_le_bytes([io[0], io[1], io[2], io[3]]).max((ins.len() / 12) as u32);
        io[0..4].copy_from_slice(&n.to_le_bytes());
        r.set(rec::IO, io);
    }
    r.set(rec::INPUTS, ins);
    Some(r.to_bytes())
}

/// The name a wrapper state shows for its plugin (record 54), if `data` is one.
pub fn plugin_name(data: &[u8]) -> Option<String> {
    Records::parse(data).map(|r| text(r.get(rec::NAME))).filter(|n| !n.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clap() -> PluginRef {
        PluginRef {
            format: "clap".into(),
            id: String::new(),
            name: "Synth".into(),
            vendor: "Maker".into(),
            path: "C:\\Plugins\\Synth.clap".into(),
            state: vec![1, 2, 3, 4],
        }
    }

    #[test]
    fn new_state_reads_back() {
        let b = encode(None, &clap());
        assert_eq!(&b[0..4], &NEW_TYPE.to_le_bytes());
        assert_eq!(decode(&b), Some(clap()));
        assert_eq!(plugin_name(&b).as_deref(), Some("Synth"));
        let mut v = clap();
        v.format = "vst3".into();
        v.path = "C:\\Plugins\\Synth.vst3".into();
        v.id = "000102030405060708090A0B0C0D0E0F".into();
        let b = encode(None, &v);
        assert_eq!(Records::parse(&b).unwrap().get(rec::GUID), Some(&(0u8..16).collect::<Vec<_>>()[..]));
        assert_eq!(decode(&b), Some(v));
    }

    /// The pitch-bend range is the MIDI record's third field (FL Studio's states hold 12 or 2).
    #[test]
    fn bend_range_from_the_midi_record() {
        let midi = [(-1i32).to_le_bytes(), (-1i32).to_le_bytes(), 12u32.to_le_bytes(), [0; 4], [0; 4]].concat();
        let r = Records { kind: 10, records: vec![(rec::MIDI, midi), (rec::NAME, b"Synth".to_vec())] };
        assert_eq!(bend_range(&r.to_bytes()), Some(12));
        assert_eq!(bend_range(&encode(None, &clap())), None, "no MIDI record");
        assert_eq!(bend_range(&[1, 2]), None);
    }

    #[test]
    fn changing_a_state_keeps_the_other_records() {
        // A VST2-style state: flags, info, fourcc, name, path (records as a plugin database entry has them).
        let base = Records {
            kind: 8,
            records: vec![
                (2, vec![0xFF; 25]),
                (rec::INFO, vec![0; 16]),
                (rec::FOURCC, vec![1, 2, 3, 4]),
                (rec::NAME, b"Old".to_vec()),
                (rec::PATH, b"C:\\VST\\Old.dll".to_vec()),
            ],
        }
        .to_bytes();
        let mut p = decode(&base).unwrap();
        assert_eq!((p.format.as_str(), p.id.as_str(), p.name.as_str()), ("vst2", "04030201", "Old"));
        assert!(p.state.is_empty());
        // Unchanged: the same bytes.
        assert_eq!(encode(Some(&base), &p), base);
        p.state = vec![9; 7];
        let out = Records::parse(&encode(Some(&base), &p)).unwrap();
        assert_eq!(out.kind, 8);
        let ids: Vec<u32> = out.records.iter().map(|r| r.0).collect();
        assert_eq!(ids, [2, rec::INFO, rec::FOURCC, rec::STATE, rec::NAME, rec::PATH, rec::VST_VERSION]);
        assert_eq!(out.get(rec::FOURCC), Some(&[1, 2, 3, 4][..]));
        assert_eq!(out.get(2), Some(&[0xFF; 25][..]));
        assert_eq!(out.get(rec::STATE), Some(&[9; 7][..]));
    }

    /// FL Studio keeps a VST2 plugin's unique id little-endian (TruePianos, `'4FTP'`, is stored `PTF4`).
    #[test]
    fn vst2_ids_are_stored_as_fl_studio_stores_them() {
        let p = PluginRef {
            format: "vst2".into(),
            id: "34465450".into(),
            name: "TruePianos".into(),
            vendor: "4Front".into(),
            path: "C:\\VstPlugins\\TruePianos x64.dll".into(),
            state: vec![0xF7, 0xFF, 0xFF, 0xFF, 13],
        };
        let b = encode(None, &p);
        let r = Records::parse(&b).unwrap();
        assert_eq!(r.get(rec::FOURCC), Some(&b"PTF4"[..]));
        assert_eq!(r.get(rec::VST_VERSION), Some(&2400u32.to_le_bytes()[..]));
        assert_eq!(r.get(rec::GUID), None);
        assert_eq!(decode(&b), Some(p.clone()));
        // The same channel saved as a VST3 plugin: no VST2 records left.
        let v3 = PluginRef { format: "vst3".into(), id: "000102030405060708090A0B0C0D0E0F".into(), path: "C:\\VST3\\TruePianos.vst3".into(), ..p };
        let r = Records::parse(&encode(Some(&b), &v3)).unwrap();
        assert_eq!((r.get(rec::FOURCC), r.get(rec::VST_VERSION)), (None, None));
        assert_eq!(decode(&r.to_bytes()).unwrap().format, "vst3");
    }

    /// FL Studio 2026 saved Krush (a VST2 plugin) with a 16-byte class-id record next to its VST2 id: still VST2. A
    /// class id without a VST2 id or a known extension is VST3.
    #[test]
    fn the_vst2_id_decides_before_a_class_id() {
        let mut r = Records { kind: 12, records: vec![] };
        r.set(rec::FOURCC, b"rKkt".to_vec());
        r.set(rec::GUID, b"tTSVKkkrrush\0\0\0\0".to_vec());
        r.set(rec::NAME, b"Krush".to_vec());
        r.set(rec::PATH, b"C:\\VSTs\\Krush.dll".to_vec());
        let p = decode(&r.to_bytes()).unwrap();
        assert_eq!((p.format.as_str(), p.id.as_str()), ("vst2", "746B4B72"));
        r.remove(rec::FOURCC);
        r.remove(rec::PATH);
        assert_eq!(decode(&r.to_bytes()).unwrap().format, "vst3");
    }

    /// Ports read from record 31 and set the FL way; a state without audio ports gets them only for a port that is not
    /// 0, before record 50 as FL orders them.
    #[test]
    fn input_ports_round_trip() {
        let b = encode(None, &clap());
        assert!(input_ports(&b).is_empty());
        assert_eq!(set_input_port(&b, 1, 0).unwrap(), b);
        let s = set_input_port(&b, 1, 2).unwrap();
        assert_eq!(input_ports(&s), [0, 2]);
        let r = Records::parse(&s).unwrap();
        assert_eq!(r.records.iter().map(|x| x.0).take(4).collect::<Vec<_>>(), [30, 31, 32, 50]);
        assert_eq!(r.get(rec::IO).unwrap(), [2u32.to_le_bytes(), 1u32.to_le_bytes(), [0; 4], [0; 4]].concat());
        assert_eq!(r.get(rec::OUTPUTS).unwrap(), [0i32.to_le_bytes(), 1u32.to_le_bytes(), [0; 4]].concat());
        assert_eq!(decode(&s), Some(clap()));
        // A state FL wrote: only the port changes, the active flag and the rest stay.
        let fl = Records {
            kind: 12,
            records: vec![
                (rec::IO, [2u32.to_le_bytes(), 1u32.to_le_bytes(), [0; 4], [0; 4]].concat()),
                (rec::INPUTS, [[0; 4], 1u32.to_le_bytes(), [0; 4], 1i32.to_le_bytes(), 0u32.to_le_bytes(), [0; 4]].concat()),
                (rec::OUTPUTS, [[0; 4], 1u32.to_le_bytes(), [0; 4]].concat()),
                (rec::NAME, b"Synth".to_vec()),
            ],
        };
        let s = set_input_port(&fl.to_bytes(), 1, 3).unwrap();
        let r = Records::parse(&s).unwrap();
        assert_eq!(input_ports(&s), [0, 3]);
        assert_eq!(&r.get(rec::INPUTS).unwrap()[16..20], &0u32.to_le_bytes());
        assert_eq!(r.records.len(), 4);
        assert_eq!(set_input_port(&[1, 2], 1, 1), None);
    }

    #[test]
    fn rejects_other_states() {
        assert_eq!(decode(&[1, 2, 3]), None);
        // A record running past the end.
        let mut b = 10u32.to_le_bytes().to_vec();
        b.extend(53u32.to_le_bytes());
        b.extend(100u64.to_le_bytes());
        b.extend([0; 10]);
        assert_eq!(decode(&b), None);
    }
}
