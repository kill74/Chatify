//! ANSI escape code utilities.
//!
//! Provides utilities for calculating terminal geometry correctly by ignoring
//! ANSI escape sequences.

use std::sync::OnceLock;
use regex::Regex;

/// Get the singleton regex for matching ANSI sequences
fn get_ansi_regex() -> &'static Regex {
    static ANSI_REGEX: OnceLock<Regex> = OnceLock::new();
    ANSI_REGEX.get_or_init(|| {
        Regex::new(r"\x1b\[[0-9;]*m").expect("Valid regex")
    })
}

/// Strips ANSI escape codes from a string
pub fn strip_ansi(input: &str) -> String {
    get_ansi_regex().replace_all(input, "").into_owned()
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
