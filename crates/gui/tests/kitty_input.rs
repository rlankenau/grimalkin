//! Tests for the kitty keyboard protocol encoder and mouse reporting. The gui
//! crate is a binary, so the module under test is included by path.

#![allow(dead_code, unused_imports)]

#[path = "../src/input.rs"]
mod input;

use grimalkin_vt::{MouseEncoding, MouseMode};
use input::*;
use winit::keyboard::{Key, KeyCode, KeyLocation, ModifiersState, NamedKey, PhysicalKey};

const DIS: u8 = 1;
const TYPES: u8 = 2;
const ALT: u8 = 4;
const ALL: u8 = 8;
const TEXT: u8 = 16;

struct K {
    logical: Key,
    physical: KeyCode,
    text: Option<&'static str>,
    pressed: bool,
    repeat: bool,
    location: KeyLocation,
    unmodified: Option<char>,
}

fn named(n: NamedKey, code: KeyCode) -> K {
    K { logical: Key::Named(n), physical: code, text: None, pressed: true, repeat: false, location: KeyLocation::Standard, unmodified: None }
}

fn chr(s: &'static str, code: KeyCode) -> K {
    let c = s.chars().next().unwrap();
    K {
        logical: Key::Character(s.into()),
        physical: code,
        text: Some(s),
        pressed: true,
        repeat: false,
        location: KeyLocation::Standard,
        unmodified: c.to_lowercase().next(),
    }
}

impl K {
    fn release(mut self) -> K {
        self.pressed = false;
        self.text = None;
        self
    }
    fn repeat(mut self) -> K {
        self.repeat = true;
        self
    }
    fn text(mut self, t: Option<&'static str>) -> K {
        self.text = t;
        self
    }
    fn keypad(mut self) -> K {
        self.location = KeyLocation::Numpad;
        self
    }
    fn unmod(mut self, c: char) -> K {
        self.unmodified = Some(c);
        self
    }
}

fn run(flags: u8, k: &K, mods: ModifiersState) -> Option<String> {
    let inp = KeyInput {
        logical: &k.logical,
        physical: PhysicalKey::Code(k.physical),
        text: k.text,
        pressed: k.pressed,
        repeat: k.repeat,
        location: k.location,
        unmodified: k.unmodified,
        caps_lock: false,
        num_lock: false,
    };
    encode_kitty(flags, &inp, mods).map(|b| String::from_utf8(b).unwrap().replace('\x1b', "ESC "))
}

fn none() -> ModifiersState {
    ModifiersState::empty()
}
const CTRL: ModifiersState = ModifiersState::CONTROL;
const SHIFT: ModifiersState = ModifiersState::SHIFT;
const ALTM: ModifiersState = ModifiersState::ALT;

fn s(x: &str) -> Option<String> {
    Some(x.to_string())
}

#[test]
fn spec_examples_disambiguate() {
    // Escape key.
    assert_eq!(run(DIS, &named(NamedKey::Escape, KeyCode::Escape), none()), s("ESC [27u"));
    // ctrl+a
    assert_eq!(run(DIS, &chr("a", KeyCode::KeyA).text(None), CTRL), s("ESC [97;5u"));
    // ctrl+shift+a still reports the unshifted key.
    assert_eq!(run(DIS, &chr("A", KeyCode::KeyA).text(None), CTRL | SHIFT), s("ESC [97;6u"));
    // alt+a, shift+alt+a
    assert_eq!(run(DIS, &chr("a", KeyCode::KeyA), ALTM), s("ESC [97;3u"));
    assert_eq!(run(DIS, &chr("A", KeyCode::KeyA), ALTM | SHIFT), s("ESC [97;4u"));
    // Ctrl+Space, ctrl+[
    assert_eq!(run(DIS, &named(NamedKey::Space, KeyCode::Space).text(Some(" ")), CTRL), s("ESC [32;5u"));
    assert_eq!(run(DIS, &chr("[", KeyCode::BracketLeft), CTRL), s("ESC [91;5u"));
}

#[test]
fn text_stays_legacy_unless_all_keys() {
    // None => the legacy path produces the text.
    assert_eq!(run(DIS, &chr("a", KeyCode::KeyA), none()), None);
    assert_eq!(run(DIS, &chr("A", KeyCode::KeyA), SHIFT), None);
    assert_eq!(run(DIS | TYPES, &chr("a", KeyCode::KeyA).release(), none()), None);
    assert_eq!(run(DIS | ALL, &chr("a", KeyCode::KeyA), none()), s("ESC [97u"));
    assert_eq!(run(DIS | ALL, &chr("A", KeyCode::KeyA), SHIFT), s("ESC [97;2u"));
}

