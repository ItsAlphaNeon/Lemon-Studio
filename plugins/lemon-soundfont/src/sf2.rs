// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! SoundFont 2 files: the RIFF reader, the preset → instrument → sample hierarchy, and what one note plays (each
//! matching zone pair resolved to its final generator values and modulators, by the SoundFont 2.01 rules).
//!
// spec: plugins/lemon-soundfont.md §1

use std::path::Path;
use std::sync::Arc;

/// Generator numbers used by name.
pub mod g {
    pub const START_OFS: usize = 0;
    pub const END_OFS: usize = 1;
    pub const STARTLOOP_OFS: usize = 2;
    pub const ENDLOOP_OFS: usize = 3;
    pub const START_COARSE: usize = 4;
    pub const MODLFO_TO_PITCH: usize = 5;
    pub const VIBLFO_TO_PITCH: usize = 6;
    pub const MODENV_TO_PITCH: usize = 7;
    pub const FILTER_FC: usize = 8;
    pub const FILTER_Q: usize = 9;
    pub const MODLFO_TO_FC: usize = 10;
    pub const MODENV_TO_FC: usize = 11;
    pub const END_COARSE: usize = 12;
    pub const MODLFO_TO_VOL: usize = 13;
    pub const CHORUS: usize = 15;
    pub const REVERB: usize = 16;
    pub const PAN: usize = 17;
    pub const MODLFO_DELAY: usize = 21;
    pub const MODLFO_FREQ: usize = 22;
    pub const VIBLFO_DELAY: usize = 23;
    pub const VIBLFO_FREQ: usize = 24;
    pub const MODENV_DELAY: usize = 25;
    pub const MODENV_ATTACK: usize = 26;
    pub const MODENV_HOLD: usize = 27;
    pub const MODENV_DECAY: usize = 28;
    pub const MODENV_SUSTAIN: usize = 29;
    pub const MODENV_RELEASE: usize = 30;
    pub const KEY_TO_MODENV_HOLD: usize = 31;
    pub const KEY_TO_MODENV_DECAY: usize = 32;
    pub const VOLENV_DELAY: usize = 33;
    pub const VOLENV_ATTACK: usize = 34;
    pub const VOLENV_HOLD: usize = 35;
    pub const VOLENV_DECAY: usize = 36;
    pub const VOLENV_SUSTAIN: usize = 37;
    pub const VOLENV_RELEASE: usize = 38;
    pub const KEY_TO_VOLENV_HOLD: usize = 39;
    pub const KEY_TO_VOLENV_DECAY: usize = 40;
    pub const INSTRUMENT: usize = 41;
    pub const KEY_RANGE: usize = 43;
    pub const VEL_RANGE: usize = 44;
    pub const STARTLOOP_COARSE: usize = 45;
    pub const KEYNUM: usize = 46;
    pub const VELOCITY: usize = 47;
    pub const ATTENUATION: usize = 48;
    pub const ENDLOOP_COARSE: usize = 50;
    pub const COARSE_TUNE: usize = 51;
    pub const FINE_TUNE: usize = 52;
    pub const SAMPLE_ID: usize = 53;
    pub const SAMPLE_MODES: usize = 54;
    pub const SCALE_TUNING: usize = 56;
    pub const EXCLUSIVE_CLASS: usize = 57;
    pub const ROOT_KEY: usize = 58;
    pub const COUNT: usize = 61;
}

/// Generator defaults (SoundFont 2.01 §8.1.3).
pub fn default_gens() -> [i32; g::COUNT] {
    let mut d = [0i32; g::COUNT];
    d[g::FILTER_FC] = 13500;
    for i in [g::MODLFO_DELAY, g::VIBLFO_DELAY, g::MODENV_DELAY, g::MODENV_ATTACK, g::MODENV_HOLD, g::MODENV_DECAY, g::MODENV_RELEASE,
        g::VOLENV_DELAY, g::VOLENV_ATTACK, g::VOLENV_HOLD, g::VOLENV_DECAY, g::VOLENV_RELEASE]
    {
        d[i] = -12000;
    }
    d[g::KEY_RANGE] = 127 << 8;
    d[g::VEL_RANGE] = 127 << 8;
    d[g::KEYNUM] = -1;
    d[g::VELOCITY] = -1;
    d[g::SCALE_TUNING] = 100;
    d[g::ROOT_KEY] = -1;
    d
}

