//! `pledit.txt` and `viscolor.txt`, and the theme derived from them.

use crate::config::{OneOrMany, Rgb};
use crate::theme::{mix, Theme, ThemeColors, ThemeFont};

/// The playlist editor colours (`[Text]` of `pledit.txt`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlEdit {
    pub normal: Rgb,
    pub current: Rgb,
    pub normal_bg: Rgb,
    pub selected_bg: Rgb,
    pub font: String,
}

impl Default for PlEdit {
    /// The base-2.91 defaults.
    fn default() -> Self {
        PlEdit {
            normal: Rgb(0x00, 0xff, 0x00),
            current: Rgb(0xff, 0xff, 0xff),
            normal_bg: Rgb(0x00, 0x00, 0x00),
            selected_bg: Rgb(0x00, 0x00, 0xc6),
            font: "Arial".into(),
        }
    }
}

/// Bytes as text: Latin-1 (skins predate UTF-8), minus a BOM.
fn latin1(bytes: &[u8]) -> String {
    let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
    bytes.iter().map(|&b| b as char).collect()
}

/// `#RRGGBB`, with the quirks skins have: no `#`, trailing junk.
fn color(v: &str) -> Option<Rgb> {
    let v = v.trim();
    let mut s = if v.starts_with('#') {
        v.to_string()
    } else {
        format!("#{v}")
    };
    // Truncate to 7 chars (not bytes: the text is Latin-1).
    s = s.chars().take(7).collect();
    Rgb::parse(&s)
}

pub fn parse_pledit(bytes: &[u8]) -> PlEdit {
    let mut out = PlEdit::default();
    let mut in_text = true; // tolerate a missing section header
    for line in latin1(bytes).lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with(';') {
            continue;
        }
        if let Some(sec) = line.strip_prefix('[') {
            in_text = sec
                .trim_end_matches(']')
                .trim()
                .eq_ignore_ascii_case("text");
            continue;
        }
        if !in_text {
            continue;
        }
        let mut parts = line.splitn(3, '=');
        let (Some(key), Some(val)) = (parts.next(), parts.next()) else {
            continue;
        };
        let val = val.trim();
        match key.trim().to_ascii_lowercase().as_str() {
            "normal" => out.normal = color(val).unwrap_or(out.normal),
            "current" => out.current = color(val).unwrap_or(out.current),
            "normalbg" => out.normal_bg = color(val).unwrap_or(out.normal_bg),
            "selectedbg" => out.selected_bg = color(val).unwrap_or(out.selected_bg),
            "font" => {
                let f = val.trim_matches(|c| c == '"' || c == '\'').trim();
                if !f.is_empty() {
                    out.font = f.to_string();
                }
            }
            _ => {}
        }
    }
    out
}

/// The 24 colours of the stock `viscolor.txt`.
const DEFAULT_VIS: [[u8; 3]; 24] = [
    [0, 0, 0],
    [24, 33, 41],
    [239, 49, 16],
    [206, 41, 16],
    [214, 90, 0],
    [214, 102, 0],
    [214, 115, 0],
    [198, 123, 8],
    [222, 165, 24],
    [214, 181, 33],
    [189, 222, 41],
    [148, 222, 33],
    [41, 206, 16],
    [50, 190, 16],
    [57, 181, 16],
    [49, 156, 8],
    [41, 148, 0],
    [24, 132, 8],
    [255, 255, 255],
    [214, 214, 222],
    [181, 189, 189],
    [160, 170, 175],
    [148, 156, 165],
    [150, 150, 150],
];

/// `^\s*(\d+)\s*,?\s*(\d+)\s*,?\s*(\d+)`
fn triplet(line: &str) -> Option<[u8; 3]> {
    let mut rest = line;
    let mut out = [0u8; 3];
    for (i, slot) in out.iter_mut().enumerate() {
        rest = rest.trim_start();
        let end = rest
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(rest.len());
        if end == 0 {
            return None;
        }
        *slot = rest[..end].parse::<u32>().unwrap_or(255).min(255) as u8;
        rest = &rest[end..];
        if i < 2 {
            rest = rest.trim_start();
            rest = rest.strip_prefix(',').unwrap_or(rest);
        }
    }
    Some(out)
}

