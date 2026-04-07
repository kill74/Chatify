//! Customizable theme system for Chatify terminal UI.
//!
//! Provides a set of built-in theme presets and support for user-defined
//! custom themes. Each theme maps semantic UI roles (header, feed, sidebar,
//! etc.) to ANSI escape sequences.

/// ANSI reset sequence.
pub const RESET: &str = "\x1b[0m";

/// A complete theme mapping semantic color roles to ANSI escape sequences.
#[derive(Debug, Clone)]
pub struct Theme {
    pub name: &'static str,
    pub header: &'static str,
    pub subtitle: &'static str,
    pub feed_text: &'static str,
    pub sidebar_text: &'static str,
    pub hint: &'static str,
    pub dim: &'static str,
    pub accent: &'static str,
    pub border: &'static str,
    pub error: &'static str,
    pub success: &'static str,
}

// ---------------------------------------------------------------------------
// Built-in presets
// ---------------------------------------------------------------------------

const RETRO_GRID: Theme = Theme {
    name: "retro-grid",
    header: "\x1b[38;5;147m",
    subtitle: "\x1b[38;5;245m",
    feed_text: "\x1b[38;5;117m",
    sidebar_text: "\x1b[38;5;120m",
    hint: "\x1b[38;5;220m",
    dim: "\x1b[38;5;245m",
    accent: "\x1b[38;5;147m",
    border: "\x1b[38;5;240m",
    error: "\x1b[38;5;196m",
    success: "\x1b[38;5;82m",
};

const DARK: Theme = Theme {
    name: "dark",
    header: "\x1b[38;5;75m",
    subtitle: "\x1b[38;5;248m",
    feed_text: "\x1b[38;5;252m",
    sidebar_text: "\x1b[38;5;109m",
    hint: "\x1b[38;5;180m",
    dim: "\x1b[38;5;243m",
    accent: "\x1b[38;5;81m",
    border: "\x1b[38;5;239m",
    error: "\x1b[38;5;203m",
    success: "\x1b[38;5;114m",
};

const LIGHT: Theme = Theme {
    name: "light",
    header: "\x1b[38;5;25m",
    subtitle: "\x1b[38;5;242m",
    feed_text: "\x1b[38;5;236m",
    sidebar_text: "\x1b[38;5;24m",
    hint: "\x1b[38;5;130m",
    dim: "\x1b[38;5;244m",
    accent: "\x1b[38;5;31m",
    border: "\x1b[38;5;246m",
    error: "\x1b[38;5;160m",
    success: "\x1b[38;5;28m",
};

const SOLARIZED: Theme = Theme {
    name: "solarized",
    header: "\x1b[38;5;37m",
    subtitle: "\x1b[38;5;245m",
    feed_text: "\x1b[38;5;152m",
    sidebar_text: "\x1b[38;5;73m",
    hint: "\x1b[38;5;136m",
    dim: "\x1b[38;5;240m",
    accent: "\x1b[38;5;33m",
    border: "\x1b[38;5;240m",
    error: "\x1b[38;5;160m",
    success: "\x1b[38;5;64m",
};

const DRACULA: Theme = Theme {
    name: "dracula",
    header: "\x1b[38;5;141m",
    subtitle: "\x1b[38;5;248m",
    feed_text: "\x1b[38;5;253m",
    sidebar_text: "\x1b[38;5;117m",
    hint: "\x1b[38;5;228m",
    dim: "\x1b[38;5;243m",
    accent: "\x1b[38;5;212m",
    border: "\x1b[38;5;60m",
    error: "\x1b[38;5;203m",
    success: "\x1b[38;5;84m",
};

/// Registry of all built-in themes, sorted by name.
const BUILTIN_THEMES: &[Theme] = &[DARK, DRACULA, LIGHT, RETRO_GRID, SOLARIZED];

/// Returns the built-in theme matching `name`, case-insensitively.
pub fn find_builtin(name: &str) -> Option<&'static Theme> {
    let lower = name.to_lowercase();
    BUILTIN_THEMES.iter().find(|t| t.name == lower)
}

/// Returns the default theme.
pub fn default_theme() -> &'static Theme {
    find_builtin("retro-grid").expect("retro-grid preset must exist")
}

/// Lists all built-in theme names.
pub fn builtin_names() -> Vec<&'static str> {
    BUILTIN_THEMES.iter().map(|t| t.name).collect()
}

fn sanitize_ansi_256(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    if let Ok(code) = trimmed.parse::<u16>() {
        if code <= 255 {
            return Some(format!("\x1b[38;5;{}m", code));
        }
    }

    let explicit = trimmed
        .strip_prefix("\x1b[38;5;")
        .and_then(|v| v.strip_suffix('m'))?;
    let code = explicit.parse::<u16>().ok()?;
    if code <= 255 {
        Some(format!("\x1b[38;5;{}m", code))
    } else {
        None
    }
}

fn sanitize_or_fallback(raw: &str, fallback: &str) -> String {
    sanitize_ansi_256(raw).unwrap_or_else(|| fallback.to_string())
}

