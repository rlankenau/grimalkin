#!/usr/bin/env python3
"""Validate the built-in skins under crates/gui/assets/skins and optionally
render contact sheets.

    python3 scripts/check-skins.py                    # validate, exit 1 on failure
    python3 scripts/check-skins.py --montage /tmp/skins   # also write PNG contact sheets

Pure Python 3 (no tomllib / Pillow needed; a tiny TOML subset parser and PNG
reader live here and in skinkit.py).
"""
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from skinkit import Img, checker, compose, hx, read_png, write_png  # noqa: E402

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SKINS = os.path.join(ROOT, 'crates', 'gui', 'assets', 'skins')
NAMES = ['llama', 'grimalkin', 'mint']
BTN = ['close', 'minimize', 'shade', 'maximize']
DESK = ((178, 184, 190, 255), (160, 166, 172, 255))  # light 'desktop' backdrop so dark shadows show


# --------------------------------------------------------------------------
# minimal TOML (tables, strings, numbers, arrays, inline tables)
# --------------------------------------------------------------------------
def _strip_comment(line):
    out, q = [], False
    for ch in line:
        if ch == '"':
            q = not q
        if ch == '#' and not q:
            break
        out.append(ch)
    return ''.join(out).strip()


class _P:
    def __init__(self, s):
        self.s, self.i = s, 0

    def ws(self):
        while self.i < len(self.s) and self.s[self.i] in ' \t':
            self.i += 1

    def value(self):
        self.ws()
        c = self.s[self.i]
        if c == '"':
            j = self.s.index('"', self.i + 1)
            v = self.s[self.i + 1:j]
            self.i = j + 1
            return v
        if c == '{':
            self.i += 1
            d = {}
            while True:
                self.ws()
                if self.s[self.i] == '}':
                    self.i += 1
                    return d
                m = re.compile(r'[A-Za-z0-9_\-]+').match(self.s, self.i)
                k = m.group(0)
                self.i = m.end()
                self.ws()
                assert self.s[self.i] == '='
                self.i += 1
                d[k] = self.value()
                self.ws()
                if self.s[self.i] == ',':
                    self.i += 1
        if c == '[':
            self.i += 1
            a = []
            while True:
                self.ws()
                if self.s[self.i] == ']':
                    self.i += 1
                    return a
                a.append(self.value())
                self.ws()
                if self.s[self.i] == ',':
                    self.i += 1
        m = re.compile(r'-?[0-9]+(\.[0-9]+)?').match(self.s, self.i)
        assert m, 'bad toml value: ' + self.s[self.i:]
        self.i = m.end()
        return float(m.group(0)) if '.' in m.group(0) else int(m.group(0))


def parse_toml(text):
    root, cur = {}, None
    cur = root
    for raw in text.splitlines():
        line = _strip_comment(raw)
        if not line:
            continue
        if line.startswith('['):
            cur = root.setdefault(line.strip('[] '), {})
            continue
        k, _, v = line.partition('=')
        cur[k.strip()] = _P(v.strip()).value()
    return root


# --------------------------------------------------------------------------
# checks
# --------------------------------------------------------------------------
class Fail(Exception):
    pass


def need(cond, msg):
    if not cond:
        raise Fail(msg)


def resolve(rect, W, H):
    x, y, w, h = rect['x'], rect['y'], rect['w'], rect['h']
    if x < 0:
        x += W
    if y < 0:
        y += H
    if w == -1:
        w = W
    return x, y, w, h


def load(name):
    d = os.path.join(SKINS, name)
    with open(os.path.join(d, 'skin.toml')) as f:
        cfg = parse_toml(f.read())
    img = {n: read_png(os.path.join(d, n + '.png')) for n in ('frame', 'buttons', 'shade', 'preview')}
    return cfg, img


