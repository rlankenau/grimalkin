//! `CreatePseudoConsole` flag computation. Kept free of Win32 types (and built
//! under `cfg(test)`) so it can be unit-tested off Windows.

// CreatePseudoConsole flags (wincon.h). Not all are exported by windows-sys.
pub(crate) const PSEUDOCONSOLE_INHERIT_CURSOR: u32 = 0x1;
pub(crate) const PSEUDOCONSOLE_RESIZE_QUIRK: u32 = 0x2;
pub(crate) const PSEUDOCONSOLE_WIN32_INPUT_MODE: u32 = 0x4;

/// Bit 0x8 means two different things depending on the ConPTY:
///
/// * In-box `kernel32` ConPTY (Windows 11 24H2+): `PSEUDOCONSOLE_PASSTHROUGH_MODE`.
/// * Microsoft's redistributable ConPTY (sideloaded `conpty.dll`, NuGet
///   `Microsoft.Windows.Console.ConPTY` 1.24+): the low bits of the
///   `PSEUDOCONSOLE_GLYPH_WIDTH_*` field (mask 0x18: GRAPHEMES 0x08,
///   WCSWIDTH 0x10, CONSOLE 0x18; `AMBIGUOUS_IS_WIDE` is 0x20), so 0x8 would
///   silently switch the glyph-width mode to GRAPHEMES. There, 0x2 and 0x4 are
///   unused (and ignored).
pub(crate) const PSEUDOCONSOLE_PASSTHROUGH_MODE: u32 = 0x8;

/// Flags every pseudoconsole gets.
///
/// `PSEUDOCONSOLE_WIN32_INPUT_MODE` (0x4) is deliberately NOT part of this.
/// In the ConPTY sources (microsoft/terminal `winconpty.cpp`, `VtIo.cpp`) that
/// flag only makes conhost write `CSI ? 9001 h` to the terminal, asking it to
/// send every key as a win32-input-mode record
/// (`CSI Vk;Sc;Uc;Kd;Cs;Rc _`, spec #4999). A terminal that does not encode
/// those records must not ask for them, so it is requested only when
/// [`pseudoconsole_flags`] is told the terminal does encode them
/// (`pty.win32_input_mode`). Without it conhost's VT input parser handles raw
/// bytes the classic way: a lone `0x03` becomes a Ctrl+C key record
/// (`InputStateMachineEngine::_DoControlCharacter`).
pub(crate) const BASE_FLAGS: u32 = PSEUDOCONSOLE_INHERIT_CURSOR | PSEUDOCONSOLE_RESIZE_QUIRK;

/// Flags for `CreatePseudoConsole`. Passthrough is only ever requested from the
/// in-box ConPTY; a sideloaded `conpty.dll` already forwards unknown sequences
/// and would misread 0x8 as `GLYPH_WIDTH_GRAPHEMES`. `win32_input_mode` adds
/// 0x4 and must only be set when the caller encodes win32-input-mode keys.
pub(crate) fn pseudoconsole_flags(sideloaded: bool, passthrough: bool, win32_input_mode: bool) -> u32 {
    let mut flags = BASE_FLAGS;
    if win32_input_mode {
        flags |= PSEUDOCONSOLE_WIN32_INPUT_MODE;
    }
    if passthrough && !sideloaded {
        flags |= PSEUDOCONSOLE_PASSTHROUGH_MODE;
    }
    flags
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags() {
        assert_eq!(pseudoconsole_flags(false, false, false), 0x3);
        assert_eq!(pseudoconsole_flags(false, true, false), 0xB);
        // Sideloaded: 0x8 must never be set, whatever was requested.
        assert_eq!(pseudoconsole_flags(true, false, false), 0x3);
        assert_eq!(pseudoconsole_flags(true, true, false), 0x3);
        assert_eq!(pseudoconsole_flags(true, true, false) & 0x18, 0);
    }

    #[test]
    fn win32_input_mode_is_opt_in() {
        // Not requested by default: grimalkin only asks conhost to request
        // win32-input-mode when it encodes it.
        assert_eq!(BASE_FLAGS & PSEUDOCONSOLE_WIN32_INPUT_MODE, 0);
        for sideloaded in [false, true] {
            for passthrough in [false, true] {
                assert_eq!(pseudoconsole_flags(sideloaded, passthrough, false) & 0x4, 0);
                assert_eq!(pseudoconsole_flags(sideloaded, passthrough, true) & 0x4, 0x4);
            }
        }
        assert_eq!(pseudoconsole_flags(false, true, true), 0xF);
    }
}
