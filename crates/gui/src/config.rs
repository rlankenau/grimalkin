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
        if !s.is_ascii() {
            return None;
        }
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
#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
pub enum OneOrMany {
    One(String),
    Many(Vec<String>),
}

fn family_list<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    Ok(match OneOrMany::deserialize(d)? {
        OneOrMany::One(s) => vec![s],
        OneOrMany::Many(v) => v,
    })
}

/// Read an integer setting, clamping instead of failing when it is negative
/// or too large for the field (`cols = 100000`, `scrollback_lines = -1`).
/// Out-of-range values are fixed up further by [`Config::sanitize`]; a value
/// that is not a number at all is still an error.
fn lenient_uint<'de, D: Deserializer<'de>>(d: D, max: u64, what: &str) -> Result<u64, D::Error> {
    let v = f64::deserialize(d)?;
    if v.is_nan() {
        log::warn!("config: {what} is not a number; ignoring");
        return Ok(0);
    }
    let clamped = v.max(0.0).min(max as f64) as u64;
    if clamped as f64 != v.trunc() {
        log::warn!("config: {what} = {v} is out of range; using {clamped}");
    }
    Ok(clamped)
}

fn lenient_u16<'de, D: Deserializer<'de>>(d: D) -> Result<u16, D::Error> {
    lenient_uint(d, u16::MAX as u64, "window size").map(|v| v as u16)
}

fn lenient_u32<'de, D: Deserializer<'de>>(d: D) -> Result<u32, D::Error> {
    lenient_uint(d, u32::MAX as u64, "duration").map(|v| v as u32)
}

