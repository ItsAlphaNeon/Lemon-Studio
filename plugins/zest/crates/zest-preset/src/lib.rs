// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Zest's preset files (`.zest`) and the banks built into the plugin.
//!
//! A `.zest` file is text. A header of `key: value` lines, then `---`, then one `module/name = value` line for each
//! parameter that differs from its default. Accuracy, offline accuracy and multicore are never stored: they stay as
//! they are when a preset loads.
//!
//! ```text
//! #zest 1
//! name: Warm Bass
//! author: Someone
//! categories: Bass:Analogue, Leads:Synth
//! pots cutoff: -15.1 100 _ 3 0 0 0 0
//! reroll: glide
//! ---
//! VCF1/Frequency = 80
//! ```
//!
//! `pots <group>` holds a voice's analog spread values, eight per group (`_`: this preset leaves that one as it is);
//! `reroll` names the groups drawn afresh when the preset loads. A file without pots leaves them all as they are.
//! `tuning` holds 128 numbers, the pitch of each key after transpose, when they are not the identity.
//!
//! **Where presets live.** A bank is a folder, `<n> <Bank>` with presets `<nn> <Preset>.zest`; the numbers give the order
//! and are not shown. The first bank is the tab the browser opens on.
//! - `banks/` at the repository root is compiled into the plugin. Swap those folders and build to ship other presets.
//! - `<Zest's user folder>/banks/` takes the same folders at run time, with no build.

use std::path::{Path, PathBuf};

/// What the first line of a `.zest` file says.
pub const MAGIC: &str = "#zest 1";
/// The start of a built-in preset's path.
pub const SCHEME: &str = "zest-bank:/";
/// The spread groups in file order (cutoff, glide, pulse width, envelope).
pub const GROUPS: [&str; 4] = ["cutoff", "glide", "pw", "env"];

/// A preset compiled into the plugin.
pub struct Embedded {
    pub folder: &'static str,
    pub file: &'static str,
    pub text: &'static str,
}

include!(concat!(env!("OUT_DIR"), "/embedded.rs"));

/// The analog spread a preset brings.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Pots {
    pub pots: [[f32; 8]; 4],
    pub have: [[bool; 8]; 4],
    pub reroll: [bool; 4],
    /// The key table (microtuning): the pitch of each key after transpose.
    pub tuning: Option<[f32; 128]>,
}

/// The header alone (a library reads only this).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Head {
    pub name: Option<String>,
    pub author: Option<String>,
    pub categories: Option<String>,
}

