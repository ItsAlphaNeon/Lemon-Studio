// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! WAV files with Edison's chunk set: `JUNK` (room for RF64's `ds64`), `fmt `, `fact` (float), `data`, `smpl` (the
//! loop), `inst`, `acid`, `strc` and `cue ` (with markers or regions), `LIST adtl` (names; regions' `ltxt` "beat") and
//! `LIST INFO` (ISFT). The reader takes Edison's files, the usual PCM / float / extensible WAVs and RF64.
//!
// spec: plugins/edison/wav-format.md, plugins/tesla/README.md "Version 1" (files)

use crate::sample::{Cue, Format, Sample};

/// What Tesla writes as the software name (`LIST INFO` ISFT); Edison writes "Edison".
pub const SOFTWARE: &str = "Tesla";

/// The bit depth to save.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Depth {
    /// The sample's own format (16-bit int or 32-bit float).
    Own,
    /// File > Save sample as 24Bit .wav file.
    Int24,
}

/// The `smpl` loop's cue point ID (it refers to no cue).
const LOOP_ID: u32 = 0x0002_0000;

struct Out(Vec<u8>);

impl Out {
    fn u16(&mut self, v: u16) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn u64(&mut self, v: u64) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn f32(&mut self, v: f32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn tag(&mut self, t: &[u8; 4]) {
        self.0.extend_from_slice(t);
    }
    /// A chunk: its ID, size and body, padded to an even size.
    fn chunk(&mut self, id: &[u8; 4], body: &[u8]) {
        self.tag(id);
        self.u32(body.len() as u32);
        self.0.extend_from_slice(body);
        if body.len() % 2 == 1 {
            self.0.push(0);
        }
    }
}

fn body(f: impl FnOnce(&mut Out)) -> Vec<u8> {
    let mut o = Out(Vec::new());
    f(&mut o);
    o.0
}

/// A zero-terminated string, as `labl` and ISFT hold it.
fn zstr(s: &str) -> Vec<u8> {
    let mut v: Vec<u8> = s.bytes().filter(|&b| b != 0).collect();
    v.push(0);
    v
}

/// The WAV file of `s` (the bytes File > Save sample as writes; the plugin state embeds the same).
pub fn write(s: &Sample, depth: Depth) -> Vec<u8> {
    write_as(s, depth, SOFTWARE)
}

/// [`write`] with another software name (Edison's state, written back for FL, says "Edison").
pub fn write_as(s: &Sample, depth: Depth, software: &str) -> Vec<u8> {
    let (tag, bits): (u16, u16) = match (depth, s.format) {
        (Depth::Int24, _) => (1, 24),
        (Depth::Own, Format::Int16) => (1, 16),
        (Depth::Own, Format::Float32) => (3, 32),
    };
    let chans = s.channels() as u16;
    let n = s.frames();
    let block = chans as u32 * bits as u32 / 8;
    let data_len = n as u64 * block as u64;
    let mut data = Vec::with_capacity(data_len as usize);
    for i in 0..n {
        for c in &s.ch {
            let v = c[i];
            match bits {
                16 => data.extend_from_slice(&((v * 32768.0).round().clamp(-32768.0, 32767.0) as i16).to_le_bytes()),
                24 => {
                    let x = (v as f64 * 8388608.0).round().clamp(-8388608.0, 8388607.0) as i32;
                    data.extend_from_slice(&x.to_le_bytes()[..3]);
                }
                _ => data.extend_from_slice(&v.to_le_bytes()),
            }
        }
    }
    let fmt = body(|o| {
        o.u16(tag);
        o.u16(chans);
        o.u32(s.rate);
        o.u32(s.rate * block);
        o.u16(block as u16);
        o.u16(bits);
    });
    let smpl = body(|o| {
        o.u32(0);
        o.u32(0);
        o.u32((1e9 / s.rate.max(1) as f64).round() as u32);
        o.u32(u32::from(s.note));
        o.u32(0);
        o.u32(0);
        o.u32(0);
        o.u32(u32::from(s.looped.is_some()));
        o.u32(0);
        if let Some((a, b)) = s.looped {
            o.u32(LOOP_ID);
            o.u32(0);
            o.u32(a as u32);
            o.u32(b as u32);
            o.u32(0);
            o.u32(0);
        }
    });
    let inst = vec![s.note, 0, 0, 0, 127, 0, 127];
    let acid = body(|o| {
        o.u32(1);
        o.u16(u16::from(s.note));
        o.u16(0x80);
        o.f32(0.0);
        o.u32(s.beats());
        o.u16(4);
        o.u16(4);
        o.f32(s.tempo);
    });
    let mut cues: Vec<&Cue> = s.cues.iter().collect();
    cues.sort_by_key(|c| c.id);
    let strc = body(|o| {
        for v in [28, cues.len() as u32 + 1, 0, 0, 1, 10, 0] {
            o.u32(v);
        }
        for v in [2, 0, 0, 0, 0, 0] {
            o.u32(v);
        }
        for c in &cues {
            for v in [0, c.pos as u32, 0, 0, 0, 0] {
                o.u32(v);
            }
        }
    });
    let cue = body(|o| {
        o.u32(cues.len() as u32);
        for c in &cues {
            o.u32(c.id);
            o.u32(c.pos as u32);
            o.tag(b"data");
            o.u32(0);
            o.u32(0);
            o.u32(c.pos as u32);
        }
    });
    let adtl = body(|o| {
        o.tag(b"adtl");
        for c in &cues {
            o.chunk(b"labl", &body(|l| {
                l.u32(c.id);
                l.0.extend_from_slice(&zstr(&c.name));
            }));
            if let Some(len) = c.len {
                o.chunk(b"ltxt", &body(|l| {
                    l.u32(c.id);
                    l.u32(len as u32);
                    l.tag(b"beat");
                    for _ in 0..4 {
                        l.u16(0);
                    }
                }));
            }
        }
    });
    let info = body(|o| {
        o.tag(b"INFO");
        o.chunk(b"ISFT", &zstr(software));
    });

    // Past 4 GB the file becomes RF64: `JUNK` turns into `ds64` and the 32-bit sizes read 0xFFFFFFFF.
    let mut rest = Out(Vec::new());
    rest.chunk(b"smpl", &smpl);
    rest.chunk(b"inst", &inst);
    rest.chunk(b"acid", &acid);
    if !cues.is_empty() {
        rest.chunk(b"strc", &strc);
        rest.chunk(b"cue ", &cue);
    }
    rest.chunk(b"LIST", &adtl);
    rest.chunk(b"LIST", &info);
    let data_padded = data_len + (data_len & 1);
    let fact_len = if tag == 3 { 12 } else { 0 };
    let riff_len = 4 + (8 + 28) + (8 + 16) + fact_len + 8 + data_padded + rest.0.len() as u64;
    let big = riff_len > u32::MAX as u64;
    let mut o = Out(Vec::with_capacity(riff_len as usize + 8));
    o.tag(if big { b"RF64" } else { b"RIFF" });
    o.u32(if big { u32::MAX } else { riff_len as u32 });
    o.tag(b"WAVE");
    if big {
        o.tag(b"ds64");
        o.u32(28);
        o.u64(riff_len);
        o.u64(data_len);
        o.u64(n as u64);
        o.u32(0);
    } else {
        o.chunk(b"JUNK", &[0; 28]);
    }
    o.chunk(b"fmt ", &fmt);
    if tag == 3 {
        o.chunk(b"fact", &(n.min(u32::MAX as usize) as u32).to_le_bytes());
    }
    o.tag(b"data");
    o.u32(if big { u32::MAX } else { data_len as u32 });
    o.0.extend_from_slice(&data);
    if data_len % 2 == 1 {
        o.0.push(0);
    }
    o.0.extend_from_slice(&rest.0);
    o.0
}

fn u16_at(b: &[u8], i: usize) -> u16 {
    b.get(i..i + 2).map_or(0, |x| u16::from_le_bytes([x[0], x[1]]))
}

fn u32_at(b: &[u8], i: usize) -> u32 {
    b.get(i..i + 4).map_or(0, |x| u32::from_le_bytes([x[0], x[1], x[2], x[3]]))
}

fn u64_at(b: &[u8], i: usize) -> u64 {
    b.get(i..i + 8).map_or(0, |x| u64::from_le_bytes(x.try_into().unwrap()))
}

fn cstr(b: &[u8]) -> String {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..end]).into_owned()
}

