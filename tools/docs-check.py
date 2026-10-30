#!/usr/bin/env python3
# Copyright (C) 2026 thacer13
# SPDX-License-Identifier: GPL-3.0-or-later
# This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
"""Docs that point at what is gone: cutting-edge's README and docs/ name files, links and code (`crate::journal`,
`App::save_copy`, `rt::tests::deadlines_under_load`); each must still exist. Run from the repository root:
`python3 tools/docs-check.py` (CI runs it). Prints one line per stale reference; exits 1 when there is one."""

import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
DOCS = [ROOT / "README.md", *sorted((ROOT / "docs").glob("*.md"))]
# Folders a path in backticks may start with.
TOP = ("crates/", "docs/", "tools/", "plugins/", ".github/", "assets/")

code = "\n".join(p.read_text(errors="ignore") for d in ("crates", "plugins", "tools") for p in (ROOT / d).rglob("*.rs"))
# Every item name the code defines.
DEFINED = set(re.findall(r"\b(?:fn|struct|enum|trait|type|const|static|mod|macro_rules!)\s+([A-Za-z_]\w*)", code))
# Enum variants and fields count too (`Then::CrashBackup`, `State::plugin_edited`).
WORDS = set(re.findall(r"\b([A-Za-z_]\w*)\b", code))
# Names compared without case or underscores: a C interface's `canResize` is the Rust `can_resize`.
FOLDED = {w.replace("_", "").lower() for w in WORDS}
MODULES = {p.stem for d in ("crates", "plugins") for p in (ROOT / d).rglob("*.rs")} | {p.parent.name for d in ("crates", "plugins") for p in (ROOT / d).rglob("mod.rs")}
CRATES = {p.parent.name.replace("-", "_") for d in ("crates", "plugins", "tools") for p in (ROOT / d).glob("*/Cargo.toml")}


def stale_name(path: str) -> bool:
    """A Rust path in backticks (`a::b::c`) whose parts the code does not define."""
    parts = [p for p in path.split("::") if p not in ("crate", "self", "super", "tests")]
    if not parts:
        return False
    *heads, last = parts
    for h in heads:
        if h not in MODULES and h not in CRATES and h not in DEFINED and h.replace("_", "").lower() not in FOLDED:
            return True
    # The last part: an item, else at least a variant or field the code spells.
    return last not in DEFINED and last.replace("_", "").lower() not in FOLDED


def main() -> int:
    bad = []
    for doc in DOCS:
        text = doc.read_text()
        rel = doc.relative_to(ROOT)
        # Fenced blocks are examples, not references.
        text = re.sub(r"```.*?```", "", text, flags=re.S)
        for n, line in enumerate(text.splitlines(), 1):
            for target in re.findall(r"\]\(([^)#\s]+)(?:#[^)]*)?\)", line):
                if re.match(r"[a-z]+:", target):
                    continue
                if not (doc.parent / target).exists():
                    bad.append(f"{rel}:{n}: link to {target}")
            for tick in re.findall(r"`([^`\s]+)`", line):
                t = tick.rstrip(".,;:")
                if t.startswith(TOP) and not any(c in t for c in "*<{"):
                    if not (ROOT / t.split(":")[0]).exists():
                        bad.append(f"{rel}:{n}: path {t}")
                elif re.fullmatch(r"[A-Za-z_]\w*(::[A-Za-z_]\w*)+", t) and stale_name(t):
                    bad.append(f"{rel}:{n}: code {t}")
    for b in bad:
        print(b)
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
