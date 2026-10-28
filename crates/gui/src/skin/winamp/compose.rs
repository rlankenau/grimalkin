//! Slice `pledit.bmp` into sprites and compose the images the skin engine
//! needs: the 9-slice frame, the windowshade bar and the button sheet
//! (docs/WINAMP_SKINS.md sections 2 and 3).

use super::super::parse::{
    Align, ButtonDef, ButtonsDef, DoubleClick, EdgeMode, FrameDef, Insets, RectDef, ResizeDef,
    ShadeDef, SkinDef, TitleImageDef, TitlebarDef,
};
use super::super::Image;
use crate::config::Rgb;

/// A sprite rectangle in the top-left-origin pledit image.
#[derive(Clone, Copy, Debug)]
pub struct R {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

const fn r(x: u32, y: u32, w: u32, h: u32) -> R {
    R { x, y, w, h }
}

// Focused ("selected") top row.
const TOP_LEFT: R = r(0, 0, 25, 20);
const TITLE: R = r(26, 0, 100, 20);
const TOP_TILE: R = r(127, 0, 25, 20);
const TOP_RIGHT: R = r(153, 0, 25, 20);
const LEFT_TILE: R = r(0, 42, 12, 29);
const RIGHT_TILE: R = r(31, 42, 20, 29);
const BOTTOM_LEFT: R = r(0, 72, 125, 38);
const BOTTOM_TILE: R = r(179, 0, 25, 38);
const BOTTOM_RIGHT: R = r(126, 72, 150, 38);
const CLOSE_PRESSED: R = r(52, 42, 9, 9);
const COLLAPSE_PRESSED: R = r(62, 42, 9, 9);
const EXPAND_PRESSED: R = r(150, 42, 9, 9);
const SHADE_LEFT: R = r(72, 42, 25, 14);
const SHADE_TILE: R = r(72, 57, 25, 14);
const SHADE_RIGHT_SEL: R = r(99, 42, 50, 14);

/// Smallest usable `pledit.bmp`: everything above is within 280 x 110.
pub const MIN_W: u32 = 280;
pub const MIN_H: u32 = 110;

// The composed frame: 300 x 87 = (20 + 29 + 38) high; the fixed left and
// right columns are 125 and 150 wide, the 25 px between them tile.
const FRAME_W: u32 = 300;
const TOP_H: u32 = 20;
const MID_H: u32 = 29;
const BOT_H: u32 = 38;
const FRAME_H: u32 = TOP_H + MID_H + BOT_H;
const LEFT_COL: u32 = 125;
const RIGHT_COL: u32 = 150;

/// Where the button art sits where Winamp bakes it into the top-right
/// corner: close at window `W-11`, shade at `W-21`, both at y = 3.
const BTN: u32 = 9;
const BTN_Y: u32 = 3;
const CLOSE_RIGHT: i32 = 11;
const SHADE_RIGHT: i32 = 21;

/// Button sheet layout: columns close, shade, expand (shade button on the
/// shaded bar); rows normal, hover, pressed. The title art sits below.
const SHEET_W: u32 = 100;
const SHEET_TITLE_Y: u32 = 3 * BTN;
const SHEET_H: u32 = SHEET_TITLE_Y + TOP_H;

/// Decode a BMP or PNG (sniffed from the bytes, never the extension) into
/// opaque RGBA: WinAMP bitmaps have no alpha, and 32-bit ones carry garbage
/// in the fourth channel.
pub fn decode(bytes: &[u8], what: &str) -> Result<Image, String> {
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(4096);
    limits.max_image_height = Some(4096);
    limits.max_alloc = Some(128 << 20);
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| format!("{what}: {e}"))?;
    reader.limits(limits);
    let img = reader
        .decode()
        .map_err(|e| format!("{what}: cannot decode: {e}"))?;
    let mut rgba = img.to_rgba8();
    for p in rgba.pixels_mut() {
        p.0[3] = 255;
    }
    Ok(Image {
        w: rgba.width(),
        h: rgba.height(),
        rgba: rgba.into_raw(),
    })
}

fn blank(w: u32, h: u32, c: Rgb) -> Image {
    let mut rgba = Vec::with_capacity(w as usize * h as usize * 4);
    for _ in 0..w * h {
        rgba.extend_from_slice(&[c.0, c.1, c.2, 255]);
    }
    Image { w, h, rgba }
}

/// Copy `s` of `src` to (`dx`, `dy`) of `dst`, clipped to `dst`.
fn blit(dst: &mut Image, src: &Image, s: R, dx: u32, dy: u32) {
    for y in 0..s.h {
        for x in 0..s.w {
            let (sx, sy, tx, ty) = (s.x + x, s.y + y, dx + x, dy + y);
            if sx >= src.w || sy >= src.h || tx >= dst.w || ty >= dst.h {
                continue;
            }
            let si = (sy as usize * src.w as usize + sx as usize) * 4;
            let di = (ty as usize * dst.w as usize + tx as usize) * 4;
            dst.rgba[di..di + 4].copy_from_slice(&src.rgba[si..si + 4]);
        }
    }
}

/// Repeat `s` horizontally over `x0..x1` at row `dy`; the period is `s.w`.
fn tile_h(dst: &mut Image, src: &Image, s: R, x0: u32, x1: u32, dy: u32) {
    let mut x = x0;
    while x < x1 {
        let w = s.w.min(x1 - x);
        blit(dst, src, R { w, ..s }, x, dy);
        x += s.w;
    }
}

/// The three composed images.
pub struct Parts {
    pub frame: Image,
    pub shade: Image,
    pub sheet: Image,
}

