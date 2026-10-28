//! Keyboard (and wheel) input: winit events -> bytes for the pty or an app
//! level action (copy, paste, zoom, scrollback).
//!
//! This implements the legacy xterm-style encoding. Two extension points are
//! left for other agents, both documented where they are:
//!
//! * [`encode_kitty`]: the kitty keyboard protocol. [`translate`] calls it
//!   first whenever `Terminal::keyboard_flags()` is non-zero; returning
//!   `Some(bytes)` bypasses the legacy path entirely.
//! * [`encode_mouse`]: mouse reporting (modes 1000/1002/1003, encodings
//!   default/UTF-8/SGR/URXVT). `app.rs` offers every mouse event to it before
//!   the selection hooks and the wheel/scrollback handling.

use winit::event::KeyEvent;
use winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey, PhysicalKey};

/// Font zoom operations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ZoomOp {
    In,
    Out,
    Reset,
}

/// What a key press should do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyAction {
    /// Not handled / swallowed.
    None,
    /// Write these bytes to the pty.
    Send(Vec<u8>),
    Copy,
    Paste,
    Zoom(ZoomOp),
    /// Scroll the scrollback by this many pages (positive = older).
    ScrollPages(i32),
}

/// Terminal state that affects encoding.
#[derive(Clone, Copy, Debug, Default)]
pub struct EncodeState {
    /// DECCKM.
    pub app_cursor: bool,
    /// Kitty keyboard protocol flags (`Terminal::keyboard_flags()`).
    pub kitty_flags: u8,
    pub alt_screen: bool,
    /// Treat Alt as a Meta/ESC prefix (false on macOS when Option composes).
    pub alt_is_meta: bool,
}

/// The parts of a winit `KeyEvent` that the encoder needs (so it can be unit
/// tested; `KeyEvent` cannot be constructed outside winit).
#[derive(Clone, Debug)]
pub struct KeyInput<'a> {
    pub logical: &'a Key,
    pub physical: PhysicalKey,
    pub text: Option<&'a str>,
    pub pressed: bool,
    #[allow(dead_code)] // for the kitty keyboard agent (event types)
    pub repeat: bool,
}

