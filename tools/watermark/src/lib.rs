// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

//! The repository watermark: every source file opens with a header naming ItsAlphaNeon as copyright holder, the SPDX
//! licence identifier, and the notice that this code, in its entirety, is not to be used as part of the ArtCraft suite.
//! `cargo test -p watermark` (part of `cargo test --workspace`) fails on any source file without it.
//!
//! The header is the same five lines in each language's comments, followed by an empty line. It comes first in the
//! file, after the `#!` line in a script that has one:
//!
//! - `//` in Rust and resource scripts (`.rs`, `.rc`)
//! - `/*`, then ` * ` lines and ` */`, in C and its relatives (`.c`, `.h`, `.cpp`, `.hpp`, `.m`, `.mm`)
//! - `#` in Python, shell and PowerShell scripts (`.py`, `.sh`, `.ps1`)
//!
//! The Tabler icon and logo generators (`tools/tabler-icons/gen.py`, `tools/logo/gen.py`) write it into the files they
//! generate.

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    /// The header's lines, without their comment markers.
    const HEADER: [&str; 5] = [
        "Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio",
        "SPDX-License-Identifier: GPL-3.0-or-later",
        "",
        "This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,",
        "and not any other part of Lemon Studio.",
    ];

    /// How a source file writes comments.
    #[derive(Clone, Copy)]
    enum Style {
        /// `// line`
        Slash,
        /// `/* line`, ` * line`, ` */`
        Block,
        /// `# line`
        Hash,
    }

    /// A file's comment style, from its extension; `None` when it is not source code.
    fn style(path: &Path) -> Option<Style> {
        match path.extension()?.to_str()? {
            "rs" | "rc" => Some(Style::Slash),
            "c" | "h" | "cpp" | "hpp" | "m" | "mm" => Some(Style::Block),
            "py" | "sh" | "ps1" => Some(Style::Hash),
            _ => None,
        }
    }

    /// The header written in `style`.
    fn header(style: Style) -> Vec<String> {
        let line = |marker: &str, text: &str| if text.is_empty() { marker.into() } else { format!("{marker} {text}") };
        match style {
            Style::Slash => HEADER.iter().map(|t| line("//", t)).collect(),
            Style::Hash => HEADER.iter().map(|t| line("#", t)).collect(),
            Style::Block => {
                let mut lines: Vec<String> =
                    HEADER.iter().enumerate().map(|(i, t)| line(if i == 0 { "/*" } else { " *" }, t)).collect();
                lines.push(" */".into());
                lines
            }
        }
    }

    /// Whether `text` opens with the header in `style`, after a `#!` line if it has one, followed by an empty line.
    fn has_header(text: &str, style: Style) -> bool {
        let mut lines = text.strip_prefix('\u{feff}').unwrap_or(text).lines().peekable();
        lines.next_if(|l| l.starts_with("#!"));
        header(style).iter().all(|h| lines.next() == Some(h.as_str())) && lines.next().is_none_or(str::is_empty)
    }

    /// The source files under `dir`, leaving out what `.gitignore` keeps out of the repository: build output (`target`,
    /// `target-*`) and `.claude`, which holds worktrees (whole copies of the repository); and `.git`.
    fn sources(dir: &Path, out: &mut Vec<(PathBuf, Style)>) {
        let list = std::fs::read_dir(dir).unwrap_or_else(|e| panic!("cannot list {}: {e}", dir.display()));
        for entry in list {
            let entry = entry.unwrap_or_else(|e| panic!("cannot list {}: {e}", dir.display()));
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                if !matches!(&*name, ".git" | ".claude" | "target") && !name.starts_with("target-") {
                    sources(&path, out);
                }
            } else if let Some(style) = style(&path) {
                out.push((path, style));
            }
        }
    }

    /// Every source file in the repository opens with the header (see the crate documentation).
    #[test]
    fn every_source_file_has_the_header() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut files = Vec::new();
        sources(&root, &mut files);
        assert!(!files.is_empty(), "no source files under {}", root.display());

        let mut missing: Vec<String> = files
            .iter()
            .filter(|(path, style)| !std::fs::read_to_string(path).is_ok_and(|text| has_header(&text, *style)))
            .map(|(path, _)| path.strip_prefix(&root).unwrap_or(path).to_string_lossy().replace('\\', "/"))
            .collect();
        missing.sort();
        assert!(
            missing.is_empty(),
            "{} of {} source files don't open with the repository header:\n  {}\n\nA source file starts with these \
             lines and an empty line, after its #! line if it has one (# comments in Python and shell, /* */ in \
             C):\n\n{}\n",
            missing.len(),
            files.len(),
            missing.join("\n  "),
            header(Style::Slash).join("\n")
        );
    }

    /// The header counts in each comment style, after a `#!` line and with Windows line endings; not in another
    /// language's comments, not below other lines, and not without the empty line after it.
    #[test]
    fn header_check() {
        let slash = header(Style::Slash).join("\n");
        assert!(has_header(&format!("{slash}\n\n//! Docs.\n"), Style::Slash));
        assert!(has_header(&format!("{slash}\n\n//! Docs.\n").replace('\n', "\r\n"), Style::Slash));
        assert!(has_header(&format!("{}\n\nint x;\n", header(Style::Block).join("\n")), Style::Block));
        assert!(has_header(&format!("#!/bin/sh\n{}\n\necho\n", header(Style::Hash).join("\n")), Style::Hash));

        assert!(!has_header("//! Docs.\n", Style::Slash));
        assert!(!has_header(&format!("{}\n\n", header(Style::Hash).join("\n")), Style::Slash));
        assert!(!has_header(&format!("//! Docs.\n{slash}\n\n"), Style::Slash));
        assert!(!has_header(&format!("{slash}\n//! Docs.\n"), Style::Slash));
    }
}
