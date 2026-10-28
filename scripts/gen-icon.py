#!/usr/bin/env python3
"""Generate the Grimalkin app icon (deterministic, pure Python 3 + zlib).

Draws a 32x32 pixel-art cat head in the built-in 'grimalkin' skin's palette
(purple fur, amber eyes, dark outline) and writes, under assets/icon/:

  grimalkin-<size>.png   16 32 48 64 128 256 512 1024 (RGBA, transparent bg)
  grimalkin.ico          16 32 48 64 128 256, PNG-compressed entries (Vista+)
  grimalkin.icns         via macOS `iconutil` (skipped, with a note, elsewhere)

Usage: python3 scripts/gen-icon.py [--no-icns]
Re-running produces byte-identical PNG/ICO files; the outputs are committed.
"""
import os
import shutil
import struct
import subprocess
import sys
import tempfile
import zlib

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT = os.path.join(ROOT, "assets", "icon")
SIZES = [16, 32, 48, 64, 128, 256, 512, 1024]
ICO_SIZES = [16, 32, 48, 64, 128, 256]
BASE = 32


def hx(s):
    s = s.lstrip("#")
    return (int(s[0:2], 16), int(s[2:4], 16), int(s[4:6], 16), 255)


# Palette: the FUR ramp and amber from scripts/gen-skins.py (grimalkin skin).
C = {
    "O": hx("#07040d"),  # outline
    "d": hx("#2f2540"),  # fur, shadow
    "m": hx("#53437a"),  # fur, mid
    "l": hx("#6c58a0"),  # fur, light
    "h": hx("#8f78c8"),  # fur, highlight
    "i": hx("#b0589a"),  # inner ear
    "I": hx("#7a3a78"),  # inner ear, shadow
    "A": hx("#ffb000"),  # amber eye
    "a": hx("#c47400"),  # amber eye, lower half
    "w": hx("#fff2c4"),  # eye glint
    "p": hx("#120e18"),  # pupil
    "n": hx("#f08cb0"),  # nose
    "W": hx("#d3c0f7"),  # whiskers
}
CLEAR = (0, 0, 0, 0)


def inside_ellipse(x, y, cx, cy, rx, ry):
    return ((x + 0.5 - cx) / rx) ** 2 + ((y + 0.5 - cy) / ry) ** 2 <= 1.0


def inside_tri(x, y, a, b, c):
    px, py = x + 0.5, y + 0.5

    def s(p, q, r):
        return (p[0] - r[0]) * (q[1] - r[1]) - (q[0] - r[0]) * (p[1] - r[1])

    d1, d2, d3 = s((px, py), a, b), s((px, py), b, c), s((px, py), c, a)
    neg = d1 < 0 or d2 < 0 or d3 < 0
    pos = d1 > 0 or d2 > 0 or d3 > 0
    return not (neg and pos)


EAR_L = ((2.0, 17.0), (4.0, 1.0), (14.0, 9.0))
EAR_R = ((30.0, 17.0), (28.0, 1.0), (18.0, 9.0))
INNER_L = ((5.5, 14.5), (6.2, 5.5), (11.5, 10.5))
INNER_R = ((26.5, 14.5), (25.8, 5.5), (20.5, 10.5))

# 6x6 eyes. Slit pupil, lower half a darker amber. '.' = leave untouched.
EYE_L = [
    ".OOOO.",
    "OApAAO",
    "OApAAO",
    "OapaaO",
    "OaaaaO",
    ".OOOO.",
]
EYE_R = [
    ".OOOO.",
    "OAApAO",
    "OAApAO",
    "OaapaO",
    "OaaaaO",
    ".OOOO.",
]


