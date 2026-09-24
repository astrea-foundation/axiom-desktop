#!/usr/bin/env python3
"""Derive every packaged brand asset from the design masters.

Sources (designer-owned, outside the repo): ../../../brand-desktop/
Outputs: apps/desktop/build/ (electron-builder buildResources) and resources/.

Requires python3 + Pillow, and macOS `iconutil` / `tiffutil` for the .icns and
the Retina DMG background. Run from apps/desktop:  python3 scripts/brand-assets.py
"""
from __future__ import annotations

import math
import os
import shutil
import subprocess
import sys
from pathlib import Path

from PIL import Image, ImageDraw, ImageFilter

HERE = Path(__file__).resolve().parent
APP = HERE.parent
SRC = Path(os.environ.get("AXIOM_BRAND_SRC", APP.parent.parent.parent / "brand-desktop"))
BUILD = APP / "build"
RESOURCES = APP / "resources"
EXPORT = SRC / "export"

# Apple's icon corner: continuous-curvature ("squircle") rounded rectangle.
# Corner radius is 22.37% of the side; smoothing 0.6 reproduces the iOS/macOS
# profile (same construction Figma uses for corner smoothing).
CORNER_RATIO = 0.2237
SMOOTHING = 0.6
SUPERSAMPLE = 4


def _corner_points(r: float, s: float, steps: int = 64) -> list[tuple[float, float]]:
    """Top-right smooth corner, local coords: starts on the top edge at
    (-p, 0) moving right, ends on the right edge at (0, p). Corner point is (0, 0)."""
    p = (1 + s) * r
    arc_sweep = 90 * (1 - s)
    arc_len = math.sin(math.radians(arc_sweep / 2)) * r * math.sqrt(2)
    alpha = (90 - arc_sweep) / 2
    p3p4 = r * math.tan(math.radians(alpha / 2))
    beta = 45 * s
    c = p3p4 * math.cos(math.radians(beta))
    d = c * math.tan(math.radians(beta))
    b = (p - arc_len - c - d) / 3
    a = 2 * b

    def bez(p0, p1, p2, p3):
        out = []
        for i in range(steps + 1):
            t = i / steps
            u = 1 - t
            out.append((
                u ** 3 * p0[0] + 3 * u * u * t * p1[0] + 3 * u * t * t * p2[0] + t ** 3 * p3[0],
                u ** 3 * p0[1] + 3 * u * u * t * p1[1] + 3 * u * t * t * p2[1] + t ** 3 * p3[1],
            ))
        return out

    # Segment 1: cubic from (-p,0) with handles (a,0),(a+b,0) to (a+b+c,d), relative.
    x0, y0 = -p, 0.0
    seg1 = bez((x0, y0), (x0 + a, y0), (x0 + a + b, y0), (x0 + a + b + c, y0 + d))
    x1, y1 = seg1[-1]
    # Segment 2: circular arc of radius r sweeping arc_sweep degrees, ending at (x1+arc_len, y1+arc_len).
    x2, y2 = x1 + arc_len, y1 + arc_len
    # Arc centre: the point at distance r from both endpoints, on the inside (down-left).
    mx, my = (x1 + x2) / 2, (y1 + y2) / 2
    half = math.dist((x1, y1), (x2, y2)) / 2
    h = math.sqrt(max(r * r - half * half, 0))
    # Perpendicular to the chord pointing down-left (into the shape).
    dx, dy = (x2 - x1) / (2 * half), (y2 - y1) / (2 * half)
    cx, cy = mx - dy * h, my + dx * h
    a0 = math.atan2(y1 - cy, x1 - cx)
    a1 = math.atan2(y2 - cy, x2 - cx)
    while a1 < a0:
        a1 += 2 * math.pi
    seg2 = [(cx + r * math.cos(a0 + (a1 - a0) * i / steps), cy + r * math.sin(a0 + (a1 - a0) * i / steps)) for i in range(steps + 1)]
    # Segment 3: mirror of segment 1, ending on the right edge at (0, p).
    seg3 = bez((x2, y2), (x2 + d, y2 + c), (x2 + d, y2 + b + c), (x2 + d, y2 + a + b + c))
    pts = seg1 + seg2[1:] + seg3[1:]
    # Numerical drift guard: snap the end onto the right edge.
    pts[-1] = (0.0, p)
    return pts


