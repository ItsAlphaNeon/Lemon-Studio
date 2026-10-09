"""Generate the OpenStudio logo data from crates/os-app/assets/logo/*.svg.

    python tools/logo/gen.py            (run from the OpenStudio folder)

Writes:
  crates/os-app/src/logo.rs        greyscale mark as absolute M/L/Q/C/Z path data (drawn in one tint)
  crates/os-app/assets/openstudio.ico  coloured mark, 16..256 px (exe and window icon)
"""
import importlib.util
import os
import re
import xml.etree.ElementTree as ET

from PIL import Image, ImageChops, ImageDraw

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
ASSETS = os.path.join(ROOT, "crates", "os-app", "assets")
spec = importlib.util.spec_from_file_location("tabler_gen", os.path.join(ROOT, "tools", "tabler-icons", "gen.py"))
tg = importlib.util.module_from_spec(spec)
spec.loader.exec_module(tg)


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


def d_string(cmds):
    return " ".join(c[0] + "".join(" " + tg.fmt(v) for v in c[1:]) for c in cmds)


def flatten(cmds, k, steps=24):
    """Subpaths as point lists, scaled by k."""
    subs, cur, p = [], [], (0.0, 0.0)
    for c in cmds:
        if c[0] == "M":
            if len(cur) > 2:
                subs.append(cur)
            p = (c[1], c[2])
            cur = [p]
        elif c[0] == "L":
            p = (c[1], c[2])
            cur.append(p)
        elif c[0] in "QC":
            pts = [p] + [(c[i], c[i + 1]) for i in range(1, len(c), 2)]
            for s in range(1, steps + 1):
                t = s / steps
                q = pts
                while len(q) > 1:
                    q = [(a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t) for a, b in zip(q, q[1:])]
                cur.append(q[0])
            p = pts[-1]
        elif c[0] == "Z":
            if len(cur) > 2:
                subs.append(cur)
            cur = [p]
    if len(cur) > 2:
        subs.append(cur)
    return [[(x * k, y * k) for x, y in s] for s in subs]


def render(vb, lys, size):
    """Coloured render, supersampled 4x, fitted (aspect kept) into a size x size square."""
    ss = size * 4
    k = ss / max(vb[2], vb[3])
    ox, oy = (ss - vb[2] * k) / 2, (ss - vb[3] * k) / 2
    img = Image.new("RGBA", (ss, ss), (0, 0, 0, 0))
    for color, cmds in lys:
        mask = Image.new("L", (ss, ss), 0)
        for sub in flatten(cmds, k):
            m = Image.new("L", (ss, ss), 0)
            ImageDraw.Draw(m).polygon([(x + ox, y + oy) for x, y in sub], fill=255)
            mask = ImageChops.logical_xor(mask.convert("1"), m.convert("1")).convert("L")
        rgb = tuple(int(color[i:i + 2], 16) for i in (1, 3, 5))
        img.paste(Image.new("RGBA", (ss, ss), rgb + (255,)), (0, 0), mask)
    return img.resize((size, size), Image.LANCZOS)


def main():
    vb, grey = layers(os.path.join(ASSETS, "logo", "logo-greyscale.svg"))
    lines = [
        "//! OpenStudio logo (greyscale mark), generated from `assets/logo/logo-greyscale.svg` by",
        "//! `tools/logo/gen.py`: do not edit. Absolute M/L/Q/C/Z path data in viewBox units.",
        "",
        "pub const VIEW_W: f32 = %s;" % tg.fmt(vb[2]),
        "pub const VIEW_H: f32 = %s;" % tg.fmt(vb[3]),
        "",
        "pub const PATHS: &[&str] = &[",
    ]
    lines += ['    "%s",' % d_string(cmds) for _, cmds in grey]
    lines.append("];")
    with open(os.path.join(ROOT, "crates", "os-app", "src", "logo.rs"), "w", newline="\n") as f:
        f.write("\n".join(lines) + "\n")

    vb, colored = layers(os.path.join(ASSETS, "logo", "logo-colored.svg"))
    big = render(vb, colored, 256)
    big.save(os.path.join(ASSETS, "openstudio.ico"), sizes=[(s, s) for s in (16, 20, 24, 32, 40, 48, 64, 128, 256)])
    big.save(os.path.join(ASSETS, "logo", "logo-colored-256.png"))


if __name__ == "__main__":
    main()
