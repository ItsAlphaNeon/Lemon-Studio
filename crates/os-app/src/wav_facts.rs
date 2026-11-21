// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! What the browser's waveform panel says about a WAV file besides its audio: the sample format's bit depth, the first
//! `smpl` loop, the `cue` markers and the `acid` beat count (spec: ui/browser.md §7b). Only chunk headers and those
//! chunks' bodies are read.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WavFacts {
    /// Bits per sample of integer PCM data; `None` for float or compressed data, and for files that aren't WAVs.
    pub pcm_bits: Option<u16>,
    /// The first `smpl` loop, as (start, end) frames, the end included.
    pub sample_loop: Option<(u32, u32)>,
    /// The `cue` markers' frames, in the file's order.
    pub cues: Vec<u32>,
    /// The `acid` chunk's beat count, when it gives one.
    pub beats: Option<u32>,
}

/// Larger chunks than this are skipped rather than read (a `cue` list of a few thousand markers fits).
const MAX_BODY: u32 = 1 << 20;

/// The facts of the WAV file at `path`; an unreadable file or another format has none.
pub fn read(path: &Path) -> WavFacts {
    std::fs::File::open(path).ok().and_then(|f| of(std::io::BufReader::new(f))).unwrap_or_default()
}

fn u16_at(b: &[u8], o: usize) -> Option<u16> {
    b.get(o..o + 2).map(|v| u16::from_le_bytes([v[0], v[1]]))
}

fn u32_at(b: &[u8], o: usize) -> Option<u32> {
    b.get(o..o + 4).map(|v| u32::from_le_bytes([v[0], v[1], v[2], v[3]]))
}

fn of(mut r: impl Read + Seek) -> Option<WavFacts> {
    let mut head = [0u8; 12];
    r.read_exact(&mut head).ok()?;
    if &head[0..4] != b"RIFF" || &head[8..12] != b"WAVE" {
        return None;
    }
    let mut facts = WavFacts::default();
    let mut chunk = [0u8; 8];
    while r.read_exact(&mut chunk).is_ok() {
        let len = u32::from_le_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]);
        let id = [chunk[0], chunk[1], chunk[2], chunk[3]];
        let wanted = matches!(&id, b"fmt " | b"smpl" | b"cue " | b"acid") && len <= MAX_BODY;
        if !wanted {
            r.seek(SeekFrom::Current(i64::from(len) + i64::from(len & 1))).ok()?;
            continue;
        }
        let mut body = vec![0u8; len as usize];
        r.read_exact(&mut body).ok()?;
        if len & 1 == 1 {
            r.seek(SeekFrom::Current(1)).ok()?;
        }
        match &id {
            b"fmt " => {
                let tag = u16_at(&body, 0)?;
                let bits = u16_at(&body, 14)?;
                // WAVE_FORMAT_EXTENSIBLE: the sub-format GUID's first two bytes are the format tag.
                let tag = if tag == 0xFFFE { u16_at(&body, 24).unwrap_or(tag) } else { tag };
                facts.pcm_bits = (tag == 1 && bits > 0).then_some(bits);
            }
            b"smpl" => {
                // 36 bytes of header (the loop count at 28), then 24 bytes per loop: id, type, start, end, ...
                if facts.sample_loop.is_none() && u32_at(&body, 28).is_some_and(|n| n > 0) {
                    facts.sample_loop = u32_at(&body, 44).zip(u32_at(&body, 48));
                }
            }
            b"cue " => {
                // A count, then 24 bytes per point; the sample offset is the point's last u32.
                let n = u32_at(&body, 0).unwrap_or(0) as usize;
                facts.cues = (0..n).map_while(|i| u32_at(&body, 4 + 24 * i + 20)).collect();
            }
            b"acid" => {
                let one_shot = body.first().is_some_and(|f| f & 1 != 0);
                facts.beats = u32_at(&body, 12).filter(|&b| b > 0 && !one_shot);
            }
            _ => {}
        }
    }
    Some(facts)
}

/// `44.1 kHz`, `48 kHz`, `22.05 kHz`: the rate in kHz without trailing zeros.
pub fn rate_text(rate: u32) -> String {
    let k = format!("{:.2}", f64::from(rate) / 1000.0);
    format!("{} kHz", k.trim_end_matches('0').trim_end_matches('.'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(id: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut v = id.to_vec();
        v.extend((body.len() as u32).to_le_bytes());
        v.extend(body);
        if body.len() % 2 == 1 {
            v.push(0);
        }
        v
    }

    fn wav(chunks: &[Vec<u8>]) -> Vec<u8> {
        let body: Vec<u8> = chunks.concat();
        let mut v = b"RIFF".to_vec();
        v.extend((body.len() as u32 + 4).to_le_bytes());
        v.extend(b"WAVE");
        v.extend(body);
        v
    }

    fn fmt(tag: u16, bits: u16) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend(tag.to_le_bytes());
        b.extend(1u16.to_le_bytes());
        b.extend(44_100u32.to_le_bytes());
        b.extend(88_200u32.to_le_bytes());
        b.extend(2u16.to_le_bytes());
        b.extend(bits.to_le_bytes());
        chunk(b"fmt ", &b)
    }

    #[test]
    fn reads_the_format_loop_cues_and_beats() {
        let mut smpl = vec![0u8; 36];
        smpl[28..32].copy_from_slice(&1u32.to_le_bytes());
        let mut lp = vec![0u8; 24];
        lp[8..12].copy_from_slice(&100u32.to_le_bytes());
        lp[12..16].copy_from_slice(&999u32.to_le_bytes());
        smpl.extend(lp);
        let mut cue = 2u32.to_le_bytes().to_vec();
        for at in [10u32, 500] {
            let mut p = vec![0u8; 24];
            p[20..24].copy_from_slice(&at.to_le_bytes());
            cue.extend(p);
        }
        let mut acid = vec![0u8; 24];
        acid[12..16].copy_from_slice(&8u32.to_le_bytes());
        let data = wav(&[fmt(1, 16), chunk(b"odd ", &[1, 2, 3]), chunk(b"smpl", &smpl), chunk(b"cue ", &cue), chunk(b"acid", &acid), chunk(b"data", &[0; 8])]);
        let f = of(std::io::Cursor::new(data)).unwrap();
        assert_eq!(f, WavFacts { pcm_bits: Some(16), sample_loop: Some((100, 999)), cues: vec![10, 500], beats: Some(8) });
        // Float and compressed data have no PCM bit depth; a one-shot's beats don't count.
        acid[0] = 1;
        let f = of(std::io::Cursor::new(wav(&[fmt(3, 32), chunk(b"acid", &acid)]))).unwrap();
        assert_eq!((f.pcm_bits, f.beats), (None, None));
        assert_eq!(of(std::io::Cursor::new(wav(&[fmt(0x674F, 0)]))).unwrap().pcm_bits, None);
        assert!(of(std::io::Cursor::new(b"OggS....".to_vec())).is_none());
    }

    #[test]
    fn rates_read_as_khz() {
        assert_eq!(rate_text(44_100), "44.1 kHz");
        assert_eq!(rate_text(48_000), "48 kHz");
        assert_eq!(rate_text(22_050), "22.05 kHz");
    }
}
