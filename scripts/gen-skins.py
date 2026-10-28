#!/usr/bin/env python3
"""Generate the built-in grimalkin skins (docs/SKINS.md) as pixel art.

    python3 scripts/gen-skins.py            # writes crates/gui/assets/skins/*

Pure Python 3, no third-party packages (a tiny zlib PNG writer lives in
skinkit.py).  Output is fully deterministic: re-running must not change the
committed PNGs.  `scripts/check-skins.py` validates the result.

Every skin is 320 wide at 1x.  Tileable edge segments (the part of the top and
bottom strips between the left/right slices, and of the left and right walls
between the top/bottom slices) repeat every 8 px: every texture below is a
function of (x mod 8, y mod 8) or of the distance to the silhouette only.
"""
import math
import os
import random
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from skinkit import (Img, Mask, bevel_rect, checker, compose, dith, finish,  # noqa: E402
                     hx, in_poly, paint_glyph, rrect_dist, write_png)

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT = os.path.join(ROOT, 'crates', 'gui', 'assets', 'skins')

W = 320
BTN_ORDER = ['close', 'minimize', 'shade', 'maximize']

# deterministic 8x8 noise tiles (period 8 in both axes)
_rng = random.Random(1997)
ROW8 = [_rng.random() for _ in range(8)]
TILE8 = [[_rng.random() for _ in range(8)] for _ in range(8)]
FLECK8 = [[_rng.random() for _ in range(8)] for _ in range(8)]


def brush(x, y):
    """Horizontal brushed-metal streak noise 0..1, periodic 8x8."""
    return 0.62 * ROW8[y & 7] + 0.38 * TILE8[y & 7][x & 7]


# --------------------------------------------------------------------------
# shared drawing pieces
# --------------------------------------------------------------------------
def sunken(img, x0, y0, x1, y1, dark, darker, light, lighter):
    """Two-pixel sunken frame around the inclusive box (drawn *on* the box)."""
    bevel_rect(img, x0, y0, x1, y1, [dark, darker], [lighter, light])


def raised(img, x0, y0, x1, y1, hi, hi2, sh, sh2):
    bevel_rect(img, x0, y0, x1, y1, [hi, hi2], [sh, sh2])


def recess_ring(img, vx0, vy0, vx1, vy1, p):
    """Sunken 2px frame hugging the outside of the viewport box."""
    # ring 0 = adjacent to the viewport
    for i, (tl, br) in enumerate([(p['rec_tl0'], p['rec_br0']), (p['rec_tl1'], p['rec_br1'])]):
        x0, y0, x1, y1 = vx0 - 1 - i, vy0 - 1 - i, vx1 + 1 + i, vy1 + 1 + i
        img.hline(x0, x1, y1, br)
        img.vline(x1, y0, y1, br)
        img.hline(x0, x1, y0, tl)
        img.vline(x0, y0, y1, tl)


def lcd_fill(img, x0, y0, x1, y1, ramp, scan, glare=None, grid=None):
    """Scanline LCD: vertical dithered gradient, every other row darker."""
    n = y1 - y0
    for y in range(y0, y1 + 1):
        t = (y - y0) / float(max(1, n))
        for x in range(x0, x1 + 1):
            c = dith(0.15 + 0.7 * t, ramp, x, y)
            if (y & 1) == 0:
                c = scan(c)
            elif grid and (x & 3) == 0:
                c = grid(c)
            img.set(x, y, c)
    if glare:
        for x in range(x0, x1 + 1):
            if (x & 1) == 0:
                img.set(x, y0, glare)


def darken(f):
    return lambda c: (int(c[0] * f), int(c[1] * f), int(c[2] * f), 255)


def lighten(add):
    return lambda c: (min(255, c[0] + add[0]), min(255, c[1] + add[1]), min(255, c[2] + add[2]), 255)


def make_button_sheet(size, drawer):
    sheet = Img(size * 4, size * 3)
    for i, kind in enumerate(BTN_ORDER):
        for s in range(3):
            sheet.blit(drawer(kind, s), i * size, s * size)
    return sheet


GLYPH8 = {
    'close': ['##....##', '###..###', '.######.', '..####..', '..####..', '.######.', '###..###', '##....##'],
    'minimize': ['........', '........', '........', '........', '........', '........', '########', '########'],
    'shade': ['########', '########', '........', '.######.', '..####..', '...##...', '........', '........'],
    'maximize': ['########', '########', '#......#', '#......#', '#......#', '#......#', '#......#', '########'],
}
GLYPH6 = {
    'close': ['##..##', '.####.', '..##..', '..##..', '.####.', '##..##'],
    'minimize': ['......', '......', '......', '......', '######', '######'],
    'shade': ['######', '######', '......', '.####.', '..##..', '......'],
    'maximize': ['######', '######', '#....#', '#....#', '#....#', '######'],
}


def write_skin(name, toml, frame, buttons, shade, preview):
    d = os.path.join(OUT, name)
    os.makedirs(d, exist_ok=True)
    write_png(os.path.join(d, 'frame.png'), frame)
    write_png(os.path.join(d, 'buttons.png'), buttons)
    write_png(os.path.join(d, 'shade.png'), shade)
    write_png(os.path.join(d, 'preview.png'), preview)
    with open(os.path.join(d, 'skin.toml'), 'w') as f:
        f.write(toml)


