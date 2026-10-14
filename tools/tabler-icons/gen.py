# Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
# SPDX-License-Identifier: GPL-3.0-or-later
#
# This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
# and not any other part of Lemon Studio.

"""Generate crates/os-ui/src/tabler.rs from an unpacked @tabler/icons npm package (MIT).

    python gen.py <path-to-package> <output.rs>

Each selected SVG becomes one absolute path string using only M/L/Q/C/Z (arcs, circles, rects and
relative commands are converted here, so the Rust side needs a trivial parser). Tabler icons are on
a 24x24 grid; outline icons are stroked (width 2, round caps and joins), filled icons are filled.
"""
import math
import re
import sys
import xml.etree.ElementTree as ET

# The notice every source file in the repository starts with (also used by tools/logo/gen.py).
HEADER = [
    "// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio",
    "// SPDX-License-Identifier: GPL-3.0-or-later",
    "//",
    "// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,",
    "// and not any other part of Lemon Studio.",
    "",
]

# (Rust const, tabler name, style)
ICONS = [
    ("MINUS", "minus", "outline"),
    ("SQUARES", "squares", "outline"),
    ("X", "x", "outline"),
    ("PLAYER_PLAY", "player-play", "filled"),
    ("PLAYER_STOP", "player-stop", "filled"),
    ("METRONOME", "metronome", "outline"),
    ("HOURGLASS", "hourglass", "outline"),
    ("STOPWATCH", "stopwatch", "outline"),
    ("BLEND_MODE", "blend-mode", "outline"),
    ("REPEAT", "repeat", "outline"),
    ("ARROW_BACK_UP", "arrow-back-up", "outline"),
    ("CUT", "cut", "outline"),
    ("MICROPHONE", "microphone", "outline"),
    ("HELP", "help", "outline"),
    ("DEVICE_FLOPPY", "device-floppy", "outline"),
    ("FILE_EXPORT", "file-export", "outline"),
    ("INFO_CIRCLE", "info-circle", "outline"),
    ("DOWNLOAD", "download", "outline"),
    ("KEYBOARD", "keyboard", "outline"),
    ("GRID_DOTS", "grid-dots", "outline"),
    ("TRENDING_UP", "trending-up", "outline"),
    ("LINK", "link", "outline"),
    ("ADJUSTMENTS_HORIZONTAL", "adjustments-horizontal", "outline"),
    ("MAGNET", "magnet", "outline"),
    ("PLAYLIST", "playlist", "outline"),
    ("PIANO", "piano", "outline"),
    ("LAYOUT_ROWS", "layout-rows", "outline"),
    ("ADJUSTMENTS", "adjustments", "outline"),
    ("FOLDERS", "folders", "outline"),
    ("FILE", "file", "outline"),
    ("FILE_TEXT", "file-text", "outline"),
    ("PLUG", "plug", "outline"),
    ("WAVE_SINE", "wave-sine", "outline"),
    ("HAND_FINGER", "hand-finger", "outline"),
    ("SHOPPING_BAG", "shopping-bag", "outline"),
    ("WORLD", "world", "outline"),
    ("CARET_RIGHT", "caret-right", "filled"),
    ("CARET_UP", "caret-up", "filled"),
    ("ARROW_LEFT", "arrow-left", "outline"),
    ("STAR", "star", "outline"),
    ("FOLDER", "folder", "outline"),
    ("FOLDER_PLUS", "folder-plus", "outline"),
    ("FILE_MUSIC", "file-music", "outline"),
    ("FILE_STAR", "file-star", "outline"),
    ("FILE_TYPOGRAPHY", "file-typography", "outline"),
    ("FILE_SETTINGS", "file-settings", "outline"),
    ("FILE_ZIP", "file-zip", "outline"),
    ("SEARCH", "search", "outline"),
    ("PENCIL", "pencil", "outline"),
    ("BRUSH", "brush", "outline"),
    ("ERASER", "eraser", "outline"),
    ("VOLUME_OFF", "volume-off", "outline"),
    ("ARROWS_HORIZONTAL", "arrows-horizontal", "outline"),
    ("SLICE", "slice", "outline"),
    ("MARQUEE_2", "marquee-2", "outline"),
    ("ZOOM_IN", "zoom-in", "outline"),
    ("VOLUME", "volume", "outline"),
    ("PLUS", "plus", "outline"),
    ("MUSIC", "music", "outline"),
    ("USER", "user", "outline"),
    ("CLOUD", "cloud", "outline"),
    ("BOX", "box", "outline"),
    ("TOOL", "tool", "outline"),
    ("SETTINGS", "settings", "outline"),
    ("WAVE_SQUARE", "wave-square", "outline"),
    ("WAVE_SAW_TOOL", "wave-saw-tool", "outline"),
    ("TIMELINE", "timeline", "outline"),
    ("CARET_LEFT", "caret-left", "filled"),
    ("FOLDER_OPEN", "folder-open", "outline"),
    ("FOLDER_SEARCH", "folder-search", "outline"),
    ("ROTATE_CLOCKWISE_2", "rotate-clockwise-2", "outline"),
    ("FLASK_2", "flask-2", "outline"),
    ("MAXIMIZE", "maximize", "outline"),
    ("SETTINGS_FILLED", "settings", "filled"),
    ("ARROW_AUTOFIT_RIGHT", "arrow-autofit-right", "outline"),
]