/// Generators that hold a value only an instrument zone may set (a preset zone ignores them, §8.5).
fn instrument_only(op: usize) -> bool {
    matches!(op, 0..=4 | 12 | 45 | 46 | 47 | 50 | 54 | 57 | 58)
}

/// Value range of each generator (§8.1.3), for the sum of the two levels.
fn clamp_gen(op: usize, v: i32) -> i32 {
    let (lo, hi) = match op {
        g::FILTER_FC => (1500, 13500),
        g::FILTER_Q => (0, 960),
        g::MODLFO_TO_PITCH | g::VIBLFO_TO_PITCH | g::MODENV_TO_PITCH | g::MODLFO_TO_FC | g::MODENV_TO_FC => (-12000, 12000),
        g::MODLFO_TO_VOL => (-960, 960),
        g::CHORUS | g::REVERB => (0, 1000),
        g::PAN => (-500, 500),
        g::MODLFO_DELAY | g::VIBLFO_DELAY | g::MODENV_DELAY | g::VOLENV_DELAY => (-12000, 5000),
        g::MODLFO_FREQ | g::VIBLFO_FREQ => (-16000, 4500),
        g::MODENV_ATTACK | g::VOLENV_ATTACK | g::MODENV_DECAY | g::VOLENV_DECAY | g::MODENV_RELEASE | g::VOLENV_RELEASE => (-12000, 8000),
        g::MODENV_HOLD | g::VOLENV_HOLD => (-12000, 5000),
        g::MODENV_SUSTAIN => (0, 1000),
        g::VOLENV_SUSTAIN => (0, 1440),
        g::KEY_TO_MODENV_HOLD | g::KEY_TO_MODENV_DECAY | g::KEY_TO_VOLENV_HOLD | g::KEY_TO_VOLENV_DECAY => (-1200, 1200),
        g::ATTENUATION => (0, 1440),
        g::COARSE_TUNE => (-120, 120),
        g::FINE_TUNE => (-99, 99),
        g::SCALE_TUNING => (0, 1200),
        _ => return v,
    };
    v.clamp(lo, hi)
}

/// A modulator (§8.2): source, destination generator, amount, amount source, transform.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Modulator {
    pub src: u16,
    pub dest: u16,
    pub amount: i16,
    pub amt_src: u16,
    pub transform: u16,
}

impl Modulator {
    /// Two modulators are the same one when all but the amount match (§9.5.1).
    fn same(&self, o: &Modulator) -> bool {
        self.src == o.src && self.dest == o.dest && self.amt_src == o.amt_src && self.transform == o.transform
    }
}

/// The default modulators (§8.4).
pub fn default_modulators() -> Vec<Modulator> {
    vec![
        // velocity → initial attenuation (negative unipolar concave), 960 cB
        Modulator { src: 0x0502, dest: g::ATTENUATION as u16, amount: 960, amt_src: 0, transform: 0 },
        // velocity → filter cutoff (negative unipolar linear), −2400 cents
        Modulator { src: 0x0102, dest: g::FILTER_FC as u16, amount: -2400, amt_src: 0, transform: 0 },
        // channel pressure → vibrato LFO pitch, 50 cents
        Modulator { src: 0x000D, dest: g::VIBLFO_TO_PITCH as u16, amount: 50, amt_src: 0, transform: 0 },
        // CC1 → vibrato LFO pitch, 50 cents
        Modulator { src: 0x0081, dest: g::VIBLFO_TO_PITCH as u16, amount: 50, amt_src: 0, transform: 0 },
        // CC7 → attenuation (negative unipolar concave), 960 cB
        Modulator { src: 0x0587, dest: g::ATTENUATION as u16, amount: 960, amt_src: 0, transform: 0 },
        // CC10 → pan (bipolar linear), 1000
        Modulator { src: 0x028A, dest: g::PAN as u16, amount: 1000, amt_src: 0, transform: 0 },
        // CC11 → attenuation (negative unipolar concave), 960 cB
        Modulator { src: 0x058B, dest: g::ATTENUATION as u16, amount: 960, amt_src: 0, transform: 0 },
        // CC91 → reverb send, 200
        Modulator { src: 0x00DB, dest: g::REVERB as u16, amount: 200, amt_src: 0, transform: 0 },
        // CC93 → chorus send, 200
        Modulator { src: 0x00DD, dest: g::CHORUS as u16, amount: 200, amt_src: 0, transform: 0 },
        // pitch wheel × wheel sensitivity → fine tune, 12700
        Modulator { src: 0x020E, dest: g::FINE_TUNE as u16, amount: 12700, amt_src: 0x0010, transform: 0 },
    ]
}