def compose_for(g, frame, sheet, title, term_lines, term_col, title_col):
    """Window mock-up built from the same geometry that drives skin.toml."""
    btns = []
    for i, kind in enumerate(BTN_ORDER):
        btns.append((W + g['btn_x'][kind], g['btn_y'], g['btn_w'], g['btn_h'], i * g['btn_w'], 0))
    tx, ty, tw = g['text']
    return compose(frame, sheet, btns, title, tx, W + tw, ty + 1, g['text_align'], title_col,
                   term_lines, term_col, (g['L'] + 6, g['T'] + 6))


def make_preview(win, g):
    """160x60 crop of the top-right of the live mock-up (title bar, buttons,
    shaped corner, a little terminal) over a checkerboard."""
    return win.crop(W - 160, 0, 160, 60).over(checker(160, 60))


def toml_common(name, author, g, bg, title, btn, shade_title=None):
    """Render skin.toml.  `g` carries the geometry; every number in here is
    produced from the same constants that drive the painters."""
    L, R, T, B = g['L'], g['R'], g['T'], g['B']
    tb = g['titlebar']
    tx = g['text']
    s = []
    s.append('name = "%s"' % name)
    s.append('author = "%s"' % author)
    s.append('')
    s.append('# 9-slice frame. Corners are fixed; the tileable edge segments repeat every 8 px,')
    s.append('# so `tile` stretches without seams.  frame.png is %dx%d at 1x.' % (g['W'], g['H']))
    s.append('[frame]')
    s.append('image = "frame.png"')
    s.append('slices = { left = %d, right = %d, top = %d, bottom = %d }' % (L, R, T, B))
    s.append('edge_mode = "tile"')
    s.append('viewport_inset = { left = %d, right = %d, top = %d, bottom = %d }' % (L, R, T, B))
    s.append('viewport_background = "%s"' % bg)
    s.append('viewport_opacity = 1.0')
    s.append('')
    s.append('[titlebar]')
    s.append('rect = { x = 0, y = %d, w = -1, h = %d }' % (tb[0], tb[1]))
    s.append('double_click = "%s"' % g['dblclick'])
    s.append('# title text sits in the %s; its right end stops where the buttons begin' % g['text_where'])
    s.append('text = { x = %d, y = %d, w = %d, font_px = 11, color = "%s", align = "%s" }' %
             (tx[0], tx[1], tx[2], g['text_color'], g['text_align']))
    s.append('')
    s.append('# buttons.png: one column per button, normal / hover / pressed stacked vertically,')
    s.append('# each cell exactly the rect size (%dx%d).' % (g['btn_w'], g['btn_h']))
    s.append('[buttons]')
    s.append('sheet = "buttons.png"')
    for i, kind in enumerate(BTN_ORDER):
        bx = g['btn_x'][kind]
        s.append('%-8s = { rect = { x = %d, y = %d, w = %d, h = %d }, sprite = [%d, 0] }' %
                 (kind, bx, g['btn_y'], g['btn_w'], g['btn_h'], i * g['btn_w']))
    s.append('')
    s.append('[resize]')
    gr = g['grip']
    s.append('grip = { x = %d, y = %d, w = %d, h = %d }' % gr)
    s.append('border = %d' % g['resize_border'])
    s.append('')
    s.append('# shade.png is the top %d rows of the frame, closed off along the bottom, %d px wide' %
             (g['shade_h'], g['W']))
    s.append('# (left/right %d px are caps, the middle repeats every 8 px).  The titlebar and button rects' % L)
    s.append('# above apply to it unchanged.')
    s.append('[shade]')
    s.append('image = "shade.png"')
    s.append('height = %d' % g['shade_h'])
    s.append('')
    return '\n'.join(s)


# ==========================================================================
# LLAMA: dark brushed steel, green scanline LCD, notched corners + stepped wing
# ==========================================================================
LL = dict(W=320, H=240, L=24, R=24, T=32, B=32)

STEEL = [hx(c) for c in ('#1b2027', '#252b34', '#303843', '#3d4753', '#4e5967', '#627080', '#7a8a9b', '#97a8ba')]
LLP = dict(out=hx('#07090c'), hi2=hx('#e4eef8'), hi3=hx('#a3b6ca'), sh2=hx('#0f1318'), sh3=hx('#222932'),
           rec_tl0=hx('#0b0e12'), rec_tl1=hx('#222932'), rec_br0=hx('#7a8a9b'), rec_br1=hx('#b7c7d8'))
LCD_G = [hx(c) for c in ('#06120a', '#0a1f10', '#0f2c18', '#153a20')]
LCD_TXT = '#9bd6a1'
LED_ON = hx('#46e07a')
LED_DIM = hx('#15552d')
ORANGE = hx('#ff9a2e')
ORANGE_D = hx('#b3600f')

LLAMA_ICON = [
    '.#......#.',
    '.#......#.',
    '.##....##.',
    '.##.##.##.',
    '..######..',
    '..#e##e#..',
    '..######..',
    '...####...',
    '...#nn#...',
    '...####...',
    '..######..',
]