def check_skin(name):
    cfg, im = load(name)
    fr, bt, sh, pv = im['frame'], im['buttons'], im['shade'], im['preview']
    W, H = fr.w, fr.h
    A = lambda im_, x, y: im_.get(x, y)[3]  # noqa: E731
    need(cfg['name'], 'name')
    fcfg = cfg['frame']
    need(fcfg['image'] == 'frame.png', 'frame image name')
    need(300 <= W <= 340 and 220 <= H <= 270, 'frame should be ~320x240, got %dx%d' % (W, H))
    s = fcfg['slices']
    L, R, T, B = s['left'], s['right'], s['top'], s['bottom']
    vi = fcfg['viewport_inset']
    need((vi['left'], vi['right'], vi['top'], vi['bottom']) == (L, R, T, B), 'viewport_inset must equal slices')
    need(L + R < W and T + B < H, 'slices exceed frame')
    need((W - L - R) % 8 == 0 and (H - T - B) % 8 == 0, 'tile spans must be multiples of 8')
    need(fcfg['edge_mode'] in ('tile', 'stretch'), 'edge_mode')
    need(len(fcfg['viewport_background']) == 7, 'viewport_background')

    # viewport fully opaque, and uniform fill (hidden under the grid)
    for y in range(T, H - B):
        for x in range(L, W - R):
            need(A(fr, x, y) == 255, 'viewport hole at %d,%d' % (x, y))

    # ---- tileable edges repeat every 8 px (pixels incl. alpha)
    def px(i, x, y):
        return i.get(x, y)
    for y in range(0, T):
        for x in range(L, W - R - 8):
            need(px(fr, x, y) == px(fr, x + 8, y), 'top edge not 8px periodic at %d,%d' % (x, y))
    for y in range(H - B, H):
        for x in range(L, W - R - 8):
            need(px(fr, x, y) == px(fr, x + 8, y), 'bottom edge not 8px periodic at %d,%d' % (x, y))
    for x in range(0, L):
        for y in range(T, H - B - 8):
            need(px(fr, x, y) == px(fr, x, y + 8), 'left edge not 8px periodic at %d,%d' % (x, y))
    for x in range(W - R, W):
        for y in range(T, H - B - 8):
            need(px(fr, x, y) == px(fr, x, y + 8), 'right edge not 8px periodic at %d,%d' % (x, y))
    # the tile must also be self-consistent when its first and last tile meet the corners:
    # (edge pixels adjacent to the corner slices equal their 8-periodic neighbours)

    # ---- alpha is either fully transparent (0) or opaque except mint shadow
    hard = name != 'mint'
    if hard:
        for p in fr.p:
            need(p[3] in (0, 255), 'unexpected partial alpha in %s' % name)
    # transparent pixels carry no colour (clean PNG)
    for p in fr.p:
        if p[3] == 0:
            need(p[:3] == (0, 0, 0), 'transparent pixel with colour')

    # ---- palette limit: count distinct opaque colours (limited palettes)
    cols = set(p for p in fr.p if p[3] == 255)
    need(len(cols) <= 200, '%s uses %d colours, expected a limited palette' % (name, len(cols)))

    # ---- titlebar / buttons / text / grip rects inside the frame and inside the opaque shape
    tb = cfg['titlebar']
    x, y, w, h = resolve(tb['rect'], W, H)
    need(x >= 0 and y >= 0 and x + w <= W and y + h <= H, 'titlebar rect outside image')
    need(tb['double_click'] in ('shade', 'maximize', 'none'), 'double_click')
    t = tb['text']
    tx = t['x']
    tw = t['w']
    tr = W + tw if tw < 0 else tx + tw
    need(0 <= tx < tr <= W and 0 <= t['y'] < H, 'title text rect outside image')
    need(re.match(r'#[0-9a-fA-F]{6}$', t['color']), 'text color')
    need(t['align'] in ('left', 'center', 'right'), 'text align')

    bc = cfg['buttons']
    need(bc['sheet'] == 'buttons.png', 'sheet name')
    rects = []
    for k in BTN:
        b = bc[k]
        bx, by, bw, bh = resolve(b['rect'], W, H)
        sx, sy = b['sprite']
        need(bx >= 0 and by >= 0 and bx + bw <= W and by + bh <= H, '%s rect outside frame' % k)
        need(sx >= 0 and sy >= 0 and sx + bw <= bt.w and sy + 3 * bh <= bt.h,
             '%s sprite states outside buttons.png' % k)
        for yy in range(by, by + bh):
            for xx in range(bx, bx + bw):
                need(A(fr, xx, yy) == 255, '%s button rect leaves the window shape at %d,%d' % (k, xx, yy))
        # three distinct states, each with some opaque pixels
        states = []
        for st in range(3):
            cell = tuple(bt.get(sx + i, sy + st * bh + j) for j in range(bh) for i in range(bw))
            need(sum(1 for c in cell if c[3]) > bw * bh // 2, '%s state %d is mostly empty' % (k, st))
            states.append(cell)
        need(len(set(states)) == 3, '%s states must differ' % k)
        rects.append((bx, by, bx + bw, by + bh))
    for i in range(4):
        for j in range(i + 1, 4):
            a, b = rects[i], rects[j]
            need(a[2] <= b[0] or b[2] <= a[0] or a[3] <= b[1] or b[3] <= a[1], 'buttons overlap')
    # title text must not run under the buttons
    need(tr <= min(r[0] for r in rects), 'title text overlaps buttons')
    # the sheet has no stray content beyond the four columns / three rows
    need(bt.w == 4 * (rects[0][2] - rects[0][0]) and bt.h == 3 * (rects[0][3] - rects[0][1]), 'sheet size')

    rz = cfg['resize']
    gx, gy, gw, gh = resolve(rz['grip'], W, H)
    need(gx >= W - R - 8 and gy >= H - B - 8 and gx + gw <= W and gy + gh <= H, 'grip must be in bottom-right corner')
    need(any(A(fr, xx, yy) == 255 for yy in range(gy, gy + gh) for xx in range(gx, gx + gw)), 'grip has no pixels')
    need(rz['border'] >= 3, 'resize border')

    # ---- shade bar
    sc = cfg['shade']
    need(sc['image'] == 'shade.png' and sc['height'] == sh.h, 'shade height mismatch')
    need(sh.w == W, 'shade width must equal frame width')
    need(sh.h >= max(r[3] for r in rects) and sh.h <= T, 'shade height vs buttons / top slice')
    for y in range(sh.h):
        for x in range(L, W - R - 8):
            need(sh.get(x, y) == sh.get(x + 8, y), 'shade not 8px periodic at %d,%d' % (x, y))
    for (bx, by, bx1, by1) in rects:
        for yy in range(by, by1):
            for xx in range(bx, bx1):
                need(A(sh, xx, yy) == 255, 'button outside shade shape')
    # the shade bar re-uses the frame's top rows (same art above the closing bevel)
    same = sum(1 for y in range(0, sh.h - 4) for x in range(W) if sh.get(x, y) == fr.get(x, y))
    need(same > 0.8 * W * (sh.h - 4), 'shade top should match frame top')
    need(all(A(sh, x, sh.h - 1) == 0 for x in range(0, 2)), 'shade bottom corner should be cut')

    # ---- preview
    need((pv.w, pv.h) == (160, 60), 'preview must be 160x60')

    # ---- per-skin silhouette expectations (alpha < 128 = outside)
    out = lambda x, y: A(fr, x, y) < 128  # noqa: E731
    ins = lambda x, y: A(fr, x, y) >= 128  # noqa: E731
    if name == 'llama':
        need(out(0, 0) and out(4, 4) and out(W - 1, 0) and out(W - 5, 4), 'llama top corners must be notched')
        need(ins(10, 0) and ins(W - 11, 0) and ins(W // 2, 0), 'llama top edge opaque')
        need(out(0, H - 1) and out(W // 2, H - 1) and out(W // 2, H - 9), 'llama bottom edge is raised')
        need(ins(W // 2, H - 10), 'llama raised bottom row')
        need(ins(W - 1, H - 1) and out(W - 12, H - 3) and ins(W - 12, H - 4) and out(W - 18, H - 6)
             and ins(W - 18, H - 7), 'llama stepped wing')
        need(W - 20 >= W - R, 'wing must live inside the right slice')
    elif name == 'grimalkin':
        need(out(0, 0) and out(W // 2, 0) and out(W // 2, 10) and out(W - 1, 0), 'cat: gap between ears')
        need(any(ins(x, 2) for x in range(6, 14)) and any(ins(x, 2) for x in range(W - 14, W - 6)),
             'cat: ear tips must reach the top')
        need(ins(W // 2, 22 + 3), 'cat: body below the ears')
        need(all(out(W // 2, y) for y in range(0, 22)), 'cat: nothing between the ears above the body')
        need(ins(3, 22 + 8) and out(0, H - 17), 'cat: rounded bottom-left corner')
        yb = H - 17
        need(out(W // 2, yb + 3) and ins(W // 2, yb), 'cat: body bottom edge')
        tail = [(x, y) for y in range(yb + 1, H) for x in range(W - R, W) if ins(x, y)]
        need(len(tail) > 60, 'cat: tail missing (%d px)' % len(tail))
        need(all(out(x, y) for y in range(yb + 1, H) for x in range(L, W - R)), 'cat: tail must stay in the corner slice')
        need(all(out(x, y) for y in range(yb + 1, H) for x in range(0, L)), 'cat: bottom edge clear')
        need(L >= 24 and 22 <= T, 'cat: ears must fit inside the corner slices')
    elif name == 'mint':
        need(out(0, 0) and out(2, 2) and ins(12, 0) and ins(0, 12), 'mint: 12px rounded top-left corner')
        need(ins(W // 2, 0) and ins(8, 4) and out(1, 1), 'mint: top edge')
        # shadow: partially transparent but >=128 pixels beyond the body at right and bottom
        sh_px = sum(1 for y in range(H) for x in range(W) if fr.get(x, y)[3] >= 128 and fr.get(x, y)[3] < 255)
        need(sh_px > 800, 'mint: baked drop shadow missing (%d)' % sh_px)
        need(sum(1 for x in range(W - 8, W) for y in range(H // 2, H // 2 + 4) if ins(x, y)) >= 6, 'mint: right shadow')
        need(sum(1 for y in range(H - 8, H) for x in range(W // 2, W // 2 + 4) if ins(x, y)) >= 6, 'mint: bottom shadow')
        need(out(W - 1, H - 1) or out(W - 1, H - 2), 'mint: shadow tapers off at the corner')
        # body alpha is 255, shadow alpha is a dark colour
        need(fr.get(W // 2, H // 2)[3] == 255, 'mint: body alpha')
        dark = [fr.get(x, y) for x in range(W - 8, W) for y in range(H // 2, H // 2 + 4)]
        need(any(d[3] in range(128, 255) and sum(d[:3]) < 200 for d in dark), 'mint: shadow should be dark + translucent')
    print('ok  %-10s frame %dx%d  slices L%d R%d T%d B%d  shade %dx%d  colours %d' %
          (name, W, H, L, R, T, B, sh.w, sh.h, len(cols)))


# --------------------------------------------------------------------------
# contact sheets
# --------------------------------------------------------------------------
def composite(name):
    """Frame with buttons (normal state), title text and a fake terminal."""
    cfg, im = load(name)
    fr, bt = im['frame'], im['buttons']
    W, H = fr.w, fr.h
    s = cfg['frame']['slices']
    L, R, T, B = s['left'], s['right'], s['top'], s['bottom']
    t = cfg['titlebar']['text']
    tcol = hx(t['color'])
    termcol = tcol if sum(tcol[:3]) > 300 else (143, 232, 188, 255)
    btns = []
    for k in BTN:
        b = cfg['buttons'][k]
        bx, by, bw, bh = resolve(b['rect'], W, H)
        btns.append((bx, by, bw, bh, b['sprite'][0], b['sprite'][1]))
    lines = ['$ GRIMALKIN --SKIN ' + name.upper(), '> SKIN LOADED', '$ _']
    out = compose(fr, bt, btns, 'GRIMALKIN - ' + name.upper(), t['x'], W + t['w'] if t['w'] < 0 else t['x'] + t['w'],
                  t['y'] + 1, t['align'], tcol, lines, termcol, (L + 6, T + 6))
    return cfg, im, out


def montage(outdir, names):
    os.makedirs(outdir, exist_ok=True)
    tiles = []
    for name in names:
        cfg, im, comp = composite(name)
        fr = im['frame']
        # per skin detail sheet @3x
        F = comp.over(checker(comp.w, comp.h, *DESK)).scaled(2)
        sh = im['shade'].over(checker(im['shade'].w, im['shade'].h, *DESK)).scaled(2)
        bt = im['buttons'].over(checker(im['buttons'].w, im['buttons'].h, (90, 60, 90, 255), (110, 80, 110, 255), 4)).scaled(6)
        pv = im['preview'].scaled(2)
        w = max(F.w, sh.w, pv.w + bt.w + 8)
        h = F.h + sh.h + 12 + max(pv.h, bt.h) + 8
        sheet = Img(w, h, (30, 30, 34, 255))
        sheet.blit(F, 0, 0)
        sheet.blit(sh, 0, F.h + 6)
        sheet.blit(pv, 0, F.h + sh.h + 12)
        sheet.blit(bt, pv.w + 8, F.h + sh.h + 12)
        write_png(os.path.join(outdir, 'skin-%s.png' % name), sheet)
        tiles.append((comp, im['shade'], im['preview']))
    # overview at 1x
    gap = 10
    w = sum(t[0].w for t in tiles) + gap * (len(tiles) + 1)
    fh = max(t[0].h for t in tiles)
    sh_h = max(t[1].h for t in tiles)
    h = fh + sh_h + 60 + gap * 4
    ov = Img(w, h, (30, 30, 34, 255))
    x = gap
    for (comp, sh, pv) in tiles:
        ov.blit(comp.over(checker(comp.w, comp.h, *DESK)), x, gap)
        ov.blit(sh.over(checker(sh.w, sh.h, *DESK)), x, gap * 2 + fh)
        ov.blit(pv, x, gap * 3 + fh + sh_h)
        x += comp.w + gap
    write_png(os.path.join(outdir, 'contact-sheet.png'), ov)
    print('wrote contact sheets to', outdir)


def main(argv):
    names = [a for a in argv if not a.startswith('--') and a in NAMES] or NAMES
    failed = False
    for n in names:
        try:
            check_skin(n)
        except (Fail, KeyError, AssertionError) as e:
            failed = True
            print('FAIL %-10s %s' % (n, e))
    if '--montage' in argv:
        montage(argv[argv.index('--montage') + 1], names)
    return 1 if failed else 0


if __name__ == '__main__':
    sys.exit(main(sys.argv[1:]))
