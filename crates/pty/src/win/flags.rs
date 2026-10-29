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

/// The flags portable-pty used, which grimalkin shipped with before this
/// backend existed: keep behavior identical unless passthrough is requested.
pub(crate) const BASE_FLAGS: u32 =
    PSEUDOCONSOLE_INHERIT_CURSOR | PSEUDOCONSOLE_RESIZE_QUIRK | PSEUDOCONSOLE_WIN32_INPUT_MODE;

/// Flags for `CreatePseudoConsole`. Passthrough is only ever requested from the
/// in-box ConPTY; a sideloaded `conpty.dll` already forwards unknown sequences
/// and would misread 0x8 as `GLYPH_WIDTH_GRAPHEMES`.
pub(crate) fn pseudoconsole_flags(sideloaded: bool, passthrough: bool) -> u32 {
    if passthrough && !sideloaded {
        BASE_FLAGS | PSEUDOCONSOLE_PASSTHROUGH_MODE
    } else {
        BASE_FLAGS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags() {
        assert_eq!(pseudoconsole_flags(false, false), 0x7);
        assert_eq!(pseudoconsole_flags(false, true), 0xF);
        // Sideloaded: 0x8 must never be set, whatever was requested.
        assert_eq!(pseudoconsole_flags(true, false), 0x7);
        assert_eq!(pseudoconsole_flags(true, true), 0x7);
        assert_eq!(pseudoconsole_flags(true, true) & 0x18, 0);
    }
}
