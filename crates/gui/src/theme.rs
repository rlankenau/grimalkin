//! Themes: colors, cursor and font overrides that can be swapped at runtime.
//!
//! A theme is looked up by name, first in `<config_dir>/grimalkin/themes/`
//! (`<name>.toml`, then `<name>.conf` in kitty's theme format), then in the
//! set embedded in the binary from `assets/themes/`. The `[colors]` table of
//! `grimalkin.toml` is applied on top, key by key (see [`resolve_for_skin`]). The file
//! formats are documented in `docs/SKINS.md` and `docs/grimalkin.example.toml`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::de::{self, Deserializer};
use serde::Deserialize;

use crate::config::{Config, CursorShapeCfg, FontConfig, OneOrMany, Rgb, DEFAULT_ANSI};
use crate::skin::winamp::SkinTheme;
use crate::skin::Skin;

/// Name of the theme used when `theme` is not set (and the fallback when a
/// named theme cannot be found).
pub const DEFAULT_THEME: &str = "default";

const BUILTIN: &[(&str, &str)] = &[
    ("default", include_str!("../assets/themes/default.toml")),
    ("catppuccin-mocha", include_str!("../assets/themes/catppuccin-mocha.toml")),
    ("dracula", include_str!("../assets/themes/dracula.toml")),
    ("gruvbox-dark", include_str!("../assets/themes/gruvbox-dark.toml")),
    ("solarized-light", include_str!("../assets/themes/solarized-light.toml")),
    ("nord", include_str!("../assets/themes/nord.toml")),
    ("tokyo-night", include_str!("../assets/themes/tokyo-night.toml")),
];

const ANSI_NAMES: [&str; 16] = [
    "black",
    "red",
    "green",
    "yellow",
    "blue",
    "magenta",
    "cyan",
    "white",
    "bright_black",
    "bright_red",
    "bright_green",
    "bright_yellow",
    "bright_blue",
    "bright_magenta",
    "bright_cyan",
    "bright_white",
];

/// Parse a color: `#rrggbb`, `#rgb`, `rrggbb` or X11 `rgb:rr/gg/bb`.
pub fn parse_color(s: &str) -> Option<Rgb> {
    let s = s.trim().trim_matches(|c| c == '"' || c == '\'');
    if let Some(rest) = s.strip_prefix("rgb:") {
        let mut it = rest.split('/');
        let mut chan = || {
            let p = it.next()?;
            if p.is_empty() || p.len() > 4 || !p.bytes().all(|b| b.is_ascii_hexdigit()) {
                return None;
            }
            let v = u32::from_str_radix(p, 16).ok()?;
            let max = (1u32 << (4 * p.len())) - 1;
            Some(((v * 255 + max / 2) / max) as u8)
        };
        let (r, g, b) = (chan()?, chan()?, chan()?);
        return if it.next().is_none() { Some(Rgb(r, g, b)) } else { None };
    }
    Rgb::parse(s)
}

/// A set of optional color settings: what a theme file or the `[colors]` table
/// of `grimalkin.toml` specifies. Unset entries fall through to the next layer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ThemeColors {
    pub foreground: Option<Rgb>,
    pub background: Option<Rgb>,
    pub cursor: Option<Rgb>,
    /// Text color under a block cursor (default: the background color).
    pub cursor_text: Option<Rgb>,
    pub selection_background: Option<Rgb>,
    pub selection_foreground: Option<Rgb>,
    /// Color of the hovered-link underline (default: the text color).
    pub url: Option<Rgb>,
    /// ANSI 0-15.
    pub ansi: [Option<Rgb>; 16],
}

