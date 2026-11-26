// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The per-voice analog spread: see `pages/voice.md` and `pages/presets-h2p.md`.

/// What loading a preset does to the pots: the pots it brings (`have` marks which) and the groups re-rolled. Groups in the
/// order cutoff, glide, pulse width, envelope; eight voices each. Parsed away from the audio thread.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PresetSlop {
    pub pots: [[f32; 8]; 4],
    pub have: [[bool; 8]; 4],
    pub rnd: [bool; 4],
    /// The key table a preset brings (`presets-h2p.md`).
    pub tuning: Option<[f32; 128]>,
}

impl From<zest_preset::Pots> for PresetSlop {
    fn from(p: zest_preset::Pots) -> PresetSlop {
        PresetSlop { pots: p.pots, have: p.have, rnd: p.reroll, tuning: p.tuning }
    }
}

impl From<PresetSlop> for zest_preset::Pots {
    fn from(p: PresetSlop) -> zest_preset::Pots {
        zest_preset::Pots { pots: p.pots, have: p.have, reroll: p.rnd, tuning: p.tuning }
    }
}

/// The pots and re-rolls a Diva preset's text holds (`pages/presets-h2p.md`).
pub fn parse_preset(text: &str) -> PresetSlop {
    const MARKER: &str = "// Section for ugly compressed binary Data";
    let mut out = PresetSlop::default();
    let mut in_opt = false;
    let mut in_vcc = false;
    let mut tpots = 10.0f32;
    let mut mtunt = 9.0f32;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with(MARKER) {
            break;
        }
        if let Some(rest) = line.strip_prefix("#cm=") {
            in_opt = rest == "OPT";
            in_vcc = rest == "VCC";
            continue;
        }
        if in_vcc {
            if let Some((k, v)) = line.split_once('=') {
                if k == "MTunT" {
                    mtunt = v.trim().parse::<f32>().unwrap_or(9.0);
                }
            }
            continue;
        }
        if !in_opt {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else { continue };
        let num = v.trim().parse::<f32>().ok();
        match k {
            "TPots" => tpots = num.unwrap_or(10.0),
            "RndC" => out.rnd[0] = num.is_some_and(|x| x >= 0.5),
            "RndG" => out.rnd[1] = num.is_some_and(|x| x >= 0.5),
            "RndP" => out.rnd[2] = num.is_some_and(|x| x >= 0.5),
            "RndE" => out.rnd[3] = num.is_some_and(|x| x >= 0.5),
            _ => {}
        }
    }
    let Some(chunk) = decode_chunk(text) else { return out };
    let want = pmax0_index(tpots);
    let want_key = pmax0_index(mtunt);
    let mut p = 0usize;
    let mut block = 0usize;
    while chunk.len().saturating_sub(p) >= 16 && &chunk[p..p + 4] == b"cbMA" {
        let m = u32::from_le_bytes([chunk[p + 4], chunk[p + 5], chunk[p + 6], chunk[p + 7]]) as usize;
        let start = p + 16;
        let end = start.saturating_add(m).min(chunk.len());
        let payload = &chunk[start..end];
        let float = |at: usize| f32::from_le_bytes([payload[at], payload[at + 1], payload[at + 2], payload[at + 3]]);
        if block == want {
            for g in 0..4 {
                for v in 0..8 {
                    let at = 4 * (24 + 8 * g + v);
                    if at + 4 <= payload.len() {
                        out.pots[g][v] = float(at);
                        out.have[g][v] = true;
                    }
                }
            }
        }
        if block == want_key && payload.len() >= 512 {
            out.tuning = Some(core::array::from_fn(|i| float(4 * i)));
        }
        p = start.saturating_add(m);
        block += 1;
    }
    out
}

fn pmax0_index(tpots: f32) -> usize {
    (if tpots > 0.0 { tpots } else { 0.0 }) as usize
}

/// Maximum decoded `.h2p` binary chunk size (1 MiB) to avoid unbounded allocations on malformed files.
const MAX_CHUNK_SIZE: usize = 1024 * 1024;

