#!/usr/bin/env python3
"""Generate the tiny `debug` placeholder skin used to exercise the skin engine.

Writes crates/gui/assets/skins/debug/{frame.png,shade.png,buttons.png}. Pure
standard library (zlib + struct); no PIL needed. The frame is 64x64 with
notched corners (top-left/top-right 45-degree cuts, bottom-left small round
cut, bottom-right big 45-degree cut) so the shape mask is easy to verify, a
striped title band so tiling is visible, and a magenta centre that must never
show (the centre is the terminal viewport).
"""
import os
import struct
import sys
import zlib

OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "crates", "gui", "assets", "skins", "debug")


def png(path, w, h, px):
    raw = b"".join(b"\x00" + bytes(c for p in px[y * w:(y + 1) * w] for c in p) for y in range(h))

    def chunk(t, d):
        c = struct.pack(">I", len(d)) + t + d
        return c + struct.pack(">I", zlib.crc32(t + d) & 0xFFFFFFFF)

    data = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 0))
    data += chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b"")
    with open(path, "wb") as f:
        f.write(data)


CLEAR = (0, 0, 0, 0)


def frame():
    w = h = 64
    px = []
    for y in range(h):
        for x in range(w):
            c = (0x2B, 0x3A, 0x55, 255)
            if y < 16:  # title band: gradient + stripes in the tiled middle
                t = y / 15.0
                c = (int(0x5A - 0x20 * t), int(0x80 - 0x28 * t), int(0xB8 - 0x30 * t), 255)
                if 8 <= x < 56 and (x - 8) % 16 < 8:
                    c = (c[0] + 18, c[1] + 18, c[2] + 18, 255)
            # bevel: light top/left, dark bottom/right
            if x == 1 or y == 1:
                c = (0xB0, 0xC8, 0xF0, 255)
            if x == w - 2 or y == h - 2:
                c = (0x10, 0x18, 0x28, 255)
            if x in (0, w - 1) or y in (0, h - 1):
                c = (0x08, 0x0C, 0x14, 255)
            # centre = viewport: must never be visible
            if 8 <= x < 56 and 16 <= y < 56:
                c = (255, 0, 255, 255)
            # notches
            if y < 6 and x < 6 - y:
                c = CLEAR
            if y < 6 and x >= w - 6 + y:
                c = CLEAR
            if y >= h - 3 and x < 3 - (h - 1 - y):
                c = CLEAR
            if x >= 56 and y >= 56 and (x - 56) + (y - 56) >= 7:
                c = CLEAR
            px.append(c)
    return w, h, px


def shade():
    w, h = 64, 12
    px = []
    for y in range(h):
        for x in range(w):
            t = y / 11.0
            c = (int(0x5A - 0x20 * t), int(0x80 - 0x28 * t), int(0xB8 - 0x30 * t), 255)
            if y == 0 or x == 0 or x == w - 1 or y == h - 1:
                c = (0x08, 0x0C, 0x14, 255)
            elif y == 1:
                c = (0xB0, 0xC8, 0xF0, 255)
            if y < 4 and x < 4 - y:
                c = CLEAR
            if y < 4 and x >= w - 4 + y:
                c = CLEAR
            px.append(c)
    return w, h, px


def buttons():
    w, h = 40, 30
    px = [CLEAR] * (w * h)

    def put(x, y, c):
        px[y * w + x] = c

    glyphs = {
        0: lambda x, y: x == y or x == 9 - y,  # close: X
        1: lambda x, y: y == 7 and 2 <= x <= 7,  # minimize: underscore
        2: lambda x, y: y in (3, 6) and 2 <= x <= 7,  # shade: two bars
        3: lambda x, y: (x in (2, 7) and 2 <= y <= 7) or (y in (2, 7) and 2 <= x <= 7),  # maximize: box
    }
    bases = [(0xC0, 0xC0, 0xC0), (0xFF, 0xE0, 0x66), (0xFF, 0x66, 0x44)]  # normal, hover, pressed
    for state in range(3):
        for b in range(4):
            for y in range(10):
                for x in range(10):
                    edge = x in (0, 9) or y in (0, 9)
                    c = (0x20, 0x20, 0x20, 255) if edge else (*bases[state], 255)
                    if not edge and glyphs[b](x, y):
                        c = (0x10, 0x10, 0x10, 255)
                    put(b * 10 + x, state * 10 + y, c)
    return w, h, px


os.makedirs(OUT, exist_ok=True)
for name, fn in (("frame.png", frame), ("shade.png", shade), ("buttons.png", buttons)):
    w, h, px = fn()
    png(os.path.join(OUT, name), w, h, px)
    print("wrote", name, w, h, file=sys.stderr)
