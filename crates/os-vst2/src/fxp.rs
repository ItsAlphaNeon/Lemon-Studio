// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! VST 2 preset files: `.fxp` (one program) and `.fxb` (a bank), the layout the VST 2 SDK documents for them, read
//! into [`Saved`] states (spec: plugins/presets.md "Import").
//!
//! Every number is big-endian. A file starts with `CcnK`, the size of what follows, then a kind: `FxCk` (a program's
//! parameter values), `FPCh` (a program's chunk), `FxBk` (a bank of `FxCk` / `FPCh` programs) or `FBCh` (a bank's
//! chunk). Then the format version, the plugin's unique id and version, and the parameter / program count. A program
//! has its 28-byte name next, then the values (`f32` each) or the chunk's size and bytes. A bank has 128 reserved
//! bytes (the current program and 124 reserved in version 2), then its programs, each a whole program record, or the
//! chunk's size and bytes.

use crate::state::Saved;

/// A preset file's contents: the plugin's unique id and its programs (name, state). A program's state is a
/// parameter list without a program number, or the plugin's chunk.
#[derive(Debug, Clone, PartialEq)]
pub struct Fxp {
    pub unique_id: i32,
    pub programs: Vec<(String, Saved)>,
}

const PROGRAM_HEAD: usize = 56;
const BANK_HEAD: usize = 156;

fn be(data: &[u8], at: usize) -> Option<i32> {
    Some(i32::from_be_bytes(data.get(at..at + 4)?.try_into().ok()?))
}

fn count(data: &[u8], at: usize) -> Option<usize> {
    usize::try_from(be(data, at)?).ok()
}

/// One program record at the start of `data`: (name, state, its length).
fn program(data: &[u8]) -> Option<(String, Saved, usize)> {
    if data.get(..4)? != b"CcnK" {
        return None;
    }
    let raw = data.get(28..PROGRAM_HEAD)?;
    let name = String::from_utf8_lossy(&raw[..raw.iter().position(|&c| c == 0).unwrap_or(raw.len())]).trim().to_string();
    match data.get(8..12)? {
        b"FxCk" => {
            let n = count(data, 24)?;
            let vals = data.get(PROGRAM_HEAD..PROGRAM_HEAD.checked_add(n.checked_mul(4)?)?)?;
            let values = vals.chunks_exact(4).map(|v| f32::from_be_bytes(v.try_into().unwrap_or_default())).collect();
            Some((name, Saved::Params { program: None, values }, PROGRAM_HEAD + n * 4))
        }
        b"FPCh" => {
            let n = count(data, PROGRAM_HEAD)?;
            let at = PROGRAM_HEAD + 4;
            let chunk = data.get(at..at.checked_add(n)?)?;
            Some((name, Saved::Chunk(chunk.to_vec()), at + n))
        }
        _ => None,
    }
}

/// Reads `.fxp` / `.fxb` bytes; `None` when they are not one, or run short.
pub fn parse(data: &[u8]) -> Option<Fxp> {
    if data.get(..4)? != b"CcnK" {
        return None;
    }
    let unique_id = be(data, 16)?;
    let programs = match data.get(8..12)? {
        b"FxCk" | b"FPCh" => vec![program(data).map(|(n, s, _)| (n, s))?],
        b"FxBk" => {
            let n = count(data, 24)?;
            let mut at = BANK_HEAD;
            let mut v = Vec::new();
            for _ in 0..n.min(4096) {
                let (name, s, len) = program(data.get(at..)?)?;
                v.push((name, s));
                at += len;
            }
            v
        }
        b"FBCh" => {
            let n = count(data, BANK_HEAD)?;
            let at = BANK_HEAD + 4;
            vec![(String::new(), Saved::Chunk(data.get(at..at.checked_add(n)?)?.to_vec()))]
        }
        _ => return None,
    };
    Some(Fxp { unique_id, programs })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn head(kind: &[u8; 4], id: i32, n: i32) -> Vec<u8> {
        let mut b = b"CcnK".to_vec();
        b.extend(0i32.to_be_bytes());
        b.extend(kind);
        b.extend(1i32.to_be_bytes());
        b.extend(id.to_be_bytes());
        b.extend(1i32.to_be_bytes());
        b.extend(n.to_be_bytes());
        b
    }

    fn named(mut b: Vec<u8>, name: &str) -> Vec<u8> {
        let mut n = [0u8; 28];
        n[..name.len()].copy_from_slice(name.as_bytes());
        b.extend(n);
        b
    }

    fn params(name: &str, vals: &[f32]) -> Vec<u8> {
        let mut b = named(head(b"FxCk", 0x41424344, vals.len() as i32), name);
        vals.iter().for_each(|v| b.extend(v.to_be_bytes()));
        b
    }

    #[test]
    fn programs_and_banks_read() {
        let p = parse(&params("Warm pad", &[0.5, 1.0])).unwrap();
        assert_eq!(p.unique_id, 0x41424344);
        assert_eq!(p.programs, [("Warm pad".to_string(), Saved::Params { program: None, values: vec![0.5, 1.0] })]);
        let mut c = named(head(b"FPCh", 7, 0), "Chunky");
        c.extend(3i32.to_be_bytes());
        c.extend([1, 2, 3]);
        assert_eq!(parse(&c).unwrap().programs, [("Chunky".to_string(), Saved::Chunk(vec![1, 2, 3]))]);
        let mut bank = head(b"FxBk", 0x41424344, 2);
        bank.extend([0u8; 128]);
        bank.extend(params("One", &[0.1]));
        bank.extend(params("Two", &[0.2]));
        let names: Vec<String> = parse(&bank).unwrap().programs.into_iter().map(|(n, _)| n).collect();
        assert_eq!(names, ["One", "Two"]);
        let mut bc = head(b"FBCh", 9, 4);
        bc.extend([0u8; 128]);
        bc.extend(2i32.to_be_bytes());
        bc.extend([8, 9]);
        assert_eq!(parse(&bc).unwrap().programs, [(String::new(), Saved::Chunk(vec![8, 9]))]);
    }

    #[test]
    fn other_bytes_are_refused() {
        assert_eq!(parse(b""), None);
        assert_eq!(parse(b"VST3"), None);
        let mut b = params("x", &[0.5, 0.5]);
        b.truncate(b.len() - 1);
        assert_eq!(parse(&b), None);
        let mut b = params("x", &[0.5]);
        b[8..12].copy_from_slice(b"Nope");
        assert_eq!(parse(&b), None);
    }
}
