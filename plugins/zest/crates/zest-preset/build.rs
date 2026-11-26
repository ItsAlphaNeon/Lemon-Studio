// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Compiles the folders of `banks/` (repository root) into the plugin: `banks/<n> <Bank>/<nn> <Preset>.zest`.
//! Swapping the shipped presets means swapping those folders and building again.

use std::fmt::Write as _;
use std::path::PathBuf;

fn sorted(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir).map(|d| d.flatten().map(|e| e.path()).collect()).unwrap_or_default();
    v.sort();
    v
}

fn main() {
    let root = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("../../banks");
    println!("cargo:rerun-if-changed={}", root.display());
    let mut out = String::from("pub static EMBEDDED: &[Embedded] = &[\n");
    for bank in sorted(&root).into_iter().filter(|p| p.is_dir()) {
        println!("cargo:rerun-if-changed={}", bank.display());
        let folder = bank.file_name().unwrap().to_string_lossy().into_owned();
        for f in sorted(&bank).into_iter().filter(|p| p.extension().is_some_and(|x| x == "zest")) {
            let file = f.file_name().unwrap().to_string_lossy().into_owned();
            let f = f.canonicalize().unwrap();
            writeln!(out, "    Embedded {{ folder: {folder:?}, file: {file:?}, text: include_str!({:?}) }},", f.display().to_string()).unwrap();
        }
    }
    out.push_str("];\n");
    let dest = PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("embedded.rs");
    std::fs::write(dest, out).unwrap();
}