#[test]
fn enter_tab_backspace() {
    let enter = || named(NamedKey::Enter, KeyCode::Enter);
    let tab = || named(NamedKey::Tab, KeyCode::Tab);
    let bs = || named(NamedKey::Backspace, KeyCode::Backspace);
    // Plain: legacy bytes (None), release never reported.
    assert_eq!(run(DIS, &enter(), none()), None);
    assert_eq!(run(DIS, &tab(), none()), None);
    assert_eq!(run(DIS, &bs(), none()), None);
    assert_eq!(run(DIS | TYPES, &enter().release(), none()), None);
    // Modified: CSI u.
    assert_eq!(run(DIS, &tab(), SHIFT), s("ESC [9;2u"));
    assert_eq!(run(DIS, &enter(), SHIFT), s("ESC [13;2u"));
    assert_eq!(run(DIS, &bs(), CTRL), s("ESC [127;5u"));
    // All keys: always CSI u, releases too.
    assert_eq!(run(DIS | ALL, &enter(), none()), s("ESC [13u"));
    assert_eq!(run(DIS | ALL | TYPES, &enter().release(), none()), s("ESC [13;1:3u"));
}

#[test]
fn legacy_translate_unchanged_with_flags_zero() {
    // translate() with flags == 0 never reaches the kitty encoder.
    let logical = Key::Named(NamedKey::Enter);
    let inp = KeyInput {
        logical: &logical,
        physical: PhysicalKey::Code(KeyCode::Enter),
        text: None,
        pressed: true,
        repeat: false,
        location: KeyLocation::Standard,
        unmodified: None,
        caps_lock: false,
        num_lock: false,
    };
    let st = EncodeState { alt_is_meta: true, ..Default::default() };
    assert_eq!(translate(&inp, none(), &st), KeyAction::Send(b"\r".to_vec()));
    // With flags=1 plain Enter is still CR via the legacy fallthrough.
    let st = EncodeState { kitty_flags: DIS, alt_is_meta: true, ..Default::default() };
    assert_eq!(translate(&inp, none(), &st), KeyAction::Send(b"\r".to_vec()));
}

#[test]
fn functional_keys_and_event_types() {
    let up = || named(NamedKey::ArrowUp, KeyCode::ArrowUp);
    assert_eq!(run(DIS, &up(), none()), s("ESC [A"));
    assert_eq!(run(DIS, &up(), CTRL), s("ESC [1;5A"));
    // Event types: press has no suffix, repeat is :2, release :3.
    assert_eq!(run(DIS | TYPES, &up(), none()), s("ESC [A"));
    assert_eq!(run(DIS | TYPES, &up().repeat(), none()), s("ESC [1;1:2A"));
    assert_eq!(run(DIS | TYPES, &up().release(), none()), s("ESC [1;1:3A"));
    assert_eq!(run(DIS | TYPES, &up().release(), CTRL), s("ESC [1;5:3A"));
    // Without the flag, repeats look like presses and releases vanish.
    assert_eq!(run(DIS, &up().repeat(), none()), s("ESC [A"));
    assert_eq!(run(DIS, &up().release(), none()), None);
    // Tilde keys, F-keys.
    assert_eq!(run(DIS, &named(NamedKey::Delete, KeyCode::Delete), none()), s("ESC [3~"));
    assert_eq!(run(DIS, &named(NamedKey::PageUp, KeyCode::PageUp), SHIFT), s("ESC [5;2~"));
    assert_eq!(run(DIS, &named(NamedKey::F1, KeyCode::F1), none()), s("ESC [P"));
    assert_eq!(run(DIS, &named(NamedKey::F3, KeyCode::F3), none()), s("ESC [13~"));
    assert_eq!(run(DIS, &named(NamedKey::F3, KeyCode::F3), CTRL), s("ESC [13;5~"));
    assert_eq!(run(DIS, &named(NamedKey::F5, KeyCode::F5), none()), s("ESC [15~"));
    assert_eq!(run(DIS, &named(NamedKey::F12, KeyCode::F12), ALTM), s("ESC [24;3~"));
    assert_eq!(run(DIS, &named(NamedKey::F13, KeyCode::F13), none()), s("ESC [57376u"));
    assert_eq!(run(DIS, &named(NamedKey::F35, KeyCode::F35), none()), s("ESC [57398u"));
    assert_eq!(run(DIS, &named(NamedKey::Home, KeyCode::Home), none()), s("ESC [H"));
    assert_eq!(run(DIS, &named(NamedKey::End, KeyCode::End), SHIFT | CTRL), s("ESC [1;6F"));
    // DECCKM is irrelevant: always CSI.
    assert_eq!(run(DIS, &named(NamedKey::Escape, KeyCode::Escape).release(), none()), None);
    assert_eq!(run(DIS | TYPES, &named(NamedKey::Escape, KeyCode::Escape).release(), none()), s("ESC [27;1:3u"));
}

