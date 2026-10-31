//! Pixel-perfect block elements (U+2580-259F) and light/heavy box drawing
//! (U+2500-257F).
//!
//! Drawing these as rectangles instead of font glyphs keeps lines joined across
//! cells whatever the font or `line_height`. Dashed lines are drawn as
//! separate dashes and rounded corners (U+256D-2570) as anti-aliased quarter
//! arcs built from per-pixel alpha rects. Double lines and diagonals (U+2550-256C,
//! U+2571-2573) return `false` from [`draw`] and fall back to the font.

use crate::renderer::{rect, RectInst};

type Rgb = (u8, u8, u8);

/// Draw `ch` into the cell at pixel position `(x, y)` of size `cw` x `chh`.
/// Returns true if the character was handled.
pub fn draw(ch: char, x: f32, y: f32, cw: f32, chh: f32, color: Rgb, out: &mut Vec<RectInst>) -> bool {
    let code = ch as u32;
    if !(0x2500..=0x259F).contains(&code) {
        return false;
    }
    let (w, h) = (cw as i32, chh as i32);
    let mut push = |rx: i32, ry: i32, rw: i32, rh: i32, a: u8| {
        if rw > 0 && rh > 0 {
            out.push(rect(x + rx as f32, y + ry as f32, rw as f32, rh as f32, color, a, 0));
        }
    };
    if code >= 0x2580 {
        let eighth_h = |n: i32| (h * n + 4) / 8;
        let eighth_w = |n: i32| (w * n + 4) / 8;
        match code {
            0x2580 => push(0, 0, w, eighth_h(4), 255),
            0x2581..=0x2587 => {
                let n = (code - 0x2580) as i32;
                let bh = eighth_h(n);
                push(0, h - bh, w, bh, 255);
            }
            0x2588 => push(0, 0, w, h, 255),
            0x2589..=0x258F => {
                let n = (0x2590 - code) as i32;
                push(0, 0, eighth_w(n), h, 255);
            }
            0x2590 => {
                let bw = eighth_w(4);
                push(w - bw, 0, bw, h, 255);
            }
            0x2591 => push(0, 0, w, h, 64),
            0x2592 => push(0, 0, w, h, 128),
            0x2593 => push(0, 0, w, h, 192),
            0x2594 => push(0, 0, w, eighth_h(1).max(1), 255),
            0x2595 => {
                let bw = eighth_w(1).max(1);
                push(w - bw, 0, bw, h, 255);
            }
            0x2596..=0x259F => {
                // Quadrant bits: UL, UR, LL, LR.
                const Q: [u8; 10] = [
                    0b0010, // 2596 lower left
                    0b0001, // 2597 lower right
                    0b1000, // 2598 upper left
                    0b1011, // 2599 UL + LL + LR
                    0b1001, // 259A UL + LR
                    0b1110, // 259B UL + UR + LL
                    0b1101, // 259C UL + UR + LR
                    0b0100, // 259D upper right
                    0b0110, // 259E UR + LL
                    0b0111, // 259F UR + LL + LR
                ];
                let bits = Q[(code - 0x2596) as usize];
                let (hw, hh) = (w / 2, h / 2);
                if bits & 0b1000 != 0 {
                    push(0, 0, hw, hh, 255);
                }
                if bits & 0b0100 != 0 {
                    push(hw, 0, w - hw, hh, 255);
                }
                if bits & 0b0010 != 0 {
                    push(0, hh, hw, h - hh, 255);
                }
                if bits & 0b0001 != 0 {
                    push(hw, hh, w - hw, h - hh, 255);
                }
            }
            _ => return false,
        }
        return true;
    }

    let light = ((w as f32 / 8.0).round() as i32).max(1);
    // Rounded corners: quarter ellipse centred on the cell corner opposite the
    // two arms, passing through the midpoints of the two arm edges.
    if (0x256D..=0x2570).contains(&code) {
        let (right, down) = match code {
            0x256D => (true, true),
            0x256E => (false, true),
            0x256F => (false, false),
            _ => (true, false),
        };
        let (rx, ry) = (w as f32 / 2.0, h as f32 / 2.0);
        let (cx, cy) = (if right { w as f32 } else { 0.0 }, if down { h as f32 } else { 0.0 });
        const N: i32 = 4;
        for py in 0..h {
            for px in 0..w {
                let mut hit = 0;
                for sy in 0..N {
                    for sx in 0..N {
                        let dx = px as f32 + (sx as f32 + 0.5) / N as f32 - cx;
                        let dy = py as f32 + (sy as f32 + 0.5) / N as f32 - cy;
                        let f = ((dx / rx).powi(2) + (dy / ry).powi(2)).sqrt();
                        if f < 1e-6 {
                            continue;
                        }
                        let g = ((dx / (rx * rx)).powi(2) + (dy / (ry * ry)).powi(2)).sqrt() / f;
                        if ((f - 1.0) / g).abs() <= light as f32 / 2.0 {
                            hit += 1;
                        }
                    }
                }
                if hit > 0 {
                    push(px, py, 1, 1, (hit * 255 / (N * N)) as u8);
                }
            }
        }
        return true;
    }
    // Dashed lines: (horizontal, segments, heavy).
    let dash = match code {
        0x2504..=0x250B => {
            let i = code - 0x2504;
            Some((i % 4 < 2, if i < 4 { 3 } else { 4 }, i % 2 == 1))
        }
        0x254C..=0x254F => {
            let i = code - 0x254C;
            Some((i < 2, 2, i % 2 == 1))
        }
        _ => None,
    };
    if let Some((horiz, n, heavy)) = dash {
        let t = if heavy { light * 2 } else { light };
        let len = if horiz { w } else { h };
        let cross = if horiz { h } else { w };
        let off = (cross - t) / 2;
        let period = len as f32 / n as f32;
        let gap = (period / 3.0).max(1.0);
        for i in 0..n {
            let a = (period * i as f32 + gap / 2.0).round() as i32;
            let b = (period * (i + 1) as f32 - gap / 2.0).round() as i32;
            if horiz {
                push(a, off, b - a, t, 255);
            } else {
                push(off, a, t, b - a, 255);
            }
        }
        return true;
    }

    // Box drawing: (left, right, up, down) weights, 0 none, 1 light, 2 heavy.
    let (l, r, u, d): (u8, u8, u8, u8) = match code {
        0x2500 | 0x2504 | 0x2508 | 0x254C => (1, 1, 0, 0),
        0x2501 | 0x2505 | 0x2509 | 0x254D => (2, 2, 0, 0),
        0x2502 | 0x2506 | 0x250A | 0x254E => (0, 0, 1, 1),
        0x2503 | 0x2507 | 0x250B | 0x254F => (0, 0, 2, 2),
        0x250C | 0x256D => (0, 1, 0, 1),
        0x250D => (0, 2, 0, 1),
        0x250E => (0, 1, 0, 2),
        0x250F => (0, 2, 0, 2),
        0x2510 | 0x256E => (1, 0, 0, 1),
        0x2511 => (2, 0, 0, 1),
        0x2512 => (1, 0, 0, 2),
        0x2513 => (2, 0, 0, 2),
        0x2514 | 0x2570 => (0, 1, 1, 0),
        0x2515 => (0, 2, 1, 0),
        0x2516 => (0, 1, 2, 0),
        0x2517 => (0, 2, 2, 0),
        0x2518 | 0x256F => (1, 0, 1, 0),
        0x2519 => (2, 0, 1, 0),
        0x251A => (1, 0, 2, 0),
        0x251B => (2, 0, 2, 0),
        0x251C => (0, 1, 1, 1),
        0x251D => (0, 2, 1, 1),
        0x251E => (0, 1, 2, 1),
        0x251F => (0, 1, 1, 2),
        0x2520 => (0, 1, 2, 2),
        0x2521 => (0, 2, 2, 1),
        0x2522 => (0, 2, 1, 2),
        0x2523 => (0, 2, 2, 2),
        0x2524 => (1, 0, 1, 1),
        0x2525 => (2, 0, 1, 1),
        0x2526 => (1, 0, 2, 1),
        0x2527 => (1, 0, 1, 2),
        0x2528 => (1, 0, 2, 2),
        0x2529 => (2, 0, 2, 1),
        0x252A => (2, 0, 1, 2),
        0x252B => (2, 0, 2, 2),
        0x252C => (1, 1, 0, 1),
        0x252D => (2, 1, 0, 1),
        0x252E => (1, 2, 0, 1),
        0x252F => (2, 2, 0, 1),
        0x2530 => (1, 1, 0, 2),
        0x2531 => (2, 1, 0, 2),
        0x2532 => (1, 2, 0, 2),
        0x2533 => (2, 2, 0, 2),
        0x2534 => (1, 1, 1, 0),
        0x2535 => (2, 1, 1, 0),
        0x2536 => (1, 2, 1, 0),
        0x2537 => (2, 2, 1, 0),
        0x2538 => (1, 1, 2, 0),
        0x2539 => (2, 1, 2, 0),
        0x253A => (1, 2, 2, 0),
        0x253B => (2, 2, 2, 0),
        0x253C => (1, 1, 1, 1),
        0x253D => (2, 1, 1, 1),
        0x253E => (1, 2, 1, 1),
        0x253F => (2, 2, 1, 1),
        0x2540 => (1, 1, 2, 1),
        0x2541 => (1, 1, 1, 2),
        0x2542 => (1, 1, 2, 2),
        0x2543 => (2, 1, 2, 1),
        0x2544 => (1, 2, 2, 1),
        0x2545 => (2, 1, 1, 2),
        0x2546 => (1, 2, 1, 2),
        0x2547 => (2, 2, 2, 1),
        0x2548 => (2, 2, 1, 2),
        0x2549 => (2, 1, 2, 2),
        0x254A => (1, 2, 2, 2),
        0x254B => (2, 2, 2, 2),
        0x2574 => (1, 0, 0, 0),
        0x2575 => (0, 0, 1, 0),
        0x2576 => (0, 1, 0, 0),
        0x2577 => (0, 0, 0, 1),
        0x2578 => (2, 0, 0, 0),
        0x2579 => (0, 0, 2, 0),
        0x257A => (0, 2, 0, 0),
        0x257B => (0, 0, 0, 2),
        0x257C => (1, 2, 0, 0),
        0x257D => (0, 0, 1, 2),
        0x257E => (2, 1, 0, 0),
        0x257F => (0, 0, 2, 1),
        _ => return false,
    };
    let th = |wt: u8| match wt {
        1 => light,
        2 => light * 2,
        _ => 0,
    };
    let hth = th(l).max(th(r)); // thickness of the horizontal band
    let vth = th(u).max(th(d)); // thickness of the vertical band
    let vx = (w - vth) / 2; // left edge of the vertical band
    let hy = (h - hth) / 2; // top edge of the horizontal band
    // Horizontal arms.
    if l > 0 {
        let t = th(l);
        let end = if vth > 0 { vx + vth } else { w / 2 };
        push(0, hy + (hth - t) / 2, end, t, 255);
    }
    if r > 0 {
        let t = th(r);
        let start = if vth > 0 { vx } else { w / 2 };
        push(start, hy + (hth - t) / 2, w - start, t, 255);
    }
    // Vertical arms.
    if u > 0 {
        let t = th(u);
        let end = if hth > 0 { hy + hth } else { h / 2 };
        push(vx + (vth - t) / 2, 0, t, end, 255);
    }
    if d > 0 {
        let t = th(d);
        let start = if hth > 0 { hy } else { h / 2 };
        push(vx + (vth - t) / 2, start, t, h - start, 255);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: usize = 16;
    const H: usize = 32;

    fn raster(ch: char) -> Vec<u8> {
        let mut out = Vec::new();
        assert!(draw(ch, 0.0, 0.0, W as f32, H as f32, (255, 255, 255), &mut out));
        let mut buf = vec![0u8; W * H];
        for r in out {
            let j = rect_parts(&r);
            for y in j.1..j.1 + j.3 {
                for x in j.0..j.0 + j.2 {
                    let p = &mut buf[y * W + x];
                    *p = (*p).max(j.4);
                }
            }
        }
        buf
    }

    // RectInst fields are private to renderer; read them through bytes.
    fn rect_parts(r: &RectInst) -> (usize, usize, usize, usize, u8) {
        let b: &[u8] = bytemuck::bytes_of(r);
        let f = |i: usize| f32::from_ne_bytes(b[i * 4..i * 4 + 4].try_into().unwrap());
        (f(0) as usize, f(1) as usize, f(2) as usize, f(3) as usize, b[16 + 3])
    }

    #[test]
    fn rounded_corners_are_arcs() {
        // (char, corner pixel, arc midpoint) ; midpoint at 45 degrees of the ellipse.
        let m = |right: bool, down: bool| {
            let k = std::f32::consts::FRAC_1_SQRT_2;
            let x = if right { W as f32 - W as f32 / 2.0 * k } else { W as f32 / 2.0 * k };
            let y = if down { H as f32 - H as f32 / 2.0 * k } else { H as f32 / 2.0 * k };
            (x as usize, y as usize)
        };
        for (ch, right, down, corner) in [
            ('\u{256D}', true, true, (0, 0)),
            ('\u{256E}', false, true, (W - 1, 0)),
            ('\u{256F}', false, false, (W - 1, H - 1)),
            ('\u{2570}', true, false, (0, H - 1)),
        ] {
            let b = raster(ch);
            // Corner of the cell opposite the arc centre is empty.
            assert_eq!(b[corner.1 * W + corner.0], 0, "{ch} corner");
            let (x, y) = m(right, down);
            assert!(b[y * W + x] > 128, "{ch} midpoint {}", b[y * W + x]);
        }
    }

    #[test]
    fn dashes_have_gaps() {
        let b = raster('\u{2504}');
        let row: Vec<u8> = (0..W).map(|x| b[(H / 2) * W + x]).collect();
        assert!(row.contains(&0) && row.contains(&255));
        let v = raster('\u{254E}');
        let col: Vec<u8> = (0..H).map(|y| v[y * W + W / 2]).collect();
        assert!(col.contains(&0) && col.contains(&255));
    }
}