/// The binary chunk after the marker line: `$$$$ size ? codebook ! stream = checksum`, all white space removed.
fn decode_chunk(text: &str) -> Option<Vec<u8>> {
    let after = &text[text.find("$$$$")? + 4..];
    let bytes = after.as_bytes();
    let digits = bytes.iter().take_while(|c| c.is_ascii_digit()).count();
    let size: usize = after[..digits].parse().ok()?;
    if size > MAX_CHUNK_SIZE {
        return None;
    }
    let rest: Vec<u8> = bytes[digits..].iter().copied().filter(|c| !c.is_ascii_whitespace()).collect();
    if rest.first() != Some(&b'?') {
        return None;
    }
    let rest = &rest[1..];
    let body = &rest[..rest.iter().position(|&c| c == b'=')?];
    let bang = body.iter().position(|&c| c == b'!')?;
    let (book, stream) = (&body[..bang], &body[bang + 1..]);
    let nibble = |c: u8| -> Option<u8> { (b'a'..=b'p').contains(&c).then(|| c - b'a') };
    let byte = |a: u8, b: u8| -> Option<u8> { Some(nibble(a)? << 4 | nibble(b)?) };
    let mut words: Vec<[u8; 4]> = Vec::new();
    let mut dict: Vec<u8> = Vec::new();
    if !book.is_empty() {
        for e in book.split(|&c| c == b':') {
            match e.len() {
                8 => words.push([byte(e[0], e[1])?, byte(e[2], e[3])?, byte(e[4], e[5])?, byte(e[6], e[7])?]),
                2 => dict.push(byte(e[0], e[1])?),
                _ => return None,
            }
        }
    }
    #[derive(Clone, Copy)]
    enum Tok {
        None,
        Byte(u8),
        Word([u8; 4]),
    }
    let mut out = Vec::with_capacity(size);
    let mut last = Tok::None;
    let mut i = 0;
    while i < stream.len() {
        let c = stream[i];
        match c {
            b'a'..=b'p' => {
                let b = byte(c, *stream.get(i + 1)?)?;
                if out.len() >= size {
                    return None;
                }
                out.push(b);
                last = Tok::Byte(b);
                i += 2;
            }
            b'A'..=b'Z' => {
                let b = *dict.get((c - b'A') as usize)?;
                if out.len() >= size {
                    return None;
                }
                out.push(b);
                last = Tok::Byte(b);
                i += 1;
            }
            b'q'..=b'z' => {
                let w = *words.get((c - b'q') as usize)?;
                if out.len().saturating_add(4) > size {
                    return None;
                }
                out.extend_from_slice(&w);
                last = Tok::Word(w);
                i += 1;
            }
            b'0'..=b'9' => {
                let n = stream[i..].iter().take_while(|c| c.is_ascii_digit()).count();
                let count: usize = core::str::from_utf8(&stream[i..i + n]).ok()?.parse().ok()?;
                i += n;
                let add = match last {
                    Tok::None => return None,
                    Tok::Byte(_) => count,
                    Tok::Word(_) => count.saturating_mul(4),
                };
                if out.len().saturating_add(add) > size {
                    return None;
                }
                for _ in 0..count {
                    match last {
                        Tok::None => return None,
                        Tok::Byte(b) => out.push(b),
                        Tok::Word(w) => out.extend_from_slice(&w),
                    }
                }
            }
            _ => return None,
        }
    }
    (out.len() == size).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_chunk_rejects_oversized_header_and_rle_overflow() {
        assert_eq!(decode_chunk("$$$$999999999999?!aa=0"), None);
        let over = format!("$$$$ {}?!aa=0", MAX_CHUNK_SIZE + 1);
        assert_eq!(decode_chunk(&over), None);
        // RLE expanding past declared size is rejected without allocating past `size`.
        assert_eq!(decode_chunk("$$$$4?!aa1000000=0"), None);
        // Valid small chunk decodes normally.
        assert_eq!(decode_chunk("$$$$3?!ab2=0"), Some(vec![0x01, 0x01, 0x01]));
    }
}
