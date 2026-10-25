# Copyright (C) 2026 ItsAlphaNeon
# SPDX-License-Identifier: GPL-3.0-or-later
# This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

"""Generate the Lemon Studio logo data from crates/os-app/assets/logo/*-lemon.svg.

    python tools/logo/gen.py            (run from the repository root)
    python tools/logo/gen.py --rs       (logo.rs only: no Pillow needed)

Writes:
  crates/os-app/src/logo.rs        the full-colour logo's parts (the startup splash) and the greyscale mark (drawn in
                                   one tint), as absolute M/L/Q/C/Z path data
  crates/os-app/assets/lemonstudio.ico  app icon, 16..256 px (exe, window, taskbar), from the owner's
                                        bitmap assets/logo/icon-lemon.png (edited in icon-lemon.pdn, Paint.NET)
  crates/os-app/assets/lemonstudio.png  the same icon at 256 px (Linux: window icon and desktop entry)
"""
import importlib.util
import os
import re
import sys
import xml.etree.ElementTree as ET

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
ASSETS = os.path.join(ROOT, "crates", "os-app", "assets")
spec = importlib.util.spec_from_file_location("tabler_gen", os.path.join(ROOT, "tools", "tabler-icons", "gen.py"))
tg = importlib.util.module_from_spec(spec)
spec.loader.exec_module(tg)

# The full-colour logo's parts, back to front: constant name and the id of the path (or of its group) in the SVG.
PARTS = [("LABEL", "Label"), ("SHIMMER", "Shimmer"), ("RECORD", "Record"), ("CENTER", "Record_Center"), ("RIND", "Rind")]
SLICES = ["Slice_1", "Slice_2", "Slice_3", "Slice_4"]


def layers(svg):
    root = ET.parse(svg).getroot()
    vb = [float(v) for v in root.attrib["viewBox"].split()]
    style = "".join(e.text or "" for e in root.iter() if e.tag.endswith("style"))
    colors = dict(re.findall(r"\.([\w-]+)\{fill:(#[0-9a-fA-F]{6})\}", style))
    out = []
    for el in root.iter():
        if el.tag.endswith("path"):
            out.append((colors.get(el.attrib.get("class"), "#000000"), tg.convert(el.attrib["d"])))
    return vb, out


def parts_by_id(svg):
    """Path data by id: a path's own, or its group's when the path has none (the label)."""
    found = {}
    for el in ET.parse(svg).getroot().iter():
        if el.tag.endswith("path") and "id" in el.attrib:
            found[el.attrib["id"]] = el.attrib["d"]
        elif el.tag.endswith("g") and "id" in el.attrib:
            paths = [c for c in el if c.tag.endswith("path") and "id" not in c.attrib]
            if len(paths) == 1:
                found[el.attrib["id"]] = paths[0].attrib["d"]
    return found


def d_string(cmds):
    return " ".join(c[0] + "".join(" " + tg.fmt(v) for v in c[1:]) for c in cmds)


def write_rs():
    vb, grey = layers(os.path.join(ASSETS, "logo", "logo-greyscale-lemon.svg"))
    colour = parts_by_id(os.path.join(ASSETS, "logo", "logo-colored-lemon.svg"))
    d = {name: d_string(tg.convert(colour[sid])) for name, sid in PARTS}
    slices = [d_string(tg.convert(colour[sid])) for sid in SLICES]
    # The greyscale mark is the coloured one's shapes without the label and the shimmer.
    shared = [d["RECORD"], d["CENTER"], d["RIND"]] + slices
    assert [d_string(cmds) for _, cmds in grey] == shared, "the greyscale and full-colour logos differ in shape"
    lines = tg.HEADER + [
        "//! Lemon Studio logo, generated from `assets/logo/logo-colored-lemon.svg` and `logo-greyscale-lemon.svg` by",
        "//! `tools/logo/gen.py`: do not edit. Absolute M/L/Q/C/Z path data in viewBox units.",
        "",
        "pub const VIEW_W: f32 = %s;" % tg.fmt(vb[2]),
        "pub const VIEW_H: f32 = %s;" % tg.fmt(vb[3]),
        "",
        "// The full-colour logo's parts, back to front (the startup splash). The greyscale mark is the same shapes without",
        "// the label and the shimmer seen through the record's two windows.",
    ]
    lines += ['pub const %s: &str = "%s";' % (name, d[name]) for name, _ in PARTS]
    lines.append("pub const SLICES: [&str; 4] = [")
    lines += ['    "%s",' % s for s in slices]
    lines += [
        "];",
        "",
        "/// The greyscale mark, drawn in one tint (`icons::logo`).",
        "pub const PATHS: &[&str] = &[RECORD, CENTER, RIND, SLICES[0], SLICES[1], SLICES[2], SLICES[3]];",
    ]
    with open(os.path.join(ROOT, "crates", "os-app", "src", "logo.rs"), "w", newline="\n") as f:
        f.write("\n".join(lines) + "\n")


def write_icons():
    from PIL import Image

    src = Image.open(os.path.join(ASSETS, "logo", "icon-lemon.png")).convert("RGBA")
    src = src.crop(src.getchannel("A").getbbox())  # no transparent margin: fill the whole icon cell
    side = max(src.size)
    sq = Image.new("RGBA", (side, side), (0, 0, 0, 0))
    sq.paste(src, ((side - src.width) // 2, (side - src.height) // 2))
    sizes = (256, 128, 64, 48, 40, 32, 24, 20, 16)
    imgs = [sq.resize((s, s), Image.LANCZOS) for s in sizes]
    imgs[0].save(os.path.join(ASSETS, "lemonstudio.ico"), sizes=[(s, s) for s in sizes], append_images=imgs[1:])
    # Linux: the freedesktop icon named after the app id (Wayland compositors show it through lemonstudio.desktop).
    imgs[0].save(os.path.join(ASSETS, "lemonstudio.png"))


def main():
    write_rs()
    if "--rs" not in sys.argv[1:]:
        write_icons()


if __name__ == "__main__":
    main()