impl ThemeColors {
    /// Set one entry by key name. Returns false if the key is unknown.
    /// Accepts grimalkin names (`selection_background`, `black`, ...), the
    /// `color0`..`color15` aliases, and kitty's spellings (`cursor_text_color`,
    /// `url_color`).
    pub fn set(&mut self, key: &str, c: Rgb) -> bool {
        match key {
            "foreground" | "fg" => self.foreground = Some(c),
            "background" | "bg" => self.background = Some(c),
            "cursor" => self.cursor = Some(c),
            "cursor_text" | "cursor_text_color" => self.cursor_text = Some(c),
            "selection_background" | "selection_bg" => self.selection_background = Some(c),
            "selection_foreground" | "selection_fg" => self.selection_foreground = Some(c),
            "url" | "url_color" => self.url = Some(c),
            _ => {
                let idx = ANSI_NAMES.iter().position(|n| *n == key).or_else(|| {
                    let n = key.strip_prefix("color")?;
                    if n.is_empty() || !n.bytes().all(|b| b.is_ascii_digit()) {
                        return None;
                    }
                    n.parse::<usize>().ok().filter(|&n| n < 16)
                });
                match idx {
                    Some(i) => self.ansi[i] = Some(c),
                    None => return false,
                }
            }
        }
        true
    }

    /// Parse a kitty theme file (`key value` lines, `#` comments).
    ///
    /// Keys that do not describe the colors handled here (tab bar, marks,
    /// `color16`+, `include`, ...) are ignored, as are `none` values.
    /// `cursor_text_color background` takes the background color.
    pub fn from_kitty(text: &str) -> ThemeColors {
        let mut c = ThemeColors::default();
        let mut cursor_text_is_bg = false;
        for line in text.trim_start_matches('\u{feff}').lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut parts = line.split_whitespace();
            let (Some(key), Some(val)) = (parts.next(), parts.next()) else { continue };
            let key = key.to_ascii_lowercase();
            if key == "cursor_text_color" && val.eq_ignore_ascii_case("background") {
                cursor_text_is_bg = true;
                continue;
            }
            if let Some(rgb) = parse_color(val) {
                c.set(&key, rgb);
            }
        }
        if cursor_text_is_bg && c.cursor_text.is_none() {
            c.cursor_text = c.background;
        }
        c
    }

    /// `self` with every entry set in `over` replaced.
    pub fn overlaid(&self, over: &ThemeColors) -> ThemeColors {
        let mut o = self.clone();
        let pick = |a: &mut Option<Rgb>, b: Option<Rgb>| {
            if b.is_some() {
                *a = b;
            }
        };
        pick(&mut o.foreground, over.foreground);
        pick(&mut o.background, over.background);
        pick(&mut o.cursor, over.cursor);
        pick(&mut o.cursor_text, over.cursor_text);
        pick(&mut o.selection_background, over.selection_background);
        pick(&mut o.selection_foreground, over.selection_foreground);
        pick(&mut o.url, over.url);
        for i in 0..16 {
            pick(&mut o.ansi[i], over.ansi[i]);
        }
        o
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ColorValue {
    One(String),
    List(Vec<String>),
    /// Anything else (a number, a table, ...): reported and ignored.
    Other(de::IgnoredAny),
}

/// Colours are read leniently: a bad entry is reported and skipped so that it
/// cannot cost the user every other setting in the file.
impl<'de> Deserialize<'de> for ThemeColors {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let map = BTreeMap::<String, ColorValue>::deserialize(d)?;
        let mut c = ThemeColors::default();
        let mut ansi_list: Option<Vec<String>> = None;
        for (key, val) in map {
            match val {
                ColorValue::List(v) if key == "ansi" => ansi_list = Some(v),
                ColorValue::List(_) | ColorValue::Other(_) if key == "ansi" => {
                    log::warn!("colors.ansi must be a list of color strings; ignored");
                }
                ColorValue::List(_) | ColorValue::Other(_) => {
                    log::warn!("colors.{key} must be a color string; ignored");
                }
                ColorValue::One(s) => match parse_color(&s) {
                    Some(rgb) => {
                        if !c.set(&key, rgb) {
                            log::warn!("unknown color key {key:?} ignored");
                        }
                    }
                    None => log::warn!("invalid color {s:?} for colors.{key}, expected \"#rrggbb\"; ignored"),
                },
            }
        }
        // `ansi = [...]` is the base; named/colorN keys win over it.
        if let Some(list) = ansi_list {
            for (i, s) in list.iter().take(16).enumerate() {
                match parse_color(s) {
                    Some(rgb) => {
                        if c.ansi[i].is_none() {
                            c.ansi[i] = Some(rgb);
                        }
                    }
                    None => log::warn!("invalid color {s:?} in colors.ansi[{i}]; ignored"),
                }
            }
        }
        Ok(c)
    }
}