def llama_wing_bottom(x, H):
    """Last inside row for column x on the frame: raised edge with a 3-step
    wing flaring down toward the bottom-right corner."""
    if x < W - 20:
        return H - 10
    if x < W - 15:
        return H - 7
    if x < W - 10:
        return H - 4
    return H - 1


def llama_mask(kind):
    H = LL['H'] if kind == 'frame' else LL['T']

    def fn(x, y):
        if x + y < 10 or (W - 1 - x) + y < 10:
            return False
        if kind == 'frame':
            yb = llama_wing_bottom(x, H)
            if y > yb:
                return False
            if x + (H - 10 - y) < 8:
                return False
            return True
        if y > H - 1:
            return False
        if x + (H - 1 - y) < 5 or (W - 1 - x) + (H - 1 - y) < 5:
            return False
        return True
    return Mask(W, H, fn)


def llama_profile(d):
    pr = [0, 0.1, 0.2, 0.3, 0.80, 0.93, 0.95, 0.9, 0.82, 0.74, 0.66, 0.6, 0.55, 0.5]
    return pr[d] if d < len(pr) else 0.46


def paint_llama(kind):
    g = LL
    L, R, T, B = g['L'], g['R'], g['T'], g['B']
    H = g['H'] if kind == 'frame' else T
    mask = llama_mask(kind)
    img = Img(W, H)
    dl, du, dr, dd = mask.depths()
    vp = (L, T, W - R - 1, g['H'] - B - 1)
    for y in range(H):
        for x in range(W):
            if not mask.m[y][x]:
                continue
            if kind == 'frame' and vp[0] <= x <= vp[2] and vp[1] <= y <= vp[3]:
                img.set(x, y, hx('#000000'))
                continue
            d = min(dl[y][x], du[y][x], dr[y][x], dd[y][x])
            t = llama_profile(d) + (brush(x, y) - 0.5) * 0.2
            img.set(x, y, dith(t, STEEL, x, y))

    # walls: engraved groove (dark then light), light comes from the top-left
    if kind == 'frame':
        for y in range(T - 4, g['H'] - B + 4):
            img.set(8, y, hx('#10151b'))
            img.set(9, y, hx('#8798ab'))
            img.set(W - 10, y, hx('#10151b'))
            img.set(W - 9, y, hx('#8798ab'))

    # --- title trough (scanline LCD)
    tx0, ty0, tx1, ty1 = 8, 7, W - 9, 28
    sunken(img, tx0, ty0, tx1, ty1, hx('#0b0e12'), hx('#222932'), hx('#7a8a9b'), hx('#b7c7d8'))
    lcd_fill(img, tx0 + 2, ty0 + 2, tx1 - 2, ty1 - 2, LCD_G, darken(0.62), glare=hx('#1d4a2b'),
             grid=lighten((4, 14, 6)))
    # mascot in the trough
    for j, row in enumerate(LLAMA_ICON):
        for i, ch in enumerate(row):
            col = {'#': hx('#f4e8c8') if j < 8 else hx('#d9c79a'), 'e': hx('#2a1a0c'), 'n': hx('#e88aa8')}.get(ch)
            if col:
                img.set(14 + i, 12 + j, col)

    if kind == 'frame':
        # --- recess around the viewport
        recess_ring(img, vp[0], vp[1], vp[2], vp[3], LLP)
        # --- status trough on the bottom edge
        sy0, sy1 = 212, 224
        sx0, sx1 = 8, W - 18
        sunken(img, sx0, sy0, sx1, sy1, hx('#0b0e12'), hx('#222932'), hx('#7a8a9b'), hx('#b7c7d8'))
        lcd_fill(img, sx0 + 2, sy0 + 2, sx1 - 2, sy1 - 2, LCD_G, darken(0.62), glare=hx('#1d4a2b'),
                 grid=lighten((4, 14, 6)))
        # LED dots: dim every 8px, a lit one every 32px
        for x in range(sx0 + 4, sx1 - 3):
            if (x & 7) == 4:
                img.rect(x, 217, x + 1, 217, LED_ON)
                img.rect(x, 218, x + 1, 218, LED_DIM)
        # --- wing grip: diagonal ridges
        for y in range(g['H'] - 19, g['H'] - 2):
            for x in range(W - 19, W - 2):
                if not mask(x, y) or mask.dist(x, y) < 4:
                    continue
                u, v = W - 1 - x, g['H'] - 1 - y
                if u + v > 28 or u + v < 3:
                    continue
                m = (u + v) & 3
                if m == 0:
                    img.set(x, y, hx('#10151b'))
                elif m == 1:
                    img.set(x, y, hx('#a9bacd'))

    finish(img, mask, LLP)
    return img


