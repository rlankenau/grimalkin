//! Configuration: a TOML file with serde defaults for every field.
//!
//! The file lives at `$XDG_CONFIG_HOME/grimalkin/grimalkin.toml` (default
//! `~/.config/grimalkin/grimalkin.toml`) on Unix and
//! `%APPDATA%\grimalkin\grimalkin.toml` on Windows. A missing file is not an
//! error: every field has a default. See `docs/grimalkin.example.toml`.

use std::fmt;
use std::path::{Path, PathBuf};

use serde::de::{self, Deserializer};
use serde::Deserialize;

/// An sRGB color, written `"#rrggbb"` (or `"#rgb"`, `"rrggbb"`) in the file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    pub fn tuple(self) -> (u8, u8, u8) {
        (self.0, self.1, self.2)
    }

    pub fn parse(s: &str) -> Option<Rgb> {
        let s = s.trim().trim_start_matches('#');
        let hex = |t: &str| u8::from_str_radix(t, 16).ok();
        match s.len() {
            6 => Some(Rgb(hex(&s[0..2])?, hex(&s[2..4])?, hex(&s[4..6])?)),
            3 => {
                let n = |i: usize| hex(&s[i..i + 1]).map(|v| v * 17);
                Some(Rgb(n(0)?, n(1)?, n(2)?))
            }
            _ => None,
        }
    }
}

impl fmt::Display for Rgb {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{:02x}{:02x}{:02x}", self.0, self.1, self.2)
    }
}

impl<'de> Deserialize<'de> for Rgb {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Rgb::parse(&s).ok_or_else(|| de::Error::custom(format!("invalid color {s:?}, expected \"#rrggbb\"")))
    }
}

/// Accept either `family = "Menlo"` or `family = ["Menlo", "Apple Color Emoji"]`.
#[derive(Deserialize)]
#[serde(untagged)]
enum OneOrMany {
    One(String),
    Many(Vec<String>),
}