/// Optional `[font]` overrides of a theme.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct ThemeFont {
    pub family: Option<OneOrMany>,
    pub size: Option<f32>,
    pub line_height: Option<f32>,
}

/// Optional `[cursor]` overrides of a theme.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct ThemeCursor {
    pub shape: Option<CursorShapeCfg>,
    pub blink: Option<bool>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Theme {
    pub name: String,
    pub colors: ThemeColors,
    pub font: ThemeFont,
    pub cursor: ThemeCursor,
}

impl Theme {
    pub fn from_toml(text: &str) -> Result<Theme, String> {
        toml::from_str(text).map_err(|e| e.to_string())
    }

    pub fn from_kitty(name: &str, text: &str) -> Theme {
        Theme {
            name: name.to_string(),
            colors: ThemeColors::from_kitty(text),
            ..Theme::default()
        }
    }
}

/// Where a theme came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    User(PathBuf),
    BuiltIn,
}

/// Theme names are file names: no separators, no leading dots.
fn valid_name(name: &str) -> bool {
    !name.is_empty() && !name.starts_with('.') && !name.contains(['/', '\\', ':', '\0'])
}

fn builtin(name: &str) -> Option<&'static str> {
    BUILTIN.iter().find(|(n, _)| *n == name).map(|(_, t)| *t)
}

/// Load a theme by name. `user_dir` is the `themes` directory to search
/// before the built-ins.
pub fn load(name: &str, user_dir: Option<&Path>) -> Result<Theme, String> {
    if !valid_name(name) {
        return Err(format!("invalid theme name {name:?}"));
    }
    if let Some(dir) = user_dir {
        let toml_path = dir.join(format!("{name}.toml"));
        if let Ok(text) = std::fs::read_to_string(&toml_path) {
            return Theme::from_toml(&text).map_err(|e| format!("{}: {e}", toml_path.display()));
        }
        let conf_path = dir.join(format!("{name}.conf"));
        if let Ok(text) = std::fs::read_to_string(&conf_path) {
            return Ok(Theme::from_kitty(name, &text));
        }
    }
    match builtin(name) {
        Some(text) => Theme::from_toml(text).map_err(|e| format!("built-in theme {name}: {e}")),
        None => Err(format!("theme {name:?} not found")),
    }
}

/// The user theme directory (`<config_dir>/grimalkin/themes`).
pub fn user_dir() -> Option<PathBuf> {
    Config::config_dir().map(|d| d.join("themes"))
}

/// All themes available, sorted by name; a user theme shadows a built-in one.
pub fn list(user_dir: Option<&Path>) -> Vec<(String, Source)> {
    let mut map: BTreeMap<String, Source> = BUILTIN.iter().map(|(n, _)| (n.to_string(), Source::BuiltIn)).collect();
    if let Some(dir) = user_dir {
        if let Ok(rd) = std::fs::read_dir(dir) {
            for e in rd.flatten() {
                let p = e.path();
                let ext = p.extension().and_then(|s| s.to_str());
                if !matches!(ext, Some("toml" | "conf")) {
                    continue;
                }
                if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                    if valid_name(stem) {
                        map.insert(stem.to_string(), Source::User(p.clone()));
                    }
                }
            }
        }
    }
    map.into_iter().collect()
}

/// Text for `--list-themes`.
pub fn list_text(user_dir: Option<&Path>) -> String {
    let mut s = String::new();
    for (name, src) in list(user_dir) {
        match src {
            Source::BuiltIn => s.push_str(&format!("{name}\n")),
            Source::User(p) => s.push_str(&format!("{name}\t(user: {})\n", p.display())),
        }
    }
    s
}

/// Fully resolved colors, ready to apply to the terminal and renderer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    pub foreground: Rgb,
    pub background: Rgb,
    /// `None`: use the foreground color.
    pub cursor: Option<Rgb>,
    /// `None`: use the background color.
    pub cursor_text: Option<Rgb>,
    pub selection_bg: Rgb,
    pub selection_fg: Rgb,
    /// `None`: underline hovered links in their text color.
    pub url: Option<Rgb>,
    pub ansi: [Rgb; 16],
}

