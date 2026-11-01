"""Build Kuality's icon assets (logo proposal A, "Geometric K") from vector sources.

Usage:  python assets/icon/src/make_icons.py
Needs:  resvg on PATH (`cargo install resvg`) and Pillow (`pip install pillow`).

Writes, into assets/icon/:
  kuality.svg, tray-white.svg, tray-black.svg   master vectors
  kuality.ico      app icon: 16 20 24 32 40 48 64 128 256 (16-32 hand-tuned on the pixel grid)
  tray-white.ico   tray icon for dark taskbars: 16 20 24 32 40 48 (each size hand-tuned)
  tray-black.ico   same, for light taskbars
  png/kuality-N.png   app tile PNGs, N = 16 20 24 32 40 48 64 128 256 512
  wizard-small-55.bmp, wizard-small-110.bmp   Inno Setup WizardSmallImageFile
  wizard-164x314.bmp,  wizard-328x628.bmp     Inno Setup WizardImageFile
and the per-size sources into assets/icon/src/.
"""

import io
import shutil
import struct
import subprocess
import tempfile
from pathlib import Path

from PIL import Image

SRC = Path(__file__).resolve().parent
OUT = SRC.parent
PNG_DIR = OUT / "png"

RESVG = shutil.which("resvg")
if not RESVG:
    raise SystemExit("resvg not found on PATH (cargo install resvg)")

# --------------------------------------------------------------------------- geometry


def k_path(x, y, h, s, r, w, a):
    """Geometric K: stem plus a chevron whose arm and leg meet the stem.

    x, y   top-left of the stem      h  glyph height     s  stem width
    r      right edge of arm/leg     w  horizontal width of the arm/leg tips
    a      where the arm's upper edge meets the stem (distance below y)
    The inner vertex is placed so the arm's two edges stay parallel.
    """
    slope = a / (r - w - x - s)
    vx = r - (h / 2) / slope
    pts = [(x, y), (x + s, y), (x + s, y + a), (r - w, y), (r, y), (vx, y + h / 2), (r, y + h),
           (r - w, y + h), (x + s, y + h - a), (x + s, y + h), (x, y + h)]
    return "M" + " L".join(f"{px:g} {py:g}" for px, py in pts) + " Z"


GRADIENTS = """
  <linearGradient id="tile" x1="0" y1="0" x2="0" y2="1">
    <stop offset="0" stop-color="#34343a"/><stop offset="1" stop-color="#101012"/>
  </linearGradient>
  <linearGradient id="rim" x1="0" y1="0" x2="0" y2="1">
    <stop offset="0" stop-color="#fff" stop-opacity=".22"/>
    <stop offset=".35" stop-color="#fff" stop-opacity=".05"/>
    <stop offset="1" stop-color="#fff" stop-opacity="0"/>
  </linearGradient>"""

# Master app tile, 256 x 256: tile with soft shadow and top rim light, white K.
MASTER = f"""<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 256 256" width="256" height="256">
<defs>{GRADIENTS}
  <filter id="shadow" x="-20%" y="-20%" width="140%" height="150%"><feGaussianBlur stdDeviation="7"/></filter>
</defs>
<rect x="24" y="26" width="208" height="210" rx="50" fill="#000" opacity=".35" filter="url(#shadow)"/>
<rect x="20" y="16" width="216" height="216" rx="52" fill="url(#tile)"/>
<rect x="21" y="17" width="214" height="214" rx="51" fill="none" stroke="url(#rim)" stroke-width="2"/>
<path fill="#fff" d="{k_path(80, 70, 108, 32, 176, 40, 30)}"/>
</svg>
"""

# Hand-tuned small tiles: same padding as the master (tile ~84% of the canvas, a bit high,
# 1 px drop shadow below), no blur, tile edges, stem and tips on whole pixels.
#        size: ((tile x, tile y, tile size), (K: x, y, h, s, r, w, a))
SMALL = {
    16: ((1, 1, 14), (5, 4, 8, 2, 12, 2.6, 3)),
    20: ((1, 1, 18), (6, 5, 10, 3, 15, 3.5, 3.1)),
    24: ((2, 1, 20), (7, 5, 12, 3, 18, 4, 5)),
    32: ((3, 2, 26), (10, 8, 14, 4, 22, 5, 3.8)),
}


def small_tile(n):
    (tx, ty, tw), k = SMALL[n]
    r = round(tw * 0.24, 1)
    return f"""<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {n} {n}" width="{n}" height="{n}">
<defs>{GRADIENTS}</defs>
<rect x="{tx}" y="{ty + 1}" width="{tw}" height="{tw}" rx="{r}" fill="#000" opacity=".3"/>
<rect x="{tx}" y="{ty}" width="{tw}" height="{tw}" rx="{r}" fill="url(#tile)"/>
<rect x="{tx + .5}" y="{ty + .5}" width="{tw - 1}" height="{tw - 1}" rx="{r - .5}" fill="none" stroke="url(#rim)"/>
<path fill="#fff" d="{k_path(*k)}"/>
</svg>
"""


# Tray glyph per size (just the K, monochrome), on whole pixels.
TRAY_K = {
    16: (3, 2, 12, 3, 13, 3.5, 4.5),
    20: (4, 3, 14, 4, 16, 4, 5),
    24: (5, 3, 18, 5, 20, 5, 6.3),
    32: (6, 4, 24, 7, 27, 7, 8.9),
    40: (8, 5, 30, 9, 34, 9, 10.2),
    48: (8, 6, 36, 10, 40, 11, 14),
}