def llama_button(kind, state, size=14):
    img = Img(size, size)
    face = {0: [hx('#4e5967'), hx('#3d4753'), hx('#303843')],
            1: [hx('#7a8a9b'), hx('#627080'), hx('#4e5967')],
            2: [hx('#1f252d'), hx('#252b34'), hx('#2c333d')]}[state]
    img.rect(0, 0, size - 1, size - 1, hx('#07090c'))
    if state == 2:
        bevel_rect(img, 1, 1, size - 2, size - 2, [hx('#0f1318'), hx('#1a2028')], [hx('#97a8ba'), hx('#627080')])
    else:
        bevel_rect(img, 1, 1, size - 2, size - 2, [hx('#e4eef8'), hx('#a3b6ca')], [hx('#0f1318'), hx('#222932')])
    for y in range(3, size - 3):
        t = (y - 3) / float(size - 7)
        for x in range(3, size - 3):
            img.set(x, y, dith(t, face, x, y))
    gcol = {0: hx('#d9e4f0'), 1: LED_ON, 2: hx('#2fae5c')}[state]
    off = 1 if state == 2 else 0
    rows = GLYPH8[kind]
    paint_glyph(img, rows, 3 + off, 3 + off, hx('#0a0d11') if state != 1 else hx('#06120a'))
    paint_glyph(img, rows, 3 + off, 3 + off, gcol)
    for (cx, cy) in ((0, 0), (size - 1, 0), (0, size - 1), (size - 1, size - 1)):
        img.set(cx, cy, (0, 0, 0, 0))
    return img


def build_llama():
    g = dict(LL)
    g.update(name='llama', titlebar=(0, LL['T']), text=(30, 12, -84), text_color=LCD_TXT, text_align='center',
             text_where='green LCD trough', dblclick='shade', btn_w=14, btn_h=14, btn_y=11,
             btn_x={'close': -26, 'minimize': -44, 'shade': -62, 'maximize': -80},
             grip=(-20, -20, 20, 20), resize_border=5, shade_h=LL['T'])
    frame = paint_llama('frame')
    shade = paint_llama('shade')
    buttons = make_button_sheet(14, llama_button)
    toml = toml_common('Llama', 'grimalkin', g, '#000000', None, None)
    prev = make_preview(compose_for(g, frame, buttons, 'GRIMALKIN - LLAMA', ['$ LLAMA', '> GRIMALKIN'],
                                    hx('#7ee09a'), hx(LCD_TXT)), g)
    write_skin('llama', toml, frame, buttons, shade, prev)
    return frame, buttons, shade, prev


# ==========================================================================
# GRIMALKIN: cat-shaped silhouette, ears above the title bar, amber eyes, tail grip
# ==========================================================================
GR = dict(W=320, H=254, L=28, R=28, T=54, B=40)
GR_BODY_TOP = 22          # ears rise from here
GR_BOTTOM = GR['H'] - 17  # last body row (tail hangs below)

FUR = [hx(c) for c in ('#120e18', '#1a1522', '#231c2f', '#2f2540', '#3f3256', '#53437a', '#6c58a0')]
GRP = dict(out=hx('#05030a'), hi2=hx('#d3c0f7'), hi3=hx('#8f78c8'), sh2=hx('#09070f'), sh3=hx('#1a1422'),
           rec_tl0=hx('#07050c'), rec_tl1=hx('#1a1422'), rec_br0=hx('#53437a'), rec_br1=hx('#9a82d0'))
LCD_A = [hx(c) for c in ('#120802', '#1f1003', '#2e1805', '#402108')]
AMBER_TXT = '#ffb000'
AMBER = hx('#ffb000')
AMBER_DIM = hx('#5a3a08')
EAR_INNER = [hx(c) for c in ('#3a2350', '#5a3470', '#86508f', '#b0688f')]

CAT_ICON = [
    '#.......#',
    '##.....##',
    '#########',
    '#ee###ee#',
    '#########',
    '####n####',
    '.#######.',
]

EAR_L = [(0, GR_BODY_TOP + 1), (11, 0), (28, GR_BODY_TOP + 1)]
EAR_R = [(W - x, y) for (x, y) in EAR_L]
EAR_IN_L = [(4.0, GR_BODY_TOP + 1), (11.5, 7.0), (23.0, GR_BODY_TOP + 1)]
EAR_IN_R = [(W - x, y) for (x, y) in EAR_IN_L]


def tail_path():
    """Centre line samples of the curled tail: (x, y, s)."""
    pts = []
    s = 0.0
    root_x = W - 5
    y = GR_BOTTOM - 9.0
    while y <= GR_BOTTOM + 3.0:
        pts.append((root_x, y, s))
        y += 0.25
        s += 0.25
    cx, cy, r = W - 14.5, GR_BOTTOM + 2.0, 9.5
    t = 0.0
    while t <= 140.0:
        a = math.radians(t)
        pts.append((cx + r * math.cos(a), cy + r * math.sin(a), s))
        t += 1.0
        s += r * math.radians(1.0)
    return pts


TAIL = tail_path()
TAIL_R = 4.2


def tail_pixels():
    px = {}
    for (x, y, s) in TAIL:
        for yy in range(int(y - TAIL_R - 1), int(y + TAIL_R + 2)):
            for xx in range(int(x - TAIL_R - 1), int(x + TAIL_R + 2)):
                if math.hypot(xx + 0.5 - x, yy + 0.5 - y) <= TAIL_R:
                    if yy > GR_BOTTOM - 4:
                        px[(xx, yy)] = s
    return px


TAIL_PX = tail_pixels()
TAIL_END = max(TAIL, key=lambda p: p[2])[2]


