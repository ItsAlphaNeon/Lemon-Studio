// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! `cargo run -p notices`: writes the last section of THIRD-PARTY-NOTICES.txt, the Rust crates Lemon Studio is built
//! with and their licence texts, with cargo-about (`cargo install cargo-about --features cli`) from `about.hbs` and
//! `about.toml` next to this crate's manifest. Everything from the section's heading to the end of the file is
//! replaced; the sections above it are written by hand.
//!
//! `cargo test -p notices` (part of `cargo test --workspace`) fails while a crate that goes into a build of Lemon
//! Studio, on any platform, is missing from the section: after a dependency changes, run `cargo run -p notices`.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The notices file, from the repository root.
const FILE: &str = "THIRD-PARTY-NOTICES.txt";

/// The heading of the generated section (`about.hbs` writes it under a rule line).
const HEADING: &str = "12. The Rust crates in Cargo.lock";

/// The width the notices are wrapped to.
const WIDTH: usize = 79;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn cargo() -> Command {
    Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
}

fn main() {
    let root = root();
    let out = cargo()
        .current_dir(&root)
        .args(["about", "generate", "--locked", "-c", "tools/notices/about.toml", "tools/notices/about.hbs"])
        .output()
        .expect("cargo runs");
    if !out.status.success() {
        eprint!("{}", String::from_utf8_lossy(&out.stderr));
        eprintln!("cargo about failed; it installs with `cargo install cargo-about --locked --features cli`");
        std::process::exit(1);
    }
    let section = tidy(&String::from_utf8_lossy(&out.stdout));
    let path = root.join(FILE);
    let text = std::fs::read_to_string(&path).expect("THIRD-PARTY-NOTICES.txt").replace('\r', "");
    let kept = match section_start(&text) {
        Some(i) => &text[..i],
        None => &text,
    };
    let kept = kept.trim_end_matches('\n');
    std::fs::write(&path, format!("{kept}\n\n{section}")).expect("THIRD-PARTY-NOTICES.txt written");
    println!("{FILE}: {} crates", listed(&section).len());
}

/// Where the generated section starts in `text`: the rule line above its heading.
fn section_start(text: &str) -> Option<usize> {
    let h = text.find(&format!("\n{HEADING}\n"))?;
    Some(text[..h].rfind('\n').map_or(0, |i| i + 1))
}

/// cargo-about's output as plain text: Unix line ends, no trailing spaces, no runs of blank lines, and each "--- licence:
/// crates" line wrapped.
fn tidy(raw: &str) -> String {
    let mut out = String::new();
    let mut blank = 0;
    for line in raw.replace('\r', "").lines() {
        let line = line.trim_end();
        if line.is_empty() {
            blank += 1;
            if blank > 2 {
                continue;
            }
        } else {
            blank = 0;
        }
        match line.strip_prefix("--- ") {
            Some(rest) => out.push_str(&wrap(&format!("--- {}", rest.trim_end_matches([',', ' '])), WIDTH)),
            None => out.push_str(line),
        }
        out.push('\n');
    }
    format!("{}\n", out.trim_end())
}

/// A "--- licence: crate version, crate version" `line` broken between its crates into lines of at most `width`
/// characters where it can, the later ones indented by four.
fn wrap(line: &str, width: usize) -> String {
    let (head, list) = line.split_once(": ").unwrap_or((line, ""));
    let items: Vec<&str> = list.split(", ").filter(|i| !i.is_empty()).collect();
    let mut pieces = vec![if items.is_empty() { head.to_string() } else { format!("{head}:") }];
    pieces.extend(items.iter().enumerate().map(|(i, item)| if i + 1 < items.len() { format!("{item},") } else { item.to_string() }));
    let mut out = String::new();
    let mut now = String::new();
    for piece in pieces {
        if !now.trim().is_empty() && now.chars().count() + 1 + piece.chars().count() > width {
            out.push_str(&now);
            out.push('\n');
            now = "    ".into();
        } else if !now.is_empty() {
            now.push(' ');
        }
        now.push_str(&piece);
    }
    out.push_str(&now);
    out
}