#[test]
fn keypad_keys() {
    let kp1 = || chr("1", KeyCode::Numpad1).keypad();
    // Keypad text with only flag 1 stays text.
    assert_eq!(run(DIS, &kp1(), none()), None);
    assert_eq!(run(DIS | ALL, &kp1(), none()), s("ESC [57400u"));
    assert_eq!(run(DIS | ALL | TEXT, &kp1(), none()), s("ESC [57400;;49u"));
    assert_eq!(run(DIS, &kp1(), CTRL), s("ESC [57400;5u"));
    let kp_left = named(NamedKey::ArrowLeft, KeyCode::Numpad4).keypad();
    assert_eq!(run(DIS, &kp_left, none()), s("ESC [57417u"));
    let kp_enter = named(NamedKey::Enter, KeyCode::NumpadEnter).keypad();
    assert_eq!(run(DIS | ALL, &kp_enter, none()), s("ESC [57414u"));
}

#[test]
fn modifier_keys_only_with_all_keys() {
    let lshift = || named(NamedKey::Shift, KeyCode::ShiftLeft);
    assert_eq!(run(DIS, &lshift(), SHIFT), None);
    assert_eq!(run(DIS | ALL, &lshift(), SHIFT), s("ESC [57441;2u"));
    assert_eq!(run(DIS | ALL | TYPES, &lshift().release(), none()), s("ESC [57441;1:3u"));
    let rctrl = named(NamedKey::Control, KeyCode::ControlRight);
    assert_eq!(run(DIS | ALL, &rctrl, CTRL), s("ESC [57448;5u"));
}

#[test]
fn alternate_keys_and_associated_text() {
    // Shifted key only: the base layout key '2' equals the key code.
    let at = chr("@", KeyCode::Digit2).unmod('2').text(Some("@"));
    assert_eq!(run(DIS | ALT, &at, CTRL | SHIFT), s("ESC [50:64;6u"));
    // Cyrillic ctrl+с (U+0441) reports base layout 'c' (99).
    let cyr = chr("с", KeyCode::KeyC).text(None);
    assert_eq!(run(DIS | ALT, &cyr, CTRL), s("ESC [1089::99;5u"));
    // Both shifted and base: Cyrillic shift+ctrl+с.
    let cyr_up = K { logical: Key::Character("С".into()), ..chr("С", KeyCode::KeyC) }.text(None).unmod('с');
    assert_eq!(run(DIS | ALT, &cyr_up, CTRL | SHIFT), s("ESC [1089:1057:99;6u"));
    // Without flag 4 there are no alternates.
    assert_eq!(run(DIS, &cyr, CTRL), s("ESC [1089;5u"));
    // Text field: shift+a -> CSI 97;2;65u
    assert_eq!(run(DIS | ALL | TEXT, &chr("A", KeyCode::KeyA), SHIFT), s("ESC [97;2;65u"));
    assert_eq!(run(DIS | ALL | TEXT, &chr("a", KeyCode::KeyA), none()), s("ESC [97;;97u"));
    // No text on release.
    assert_eq!(run(DIS | ALL | TEXT | TYPES, &chr("a", KeyCode::KeyA).release(), none()), s("ESC [97;1:3u"));
    // Flag 16 alone (no flag 8) never adds text.
    assert_eq!(run(DIS | TEXT, &chr("a", KeyCode::KeyA), CTRL), s("ESC [97;5u"));
}

#[test]
fn composed_text_without_key() {
    // A multi-codepoint grapheme has no single key code: reported with key 0.
    let k = K { logical: Key::Character("e\u{301}".into()), unmodified: None, ..chr("é", KeyCode::KeyE) }.text(Some("e\u{301}"));
    assert_eq!(run(DIS | ALL | TEXT, &k, none()), s("ESC [0;;101:769u"));
}

