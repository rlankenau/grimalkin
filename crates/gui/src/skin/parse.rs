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

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct TitlebarDef {
    pub rect: RectDef,
    pub double_click: DoubleClick,
    pub text: Option<TextDef>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize)]
pub struct ButtonDef {
    pub rect: RectDef,
    /// Top-left of the "normal" sprite in the sheet; hover and pressed are
    /// stacked below it, each `rect.h` apart.
    pub sprite: [u32; 2],
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
            t.font_px = if t.font_px.is_finite() { t.font_px.clamp(4.0, 96.0) } else { 11.0 };
        }
        d.resize.border = d.resize.border.min(64);
        Ok(d)
    }

    /// Check the definition against the frame image size.
    pub fn validate(&self, img_w: u32, img_h: u32) -> Result<(), String> {
        let s = self.frame.slices;
        if s.left + s.right > img_w || s.top + s.bottom > img_h {
            return Err(format!(
                "frame slices ({}+{} x {}+{}) exceed the {img_w}x{img_h} frame image",
                s.left, s.right, s.top, s.bottom
            ));
        }
        Ok(())
    }
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
        assert!(d.validate(8, 8).is_ok());
        assert!(d.validate(7, 8).is_err());
        assert!(SkinDef::from_toml("[frame]\nedge_mode = \"wobble\"").is_err());
    }
}
