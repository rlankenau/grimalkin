//! The data side of the Preferences window: which keys belong to which
//! section, the queue of pending config edits, and the lists the combo boxes
//! show (themes with their palettes, skins). No egui or winit in here, so it
//! is unit-tested.

use std::io;
use std::path::Path;
use std::time::{Duration, Instant};

use toml_edit::Value;

use crate::config::{BellConfig, Rgb};
use crate::config_edit::ConfigEditor;
use crate::{skin, theme};

/// Writes that arrive faster than this (slider drags) are held back and
/// written together, so the terminal re-applies the config ~10 times a second
/// at most while a slider moves.
pub const THROTTLE: Duration = Duration::from_millis(100);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Section {
    Appearance,
    Font,
    Cursor,
    Terminal,
    Advanced,
}

impl Section {
    pub const ALL: [Section; 5] = [Section::Appearance, Section::Font, Section::Cursor, Section::Terminal, Section::Advanced];

    pub fn label(self) -> &'static str {
        match self {
            Section::Appearance => "Appearance",
            Section::Font => "Font",
            Section::Cursor => "Cursor",
            Section::Terminal => "Terminal",
            Section::Advanced => "Advanced",
        }
    }

    /// The section `step` places after this one, wrapping around.
    pub fn cycle(self, step: i32) -> Section {
        let n = Section::ALL.len() as i32;
        let i = Section::ALL.iter().position(|s| *s == self).unwrap_or(0) as i32;
        Section::ALL[(i + step).rem_euclid(n) as usize]
    }

    /// The config keys "Reset section to defaults" removes.
    pub fn keys(self) -> &'static [&'static str] {
        match self {
            Section::Appearance => &["theme", "skin", "window.opacity", "window.padding"],
            Section::Font => &["font.family", "font.size", "font.line_height"],
            Section::Cursor => &["cursor.shape", "cursor.blink"],
            Section::Terminal => &[
                "scrollback_lines",
                "shell",
                "shell_args",
                "window.title",
                "window.dynamic_title",
                "bell.visual",
                "bell.visual_ms",
                "bell.audible",
                "input.option_as_alt",
            ],
            Section::Advanced => &[
                "pty.graphics_cursor",
                "pty.passthrough",
                "clipboard.osc52_write",
                "clipboard.osc52_read",
                "winamp_viscolor_palette",
            ],
        }
    }
}

/// A value a control writes.
#[derive(Clone, Debug, PartialEq)]
pub enum EditValue {
    Str(String),
    Float(f64),
    Int(i64),
    Bool(bool),
    StrList(Vec<String>),
}