#[derive(Clone, Debug, Default)]
pub struct Zone {
    /// Generators the zone sets (operator, raw amount).
    pub gens: Vec<(u16, i16)>,
    pub mods: Vec<Modulator>,
}

impl Zone {
    fn get(&self, op: usize) -> Option<i16> {
        self.gens.iter().rev().find(|(o, _)| *o as usize == op).map(|(_, v)| *v)
    }

    fn range(&self, op: usize) -> (u8, u8) {
        self.get(op).map_or((0, 127), |v| ((v as u16 & 0xFF) as u8, ((v as u16) >> 8) as u8))
    }

    /// The instrument (preset zone) or sample (instrument zone) the zone plays, when it is not a global zone.
    fn link(&self, op: usize) -> Option<usize> {
        self.gens.last().filter(|(o, _)| *o as usize == op).map(|(_, v)| *v as u16 as usize)
    }
}

#[derive(Clone, Debug, Default)]
pub struct Preset {
    pub name: String,
    pub program: u16,
    pub bank: u16,
    pub global: Option<Zone>,
    pub zones: Vec<Zone>,
}

#[derive(Clone, Debug, Default)]
pub struct Instrument {
    pub name: String,
    pub global: Option<Zone>,
    pub zones: Vec<Zone>,
}

#[derive(Clone, Debug, Default)]
pub struct SampleHeader {
    pub name: String,
    pub start: u32,
    pub end: u32,
    pub loop_start: u32,
    pub loop_end: u32,
    pub rate: u32,
    pub root: u8,
    pub correction: i8,
    pub kind: u16,
}

/// A whole SoundFont: its presets (sorted by bank, then program), instruments, sample headers and sample data
/// (16-bit, as floats in −1..1).
#[derive(Debug, Default)]
pub struct Font {
    pub name: String,
    pub presets: Vec<Preset>,
    pub instruments: Vec<Instrument>,
    pub samples: Vec<SampleHeader>,
    pub data: Arc<Vec<f32>>,
}

/// What one zone pair plays for a note: the sample, every generator's final value, and the modulators.
#[derive(Clone, Debug)]
pub struct VoiceSpec {
    pub sample: usize,
    pub gens: [i32; g::COUNT],
    pub mods: Vec<Modulator>,
}

fn u16_at(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}

fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

fn name20(b: &[u8]) -> String {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    b[..end].iter().map(|&c| c as char).collect::<String>().trim_end().to_string()
}

/// The chunks of one RIFF list body.
fn chunks(b: &[u8]) -> Vec<(&[u8], &[u8])> {
    let mut out = Vec::new();
    let mut o = 0;
    while o + 8 <= b.len() {
        let size = u32_at(b, o + 4) as usize;
        let body = &b[o + 8..(o + 8 + size).min(b.len())];
        out.push((&b[o..o + 4], body));
        o += 8 + size + (size & 1);
    }
    out
}

impl Font {
    pub fn open(path: &Path) -> Result<Font, String> {
        let b = std::fs::read(path).map_err(|e| e.to_string())?;
        Font::parse(&b)
    }