// -- mouse ------------------------------------------------------------------

fn mev(kind: MouseKind, button: MouseBtn, col: usize, row: usize, mods: ModifiersState) -> MouseEvent {
    MouseEvent { kind, button, col, row, px: (0.0, 0.0), mods }
}

fn menc(mode: MouseMode, enc: MouseEncoding, ev: MouseEvent) -> Option<String> {
    encode_mouse(mode, enc, &ev).map(|b| String::from_utf8_lossy(&b).replace('\x1b', "ESC "))
}

#[test]
fn mouse_sgr() {
    use MouseBtn::{Left, Middle, Right, WheelDown, WheelUp};
    use MouseKind::*;
    let sgr = MouseEncoding::Sgr;
    assert_eq!(menc(MouseMode::Press, sgr, mev(Press, Left, 0, 0, none())), s("ESC [<0;1;1M"));
    assert_eq!(menc(MouseMode::Press, sgr, mev(Release, Left, 9, 4, none())), s("ESC [<0;10;5m"));
    assert_eq!(menc(MouseMode::Press, sgr, mev(Press, Right, 299, 99, none())), s("ESC [<2;300;100M"));
    assert_eq!(menc(MouseMode::Press, sgr, mev(Press, Middle, 1, 1, CTRL)), s("ESC [<17;2;2M"));
    assert_eq!(menc(MouseMode::Press, sgr, mev(Press, Left, 1, 1, ALTM)), s("ESC [<8;2;2M"));
    // Wheel.
    assert_eq!(menc(MouseMode::Press, sgr, mev(Wheel, WheelUp, 2, 3, none())), s("ESC [<64;3;4M"));
    assert_eq!(menc(MouseMode::Press, sgr, mev(Wheel, WheelDown, 2, 3, none())), s("ESC [<65;3;4M"));
    // Motion by mode.
    assert_eq!(menc(MouseMode::Press, sgr, mev(Motion, Left, 2, 3, none())), Option::None);
    assert_eq!(menc(MouseMode::Drag, sgr, mev(Motion, MouseBtn::None, 2, 3, none())), Option::None);
    assert_eq!(menc(MouseMode::Drag, sgr, mev(Motion, Left, 2, 3, none())), s("ESC [<32;3;4M"));
    assert_eq!(menc(MouseMode::Any, sgr, mev(Motion, MouseBtn::None, 2, 3, none())), s("ESC [<35;3;4M"));
    assert_eq!(menc(MouseMode::None, sgr, mev(Press, Left, 0, 0, none())), Option::None);
}

#[test]
fn mouse_legacy_encodings() {
    use MouseBtn::{Left, Middle, Right, WheelDown, WheelUp};
    use MouseKind::*;
    // X10/normal: bytes are value + 32 (button) / + 33 (1-based coordinate + 32).
    let b = encode_mouse(MouseMode::Press, MouseEncoding::Default, &mev(Press, Left, 0, 0, none())).unwrap();
    assert_eq!(b, vec![0x1b, b'[', b'M', 32, 33, 33]);
    // Release is always button 3.
    let b = encode_mouse(MouseMode::Press, MouseEncoding::Default, &mev(Release, Right, 4, 5, none())).unwrap();
    assert_eq!(b, vec![0x1b, b'[', b'M', 35, 37, 38]);
    // Out of range coordinate (col 223 -> 256) cannot be encoded.
    assert_eq!(encode_mouse(MouseMode::Press, MouseEncoding::Default, &mev(Press, Left, 223, 0, none())), Option::None);
    assert!(encode_mouse(MouseMode::Press, MouseEncoding::Default, &mev(Press, Left, 222, 0, none())).is_some());
    // UTF-8: large coordinates become multibyte characters.
    let b = encode_mouse(MouseMode::Press, MouseEncoding::Utf8, &mev(Press, Left, 299, 0, none())).unwrap();
    assert_eq!(String::from_utf8(b).unwrap(), format!("\x1b[M{}{}{}", ' ', char::from_u32(300 + 32).unwrap(), '!'));
    // urxvt.
    assert_eq!(menc(MouseMode::Press, MouseEncoding::Urxvt, mev(Press, Left, 9, 9, none())), s("ESC [32;10;10M"));
    assert_eq!(menc(MouseMode::Drag, MouseEncoding::Urxvt, mev(Motion, Left, 0, 0, SHIFT)), s("ESC [68;1;1M"));
}
