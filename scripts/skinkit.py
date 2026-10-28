"""Tiny pure-Python raster toolkit shared by gen-skins.py and check-skins.py.

No third-party packages: images are lists of RGBA tuples, PNGs are written and
read with zlib only.  Everything here is deterministic.
"""
import math
import struct
import zlib


# --------------------------------------------------------------------------
# colours
# --------------------------------------------------------------------------
def hx(s):
    s = s.lstrip('#')
    return (int(s[0:2], 16), int(s[2:4], 16), int(s[4:6], 16), 255)


def mix(a, b, t):
    return tuple(int(round(a[i] + (b[i] - a[i]) * t)) for i in range(3)) + (255,)


def clamp01(v):
    return 0.0 if v < 0 else 1.0 if v > 1 else v


BAYER4 = (
    (0, 8, 2, 10),
    (12, 4, 14, 6),
    (3, 11, 1, 9),
    (15, 7, 13, 5),
)


def dith(t, ramp, x, y):
    """Ordered-dither pick from a colour ramp for t in 0..1 (period 4 in x and y)."""
    t = clamp01(t)
    pos = t * (len(ramp) - 1)
    i = int(pos)
    if i >= len(ramp) - 1:
        return ramp[-1]
    f = pos - i
    return ramp[i + 1] if f > (BAYER4[y & 3][x & 3] + 0.5) / 16.0 else ramp[i]


# --------------------------------------------------------------------------
# image
# --------------------------------------------------------------------------
CLEAR = (0, 0, 0, 0)