    pub fn parse(b: &[u8]) -> Result<Font, String> {
        if b.len() < 12 || &b[0..4] != b"RIFF" || &b[8..12] != b"sfbk" {
            return Err("not a SoundFont 2 file".into());
        }
        let mut font = Font::default();
        let (mut smpl, mut pdta): (&[u8], Vec<(&[u8], &[u8])>) = (&[], Vec::new());
        for (id, body) in chunks(&b[12..]) {
            if id != b"LIST" || body.len() < 4 {
                continue;
            }
            let sub = chunks(&body[4..]);
            match &body[0..4] {
                b"INFO" => {
                    if let Some((_, n)) = sub.iter().find(|(i, _)| *i == b"INAM") {
                        font.name = name20(n);
                    }
                }
                b"sdta" => {
                    if let Some((_, s)) = sub.iter().find(|(i, _)| *i == b"smpl") {
                        smpl = s;
                    }
                }
                b"pdta" => pdta = sub,
                _ => {}
            }
        }
        let get = |name: &[u8], size: usize| -> Result<&[u8], String> {
            let c = pdta.iter().find(|(i, _)| *i == name).map(|(_, b)| *b).ok_or_else(|| format!("missing {}", String::from_utf8_lossy(name)))?;
            if c.len() % size != 0 || c.len() < size {
                return Err(format!("bad {} chunk", String::from_utf8_lossy(name)));
            }
            Ok(c)
        };
        let (phdr, pbag, pmod, pgen) = (get(b"phdr", 38)?, get(b"pbag", 4)?, get(b"pmod", 10)?, get(b"pgen", 4)?);
        let (inst, ibag, imod, igen, shdr) = (get(b"inst", 22)?, get(b"ibag", 4)?, get(b"imod", 10)?, get(b"igen", 4)?, get(b"shdr", 46)?);
        font.data = Arc::new(smpl.chunks_exact(2).map(|c| f32::from(i16::from_le_bytes([c[0], c[1]])) / 32768.0).collect());

        let zones = |bag: &[u8], gv: &[u8], md: &[u8], first: usize, last: usize, link: usize| -> Vec<(Option<usize>, Zone)> {
            let nbag = bag.len() / 4;
            let mut out = Vec::new();
            for z in first..last.min(nbag.saturating_sub(1)) {
                let (g0, m0) = (u16_at(bag, z * 4) as usize, u16_at(bag, z * 4 + 2) as usize);
                let (g1, m1) = (u16_at(bag, z * 4 + 4) as usize, u16_at(bag, z * 4 + 6) as usize);
                let mut zone = Zone::default();
                for i in g0..g1.min(gv.len() / 4) {
                    zone.gens.push((u16_at(gv, i * 4), u16_at(gv, i * 4 + 2) as i16));
                }
                for i in m0..m1.min(md.len() / 10) {
                    let o = i * 10;
                    zone.mods.push(Modulator { src: u16_at(md, o), dest: u16_at(md, o + 2), amount: u16_at(md, o + 4) as i16, amt_src: u16_at(md, o + 6), transform: u16_at(md, o + 8) });
                }
                out.push((zone.link(link), zone));
            }
            out
        };
        let split = |zs: Vec<(Option<usize>, Zone)>| -> (Option<Zone>, Vec<Zone>) {
            let mut global = None;
            let mut local = Vec::new();
            for (i, (link, z)) in zs.into_iter().enumerate() {
                match link {
                    Some(_) => local.push(z),
                    // Only the first zone may be global; a later linkless zone is ignored (§7.3).
                    None if i == 0 => global = Some(z),
                    None => {}
                }
            }
            (global, local)
        };
        let ni = inst.len() / 22;
        for i in 0..ni.saturating_sub(1) {
            let o = i * 22;
            let (first, last) = (u16_at(inst, o + 20) as usize, u16_at(inst, o + 22 + 20) as usize);
            let (global, zones) = split(zones(ibag, igen, imod, first, last, g::SAMPLE_ID));
            font.instruments.push(Instrument { name: name20(&inst[o..o + 20]), global, zones });
        }
        let np = phdr.len() / 38;
        for i in 0..np.saturating_sub(1) {
            let o = i * 38;
            let (first, last) = (u16_at(phdr, o + 24) as usize, u16_at(phdr, o + 38 + 24) as usize);
            let (global, zones) = split(zones(pbag, pgen, pmod, first, last, g::INSTRUMENT));
            font.presets.push(Preset { name: name20(&phdr[o..o + 20]), program: u16_at(phdr, o + 20), bank: u16_at(phdr, o + 22), global, zones });
        }
        font.presets.sort_by_key(|p| (p.bank, p.program));
        let ns = shdr.len() / 46;
        for i in 0..ns.saturating_sub(1) {
            let o = i * 46;
            font.samples.push(SampleHeader {
                name: name20(&shdr[o..o + 20]),
                start: u32_at(shdr, o + 20),
                end: u32_at(shdr, o + 24),
                loop_start: u32_at(shdr, o + 28),
                loop_end: u32_at(shdr, o + 32),
                rate: u32_at(shdr, o + 36),
                root: shdr[o + 40],
                correction: shdr[o + 41] as i8,
                kind: u16_at(shdr, o + 44),
            });
        }
        Ok(font)
    }