def grim_mask(kind):
    H = GR['H'] if kind == 'frame' else GR['T']
    bottom = GR_BOTTOM if kind == 'frame' else H - 1
    r = 10 if kind == 'frame' else 5

    def fn(x, y):
        px, py = x + 0.5, y + 0.5
        if y < GR_BODY_TOP:
            return in_poly(px, py, EAR_L) or in_poly(px, py, EAR_R)
        if y <= bottom:
            if rrect_dist(px, py, 0, GR_BODY_TOP - 40, W, bottom + 1, r) <= 0:
                return True
            return kind == 'frame' and x >= W - 12   # square corner: the tail root continues the wall
        if kind == 'frame' and (x, y) in TAIL_PX:
            return True
        return False
    return Mask(W, H, fn)


def grim_profile(d):
    pr = [0, 0.1, 0.2, 0.3, 0.72, 0.9, 0.9, 0.84, 0.76, 0.68, 0.6, 0.52, 0.46, 0.42]
    return pr[d] if d < len(pr) else 0.38


def paint_grim(kind):
    g = GR
    L, R, T, B = g['L'], g['R'], g['T'], g['B']
    H = g['H'] if kind == 'frame' else T
    mask = grim_mask(kind)
    img = Img(W, H)
    dl, du, dr, dd = mask.depths()
    vp = (L, T, W - R - 1, g['H'] - B - 1)
    for y in range(H):
        for x in range(W):
            if not mask.m[y][x]:
                continue
            if kind == 'frame' and vp[0] <= x <= vp[2] and vp[1] <= y <= vp[3]:
                img.set(x, y, hx('#08060c'))
                continue
            d = min(dl[y][x], du[y][x], dr[y][x], dd[y][x])
            f = FLECK8[y & 7][x & 7]
            t = grim_profile(d) + (f - 0.5) * 0.1
            if f > 0.86:
                t += 0.14
            elif f < 0.1:
                t -= 0.1
            img.set(x, y, dith(t, FUR, x, y))

    # inner ears
    for (poly, y0) in ((EAR_IN_L, 0), (EAR_IN_R, 0)):
        for y in range(0, GR_BODY_TOP + 2):
            for x in range(W):
                if mask(x, y) and mask.dist(x, y) >= 3 and in_poly(x + 0.5, y + 0.5, poly):
                    t = 1.0 - (y / float(GR_BODY_TOP + 1))
                    img.set(x, y, dith(0.15 + 0.8 * t, EAR_INNER, x, y))

    # tabby stripes on the walls (tile-periodic: depend on (y-T) mod 8 only)
    if kind == 'frame':
        for y in range(T - 6, g['H'] - B + 6):
            m = (y - T) & 7
            ln = {0: 9, 1: 6, 2: 3}.get(m, 0)
            if ln:
                for k in range(ln):
                    if mask(4 + k, y) and mask.dist(4 + k, y) >= 4:
                        img.set(4 + k, y, hx('#0e0a14') if m < 2 else hx('#1c1428'))
                    if mask(W - 5 - k, y) and mask.dist(W - 5 - k, y) >= 4:
                        img.set(W - 5 - k, y, hx('#0e0a14') if m < 2 else hx('#1c1428'))
        # lighter whisker-fur ticks between the stripes
        for y in range(T - 6, g['H'] - B + 6):
            m = (y - T) & 7
            if m == 5:
                for k in range(3):
                    img.set(14 + k, y, hx('#6c58a0'))
                    img.set(W - 17 + k, y, hx('#6c58a0'))

    # --- title trough (amber scanline LCD)
    tx0, ty0, tx1, ty1 = 8, GR_BODY_TOP + 4, W - 9, GR_BODY_TOP + 27
    sunken(img, tx0, ty0, tx1, ty1, hx('#07050c'), hx('#1a1422'), hx('#53437a'), hx('#9a82d0'))
    lcd_fill(img, tx0 + 2, ty0 + 2, tx1 - 2, ty1 - 2, LCD_A, darken(0.6), glare=hx('#5a3008'),
             grid=lighten((14, 6, 0)))
    for j, row in enumerate(CAT_ICON):
        for i, ch in enumerate(row):
            col = {'#': AMBER, 'n': hx('#ff7a8a'), 'e': hx('#2a1604')}.get(ch)
            if col:
                img.set(13 + i, GR_BODY_TOP + 11 + j, col)

    if kind == 'frame':
        recess_ring(img, vp[0], vp[1], vp[2], vp[3], GRP)
        sy0, sy1 = g['H'] - B + 4, g['H'] - B + 17
        sx0, sx1 = 8, W - 20
        sunken(img, sx0, sy0, sx1, sy1, hx('#07050c'), hx('#1a1422'), hx('#53437a'), hx('#9a82d0'))
        lcd_fill(img, sx0 + 2, sy0 + 2, sx1 - 2, sy1 - 2, LCD_A, darken(0.6), glare=hx('#5a3008'),
                 grid=lighten((14, 6, 0)))
        for x in range(sx0 + 4, sx1 - 3):
            if (x & 7) == 4:
                img.rect(x, sy0 + 5, x + 1, sy0 + 5, AMBER)
                img.rect(x, sy0 + 6, x + 1, sy0 + 6, AMBER_DIM)
        # tail: tabby rings, no bevel (outline only)
        nob = set()
        for (xy, s) in TAIL_PX.items():
            if not mask(*xy):
                continue
            ring = int(s / 5.0) & 1
            tip = s > TAIL_END - 3.0
            if tip:
                col = hx('#c8b0f0')
            else:
                col = hx('#8a74c4') if ring == 0 else hx('#3f3256')
            img.set(xy[0], xy[1], col)
            nob.add(xy)
        # light edge on the upper-left of the tail
        for (x, y) in list(nob):
            if mask.dist(x, y) == 2 and (not mask(x - 1, y - 1) or not mask(x - 1, y) or not mask(x, y - 1)):
                pass
    else:
        nob = set()

    finish(img, mask, GRP, nob)
    return img