class Img:
    def __init__(self, w, h, fill=CLEAR):
        self.w = w
        self.h = h
        self.p = [fill] * (w * h)

    def get(self, x, y):
        if 0 <= x < self.w and 0 <= y < self.h:
            return self.p[y * self.w + x]
        return CLEAR

    def set(self, x, y, c):
        if 0 <= x < self.w and 0 <= y < self.h:
            self.p[y * self.w + x] = c if len(c) == 4 else (c[0], c[1], c[2], 255)

    def rect(self, x0, y0, x1, y1, c):
        """Fill inclusive rectangle."""
        for y in range(y0, y1 + 1):
            for x in range(x0, x1 + 1):
                self.set(x, y, c)

    def hline(self, x0, x1, y, c):
        for x in range(x0, x1 + 1):
            self.set(x, y, c)

    def vline(self, x, y0, y1, c):
        for y in range(y0, y1 + 1):
            self.set(x, y, c)

    def blit(self, src, ox, oy):
        for y in range(src.h):
            for x in range(src.w):
                c = src.p[y * src.w + x]
                if c[3]:
                    self.set(ox + x, oy + y, c)

    def crop(self, x0, y0, w, h):
        out = Img(w, h)
        for y in range(h):
            for x in range(w):
                out.p[y * w + x] = self.get(x0 + x, y0 + y)
        return out

    def scaled(self, k):
        out = Img(self.w * k, self.h * k)
        for y in range(out.h):
            for x in range(out.w):
                out.p[y * out.w + x] = self.p[(y // k) * self.w + (x // k)]
        return out

    def over(self, bg):
        """Composite onto an opaque image of the same size (hard alpha at >=128)."""
        out = Img(self.w, self.h)
        for i, c in enumerate(self.p):
            if c[3] >= 255:
                out.p[i] = c
            elif c[3] == 0:
                out.p[i] = bg.p[i]
            else:
                b = bg.p[i]
                a = c[3] / 255.0
                out.p[i] = tuple(int(round(c[k] * a + b[k] * (1 - a))) for k in range(3)) + (255,)
        return out


def checker(w, h, a=(52, 52, 56, 255), b=(70, 70, 76, 255), cell=8):
    im = Img(w, h)
    for y in range(h):
        for x in range(w):
            im.p[y * w + x] = a if ((x // cell) + (y // cell)) & 1 else b
    return im


# --------------------------------------------------------------------------
# PNG io (RGBA8, filter 0 on write; all five filters on read)
# --------------------------------------------------------------------------
def png_bytes(img):
    raw = bytearray()
    for y in range(img.h):
        raw.append(0)
        row = img.p[y * img.w:(y + 1) * img.w]
        for c in row:
            if c[3] == 0:
                raw.extend(b'\x00\x00\x00\x00')
            else:
                raw.extend(bytes(c))

    def chunk(t, d):
        body = t + d
        return struct.pack('>I', len(d)) + body + struct.pack('>I', zlib.crc32(body) & 0xFFFFFFFF)

    return (b'\x89PNG\r\n\x1a\n'
            + chunk(b'IHDR', struct.pack('>IIBBBBB', img.w, img.h, 8, 6, 0, 0, 0))
            + chunk(b'IDAT', zlib.compress(bytes(raw), 9))
            + chunk(b'IEND', b''))


def write_png(path, img):
    with open(path, 'wb') as f:
        f.write(png_bytes(img))


def read_png(path):
    with open(path, 'rb') as f:
        data = f.read()
    assert data[:8] == b'\x89PNG\r\n\x1a\n', 'not a PNG: ' + path
    pos = 8
    idat = b''
    w = h = ct = bd = None
    while pos < len(data):
        n = struct.unpack('>I', data[pos:pos + 4])[0]
        t = data[pos + 4:pos + 8]
        body = data[pos + 8:pos + 8 + n]
        pos += 12 + n
        if t == b'IHDR':
            w, h, bd, ct = struct.unpack('>IIBB', body[:10])
        elif t == b'IDAT':
            idat += body
    assert bd == 8 and ct == 6, 'expected 8-bit RGBA PNG: ' + path
    raw = zlib.decompress(idat)
    stride = w * 4
    img = Img(w, h)
    prev = bytearray(stride)
    p = 0
    for y in range(h):
        ft = raw[p]
        line = bytearray(raw[p + 1:p + 1 + stride])
        p += 1 + stride
        for i in range(stride):
            a = line[i - 4] if i >= 4 else 0
            b = prev[i]
            c = prev[i - 4] if i >= 4 else 0
            if ft == 1:
                line[i] = (line[i] + a) & 255
            elif ft == 2:
                line[i] = (line[i] + b) & 255
            elif ft == 3:
                line[i] = (line[i] + ((a + b) >> 1)) & 255
            elif ft == 4:
                pa, pb, pc = abs(b - c), abs(a - c), abs(a + b - 2 * c)
                pr = a if (pa <= pb and pa <= pc) else (b if pb <= pc else c)
                line[i] = (line[i] + pr) & 255
        prev = line
        for x in range(w):
            img.p[y * w + x] = tuple(line[x * 4:x * 4 + 4])
    return img


# --------------------------------------------------------------------------
# masks, depth fields, bevel/outline pass
# --------------------------------------------------------------------------
class Mask:
    def __init__(self, w, h, fn):
        self.w = w
        self.h = h
        self.m = [[bool(fn(x, y)) for x in range(w)] for y in range(h)]
        self._d = None

    def __call__(self, x, y):
        return 0 <= x < self.w and 0 <= y < self.h and self.m[y][x]

    def depths(self):
        """Per pixel run lengths (including self) of inside pixels to the
        left / up / right / down.  1 means the neighbour in that direction is
        outside (or beyond the image)."""
        if self._d:
            return self._d
        w, h, m = self.w, self.h, self.m
        dl = [[0] * w for _ in range(h)]
        du = [[0] * w for _ in range(h)]
        dr = [[0] * w for _ in range(h)]
        dd = [[0] * w for _ in range(h)]
        for y in range(h):
            for x in range(w):
                if m[y][x]:
                    dl[y][x] = (dl[y][x - 1] if x else 0) + 1
                    du[y][x] = (du[y - 1][x] if y else 0) + 1
        for y in range(h - 1, -1, -1):
            for x in range(w - 1, -1, -1):
                if m[y][x]:
                    dr[y][x] = (dr[y][x + 1] if x < w - 1 else 0) + 1
                    dd[y][x] = (dd[y + 1][x] if y < h - 1 else 0) + 1
        self._d = (dl, du, dr, dd)
        return self._d

    def dist(self, x, y):
        dl, du, dr, dd = self.depths()
        return min(dl[y][x], du[y][x], dr[y][x], dd[y][x])


def finish(img, mask, pal, nobevel=()):
    """Final pass: transparent outside, 1px outline, 2px light-from-top-left bevel."""
    dl, du, dr, dd = mask.depths()
    for y in range(img.h):
        for x in range(img.w):
            if not mask.m[y][x]:
                img.p[y * img.w + x] = CLEAR
                continue
            a, b, c, d = dl[y][x], du[y][x], dr[y][x], dd[y][x]
            if min(a, b, c, d) == 1:
                img.p[y * img.w + x] = pal['out']
                continue
            if (x, y) in nobevel:
                continue
            hi = [v for v in (a, b) if v in (2, 3)]
            lo = [v for v in (c, d) if v in (2, 3)]
            if len(hi) > len(lo):
                img.p[y * img.w + x] = pal['hi2'] if min(hi) == 2 else pal['hi3']
            elif len(lo) > len(hi):
                img.p[y * img.w + x] = pal['sh2'] if min(lo) == 2 else pal['sh3']
            else:
                cur = img.p[y * img.w + x]
                img.p[y * img.w + x] = (cur[0], cur[1], cur[2], 255)


def bevel_rect(img, x0, y0, x1, y1, tl, br):
    """Concentric 1px rings; tl/br are colour lists, ring 0 outermost."""
    for i in range(len(tl)):
        a0, b0, a1, b1 = x0 + i, y0 + i, x1 - i, y1 - i
        if a1 < a0 or b1 < b0:
            break
        img.hline(a0, a1, b1, br[i])
        img.vline(a1, b0, b1, br[i])
        img.hline(a0, a1, b0, tl[i])
        img.vline(a0, b0, b1, tl[i])


# --------------------------------------------------------------------------
# geometry helpers
# --------------------------------------------------------------------------
def in_poly(px, py, poly):
    inside = False
    n = len(poly)
    j = n - 1
    for i in range(n):
        xi, yi = poly[i]
        xj, yj = poly[j]
        if (yi > py) != (yj > py) and px < (xj - xi) * (py - yi) / (yj - yi) + xi:
            inside = not inside
        j = i
    return inside


def rrect_dist(px, py, x0, y0, x1, y1, r):
    """Signed distance (<=0 inside) from a point to a rounded rectangle
    spanning the continuous box [x0,x1]x[y0,y1] with corner radius r."""
    cx, cy = (x0 + x1) / 2.0, (y0 + y1) / 2.0
    hx_, hy_ = (x1 - x0) / 2.0 - r, (y1 - y0) / 2.0 - r
    qx, qy = abs(px - cx) - hx_, abs(py - cy) - hy_
    return math.hypot(max(qx, 0), max(qy, 0)) + min(max(qx, qy), 0) - r


def paint_glyph(img, rows, ox, oy, color):
    for j, row in enumerate(rows):
        for i, ch in enumerate(row):
            if ch == '#':
                img.set(ox + i, oy + j, color)


# --------------------------------------------------------------------------
# 5x7 pixel font (only what the previews need)
# --------------------------------------------------------------------------
FONT = {
    'A': ['.###.', '#...#', '#...#', '#####', '#...#', '#...#', '#...#'],
    'C': ['.####', '#....', '#....', '#....', '#....', '#....', '.####'],
    'D': ['####.', '#...#', '#...#', '#...#', '#...#', '#...#', '####.'],
    'E': ['#####', '#....', '#....', '####.', '#....', '#....', '#####'],
    'G': ['.####', '#....', '#....', '#.###', '#...#', '#...#', '.###.'],
    'H': ['#...#', '#...#', '#...#', '#####', '#...#', '#...#', '#...#'],
    'I': ['.###.', '..#..', '..#..', '..#..', '..#..', '..#..', '.###.'],
    'K': ['#...#', '#..#.', '#.#..', '##...', '#.#..', '#..#.', '#...#'],
    'L': ['#....', '#....', '#....', '#....', '#....', '#....', '#####'],
    'M': ['#...#', '##.##', '#.#.#', '#.#.#', '#...#', '#...#', '#...#'],
    'N': ['#...#', '##..#', '#.#.#', '#.#.#', '#..##', '#...#', '#...#'],
    'O': ['.###.', '#...#', '#...#', '#...#', '#...#', '#...#', '.###.'],
    'P': ['####.', '#...#', '#...#', '####.', '#....', '#....', '#....'],
    'R': ['####.', '#...#', '#...#', '####.', '#.#..', '#..#.', '#...#'],
    'S': ['.####', '#....', '#....', '.###.', '....#', '....#', '####.'],
    'T': ['#####', '..#..', '..#..', '..#..', '..#..', '..#..', '..#..'],
    'U': ['#...#', '#...#', '#...#', '#...#', '#...#', '#...#', '.###.'],
    '>': ['#....', '.#...', '..#..', '...#.', '..#..', '.#...', '#....'],
    '$': ['..#..', '.####', '#.#..', '.###.', '..#.#', '####.', '..#..'],
    '_': ['.....', '.....', '.....', '.....', '.....', '.....', '#####'],
    ':': ['.....', '..#..', '..#..', '.....', '..#..', '..#..', '.....'],
    '-': ['.....', '.....', '.....', '#####', '.....', '.....', '.....'],
    '\\': ['#....', '#....', '.#...', '.#...', '..#..', '..#..', '...#.'],
    ' ': ['.....'] * 7,
}


def draw_text(img, x, y, text, color, spacing=1):
    for ch in text.upper():
        g = FONT.get(ch, FONT[' '])
        paint_glyph(img, g, x, y, color)
        x += 5 + spacing
    return x


def compose(frame, sheet, buttons, title, text_x0, text_x1, text_y, align, title_col, term_lines, term_col, origin):
    """Mock-up of a live window: frame + button sprites (normal state) + title
    text + a few terminal lines.  `buttons` = [(x, y, w, h, sx, sy)]."""
    out = Img(frame.w, frame.h)
    out.p = list(frame.p)
    for i, line in enumerate(term_lines):
        draw_text(out, origin[0], origin[1] + i * 10, line, term_col)
    tw = len(title) * 6 - 1
    tx = text_x0 + (text_x1 - text_x0 - tw) // 2 if align == 'center' else text_x0
    draw_text(out, tx, text_y, title, title_col)
    for (x, y, w, h, sx, sy) in buttons:
        out.blit(sheet.crop(sx, sy, w, h), x, y)
    return out