// ---------------------------------------------------------------------------
// Custom theme support
// ---------------------------------------------------------------------------

/// A user-defined custom theme loaded from config.toml.
#[derive(Debug, Clone)]
pub struct CustomTheme {
    pub name: String,
    pub header: String,
    pub subtitle: String,
    pub feed_text: String,
    pub sidebar_text: String,
    pub hint: String,
    pub dim: String,
    pub accent: String,
    pub border: String,
    pub error: String,
    pub success: String,
}

/// Owned variant of Theme that can hold either static or heap-allocated strings.
/// Used as the return type when a custom theme is resolved.
#[derive(Debug, Clone)]
pub struct OwnedTheme {
    pub name: String,
    pub header: String,
    pub subtitle: String,
    pub feed_text: String,
    pub sidebar_text: String,
    pub hint: String,
    pub dim: String,
    pub accent: String,
    pub border: String,
    pub error: String,
    pub success: String,
}

/// Parses an ANSI color escape sequence into a `ratatui::style::Color`.
///
/// Supports `\x1b[38;5;Nm` (256-color) and `\x1b[38;2;R;G;Bm` (true-color).
/// Falls back to `Color::Reset` for unrecognized sequences.
pub fn ansi_to_ratatui_color(ansi: &str) -> ratatui::style::Color {
    use ratatui::style::Color;

    let trimmed = ansi.trim();

    // Try 256-color: \x1b[38;5;Nm
    if let Some(rest) = trimmed.strip_prefix("\x1b[38;5;") {
        if let Some(code_str) = rest.strip_suffix('m') {
            if let Ok(code) = code_str.parse::<u8>() {
                return Color::Indexed(code);
            }
        }
    }

    // Try true-color: \x1b[38;2;R;G;Bm
    if let Some(rest) = trimmed.strip_prefix("\x1b[38;2;") {
        if let Some(code_str) = rest.strip_suffix('m') {
            let parts: Vec<&str> = code_str.split(';').collect();
            if parts.len() == 3 {
                if let (Ok(r), Ok(g), Ok(b)) = (
                    parts[0].parse::<u8>(),
                    parts[1].parse::<u8>(),
                    parts[2].parse::<u8>(),
                ) {
                    return Color::Rgb(r, g, b);
                }
            }
        }
    }

    // Try plain number (0-255) as 256-color index
    if let Ok(code) = trimmed.parse::<u8>() {
        return Color::Indexed(code);
    }

    Color::Reset
}

impl OwnedTheme {
    /// Creates an OwnedTheme from a built-in Theme by cloning the static strings.
    pub fn from_builtin(t: &Theme) -> Self {
        Self {
            name: t.name.to_string(),
            header: t.header.to_string(),
            subtitle: t.subtitle.to_string(),
            feed_text: t.feed_text.to_string(),
            sidebar_text: t.sidebar_text.to_string(),
            hint: t.hint.to_string(),
            dim: t.dim.to_string(),
            accent: t.accent.to_string(),
            border: t.border.to_string(),
            error: t.error.to_string(),
            success: t.success.to_string(),
        }
    }

    /// Creates an OwnedTheme from a custom theme config, wrapping ANSI codes.
    pub fn from_custom(ct: &CustomTheme) -> Self {
        let fallback = default_theme();
        Self {
            name: ct.name.clone(),
            header: sanitize_or_fallback(&ct.header, fallback.header),
            subtitle: sanitize_or_fallback(&ct.subtitle, fallback.subtitle),
            feed_text: sanitize_or_fallback(&ct.feed_text, fallback.feed_text),
            sidebar_text: sanitize_or_fallback(&ct.sidebar_text, fallback.sidebar_text),
            hint: sanitize_or_fallback(&ct.hint, fallback.hint),
            dim: sanitize_or_fallback(&ct.dim, fallback.dim),
            accent: sanitize_or_fallback(&ct.accent, fallback.accent),
            border: sanitize_or_fallback(&ct.border, fallback.border),
            error: sanitize_or_fallback(&ct.error, fallback.error),
            success: sanitize_or_fallback(&ct.success, fallback.success),
        }
    }

    /// Resolve a theme by name. Checks custom themes first, then built-ins.
    /// Falls back to retro-grid if nothing matches.
    pub fn resolve(name: &str, custom_themes: &[CustomTheme]) -> Self {
        // Check custom themes first
        let lower = name.to_lowercase();
        for ct in custom_themes {
            if ct.name.to_lowercase() == lower {
                return Self::from_custom(ct);
            }
        }
        // Fall back to built-in
        match find_builtin(name) {
            Some(t) => Self::from_builtin(t),
            None => Self::from_builtin(default_theme()),
        }
    }