/// Whether `b` starts like a WAV file.
pub fn is_wav(b: &[u8]) -> bool {
    b.len() >= 12 && (&b[0..4] == b"RIFF" || &b[0..4] == b"RF64") && &b[8..12] == b"WAVE"
}

/// The whole length of the RIFF file at the start of `b` (its size field + 8), as far as `b` holds it.
pub fn riff_len(b: &[u8]) -> Option<usize> {
    if !is_wav(b) {
        return None;
    }
    if &b[0..4] == b"RF64" && &b[12..16] == b"ds64" {
        return Some((u64_at(b, 20) as usize).saturating_add(8).min(b.len()));
    }
    Some((u32_at(b, 4) as usize).saturating_add(8).min(b.len()))
}

/// Reads a WAV file: the audio, the loop (`smpl`), markers and regions (`cue ` + `LIST adtl`), the note (`smpl` /
/// `inst`) and the tempo (`acid`).
pub fn read(b: &[u8]) -> Result<Sample, String> {
    if !is_wav(b) {
        return Err("not a WAV file".into());
    }
    let rf64 = &b[0..4] == b"RF64";
    let end = riff_len(b).unwrap_or(b.len());
    let mut ds64_data: Option<u64> = None;
    let mut fmt: Option<&[u8]> = None;
    let mut data: Option<&[u8]> = None;
    let mut smpl: Option<&[u8]> = None;
    let mut inst: Option<&[u8]> = None;
    let mut acid: Option<&[u8]> = None;
    let mut cue: Option<&[u8]> = None;
    let mut adtl: Vec<&[u8]> = Vec::new();
    let mut i = 12;
    while i + 8 <= end {
        let id = &b[i..i + 4];
        let mut len = u32_at(b, i + 4) as u64;
        if rf64 && id == b"data" && len == u32::MAX as u64 {
            len = ds64_data.unwrap_or((end - i - 8) as u64);
        }
        let start = i + 8;
        let stop = (start as u64 + len).min(end as u64) as usize;
        let body = &b[start..stop];
        match id {
            b"ds64" => ds64_data = Some(u64_at(body, 8)),
            b"fmt " => fmt = Some(body),
            b"data" => data = Some(body),
            b"smpl" => smpl = Some(body),
            b"inst" => inst = Some(body),
            b"acid" => acid = Some(body),
            b"cue " => cue = Some(body),
            b"LIST" if body.len() >= 4 && &body[0..4] == b"adtl" => adtl.push(&body[4..]),
            _ => {}
        }
        i = start.saturating_add(len as usize).saturating_add((len & 1) as usize);
    }
    let fmt = fmt.ok_or("no fmt chunk")?;
    let mut tag = u16_at(fmt, 0);
    let chans = u16_at(fmt, 2).max(1) as usize;
    let rate = u32_at(fmt, 4);
    let bits = u16_at(fmt, 14);
    if tag == 0xFFFE && fmt.len() >= 26 {
        tag = u16_at(fmt, 24);
    }
    if rate == 0 {
        return Err("no sample rate".into());
    }
    let data = data.unwrap_or(&[]);
    let bytes = (bits as usize).div_ceil(8);
    if bytes == 0 {
        return Err("no bit depth".into());
    }
    let frame = bytes * chans;
    let n = data.len() / frame;
    let keep = chans.min(2);
    let mut ch: Vec<Vec<f32>> = (0..keep).map(|_| Vec::with_capacity(n)).collect();
    let decode: fn(&[u8]) -> f32 = match (tag, bits) {
        (1, 8) => |x| (x[0] as f32 - 128.0) / 128.0,
        (1, 16) => |x| i16::from_le_bytes([x[0], x[1]]) as f32 / 32768.0,
        (1, 24) => |x| (i32::from_le_bytes([0, x[0], x[1], x[2]]) >> 8) as f32 / 8388608.0,
        (1, 32) => |x| (i32::from_le_bytes([x[0], x[1], x[2], x[3]]) as f64 / 2147483648.0) as f32,
        (3, 32) => |x| f32::from_le_bytes([x[0], x[1], x[2], x[3]]),
        (3, 64) => |x| f64::from_le_bytes(x[..8].try_into().unwrap()) as f32,
        _ => return Err(format!("unsupported WAV format (tag {tag}, {bits} bit)")),
    };
    for f in data.chunks_exact(frame) {
        for (c, out) in ch.iter_mut().enumerate() {
            out.push(decode(&f[c * bytes..c * bytes + bytes]));
        }
    }
    // Edison keeps 16-bit int or 32-bit float: anything not 8 / 16-bit int is shown as float.
    let format = if tag == 1 && bits <= 16 { Format::Int16 } else { Format::Float32 };
    let mut s = Sample::from_channels(rate, ch, format);
    if let Some(sm) = smpl {
        let note = u32_at(sm, 12);
        if note <= 127 {
            s.note = note as u8;
        }
        if u32_at(sm, 28) > 0 && sm.len() >= 36 + 24 {
            let (a, z) = (u32_at(sm, 36 + 8) as usize, u32_at(sm, 36 + 12) as usize);
            if z >= a && a < s.frames().max(1) {
                s.looped = Some((a, z.min(s.frames().saturating_sub(1))));
            }
        }
    } else if let Some(ins) = inst {
        if let Some(&note) = ins.first() {
            if note <= 127 {
                s.note = note;
            }
        }
    }
    if let Some(ac) = acid {
        let t = f32::from_le_bytes(ac.get(20..24).map_or([0; 4], |x| x.try_into().unwrap()));
        if t.is_finite() && t > 0.0 && t < 1000.0 {
            s.tempo = t;
        }
    }
    if let Some(cb) = cue {
        let count = u32_at(cb, 0) as usize;
        let mut names = std::collections::HashMap::new();
        let mut lens = std::collections::HashMap::new();
        for list in adtl {
            let mut j = 0;
            while j + 8 <= list.len() {
                let id = &list[j..j + 4];
                let len = u32_at(list, j + 4) as usize;
                let sub = &list[(j + 8).min(list.len())..(j + 8 + len).min(list.len())];
                match id {
                    b"labl" | b"note" if sub.len() >= 4 => {
                        if id == b"labl" {
                            names.insert(u32_at(sub, 0), cstr(&sub[4..]));
                        }
                    }
                    b"ltxt" if sub.len() >= 8 => {
                        lens.insert(u32_at(sub, 0), u32_at(sub, 4) as usize);
                    }
                    _ => {}
                }
                j += 8 + len + (len & 1);
            }
        }
        for k in 0..count {
            let e = 4 + k * 24;
            if e + 24 > cb.len() {
                break;
            }
            let id = u32_at(cb, e);
            let pos = u32_at(cb, e + 20) as usize;
            let len = lens.get(&id).copied().filter(|&l| l > 0);
            let name = names.get(&id).cloned().unwrap_or_default();
            s.cues.push(Cue { id, name, pos, len });
        }
        s.cues.sort_by_key(|c| c.id);
        s.clip_marks();
    }
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The worked example of the spec: 1 s stereo float at 44.1 kHz, a whole-sample loop, mk1 at 0, rg1 at 7528
    /// (11855 long), mk2 at 7528.
    fn example() -> Sample {
        let n = 44100;
        let l: Vec<f32> = (0..n).map(|i| (i as f32 * 0.01).sin() * 0.5).collect();
        let r: Vec<f32> = (0..n).map(|i| (i as f32 * 0.02).cos() * 0.25).collect();
        let mut s = Sample::from_channels(44100, vec![l, r], Format::Float32);
        s.looped = Some((0, 44099));
        s.add_cue("mk1", 0, None);
        s.add_cue("rg1", 7528, Some(11855));
        s.add_cue("mk2", 7528, None);
        s
    }

    fn chunks(b: &[u8]) -> Vec<(String, u32)> {
        let mut v = Vec::new();
        let mut i = 12;
        while i + 8 <= b.len() {
            let len = u32_at(b, i + 4);
            let mut id = String::from_utf8_lossy(&b[i..i + 4]).into_owned();
            if id == "LIST" {
                id = format!("LIST {}", String::from_utf8_lossy(&b[i + 8..i + 12]));
            }
            v.push((id, len));
            i += 8 + len as usize + (len & 1) as usize;
        }
        v
    }

    #[test]
    fn writes_edisons_chunk_set_in_order() {
        let b = write_as(&example(), Depth::Own, "Edison");
        let c = chunks(&b);
        let want: Vec<(&str, u32)> = vec![
            ("JUNK", 28),
            ("fmt ", 16),
            ("fact", 4),
            ("data", 352800),
            ("smpl", 60),
            ("inst", 7),
            ("acid", 24),
            ("strc", 124),
            ("cue ", 76),
            ("LIST adtl", 80),
            ("LIST INFO", 20),
        ];
        assert_eq!(c.iter().map(|(a, b)| (a.as_str(), *b)).collect::<Vec<_>>(), want);
        // The whole file is the size Edison's state records for this example.
        assert_eq!(b.len(), 353_340);
        assert_eq!(u32_at(&b, 4) as usize + 8, b.len());
        // smpl: period 22676, unity 60, one loop 0..44099 with the fixed ID.
        let sm = b.windows(4).position(|w| w == b"smpl").unwrap() + 8;
        assert_eq!(u32_at(&b, sm + 8), 22676);
        assert_eq!(u32_at(&b, sm + 12), 60);
        assert_eq!(u32_at(&b, sm + 36), 0x20000);
        assert_eq!((u32_at(&b, sm + 44), u32_at(&b, sm + 48)), (0, 44099));
        // acid: 2 beats, 4/4.
        let ac = b.windows(4).position(|w| w == b"acid").unwrap() + 8;
        assert_eq!(u32_at(&b, ac + 12), 2);
        // Tesla's own name: 6 bytes, so LIST INFO is 18.
        let t = write(&example(), Depth::Own);
        assert_eq!(chunks(&t).last().unwrap(), &("LIST INFO".to_string(), 18));
    }

    #[test]
    fn reads_back_what_it_writes() {
        let s = example();
        let r = read(&write(&s, Depth::Own)).unwrap();
        assert_eq!(r, s);
        let mut s16 = s.clone();
        s16.format = Format::Int16;
        s16.quantize();
        let r = read(&write(&s16, Depth::Own)).unwrap();
        assert_eq!(r, s16);
        // 24-bit: within a 24-bit step, shown as float.
        let r = read(&write(&s, Depth::Int24)).unwrap();
        assert_eq!(r.format, Format::Float32);
        assert!(r.ch[0].iter().zip(&s.ch[0]).all(|(a, b)| (a - b).abs() < 2.0 / 8388608.0));
        assert_eq!(r.cues, s.cues);
        // An empty sample: an empty data chunk, no strc / cue.
        let e = write(&Sample::default(), Depth::Own);
        assert!(chunks(&e).iter().all(|(id, _)| id != "cue " && id != "strc"));
        assert_eq!(read(&e).unwrap(), Sample::default());
    }

    #[test]
    fn mono_and_other_pcm() {
        let s = Sample::from_channels(48000, vec![vec![0.25, -0.5, 0.0]], Format::Int16);
        let r = read(&write(&s, Depth::Own)).unwrap();
        assert_eq!(r.channels(), 1);
        assert_eq!(r.ch[0], vec![0.25, -0.5, 0.0]);
        // A minimal 8-bit PCM file.
        let mut b = Vec::new();
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&(4 + 24 + 8 + 2u32).to_le_bytes());
        b.extend_from_slice(b"WAVEfmt ");
        b.extend_from_slice(&16u32.to_le_bytes());
        for v in [1u16, 1] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        b.extend_from_slice(&8000u32.to_le_bytes());
        b.extend_from_slice(&8000u32.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&8u16.to_le_bytes());
        b.extend_from_slice(b"data");
        b.extend_from_slice(&2u32.to_le_bytes());
        b.extend_from_slice(&[128, 192]);
        let r = read(&b).unwrap();
        assert_eq!((r.rate, r.ch[0].clone()), (8000, vec![0.0, 0.5]));
    }
}
