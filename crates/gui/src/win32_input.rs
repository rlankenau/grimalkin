//! Win32 input mode (`CSI ? 9001 h`): keys as Windows `KEY_EVENT_RECORD`s.
//!
//! ConPTY asks for this mode (see `pty.win32_input_mode`). Each key press and
//! release is sent as
//!
//! ```text
//! CSI Vk ; Sc ; Uc ; Kd ; Cs ; Rc _
//! ```
//!
//! with `Vk` the virtual-key code, `Sc` the scan code, `Uc` the decimal
//! UTF-16 unit of the character (0 for none), `Kd` 1 for a press and 0 for a
//! release, `Cs` the `dwControlKeyState` bits and `Rc` the repeat count. This
//! is the microsoft/terminal spec #4999; the encoder always writes all six
//! fields, as conhost's own `TerminalInput::_makeWin32Output` does. Unlike the
//! legacy encoding it carries releases, modifier keys on their own, and
//! chords such as Ctrl+Shift+letter or Ctrl+Break that have no byte form.
//!
//! The mapping is from winit's physical key (`KeyCode`) to the US-layout
//! virtual key and set-1 scan code; the character comes from the logical key
//! or the event text.

use crate::input::KeyInput;
use winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey, PhysicalKey};

// dwControlKeyState bits (wincon.h).
pub const RIGHT_ALT_PRESSED: u32 = 0x0001;
pub const LEFT_ALT_PRESSED: u32 = 0x0002;
pub const RIGHT_CTRL_PRESSED: u32 = 0x0004;
pub const LEFT_CTRL_PRESSED: u32 = 0x0008;
pub const SHIFT_PRESSED: u32 = 0x0010;
pub const ENHANCED_KEY: u32 = 0x0100;

const VK_PACKET: u16 = 0xE7;
const VK_CANCEL: u16 = 0x03;

/// One Windows key record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Win32Key {
    /// `wVirtualKeyCode`.
    pub vk: u16,
    /// `wVirtualScanCode`.
    pub sc: u16,
    /// `uChar.UnicodeChar` (one UTF-16 unit).
    pub uc: u16,
    /// `bKeyDown`.
    pub down: bool,
    /// `dwControlKeyState`.
    pub cs: u32,
    /// `wRepeatCount`.
    pub rc: u16,
}

/// The `CSI Vk;Sc;Uc;Kd;Cs;Rc _` sequence for one key record.
pub fn encode(k: &Win32Key) -> Vec<u8> {
    format!("\x1b[{};{};{};{};{};{}_", k.vk, k.sc, k.uc, u8::from(k.down), k.cs, k.rc).into_bytes()
}