def draw_base():
    """Return a 32x32 list of rows of RGBA tuples."""
    n = BASE
    shape = [[False] * n for _ in range(n)]
    for y in range(n):
        for x in range(n):
            if (
                inside_ellipse(x, y, 16, 19.5, 14.6, 11.6)
                or inside_tri(x, y, *EAR_L)
                or inside_tri(x, y, *EAR_R)
            ):
                shape[y][x] = True

    img = [[CLEAR] * n for _ in range(n)]

    def at(x, y):
        return shape[y][x] if 0 <= x < n and 0 <= y < n else False

    for y in range(n):
        for x in range(n):
            if not shape[y][x]:
                continue
            edge = not (at(x - 1, y) and at(x + 1, y) and at(x, y - 1) and at(x, y + 1))
            if edge:
                img[y][x] = C["O"]
                continue
            # fur shading: light from the upper left, darker toward the chin
            t = (x * 0.55 + y * 1.0) / (n * 1.4)
            if t < 0.30:
                k = "l"
            elif t < 0.62:
                k = "m"
            else:
                k = "d"
            # checker dither on the band edges for a retro look
            if (x + y) % 2 == 0:
                if 0.26 < t < 0.34:
                    k = "m"
                elif 0.58 < t < 0.66:
                    k = "d"
            img[y][x] = C[k]

    # inner ears (pink-purple), drawn inside the fur, darker toward the base
    for tri in (INNER_L, INNER_R):
        for y in range(n):
            for x in range(n):
                if inside_tri(x, y, *tri) and img[y][x] not in (C["O"], CLEAR):
                    img[y][x] = C["i"] if y < 10 else C["I"]

    def put(x, y, k):
        img[y][x] = C[k]

    # forehead tufts between the ears
    for x, y in ((15, 11), (16, 11), (14, 12), (17, 12)):
        put(x, y, "h")

    for x0, rows in ((6, EYE_L), (20, EYE_R)):
        for dy, row in enumerate(rows):
            for dx, ch in enumerate(row):
                if ch != ".":
                    put(x0 + dx, 14 + dy, ch)
    # glints on the outer upper corner of each eye
    put(7, 15, "w")
    put(24, 15, "w")

    # nose + mouth
    put(15, 21, "n")
    put(16, 21, "n")
    put(15, 22, "O")
    put(16, 22, "O")
    put(14, 23, "O")
    put(17, 23, "O")
    put(13, 22, "O")
    put(18, 22, "O")

    # whiskers: short light strokes on the cheeks
    for x, y in ((3, 22), (4, 22), (5, 23), (3, 25), (4, 24), (5, 24)):
        put(x, y, "W")
    for x, y in ((28, 22), (27, 22), (26, 23), (28, 25), (27, 24), (26, 24)):
        put(x, y, "W")
    return img