def squircle_mask(size: int, radius: float, smoothing: float = SMOOTHING) -> Image.Image:
    """Alpha mask of a smooth-cornered square, antialiased via supersampling."""
    S = size * SUPERSAMPLE
    r = radius * SUPERSAMPLE
    corner = _corner_points(r, smoothing)
    # Top-right corner in image coords: corner point at (S, 0); local x→right, y→down.
    tr = [(S + x, 0 + y) for x, y in corner]
    # Rotate the corner around the centre for the other three (clockwise order).
    def rot(pts, k):
        out = []
        for x, y in pts:
            x, y = x - S / 2, y - S / 2
            for _ in range(k):
                x, y = -y, x
            out.append((x + S / 2, y + S / 2))
        return out
    poly = tr + rot(tr, 1) + rot(tr, 2) + rot(tr, 3)
    mask = Image.new("L", (S, S), 0)
    ImageDraw.Draw(mask).polygon(poly, fill=255)
    return mask.resize((size, size), Image.LANCZOS)


def masked(img: Image.Image, mask: Image.Image) -> Image.Image:
    out = img.convert("RGBA").copy()
    alpha = out.getchannel("A")
    out.putalpha(Image.fromarray(__import__("numpy").minimum(__import__("numpy").array(alpha), __import__("numpy").array(mask)))) if False else out.putalpha(mask)
    return out


def legacy_mac_icon(master: Image.Image) -> Image.Image:
    """macOS ≤15 shape: 824px smooth square centred on a 1024 canvas, soft shadow."""
    canvas = 1024
    inner = 824
    art = master.convert("RGBA").resize((inner, inner), Image.LANCZOS)
    art = masked(art, squircle_mask(inner, inner * 0.225))
    out = Image.new("RGBA", (canvas, canvas), (0, 0, 0, 0))
    # Shadow: Apple's template ≈ 30% black, y offset 10, blur ~12.
    shadow = Image.new("RGBA", (canvas, canvas), (0, 0, 0, 0))
    shadow_mask = Image.new("L", (canvas, canvas), 0)
    shadow_mask.paste(art.getchannel("A"), (100, 110))
    shadow_mask = shadow_mask.filter(ImageFilter.GaussianBlur(12))
    shadow.putalpha(shadow_mask.point(lambda v: int(v * 0.30)))
    out.alpha_composite(shadow)
    out.alpha_composite(art, (100, 100))
    return out


def write_iconset(icon1024: Image.Image, path: Path) -> None:
    if path.exists():
        shutil.rmtree(path)
    path.mkdir(parents=True)
    for size in (16, 32, 128, 256, 512):
        icon1024.resize((size, size), Image.LANCZOS).save(path / f"icon_{size}x{size}.png")
        icon1024.resize((size * 2, size * 2), Image.LANCZOS).save(path / f"icon_{size}x{size}@2x.png")