/// The line after the generated section's list of crates (`about.hbs`).
const TEXTS: &str = "The licence texts follow, each after the crates it covers.";

/// The crates the generated `section` lists, as `name version`.
fn listed(section: &str) -> Vec<String> {
    section
        .lines()
        .take_while(|l| *l != TEXTS)
        .filter_map(|l| l.strip_prefix("  "))
        .filter_map(|l| l.split_once(" (").map(|(crate_, _)| crate_.to_string()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The crates that go into a build on any platform (normal dependencies, procedural macros left out) and are not
    /// the workspace's own, as `name version`, from `cargo tree`.
    fn shipped() -> Vec<String> {
        let out = cargo()
            .current_dir(root())
            .args(["tree", "--workspace", "--locked", "--offline", "--color", "never", "--target", "all", "-e", "normal,no-proc-macro"])
            .args(["--prefix", "none", "--format", "{p}"])
            .output()
            .expect("cargo tree runs");
        assert!(out.status.success(), "cargo tree: {}", String::from_utf8_lossy(&out.stderr));
        let mut v: Vec<String> = String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter_map(|l| {
                let (head, paren) = l.split_once(" (").unwrap_or((l, ""));
                // "(*)" marks a crate shown before; a path (the workspace's crates) is not third-party; a git
                // source is.
                let source = paren.split(')').next().unwrap_or("");
                let ours = !source.is_empty() && source != "*" && !source.starts_with("http") && !source.starts_with("registry");
                let (name, version) = head.trim().split_once(" v")?;
                (!ours).then(|| format!("{name} {version}"))
            })
            .collect();
        v.sort();
        v.dedup();
        v
    }

    /// Every crate shipped is in the generated section.
    #[test]
    fn every_crate_shipped_is_listed() {
        let text = std::fs::read_to_string(root().join(FILE)).expect("THIRD-PARTY-NOTICES.txt").replace('\r', "");
        let start = section_start(&text).expect("the generated section: run `cargo run -p notices`");
        let listed = listed(&text[start..]);
        let shipped = shipped();
        assert!(shipped.len() > 100, "cargo tree found {} crates", shipped.len());
        let missing: Vec<&String> = shipped.iter().filter(|c| !listed.contains(c)).collect();
        assert!(missing.is_empty(), "not in {FILE} (run `cargo run -p notices`): {missing:?}");
    }

    /// The contents list the sections' headings (each between two rule lines) in order, the generated one last.
    #[test]
    fn the_contents_name_every_section() {
        let text = std::fs::read_to_string(root().join(FILE)).expect("THIRD-PARTY-NOTICES.txt").replace('\r', "");
        let rule = "-".repeat(WIDTH);
        let lines: Vec<&str> = text.lines().collect();
        let headings: Vec<&str> = lines.windows(3).filter(|w| w[0] == rule && w[2] == rule).map(|w| w[1]).collect();
        let contents: Vec<&str> = text
            .split("\nContents\n")
            .nth(1)
            .expect("a Contents list")
            .lines()
            .skip_while(|l| l.is_empty())
            .take_while(|l| !l.is_empty())
            .map(str::trim)
            .collect();
        assert_eq!(contents, headings);
        assert_eq!(headings.last(), Some(&HEADING));
    }

    #[test]
    fn wrapping() {
        assert_eq!(wrap("--- MIT License: a 1, b 2, c 3", 24), "--- MIT License: a 1,\n    b 2, c 3", "between crates");
        assert_eq!(wrap("--- MIT License: a 1, b 2", 8), "--- MIT License:\n    a 1,\n    b 2");
        assert_eq!(tidy("a  \r\n\r\n\r\n\r\nb\n--- X: a 1, \n"), "a\n\n\nb\n--- X: a 1\n");
    }
}