    // --- ratatui color accessors ---
    pub fn header_color(&self) -> ratatui::style::Color {
        ansi_to_ratatui_color(&self.header)
    }
    pub fn subtitle_color(&self) -> ratatui::style::Color {
        ansi_to_ratatui_color(&self.subtitle)
    }
    pub fn feed_text_color(&self) -> ratatui::style::Color {
        ansi_to_ratatui_color(&self.feed_text)
    }
    pub fn sidebar_text_color(&self) -> ratatui::style::Color {
        ansi_to_ratatui_color(&self.sidebar_text)
    }
    pub fn hint_color(&self) -> ratatui::style::Color {
        ansi_to_ratatui_color(&self.hint)
    }
    pub fn dim_color(&self) -> ratatui::style::Color {
        ansi_to_ratatui_color(&self.dim)
    }
    pub fn accent_color(&self) -> ratatui::style::Color {
        ansi_to_ratatui_color(&self.accent)
    }
    pub fn border_color(&self) -> ratatui::style::Color {
        ansi_to_ratatui_color(&self.border)
    }
    pub fn error_color(&self) -> ratatui::style::Color {
        ansi_to_ratatui_color(&self.error)
    }
    pub fn success_color(&self) -> ratatui::style::Color {
        ansi_to_ratatui_color(&self.success)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_builtin_returns_theme() {
        let t = find_builtin("dark").unwrap();
        assert_eq!(t.name, "dark");
    }

    #[test]
    fn test_find_builtin_case_insensitive() {
        assert!(find_builtin("Dark").is_some());
        assert!(find_builtin("DRACULA").is_some());
    }

    #[test]
    fn test_find_builtin_unknown_returns_none() {
        assert!(find_builtin("nonexistent").is_none());
    }

    #[test]
    fn test_builtin_names_sorted() {
        let names = builtin_names();
        assert_eq!(names.len(), 5);
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted);
    }

    #[test]
    fn test_default_theme_is_retro_grid() {
        assert_eq!(default_theme().name, "retro-grid");
    }

    #[test]
    fn test_owned_theme_from_builtin() {
        let t = OwnedTheme::from_builtin(default_theme());
        assert_eq!(t.name, "retro-grid");
        assert!(t.header.contains("\x1b["));
    }

    #[test]
    fn test_owned_theme_from_custom_ansi_passthrough() {
        let ct = CustomTheme {
            name: "test".into(),
            header: "\x1b[38;5;99m".into(),
            subtitle: "\x1b[38;5;200m".into(),
            feed_text: "\x1b[38;5;100m".into(),
            sidebar_text: "\x1b[38;5;101m".into(),
            hint: "\x1b[38;5;102m".into(),
            dim: "\x1b[38;5;103m".into(),
            accent: "\x1b[38;5;104m".into(),
            border: "\x1b[38;5;105m".into(),
            error: "\x1b[38;5;106m".into(),
            success: "\x1b[38;5;107m".into(),
        };
        let t = OwnedTheme::from_custom(&ct);
        assert_eq!(t.header, "\x1b[38;5;99m");
    }

    #[test]
    fn test_owned_theme_from_custom_256_code() {
        let ct = CustomTheme {
            name: "test".into(),
            header: "99".into(),
            subtitle: "200".into(),
            feed_text: "100".into(),
            sidebar_text: "101".into(),
            hint: "102".into(),
            dim: "103".into(),
            accent: "104".into(),
            border: "105".into(),
            error: "106".into(),
            success: "107".into(),
        };
        let t = OwnedTheme::from_custom(&ct);
        assert_eq!(t.header, "\x1b[38;5;99m");
    }

    #[test]
    fn test_resolve_falls_back_to_default() {
        let t = OwnedTheme::resolve("nonexistent", &[]);
        assert_eq!(t.name, "retro-grid");
    }

    #[test]
    fn test_resolve_custom_over_builtin() {
        let custom = vec![CustomTheme {
            name: "dark".into(),
            header: "200".into(),
            subtitle: "201".into(),
            feed_text: "202".into(),
            sidebar_text: "203".into(),
            hint: "204".into(),
            dim: "205".into(),
            accent: "206".into(),
            border: "207".into(),
            error: "208".into(),
            success: "209".into(),
        }];
        let t = OwnedTheme::resolve("dark", &custom);
        assert_eq!(t.header, "\x1b[38;5;200m");
    }

    #[test]
    fn test_from_custom_invalid_escape_falls_back_to_default() {
        let ct = CustomTheme {
            name: "test".into(),
            header: "\x1b]52;c;AAAA".into(),
            subtitle: "200".into(),
            feed_text: "100".into(),
            sidebar_text: "101".into(),
            hint: "102".into(),
            dim: "103".into(),
            accent: "104".into(),
            border: "105".into(),
            error: "106".into(),
            success: "107".into(),
        };
        let t = OwnedTheme::from_custom(&ct);
        assert_eq!(t.header, default_theme().header);
    }

    #[test]
    fn test_from_custom_out_of_range_code_falls_back_to_default() {
        let ct = CustomTheme {
            name: "test".into(),
            header: "999".into(),
            subtitle: "200".into(),
            feed_text: "100".into(),
            sidebar_text: "101".into(),
            hint: "102".into(),
            dim: "103".into(),
            accent: "104".into(),
            border: "105".into(),
            error: "106".into(),
            success: "107".into(),
        };
        let t = OwnedTheme::from_custom(&ct);
        assert_eq!(t.header, default_theme().header);
    }
}