/// `(virtual key, scan code, enhanced)` for a physical key (US layout).
fn key_info(code: KeyCode) -> Option<(u16, u16, bool)> {
    use KeyCode::*;
    Some(match code {
        Escape => (0x1B, 1, false),
        Digit1 => (0x31, 2, false),
        Digit2 => (0x32, 3, false),
        Digit3 => (0x33, 4, false),
        Digit4 => (0x34, 5, false),
        Digit5 => (0x35, 6, false),
        Digit6 => (0x36, 7, false),
        Digit7 => (0x37, 8, false),
        Digit8 => (0x38, 9, false),
        Digit9 => (0x39, 10, false),
        Digit0 => (0x30, 11, false),
        Minus => (0xBD, 12, false),
        Equal => (0xBB, 13, false),
        Backspace => (0x08, 14, false),
        Tab => (0x09, 15, false),
        KeyQ => (0x51, 16, false),
        KeyW => (0x57, 17, false),
        KeyE => (0x45, 18, false),
        KeyR => (0x52, 19, false),
        KeyT => (0x54, 20, false),
        KeyY => (0x59, 21, false),
        KeyU => (0x55, 22, false),
        KeyI => (0x49, 23, false),
        KeyO => (0x4F, 24, false),
        KeyP => (0x50, 25, false),
        BracketLeft => (0xDB, 26, false),
        BracketRight => (0xDD, 27, false),
        Enter => (0x0D, 28, false),
        ControlLeft => (0x11, 29, false),
        KeyA => (0x41, 30, false),
        KeyS => (0x53, 31, false),
        KeyD => (0x44, 32, false),
        KeyF => (0x46, 33, false),
        KeyG => (0x47, 34, false),
        KeyH => (0x48, 35, false),
        KeyJ => (0x4A, 36, false),
        KeyK => (0x4B, 37, false),
        KeyL => (0x4C, 38, false),
        Semicolon => (0xBA, 39, false),
        Quote => (0xDE, 40, false),
        Backquote => (0xC0, 41, false),
        ShiftLeft => (0x10, 42, false),
        Backslash => (0xDC, 43, false),
        KeyZ => (0x5A, 44, false),
        KeyX => (0x58, 45, false),
        KeyC => (0x43, 46, false),
        KeyV => (0x56, 47, false),
        KeyB => (0x42, 48, false),
        KeyN => (0x4E, 49, false),
        KeyM => (0x4D, 50, false),
        Comma => (0xBC, 51, false),
        Period => (0xBE, 52, false),
        Slash => (0xBF, 53, false),
        ShiftRight => (0x10, 54, false),
        NumpadMultiply => (0x6A, 55, false),
        AltLeft => (0x12, 56, false),
        Space => (0x20, 57, false),
        CapsLock => (0x14, 58, false),
        F1 => (0x70, 59, false),
        F2 => (0x71, 60, false),
        F3 => (0x72, 61, false),
        F4 => (0x73, 62, false),
        F5 => (0x74, 63, false),
        F6 => (0x75, 64, false),
        F7 => (0x76, 65, false),
        F8 => (0x77, 66, false),
        F9 => (0x78, 67, false),
        F10 => (0x79, 68, false),
        NumLock => (0x90, 69, true),
        ScrollLock => (0x91, 70, false),
        Numpad7 => (0x67, 71, false),
        Numpad8 => (0x68, 72, false),
        Numpad9 => (0x69, 73, false),
        NumpadSubtract => (0x6D, 74, false),
        Numpad4 => (0x64, 75, false),
        Numpad5 => (0x65, 76, false),
        Numpad6 => (0x66, 77, false),
        NumpadAdd => (0x6B, 78, false),
        Numpad1 => (0x61, 79, false),
        Numpad2 => (0x62, 80, false),
        Numpad3 => (0x63, 81, false),
        Numpad0 => (0x60, 82, false),
        NumpadDecimal => (0x6E, 83, false),
        IntlBackslash => (0xE2, 86, false),
        F11 => (0x7A, 87, false),
        F12 => (0x7B, 88, false),
        F13 => (0x7C, 100, false),
        F14 => (0x7D, 101, false),
        F15 => (0x7E, 102, false),
        F16 => (0x7F, 103, false),
        F17 => (0x80, 104, false),
        F18 => (0x81, 105, false),
        F19 => (0x82, 106, false),
        F20 => (0x83, 107, false),
        F21 => (0x84, 108, false),
        F22 => (0x85, 109, false),
        F23 => (0x86, 110, false),
        F24 => (0x87, 118, false),
        // Extended (E0-prefixed) keys.
        NumpadEnter => (0x0D, 28, true),
        ControlRight => (0x11, 29, true),
        NumpadDivide => (0x6F, 53, true),
        PrintScreen => (0x2C, 55, true),
        AltRight => (0x12, 56, true),
        Pause => (0x13, 69, false),
        Home => (0x24, 71, true),
        ArrowUp => (0x26, 72, true),
        PageUp => (0x21, 73, true),
        ArrowLeft => (0x25, 75, true),
        ArrowRight => (0x27, 77, true),
        End => (0x23, 79, true),
        ArrowDown => (0x28, 80, true),
        PageDown => (0x22, 81, true),
        Insert => (0x2D, 82, true),
        Delete => (0x2E, 83, true),
        SuperLeft => (0x5B, 91, true),
        SuperRight => (0x5C, 92, true),
        ContextMenu => (0x5D, 93, true),
        _ => return None,
    })
}

/// The control-key-state flag a modifier key itself contributes.
fn own_modifier_flag(code: KeyCode) -> u32 {
    match code {
        KeyCode::ShiftLeft | KeyCode::ShiftRight => SHIFT_PRESSED,
        KeyCode::ControlLeft => LEFT_CTRL_PRESSED,
        KeyCode::ControlRight => RIGHT_CTRL_PRESSED,
        KeyCode::AltLeft => LEFT_ALT_PRESSED,
        KeyCode::AltRight => RIGHT_ALT_PRESSED,
        _ => 0,
    }
}

fn single_char(s: &str) -> Option<char> {
    let mut it = s.chars();
    match (it.next(), it.next()) {
        (Some(c), None) => Some(c),
        _ => None,
    }
}