def eye_img(state, size=16):
    img = Img(size, size)
    c = (size - 1) / 2.0
    ramp = [hx('#c25a00'), hx('#ff9100'), hx('#ffb800'), hx('#ffdc5a')]
    sock = [hx('#120a1a'), hx('#1e1428'), hx('#2b1d3a')]
    for y in range(size):
        for x in range(size):
            d = math.hypot(x - c, y - c)
            if d <= c + 0.2:
                img.set(x, y, hx('#05030a'))
    for y in range(size):
        for x in range(size):
            d = math.hypot(x - c, y - c)
            if d > c - 0.9:
                continue
            if state == 1:   # blink: closed lid on a dark socket
                img.set(x, y, dith(0.2 + 0.6 * (y / float(size)), sock, x, y))
            else:
                t = 1.0 - (y / float(size - 1)) * 0.9
                col = dith(t, ramp, x, y)
                if d > c - 1.9:
                    col = ramp[0] if y > c else ramp[1]
                img.set(x, y, col)
    if state == 0 or state == 2:
        # vertical slit pupil
        for y in range(size):
            for x in range(size):
                if ((x + 0.5 - (c + 0.5)) / 1.5) ** 2 + ((y + 0.5 - (c + 0.5)) / 5.3) ** 2 <= 1.0:
                    img.set(x, y, hx('#160a00'))
        img.set(4, 4, hx('#fff2b8'))
        img.set(5, 4, hx('#fff2b8'))
        img.set(4, 5, hx('#fff2b8'))
    if state == 2:
        # pressed: upper half squeezed shut
        for y in range(0, 7):
            for x in range(size):
                d = math.hypot(x - c, y - c)
                if d <= c - 0.9:
                    img.set(x, y, dith(0.3 + 0.4 * (y / 7.0), sock, x, y))
        for x in range(3, 13):
            img.set(x, 7, hx('#e07800'))
    if state == 1:
        # closed eye: smile-shaped lid line with two lashes
        for x in range(3, 13):
            u = (x - c) / 5.0
            y = 6 + int(round(2.2 * (1 - u * u)))
            img.set(x, y, AMBER)
            img.set(x, y + 1, hx('#a85e00'))
        img.set(2, 6, AMBER)
        img.set(13, 6, AMBER)
    return img


def grim_button(kind, state, size=16):
    if kind in ('close', 'minimize'):
        return eye_img(state, size)
    img = Img(size, size)
    face = {0: [hx('#43355a'), hx('#53437a'), hx('#6c58a0')],
            1: [hx('#6c58a0'), hx('#8a74c4'), hx('#a890e0')],
            2: [hx('#1a1422'), hx('#231c2f'), hx('#2f2540')]}[state]
    img.rect(0, 0, size - 1, size - 1, hx('#05030a'))
    if state == 2:
        bevel_rect(img, 1, 1, size - 2, size - 2, [hx('#09070f'), hx('#1a1422')], [hx('#8f78c8'), hx('#53437a')])
    else:
        bevel_rect(img, 1, 1, size - 2, size - 2, [hx('#d3c0f7'), hx('#8f78c8')], [hx('#09070f'), hx('#1a1422')])
    for y in range(3, size - 3):
        t = (y - 3) / float(size - 7)
        for x in range(3, size - 3):
            img.set(x, y, dith(t, face, x, y))
    off = 1 if state == 2 else 0
    gcol = {0: hx('#ffcf5a'), 1: hx('#fff0b0'), 2: hx('#c88a10')}[state]
    paint_glyph(img, GLYPH8[kind], 4 + off, 4 + off, gcol)
    for (cx, cy) in ((0, 0), (size - 1, 0), (0, size - 1), (size - 1, size - 1)):
        img.set(cx, cy, (0, 0, 0, 0))
    return img


def build_grimalkin():
    g = dict(GR)
    g.update(name='grimalkin', titlebar=(GR_BODY_TOP, GR['T'] - GR_BODY_TOP), text=(32, GR_BODY_TOP + 10, -94),
             text_color=AMBER_TXT, text_align='center', text_where='amber LCD trough', dblclick='shade',
             btn_w=16, btn_h=16, btn_y=GR_BODY_TOP + 8,
             btn_x={'close': -28, 'minimize': -48, 'shade': -68, 'maximize': -88},
             grip=(-28, -34, 28, 34), resize_border=5, shade_h=GR['T'])
    frame = paint_grim('frame')
    shade = paint_grim('shade')
    buttons = make_button_sheet(16, grim_button)
    toml = toml_common('Grimalkin', 'grimalkin', g, '#08060c', None, None)
    prev = make_preview(compose_for(g, frame, buttons, 'GRIMALKIN', ['$ GRIMALKIN', '> MEOW'],
                                    hx('#ffb000'), hx(AMBER_TXT)), g)
    write_skin('grimalkin', toml, frame, buttons, shade, prev)
    return frame, buttons, shade, prev