def main() -> int:
    if not SRC.exists():
        print(f"brand sources not found at {SRC} (set AXIOM_BRAND_SRC)", file=sys.stderr)
        return 1
    BUILD.mkdir(exist_ok=True)
    EXPORT.mkdir(exist_ok=True)
    master = Image.open(SRC / "icon-master.png").convert("RGBA")
    assert master.size == (1024, 1024), master.size

    # --- macOS (legacy shape baked in) ---------------------------------------
    mac = legacy_mac_icon(master)
    mac.save(RESOURCES / "icon-mac.png")
    iconset = BUILD / "icon.iconset"
    write_iconset(mac, iconset)
    subprocess.run(["iconutil", "-c", "icns", str(iconset), "-o", str(BUILD / "icon.icns")], check=True)
    shutil.rmtree(iconset)

    # --- Windows (full bleed) ----------------------------------------------
    ico_sizes = [16, 24, 32, 48, 64, 128, 256]
    frames = [master.resize((s, s), Image.LANCZOS) for s in ico_sizes]
    frames[-1].save(BUILD / "icon.ico", format="ICO", sizes=[(s, s) for s in ico_sizes], append_images=frames[:-1])

    # --- Linux hicolor set + scalable SVG ------------------------------------
    icons = BUILD / "icons"
    icons.mkdir(exist_ok=True)
    for s in (16, 22, 24, 32, 48, 64, 128, 256, 512, 1024):
        master.resize((s, s), Image.LANCZOS).save(icons / f"{s}x{s}.png")
    bg = (SRC / "icon-background.svg").read_text()
    fg = (SRC / "icon-foreground.svg").read_text()
    def inner(svg: str) -> str:
        return svg[svg.index(">") + 1 : svg.rindex("</svg>")]
    combined = (
        '<svg width="1024" height="1024" viewBox="0 0 1024 1024" fill="none" xmlns="http://www.w3.org/2000/svg">'
        + inner(bg) + inner(fg) + "</svg>\n"
    )
    (BUILD / "icon.svg").write_text(combined)
    (RESOURCES / "icon.svg").write_text(combined)
    master.resize((512, 512), Image.LANCZOS).save(RESOURCES / "icon.png")

    # --- macOS 26 Icon Composer layers ----------------------------------------
    tahoe = BUILD / "tahoe-icon-layers"
    tahoe.mkdir(exist_ok=True)
    shutil.copy(SRC / "icon-background.svg", tahoe / "background.svg")
    shutil.copy(SRC / "icon-foreground.svg", tahoe / "foreground.svg")

    # --- DMG background: 1x + 2x + multi-resolution TIFF ------------------------
    dmg = Image.open(SRC / "dmg-background.png").convert("RGB")
    bg2x = dmg.resize((1320, 800), Image.LANCZOS)
    bg1x = dmg.resize((660, 400), Image.LANCZOS)
    bg2x.save(BUILD / "background@2x.png")
    bg1x.save(BUILD / "background.png")
    subprocess.run(["tiffutil", "-cathidpicheck", str(BUILD / "background.png"), str(BUILD / "background@2x.png"), "-out", str(BUILD / "background.tiff")], check=True)

    # --- Windows NSIS bitmaps (24-bit BMP, no alpha) ----------------------------
    side = Image.open(SRC / "installer-sidebar.png").convert("RGB").resize((164, 314), Image.LANCZOS)
    side.save(BUILD / "installerSidebar.bmp", format="BMP")
    head = Image.open(SRC / "installer-header.png").convert("RGB")
    scale = 57 / head.height
    head = head.resize((round(head.width * scale), 57), Image.LANCZOS)
    left = (head.width - 150) // 2
    head.crop((left, 0, left + 150, 57)).save(BUILD / "installerHeader.bmp", format="BMP")

    # --- Designer preview: what each platform actually shows ---------------------
    tile = 512
    pad = 48
    sheet = Image.new("RGBA", (tile * 3 + pad * 4, tile + pad * 2 + 40), (232, 232, 236, 255))
    checker = Image.new("RGBA", (tile, tile), (255, 255, 255, 255))
    cd = ImageDraw.Draw(checker)
    for y in range(0, tile, 32):
        for x in range(0, tile, 32):
            if (x // 32 + y // 32) % 2:
                cd.rectangle((x, y, x + 31, y + 31), fill=(238, 238, 242, 255))
    # 1) macOS 26: system squircle applied to the full-bleed square; clipped corners shown dimmed.
    m26 = squircle_mask(1024, 1024 * CORNER_RATIO)
    kept = masked(master, m26)
    dimmed = master.copy()
    dimmed.putalpha(m26.point(lambda v: 90 if v < 128 else 0))
    p1 = checker.copy()
    p1.alpha_composite(dimmed.resize((tile, tile), Image.LANCZOS))
    p1.alpha_composite(kept.resize((tile, tile), Image.LANCZOS))
    # 2) macOS ≤15: the baked legacy icon.
    p2 = checker.copy()
    p2.alpha_composite(mac.resize((tile, tile), Image.LANCZOS))
    # 3) Windows / Linux: full bleed.
    p3 = checker.copy()
    p3.alpha_composite(master.resize((tile, tile), Image.LANCZOS))
    labels = ("macOS 26+ (system mask; dimmed = clipped)", "macOS 15 and earlier (baked)", "Windows / Linux (as designed)")
    sd = ImageDraw.Draw(sheet)
    for i, (panel, label) in enumerate(zip((p1, p2, p3), labels)):
        x = pad + i * (tile + pad)
        sheet.alpha_composite(panel, (x, pad))
        sd.text((x, pad + tile + 12), label, fill=(60, 60, 68, 255))
    sheet.convert("RGB").save(EXPORT / "icon-preview.png")

    # Report the tightest point of the artwork against the macOS 26 cut.
    alpha_art = fg_alpha_bbox(master)
    print(f"artwork bbox in master: {alpha_art}")
    print("wrote:", ", ".join(sorted(p.name for p in BUILD.iterdir())))
    return 0


def fg_alpha_bbox(master: Image.Image):
    """Bounding box of pixels that differ from the background colour."""
    bg = master.getpixel((4, 4))
    diff = Image.new("L", master.size, 0)
    px = master.load()
    d = diff.load()
    for y in range(0, master.height, 2):
        for x in range(0, master.width, 2):
            p = px[x, y]
            if abs(p[0] - bg[0]) + abs(p[1] - bg[1]) + abs(p[2] - bg[2]) > 40:
                d[x, y] = 255
    return diff.getbbox()


if __name__ == "__main__":
    sys.exit(main())