impl<'a> KeyInput<'a> {
    pub fn from_event(ev: &'a KeyEvent) -> KeyInput<'a> {
        KeyInput {
            logical: &ev.logical_key,
            physical: ev.physical_key,
            text: ev.text.as_deref(),
            pressed: ev.state.is_pressed(),
            repeat: ev.repeat,
        }
    }
}

pub const IS_MAC: bool = cfg!(target_os = "macos");

/// Translate a key event. Application shortcuts are checked first, then the
/// kitty protocol hook, then the legacy encoding.
pub fn translate(input: &KeyInput, mods: ModifiersState, st: &EncodeState) -> KeyAction {
    if let Some(a) = shortcut(input, mods, st) {
        return a;
    }
    // Swallow unbound Cmd combinations on macOS instead of sending garbage.
    if IS_MAC && mods.super_key() {
        return KeyAction::None;
    }
    if st.kitty_flags != 0 {
        if let Some(bytes) = encode_kitty(st.kitty_flags, input, mods) {
            return KeyAction::Send(bytes);
        }
    }
    if !input.pressed {
        return KeyAction::None;
    }
    match encode_legacy(input.logical, input.text, mods, st) {
        Some(b) => KeyAction::Send(b),
        None => KeyAction::None,
    }
}

/// Is this physical key one of the "plus" keys (= with or without shift, or
/// the keypad +)?
fn is_zoom_in(code: KeyCode) -> bool {
    matches!(code, KeyCode::Equal | KeyCode::NumpadAdd)
}

fn is_zoom_out(code: KeyCode) -> bool {
    matches!(code, KeyCode::Minus | KeyCode::NumpadSubtract)
}

fn is_zoom_reset(code: KeyCode) -> bool {
    matches!(code, KeyCode::Digit0 | KeyCode::Numpad0)
}

fn shortcut(input: &KeyInput, mods: ModifiersState, st: &EncodeState) -> Option<KeyAction> {
    let PhysicalKey::Code(code) = input.physical else { return None };
    if !input.pressed {
        // Releases of shortcut keys are swallowed so the kitty protocol hook
        // never sees half of a shortcut.
        let is_shortcut_key = matches!(code, KeyCode::KeyC | KeyCode::KeyV)
            || is_zoom_in(code)
            || is_zoom_out(code)
            || is_zoom_reset(code);
        let combo = (mods.control_key() && mods.shift_key()) || (IS_MAC && mods.super_key());
        return (is_shortcut_key && combo).then_some(KeyAction::None);
    }
    let ctrl_shift = mods.control_key() && mods.shift_key() && !mods.alt_key() && !mods.super_key();
    let cmd = IS_MAC && mods.super_key() && !mods.control_key() && !mods.alt_key();
    if ctrl_shift || cmd {
        match code {
            KeyCode::KeyC => return Some(KeyAction::Copy),
            KeyCode::KeyV => return Some(KeyAction::Paste),
            c if is_zoom_in(c) => return Some(KeyAction::Zoom(ZoomOp::In)),
            c if is_zoom_out(c) => return Some(KeyAction::Zoom(ZoomOp::Out)),
            c if is_zoom_reset(c) => return Some(KeyAction::Zoom(ZoomOp::Reset)),
            _ => {}
        }
    }
    if mods.shift_key() && !mods.control_key() && !mods.alt_key() && !mods.super_key() {
        match code {
            KeyCode::Insert => return Some(KeyAction::Paste),
            KeyCode::PageUp if !st.alt_screen => return Some(KeyAction::ScrollPages(1)),
            KeyCode::PageDown if !st.alt_screen => return Some(KeyAction::ScrollPages(-1)),
            _ => {}
        }
    }
    None
}

/// Kitty keyboard protocol encoder.
///
/// TODO(kitty keyboard agent): implement https://sw.kovidgoyal.net/kitty/keyboard-protocol/
/// for the flags in `flags` (1 disambiguate, 2 report event types, 4 report
/// alternate keys, 8 report all keys as escape codes, 16 report associated
/// text). Called for press, repeat *and release* events whenever the terminal
/// has pushed non-zero flags. `mods` carries shift/ctrl/alt/super; use
/// `input.physical` for the unshifted key code and `input.text` for associated
/// text. Return `None` to fall back to the legacy encoding (which is correct
/// for keys the protocol leaves alone, e.g. plain text with flags == 1).
pub fn encode_kitty(_flags: u8, _input: &KeyInput, _mods: ModifiersState) -> Option<Vec<u8>> {
    None
}

/// A mouse event in terminal coordinates, for mouse reporting.
#[allow(dead_code)] // fields are consumed by the mouse reporting agent
#[derive(Clone, Copy, Debug)]
pub struct MouseEvent {
    pub kind: MouseKind,
    pub button: MouseBtn,
    /// 0-based cell coordinates.
    pub col: usize,
    pub row: usize,
    /// Pixel position within the terminal area.
    pub px: (f64, f64),
    pub mods: ModifiersState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseKind {
    Press,
    Release,
    Motion,
    /// `button` is WheelUp / WheelDown.
    Wheel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseBtn {
    Left,
    Middle,
    Right,
    WheelUp,
    WheelDown,
    /// Motion with no button held.
    None,
}

/// Mouse reporting encoder.
///
/// TODO(mouse agent): encode `ev` according to `mode`/`encoding` and return
/// the bytes to write to the pty, or `None` if the event should not be
/// reported (mouse mode off, motion not requested by the mode, ...). While
/// this returns `None` for everything, `app.rs` falls back to selection and
/// scrollback handling. By convention holding Shift bypasses reporting; the
/// caller checks that before calling.
pub fn encode_mouse(
    _mode: grimalkin_vt::MouseMode,
    _encoding: grimalkin_vt::MouseEncoding,
    _ev: &MouseEvent,
) -> Option<Vec<u8>> {
    None
}

fn xterm_mod_param(mods: ModifiersState) -> u8 {
    1 + mods.shift_key() as u8 + 2 * mods.alt_key() as u8 + 4 * mods.control_key() as u8
}

fn csi_with_mod(final_byte: u8, number: Option<u8>, param: u8) -> Vec<u8> {
    let mut v = b"\x1b[".to_vec();
    if param > 1 {
        v.extend_from_slice(number.unwrap_or(1).to_string().as_bytes());
        v.push(b';');
        v.extend_from_slice(param.to_string().as_bytes());
    } else if let Some(n) = number {
        v.extend_from_slice(n.to_string().as_bytes());
    }
    v.push(final_byte);
    v
}

fn tilde(number: u8, param: u8) -> Vec<u8> {
    csi_with_mod(b'~', Some(number), param)
}

fn ss3_or_csi(final_byte: u8, param: u8, app: bool) -> Vec<u8> {
    if param > 1 {
        csi_with_mod(final_byte, None, param)
    } else if app {
        vec![0x1b, b'O', final_byte]
    } else {
        vec![0x1b, b'[', final_byte]
    }
}

/// Control code for Ctrl+`c`, following xterm.
fn ctrl_code(c: char) -> Option<u8> {
    Some(match c {
        'a'..='z' => c as u8 - b'a' + 1,
        'A'..='Z' => c as u8 - b'A' + 1,
        '@' | ' ' | '2' => 0,
        '[' | '3' => 0x1b,
        '\\' | '4' => 0x1c,
        ']' | '5' => 0x1d,
        '^' | '6' => 0x1e,
        '_' | '/' | '7' => 0x1f,
        '?' | '8' => 0x7f,
        _ => return None,
    })
}

/// Legacy (xterm/VT) encoding of a key press.
pub fn encode_legacy(key: &Key, text: Option<&str>, mods: ModifiersState, st: &EncodeState) -> Option<Vec<u8>> {
    let altgr = cfg!(windows) && mods.control_key() && mods.alt_key();
    let ctrl = mods.control_key() && !altgr;
    let alt = mods.alt_key() && !altgr && st.alt_is_meta;
    let param = xterm_mod_param(ModifiersState::from_bits_truncate(
        (if mods.shift_key() { ModifiersState::SHIFT.bits() } else { 0 })
            | (if alt { ModifiersState::ALT.bits() } else { 0 })
            | (if ctrl { ModifiersState::CONTROL.bits() } else { 0 }),
    ));
    let with_esc = |mut v: Vec<u8>| {
        if alt {
            v.insert(0, 0x1b);
        }
        v
    };

    match key {
        Key::Named(n) => {
            let fkey = |num: u8| -> Vec<u8> { tilde(num, param) };
            let bytes = match n {
                NamedKey::Enter => return Some(with_esc(vec![b'\r'])),
                NamedKey::Tab => {
                    if mods.shift_key() && !ctrl {
                        return Some(with_esc(b"\x1b[Z".to_vec()));
                    }
                    return Some(with_esc(vec![b'\t']));
                }
                NamedKey::Backspace => {
                    return Some(with_esc(vec![if ctrl { 0x08 } else { 0x7f }]));
                }
                NamedKey::Escape => return Some(with_esc(vec![0x1b])),
                NamedKey::Space => {
                    if ctrl {
                        return Some(with_esc(vec![0]));
                    }
                    return Some(with_esc(vec![b' ']));
                }
                NamedKey::ArrowUp => ss3_or_csi(b'A', param, st.app_cursor),
                NamedKey::ArrowDown => ss3_or_csi(b'B', param, st.app_cursor),
                NamedKey::ArrowRight => ss3_or_csi(b'C', param, st.app_cursor),
                NamedKey::ArrowLeft => ss3_or_csi(b'D', param, st.app_cursor),
                NamedKey::Home => ss3_or_csi(b'H', param, st.app_cursor),
                NamedKey::End => ss3_or_csi(b'F', param, st.app_cursor),
                NamedKey::Insert => fkey(2),
                NamedKey::Delete => fkey(3),
                NamedKey::PageUp => fkey(5),
                NamedKey::PageDown => fkey(6),
                NamedKey::F1 => ss3_or_csi(b'P', param, true),
                NamedKey::F2 => ss3_or_csi(b'Q', param, true),
                NamedKey::F3 => ss3_or_csi(b'R', param, true),
                NamedKey::F4 => ss3_or_csi(b'S', param, true),
                NamedKey::F5 => fkey(15),
                NamedKey::F6 => fkey(17),
                NamedKey::F7 => fkey(18),
                NamedKey::F8 => fkey(19),
                NamedKey::F9 => fkey(20),
                NamedKey::F10 => fkey(21),
                NamedKey::F11 => fkey(23),
                NamedKey::F12 => fkey(24),
                NamedKey::F13 => fkey(25),
                NamedKey::F14 => fkey(26),
                NamedKey::F15 => fkey(28),
                NamedKey::F16 => fkey(29),
                NamedKey::F17 => fkey(31),
                NamedKey::F18 => fkey(32),
                NamedKey::F19 => fkey(33),
                NamedKey::F20 => fkey(34),
                _ => {
                    // Other named keys (media keys, modifiers, ...) may still
                    // carry text (e.g. keypad on some platforms).
                    return text_bytes(text, alt);
                }
            };
            Some(bytes)
        }
        Key::Character(s) => {
            if ctrl {
                let mut chars = s.chars();
                if let (Some(c), None) = (chars.next(), chars.next()) {
                    if let Some(code) = ctrl_code(c) {
                        return Some(with_esc(vec![code]));
                    }
                }
            }
            match text {
                Some(t) if !t.is_empty() => text_bytes(Some(t), alt),
                _ => text_bytes(Some(s.as_str()), alt),
            }
        }
        Key::Dead(_) | Key::Unidentified(_) => None,
    }
}

fn text_bytes(text: Option<&str>, alt: bool) -> Option<Vec<u8>> {
    let t = text?;
    if t.is_empty() || t.chars().any(|c| c.is_control()) {
        return None;
    }
    let mut v = Vec::with_capacity(t.len() + 1);
    if alt {
        v.push(0x1b);
    }
    v.extend_from_slice(t.as_bytes());
    Some(v)
}

/// Bytes to send for a paste. Newlines become CR (what Enter sends). Control
/// characters other than tab and newline (ESC, Ctrl+C, the C1 range, ...) are
/// dropped so pasted text cannot inject key presses or end the bracketed
/// paste early; in bracketed mode the text is wrapped in the paste markers.
pub fn paste_bytes(text: &str, bracketed: bool) -> Vec<u8> {
    let mut s = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                s.push('\r');
            }
            '\n' => s.push('\r'),
            '\t' => s.push('\t'),
            c if c.is_control() => {}
            _ => s.push(c),
        }
    }
    if bracketed {
        let mut v = b"\x1b[200~".to_vec();
        v.extend_from_slice(s.as_bytes());
        v.extend_from_slice(b"\x1b[201~");
        v
    } else {
        s.into_bytes()
    }
}

