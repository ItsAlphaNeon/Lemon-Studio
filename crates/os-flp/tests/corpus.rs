// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

// spec: flp/container.md (Conformance requirements; Alignment checks)
//
// Corpus round-trip. Runs only when OPENSTUDIO_FLP_CORPUS names a text file listing FLP
// paths, one per line (blank lines ignored). The corpus is never committed.

use os_flp::{FlpFile, check_alignment};

#[test]
fn corpus_roundtrip_and_alignment() {
    let Some(list) = std::env::var_os("OPENSTUDIO_FLP_CORPUS") else {
        eprintln!("OPENSTUDIO_FLP_CORPUS not set; skipping corpus test");
        return;
    };
    let list = std::fs::read_to_string(&list)
        .unwrap_or_else(|e| panic!("cannot read corpus list {}: {e}", list.to_string_lossy()));
    let paths: Vec<&str> = list
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    assert!(!paths.is_empty(), "corpus list is empty");

    let mut failures = Vec::new();
    for path in &paths {
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) => {
                failures.push(format!("{path}: read failed: {e}"));
                continue;
            }
        };
        let file = match FlpFile::parse(&bytes) {
            Ok(f) => f,
            Err(e) => {
                failures.push(format!("{path}: {e}"));
                continue;
            }
        };
        match file.to_bytes() {
            Ok(out) if out == bytes => {}
            Ok(out) => {
                let at = out.iter().zip(&bytes).position(|(a, b)| a != b);
                let at = at.unwrap_or(out.len().min(bytes.len()));
                failures.push(format!(
                    "{path}: round-trip differs at offset {at:#x} ({} vs {} bytes)",
                    out.len(),
                    bytes.len()
                ));
            }
            Err(e) => failures.push(format!("{path}: write failed: {e}")),
        }
        for issue in check_alignment(&file) {
            failures.push(format!("{path}: alignment: {issue}"));
        }
    }

    assert!(
        failures.is_empty(),
        "{} of {} corpus files failed:\n{}",
        failures.len(),
        paths.len(),
        failures.join("\n")
    );
    eprintln!(
        "{} corpus files passed round-trip and alignment checks",
        paths.len()
    );
}
