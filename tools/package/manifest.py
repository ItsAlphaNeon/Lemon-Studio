#!/usr/bin/env python3
# Copyright (C) 2026 ItsAlphaNeon
# SPDX-License-Identifier: GPL-3.0-or-later
# This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

"""Writes a release channel's manifest (`nightly.json` or `stable.json`) for Lemon Studio's updater
(`tools/updater`, whose `manifest` module reads it): the release's version, commit and changes, and per platform the
program to download, with its size and SHA-256. The release workflow signs it with minisign.

    manifest.py --channel nightly --version 2026.11.7.1 --commit <sha> --repo owner/name --tag nightly-2026.11.7.1 \\
        --notes generated-notes.md --dist dist --out dist/nightly.json

The programs are found in `dist` by name (`programs`), per platform as Rust names it. The plugin downloads
(`LemonStudio-Plugins-...`) are not programs to update and are left out. The notes are GitHub's generated release
notes, one pull request a line.
"""

import argparse
import datetime
import hashlib
import json
import os
import re
import sys


def notes(markdown):
    """The pull requests' titles in GitHub's generated notes ("* Title by @someone in https://.../pull/1"), from their
    "What's Changed" section (not "New Contributors")."""
    out = []
    listing = True
    for line in markdown.splitlines():
        if line.startswith("#"):
            listing = "what's changed" in line.lower()
            continue
        m = re.match(r"^\s*[*-]\s+(.+?)(?:\s+by\s+@\S+)?(?:\s+in\s+https://\S+)?\s*$", line)
        if listing and m:
            out.append(m.group(1))
    return out


def programs(channel, version):
    """The file holding the program, per platform. The `.exe` and the AppImage carry no version: an update replaces the
    file in place, so a shortcut to it never breaks. A nightly's have a name of their own, so that a nightly and a
    tagged release can sit in one folder. macOS's zip is only unpacked; the `.app` inside has a fixed name."""
    stem = "LemonStudioNightly" if channel == "nightly" else "LemonStudio"
    return {
        "windows-x86_64": f"{stem}.exe",
        "linux-x86_64": f"{stem}.AppImage",
        "macos-aarch64": f"LemonStudio-{version}-macos-aarch64.zip",
    }


def assets(dist, channel, version, base):
    found = {}
    for platform, name in programs(channel, version).items():
        path = os.path.join(dist, name)
        if not os.path.isfile(path):
            continue
        h = hashlib.sha256()
        with open(path, "rb") as f:
            for chunk in iter(lambda: f.read(1 << 20), b""):
                h.update(chunk)
        found[platform] = {"url": f"{base}/{name}", "size": os.path.getsize(path), "sha256": h.hexdigest()}
    return found


def main():
    p = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    for arg in ("channel", "version", "commit", "repo", "tag", "notes", "dist", "out"):
        p.add_argument("--" + arg, required=True)
    a = p.parse_args()
    if a.channel not in ("nightly", "stable"):
        sys.exit(f"unknown channel {a.channel!r}")
    with open(a.notes, encoding="utf-8") as f:
        changes = notes(f.read())
    page = f"https://github.com/{a.repo}/releases/tag/{a.tag}"
    found = assets(a.dist, a.channel, a.version, f"https://github.com/{a.repo}/releases/download/{a.tag}")
    if not found:
        sys.exit(f"none of the programs in {a.dist}: {', '.join(programs(a.channel, a.version).values())}")
    manifest = {
        "channel": a.channel,
        "version": a.version,
        "commit": a.commit,
        "date": datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "page": page,
        "notes": changes,
        "assets": found,
    }
    with open(a.out, "w", encoding="utf-8", newline="\n") as f:
        json.dump(manifest, f, indent=2)
        f.write("\n")
    print(f"{a.out}: {a.channel} {a.version}, {', '.join(found)}; {len(changes)} changes")


if __name__ == "__main__":
    main()