def tray(n, color):
    return (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {n} {n}" width="{n}" height="{n}">\n'
            f'<path fill="{color}" d="{k_path(*TRAY_K[n])}"/>\n</svg>\n')


# Inno Setup large wizard image, designed at 164 x 314 (rendered 1x and 2x).
WIZARD = f"""<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 164 314" width="164" height="314">
<defs>{GRADIENTS}
  <linearGradient id="bg" x1="0" y1="0" x2="0" y2="1">
    <stop offset="0" stop-color="#1d1d21"/><stop offset="1" stop-color="#09090a"/>
  </linearGradient>
  <filter id="shadow" x="-30%" y="-30%" width="160%" height="170%"><feGaussianBlur stdDeviation="3"/></filter>
</defs>
<rect width="164" height="314" fill="url(#bg)"/>
<g transform="translate(32 86) scale(.390625)">
  <rect x="24" y="26" width="208" height="210" rx="50" fill="#000" opacity=".6" filter="url(#shadow)"/>
  <rect x="20" y="16" width="216" height="216" rx="52" fill="url(#tile)"/>
  <rect x="21" y="17" width="214" height="214" rx="51" fill="none" stroke="url(#rim)" stroke-width="2"/>
  <path fill="#fff" d="{k_path(80, 70, 108, 32, 176, 40, 30)}"/>
</g>
<text x="82" y="214" fill="#fff" font-family="Segoe UI Variable Display, Segoe UI" font-weight="600"
  font-size="22" text-anchor="middle">Kuality</text>
</svg>
"""

# --------------------------------------------------------------------------- rendering


def render(svg_text, w, h=None):
    h = h or w
    with tempfile.TemporaryDirectory() as tmp:
        src, dst = Path(tmp) / "in.svg", Path(tmp) / "out.png"
        src.write_text(svg_text, encoding="utf-8")
        subprocess.run([RESVG, "-w", str(w), "-h", str(h), str(src), str(dst)], check=True)
        return Image.open(dst).convert("RGBA").copy()


def ico_dib(img):
    """32-bit BGRA DIB entry with an all-zero AND mask (alpha does the masking)."""
    w, h = img.size
    header = struct.pack("<IiiHHIIiiII", 40, w, h * 2, 1, 32, 0, 0, 0, 0, 0, 0)
    px = img.tobytes("raw", "BGRA")
    rows = [px[y * w * 4:(y + 1) * w * 4] for y in range(h)][::-1]  # bottom-up
    mask_row = ((w + 31) // 32) * 4
    return header + b"".join(rows) + b"\0" * (mask_row * h)


def write_ico(path, images):
    """ICO with BMP entries up to 64 px and PNG entries above (Windows Vista+)."""
    entries = []
    for img in sorted(images, key=lambda i: i.size[0]):
        if img.size[0] > 64:
            buf = io.BytesIO()
            img.save(buf, "PNG", optimize=True)
            data = buf.getvalue()
        else:
            data = ico_dib(img)
        entries.append((img.size[0], data))
    offset = 6 + 16 * len(entries)
    out = struct.pack("<HHH", 0, 1, len(entries))
    blobs = b""
    for size, data in entries:
        dim = 0 if size >= 256 else size
        out += struct.pack("<BBBBHHII", dim, dim, 0, 0, 1, 32, len(data), offset + len(blobs))
        blobs += data
    path.write_bytes(out + blobs)


def flatten(img, bg):
    base = Image.new("RGB", img.size, bg)
    base.paste(img, mask=img.split()[3])
    return base


# --------------------------------------------------------------------------- build


def main():
    PNG_DIR.mkdir(exist_ok=True)

    (OUT / "kuality.svg").write_text(MASTER, encoding="utf-8")
    (OUT / "tray-white.svg").write_text(tray(16, "#ffffff"), encoding="utf-8")
    (OUT / "tray-black.svg").write_text(tray(16, "#000000"), encoding="utf-8")
    for n in SMALL:
        (SRC / f"tile-{n}.svg").write_text(small_tile(n), encoding="utf-8")
    for n in TRAY_K:
        (SRC / f"tray-{n}.svg").write_text(tray(n, "#ffffff"), encoding="utf-8")
    (SRC / "wizard.svg").write_text(WIZARD, encoding="utf-8")

    def tile_png(n):
        return render(small_tile(n), n) if n in SMALL else render(MASTER, n)

    tiles = {n: tile_png(n) for n in (16, 20, 24, 32, 40, 48, 64, 128, 256, 512)}
    for n, img in tiles.items():
        img.save(PNG_DIR / f"kuality-{n}.png", optimize=True)
    write_ico(OUT / "kuality.ico", [tiles[n] for n in (16, 20, 24, 32, 40, 48, 64, 128, 256)])

    for name, color in (("tray-white", "#ffffff"), ("tray-black", "#000000")):
        write_ico(OUT / f"{name}.ico", [render(tray(n, color), n) for n in TRAY_K])

    for n in (55, 110):
        flatten(render(MASTER, n), "#ffffff").save(OUT / f"wizard-small-{n}.bmp")
    for w, h in ((164, 314), (328, 628)):
        flatten(render(WIZARD, w, h), "#000000").save(OUT / f"wizard-{w}x{h}.bmp")


if __name__ == "__main__":
    main()