/// The arrow key sequences sent for wheel scrolling in the alternate screen:
/// `lines` > 0 scrolls up (Up arrows), < 0 down. Three arrows per notch is
/// applied by the caller.
pub fn wheel_arrows(lines: i32, app_cursor: bool) -> Vec<u8> {
    let seq: &[u8] = match (lines > 0, app_cursor) {
        (true, true) => b"\x1bOA",
        (true, false) => b"\x1b[A",
        (false, true) => b"\x1bOB",
        (false, false) => b"\x1b[B",
    };
    seq.repeat(lines.unsigned_abs() as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st() -> EncodeState {
        EncodeState {
            alt_is_meta: true,
            ..Default::default()
        }
    }

    fn ch(s: &str) -> Key {
        Key::Character(s.into())
    }

    fn enc(key: Key, text: Option<&str>, mods: ModifiersState, st: &EncodeState) -> Option<Vec<u8>> {
        encode_legacy(&key, text, mods, st)
    }

    #[test]
    fn printable_and_utf8() {
        let m = ModifiersState::empty();
        assert_eq!(enc(ch("a"), Some("a"), m, &st()).unwrap(), b"a");
        assert_eq!(enc(ch("é"), Some("é"), m, &st()).unwrap(), "é".as_bytes());
        assert_eq!(enc(ch("A"), Some("A"), ModifiersState::SHIFT, &st()).unwrap(), b"A");
    }

    #[test]
    fn simple_named() {
        let m = ModifiersState::empty();
        assert_eq!(enc(Key::Named(NamedKey::Enter), None, m, &st()).unwrap(), b"\r");
        assert_eq!(enc(Key::Named(NamedKey::Tab), None, m, &st()).unwrap(), b"\t");
        assert_eq!(enc(Key::Named(NamedKey::Tab), None, ModifiersState::SHIFT, &st()).unwrap(), b"\x1b[Z");
        assert_eq!(enc(Key::Named(NamedKey::Backspace), None, m, &st()).unwrap(), b"\x7f");
        assert_eq!(enc(Key::Named(NamedKey::Escape), None, m, &st()).unwrap(), b"\x1b");
        assert_eq!(enc(Key::Named(NamedKey::Space), Some(" "), m, &st()).unwrap(), b" ");
    }

    #[test]
    fn arrows_honor_decckm() {
        let m = ModifiersState::empty();
        let mut s = st();
        assert_eq!(enc(Key::Named(NamedKey::ArrowUp), None, m, &s).unwrap(), b"\x1b[A");
        s.app_cursor = true;
        assert_eq!(enc(Key::Named(NamedKey::ArrowUp), None, m, &s).unwrap(), b"\x1bOA");
        assert_eq!(enc(Key::Named(NamedKey::Home), None, m, &s).unwrap(), b"\x1bOH");
        // Modifiers force CSI form.
        assert_eq!(
            enc(Key::Named(NamedKey::ArrowLeft), None, ModifiersState::CONTROL, &s).unwrap(),
            b"\x1b[1;5D"
        );
    }

    #[test]
    fn nav_and_function_keys() {
        let m = ModifiersState::empty();
        assert_eq!(enc(Key::Named(NamedKey::Delete), None, m, &st()).unwrap(), b"\x1b[3~");
        assert_eq!(enc(Key::Named(NamedKey::PageUp), None, m, &st()).unwrap(), b"\x1b[5~");
        assert_eq!(enc(Key::Named(NamedKey::Insert), None, m, &st()).unwrap(), b"\x1b[2~");
        assert_eq!(enc(Key::Named(NamedKey::F1), None, m, &st()).unwrap(), b"\x1bOP");
        assert_eq!(enc(Key::Named(NamedKey::F5), None, m, &st()).unwrap(), b"\x1b[15~");
        assert_eq!(enc(Key::Named(NamedKey::F12), None, m, &st()).unwrap(), b"\x1b[24~");
        assert_eq!(
            enc(Key::Named(NamedKey::F5), None, ModifiersState::SHIFT, &st()).unwrap(),
            b"\x1b[15;2~"
        );
        assert_eq!(
            enc(Key::Named(NamedKey::F1), None, ModifiersState::CONTROL, &st()).unwrap(),
            b"\x1b[1;5P"
        );
    }

    #[test]
    fn ctrl_and_alt() {
        assert_eq!(enc(ch("c"), Some("\u{3}"), ModifiersState::CONTROL, &st()).unwrap(), b"\x03");
        assert_eq!(enc(ch("C"), Some("C"), ModifiersState::CONTROL | ModifiersState::SHIFT, &st()).unwrap(), b"\x03");
        assert_eq!(enc(ch("["), None, ModifiersState::CONTROL, &st()).unwrap(), b"\x1b");
        assert_eq!(enc(Key::Named(NamedKey::Space), Some(" "), ModifiersState::CONTROL, &st()).unwrap(), b"\x00");
        assert_eq!(enc(ch("b"), Some("b"), ModifiersState::ALT, &st()).unwrap(), b"\x1bb");
        assert_eq!(enc(Key::Named(NamedKey::Enter), None, ModifiersState::ALT, &st()).unwrap(), b"\x1b\r");
        assert_eq!(
            enc(ch("b"), Some("b"), ModifiersState::CONTROL | ModifiersState::ALT, &st()).unwrap(),
            if cfg!(windows) { b"b".to_vec() } else { b"\x1b\x02".to_vec() }
        );
        let mut s = st();
        s.alt_is_meta = false;
        assert_eq!(enc(ch("∫"), Some("∫"), ModifiersState::ALT, &s).unwrap(), "∫".as_bytes());
    }

    #[test]
    fn shortcuts() {
        let s = st();
        let key = |code| KeyInput {
            logical: &Key::Unidentified(winit::keyboard::NativeKey::Unidentified),
            physical: PhysicalKey::Code(code),
            text: None,
            pressed: true,
            repeat: false,
        };
        let cs = ModifiersState::CONTROL | ModifiersState::SHIFT;
        assert_eq!(translate(&key(KeyCode::KeyC), cs, &s), KeyAction::Copy);
        assert_eq!(translate(&key(KeyCode::KeyV), cs, &s), KeyAction::Paste);
        assert_eq!(translate(&key(KeyCode::Equal), cs, &s), KeyAction::Zoom(ZoomOp::In));
        assert_eq!(translate(&key(KeyCode::Minus), cs, &s), KeyAction::Zoom(ZoomOp::Out));
        assert_eq!(translate(&key(KeyCode::Digit0), cs, &s), KeyAction::Zoom(ZoomOp::Reset));
        assert_eq!(
            translate(&key(KeyCode::PageUp), ModifiersState::SHIFT, &s),
            KeyAction::ScrollPages(1)
        );
        let mut alt = s;
        alt.alt_screen = true;
        assert_eq!(
            translate(&key(KeyCode::PageUp), ModifiersState::SHIFT, &alt),
            KeyAction::None // Unidentified logical key sends nothing
        );
    }

    #[test]
    fn paste() {
        assert_eq!(paste_bytes("a\nb\r\nc", false), b"a\rb\rc");
        assert_eq!(paste_bytes("hi\x1b[201~x", true), b"\x1b[200~hi[201~x\x1b[201~");
        // Control characters never reach the program, bracketed or not.
        assert_eq!(paste_bytes("a\x1b[31m\x03\x7fb\u{9b}c\td", false), b"a[31mbc\td");
        assert_eq!(paste_bytes("a\r\rb", false), b"a\r\rb");
    }

    #[test]
    fn wheel() {
        assert_eq!(wheel_arrows(3, false), b"\x1b[A\x1b[A\x1b[A");
        assert_eq!(wheel_arrows(-2, true), b"\x1bOB\x1bOB");
    }
}
