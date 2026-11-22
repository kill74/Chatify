//! ANSI escape code utilities.
//!
//! Provides utilities for calculating terminal geometry correctly by ignoring
//! ANSI escape sequences.

/// Removes all terminal control sequences and control characters from
/// untrusted text before it is written to a terminal.
///
/// This is intentionally stricter than `strip_ansi`: besides CSI/SGR codes it
/// removes OSC sequences (e.g. OSC 52 clipboard writes, OSC 8 hyperlinks),
/// escape sequences of every other kind, and bare C0/C1 control characters.
/// Only newline, carriage return and tab are preserved as harmless layout
/// characters.
pub fn sanitize_terminal(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\n' | '\r' | '\t' => out.push(ch),
            '\x1b' => {
                // ESC: consume the entire control sequence, including OSC
                // strings terminated by BEL or ST, and single-character Fe
                // escapes. If the sequence is truncated, the rest of the
                // input is discarded rather than emitting raw control bytes.
                match chars.peek().copied() {
                    Some('[') => {
                        chars.next();
                        // CSI: final byte in 0x40..=0x7E
                        for c in chars.by_ref() {
                            if ('\x40'..='\x7e').contains(&c) {
                                break;
                            }
                        }
                    }
                    Some(']') => {
                        chars.next();
                        // OSC: terminated by BEL or ST (ESC \\)
                        let mut prev_was_esc = false;
                        for c in chars.by_ref() {
                            if c == '\x07' {
                                break;
                            }
                            if prev_was_esc && c == '\\' {
                                break;
                            }
                            prev_was_esc = c == '\x1b';
                        }
                    }
                    Some(_) => {
                        chars.next();
                    }
                    None => break,
                }
            }
            c if c.is_control() => {
                // Drop all other C0/C1 controls (including NUL and DEL).
                continue;
            }
            c => out.push(c),
        }
    }
    out
}

/// Strips ANSI escape codes from a string.
///
/// Delegates to `sanitize_terminal` so that OSC sequences and bare control
/// characters are removed as well as SGR codes.
pub fn strip_ansi(input: &str) -> String {
    sanitize_terminal(input)
}

/// Computes the visual width of a string by stripping ANSI escape codes
/// and calculating the unicode width.
pub fn visible_width(input: &str) -> usize {
    let stripped = strip_ansi(input);
    // Ideally we'd use `unicode_width::UnicodeWidthStr::width` here for
    // exact terminal cells, but chars().count() is a reasonable approximation
    // that matches the original logic.
    stripped.chars().count()
}

#[cfg(test)]
mod tests {
    use super::{sanitize_terminal, visible_width};

    #[test]
    fn strips_osc_52_clipboard_payload() {
        let input = "hello\x1b]52;c;c2VjcmV0\x07world";
        assert_eq!(sanitize_terminal(input), "helloworld");
    }

    #[test]
    fn strips_csi_and_bare_controls_but_keeps_layout() {
        let input = "\x1b[31mred\x1b[0m\nnext\tcol\x00done";
        assert_eq!(sanitize_terminal(input), "red\nnext\tcoldone");
    }

    #[test]
    fn strips_osc_8_hyperlinks_and_incomplete_sequences() {
        assert_eq!(
            sanitize_terminal("a\x1b]8;;https://example.com\x1b\\link\x1b]8;;\x1b\\b"),
            "alinkb"
        );
        // Truncated escape at EOF is dropped, not emitted raw.
        assert_eq!(sanitize_terminal("safe\x1b"), "safe");
    }

    #[test]
    fn visible_width_ignores_control_sequences() {
        assert_eq!(visible_width("\x1b[31mabc\x1b[0m"), 3);
    }
}