# ==========================================================================
# MINT: pastel 1998, 12px rounded corners, baked drop shadow, chrome title bar
# ==========================================================================
MT = dict(W=320, H=244, L=16, R=24, T=28, B=24)
MT_BODY_W = 312      # body spans x 0..311, the last 8 columns are shadow
MT_BODY_H = 236
MT_R = 12

MINT = [hx(c) for c in ('#6fcba3', '#86d8b2', '#9ce3c4', '#b1ecd3', '#c6f4e1', '#dbfaee')]
MTP = dict(out=hx('#1f5446'), hi2=hx('#ffffff'), hi3=hx('#e6fcf0'), sh2=hx('#4a9c80'), sh3=hx('#86cfb0'),
           rec_tl0=hx('#3f8a73'), rec_tl1=hx('#7dc4a8'), rec_br0=hx('#f4fffa'), rec_br1=hx('#ffffff'))
MT_SHADOW = hx('#16322a')
CHROME_TOP = [hx(c) for c in ('#ffffff', '#f3fffa', '#e2f8ee')]
CHROME_BOT = [hx(c) for c in ('#9fd6c0', '#b7e4d2', '#d2f2e5')]
MT_TEXT = '#1f5446'


def mint_body_dist(x, y, h_body):
    return rrect_dist(x + 0.5, y + 0.5, 0, 0, MT_BODY_W, h_body, MT_R)


def mint_mask(kind):
    h_body = MT_BODY_H if kind == 'frame' else MT['T'] - 2
    H = MT['H'] if kind == 'frame' else MT['T']
    return Mask(W, H, lambda x, y: mint_body_dist(x, y, h_body) <= 0), h_body, H


def mint_shadow(img, kind, h_body):
    H = img.h
    sh_h = (h_body + 8) if kind == 'frame' else H + 8
    for y in range(H):
        for x in range(W):
            if img.get(x, y)[3]:
                continue
            s = mint_body_dist(x, y, h_body)
            if s <= 0 or s > 8.6:
                continue
            # shadow silhouette: body shifted by (+4,+4) with 8px rounded corners
            if rrect_dist(x + 0.5, y + 0.5, 4, 4, W, sh_h, 8) > 0:
                continue
            if s <= 3.6:
                a = True
            elif s <= 5.6:
                a = ((x + y) & 1) == 0
            elif s <= 7.6:
                a = (x & 1) == 0 and (y & 1) == 0
            else:
                a = False
            if a:
                img.set(x, y, (MT_SHADOW[0], MT_SHADOW[1], MT_SHADOW[2], 200 if s <= 3.6 else 180))


def mint_profile(d):
    pr = [0, 0.1, 0.2, 0.3, 0.95, 0.9, 0.8, 0.7, 0.62, 0.55, 0.5, 0.48]
    return pr[d] if d < len(pr) else 0.46


def peppermint(img, ox, oy):
    """7x7 peppermint candy."""
    for y in range(7):
        for x in range(7):
            dx, dy = x - 3, y - 3
            d = math.hypot(dx, dy)
            if d <= 3.5:
                ang = math.atan2(dy, dx) + math.pi
                sector = int(ang / (math.pi / 3.0)) % 2
                img.set(ox + x, oy + y, hx('#ff9fc4') if sector else hx('#ffffff'))
            if 2.6 < d <= 3.7:
                img.set(ox + x, oy + y, hx('#d9608f'))
    img.set(ox + 3, oy + 3, hx('#ffffff'))