pub(crate) fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Rgb(m(a.0, b.0), m(a.1, b.1), m(a.2, b.2))
}

impl Palette {
    /// Fill the gaps of `c` with defaults.
    pub fn from_colors(c: &ThemeColors) -> Palette {
        let foreground = c.foreground.unwrap_or(Rgb(0xdd, 0xdd, 0xdd));
        let background = c.background.unwrap_or(Rgb(0, 0, 0));
        let mut ansi = DEFAULT_ANSI;
        for (i, a) in c.ansi.iter().enumerate() {
            if let Some(a) = a {
                ansi[i] = *a;
            }
        }
        Palette {
            foreground,
            background,
            cursor: c.cursor,
            cursor_text: c.cursor_text,
            selection_bg: c.selection_background.unwrap_or_else(|| mix(background, foreground, 0.3)),
            selection_fg: c.selection_foreground.unwrap_or(foreground),
            url: c.url,
            ansi,
        }
    }
}

/// The outcome of [`resolve_for_skin`].
pub struct Resolved {
    /// Theme actually used (after fallback).
    pub name: String,
    pub palette: Palette,
    /// The config with the theme's font and cursor overrides applied.
    pub config: Config,
}

/// Resolve the effective colors and settings: the theme named by
/// `cfg.theme` (falling back to `default` with an error in the log), then the
/// `[colors]` table of the config on top, key by key. The theme's `[font]`
/// and `[cursor]` keys, when present, replace the config's.
///
/// For a window that runs `skin`, an imported skin that brings a theme
/// (WinAMP `pledit.txt`) supplies the base colours unless the user picked a
/// theme of their own.
pub fn resolve_for_skin(cfg: &Config, skin: Option<&Skin>) -> Resolved {
    resolve_with(cfg, user_dir().as_deref(), skin.and_then(|s| s.theme.as_ref()))
}

#[cfg(test)]
pub fn resolve_in(cfg: &Config, dir: Option<&Path>) -> Resolved {
    resolve_with(cfg, dir, None)
}

/// The skin's theme laid over the built-in default (a skin theme sets only
/// some colours), with the ANSI palette when the user opted in.
fn skin_base_theme(cfg: &Config, st: &SkinTheme) -> Theme {
    let mut theme = load(DEFAULT_THEME, None).expect("built-in default theme parses");
    let mut over = st.theme.colors.clone();
    if cfg.winamp_viscolor_palette {
        over.ansi = st.ansi;
    }
    theme.colors = theme.colors.overlaid(&over);
    theme.name = st.theme.name.clone();
    // The font hint only stands in for the default font, never for one the
    // user configured.
    if st.theme.font.family.is_some() && cfg.font.family == FontConfig::default().family {
        theme.font = st.theme.font.clone();
    }
    theme
}