/// Parse `viscolor.txt`; missing entries come from the stock skin.
pub fn parse_viscolor(bytes: &[u8]) -> [Rgb; 24] {
    let mut out = DEFAULT_VIS.map(|c| Rgb(c[0], c[1], c[2]));
    let mut n = 0;
    for line in latin1(bytes).lines() {
        if n == 24 {
            break;
        }
        if let Some(c) = triplet(line) {
            out[n] = Rgb(c[0], c[1], c[2]);
            n += 1;
        }
    }
    out
}

/// The stock viscolor table (for skins without a `viscolor.txt`).
pub fn default_viscolor() -> [Rgb; 24] {
    DEFAULT_VIS.map(|c| Rgb(c[0], c[1], c[2]))
}

fn lin(c: u8) -> f32 {
    let c = c as f32 / 255.0;
    if c <= 0.03928 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn luminance(c: Rgb) -> f32 {
    0.2126 * lin(c.0) + 0.7152 * lin(c.1) + 0.0722 * lin(c.2)
}

/// WCAG contrast ratio, 1..21.
pub fn contrast(a: Rgb, b: Rgb) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// The theme a skin brings along.
#[derive(Clone, Debug)]
pub struct SkinTheme {
    pub theme: Theme,
    /// Optional ANSI palette (from `viscolor.txt`); applied only when the
    /// `winamp_viscolor_palette` option is on.
    pub ansi: [Option<Rgb>; 16],
}

/// Theme colours from `pledit.txt` (see docs/WINAMP_SKINS.md).
pub fn theme_colors(p: &PlEdit) -> ThemeColors {
    // Selected text: the normal colour unless it vanishes on the selection.
    let selection_fg = if contrast(p.normal, p.selected_bg) >= 3.0
        || contrast(p.normal, p.selected_bg) >= contrast(p.current, p.selected_bg)
    {
        p.normal
    } else {
        p.current
    };
    // A cursor that blends into the list background is useless.
    let cursor = if contrast(p.current, p.normal_bg) >= 1.5 {
        p.current
    } else {
        p.normal
    };
    ThemeColors {
        foreground: Some(p.normal),
        background: Some(p.normal_bg),
        cursor: Some(cursor),
        cursor_text: Some(p.normal_bg),
        selection_background: Some(p.selected_bg),
        selection_foreground: Some(selection_fg),
        url: Some(cursor),
        ansi: [None; 16],
    }
}

/// The 16 ANSI colours derived from the viscolor table.
pub fn viscolor_ansi(c: &[Rgb; 24], p: &PlEdit) -> [Rgb; 16] {
    let red = c[2];
    let yellow = c[8];
    let green = c[12];
    let cyan = mix(green, c[19], 0.5);
    let blue = mix(c[19], p.selected_bg, 0.5);
    let magenta = mix(red, blue, 0.5);
    let black = p.normal_bg;
    let white = c[20];
    let br = |x: Rgb| mix(x, white, 0.25);
    [
        black,
        red,
        green,
        yellow,
        blue,
        magenta,
        cyan,
        white,
        c[1],
        br(red),
        br(green),
        br(yellow),
        br(blue),
        br(magenta),
        br(cyan),
        br(white),
    ]
}

/// Build the skin's theme. `font_ok` says whether a font family is usable
/// (installed and monospaced); without it the font hint is dropped.
pub fn build_theme(
    name: &str,
    p: &PlEdit,
    vis: &[Rgb; 24],
    font_ok: Option<&dyn Fn(&str) -> bool>,
) -> SkinTheme {
    let mut font = ThemeFont::default();
    if font_ok.is_some_and(|ok| ok(&p.font)) {
        font.family = Some(OneOrMany::One(p.font.clone()));
    }
    let ansi = viscolor_ansi(vis, p).map(Some);
    SkinTheme {
        theme: Theme {
            name: name.to_string(),
            colors: theme_colors(p),
            font,
            ..Theme::default()
        },
        ansi,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pledit_parses_the_usual_forms() {
        let p = parse_pledit(
            b"[Text]\r\nNormal=#00FF00\r\nCurrent=#FFFFFF\r\nNormalBG=#000000\r\nSelectedBG=#0000C6\r\nFont=Arial\r\n",
        );
        assert_eq!(p, PlEdit::default());
        let p = parse_pledit(b"\xef\xbb\xbf; comment\n[text]\nNORMAL=ff8000\nCurrent = #123456789\nnormalbg=#102030 ; x\nSelectedBG==#0a0b0c=zz\nFont = \"Lucida Sans\"\n[Other]\nNormal=#111111\n");
        assert_eq!(p.normal, Rgb(0xff, 0x80, 0x00), "missing # is added");
        assert_eq!(p.current, Rgb(0x12, 0x34, 0x56), "extra digits are cut");
        assert_eq!(p.normal_bg, Rgb(0x10, 0x20, 0x30));
        assert_eq!(
            p.selected_bg,
            PlEdit::default().selected_bg,
            "garbage keeps the default"
        );
        assert_eq!(p.font, "Lucida Sans");
    }

    #[test]
    fn missing_or_empty_pledit_gives_the_base_skin_colours() {
        assert_eq!(parse_pledit(b""), PlEdit::default());
        assert_eq!(
            parse_pledit(b"[Other]\nNormal=#111111\n"),
            PlEdit::default()
        );
        // Keys before any header are tolerated.
        assert_eq!(
            parse_pledit(b"Normal=#111111\n").normal,
            Rgb(0x11, 0x11, 0x11)
        );
    }

    #[test]
    fn viscolor_tolerates_spaces_comments_and_gaps() {
        let v = parse_viscolor(b"1,2,3 // black\n 4 5 6\nnot a colour\n7, 8, 9\n300,0,0\n");
        assert_eq!(v[0], Rgb(1, 2, 3));
        assert_eq!(v[1], Rgb(4, 5, 6));
        assert_eq!(v[2], Rgb(7, 8, 9));
        assert_eq!(v[3], Rgb(255, 0, 0), "clamped");
        assert_eq!(
            v[4],
            default_viscolor()[4],
            "missing entries use the stock table"
        );
        assert_eq!(parse_viscolor(b""), default_viscolor());
    }

    #[test]
    fn theme_maps_pledit_colours() {
        let p = PlEdit::default();
        let t = build_theme("base", &p, &default_viscolor(), None);
        let c = &t.theme.colors;
        assert_eq!(c.foreground, Some(Rgb(0, 0xff, 0)));
        assert_eq!(c.background, Some(Rgb(0, 0, 0)));
        assert_eq!(c.selection_background, Some(Rgb(0, 0, 0xc6)));
        assert_eq!(c.cursor, Some(Rgb(255, 255, 255)));
        assert_eq!(c.cursor_text, Some(Rgb(0, 0, 0)));
        assert_eq!(c.url, Some(Rgb(255, 255, 255)));
        // Green on the blue selection has a contrast ratio above 3.
        assert_eq!(c.selection_foreground, Some(Rgb(0, 0xff, 0)));
        assert!(t.theme.font.family.is_none(), "no font probe, no hint");
        assert!(t.ansi.iter().all(|a| a.is_some()));
    }

    #[test]
    fn selection_text_falls_back_to_current_when_it_vanishes() {
        let p = PlEdit {
            normal: Rgb(0x20, 0x20, 0x20),
            selected_bg: Rgb(0x10, 0x10, 0x10),
            current: Rgb(0xff, 0xff, 0xff),
            ..PlEdit::default()
        };
        assert_eq!(
            theme_colors(&p).selection_foreground,
            Some(Rgb(255, 255, 255))
        );
    }

    #[test]
    fn font_hint_needs_the_probe_to_agree() {
        let p = PlEdit::default();
        let yes = |f: &str| f == "Arial";
        let no = |_: &str| false;
        assert!(build_theme("x", &p, &default_viscolor(), Some(&yes))
            .theme
            .font
            .family
            .is_some());
        assert!(build_theme("x", &p, &default_viscolor(), Some(&no))
            .theme
            .font
            .family
            .is_none());
    }

    #[test]
    fn contrast_ratio_extremes() {
        assert!((contrast(Rgb(0, 0, 0), Rgb(255, 255, 255)) - 21.0).abs() < 0.01);
        assert!((contrast(Rgb(9, 9, 9), Rgb(9, 9, 9)) - 1.0).abs() < 1e-6);
    }
}