NUM = re.compile(r"[-+]?(?:\d+\.?\d*|\.\d+)(?:[eE][-+]?\d+)?")


def tokens(d):
    for m in re.finditer(r"([MmLlHhVvCcSsQqTtAaZz])|" + NUM.pattern, d):
        yield m.group(0)


def arc_to_cubics(x1, y1, rx, ry, phi, fa, fs, x2, y2):
    if (x1, y1) == (x2, y2):
        return []
    if rx == 0 or ry == 0:
        return [("L", x2, y2)]
    rx, ry = abs(rx), abs(ry)
    cp, sp = math.cos(math.radians(phi)), math.sin(math.radians(phi))
    dx, dy = (x1 - x2) / 2, (y1 - y2) / 2
    x1p, y1p = cp * dx + sp * dy, -sp * dx + cp * dy
    lam = x1p ** 2 / rx ** 2 + y1p ** 2 / ry ** 2
    if lam > 1:
        rx, ry = rx * math.sqrt(lam), ry * math.sqrt(lam)
    num = rx ** 2 * ry ** 2 - rx ** 2 * y1p ** 2 - ry ** 2 * x1p ** 2
    den = rx ** 2 * y1p ** 2 + ry ** 2 * x1p ** 2
    k = math.sqrt(max(0.0, num / den)) * (-1 if fa == fs else 1)
    cxp, cyp = k * rx * y1p / ry, -k * ry * x1p / rx
    cx = cp * cxp - sp * cyp + (x1 + x2) / 2
    cy = sp * cxp + cp * cyp + (y1 + y2) / 2

    def ang(ux, uy, vx, vy):
        a = math.atan2(ux * vy - uy * vx, ux * vx + uy * vy)
        return a

    t1 = ang(1, 0, (x1p - cxp) / rx, (y1p - cyp) / ry)
    dt = ang((x1p - cxp) / rx, (y1p - cyp) / ry, (-x1p - cxp) / rx, (-y1p - cyp) / ry)
    if not fs and dt > 0:
        dt -= 2 * math.pi
    elif fs and dt < 0:
        dt += 2 * math.pi
    n = max(1, math.ceil(abs(dt) / (math.pi / 2) - 1e-9))
    seg = dt / n
    out = []
    for i in range(n):
        a1, a2 = t1 + i * seg, t1 + (i + 1) * seg
        al = 4 / 3 * math.tan((a2 - a1) / 4)

        def pt(a):
            x, y = rx * math.cos(a), ry * math.sin(a)
            return cx + cp * x - sp * y, cy + sp * x + cp * y

        def dv(a):
            x, y = -rx * math.sin(a), ry * math.cos(a)
            return cp * x - sp * y, sp * x + cp * y

        p1, p2 = pt(a1), pt(a2)
        d1, d2 = dv(a1), dv(a2)
        out.append(("C", p1[0] + al * d1[0], p1[1] + al * d1[1], p2[0] - al * d2[0], p2[1] - al * d2[1], p2[0], p2[1]))
    return out