/// The C0 code Windows produces for Ctrl+`c` (what `ToUnicode` returns).
fn ctrl_char(c: char) -> char {
    match c {
        'a'..='z' => (c as u8 - b'a' + 1) as char,
        'A'..='Z' => (c as u8 - b'A' + 1) as char,
        '[' => '\u{1b}',
        '\\' => '\u{1c}',
        ']' => '\u{1d}',
        // Windows turns Ctrl+Enter into LF and Ctrl+Backspace into DEL; and
        // leaves Ctrl+Space as a plain space.
        '\r' => '\n',
        '\u{8}' => '\u{7f}',
        other => other,
    }
}

/// The character Windows would put in the record: the event text (winit's
/// `text` has no Ctrl applied) or else the logical key, with Ctrl turned into
/// the C0 code. `None` for keys without a character.
fn record_char(input: &KeyInput, ctrl: bool) -> Option<char> {
    let c = match input.text.and_then(single_char) {
        Some(c) => c,
        None => match input.logical {
            Key::Character(s) => single_char(s)?,
            Key::Named(NamedKey::Space) => ' ',
            Key::Named(NamedKey::Enter) => '\r',
            Key::Named(NamedKey::Tab) => '\t',
            Key::Named(NamedKey::Backspace) => '\u{8}',
            Key::Named(NamedKey::Escape) => '\u{1b}',
            _ => return None,
        },
    };
    let convertible = !c.is_control() || matches!(c, '\r' | '\u{8}');
    Some(if ctrl && convertible { ctrl_char(c) } else { c })
}

/// The layout-aware virtual key for letter and digit keys: Windows reports
/// the virtual key of the character, not of the key position (Dvorak Ctrl+C
/// is `VK_C`), while the scan code stays physical.
fn layout_vk(input: &KeyInput) -> Option<u16> {
    let c = input.unmodified.or_else(|| match input.logical {
        Key::Character(s) => single_char(s),
        _ => None,
    })?;
    c.is_ascii_alphanumeric().then(|| c.to_ascii_uppercase() as u16)
}

/// The key records for one winit key event (press or release), as bytes.
/// `None` when the key has no Windows equivalent (nothing is sent).
pub fn encode_event(input: &KeyInput, mods: ModifiersState) -> Option<Vec<u8>> {
    let mut ctrl = mods.control_key();
    let mut alt = mods.alt_key();
    let shift = mods.shift_key();
    let mut cs = 0u32;
    let own = match input.physical {
        PhysicalKey::Code(c) => own_modifier_flag(c),
        PhysicalKey::Unidentified(_) => 0,
    };
    // Windows reports AltGr as Ctrl+RightAlt; a printable character produced
    // with both held is that, not a chord.
    let printable = input.text.and_then(single_char).is_some_and(|c| !c.is_control());
    let altgr = ctrl && alt && printable;
    if altgr {
        cs |= LEFT_CTRL_PRESSED | RIGHT_ALT_PRESSED;
        ctrl = false;
        alt = false;
    }
    if ctrl {
        cs |= LEFT_CTRL_PRESSED;
    }
    if alt {
        cs |= LEFT_ALT_PRESSED;
    }
    if shift {
        cs |= SHIFT_PRESSED;
    }
    // The winit modifier state can lag the key event that changes it, so the
    // modifier key's own flag follows its own press/release.
    if own != 0 {
        if input.pressed {
            cs |= own;
        } else {
            cs &= !own;
        }
    }

    let info = match input.physical {
        PhysicalKey::Code(code) => key_info(code),
        PhysicalKey::Unidentified(_) => None,
    };
    let Some((mut vk, sc, enhanced)) = info else {
        // No physical key (IME, layouts winit cannot map): deliver the text
        // as VK_PACKET records, the way Windows delivers synthesized input.
        return packet_events(input, cs);
    };
    if enhanced {
        cs |= ENHANCED_KEY;
    }
    // Ctrl+Pause is Break, which has its own virtual key.
    if vk == 0x13 && ctrl {
        vk = VK_CANCEL;
        cs |= ENHANCED_KEY;
    }
    // Dead-key results such as "´x" are several characters: one key press,
    // several records.
    if input.text.is_some_and(|t| t.chars().count() > 1) {
        return packet_events(input, cs);
    }
    if let Some(v) = layout_vk(input) {
        vk = v;
    }
    // Text that is more than one UTF-16 unit cannot be one record.
    let ch = record_char(input, ctrl);
    let uc = match ch {
        Some(c) if c.len_utf16() == 1 => c as u16,
        Some(_) => return packet_events(input, cs),
        None => 0,
    };
    Some(encode(&Win32Key { vk, sc, uc, down: input.pressed, cs, rc: 1 }))
}