impl From<EditValue> for Value {
    fn from(v: EditValue) -> Value {
        match v {
            EditValue::Str(s) => Value::from(s),
            EditValue::Float(f) => Value::from(f),
            EditValue::Int(i) => Value::from(i),
            EditValue::Bool(b) => Value::from(b),
            EditValue::StrList(l) => Value::from_iter(l),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Edit {
    Set(String, EditValue),
    Unset(String),
}

impl Edit {
    pub fn set(path: &str, v: EditValue) -> Edit {
        Edit::Set(path.to_string(), v)
    }
    /// A float rounded to 3 decimals, so `0.1 + 0.2` style noise never
    /// reaches the file.
    pub fn float(path: &str, v: f32) -> Edit {
        Edit::Set(path.to_string(), EditValue::Float((f64::from(v) * 1000.0).round() / 1000.0))
    }
    pub fn path(&self) -> &str {
        match self {
            Edit::Set(p, _) | Edit::Unset(p) => p,
        }
    }
}

/// Apply `edits` to the document, in order.
pub fn apply_edits(ed: &mut ConfigEditor, edits: &[Edit]) -> io::Result<()> {
    for e in edits {
        match e {
            Edit::Set(path, v) => ed.set(path, v.clone())?,
            Edit::Unset(path) => {
                ed.unset(path);
            }
        }
    }
    Ok(())
}

/// Edits waiting to be written. A later edit of the same key replaces the
/// earlier one.
#[derive(Default)]
pub struct EditQueue {
    edits: Vec<Edit>,
    last_flush: Option<Instant>,
}

impl EditQueue {
    pub fn push(&mut self, e: Edit) {
        self.edits.retain(|x| x.path() != e.path());
        self.edits.push(e);
    }

    pub fn is_empty(&self) -> bool {
        self.edits.is_empty()
    }

    /// Should the queue be written now? Always when `settled` (no drag in
    /// progress); during a drag at most once per [`THROTTLE`].
    pub fn due(&self, now: Instant, settled: bool) -> bool {
        !self.edits.is_empty() && (settled || self.last_flush.is_none_or(|t| now.duration_since(t) >= THROTTLE))
    }

    /// When a held-back queue becomes due (to schedule a repaint).
    pub fn due_at(&self) -> Option<Instant> {
        if self.edits.is_empty() {
            return None;
        }
        Some(self.last_flush.map_or_else(Instant::now, |t| t + THROTTLE))
    }

    /// Write everything to the config file at `path`. The file is re-read
    /// first, so edits made elsewhere in the meantime are not overwritten.
    /// The queue is emptied either way (an error is reported once, not
    /// retried every frame).
    pub fn flush(&mut self, path: &Path, now: Instant) -> io::Result<usize> {
        let edits = std::mem::take(&mut self.edits);
        self.last_flush = Some(now);
        if edits.is_empty() {
            return Ok(0);
        }
        let mut ed = ConfigEditor::open_at(path)?;
        apply_edits(&mut ed, &edits)?;
        ed.save()?;
        Ok(edits.len())
    }
}

/// How the bell behaves, as one choice instead of two switches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BellMode {
    Visual,
    Audible,
    Both,
    None,
}

impl BellMode {
    pub const ALL: [BellMode; 4] = [BellMode::Visual, BellMode::Audible, BellMode::Both, BellMode::None];

    pub fn of(b: &BellConfig) -> BellMode {
        match (b.visual, b.audible) {
            (true, false) => BellMode::Visual,
            (false, true) => BellMode::Audible,
            (true, true) => BellMode::Both,
            (false, false) => BellMode::None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            BellMode::Visual => "Visual flash",
            BellMode::Audible => "Sound",
            BellMode::Both => "Flash and sound",
            BellMode::None => "None",
        }
    }

    pub fn edits(self) -> [Edit; 2] {
        let (visual, audible) = match self {
            BellMode::Visual => (true, false),
            BellMode::Audible => (false, true),
            BellMode::Both => (true, true),
            BellMode::None => (false, false),
        };
        [Edit::set("bell.visual", EditValue::Bool(visual)), Edit::set("bell.audible", EditValue::Bool(audible))]
    }
}

/// A multi-line parse error (TOML prints the offending source line and a
/// caret) reduced to where it is and what is wrong, without the file path:
/// `TOML parse error at line 14, column 9: unknown variant ...`.
pub fn short_error(msg: &str) -> String {
    let mut lines = msg.lines().map(str::trim).filter(|l| !l.is_empty());
    let Some(first) = lines.next() else { return String::new() };
    // "<path>: TOML parse error ..." (the path may itself contain ": " on
    // Windows, so cut at the marker instead).
    let first = first.find("TOML parse error").map_or(first, |i| &first[i..]);
    match lines.next_back() {
        Some(last) if last != first && !last.starts_with('|') && !last.starts_with('^') => format!("{first}: {last}"),
        _ => first.to_string(),
    }
}

/// `"JetBrains Mono, Consolas"` to the family list.
pub fn parse_families(text: &str) -> Vec<String> {
    text.split(',').map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect()
}

/// The edit that stores `families` (a single family as a plain string, like
/// the example config; none at all removes the key to get the default list).
pub fn family_edit(families: &[String]) -> Edit {
    match families {
        [] => Edit::Unset("font.family".into()),
        [one] => Edit::set("font.family", EditValue::Str(one.clone())),
        many => Edit::set("font.family", EditValue::StrList(many.to_vec())),
    }
}

/// `font.family` with `primary` first and the old fallbacks after it.
pub fn with_primary_family(current: &[String], primary: &str) -> Vec<String> {
    let mut v = vec![primary.to_string()];
    v.extend(current.iter().skip(1).filter(|f| !f.eq_ignore_ascii_case(primary)).cloned());
    v
}

/// A theme for the combo box: its name and the colors of the preview strip.
#[derive(Clone, Debug, PartialEq)]
pub struct ThemeEntry {
    pub name: String,
    pub user: bool,
    pub foreground: Rgb,
    pub background: Rgb,
    pub ansi: [Rgb; 16],
}

pub fn theme_entries(user_dir: Option<&Path>) -> Vec<ThemeEntry> {
    theme::list(user_dir)
        .into_iter()
        .filter_map(|(name, src)| {
            let t = theme::load(&name, user_dir).ok()?;
            let p = theme::Palette::from_colors(&t.colors);
            Some(ThemeEntry {
                name,
                user: matches!(src, theme::Source::User(_)),
                foreground: p.foreground,
                background: p.background,
                ansi: p.ansi,
            })
        })
        .collect()
}

/// Does the theme `name` set its own font or cursor, which then wins over the
/// matching keys of the config file?
pub fn theme_overrides(name: &str, user_dir: Option<&Path>) -> (bool, bool) {
    match theme::load(name, user_dir) {
        Ok(t) => (
            t.font.family.is_some() || t.font.size.is_some() || t.font.line_height.is_some(),
            t.cursor.shape.is_some() || t.cursor.blink.is_some(),
        ),
        Err(_) => (false, false),
    }
}

/// One entry of the skin combo box.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkinOption {
    pub value: String,
    pub label: String,
}

/// Every skin choice in menu order (none, built-ins, user, WinAMP), with the
/// current value added when it is none of those (a path in the config).
pub fn skin_options(user_dir: Option<&Path>, current: &str) -> Vec<SkinOption> {
    let mut v: Vec<SkinOption> = skin::choices(user_dir)
        .into_iter()
        .flatten()
        .map(|(value, label)| SkinOption { value, label })
        .collect();
    if !v.iter().any(|o| skin::same_choice(current, &o.value)) {
        v.push(SkinOption { value: current.to_string(), label: current.to_string() });
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use std::fs;
    use std::path::PathBuf;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("grimalkin-prefs-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn queue_coalesces_and_throttles() {
        let mut q = EditQueue::default();
        let t0 = Instant::now();
        assert!(!q.due(t0, true), "nothing to write");
        q.push(Edit::float("font.size", 13.0));
        q.push(Edit::float("font.size", 14.5));
        q.push(Edit::set("cursor.blink", EditValue::Bool(true)));
        assert_eq!(q.edits.len(), 2);
        assert_eq!(q.edits[0], Edit::float("font.size", 14.5), "latest value of a key wins");
        assert!(q.due(t0, false), "first write of a drag goes out at once");
        q.last_flush = Some(t0);
        assert!(!q.due(t0 + Duration::from_millis(40), false), "held back during the drag");
        assert!(q.due(t0 + Duration::from_millis(100), false));
        assert!(q.due(t0 + Duration::from_millis(40), true), "released: written at once");
        assert_eq!(q.due_at(), Some(t0 + THROTTLE));
    }

    #[test]
    fn flush_writes_the_file_and_keeps_comments() {
        let d = tmp("flush");
        let f = d.join("grimalkin.toml");
        fs::write(&f, "# mine\nscrollback_lines = 500 # history\n[font]\nsize = 12.0\n").unwrap();
        let mut q = EditQueue::default();
        q.push(Edit::float("font.size", 15.5));
        q.push(Edit::set("scrollback_lines", EditValue::Int(2000)));
        q.push(family_edit(&["JetBrains Mono".into(), "Menlo".into()]));
        q.push(Edit::set("cursor.blink", EditValue::Bool(true)));
        assert_eq!(q.flush(&f, Instant::now()).unwrap(), 4);
        assert!(q.is_empty());
        let text = fs::read_to_string(&f).unwrap();
        assert!(text.starts_with("# mine\n"), "{text}");
        assert!(text.contains("# history"), "{text}");
        let c = Config::try_load(Some(&f)).unwrap();
        assert_eq!(c.font.size, 15.5);
        assert_eq!(c.scrollback_lines, 2000);
        assert_eq!(c.font.family, vec!["JetBrains Mono".to_string(), "Menlo".to_string()]);
        assert!(c.cursor.blink);
        // An external edit made in between survives the next flush.
        fs::write(&f, format!("{text}\n[window]\ntitle = \"hi\"\n")).unwrap();
        q.push(Edit::float("window.opacity", 0.9));
        q.flush(&f, Instant::now()).unwrap();
        let c = Config::try_load(Some(&f)).unwrap();
        assert_eq!(c.window.title, "hi");
        assert!((c.window.opacity - 0.9).abs() < 1e-6);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn floats_are_rounded_for_the_file() {
        let d = tmp("float");
        let f = d.join("grimalkin.toml");
        fs::write(&f, "").unwrap();
        let mut q = EditQueue::default();
        q.push(Edit::float("window.opacity", 0.1 + 0.2));
        q.flush(&f, Instant::now()).unwrap();
        assert!(fs::read_to_string(&f).unwrap().contains("opacity = 0.3\n"));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn reset_section_removes_exactly_its_keys() {
        let d = tmp("reset");
        let f = d.join("grimalkin.toml");
        fs::write(
            &f,
            "theme = \"nord\"\nscrollback_lines = 9\n[font]\nsize = 20.0\nline_height = 1.2\n[cursor]\nblink = true\n[window]\nopacity = 0.8\ncols = 90\n",
        )
        .unwrap();
        let mut q = EditQueue::default();
        for k in Section::Appearance.keys() {
            q.push(Edit::Unset((*k).into()));
        }
        q.flush(&f, Instant::now()).unwrap();
        let c = Config::try_load(Some(&f)).unwrap();
        assert_eq!(c.theme, "default");
        assert_eq!(c.window.opacity, 1.0);
        assert_eq!(c.window.cols, 90, "keys outside the section stay");
        assert_eq!(c.font.size, 20.0);
        assert_eq!(c.scrollback_lines, 9);
        assert!(c.cursor.blink);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn section_keys_are_valid_and_disjoint() {
        let mut seen = std::collections::HashSet::new();
        for s in Section::ALL {
            for k in s.keys() {
                assert!(seen.insert(*k), "{k} is in two sections");
            }
        }
        // Every listed key is accepted by the editor and by the config parser.
        let d = tmp("keys");
        let f = d.join("grimalkin.toml");
        fs::write(&f, "").unwrap();
        let mut ed = ConfigEditor::open_at(&f).unwrap();
        for k in seen {
            ed.set(k, 1).unwrap();
        }
    }

    #[test]
    fn parse_errors_are_shortened() {
        let e = "/tmp/x/grimalkin.toml: TOML parse error at line 14, column 9\n   |\n14 | shape = \"banana\"\n   |         ^^^^^^^^\n    unknown variant `banana`, expected one of `block`, `underline`, `bar`\n    ";
        assert_eq!(
            short_error(e),
            "TOML parse error at line 14, column 9: unknown variant `banana`, expected one of `block`, `underline`, `bar`"
        );
        assert_eq!(short_error("cannot read: denied"), "cannot read: denied");
        assert_eq!(short_error(""), "");
    }

    #[test]
    fn bell_mode_round_trips() {
        for m in BellMode::ALL {
            let [v, a] = m.edits();
            let mut b = BellConfig::default();
            if let Edit::Set(_, EditValue::Bool(x)) = v {
                b.visual = x;
            }
            if let Edit::Set(_, EditValue::Bool(x)) = a {
                b.audible = x;
            }
            assert_eq!(BellMode::of(&b), m);
        }
    }

    #[test]
    fn families_parse_and_compose() {
        assert_eq!(parse_families(" JetBrains Mono ,, Consolas,"), vec!["JetBrains Mono".to_string(), "Consolas".to_string()]);
        assert!(parse_families("  , ").is_empty());
        assert_eq!(family_edit(&[]), Edit::Unset("font.family".into()));
        assert_eq!(family_edit(&["A".into()]), Edit::set("font.family", EditValue::Str("A".into())));
        let cur = vec!["Menlo".to_string(), "Monaco".to_string(), "Fira".to_string()];
        assert_eq!(with_primary_family(&cur, "Fira"), vec!["Fira".to_string(), "Monaco".to_string()]);
        assert_eq!(with_primary_family(&[], "A"), vec!["A".to_string()]);
    }

    #[test]
    fn theme_entries_have_palettes() {
        let v = theme_entries(None);
        let nord = v.iter().find(|t| t.name == "nord").expect("nord is built in");
        assert!(!nord.user);
        assert_ne!(nord.background, nord.foreground);
        assert!(v.iter().any(|t| t.name == "default"));
        assert_eq!(theme_overrides("nord", None), (false, false));
        assert_eq!(theme_overrides("no-such-theme", None), (false, false));
    }

    #[test]
    fn skin_options_include_unknown_current_value() {
        let v = skin_options(None, "none");
        assert_eq!(v[0].value, "none");
        assert!(v.iter().any(|o| o.value == "llama"));
        assert!(!v.iter().any(|o| o.value == "debug"));
        let v = skin_options(None, "/skins/mine");
        assert_eq!(v.last().unwrap().value, "/skins/mine");
        assert_eq!(skin_options(None, "llama").len(), skin_options(None, "none").len());
    }
}