    /// The preset at (bank, program), if the font has one.
    pub fn find(&self, bank: u16, program: u16) -> Option<usize> {
        self.presets.iter().position(|p| p.bank == bank && p.program == program)
    }

    /// What preset `p` plays for `key` at `vel` (§9.4): every matching preset zone × matching instrument zone.
    pub fn voices(&self, p: usize, key: u8, vel: u8) -> Vec<VoiceSpec> {
        let Some(preset) = self.presets.get(p) else { return Vec::new() };
        let inside = |z: &Zone, op: usize, v: u8| {
            let (lo, hi) = z.range(op);
            lo <= v && v <= hi
        };
        let mut out = Vec::new();
        for pz in &preset.zones {
            if !(inside(pz, g::KEY_RANGE, key) && inside(pz, g::VEL_RANGE, vel)) {
                continue;
            }
            let Some(inst) = pz.link(g::INSTRUMENT).and_then(|i| self.instruments.get(i)) else { continue };
            for iz in &inst.zones {
                if !(inside(iz, g::KEY_RANGE, key) && inside(iz, g::VEL_RANGE, vel)) {
                    continue;
                }
                let Some(sample) = iz.link(g::SAMPLE_ID).filter(|&s| s < self.samples.len()) else { continue };
                // Instrument level: defaults, the global zone, then the zone itself (absolute values).
                let mut gens = default_gens();
                for z in inst.global.iter().chain(std::iter::once(iz)) {
                    for &(op, v) in &z.gens {
                        let op = op as usize;
                        if op < g::COUNT && op != g::KEY_RANGE && op != g::VEL_RANGE && op != g::SAMPLE_ID {
                            gens[op] = i32::from(v);
                        }
                    }
                }
                // Preset level: the global zone's offsets, overridden by the zone's own, added on.
                let mut offs = [0i32; g::COUNT];
                for z in preset.global.iter().chain(std::iter::once(pz)) {
                    for &(op, v) in &z.gens {
                        let op = op as usize;
                        if op < g::COUNT && !instrument_only(op) && op != g::KEY_RANGE && op != g::VEL_RANGE && op != g::INSTRUMENT {
                            offs[op] = i32::from(v);
                        }
                    }
                }
                for op in 0..g::COUNT {
                    if offs[op] != 0 {
                        gens[op] = clamp_gen(op, gens[op] + offs[op]);
                    } else {
                        gens[op] = clamp_gen(op, gens[op]);
                    }
                }
                // Modulators: the defaults, overridden by the instrument's (global, then local), then the preset's
                // added on (§9.5).
                let mut mods = default_modulators();
                let mut inst_mods: Vec<Modulator> = Vec::new();
                for z in inst.global.iter().chain(std::iter::once(iz)) {
                    for m in &z.mods {
                        if let Some(i) = inst_mods.iter().position(|x| x.same(m)) {
                            inst_mods[i] = *m;
                        } else {
                            inst_mods.push(*m);
                        }
                    }
                }
                for m in inst_mods {
                    if let Some(i) = mods.iter().position(|x| x.same(&m)) {
                        mods[i] = m;
                    } else {
                        mods.push(m);
                    }
                }
                let mut pre_mods: Vec<Modulator> = Vec::new();
                for z in preset.global.iter().chain(std::iter::once(pz)) {
                    for m in &z.mods {
                        if let Some(i) = pre_mods.iter().position(|x| x.same(m)) {
                            pre_mods[i] = *m;
                        } else {
                            pre_mods.push(*m);
                        }
                    }
                }
                for m in pre_mods {
                    if let Some(i) = mods.iter().position(|x| x.same(&m)) {
                        mods[i].amount = mods[i].amount.saturating_add(m.amount);
                    } else {
                        mods.push(m);
                    }
                }
                out.push(VoiceSpec { sample, gens, mods });
            }
        }
        out
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A minimal font: one preset (bank 0, program `prog`) whose one instrument zone plays one sample with `igens`.
    pub(crate) fn font_bytes(data: &[i16], rate: u32, loop_: (u32, u32), igens: &[(u16, i16)], prog: u16) -> Vec<u8> {
        named_font_bytes(data, rate, loop_, igens, prog, "test", "p")
    }

    /// [`font_bytes`] with the font's and the preset's names.
    pub(crate) fn named_font_bytes(data: &[i16], rate: u32, loop_: (u32, u32), igens: &[(u16, i16)], prog: u16, font: &str, preset: &str) -> Vec<u8> {
        fn ck(id: &[u8], body: &[u8]) -> Vec<u8> {
            let mut v = id.to_vec();
            v.extend((body.len() as u32).to_le_bytes());
            v.extend(body);
            if body.len() & 1 == 1 {
                v.push(0);
            }
            v
        }
        fn list(kind: &[u8], body: Vec<u8>) -> Vec<u8> {
            let mut b = kind.to_vec();
            b.extend(body);
            ck(b"LIST", &b)
        }
        fn n20(s: &str) -> Vec<u8> {
            let mut v = s.as_bytes().to_vec();
            v.resize(20, 0);
            v
        }
        let mut smpl: Vec<u8> = data.iter().flat_map(|v| v.to_le_bytes()).collect();
        smpl.extend([0u8; 92]);
        let mut shdr = n20("s");
        for v in [0, data.len() as u32, loop_.0, loop_.1, rate] {
            shdr.extend(v.to_le_bytes());
        }
        shdr.extend([60u8, 0, 0, 0, 1, 0]);
        shdr.extend(n20("EOS"));
        shdr.extend([0u8; 26]);
        let mut phdr = n20(preset);
        phdr.extend(prog.to_le_bytes());
        phdr.extend([0u8; 2 + 2 + 12]);
        phdr.extend(n20("EOP"));
        phdr.extend([0u8; 4]);
        phdr.extend(1u16.to_le_bytes());
        phdr.extend([0u8; 12]);
        let pbag = [0u8, 0, 0, 0, 1, 0, 0, 0];
        let mut pgen = vec![41u8, 0, 0, 0];
        pgen.extend([0u8; 4]);
        let mut inst = n20("i");
        inst.extend([0u8, 0]);
        inst.extend(n20("EOI"));
        inst.extend([1u8, 0]);
        let ng = igens.len() as u16 + 1;
        let mut ibag = vec![0u8, 0, 0, 0];
        ibag.extend(ng.to_le_bytes());
        ibag.extend([0u8, 0]);
        let mut igen = Vec::new();
        for (op, v) in igens {
            igen.extend(op.to_le_bytes());
            igen.extend(v.to_le_bytes());
        }
        igen.extend([53u8, 0, 0, 0]);
        igen.extend([0u8; 4]);
        let modt = [0u8; 10];
        let mut inam = font.as_bytes().to_vec();
        inam.extend([0, 0]);
        let info = list(b"INFO", [ck(b"ifil", &[2, 0, 1, 0]), ck(b"INAM", &inam)].concat());
        let sdta = list(b"sdta", ck(b"smpl", &smpl));
        let pdta = list(
            b"pdta",
            [ck(b"phdr", &phdr), ck(b"pbag", &pbag), ck(b"pmod", &modt), ck(b"pgen", &pgen), ck(b"inst", &inst), ck(b"ibag", &ibag), ck(b"imod", &modt), ck(b"igen", &igen), ck(b"shdr", &shdr)]
                .concat(),
        );
        let mut body = b"sfbk".to_vec();
        body.extend(info);
        body.extend(sdta);
        body.extend(pdta);
        ck(b"RIFF", &body)
    }

    #[test]
    fn a_font_resolves_its_zones() {
        let f = Font::parse(&font_bytes(&[0, 1000, 2000, 1000], 22050, (1, 3), &[(54, 1), (38, -1200), (17, 250)], 5)).unwrap();
        assert_eq!(f.presets.len(), 1);
        assert_eq!((f.presets[0].bank, f.presets[0].program), (0, 5));
        assert_eq!(f.samples[0].rate, 22050);
        let v = f.voices(0, 60, 100);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].gens[g::SAMPLE_MODES], 1);
        assert_eq!(v[0].gens[g::VOLENV_RELEASE], -1200);
        assert_eq!(v[0].gens[g::PAN], 250);
        assert_eq!(v[0].gens[g::VOLENV_ATTACK], -12000);
        assert_eq!(v[0].mods.len(), 10);
        assert!((f.data[1] - 1000.0 / 32768.0).abs() < 1e-7);
    }
}