def convert(d):
    t = list(tokens(d))
    i = 0
    cmd = None
    cx = cy = sx = sy = 0.0
    lc = None  # last control point for S/T
    out = []

    def num():
        nonlocal i
        v = float(t[i])
        i += 1
        return v

    while i < len(t):
        if t[i].isalpha():
            cmd = t[i]
            i += 1
            if cmd in "Zz":
                out.append(("Z",))
                cx, cy = sx, sy
                lc = None
                continue
        rel = cmd.islower()
        c = cmd.upper()
        ox, oy = (cx, cy) if rel else (0.0, 0.0)
        if c == "M":
            cx, cy = ox + num(), oy + num()
            sx, sy = cx, cy
            out.append(("M", cx, cy))
            cmd = "l" if rel else "L"
            lc = None
        elif c == "L":
            cx, cy = ox + num(), oy + num()
            out.append(("L", cx, cy))
            lc = None
        elif c == "H":
            cx = (cx if rel else 0.0) + num()
            out.append(("L", cx, cy))
            lc = None
        elif c == "V":
            cy = (cy if rel else 0.0) + num()
            out.append(("L", cx, cy))
            lc = None
        elif c == "C":
            x1, y1, x2, y2, x, y = (ox + num(), oy + num(), ox + num(), oy + num(), ox + num(), oy + num())
            out.append(("C", x1, y1, x2, y2, x, y))
            lc = ("C", x2, y2)
            cx, cy = x, y
        elif c == "S":
            x1, y1 = (2 * cx - lc[1], 2 * cy - lc[2]) if lc and lc[0] == "C" else (cx, cy)
            x2, y2, x, y = ox + num(), oy + num(), ox + num(), oy + num()
            out.append(("C", x1, y1, x2, y2, x, y))
            lc = ("C", x2, y2)
            cx, cy = x, y
        elif c == "Q":
            x1, y1, x, y = ox + num(), oy + num(), ox + num(), oy + num()
            out.append(("Q", x1, y1, x, y))
            lc = ("Q", x1, y1)
            cx, cy = x, y
        elif c == "T":
            x1, y1 = (2 * cx - lc[1], 2 * cy - lc[2]) if lc and lc[0] == "Q" else (cx, cy)
            x, y = ox + num(), oy + num()
            out.append(("Q", x1, y1, x, y))
            lc = ("Q", x1, y1)
            cx, cy = x, y
        elif c == "A":
            rx, ry, phi, fa, fs = num(), num(), num(), num(), num()
            x, y = ox + num(), oy + num()
            out.extend(arc_to_cubics(cx, cy, rx, ry, phi, int(fa), int(fs), x, y))
            cx, cy = x, y
            lc = None
        else:
            raise ValueError(cmd)
    return out


def fmt(v):
    s = ("%.3f" % v).rstrip("0").rstrip(".")
    return "0" if s in ("-0", "") else s


def element_d(el):
    tag = el.tag.split("}")[-1]
    a = el.attrib
    if a.get("stroke") == "none" and a.get("fill") == "none":
        return None
    if tag == "path":
        return a["d"]
    if tag == "circle":
        cx, cy, r = float(a["cx"]), float(a["cy"]), float(a["r"])
        return "M%s %sa%s %s 0 1 0 %s 0a%s %s 0 1 0 %s 0" % (cx - r, cy, r, r, 2 * r, r, r, -2 * r)
    if tag == "rect":
        x, y, w, h = (float(a.get(k, 0)) for k in ("x", "y", "width", "height"))
        return "M%s %sh%sv%sh%sz" % (x, y, w, h, -w)
    if tag in ("svg", "g"):
        return None
    raise ValueError(tag)


def main():
    pkg, out_path = sys.argv[1], sys.argv[2]
    lines = HEADER + [
        "//! Icon outlines from Tabler Icons 3.31.0 (MIT, https://tabler.io/icons; licence in",
        "//! `crates/os-ui/icons/TABLER-LICENSE.txt`). Generated by `tools/tabler-icons/gen.py`: do not edit.",
        "//! 24x24 grid, absolute M/L/Q/C/Z commands only.",
        "",
        "pub struct TablerIcon {",
        "    pub filled: bool,",
        "    pub d: &'static str,",
        "}",
        "",
    ]
    for const, name, style in ICONS:
        root = ET.parse("%s/icons/%s/%s.svg" % (pkg, style, name)).getroot()
        cmds = []
        for el in root.iter():
            d = element_d(el)
            if d:
                cmds.extend(convert(d))
        d = " ".join(c[0] + "".join(" " + fmt(v) for v in c[1:]) for c in cmds)
        lines.append("/// `%s` (%s)" % (name, style))
        lines.append('pub const %s: TablerIcon = TablerIcon { filled: %s, d: "%s" };' % (const, "true" if style == "filled" else "false", d))
    with open(out_path, "w", newline="\n") as f:
        f.write("\n".join(lines) + "\n")


if __name__ == "__main__":
    main()