/// `VK_PACKET` records (down then up per UTF-16 unit) for text with no key
/// mapping. Releases produce nothing, as the press already sent both halves.
fn packet_events(input: &KeyInput, cs: u32) -> Option<Vec<u8>> {
    if !input.pressed {
        return None;
    }
    let text = input.text.filter(|t| !t.is_empty()).or(match input.logical {
        Key::Character(s) => Some(s.as_str()),
        _ => None,
    })?;
    let mut out = Vec::new();
    for unit in text.encode_utf16() {
        for down in [true, false] {
            out.extend(encode(&Win32Key { vk: VK_PACKET, sc: 0, uc: unit, down, cs, rc: 1 }));
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use winit::keyboard::SmolStr;

    fn s(k: &Win32Key) -> String {
        String::from_utf8(encode(k)).unwrap()
    }

    fn key(vk: u16, sc: u16, uc: u16, down: bool, cs: u32) -> Win32Key {
        Win32Key { vk, sc, uc, down, cs, rc: 1 }
    }

    // Examples from microsoft/terminal doc/specs/#4999 (full forms).

    #[test]
    fn spec_ctrl_f1() {
        assert_eq!(s(&key(17, 29, 0, true, 8)), "\x1b[17;29;0;1;8;1_");
        assert_eq!(s(&key(112, 59, 0, true, 8)), "\x1b[112;59;0;1;8;1_");
        assert_eq!(s(&key(112, 59, 0, false, 8)), "\x1b[112;59;0;0;8;1_");
        assert_eq!(s(&key(17, 29, 0, false, 0)), "\x1b[17;29;0;0;0;1_");
    }

    #[test]
    fn spec_ctrl_alt_a_and_shift_a() {
        assert_eq!(s(&key(18, 56, 0, true, LEFT_CTRL_PRESSED | LEFT_ALT_PRESSED)), "\x1b[18;56;0;1;10;1_");
        assert_eq!(s(&key(65, 30, 0, true, 10)), "\x1b[65;30;0;1;10;1_");
        assert_eq!(s(&key(16, 42, 0, true, SHIFT_PRESSED)), "\x1b[16;42;0;1;16;1_");
        assert_eq!(s(&key(65, 30, 65, true, SHIFT_PRESSED)), "\x1b[65;30;65;1;16;1_");
        assert_eq!(s(&key(65, 30, 97, false, 0)), "\x1b[65;30;97;0;0;1_");
    }

    fn input<'a>(
        logical: &'a Key,
        code: KeyCode,
        text: Option<&'a str>,
        pressed: bool,
    ) -> KeyInput<'a> {
        KeyInput {
            logical,
            physical: PhysicalKey::Code(code),
            text,
            pressed,
            repeat: false,
            location: winit::keyboard::KeyLocation::Standard,
            unmodified: None,
            caps_lock: false,
            num_lock: false,
        }
    }

    fn ev(i: &KeyInput, mods: ModifiersState) -> String {
        String::from_utf8(encode_event(i, mods).unwrap()).unwrap()
    }

    #[test]
    fn ctrl_c_press_and_release() {
        let k = Key::Character(SmolStr::new("c"));
        let down = input(&k, KeyCode::KeyC, Some("c"), true);
        assert_eq!(ev(&down, ModifiersState::CONTROL), "\x1b[67;46;3;1;8;1_");
        // Release has no text; Ctrl is mapped from the logical key.
        let up = input(&k, KeyCode::KeyC, None, false);
        assert_eq!(ev(&up, ModifiersState::CONTROL), "\x1b[67;46;3;0;8;1_");
    }

    #[test]
    fn plain_and_shifted_letters() {
        let a = Key::Character(SmolStr::new("a"));
        assert_eq!(ev(&input(&a, KeyCode::KeyA, Some("a"), true), ModifiersState::empty()), "\x1b[65;30;97;1;0;1_");
        let up = Key::Character(SmolStr::new("A"));
        assert_eq!(ev(&input(&up, KeyCode::KeyA, Some("A"), true), ModifiersState::SHIFT), "\x1b[65;30;65;1;16;1_");
    }

    #[test]
    fn modifier_keys_carry_their_own_flag_even_if_state_lags() {
        let ctrl = Key::Named(NamedKey::Control);
        let down = input(&ctrl, KeyCode::ControlLeft, None, true);
        assert_eq!(ev(&down, ModifiersState::empty()), "\x1b[17;29;0;1;8;1_");
        let up = input(&ctrl, KeyCode::ControlLeft, None, false);
        assert_eq!(ev(&up, ModifiersState::CONTROL), "\x1b[17;29;0;0;0;1_");
        let rctrl = input(&ctrl, KeyCode::ControlRight, None, true);
        assert_eq!(ev(&rctrl, ModifiersState::CONTROL), "\x1b[17;29;0;1;268;1_");
    }

    #[test]
    fn function_navigation_and_ctrl_break() {
        let f1 = Key::Named(NamedKey::F1);
        assert_eq!(ev(&input(&f1, KeyCode::F1, None, true), ModifiersState::CONTROL), "\x1b[112;59;0;1;8;1_");
        let up = Key::Named(NamedKey::ArrowUp);
        assert_eq!(ev(&input(&up, KeyCode::ArrowUp, None, true), ModifiersState::empty()), "\x1b[38;72;0;1;256;1_");
        let pause = Key::Named(NamedKey::Pause);
        assert_eq!(ev(&input(&pause, KeyCode::Pause, None, true), ModifiersState::CONTROL), "\x1b[3;69;0;1;264;1_");
        let enter = Key::Named(NamedKey::Enter);
        assert_eq!(ev(&input(&enter, KeyCode::Enter, Some("\r"), true), ModifiersState::empty()), "\x1b[13;28;13;1;0;1_");
    }

    #[test]
    fn ctrl_space_and_ctrl_shift_letter() {
        let sp = Key::Named(NamedKey::Space);
        assert_eq!(ev(&input(&sp, KeyCode::Space, None, true), ModifiersState::CONTROL), "\x1b[32;57;32;1;8;1_");
        let k = Key::Character(SmolStr::new("A"));
        let m = ModifiersState::CONTROL | ModifiersState::SHIFT;
        assert_eq!(ev(&input(&k, KeyCode::KeyA, None, true), m), "\x1b[65;30;1;1;24;1_");
    }

    #[test]
    fn ctrl_is_applied_to_event_text() {
        let d = Key::Character(SmolStr::new("d"));
        assert_eq!(ev(&input(&d, KeyCode::KeyD, Some("d"), true), ModifiersState::CONTROL), "\x1b[68;32;4;1;8;1_");
        let enter = Key::Named(NamedKey::Enter);
        assert_eq!(ev(&input(&enter, KeyCode::Enter, Some("\r"), true), ModifiersState::CONTROL), "\x1b[13;28;10;1;8;1_");
    }

    #[test]
    fn virtual_key_follows_the_layout_but_scan_code_stays_physical() {
        // Dvorak: the key at the US "I" position types "c".
        let c = Key::Character(SmolStr::new("c"));
        assert_eq!(ev(&input(&c, KeyCode::KeyI, Some("c"), true), ModifiersState::CONTROL), "\x1b[67;23;3;1;8;1_");
    }

    #[test]
    fn multi_char_text_from_a_mapped_key_uses_packets() {
        let x = Key::Character(SmolStr::new("x"));
        let i = input(&x, KeyCode::KeyX, Some("´x"), true);
        assert_eq!(
            ev(&i, ModifiersState::empty()),
            "\x1b[231;0;180;1;0;1_\x1b[231;0;180;0;0;1_\x1b[231;0;120;1;0;1_\x1b[231;0;120;0;0;1_"
        );
    }

    #[test]
    fn altgr_is_ctrl_plus_right_alt() {
        let k = Key::Character(SmolStr::new("@"));
        let m = ModifiersState::CONTROL | ModifiersState::ALT;
        assert_eq!(ev(&input(&k, KeyCode::KeyQ, Some("@"), true), m), "\x1b[81;16;64;1;9;1_");
    }

    #[test]
    fn unmapped_text_becomes_packets() {
        let k = Key::Character(SmolStr::new("é"));
        let i = KeyInput {
            physical: PhysicalKey::Unidentified(winit::keyboard::NativeKeyCode::Unidentified),
            ..input(&k, KeyCode::KeyA, Some("é"), true)
        };
        assert_eq!(ev(&i, ModifiersState::empty()), "\x1b[231;0;233;1;0;1_\x1b[231;0;233;0;0;1_");
        let r = KeyInput { pressed: false, ..i };
        assert!(encode_event(&r, ModifiersState::empty()).is_none());
        // Non-BMP: one packet pair per UTF-16 unit.
        let emoji = Key::Character(SmolStr::new("😀"));
        let e = KeyInput {
            physical: PhysicalKey::Unidentified(winit::keyboard::NativeKeyCode::Unidentified),
            ..input(&emoji, KeyCode::KeyA, Some("😀"), true)
        };
        assert_eq!(String::from_utf8(encode_event(&e, ModifiersState::empty()).unwrap()).unwrap().matches('_').count(), 4);
    }
}
