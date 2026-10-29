//! Helpers for text that comes from outside (file names, window titles, link
//! targets) and is shown to the user or printed to their terminal.

/// Bidirectional controls and line / paragraph separators: invisible, and
/// able to make text look like something else.
pub fn is_bidi_or_separator(c: char) -> bool {
    matches!(
        c,
        '\u{200e}' | '\u{200f}' | '\u{061c}' | '\u{2028}' | '\u{2029}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'
    )
}

/// True when `name` contains a control character (ESC, BEL, NUL, C1, ...) or
/// an invisible bidi / separator mark, i.e. anything that must not reach a
/// terminal or a menu label unescaped.
pub fn has_unsafe_chars(name: &str) -> bool {
    name.chars().any(|c| c.is_control() || is_bidi_or_separator(c))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_control_and_bidi() {
        assert!(has_unsafe_chars("x\x1b]0;PWNED\x07"));
        assert!(has_unsafe_chars("a\u{202e}b"));
        assert!(has_unsafe_chars("a\u{9b}b"));
        assert!(!has_unsafe_chars("plain \u{65e5}\u{672c}\u{8a9e}"));
    }
}