/// Compose frame, shade bar and button sheet from the decoded `pledit`.
pub fn compose(pl: &Image, bg: Rgb) -> Result<Parts, String> {
    if pl.w < MIN_W || pl.h < MIN_H {
        return Err(format!(
            "pledit is {}x{}, too small for a playlist skin (need at least {MIN_W}x{MIN_H})",
            pl.w, pl.h
        ));
    }

    // Frame. Everything that the terminal viewport will cover is painted in
    // the list background, so a translucent viewport still looks right.
    let mut f = blank(FRAME_W, FRAME_H, bg);
    tile_h(&mut f, pl, TOP_TILE, 0, FRAME_W, 0);
    blit(&mut f, pl, TOP_LEFT, 0, 0);
    blit(&mut f, pl, TOP_RIGHT, FRAME_W - TOP_RIGHT.w, 0);
    blit(&mut f, pl, LEFT_TILE, 0, TOP_H);
    blit(&mut f, pl, RIGHT_TILE, FRAME_W - RIGHT_TILE.w, TOP_H);
    let by = TOP_H + MID_H;
    blit(&mut f, pl, BOTTOM_LEFT, 0, by);
    tile_h(&mut f, pl, BOTTOM_TILE, LEFT_COL, FRAME_W - RIGHT_COL, by);
    blit(&mut f, pl, BOTTOM_RIGHT, FRAME_W - RIGHT_COL, by);

    // Windowshade bar: left cap, tiled middle, focused right cap. As wide as
    // the frame so the engine can 3-slice it with the frame's slices.
    let mut s = blank(FRAME_W, SHADE_LEFT.h, bg);
    tile_h(&mut s, pl, SHADE_TILE, 0, FRAME_W, 0);
    blit(&mut s, pl, SHADE_LEFT, 0, 0);
    blit(&mut s, pl, SHADE_RIGHT_SEL, FRAME_W - SHADE_RIGHT_SEL.w, 0);

    // Buttons: normal and hover are the art baked into the corners, pressed
    // is the dedicated sprite.
    let mut sheet = blank(SHEET_W, SHEET_H, bg);
    let corner_x = |from_right: i32| (TOP_RIGHT.x as i32 + TOP_RIGHT.w as i32 - from_right) as u32;
    let cap_x =
        |from_right: i32| (SHADE_RIGHT_SEL.x as i32 + SHADE_RIGHT_SEL.w as i32 - from_right) as u32;
    let columns: [(R, R); 3] = [
        (
            r(corner_x(CLOSE_RIGHT), TOP_RIGHT.y + BTN_Y, BTN, BTN),
            CLOSE_PRESSED,
        ),
        (
            r(corner_x(SHADE_RIGHT), TOP_RIGHT.y + BTN_Y, BTN, BTN),
            COLLAPSE_PRESSED,
        ),
        (
            r(cap_x(SHADE_RIGHT), SHADE_RIGHT_SEL.y + BTN_Y, BTN, BTN),
            EXPAND_PRESSED,
        ),
    ];
    for (i, (normal, pressed)) in columns.iter().enumerate() {
        let x = i as u32 * BTN;
        blit(&mut sheet, pl, *normal, x, 0);
        blit(&mut sheet, pl, *normal, x, BTN);
        blit(&mut sheet, pl, *pressed, x, 2 * BTN);
    }
    blit(&mut sheet, pl, TITLE, 0, SHEET_TITLE_Y);

    Ok(Parts {
        frame: f,
        shade: s,
        sheet,
    })
}

/// The engine definition for the composed images.
pub fn skin_def(name: &str, bg: Rgb) -> SkinDef {
    let button = |col: u32, right: i32, shaded: bool| ButtonDef {
        rect: RectDef {
            x: -right,
            y: BTN_Y as i32,
            w: BTN as i32,
            h: BTN as i32,
        },
        sprite: [col * BTN, 0],
        shaded_sprite: shaded.then_some([2 * BTN, 0]),
    };
    SkinDef {
        name: name.to_string(),
        author: String::new(),
        frame: FrameDef {
            image: "frame.png".into(),
            slices: Insets {
                left: LEFT_COL,
                right: RIGHT_COL,
                top: TOP_H,
                bottom: BOT_H,
            },
            // The top edge is tiled in two runs that meet under the centred
            // title art, so no tile is cut at a corner whatever the width
            // (Winamp itself only has the 25 px steps of its window sizes).
            edge_mode: EdgeMode::TileSplit,
            // The viewport overlaps the left and right columns: it covers
            // everything but Winamp's 12 px left and 20 px right borders.
            viewport_inset: Insets {
                left: LEFT_TILE.w,
                right: RIGHT_TILE.w,
                top: TOP_H,
                bottom: BOT_H,
            },
            viewport_background: Some(bg),
            viewport_opacity: 1.0,
        },
        titlebar: Some(TitlebarDef {
            rect: RectDef {
                x: 0,
                y: 0,
                w: -1,
                h: TOP_H as i32,
            },
            double_click: DoubleClick::Shade,
            // The title is a picture baked into the skin, not text.
            text: None,
            image: Some(TitleImageDef {
                sprite: [0, SHEET_TITLE_Y],
                size: [TITLE.w, TITLE.h],
                align: Align::Center,
            }),
        }),
        buttons: ButtonsDef {
            sheet: Some("buttons.png".into()),
            close: Some(button(0, CLOSE_RIGHT, false)),
            minimize: None,
            // On the shaded bar the same spot shows the expand art.
            shade: Some(button(1, SHADE_RIGHT, true)),
            maximize: None,
        },
        resize: ResizeDef {
            grip: Some(RectDef {
                x: -20,
                y: -20,
                w: 20,
                h: 20,
            }),
            border: 4,
        },
        shade: Some(ShadeDef {
            image: Some("shade.png".into()),
            height: SHADE_LEFT.h,
        }),
    }
}