fn lenient_usize<'de, D: Deserializer<'de>>(d: D) -> Result<usize, D::Error> {
    lenient_uint(d, u32::MAX as u64, "scrollback_lines").map(|v| v as usize)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CursorShapeCfg {
    Block,
    Underline,
    Bar,
}

impl FontConfig {
    /// `family` followed by `fallback`, without case-insensitive duplicates:
    /// the order glyph lookup tries families in.
    pub fn effective_families(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for n in self.family.iter().chain(self.fallback.iter()) {
            if !out.iter().any(|o| o.eq_ignore_ascii_case(n)) {
                out.push(n.clone());
            }
        }
        out
    }
}

/// Default CJK fallback, one family per script group where possible so kanji
/// and kana come from the same font (Yu Gothic UI, PingFang SC and Noto Sans
/// CJK all cover both).
fn default_fallback() -> Vec<String> {
    let v: &[&str] = if cfg!(target_os = "macos") {
        &["PingFang SC", "Hiragino Sans"]
    } else if cfg!(windows) {
        &["Yu Gothic UI", "Microsoft YaHei", "Malgun Gothic"]
    } else {
        &["Noto Sans CJK SC", "Noto Sans CJK JP", "Noto Sans CJK KR"]
    };
    v.iter().map(|s| s.to_string()).collect()
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct FontConfig {
    /// Font families in fallback order. The first one that is installed
    /// defines the cell metrics; later ones are tried (in order) for glyphs
    /// the earlier ones lack, before the system fallback kicks in.
    #[serde(deserialize_with = "family_list")]
    pub family: Vec<String>,
    /// CJK / symbol fallback families, tried in order after `family` and
    /// before the system fallback. Empty uses the per-OS default.
    #[serde(deserialize_with = "family_list")]
    pub fallback: Vec<String>,
    /// Size in points (logical pixels at 1x scale).
    pub size: f32,
    /// Multiplier applied to the natural line height (ascent + descent).
    pub line_height: f32,
}

impl Default for FontConfig {
    fn default() -> Self {
        FontConfig {
            family: default_families(),
            fallback: default_fallback(),
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
    #[serde(deserialize_with = "lenient_u16")]
    pub cols: u16,
    #[serde(deserialize_with = "lenient_u16")]
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

/// The `[colors]` table: per-key overrides applied on top of the theme (see
/// `theme.rs`). Keys: foreground, background, cursor, cursor_text,
/// selection_background (`selection_bg`), selection_foreground
/// (`selection_fg`), url, the ANSI names black..bright_white or
/// color0..color15, and `ansi = [..]` (first N entries).
pub use crate::theme::ThemeColors as ColorsConfig;

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
    /// Plain Ctrl+V is grimalkin's own (sanitised) paste instead of sending
    /// 0x16 to the program. Default: on for Windows, off elsewhere.
    pub ctrl_v_paste: bool,
}

impl Default for InputConfig {
    fn default() -> Self {
        InputConfig {
            option_as_alt: true,
            ctrl_v_paste: cfg!(windows),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct BellConfig {
    /// Flash the window briefly when a program rings the bell.
    pub visual: bool,
    /// Duration of the flash in milliseconds.
    #[serde(deserialize_with = "lenient_u32")]
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

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct PasteConfig {
    /// Ask before pasting text that contains a newline (not asked when the
    /// program has bracketed paste on: it handles multi-line text itself).
    pub confirm_multiline: bool,
    /// Ask before pasting more than this many bytes (0 = never ask).
    #[serde(deserialize_with = "lenient_usize")]
    pub confirm_large_bytes: usize,
}

impl Default for PasteConfig {
    fn default() -> Self {
        PasteConfig {
            confirm_multiline: true,
            confirm_large_bytes: 1_048_576,
        }
    }
}

/// Environment variable that overrides `[pty] passthrough` (`1`/`true`/`yes`/
/// `on` force it on, `0`/`false`/`no`/`off` force it off).
pub const CONPTY_PASSTHROUGH_ENV: &str = "GRIMALKIN_CONPTY_PASSTHROUGH";

/// How the cursor behaves after a kitty graphics placement (`[pty]
/// graphics_cursor`).
///
/// The protocol moves the cursor past a placed image unless the command has
/// `C=1`. Windows ConPTY's OpenConsole never learns that an APC placement moved
/// the cursor (it does not parse the sequence), so the next output is drawn too
/// high, overlapping the image. Tools that print their own newlines (such as
/// `gcat --conpty`) work if the terminal does not move the cursor either.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GraphicsCursor {
    /// `None` under ConPTY (every Windows session), `Protocol` elsewhere.
    #[default]
    Auto,
    /// Apply the kitty protocol's cursor movement after placements.
    Protocol,
    /// Never move the cursor after a placement (`C=1` for every placement).
    None,
}

impl GraphicsCursor {
    /// Whether the cursor should follow the protocol's movement after a
    /// placement, given whether this session runs under ConPTY.
    pub fn follows_protocol(self, conpty: bool) -> bool {
        match self {
            GraphicsCursor::Protocol => true,
            GraphicsCursor::None => false,
            GraphicsCursor::Auto => !conpty,
        }
    }

    /// [`follows_protocol`](Self::follows_protocol) for this platform: all
    /// Windows sessions are ConPTY sessions.
    pub fn follows_protocol_here(self) -> bool {
        self.follows_protocol(cfg!(windows))
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct PtyConfig {
    /// Windows only: create the ConPTY pseudoconsole in passthrough mode
    /// (`PSEUDOCONSOLE_PASSTHROUGH_MODE`) so a new enough ConPTY forwards APC
    /// (kitty graphics) and other sequences untouched. Needs a sideloaded
    /// `conpty.dll` + `OpenConsole.exe` or Windows 11. Ignored elsewhere.
    pub passthrough: bool,
    /// Cursor movement after kitty graphics placements: `auto` (default),
    /// `protocol` or `none`; see [`GraphicsCursor`].
    pub graphics_cursor: GraphicsCursor,
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
    /// Theme name: a user theme in `<config_dir>/grimalkin/themes/` or a
    /// built-in one (`--list-themes`).
    pub theme: String,
    pub cursor: CursorConfig,
    pub input: InputConfig,
    pub bell: BellConfig,
    pub clipboard: ClipboardConfig,
    pub paste: PasteConfig,
    pub pty: PtyConfig,
    #[serde(deserialize_with = "lenient_usize")]
    pub scrollback_lines: usize,
    /// Window skin (docs/SKINS.md): a name from `<config_dir>/grimalkin/skins/`
    /// or the built-ins, a directory path, or `"none"` for the normal OS
    /// window. Overridden by `--skin`.
    pub skin: String,
    /// WinAMP skins (`skin = "winamp:<name>"`) also set the ANSI palette from
    /// their `viscolor.txt`. Off by default: the result is a mood, not a
    /// calibrated palette.
    pub winamp_viscolor_palette: bool,
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
            theme: crate::theme::DEFAULT_THEME.to_string(),
            cursor: CursorConfig::default(),
            input: InputConfig::default(),
            bell: BellConfig::default(),
            clipboard: ClipboardConfig::default(),
            paste: PasteConfig::default(),
            pty: PtyConfig::default(),
            scrollback_lines: 10_000,
            skin: "none".to_string(),
            winamp_viscolor_palette: false,
            shell: String::new(),
            shell_args: Vec::new(),
        }
    }
}

impl Config {
    /// Per-OS config directory (`.../grimalkin`): config file, `themes/`, `skins/`.
    pub fn config_dir() -> Option<PathBuf> {
        #[cfg(windows)]
        let base = dirs::config_dir();
        #[cfg(not(windows))]
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| dirs::home_dir().map(|h| h.join(".config")));
        base.map(|b| b.join("grimalkin"))
    }

    /// Per-OS default config file location.
    pub fn default_path() -> Option<PathBuf> {
        Config::config_dir().map(|d| d.join("grimalkin.toml"))
    }

    /// Read and parse the config file for a reload. Unlike [`Config::load`],
    /// failure is returned so the caller can keep the running settings.
    /// A missing file yields the defaults.
    pub fn try_load(explicit: Option<&Path>) -> Result<Config, String> {
        let Some(path) = explicit.map(Path::to_path_buf).or_else(Config::default_path) else {
            return Ok(Config::default());
        };
        match crate::fsutil::read_regular_string(&path, crate::fsutil::MAX_CONFIG_BYTES) {
            Ok(text) => Config::from_toml(&text).map_err(|e| format!("{}: {e}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
            Err(e) => Err(format!("cannot read {}: {e}", path.display())),
        }
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
        match crate::fsutil::read_regular_string(&path, crate::fsutil::MAX_CONFIG_BYTES) {
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

    /// Clamp values to something the renderer can work with, warning about
    /// each one that had to change (the rest of the file stays in force).
    pub fn sanitize(&mut self) {
        fn fix(name: &str, v: &mut f32, lo: f32, hi: f32, fallback: f32) {
            let new = if v.is_finite() { v.clamp(lo, hi) } else { fallback };
            if new != *v && !(v.is_nan() && new.is_nan()) {
                log::warn!("config: {name} = {v} is outside {lo}..={hi}; using {new}");
            }
            *v = new;
        }
        fix("font.size", &mut self.font.size, FONT_SIZE_RANGE.0, FONT_SIZE_RANGE.1, 13.0);
        fix("font.line_height", &mut self.font.line_height, 0.5, 3.0, 1.0);
        fix("window.opacity", &mut self.window.opacity, 0.0, 1.0, 1.0);
        fix("window.padding", &mut self.window.padding, 0.0, 200.0, 4.0);
        let (cols, rows) = (self.window.cols, self.window.rows);
        self.window.cols = cols.clamp(2, 1000);
        self.window.rows = rows.clamp(1, 1000);
        if (cols, rows) != (self.window.cols, self.window.rows) {
            log::warn!(
                "config: window size {cols}x{rows} is outside 2x1..=1000x1000; using {}x{}",
                self.window.cols,
                self.window.rows
            );
        }
        if self.scrollback_lines > MAX_SCROLLBACK_LINES {
            log::warn!(
                "config: scrollback_lines = {} is too large; using {MAX_SCROLLBACK_LINES}",
                self.scrollback_lines
            );
            self.scrollback_lines = MAX_SCROLLBACK_LINES;
        }
        if self.font.family.is_empty() {
            self.font.family = default_families();
        }
        if self.font.fallback.is_empty() {
            self.font.fallback = default_fallback();
        }
    }
}

/// Upper bound for `scrollback_lines`.
/// Allowed `font.size` (points); the zoom commands use the same range so
/// zooming never moves the size the wrong way at the ends.
pub const FONT_SIZE_RANGE: (f32, f32) = (4.0, 200.0);

pub const MAX_SCROLLBACK_LINES: usize = 1_000_000;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn out_of_range_values_are_clamped() {
        let c = Config::from_toml(
            "scrollback_lines = 99999999999\n[font]\nsize = 500.0\nline_height = 0.1\n[window]\nopacity = 7.0\npadding = -5.0\ncols = 1\nrows = 5000\n",
        )
        .unwrap();
        assert_eq!((c.font.size, c.font.line_height), (200.0, 0.5));
        assert_eq!((c.window.opacity, c.window.padding), (1.0, 0.0));
        assert_eq!((c.window.cols, c.window.rows), (2, 1000));
        assert_eq!(c.scrollback_lines, 1_000_000);
        let c = Config::from_toml("[font]\nsize = -3.0\n[window]\nopacity = -1.0\n").unwrap();
        assert_eq!((c.font.size, c.window.opacity), (4.0, 0.0));
    }

    #[test]
    fn defaults_without_file() {
        let c = Config::from_toml("").unwrap();
        assert_eq!(c.window.cols, 120);
        assert_eq!(c.window.rows, 35);
        assert_eq!(c.scrollback_lines, 10_000);
        assert_eq!(c.theme, "default");
        assert_eq!(c.colors, ColorsConfig::default());
    }

    #[test]
    fn effective_families_order_and_dedupe() {
        let c = Config::from_toml("[font]\nfamily = [\"A\", \"b\"]\nfallback = [\"B\", \"C\"]").unwrap();
        assert_eq!(c.font.effective_families(), vec!["A", "b", "C"]);
        let d = Config::default();
        assert!(!d.font.fallback.is_empty());
        assert_eq!(d.font.effective_families()[..d.font.family.len()], d.font.family[..]);
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
        assert_eq!(c.colors.background, Some(Rgb(0x10, 0x20, 0x30)));
        let p = c.colors.ansi;
        assert_eq!(p[0], Some(Rgb(0, 0, 1)));
        assert_eq!(p[1], Some(Rgb(255, 0, 0)));
        assert_eq!(p[2], None);
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
    fn graphics_cursor_config_and_policy() {
        assert_eq!(Config::from_toml("").unwrap().pty.graphics_cursor, GraphicsCursor::Auto);
        for (v, want) in [
            ("auto", GraphicsCursor::Auto),
            ("protocol", GraphicsCursor::Protocol),
            ("none", GraphicsCursor::None),
        ] {
            let c = Config::from_toml(&format!("[pty]\ngraphics_cursor = \"{v}\"")).unwrap();
            assert_eq!(c.pty.graphics_cursor, want);
        }
        assert!(Config::from_toml("[pty]\ngraphics_cursor = \"bogus\"").is_err());
        // auto: no movement under ConPTY, protocol movement elsewhere.
        assert!(!GraphicsCursor::Auto.follows_protocol(true));
        assert!(GraphicsCursor::Auto.follows_protocol(false));
        assert!(GraphicsCursor::Protocol.follows_protocol(true));
        assert!(GraphicsCursor::Protocol.follows_protocol(false));
        assert!(!GraphicsCursor::None.follows_protocol(true));
        assert!(!GraphicsCursor::None.follows_protocol(false));
        assert_eq!(GraphicsCursor::Auto.follows_protocol_here(), !cfg!(windows));
    }

    #[test]
    fn skin_key() {
        assert_eq!(Config::from_toml("").unwrap().skin, "none");
        assert_eq!(Config::from_toml("skin = \"debug\"").unwrap().skin, "debug");
    }

    #[test]
    fn bad_color_only_loses_that_key() {
        let c = Config::from_toml(
            "scrollback_lines = 77\n[colors]\nforeground = \"nope\"\nbackground = \"#102030\"\nansi = [\"#000001\", \"zzz\", \"#000003\"]\ncursor = [\"#000000\"]\nurl = 5\n[cursor]\nshape = \"bar\"\n",
        )
        .unwrap();
        assert_eq!(c.colors.foreground, None);
        assert_eq!(c.colors.background, Some(Rgb(0x10, 0x20, 0x30)));
        assert_eq!(c.colors.ansi[0], Some(Rgb(0, 0, 1)));
        assert_eq!(c.colors.ansi[1], None);
        assert_eq!(c.colors.ansi[2], Some(Rgb(0, 0, 3)));
        assert_eq!(c.colors.cursor, None);
        assert_eq!(c.colors.url, None);
        // Everything else in the file is kept.
        assert_eq!(c.scrollback_lines, 77);
        assert_eq!(c.cursor.shape, CursorShapeCfg::Bar);
    }

    #[test]
    fn toml_syntax_error_is_still_whole_file_error() {
        assert!(Config::from_toml("[colors\nforeground = ").is_err());
        assert!(Config::from_toml("scrollback_lines = \"lots\"").is_err());
    }

    #[test]
    fn absurd_integers_are_clamped_not_rejected() {
        let c = Config::from_toml(
            "scrollback_lines = -4\n[window]\ncols = 100000\nrows = -2\n[bell]\nvisual_ms = 99999999999\n[font]\nsize = 1e9\n",
        )
        .unwrap();
        assert_eq!(c.scrollback_lines, 0);
        assert_eq!((c.window.cols, c.window.rows), (1000, 1));
        assert_eq!(c.bell.visual_ms, u32::MAX);
        assert_eq!(c.font.size, 200.0);
        let c = Config::from_toml("scrollback_lines = 9223372036854775807").unwrap();
        assert_eq!(c.scrollback_lines, 1_000_000);
    }

    #[test]
    fn example_file_parses() {
        let text = include_str!("../../../docs/grimalkin.example.toml");
        let c = Config::from_toml(text).unwrap();
        // A config created from the example must not pin colours: any
        // `[colors]` key beats the theme, which would make every theme
        // switch (menu, Preferences) invisible.
        assert_eq!(c.colors, ColorsConfig::default());
    }
}
