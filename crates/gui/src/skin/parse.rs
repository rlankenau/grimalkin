//! `skin.toml` schema (see docs/SKINS.md) and its validation.

use serde::Deserialize;

use crate::config::Rgb;

/// Pixel insets / 9-slice caps, in skin (1x) pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Insets {
    pub left: u32,
    pub right: u32,
    pub top: u32,
    pub bottom: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EdgeMode {
    #[default]
    Tile,
    Stretch,
    /// Like `Tile`, but the top edge is laid out as two runs that meet at the
    /// window centre: the left run starts at the left cap, the right run ends
    /// flush with the right cap. A tile is therefore never cut at a cap, for
    /// any window width; the one seam sits mid-edge, where a centred
    /// titlebar image normally hides it (WinAMP's playlist frame).
    #[serde(rename = "tile_split")]
    TileSplit,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DoubleClick {
    Shade,
    Maximize,
    #[default]
    None,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
}

/// A rectangle in frame space (skin pixels). Negative values are relative to
/// the far edge: `x < 0` counts from the right edge, `y < 0` from the bottom;
/// `w == -1` / `h == -1` extend to the right / bottom edge, and `w < -1` /
/// `h < -1` leave a margin of `-w` / `-h` pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct RectDef {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct FrameDef {
    pub image: String,
    pub slices: Insets,
    pub edge_mode: EdgeMode,
    pub viewport_inset: Insets,
    pub viewport_background: Option<Rgb>,
    pub viewport_opacity: f32,
}

