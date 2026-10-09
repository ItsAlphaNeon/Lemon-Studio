"""Generate the OpenStudio logo data from crates/os-app/assets/logo/*.svg.

    python tools/logo/gen.py            (run from the OpenStudio folder)

Writes:
  crates/os-app/src/logo.rs        greyscale mark as absolute M/L/Q/C/Z path data (drawn in one tint)
  crates/os-app/assets/openstudio.ico  app icon, 16..256 px (exe, window, taskbar), from the owner's
                                       bitmap assets/logo/icon.png (edited in icon.pdn, Paint.NET)
"""
import importlib.util
import os
import re
import xml.etree.ElementTree as ET

from PIL import Image

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

    src = Image.open(os.path.join(ASSETS, "logo", "icon.png")).convert("RGBA")
    src = src.crop(src.getchannel("A").getbbox())  # no transparent margin: fill the whole icon cell
    side = max(src.size)
    sq = Image.new("RGBA", (side, side), (0, 0, 0, 0))
    sq.paste(src, ((side - src.width) // 2, (side - src.height) // 2))
    sizes = (256, 128, 64, 48, 40, 32, 24, 20, 16)
    imgs = [sq.resize((s, s), Image.LANCZOS) for s in sizes]
    imgs[0].save(os.path.join(ASSETS, "openstudio.ico"), sizes=[(s, s) for s in sizes], append_images=imgs[1:])


if __name__ == "__main__":
    main()