def resample(img, size):
    """Resample the 32x32 image to size x size. Integer multiples use
    nearest-neighbour (crisp pixel art); smaller sizes box-average
    (premultiplied alpha); other sizes (48) use bilinear."""
    n = len(img)
    if size == n:
        return [row[:] for row in img]
    if size % n == 0:
        f = size // n
        return [[img[y // f][x // f] for x in range(size)] for y in range(size)]
    if size < n:
        f = n // size
        out = []
        for y in range(size):
            row = []
            for x in range(size):
                r = g = b = a = 0
                for dy in range(f):
                    for dx in range(f):
                        p = img[y * f + dy][x * f + dx]
                        r += p[0] * p[3]
                        g += p[1] * p[3]
                        b += p[2] * p[3]
                        a += p[3]
                if a == 0:
                    row.append(CLEAR)
                else:
                    row.append((round(r / a), round(g / a), round(b / a), round(a / (f * f))))
            out.append(row)
        return out
    out = []
    s = n / size
    for y in range(size):
        sy = min(max((y + 0.5) * s - 0.5, 0), n - 1)
        y0 = int(sy)
        y1 = min(y0 + 1, n - 1)
        fy = sy - y0
        row = []
        for x in range(size):
            sx = min(max((x + 0.5) * s - 0.5, 0), n - 1)
            x0 = int(sx)
            x1 = min(x0 + 1, n - 1)
            fx = sx - x0
            acc = [0.0, 0.0, 0.0, 0.0]
            for px, py, w in (
                (x0, y0, (1 - fx) * (1 - fy)),
                (x1, y0, fx * (1 - fy)),
                (x0, y1, (1 - fx) * fy),
                (x1, y1, fx * fy),
            ):
                p = img[py][px]
                acc[0] += p[0] * p[3] * w
                acc[1] += p[1] * p[3] * w
                acc[2] += p[2] * p[3] * w
                acc[3] += p[3] * w
            if acc[3] <= 0:
                row.append(CLEAR)
            else:
                row.append(
                    (round(acc[0] / acc[3]), round(acc[1] / acc[3]), round(acc[2] / acc[3]), round(acc[3]))
                )
        out.append(row)
    return out


def fix_small(img):
    """Box-averaging smears the 6x6 eyes into brown mud at 16 px, so repaint
    them by hand: a 2x3 amber block (darker bottom row) with a dark rim."""
    for x0 in (3, 11):
        for y in range(6, 11):
            for x in range(x0 - 1, x0 + 3):
                img[y][x] = C["O"] if y in (6, 10) or x in (x0 - 1, x0 + 2) else img[y][x]
        for y, k in ((7, "A"), (8, "A"), (9, "a")):
            for x in (x0, x0 + 1):
                img[y][x] = C[k]
        img[7][x0 + (0 if x0 == 3 else 1)] = C["w"]


def png_bytes(img):
    h = len(img)
    w = len(img[0])
    raw = bytearray()
    for row in img:
        raw.append(0)
        for r, g, b, a in row:
            raw += bytes((r, g, b, a))

    def chunk(tag, data):
        c = struct.pack(">I", len(data)) + tag + data
        return c + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)

    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(bytes(raw), 9))
        + chunk(b"IEND", b"")
    )


def ico_bytes(pngs):
    """ICO container with PNG-compressed entries (supported since Vista)."""
    sizes = sorted(pngs)
    out = bytearray(struct.pack("<HHH", 0, 1, len(sizes)))
    offset = 6 + 16 * len(sizes)
    body = bytearray()
    for s in sizes:
        data = pngs[s]
        dim = 0 if s >= 256 else s  # 0 means 256
        out += struct.pack("<BBBBHHII", dim, dim, 0, 0, 1, 32, len(data), offset + len(body))
        body += data
    return bytes(out + body)


def write(path, data):
    with open(path, "wb") as f:
        f.write(data)


def main():
    os.makedirs(OUT, exist_ok=True)
    base = draw_base()
    pngs = {}
    for s in SIZES:
        img = resample(base, s)
        if s == 16:
            fix_small(img)
        pngs[s] = png_bytes(img)
    for s, data in pngs.items():
        write(os.path.join(OUT, f"grimalkin-{s}.png"), data)
    write(os.path.join(OUT, "grimalkin.ico"), ico_bytes({s: pngs[s] for s in ICO_SIZES}))

    if "--no-icns" in sys.argv:
        return
    if not shutil.which("iconutil"):
        print("iconutil not found (macOS only): leaving assets/icon/grimalkin.icns as is")
        return
    with tempfile.TemporaryDirectory() as tmp:
        iconset = os.path.join(tmp, "grimalkin.iconset")
        os.makedirs(iconset)
        for size in (16, 32, 128, 256, 512):
            write(os.path.join(iconset, f"icon_{size}x{size}.png"), pngs[size])
            write(os.path.join(iconset, f"icon_{size}x{size}@2x.png"), pngs[size * 2])
        subprocess.run(
            ["iconutil", "-c", "icns", iconset, "-o", os.path.join(OUT, "grimalkin.icns")],
            check=True,
        )


if __name__ == "__main__":
    main()