fn family_list<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    Ok(match OneOrMany::deserialize(d)? {
        OneOrMany::One(s) => vec![s],
        OneOrMany::Many(v) => v,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CursorShapeCfg {
    Block,
    Underline,
    Bar,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct FontConfig {
    /// Font families in fallback order. The first one that is installed
    /// defines the cell metrics; later ones are tried (in order) for glyphs
    /// the earlier ones lack, before the system fallback kicks in.
    #[serde(deserialize_with = "family_list")]
    pub family: Vec<String>,
    /// Size in points (logical pixels at 1x scale).
    pub size: f32,
    /// Multiplier applied to the natural line height (ascent + descent).
    pub line_height: f32,
}

impl Default for FontConfig {
    fn default() -> Self {
        FontConfig {
            family: default_families(),
            size: 13.0,
            line_height: 1.0,
        }
    }
}

fn default_families() -> Vec<String> {
    let v: &[&str] = if cfg!(target_os = "macos") {
        &["Menlo", "SF Mono", "Monaco"]
    } else if cfg!(windows) {
        &["Cascadia Mono", "Consolas", "Courier New"]
    } else {
        &["DejaVu Sans Mono", "Liberation Mono", "Noto Sans Mono"]
    };
    v.iter().map(|s| s.to_string()).collect()
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct WindowConfig {
    pub cols: u16,
    pub rows: u16,
    /// Padding around the cell grid, in logical pixels.
    pub padding: f32,
    /// Background opacity, 0.0 (transparent) to 1.0 (opaque).
    pub opacity: f32,
    /// Window title shown until (or instead of) a program sets one.
    pub title: String,
    /// Follow titles set by programs (OSC 0/2). When false they are ignored
    /// and `title` is always shown.
    pub dynamic_title: bool,
}

impl Default for WindowConfig {
    fn default() -> Self {
        WindowConfig {
            cols: 120,
            rows: 35,
            padding: 4.0,
            opacity: 1.0,
            title: "grimalkin".to_string(),
            dynamic_title: true,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct ColorsConfig {
    pub foreground: Rgb,
    pub background: Rgb,
    pub cursor: Rgb,
    pub selection_bg: Rgb,
    pub selection_fg: Rgb,
    /// The 16 ANSI colors (0-7 normal, 8-15 bright). A shorter list overrides
    /// only the first N entries.
    pub ansi: Vec<Rgb>,
}

/// kitty's default theme.
pub const DEFAULT_ANSI: [Rgb; 16] = [
    Rgb(0x00, 0x00, 0x00),
    Rgb(0xcc, 0x04, 0x03),
    Rgb(0x19, 0xcb, 0x00),
    Rgb(0xce, 0xcb, 0x00),
    Rgb(0x0d, 0x73, 0xcc),
    Rgb(0xcb, 0x1e, 0xd1),
    Rgb(0x0d, 0xcd, 0xcd),
    Rgb(0xdd, 0xdd, 0xdd),
    Rgb(0x76, 0x76, 0x76),
    Rgb(0xf2, 0x20, 0x1f),
    Rgb(0x23, 0xfd, 0x00),
    Rgb(0xff, 0xfd, 0x00),
    Rgb(0x1a, 0x8f, 0xff),
    Rgb(0xfd, 0x28, 0xff),
    Rgb(0x14, 0xff, 0xff),
    Rgb(0xff, 0xff, 0xff),
];

impl Default for ColorsConfig {
    fn default() -> Self {
        ColorsConfig {
            foreground: Rgb(0xdd, 0xdd, 0xdd),
            background: Rgb(0x00, 0x00, 0x00),
            cursor: Rgb(0xcc, 0xcc, 0xcc),
            selection_bg: Rgb(0xff, 0xfa, 0xcd),
            selection_fg: Rgb(0x00, 0x00, 0x00),
            ansi: DEFAULT_ANSI.to_vec(),
        }
    }
}

impl ColorsConfig {
    /// The full 16-entry palette with partial user overrides applied.
    pub fn palette16(&self) -> [Rgb; 16] {
        let mut p = DEFAULT_ANSI;
        for (i, c) in self.ansi.iter().take(16).enumerate() {
            p[i] = *c;
        }
        p
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct CursorConfig {
    pub shape: CursorShapeCfg,
    pub blink: bool,
}

impl Default for CursorConfig {
    fn default() -> Self {
        CursorConfig {
            shape: CursorShapeCfg::Block,
            blink: false,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct InputConfig {
    /// macOS only: treat the Option key as Alt (ESC prefix) instead of
    /// composing special characters.
    pub option_as_alt: bool,
}

impl Default for InputConfig {
    fn default() -> Self {
        InputConfig { option_as_alt: true }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct BellConfig {
    /// Flash the window briefly when a program rings the bell.
    pub visual: bool,
    /// Duration of the flash in milliseconds.
    pub visual_ms: u32,
    /// Play the system alert sound.
    pub audible: bool,
}

impl Default for BellConfig {
    fn default() -> Self {
        BellConfig {
            visual: true,
            visual_ms: 150,
            audible: false,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct ClipboardConfig {
    /// Let programs set the clipboard with OSC 52.
    pub osc52_write: bool,
    /// Let programs read the clipboard with OSC 52 (off by default: any
    /// program in the terminal, including remote ones, could read it).
    pub osc52_read: bool,
}

impl Default for ClipboardConfig {
    fn default() -> Self {
        ClipboardConfig {
            osc52_write: true,
            osc52_read: false,
        }
    }
}

/// Environment variable that overrides `[pty] passthrough` (`1`/`true`/`yes`/
/// `on` force it on, `0`/`false`/`no`/`off` force it off).
pub const CONPTY_PASSTHROUGH_ENV: &str = "GRIMALKIN_CONPTY_PASSTHROUGH";

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct PtyConfig {
    /// Windows only: create the ConPTY pseudoconsole in passthrough mode
    /// (`PSEUDOCONSOLE_PASSTHROUGH_MODE`) so a new enough ConPTY forwards APC
    /// (kitty graphics) and other sequences untouched. Needs a sideloaded
    /// `conpty.dll` + `OpenConsole.exe` or Windows 11. Ignored elsewhere.
    pub passthrough: bool,
}

impl PtyConfig {
    /// The effective setting: `GRIMALKIN_CONPTY_PASSTHROUGH` wins over the file.
    pub fn passthrough_enabled(&self) -> bool {
        passthrough_with_env(self.passthrough, std::env::var(CONPTY_PASSTHROUGH_ENV).ok().as_deref())
    }
}

fn passthrough_with_env(configured: bool, env: Option<&str>) -> bool {
    match env.map(|v| v.trim().to_ascii_lowercase()).as_deref() {
        Some("1" | "true" | "yes" | "on") => true,
        Some("0" | "false" | "no" | "off") => false,
        _ => configured,
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct Config {
    pub font: FontConfig,
    pub window: WindowConfig,
    pub colors: ColorsConfig,
    pub cursor: CursorConfig,
    pub input: InputConfig,
    pub bell: BellConfig,
    pub clipboard: ClipboardConfig,
    pub pty: PtyConfig,
    pub scrollback_lines: usize,
    /// Window skin (docs/SKINS.md): a name from `<config_dir>/grimalkin/skins/`
    /// or the built-ins, a directory path, or `"none"` for the normal OS
    /// window. Overridden by `--skin`.
    pub skin: String,
    /// Program to run; empty means the user's default shell.
    pub shell: String,
    pub shell_args: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            font: FontConfig::default(),
            window: WindowConfig::default(),
            colors: ColorsConfig::default(),
            cursor: CursorConfig::default(),
            input: InputConfig::default(),
            bell: BellConfig::default(),
            clipboard: ClipboardConfig::default(),
            pty: PtyConfig::default(),
            scrollback_lines: 10_000,
            skin: "none".to_string(),
            shell: String::new(),
            shell_args: Vec::new(),
        }
    }
}

impl Config {
    /// Per-OS default config file location.
    pub fn default_path() -> Option<PathBuf> {
        #[cfg(windows)]
        let base = dirs::config_dir();
        #[cfg(not(windows))]
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| dirs::home_dir().map(|h| h.join(".config")));
        base.map(|b| b.join("grimalkin").join("grimalkin.toml"))
    }

    /// Load the config. An explicit `path` that cannot be read or parsed is
    /// reported and falls back to defaults; a missing default file is silent.
    pub fn load(explicit: Option<&Path>) -> Config {
        let (path, required) = match explicit {
            Some(p) => (Some(p.to_path_buf()), true),
            None => (Config::default_path(), false),
        };
        let Some(path) = path else {
            return Config::default();
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => match Config::from_toml(&text) {
                Ok(c) => {
                    log::info!("loaded config from {}", path.display());
                    c
                }
                Err(e) => {
                    log::error!("{}: {e}", path.display());
                    Config::default()
                }
            },
            Err(e) => {
                if required || e.kind() != std::io::ErrorKind::NotFound {
                    log::error!("cannot read {}: {e}", path.display());
                }
                Config::default()
            }
        }
    }

    pub fn from_toml(text: &str) -> Result<Config, toml::de::Error> {
        let mut c: Config = toml::from_str(text)?;
        c.sanitize();
        Ok(c)
    }

    /// Clamp values to something the renderer can work with.
    pub fn sanitize(&mut self) {
        self.font.size = if self.font.size.is_finite() { self.font.size.clamp(4.0, 200.0) } else { 13.0 };
        self.font.line_height = if self.font.line_height.is_finite() {
            self.font.line_height.clamp(0.5, 3.0)
        } else {
            1.0
        };
        self.window.opacity = if self.window.opacity.is_finite() { self.window.opacity.clamp(0.0, 1.0) } else { 1.0 };
        self.window.padding = if self.window.padding.is_finite() { self.window.padding.clamp(0.0, 200.0) } else { 4.0 };
        self.window.cols = self.window.cols.clamp(2, 1000);
        self.window.rows = self.window.rows.clamp(1, 1000);
        self.scrollback_lines = self.scrollback_lines.min(1_000_000);
        if self.font.family.is_empty() {
            self.font.family = default_families();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_without_file() {
        let c = Config::from_toml("").unwrap();
        assert_eq!(c.window.cols, 120);
        assert_eq!(c.window.rows, 35);
        assert_eq!(c.scrollback_lines, 10_000);
        assert_eq!(c.colors.palette16()[1], Rgb(0xcc, 0x04, 0x03));
    }

    #[test]
    fn partial_override() {
        let c = Config::from_toml(
            r##"
            scrollback_lines = 5
            [font]
            family = "Fira Code"
            size = 15
            [colors]
            background = "#102030"
            ansi = ["#000001", "#ff0000"]
            [cursor]
            shape = "bar"
            blink = true
            "##,
        )
        .unwrap();
        assert_eq!(c.font.family, vec!["Fira Code".to_string()]);
        assert_eq!(c.colors.background, Rgb(0x10, 0x20, 0x30));
        let p = c.colors.palette16();
        assert_eq!(p[0], Rgb(0, 0, 1));
        assert_eq!(p[1], Rgb(255, 0, 0));
        assert_eq!(p[2], DEFAULT_ANSI[2]);
        assert_eq!(c.cursor.shape, CursorShapeCfg::Bar);
        assert!(c.cursor.blink);
        assert_eq!(c.scrollback_lines, 5);
    }

    #[test]
    fn pty_passthrough_config_and_env_override() {
        assert!(!Config::from_toml("").unwrap().pty.passthrough);
        assert!(Config::from_toml("[pty]\npassthrough = true").unwrap().pty.passthrough);
        assert!(!passthrough_with_env(false, None));
        assert!(passthrough_with_env(true, None));
        assert!(passthrough_with_env(false, Some("1")));
        assert!(passthrough_with_env(false, Some("true")));
        assert!(!passthrough_with_env(true, Some("0")));
        assert!(passthrough_with_env(true, Some("garbage")));
        assert!(!passthrough_with_env(false, Some("")));
    }

    #[test]
    fn skin_key() {
        assert_eq!(Config::from_toml("").unwrap().skin, "none");
        assert_eq!(Config::from_toml("skin = \"debug\"").unwrap().skin, "debug");
    }

    #[test]
    fn bad_color_is_error() {
        assert!(Config::from_toml("[colors]\nforeground = \"nope\"").is_err());
    }

    #[test]
    fn example_file_parses() {
        let text = include_str!("../../../docs/grimalkin.example.toml");
        Config::from_toml(text).unwrap();
    }
}