def paint_mint(kind):
    g = MT
    L, R, T, B = g['L'], g['R'], g['T'], g['B']
    mask, h_body, H = mint_mask(kind)
    img = Img(W, H)
    dl, du, dr, dd = mask.depths()
    vp = (L, T, W - R - 1, g['H'] - B - 1)
    for y in range(H):
        for x in range(W):
            if not mask.m[y][x]:
                continue
            if kind == 'frame' and vp[0] <= x <= vp[2] and vp[1] <= y <= vp[3]:
                img.set(x, y, hx('#0f2a24'))
                continue
            d = min(dl[y][x], du[y][x], dr[y][x], dd[y][x])
            t = mint_profile(d)
            c = dith(t, MINT, x, y)
            # polka dots (period 8 in both axes)
            m, n = x & 7, y & 7
            if (m in (2, 3) and n in (2, 3)) or (m in (6, 7) and n in (6, 7)):
                c = hx('#7ed6ac')
                if (m, n) in ((2, 2), (6, 6)):
                    c = hx('#ffffff')
            img.set(x, y, c)

    # --- chrome title plate
    px0, py0, px1, py1 = 5, 4, W - 15, 22
    img.rect(px0, py0, px1, py1, hx('#2f7360'))
    img.rect(px0 + 1, py0 + 1, px1 - 1, py1 - 1, hx('#ffffff'))
    hor = py0 + 1 + int((py1 - py0 - 1) * 0.55)
    for y in range(py0 + 2, py1 - 1):
        for x in range(px0 + 2, px1 - 1):
            if y < hor:
                t = (y - (py0 + 2)) / float(hor - py0 - 2)
                c = dith(t * 0.9, CHROME_TOP, x, y)
            else:
                t = (y - hor) / float(py1 - 2 - hor)
                c = dith(0.1 + t * 0.9, CHROME_BOT, x, y)
                if y == hor:
                    c = hx('#7dbfa6')
            # fine vertical pinstripes, period 4
            if (x & 3) == 0 and y != hor:
                c = (max(0, c[0] - 10), max(0, c[1] - 6), max(0, c[2] - 8), 255)
            img.set(x, y, c)
    # shaded bottom edge of the plate
    img.hline(px0 + 1, px1 - 1, py1 - 1, hx('#5fae92'))
    img.vline(px1 - 1, py0 + 1, py1 - 1, hx('#7dc4a8'))
    # trim the plate's corners
    peppermint(img, 8, 10)

    if kind == 'frame':
        recess_ring(img, vp[0], vp[1], vp[2], vp[3], MTP)
        # lilac pinstripe on the walls and bottom, inside the border
        for y in range(T - 4, g['H'] - B + 5):
            img.set(6, y, hx('#c7b6f2'))
            img.set(W - 15, y, hx('#c7b6f2'))
        # grip: pastel dotted diagonal ridges in the bottom-right of the body
        for y in range(g['H'] - 24, g['H'] - 9):
            for x in range(W - 24, W - 9):
                if not mask(x, y) or mask.dist(x, y) < 4:
                    continue
                u, v = (W - 9) - x, (g['H'] - 9) - y
                if u + v > 22 or u + v < 3:
                    continue
                m = (u + v) & 3
                if m == 0:
                    img.set(x, y, hx('#4a9c80'))
                elif m == 1:
                    img.set(x, y, hx('#ffffff'))
        # status pinstripe: a thin lilac groove along the bottom border
        for x in range(6, W - 14):
            img.set(x, g['H'] - 9 - 6, hx('#c7b6f2'))

    finish(img, mask, MTP)
    mint_shadow(img, kind, h_body)
    return img


def mint_button(kind, state, size=12):
    img = Img(size, size)
    img.rect(0, 0, size - 1, size - 1, hx('#2f7360'))
    if state == 2:
        top = [hx('#7dc4a8'), hx('#95d3ba'), hx('#a9dcc6')]
        bot = [hx('#a9dcc6'), hx('#bfe8d6'), hx('#cdeee0')]
        bevel_rect(img, 1, 1, size - 2, size - 2, [hx('#4a9c80'), hx('#7dc4a8')], [hx('#ffffff'), hx('#e6fcf0')])
    else:
        if state == 1:
            top = [hx('#ffffff'), hx('#fff0f7'), hx('#ffdcec')]
            bot = [hx('#ffb8d6'), hx('#ffcce2'), hx('#ffe0ee')]
        else:
            top = CHROME_TOP
            bot = CHROME_BOT
        bevel_rect(img, 1, 1, size - 2, size - 2, [hx('#ffffff'), hx('#e6fcf0')], [hx('#4a9c80'), hx('#86cfb0')])
    for y in range(3, size - 3):
        t = (y - 3) / float(size - 7)
        for x in range(3, size - 3):
            img.set(x, y, dith(t * 0.9, top, x, y) if t < 0.5 else dith(0.1 + t * 0.9, bot, x, y))
    off = 1 if state == 2 else 0
    gcol = hx('#1f5446') if state != 1 else hx('#a0285f')
    paint_glyph(img, GLYPH6[kind], 3 + off, 3 + off, gcol)
    for (cx, cy) in ((0, 0), (size - 1, 0), (0, size - 1), (size - 1, size - 1)):
        img.set(cx, cy, (0, 0, 0, 0))
    return img


def build_mint():
    g = dict(MT)
    g.update(name='mint', titlebar=(0, MT['T']), text=(26, 9, -90), text_color=MT_TEXT, text_align='left',
             text_where='chrome title plate', dblclick='maximize', btn_w=12, btn_h=12, btn_y=7,
             btn_x={'close': -34, 'minimize': -50, 'shade': -66, 'maximize': -82},
             grip=(-24, -24, 16, 16), resize_border=5, shade_h=MT['T'])
    frame = paint_mint('frame')
    shade = paint_mint('shade')
    buttons = make_button_sheet(12, mint_button)
    toml = toml_common('Mint', 'grimalkin', g, '#0f2a24', None, None)
    prev = make_preview(compose_for(g, frame, buttons, 'GRIMALKIN - MINT', ['C:\\> GRIMALKIN', 'C:\\> _'],
                                    hx('#8fe8bc'), hx(MT_TEXT)), g)
    write_skin('mint', toml, frame, buttons, shade, prev)
    return frame, buttons, shade, prev


if __name__ == '__main__':
    which = sys.argv[1:] or ['llama', 'grimalkin', 'mint']
    for n in which:
        globals()['build_' + n]()
        print('wrote', n)
