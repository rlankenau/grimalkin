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
use winit::keyboard::{Key, KeyCode, KeyLocation, ModifiersState, NamedKey, PhysicalKey};

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
    /// Cmd+, (macOS) / Ctrl+, (elsewhere): open the Preferences window.
    OpenPreferences,
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
    pub repeat: bool,
    /// Where on the keyboard the key is (keypad keys have their own codes in
    /// the kitty protocol).
    pub location: KeyLocation,
    /// The key with all modifiers (Shift, Ctrl, Alt, Caps) undone, when the
    /// platform can tell us (the kitty protocol's "unshifted key").
    pub unmodified: Option<char>,
    /// Lock modifier state; winit 0.30 does not report it, so these are only
    /// set by callers that track it themselves.
    pub caps_lock: bool,
    pub num_lock: bool,
}

impl<'a> KeyInput<'a> {
    pub fn from_event(ev: &'a KeyEvent) -> KeyInput<'a> {
        KeyInput {
            logical: &ev.logical_key,
            physical: ev.physical_key,
            text: ev.text.as_deref(),
            pressed: ev.state.is_pressed(),
            repeat: ev.repeat,
            location: ev.location,
            unmodified: unmodified_key(ev),
            caps_lock: false,
            num_lock: false,
        }
    }
}

/// The single character of `key_without_modifiers`, where winit offers it.
#[cfg(any(windows, target_os = "macos", all(unix, not(any(target_os = "android", target_os = "ios", target_os = "redox")))))]
fn unmodified_key(ev: &KeyEvent) -> Option<char> {
    use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
    match ev.key_without_modifiers() {
        Key::Character(s) => {
            let mut it = s.chars();
            match (it.next(), it.next()) {
                (Some(c), None) => Some(c),
                _ => None,
            }
        }
        _ => None,
    }
}

#[cfg(not(any(windows, target_os = "macos", all(unix, not(any(target_os = "android", target_os = "ios", target_os = "redox"))))))]
fn unmodified_key(_ev: &KeyEvent) -> Option<char> {
    None
}

pub const IS_MAC: bool = cfg!(target_os = "macos");

/// Translate a key event. Application shortcuts are checked first, then the
/// kitty protocol hook, then the legacy encoding.
pub fn translate(input: &KeyInput, mods: ModifiersState, st: &EncodeState) -> KeyAction {
    if let Some(a) = shortcut(input, mods, st) {
        return a;
    }
    if st.kitty_flags != 0 {
        if let Some(bytes) = encode_kitty(st.kitty_flags, input, kitty_mods(input, mods, st)) {
            return KeyAction::Send(bytes);
        }
    }
    // Swallow unbound Cmd combinations on macOS instead of sending garbage
    // (the kitty protocol can report Super, so it gets them above).
    if IS_MAC && mods.super_key() {
        return KeyAction::None;
    }
    if !input.pressed {
        return KeyAction::None;
    }
    match encode_legacy(input.logical, input.text, mods, st) {
        Some(b) => KeyAction::Send(b),
        None => KeyAction::None,
    }
}

/// Modifiers as the kitty encoder should see them: Alt that only composes
/// text (macOS Option without `option_as_alt`) and AltGr (reported by Windows
/// as Ctrl+Alt) are not modifiers when they produced printable text.
fn kitty_mods(input: &KeyInput, mods: ModifiersState, st: &EncodeState) -> ModifiersState {
    let printable = input.text.is_some_and(|t| !t.is_empty() && !t.chars().any(char::is_control));
    let mut m = mods;
    if printable && mods.alt_key() {
        if cfg!(windows) && mods.control_key() {
            m.remove(ModifiersState::ALT | ModifiersState::CONTROL);
        } else if !st.alt_is_meta {
            m.remove(ModifiersState::ALT);
        }
    }
    m
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

/// Ctrl+, off macOS (on macOS Cmd+, is the menu's accelerator and is matched
/// below as a Cmd shortcut so it also works when the menu is missing).
fn is_prefs_key(code: KeyCode, mods: ModifiersState) -> bool {
    code == KeyCode::Comma
        && if IS_MAC {
            mods.super_key() && !mods.control_key() && !mods.alt_key() && !mods.shift_key()
        } else {
            mods.control_key() && !mods.shift_key() && !mods.alt_key() && !mods.super_key()
        }
}

fn shortcut(input: &KeyInput, mods: ModifiersState, st: &EncodeState) -> Option<KeyAction> {
    let PhysicalKey::Code(code) = input.physical else { return None };
    if !input.pressed {
        // Releases of shortcut keys are swallowed so the kitty protocol hook
        // never sees half of a shortcut.
        if is_prefs_key(code, mods) {
            return Some(KeyAction::None);
        }
        let is_shortcut_key = matches!(code, KeyCode::KeyC | KeyCode::KeyV | KeyCode::Comma)
            || is_zoom_in(code)
            || is_zoom_out(code)
            || is_zoom_reset(code);
        let combo = (mods.control_key() && mods.shift_key()) || (IS_MAC && mods.super_key());
        return (is_shortcut_key && combo).then_some(KeyAction::None);
    }
    if is_prefs_key(code, mods) {
        return Some(KeyAction::OpenPreferences);
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

// ---------------------------------------------------------------------------
// Kitty keyboard protocol
// ---------------------------------------------------------------------------

/// Kitty keyboard protocol flag bits (`Terminal::keyboard_flags()`).
#[allow(dead_code)]
pub const KITTY_DISAMBIGUATE: u8 = 1;
pub const KITTY_EVENT_TYPES: u8 = 2;
pub const KITTY_ALTERNATE_KEYS: u8 = 4;
pub const KITTY_ALL_KEYS: u8 = 8;
pub const KITTY_ASSOCIATED_TEXT: u8 = 16;

/// Modifier bits as defined by the protocol (the wire value is `1 + bits`).
const K_SHIFT: u8 = 1;
const K_ALT: u8 = 2;
const K_CTRL: u8 = 4;
const K_SUPER: u8 = 8;
const K_CAPS_LOCK: u8 = 64;
const K_NUM_LOCK: u8 = 128;

const PRESS: u8 = 1;
const REPEAT: u8 = 2;
const RELEASE: u8 = 3;

/// A functional key: the number and the final byte (`u`, `~` or a letter).
/// Letter forms use number 1 (`CSI 1;mods A`, `1` omitted without modifiers).
type FKey = (u32, u8);

fn kitty_mod_bits(mods: ModifiersState) -> u8 {
    (mods.shift_key() as u8) * K_SHIFT
        + (mods.alt_key() as u8) * K_ALT
        + (mods.control_key() as u8) * K_CTRL
        + (mods.super_key() as u8) * K_SUPER
}

/// Modifier key events as (key number, modifier bit).
fn kitty_modifier_key(input: &KeyInput) -> Option<(u32, u8)> {
    let PhysicalKey::Code(c) = input.physical else { return None };
    Some(match c {
        KeyCode::ShiftLeft => (57441, K_SHIFT),
        KeyCode::ControlLeft => (57442, K_CTRL),
        KeyCode::AltLeft => (57443, K_ALT),
        KeyCode::SuperLeft => (57444, K_SUPER),
        KeyCode::ShiftRight => (57447, K_SHIFT),
        KeyCode::ControlRight => (57448, K_CTRL),
        KeyCode::AltRight => (57449, K_ALT),
        KeyCode::SuperRight => (57450, K_SUPER),
        _ => return None,
    })
}

/// Keypad key number from the physical key.
fn kitty_keypad_number(code: KeyCode) -> Option<u32> {
    Some(match code {
        KeyCode::Numpad0 => 57399,
        KeyCode::Numpad1 => 57400,
        KeyCode::Numpad2 => 57401,
        KeyCode::Numpad3 => 57402,
        KeyCode::Numpad4 => 57403,
        KeyCode::Numpad5 => 57404,
        KeyCode::Numpad6 => 57405,
        KeyCode::Numpad7 => 57406,
        KeyCode::Numpad8 => 57407,
        KeyCode::Numpad9 => 57408,
        KeyCode::NumpadDecimal => 57409,
        KeyCode::NumpadDivide => 57410,
        KeyCode::NumpadMultiply => 57411,
        KeyCode::NumpadSubtract => 57412,
        KeyCode::NumpadAdd => 57413,
        KeyCode::NumpadEnter => 57414,
        KeyCode::NumpadEqual => 57415,
        KeyCode::NumpadComma => 57416,
        _ => return None,
    })
}

/// Functional (non-text) keys.
fn kitty_functional_key(input: &KeyInput) -> Option<FKey> {
    let Key::Named(n) = input.logical else { return None };
    let kp = input.location == KeyLocation::Numpad;
    // Navigation keys with a legacy `CSI n ~` form; the keypad variants only
    // exist as `CSI n u`.
    let tilde = |legacy: u32, keypad: u32| if kp { (keypad, b'u') } else { (legacy, b'~') };
    let letter = |l: u8, keypad: u32| if kp { (keypad, b'u') } else { (1, l) };
    let k = match n {
        NamedKey::Escape => (27, b'u'),
        NamedKey::Enter => {
            if kp || matches!(input.physical, PhysicalKey::Code(KeyCode::NumpadEnter)) {
                (57414, b'u')
            } else {
                (13, b'u')
            }
        }
        NamedKey::Tab => (9, b'u'),
        NamedKey::Backspace => (127, b'u'),
        NamedKey::Insert => tilde(2, 57425),
        NamedKey::Delete => tilde(3, 57426),
        NamedKey::PageUp => tilde(5, 57421),
        NamedKey::PageDown => tilde(6, 57422),
        NamedKey::ArrowLeft => letter(b'D', 57417),
        NamedKey::ArrowRight => letter(b'C', 57418),
        NamedKey::ArrowUp => letter(b'A', 57419),
        NamedKey::ArrowDown => letter(b'B', 57420),
        NamedKey::Home => letter(b'H', 57423),
        NamedKey::End => letter(b'F', 57424),
        NamedKey::Clear if kp => (1, b'E'),
        NamedKey::F1 => (1, b'P'),
        NamedKey::F2 => (1, b'Q'),
        NamedKey::F3 => (13, b'~'),
        NamedKey::F4 => (1, b'S'),
        NamedKey::F5 => (15, b'~'),
        NamedKey::F6 => (17, b'~'),
        NamedKey::F7 => (18, b'~'),
        NamedKey::F8 => (19, b'~'),
        NamedKey::F9 => (20, b'~'),
        NamedKey::F10 => (21, b'~'),
        NamedKey::F11 => (23, b'~'),
        NamedKey::F12 => (24, b'~'),
        NamedKey::F13 => (57376, b'u'),
        NamedKey::F14 => (57377, b'u'),
        NamedKey::F15 => (57378, b'u'),
        NamedKey::F16 => (57379, b'u'),
        NamedKey::F17 => (57380, b'u'),
        NamedKey::F18 => (57381, b'u'),
        NamedKey::F19 => (57382, b'u'),
        NamedKey::F20 => (57383, b'u'),
        NamedKey::F21 => (57384, b'u'),
        NamedKey::F22 => (57385, b'u'),
        NamedKey::F23 => (57386, b'u'),
        NamedKey::F24 => (57387, b'u'),
        NamedKey::F25 => (57388, b'u'),
        NamedKey::F26 => (57389, b'u'),
        NamedKey::F27 => (57390, b'u'),
        NamedKey::F28 => (57391, b'u'),
        NamedKey::F29 => (57392, b'u'),
        NamedKey::F30 => (57393, b'u'),
        NamedKey::F31 => (57394, b'u'),
        NamedKey::F32 => (57395, b'u'),
        NamedKey::F33 => (57396, b'u'),
        NamedKey::F34 => (57397, b'u'),
        NamedKey::F35 => (57398, b'u'),
        NamedKey::CapsLock => (57358, b'u'),
        NamedKey::ScrollLock => (57359, b'u'),
        NamedKey::NumLock => (57360, b'u'),
        NamedKey::PrintScreen => (57361, b'u'),
        NamedKey::Pause => (57362, b'u'),
        NamedKey::ContextMenu => (57363, b'u'),
        NamedKey::MediaPlay => (57428, b'u'),
        NamedKey::MediaPause => (57429, b'u'),
        NamedKey::MediaPlayPause => (57430, b'u'),
        NamedKey::MediaStop => (57432, b'u'),
        NamedKey::MediaFastForward => (57433, b'u'),
        NamedKey::MediaRewind => (57434, b'u'),
        NamedKey::MediaTrackNext => (57435, b'u'),
        NamedKey::MediaTrackPrevious => (57436, b'u'),
        NamedKey::MediaRecord => (57437, b'u'),
        NamedKey::AudioVolumeDown => (57438, b'u'),
        NamedKey::AudioVolumeUp => (57439, b'u'),
        NamedKey::AudioVolumeMute => (57440, b'u'),
        NamedKey::AltGraph => (57453, b'u'),
        _ => return None,
    };
    Some(k)
}

/// The key on a PC-101 (US) layout at this physical position, if it is one of
/// the text-producing keys. This is the protocol's "base layout key".
fn base_layout_char(code: KeyCode) -> Option<char> {
    Some(match code {
        KeyCode::KeyA => 'a',
        KeyCode::KeyB => 'b',
        KeyCode::KeyC => 'c',
        KeyCode::KeyD => 'd',
        KeyCode::KeyE => 'e',
        KeyCode::KeyF => 'f',
        KeyCode::KeyG => 'g',
        KeyCode::KeyH => 'h',
        KeyCode::KeyI => 'i',
        KeyCode::KeyJ => 'j',
        KeyCode::KeyK => 'k',
        KeyCode::KeyL => 'l',
        KeyCode::KeyM => 'm',
        KeyCode::KeyN => 'n',
        KeyCode::KeyO => 'o',
        KeyCode::KeyP => 'p',
        KeyCode::KeyQ => 'q',
        KeyCode::KeyR => 'r',
        KeyCode::KeyS => 's',
        KeyCode::KeyT => 't',
        KeyCode::KeyU => 'u',
        KeyCode::KeyV => 'v',
        KeyCode::KeyW => 'w',
        KeyCode::KeyX => 'x',
        KeyCode::KeyY => 'y',
        KeyCode::KeyZ => 'z',
        KeyCode::Digit0 => '0',
        KeyCode::Digit1 => '1',
        KeyCode::Digit2 => '2',
        KeyCode::Digit3 => '3',
        KeyCode::Digit4 => '4',
        KeyCode::Digit5 => '5',
        KeyCode::Digit6 => '6',
        KeyCode::Digit7 => '7',
        KeyCode::Digit8 => '8',
        KeyCode::Digit9 => '9',
        KeyCode::Minus => '-',
        KeyCode::Equal => '=',
        KeyCode::BracketLeft => '[',
        KeyCode::BracketRight => ']',
        KeyCode::Backslash => '\\',
        KeyCode::Semicolon => ';',
        KeyCode::Quote => '\'',
        KeyCode::Backquote => '`',
        KeyCode::Comma => ',',
        KeyCode::Period => '.',
        KeyCode::Slash => '/',
        _ => return None,
    })
}

fn single_char(s: &str) -> Option<char> {
    let mut it = s.chars();
    match (it.next(), it.next()) {
        (Some(c), None) => Some(c),
        _ => None,
    }
}

fn lowercase_char(c: char) -> char {
    let mut it = c.to_lowercase();
    match (it.next(), it.next()) {
        (Some(l), None) => l,
        _ => c,
    }
}

fn uppercase_char(c: char) -> Option<char> {
    let mut it = c.to_uppercase();
    match (it.next(), it.next()) {
        (Some(u), None) if u != c => Some(u),
        _ => None,
    }
}

/// Printable text of the event (no C0/C1 control codes), if any.
fn printable_text<'a>(input: &KeyInput<'a>) -> Option<&'a str> {
    let t = input.text?;
    (!t.is_empty() && !t.chars().any(|c| c.is_control())).then_some(t)
}

/// Assemble `CSI number[:shifted[:base]] ; mods[:event] ; text terminator`.
fn kitty_csi(
    number: u32,
    term: u8,
    shifted: Option<u32>,
    base: Option<u32>,
    mod_bits: u8,
    event: u8,
    text: &[u32],
) -> Vec<u8> {
    let m = mod_bits as u32 + 1;
    let has_mods = m > 1 || event > PRESS;
    let mut s = String::from("\x1b[");
    // Letter forms omit the leading 1 when nothing else follows.
    if !(number == 1 && term != b'u' && term != b'~' && !has_mods && text.is_empty()) {
        s.push_str(&number.to_string());
    }
    if shifted.is_some() || base.is_some() {
        s.push(':');
        if let Some(c) = shifted {
            s.push_str(&c.to_string());
        }
        if let Some(c) = base {
            s.push(':');
            s.push_str(&c.to_string());
        }
    }
    if has_mods || !text.is_empty() {
        s.push(';');
        if has_mods {
            s.push_str(&m.to_string());
            if event > PRESS {
                s.push(':');
                s.push_str(&event.to_string());
            }
        }
    }
    if !text.is_empty() {
        s.push(';');
        let parts: Vec<String> = text.iter().map(|c| c.to_string()).collect();
        s.push_str(&parts.join(":"));
    }
    s.push(term as char);
    s.into_bytes()
}

/// Kitty keyboard protocol encoder
/// (<https://sw.kovidgoyal.net/kitty/keyboard-protocol/>).
///
/// Called for press, repeat and release events whenever the terminal has
/// non-zero flags. Returns `Some(bytes)` for events encoded as escape codes,
/// and `None` to fall back to the legacy path, which is correct for keys the
/// protocol leaves alone (plain text without "report all keys"; Enter, Tab
/// and Backspace without modifiers) and which drops release events.
pub fn encode_kitty(flags: u8, input: &KeyInput, mods: ModifiersState) -> Option<Vec<u8>> {
    let all = flags & KITTY_ALL_KEYS != 0;
    let types = flags & KITTY_EVENT_TYPES != 0;
    let alternates = flags & KITTY_ALTERNATE_KEYS != 0;
    let assoc_text = flags & KITTY_ASSOCIATED_TEXT != 0;
    let event = if !input.pressed {
        RELEASE
    } else if input.repeat {
        REPEAT
    } else {
        PRESS
    };
    if event == RELEASE && !types {
        return None;
    }
    // Without event types, repeats look like presses.
    let event = if types { event } else { PRESS };

    let mut bits = kitty_mod_bits(mods);
    if all {
        if input.caps_lock {
            bits |= K_CAPS_LOCK;
        }
        if input.num_lock {
            bits |= K_NUM_LOCK;
        }
    }

    // Modifier keys themselves: only with "report all keys". The modifier
    // bits reflect the state after the event.
    if let Some((number, bit)) = kitty_modifier_key(input) {
        if !all {
            return None;
        }
        let b = if input.pressed { bits | bit } else { bits & !bit };
        return Some(kitty_csi(number, b'u', None, None, b, event, &[]));
    }

    if let Some((number, term)) = kitty_functional_key(input) {
        // Enter, Tab and Backspace stay legacy without modifiers (so `reset`
        // still works) and never report releases; with modifiers they use
        // CSI u.
        let legacy_key = term == b'u' && matches!(number, 13 | 9 | 127 | 57414);
        if legacy_key && !all && (event == RELEASE || bits == 0) {
            return None;
        }
        // Caps/Num lock are modifier-like: only with "report all keys".
        if matches!(number, 57358 | 57360) && !all {
            return None;
        }
        return Some(kitty_csi(number, term, None, None, bits, event, &[]));
    }

    // Text keys (and keypad text keys).
    let (logical_char, is_space) = match input.logical {
        Key::Character(s) => (single_char(s), false),
        Key::Named(NamedKey::Space) => (Some(' '), true),
        _ => return None,
    };
    let non_shift = mods.alt_key() || mods.control_key() || mods.super_key();
    if !all && !non_shift {
        // Plain and Shift+text is sent as text by the legacy path.
        return None;
    }

    let keypad_number = match (input.location, input.physical) {
        (KeyLocation::Numpad, PhysicalKey::Code(c)) => kitty_keypad_number(c),
        _ => None,
    };
    let unmodified = input
        .unmodified
        .or(logical_char.filter(|c| !c.is_control()))
        .map(lowercase_char);
    let (number, shifted, base) = if let Some(k) = keypad_number {
        (k, None, None)
    } else {
        let number = unmodified.map_or(0, |c| c as u32);
        let mut shifted = None;
        let mut base = None;
        if alternates {
            if mods.shift_key() && !is_space {
                // The platform's logical key may be altered by Alt (macOS
                // Option composes), so derive the shifted letter ourselves.
                let cand = match unmodified {
                    Some(u) if u.is_alphabetic() => uppercase_char(u),
                    _ => logical_char.filter(|c| !c.is_control()),
                };
                shifted = cand.filter(|c| Some(*c) != unmodified).map(|c| c as u32);
            }
            if let PhysicalKey::Code(c) = input.physical {
                base = base_layout_char(c).filter(|b| Some(*b) != unmodified).map(|b| b as u32);
            }
        }
        (number, shifted, base)
    };

    let mut text_cps: Vec<u32> = Vec::new();
    if all && assoc_text && event != RELEASE && !non_shift {
        if let Some(t) = printable_text(input) {
            text_cps = t.chars().map(|c| c as u32).collect();
        }
    }
    if number == 0 && text_cps.is_empty() {
        // Unknown key without text: nothing sensible to report.
        return None;
    }
    Some(kitty_csi(number, b'u', shifted, base, bits, event, &text_cps))
}

// ---------------------------------------------------------------------------
// Mouse reporting
// ---------------------------------------------------------------------------

/// A mouse event in terminal coordinates, for mouse reporting.
#[derive(Clone, Copy, Debug)]
pub struct MouseEvent {
    pub kind: MouseKind,
    pub button: MouseBtn,
    /// 0-based cell coordinates.
    pub col: usize,
    pub row: usize,
    /// Pixel position within the terminal area.
    #[allow(dead_code)] // for SGR-pixels (1016), not implemented
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

/// Mouse reporting encoder: bytes for the pty, or `None` if the event is not
/// reported (mouse mode off, motion not requested by the mode, coordinates
/// not representable in the legacy encoding, ...). Holding Shift bypasses
/// reporting; the caller checks that before calling.
pub fn encode_mouse(
    mode: grimalkin_vt::MouseMode,
    encoding: grimalkin_vt::MouseEncoding,
    ev: &MouseEvent,
) -> Option<Vec<u8>> {
    use grimalkin_vt::{MouseEncoding, MouseMode};
    let motion = ev.kind == MouseKind::Motion;
    match mode {
        MouseMode::None => return None,
        MouseMode::Press if motion => return None,
        MouseMode::Drag if motion && ev.button == MouseBtn::None => return None,
        _ => {}
    }
    let release = ev.kind == MouseKind::Release;
    let sgr = encoding == MouseEncoding::Sgr;
    let mut code: u32 = match ev.button {
        MouseBtn::Left => 0,
        MouseBtn::Middle => 1,
        MouseBtn::Right => 2,
        MouseBtn::None => 3,
        MouseBtn::WheelUp => 64,
        MouseBtn::WheelDown => 65,
    };
    if release && !sgr {
        // Legacy encodings do not say which button was released.
        code = 3;
    }
    if ev.mods.shift_key() {
        code += 4;
    }
    if ev.mods.alt_key() {
        code += 8;
    }
    if ev.mods.control_key() {
        code += 16;
    }
    if motion {
        code += 32;
    }
    let (x, y) = (ev.col as u32 + 1, ev.row as u32 + 1);
    let out = match encoding {
        MouseEncoding::Sgr => format!("\x1b[<{code};{x};{y}{}", if release { 'm' } else { 'M' }).into_bytes(),
        MouseEncoding::Urxvt => format!("\x1b[{};{x};{y}M", code + 32).into_bytes(),
        MouseEncoding::Utf8 => {
            let mut s = String::from("\x1b[M");
            for v in [code + 32, x + 32, y + 32] {
                // The protocol caps UTF-8 coordinates at 2015.
                s.push(char::from_u32(v.min(2047))?);
            }
            s.into_bytes()
        }
        MouseEncoding::Default => {
            let (b, cx, cy) = (code + 32, x + 32, y + 32);
            if b > 255 || cx > 255 || cy > 255 {
                return None;
            }
            vec![0x1b, b'[', b'M', b as u8, cx as u8, cy as u8]
        }
    };
    Some(out)
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
            location: KeyLocation::Standard,
            unmodified: None,
            caps_lock: false,
            num_lock: false,
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
        let prefs = if IS_MAC { ModifiersState::SUPER } else { ModifiersState::CONTROL };
        assert_eq!(translate(&key(KeyCode::Comma), prefs, &s), KeyAction::OpenPreferences);
        let mut rel = key(KeyCode::Comma);
        rel.pressed = false;
        assert_eq!(translate(&rel, prefs, &s), KeyAction::None);
        // Plain comma and Ctrl+Shift+, are not the shortcut.
        assert_ne!(translate(&key(KeyCode::Comma), ModifiersState::empty(), &s), KeyAction::OpenPreferences);
        assert_ne!(translate(&key(KeyCode::Comma), prefs | ModifiersState::SHIFT, &s), KeyAction::OpenPreferences);
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