impl Default for FrameDef {
    fn default() -> Self {
        FrameDef {
            image: "frame.png".into(),
            slices: Insets::default(),
            edge_mode: EdgeMode::Tile,
            viewport_inset: Insets::default(),
            viewport_background: None,
            viewport_opacity: 1.0,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct TextDef {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub font_px: f32,
    pub color: Rgb,
    pub align: Align,
}

impl Default for TextDef {
    fn default() -> Self {
        TextDef {
            x: 0,
            y: 0,
            w: -1,
            font_px: 11.0,
            color: Rgb(0xff, 0xff, 0xff),
            align: Align::Left,
        }
    }
}

/// A picture in the titlebar (sprite in the buttons sheet), e.g. WinAMP's
/// baked-in "WINAMP PLAYLIST EDITOR" art. Not drawn on the shade bar.
#[derive(Clone, Copy, Debug, Default, Deserialize)]
#[serde(default)]
pub struct TitleImageDef {
    /// Top-left of the sprite in the buttons sheet.
    pub sprite: [u32; 2],
    /// Sprite size in skin pixels.
    pub size: [u32; 2],
    pub align: Align,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct TitlebarDef {
    pub rect: RectDef,
    pub double_click: DoubleClick,
    pub text: Option<TextDef>,
    pub image: Option<TitleImageDef>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize)]
pub struct ButtonDef {
    pub rect: RectDef,
    /// Top-left of the "normal" sprite in the sheet; hover and pressed are
    /// stacked below it, each `rect.h` apart.
    pub sprite: [u32; 2],
    /// Sprite to use instead while the window is shaded (optional).
    pub shaded_sprite: Option<[u32; 2]>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct ButtonsDef {
    pub sheet: Option<String>,
    pub close: Option<ButtonDef>,
    pub minimize: Option<ButtonDef>,
    pub shade: Option<ButtonDef>,
    pub maximize: Option<ButtonDef>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct ResizeDef {
    pub grip: Option<RectDef>,
    /// Width in skin pixels of the resize zone along the window edges.
    pub border: u32,
}

impl Default for ResizeDef {
    fn default() -> Self {
        ResizeDef { grip: None, border: 6 }
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct ShadeDef {
    pub image: Option<String>,
    /// Height of the shaded window in skin pixels.
    pub height: u32,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct SkinDef {
    pub name: String,
    pub author: String,
    pub frame: FrameDef,
    pub titlebar: Option<TitlebarDef>,
    pub buttons: ButtonsDef,
    pub resize: ResizeDef,
    pub shade: Option<ShadeDef>,
}

impl SkinDef {
    pub fn from_toml(text: &str) -> Result<SkinDef, String> {
        let mut d: SkinDef = toml::from_str(text).map_err(|e| format!("skin.toml: {e}"))?;
        d.frame.viewport_opacity = if d.frame.viewport_opacity.is_finite() {
            d.frame.viewport_opacity.clamp(0.0, 1.0)
        } else {
            1.0
        };
        if let Some(t) = d.titlebar.as_mut().and_then(|t| t.text.as_mut()) {
            t.font_px = if t.font_px.is_finite() { t.font_px.clamp(MIN_FONT_PX, MAX_FONT_PX) } else { 11.0 };
        }
        d.resize.border = d.resize.border.min(MAX_BORDER);
        Ok(d)
    }

    /// Check every numeric field against the frame image size. This is the one
    /// place hostile or nonsensical values are stopped: anything that is
    /// rejected here never reaches layout, the renderer or the window system,
    /// so the arithmetic downstream only ever sees bounded values.
    pub fn validate(&self, img_w: u32, img_h: u32) -> Result<(), String> {
        if img_w == 0 || img_h == 0 || img_w > MAX_SKIN_DIM || img_h > MAX_SKIN_DIM {
            return Err(format!("frame image is {img_w}x{img_h}; it must be 1..={MAX_SKIN_DIM} pixels per side"));
        }
        let (w, h) = (img_w as i64, img_h as i64);
        check_insets("frame.slices", &self.frame.slices, w, h)?;
        check_insets("frame.viewport_inset", &self.frame.viewport_inset, w, h)?;

        let op = self.frame.viewport_opacity;
        if !op.is_finite() || !(0.0..=1.0).contains(&op) {
            return Err(format!("frame.viewport_opacity {op} is outside 0..=1"));
        }

        if let Some(t) = &self.titlebar {
            check_rect("titlebar.rect", &t.rect, w, h)?;
            if let Some(tx) = &t.text {
                check_extent("titlebar.text.x", tx.x as i64, w)?;
                check_extent("titlebar.text.y", tx.y as i64, h)?;
                check_len("titlebar.text.w", tx.w as i64, w)?;
                if !tx.font_px.is_finite() || !(MIN_FONT_PX..=MAX_FONT_PX).contains(&tx.font_px) {
                    return Err(format!(
                        "titlebar.text.font_px {} is outside {MIN_FONT_PX}..={MAX_FONT_PX}",
                        tx.font_px
                    ));
                }
            }
            if let Some(im) = &t.image {
                check_sprite("titlebar.image.sprite", im.sprite)?;
                check_sprite("titlebar.image.size", im.size)?;
            }
        }

        let b = &self.buttons;
        for (name, def) in [
            ("buttons.close", &b.close),
            ("buttons.minimize", &b.minimize),
            ("buttons.shade", &b.shade),
            ("buttons.maximize", &b.maximize),
        ] {
            let Some(d) = def else { continue };
            check_rect(&format!("{name}.rect"), &d.rect, w, h)?;
            // Hover and pressed sprites are stacked below the normal one, so
            // the sprite row plus three heights must stay representable.
            check_sprite(&format!("{name}.sprite"), d.sprite)?;
            if let Some(sp) = d.shaded_sprite {
                check_sprite(&format!("{name}.shaded_sprite"), sp)?;
            }
        }

        if let Some(g) = &self.resize.grip {
            check_rect("resize.grip", g, w, h)?;
        }
        if self.resize.border > MAX_BORDER {
            return Err(format!("resize.border {} exceeds {MAX_BORDER}", self.resize.border));
        }

        if let Some(sh) = &self.shade {
            // 0 means "derive from the titlebar" (see `Skin::shade_height`).
            if sh.height > MAX_SHADE_HEIGHT {
                return Err(format!("shade.height {} exceeds {MAX_SHADE_HEIGHT}", sh.height));
            }
            if sh.height as i64 > h {
                return Err(format!("shade.height {} exceeds the {img_h} pixel frame image height", sh.height));
            }
        }
        Ok(())
    }
}

/// Largest frame image side (and sprite coordinate / size) a skin may use.
pub const MAX_SKIN_DIM: u32 = 16384;
/// Largest windowshade bar height, in skin pixels.
pub const MAX_SHADE_HEIGHT: u32 = 4096;
/// Largest resize zone width, in skin pixels.
pub const MAX_BORDER: u32 = 64;
pub const MIN_FONT_PX: f32 = 4.0;
pub const MAX_FONT_PX: f32 = 96.0;

fn check_insets(what: &str, v: &Insets, w: i64, h: i64) -> Result<(), String> {
    // i64 so that no u32 sum can wrap.
    let (l, r, t, b) = (v.left as i64, v.right as i64, v.top as i64, v.bottom as i64);
    if l + r >= w || t + b >= h {
        return Err(format!(
            "{what} ({}+{} x {}+{}) must be smaller than the {w}x{h} frame image",
            v.left, v.right, v.top, v.bottom
        ));
    }
    Ok(())
}

/// A frame-space position: negative counts from the far edge; either way its
/// magnitude is bounded by the frame size.
fn check_extent(what: &str, v: i64, limit: i64) -> Result<(), String> {
    if v.abs() > limit {
        return Err(format!("{what} {v} is outside the +/-{limit} frame range"));
    }
    Ok(())
}

/// A frame-space length: -1 (to the edge), `< -1` (a margin) or a size.
fn check_len(what: &str, v: i64, limit: i64) -> Result<(), String> {
    if v.abs() > limit {
        return Err(format!("{what} {v} is outside the +/-{limit} frame range"));
    }
    Ok(())
}

fn check_rect(what: &str, r: &RectDef, w: i64, h: i64) -> Result<(), String> {
    check_extent(&format!("{what}.x"), r.x as i64, w)?;
    check_extent(&format!("{what}.y"), r.y as i64, h)?;
    check_len(&format!("{what}.w"), r.w as i64, w)?;
    check_len(&format!("{what}.h"), r.h as i64, h)?;
    // A rect measured from the left/top edge must also end inside the frame.
    if r.x >= 0 && r.w >= 0 && r.x as i64 + r.w as i64 > w {
        return Err(format!("{what} (x {} + w {}) extends past the {w} pixel frame width", r.x, r.w));
    }
    if r.y >= 0 && r.h >= 0 && r.y as i64 + r.h as i64 > h {
        return Err(format!("{what} (y {} + h {}) extends past the {h} pixel frame height", r.y, r.h));
    }
    Ok(())
}

fn check_sprite(what: &str, v: [u32; 2]) -> Result<(), String> {
    if v[0] > MAX_SKIN_DIM || v[1] > MAX_SKIN_DIM {
        return Err(format!("{what} {v:?} exceeds {MAX_SKIN_DIM}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_spec_example() {
        let d = SkinDef::from_toml(
            r##"
            name = "Llama"
            author = "x"
            [frame]
            image = "frame.png"
            slices = { left = 24, right = 24, top = 36, bottom = 28 }
            edge_mode = "stretch"
            viewport_inset = { left = 24, right = 24, top = 36, bottom = 28 }
            viewport_background = "#000000"
            viewport_opacity = 2.0
            [titlebar]
            rect = { x = 0, y = 0, w = -1, h = 36 }
            double_click = "shade"
            text = { x = 40, y = 10, w = -120, font_px = 11, color = "#9bd6a1", align = "center" }
            [buttons]
            sheet = "buttons.png"
            close = { rect = { x = -22, y = 8, w = 14, h = 14 }, sprite = [0, 0] }
            shade = { rect = { x = -58, y = 8, w = 14, h = 14 }, sprite = [28, 0] }
            [resize]
            grip = { x = -20, y = -20, w = 20, h = 20 }
            border = 6
            [shade]
            image = "shade.png"
            height = 14
            "##,
        )
        .unwrap();
        assert_eq!(d.name, "Llama");
        assert_eq!(d.frame.edge_mode, EdgeMode::Stretch);
        assert_eq!(d.frame.slices.top, 36);
        assert_eq!(d.frame.viewport_opacity, 1.0, "clamped");
        let tb = d.titlebar.unwrap();
        assert_eq!(tb.double_click, DoubleClick::Shade);
        assert_eq!(tb.text.unwrap().align, Align::Center);
        assert_eq!(d.buttons.close.unwrap().rect.x, -22);
        assert!(d.buttons.minimize.is_none());
        assert_eq!(d.buttons.shade.unwrap().sprite, [28, 0]);
        assert_eq!(d.resize.grip.unwrap().w, 20);
        assert_eq!(d.shade.unwrap().height, 14);
    }

    #[test]
    fn minimal_and_invalid() {
        let d = SkinDef::from_toml("[frame]\nslices = { left = 4, right = 4, top = 4, bottom = 4 }").unwrap();
        assert!(d.titlebar.is_none());
        assert_eq!(d.resize.border, 6);
        // Slices must leave at least one texel for the centre.
        assert!(d.validate(9, 9).is_ok());
        assert!(d.validate(8, 9).is_err());
        assert!(d.validate(9, 8).is_err());
        assert!(SkinDef::from_toml("[frame]\nedge_mode = \"wobble\"").is_err());
    }

    #[test]
    fn validate_uses_wide_arithmetic_and_bounds_every_field() {
        let ok = |t: &str| SkinDef::from_toml(t).unwrap().validate(64, 64);
        // A: u32 sums cannot wrap.
        assert!(ok("[frame]\nslices = { left = 4294967295, right = 4294967295 }").is_err());
        // Frame image size itself is bounded.
        assert!(SkinDef::default().validate(0, 10).is_err());
        assert!(SkinDef::default().validate(MAX_SKIN_DIM + 1, 10).is_err());
        // Negative (from-the-far-edge) positions are fine within the frame, not beyond it.
        assert!(ok("[buttons]\nclose = { rect = { x = -22, y = -20, w = 14, h = 14 }, sprite = [0, 0] }").is_ok());
        assert!(ok("[buttons]\nclose = { rect = { x = -65, y = 0, w = 14, h = 14 }, sprite = [0, 0] }").is_err());
        assert!(ok("[buttons]\nclose = { rect = { x = 60, y = 0, w = 14, h = 14 }, sprite = [0, 0] }").is_err());
        // w = -1 and margins stay valid.
        assert!(ok("[titlebar]\nrect = { x = 0, y = 0, w = -1, h = 12 }").is_ok());
        assert!(ok("[titlebar]\nrect = { x = 0, y = 0, w = -10, h = 12 }").is_ok());
        assert!(ok("[titlebar]\nrect = { x = 0, y = 0, w = -65, h = 12 }").is_err());
        assert!(ok("[resize]\ngrip = { x = 2147483647, y = 0, w = 1, h = 1 }").is_err());
        // Shade: 0 = derive from the titlebar; the cap and the frame height both apply.
        assert!(ok("[shade]\nheight = 0").is_ok());
        assert!(ok("[shade]\nheight = 12").is_ok());
        assert!(ok("[shade]\nheight = 65").is_err());
        assert!(SkinDef::from_toml("[shade]\nheight = 5000").unwrap().validate(64, 16384).is_err());
        // Definitions built without from_toml are checked too.
        let mut d = SkinDef::default();
        d.resize.border = 65;
        assert!(d.validate(64, 64).is_err());
        let mut d = SkinDef::from_toml("[titlebar]\ntext = { font_px = 11 }").unwrap();
        d.titlebar.as_mut().unwrap().text.as_mut().unwrap().font_px = f32::NAN;
        assert!(d.validate(64, 64).is_err());
    }
}