fn resolve_with(cfg: &Config, dir: Option<&Path>, skin_theme: Option<&SkinTheme>) -> Resolved {
    let wanted = cfg.theme.as_str();
    let theme = match skin_theme.filter(|_| wanted == DEFAULT_THEME) {
        Some(st) => skin_base_theme(cfg, st),
        None => match load(wanted, dir) {
            Ok(t) => t,
            Err(e) => {
                log::error!("{e}; using the default theme");
                load(DEFAULT_THEME, None).expect("built-in default theme parses")
            }
        },
    };
    let colors = theme.colors.overlaid(&cfg.colors);
    let mut config = cfg.clone();
    if let Some(f) = &theme.font.family {
        config.font.family = match f {
            OneOrMany::One(s) => vec![s.clone()],
            OneOrMany::Many(v) => v.clone(),
        };
    }
    if let Some(s) = theme.font.size {
        config.font.size = s;
    }
    if let Some(l) = theme.font.line_height {
        config.font.line_height = l;
    }
    if let Some(s) = theme.cursor.shape {
        config.cursor.shape = s;
    }
    if let Some(b) = theme.cursor.blink {
        config.cursor.blink = b;
    }
    config.sanitize();
    let name = if theme.name.is_empty() { wanted.to_string() } else { theme.name };
    Resolved {
        name,
        palette: Palette::from_colors(&colors),
        config,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("grimalkin-theme-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn toml_theme_parses_all_keys() {
        let t = Theme::from_toml(
            r##"
            name = "Test"
            [colors]
            foreground = "#cdd6f4"
            background = "1e1e2e"
            cursor = "#f5e0dc"
            cursor_text = "#111"
            selection_background = "#585b70"
            selection_foreground = "#cdd6f4"
            url = "#89b4fa"
            black = "#000001"
            bright_white = "#fffffe"
            [font]
            family = ["JetBrains Mono"]
            size = 15
            [cursor]
            shape = "bar"
            blink = true
            "##,
        )
        .unwrap();
        assert_eq!(t.name, "Test");
        assert_eq!(t.colors.foreground, Some(Rgb(0xcd, 0xd6, 0xf4)));
        assert_eq!(t.colors.background, Some(Rgb(0x1e, 0x1e, 0x2e)));
        assert_eq!(t.colors.cursor_text, Some(Rgb(0x11, 0x11, 0x11)));
        assert_eq!(t.colors.selection_background, Some(Rgb(0x58, 0x5b, 0x70)));
        assert_eq!(t.colors.url, Some(Rgb(0x89, 0xb4, 0xfa)));
        assert_eq!(t.colors.ansi[0], Some(Rgb(0, 0, 1)));
        assert_eq!(t.colors.ansi[15], Some(Rgb(0xff, 0xff, 0xfe)));
        assert_eq!(t.colors.ansi[1], None);
        assert!(matches!(t.font.family, Some(OneOrMany::Many(ref v)) if v == &["JetBrains Mono"]));
        assert_eq!(t.font.size, Some(15.0));
        assert_eq!(t.cursor.shape, Some(CursorShapeCfg::Bar));
        assert_eq!(t.cursor.blink, Some(true));
    }

    #[test]
    fn toml_color_aliases_and_ansi_list() {
        let t = Theme::from_toml(
            r##"
            [colors]
            color1 = "#ff0000"
            color15 = "#fefefe"
            red = "#00ff00"
            ansi = ["#010101", "#020202", "#030303"]
            "##,
        )
        .unwrap();
        // The ansi list is a base; named keys win over it.
        assert_eq!(t.colors.ansi[0], Some(Rgb(1, 1, 1)));
        assert_eq!(t.colors.ansi[2], Some(Rgb(3, 3, 3)));
        assert_eq!(t.colors.ansi[1], Some(Rgb(0, 0xff, 0)));
        assert_eq!(t.colors.ansi[15], Some(Rgb(0xfe, 0xfe, 0xfe)));
        // color16 is not an ANSI slot: ignored, not an error.
        assert!(Theme::from_toml("[colors]\ncolor16 = \"#000000\"").is_ok());
    }

    #[test]
    fn toml_errors() {
        // Bad colour entries are skipped, not fatal; syntax errors still are.
        let t = Theme::from_toml("[colors]\nforeground = \"nope\"\nbackground = \"#000001\"").unwrap();
        assert_eq!((t.colors.foreground, t.colors.background), (None, Some(Rgb(0, 0, 1))));
        assert_eq!(Theme::from_toml("[colors]\nforeground = [\"#000000\"]").unwrap().colors.foreground, None);
        assert_eq!(Theme::from_toml("[colors]\nansi = [\"zzz\"]").unwrap().colors.ansi[0], None);
        assert!(Theme::from_toml("[colors\nforeground =").is_err());
    }

    #[test]
    fn color_parsing() {
        assert_eq!(parse_color("#abc"), Some(Rgb(0xaa, 0xbb, 0xcc)));
        assert_eq!(parse_color(" \"#010203\" "), Some(Rgb(1, 2, 3)));
        assert_eq!(parse_color("rgb:ff/80/00"), Some(Rgb(255, 128, 0)));
        assert_eq!(parse_color("rgb:ffff/0000/8080"), Some(Rgb(255, 0, 128)));
        assert_eq!(parse_color("rgb:f/0/0"), Some(Rgb(255, 0, 0)));
        assert_eq!(parse_color("rgb:ff/80"), None);
        assert_eq!(parse_color("none"), None);
        assert_eq!(parse_color("#ééé"), None);
        assert_eq!(parse_color(""), None);
    }

    const KITTY: &str = "\u{feff}# vim:ft=kitty\n\
        ## name: Test\n\
        \n\
        foreground\t#c0caf5\n\
        background    #1a1b26\n\
        selection_foreground #c0caf5\n\
        selection_background #33467c\n\
        cursor #c0caf5\n\
        cursor_text_color background\n\
        url_color #73daca\n\
        active_tab_foreground #1f2335\n\
        mark1_background #394b70\n\
        color0 #15161e\n\
        color1  #f7768e # trailing comment\n\
        COLOR8 #414868\n\
        color15 #c0caf5\n\
        color16 #ff9e64\n\
        color200 #000000\n\
        include other.conf\n\
        lonely_key\n\
        color2 notacolor\n";

    #[test]
    fn kitty_theme_parses() {
        let c = ThemeColors::from_kitty(KITTY);
        assert_eq!(c.foreground, Some(Rgb(0xc0, 0xca, 0xf5)));
        assert_eq!(c.background, Some(Rgb(0x1a, 0x1b, 0x26)));
        assert_eq!(c.selection_foreground, Some(Rgb(0xc0, 0xca, 0xf5)));
        assert_eq!(c.selection_background, Some(Rgb(0x33, 0x46, 0x7c)));
        assert_eq!(c.cursor, Some(Rgb(0xc0, 0xca, 0xf5)));
        // `cursor_text_color background` resolves to the background color.
        assert_eq!(c.cursor_text, Some(Rgb(0x1a, 0x1b, 0x26)));
        assert_eq!(c.url, Some(Rgb(0x73, 0xda, 0xca)));
        assert_eq!(c.ansi[0], Some(Rgb(0x15, 0x16, 0x1e)));
        // Trailing tokens are ignored; keys are case-insensitive.
        assert_eq!(c.ansi[1], Some(Rgb(0xf7, 0x76, 0x8e)));
        assert_eq!(c.ansi[8], Some(Rgb(0x41, 0x48, 0x68)));
        assert_eq!(c.ansi[15], Some(Rgb(0xc0, 0xca, 0xf5)));
        // Bad values, color16+ and tab-bar keys are skipped.
        assert_eq!(c.ansi[2], None);
        assert_eq!(c.ansi[3], None);
    }

    #[test]
    fn kitty_none_and_cursor_text() {
        let c = ThemeColors::from_kitty("cursor none\nselection_foreground none\ncursor_text_color #101010\n");
        assert_eq!(c.cursor, None);
        assert_eq!(c.selection_foreground, None);
        assert_eq!(c.cursor_text, Some(Rgb(0x10, 0x10, 0x10)));
    }

    #[test]
    fn builtins_all_parse_and_are_complete() {
        for (name, _) in BUILTIN {
            let t = load(name, None).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert!(!t.name.is_empty(), "{name}");
            let c = &t.colors;
            assert!(c.foreground.is_some() && c.background.is_some() && c.cursor.is_some(), "{name}");
            assert!(c.selection_background.is_some() && c.selection_foreground.is_some(), "{name}");
            assert!(c.ansi.iter().all(|a| a.is_some()), "{name}: missing ansi colors");
        }
        let names: Vec<_> = list(None).into_iter().map(|(n, _)| n).collect();
        for want in ["default", "catppuccin-mocha", "dracula", "gruvbox-dark", "solarized-light", "nord", "tokyo-night"] {
            assert!(names.iter().any(|n| n == want), "{want}");
        }
    }

    #[test]
    fn default_theme_matches_config_defaults() {
        let p = Palette::from_colors(&load("default", None).unwrap().colors);
        assert_eq!(p.ansi, DEFAULT_ANSI);
        assert_eq!(p.foreground, Rgb(0xdd, 0xdd, 0xdd));
        assert_eq!(p.background, Rgb(0, 0, 0));
        assert_eq!(p.cursor, Some(Rgb(0xcc, 0xcc, 0xcc)));
        assert_eq!(p.selection_bg, Rgb(0xff, 0xfa, 0xcd));
    }

    #[test]
    fn lookup_prefers_user_dir_and_supports_kitty() {
        let d = tmpdir("lookup");
        std::fs::write(d.join("dracula.toml"), "name = \"Mine\"\n[colors]\nbackground = \"#010203\"\n").unwrap();
        std::fs::write(d.join("kit.conf"), "background #0a0b0c\ncolor3 #111111\n").unwrap();
        assert_eq!(load("dracula", Some(&d)).unwrap().name, "Mine");
        assert_eq!(load("dracula", None).unwrap().name, "Dracula");
        let k = load("kit", Some(&d)).unwrap();
        assert_eq!(k.colors.background, Some(Rgb(0x0a, 0x0b, 0x0c)));
        assert_eq!(k.colors.ansi[3], Some(Rgb(0x11, 0x11, 0x11)));
        assert!(load("nord", Some(&d)).is_ok());
        assert!(load("missing", Some(&d)).is_err());
        let l = list(Some(&d));
        assert!(l.iter().any(|(n, s)| n == "kit" && matches!(s, Source::User(_))));
        assert!(l.iter().any(|(n, s)| n == "dracula" && matches!(s, Source::User(_))));
        assert!(l.iter().any(|(n, s)| n == "nord" && *s == Source::BuiltIn));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn rejects_path_traversal_names() {
        for n in ["../x", "a/b", "a\\b", ".hidden", "", "c:x"] {
            assert!(load(n, None).is_err(), "{n}");
        }
    }

    #[test]
    fn config_colors_override_theme_per_key() {
        let cfg = Config::from_toml(
            r##"
            theme = "dracula"
            [colors]
            background = "#101010"
            selection_bg = "#202020"
            red = "#ff0001"
            "##,
        )
        .unwrap();
        let r = resolve_in(&cfg, None);
        assert_eq!(r.name, "Dracula");
        let p = r.palette;
        assert_eq!(p.background, Rgb(0x10, 0x10, 0x10));
        assert_eq!(p.selection_bg, Rgb(0x20, 0x20, 0x20));
        assert_eq!(p.ansi[1], Rgb(0xff, 0, 1));
        // Untouched keys come from the theme.
        assert_eq!(p.foreground, Rgb(0xf8, 0xf8, 0xf2));
        assert_eq!(p.ansi[2], Rgb(0x50, 0xfa, 0x7b));
        assert_eq!(p.selection_fg, Rgb(0xf8, 0xf8, 0xf2));
    }

    #[test]
    fn unknown_theme_falls_back_to_default() {
        let cfg = Config::from_toml("theme = \"nope\"").unwrap();
        let r = resolve_in(&cfg, None);
        assert_eq!(r.palette.ansi, DEFAULT_ANSI);
    }

    #[test]
    fn theme_font_and_cursor_override_config() {
        let d = tmpdir("font");
        std::fs::write(
            d.join("f.toml"),
            "[font]\nfamily = \"Fira Code\"\nsize = 17\n[cursor]\nshape = \"underline\"\n",
        )
        .unwrap();
        let cfg = Config::from_toml("theme = \"f\"\n[font]\nline_height = 1.2\n[cursor]\nblink = true").unwrap();
        let r = resolve_in(&cfg, Some(&d));
        assert_eq!(r.config.font.family, vec!["Fira Code".to_string()]);
        assert_eq!(r.config.font.size, 17.0);
        assert_eq!(r.config.font.line_height, 1.2);
        assert_eq!(r.config.cursor.shape, CursorShapeCfg::Underline);
        assert!(r.config.cursor.blink);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn derived_selection_when_theme_lacks_it() {
        let c = ThemeColors::from_kitty("foreground #ffffff\nbackground #000000\n");
        let p = Palette::from_colors(&c);
        assert_eq!(p.selection_bg, Rgb(77, 77, 77));
        assert_eq!(p.selection_fg, Rgb(255, 255, 255));
    }
}