impl Head {
    /// The first of the categories, up to its colon ("Bass:Analogue, Leads:Synth" -> "Bass").
    pub fn first_category(&self) -> Option<&str> {
        self.categories.as_deref()?.split(',').next()?.split(':').next().map(str::trim).filter(|c| !c.is_empty())
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PresetFile {
    pub head: Head,
    pub spread: Pots,
    /// Parameter key and plain value.
    pub params: Vec<(String, f32)>,
}

/// Reads the header. `None`: not a `.zest` text.
pub fn parse_head(text: &str) -> Option<Head> {
    let mut lines = text.lines();
    if lines.next()?.trim() != MAGIC {
        return None;
    }
    let mut h = Head::default();
    for l in lines {
        if l.trim() == "---" {
            break;
        }
        let Some((k, v)) = l.split_once(':') else { continue };
        let v = v.trim();
        if v.is_empty() {
            continue;
        }
        match k.trim() {
            "name" => h.name = Some(v.to_string()),
            "author" => h.author = Some(v.to_string()),
            "categories" => h.categories = Some(v.to_string()),
            _ => {}
        }
    }
    Some(h)
}

pub fn parse(text: &str) -> Result<PresetFile, String> {
    let head = parse_head(text).ok_or("not a Zest preset")?;
    let mut spread = Pots::default();
    let mut params = Vec::new();
    let mut body = false;
    for (n, l) in text.lines().enumerate().skip(1) {
        let l = l.trim();
        if l.is_empty() {
            continue;
        }
        if !body {
            if l == "---" {
                body = true;
            } else if let Some(rest) = l.strip_prefix("pots ") {
                let (g, vals) = rest.split_once(':').ok_or_else(|| format!("line {}: pots", n + 1))?;
                let g = GROUPS.iter().position(|x| *x == g.trim()).ok_or_else(|| format!("line {}: pots group", n + 1))?;
                for (v, word) in vals.split_whitespace().take(8).enumerate() {
                    if word != "_" {
                        spread.pots[g][v] = word.parse().map_err(|_| format!("line {}: {word}", n + 1))?;
                        spread.have[g][v] = true;
                    }
                }
            } else if let Some(rest) = l.strip_prefix("tuning:") {
                let mut t = [0.0f32; 128];
                let mut n = 0;
                for w in rest.split_whitespace().take(128) {
                    t[n] = w.parse().map_err(|_| format!("tuning: {w}"))?;
                    n += 1;
                }
                if n == 128 {
                    spread.tuning = Some(t);
                }
            } else if let Some(rest) = l.strip_prefix("reroll:") {
                for w in rest.split_whitespace() {
                    if let Some(g) = GROUPS.iter().position(|x| *x == w) {
                        spread.reroll[g] = true;
                    }
                }
            }
            continue;
        }
        let (k, v) = l.split_once('=').ok_or_else(|| format!("line {}: no =", n + 1))?;
        let v = v.trim().parse::<f32>().map_err(|_| format!("line {}: {}", n + 1, v.trim()))?;
        params.push((k.trim().to_string(), v));
    }
    Ok(PresetFile { head, spread, params })
}

pub fn write(p: &PresetFile) -> String {
    let mut s = String::from(MAGIC);
    s.push('\n');
    let mut field = |k: &str, v: &Option<String>| {
        if let Some(v) = v {
            s.push_str(&format!("{k}: {v}\n"));
        }
    };
    field("name", &p.head.name);
    field("author", &p.head.author);
    field("categories", &p.head.categories);
    for g in 0..4 {
        if p.spread.have[g].iter().any(|h| *h) {
            let vals: Vec<String> = (0..8).map(|v| if p.spread.have[g][v] { format!("{}", p.spread.pots[g][v]) } else { "_".into() }).collect();
            s.push_str(&format!("pots {}: {}\n", GROUPS[g], vals.join(" ")));
        }
    }
    let rr: Vec<&str> = (0..4).filter(|g| p.spread.reroll[*g]).map(|g| GROUPS[g]).collect();
    if !rr.is_empty() {
        s.push_str(&format!("reroll: {}\n", rr.join(" ")));
    }
    if let Some(t) = &p.spread.tuning {
        if t.iter().enumerate().any(|(i, x)| *x != i as f32) {
            let vals: Vec<String> = t.iter().map(|x| format!("{x}")).collect();
            s.push_str(&format!("tuning: {}\n", vals.join(" ")));
        }
    }
    s.push_str("---\n");
    for (k, v) in &p.params {
        s.push_str(&format!("{k} = {v}\n"));
    }
    s
}

/// A name without its leading order number: "07 Joy" -> "Joy", "1 Space" -> "Space".
pub fn unnumbered(s: &str) -> &str {
    let t = s.trim_start_matches(|c: char| c.is_ascii_digit());
    match t.strip_prefix(' ') {
        Some(rest) if t.len() < s.len() && !rest.is_empty() => rest,
        _ => s,
    }
}

/// The built-in preset's path.
pub fn embedded_path(e: &Embedded) -> PathBuf {
    PathBuf::from(format!("{SCHEME}{}/{}", e.folder, e.file))
}

/// The text of a preset file: a built-in one, or from disk. Not valid UTF-8: replaced, as presets are mostly ASCII.
pub fn read(path: &Path) -> Result<String, String> {
    let shown = path.to_string_lossy();
    if let Some(rest) = shown.strip_prefix(SCHEME) {
        let (folder, file) = rest.split_once('/').ok_or("bad built-in preset path")?;
        return EMBEDDED.iter().find(|e| e.folder == folder && e.file == file).map(|e| e.text.to_string()).ok_or_else(|| "this build does not hold that preset".to_string());
    }
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_keeps_every_value() {
        let mut p = PresetFile::default();
        p.head.name = Some("Warm Bass".into());
        p.head.author = Some("Someone".into());
        p.head.categories = Some("Bass:Analogue, Leads:Synth".into());
        p.spread.pots[1] = [-15.123457, 100.0, 0.1, 3.0, 0.0, 0.0, 0.0, -0.0000012];
        p.spread.have[1] = [true, true, true, true, false, false, true, true];
        p.spread.pots[1][4] = 0.0;
        p.spread.pots[1][5] = 0.0;
        p.spread.reroll = [false, true, false, true];
        p.params = vec![("VCF1/Frequency".into(), 80.125), ("main/Active #FX2".into(), 1.0)];
        let t = write(&p);
        assert_eq!(parse(&t).unwrap(), p);
        assert_eq!(parse_head(&t).unwrap().first_category(), Some("Bass"));
    }

    #[test]
    fn other_text_is_not_a_preset() {
        assert!(parse("/*@Meta\n*/\n#AM=Diva\n").is_err());
        assert_eq!(parse_head("{}"), None);
    }

    #[test]
    fn order_numbers_are_not_shown() {
        assert_eq!(unnumbered("07 Joy"), "Joy");
        assert_eq!(unnumbered("1 Space"), "Space");
        assert_eq!(unnumbered("Joy"), "Joy");
        assert_eq!(unnumbered("808 "), "808 ");
        assert_eq!(unnumbered("808"), "808");
    }

    #[test]
    fn built_in_presets_are_found_and_valid() {
        for e in EMBEDDED {
            assert_eq!(read(&embedded_path(e)).unwrap(), e.text);
            parse(e.text).unwrap_or_else(|m| panic!("{}: {m}", e.file));
        }
    }
}
