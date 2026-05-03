//! Markdown rendering for the terminal.
//!
//! Converts markdown text into ANSI-escaped strings for display in the client dashboard.
//! Uses `pulldown-cmark` for parsing and `syntect` for syntax highlighting of code blocks.

use pulldown_cmark::{Event, Parser, Tag, TagEnd};
use std::sync::OnceLock;
use syntect::easy::HighlightLines;
use syntect::highlighting::ThemeSet;
use syntect::parsing::SyntaxSet;
use syntect::util::{as_24_bit_terminal_escaped, LinesWithEndings};

// Global syntax and theme sets to avoid reloading them on every message
static SYNTAX_SET: OnceLock<SyntaxSet> = OnceLock::new();
static THEME_SET: OnceLock<ThemeSet> = OnceLock::new();

fn get_syntax_set() -> &'static SyntaxSet {
    SYNTAX_SET.get_or_init(SyntaxSet::load_defaults_newlines)
}

fn get_theme_set() -> &'static ThemeSet {
    THEME_SET.get_or_init(ThemeSet::load_defaults)
}

/// Renders markdown text into an ANSI-escaped terminal string.
///
/// # Arguments
///
/// * `input` - The raw markdown string.
/// * `enable_markdown` - If false, returns the input unmodified.
/// * `enable_syntax_highlighting` - If true, highlights code blocks using `syntect`.
pub fn render_markdown(
    input: &str,
    enable_markdown: bool,
    enable_syntax_highlighting: bool,
) -> String {
    if !enable_markdown {
        return input.to_string();
    }

    let parser = Parser::new(input);
    let mut out = String::new();
    let mut in_code_block = false;
    let mut code_lang = String::new();
    let mut code_buffer = String::new();

    for event in parser {
        match event {
            Event::Text(text) => {
                if in_code_block {
                    code_buffer.push_str(&text);
                } else {
                    out.push_str(&text);
                }
            }
            Event::Code(code) => {
                // Inline code: highlight slightly
                out.push_str("\x1b[36m"); // Cyan for inline code
                out.push_str(&code);
                out.push_str("\x1b[0m");
            }
            Event::Start(Tag::CodeBlock(pulldown_cmark::CodeBlockKind::Fenced(lang))) => {
                in_code_block = true;
                code_lang = lang.to_string();
            }
            Event::Start(Tag::CodeBlock(pulldown_cmark::CodeBlockKind::Indented)) => {
                in_code_block = true;
                code_lang = String::new();
            }
            Event::End(TagEnd::CodeBlock) => {
                in_code_block = false;
                if !code_lang.is_empty() {
                    out.push_str("\x1b[35m["); // Magenta for language label
                    out.push_str(&code_lang);
                    out.push_str("]\x1b[0m\n");
                }
                if enable_syntax_highlighting {
                    let highlighted = highlight_code(&code_lang, &code_buffer);
                    out.push_str(&highlighted);
                    if !highlighted.ends_with('\n') {
                        out.push('\n');
                    }
                } else {
                    out.push_str("\x1b[2m"); // Dim for plain code blocks
                    out.push_str(&code_buffer);
                    out.push_str("\x1b[0m\n");
                }
                code_buffer.clear();
            }
            Event::Start(Tag::Strong) => out.push_str("\x1b[1m"),
            Event::End(TagEnd::Strong) => out.push_str("\x1b[22m"),
            Event::Start(Tag::Emphasis) => out.push_str("\x1b[3m"),
            Event::End(TagEnd::Emphasis) => out.push_str("\x1b[23m"),
            Event::Start(Tag::Strikethrough) => out.push_str("\x1b[9m"),
            Event::End(TagEnd::Strikethrough) => out.push_str("\x1b[29m"),
            Event::SoftBreak | Event::HardBreak => out.push('\n'),
            // Basic lists and others can just fall back or be implemented as needed
            Event::Start(Tag::Item) => out.push_str(" • "),
            _ => {} // Ignore other tags to keep rendering simple and safe
        }
    }

    out
}

/// Highlights a block of code using `syntect`.
fn highlight_code(lang: &str, code: &str) -> String {
    let ps = get_syntax_set();
    let ts = get_theme_set();

    // Default to plain text if syntax not found or not specified
    let syntax = ps
        .find_syntax_by_token(lang)
        .unwrap_or_else(|| ps.find_syntax_plain_text());

    // "base16-ocean.dark" is a nice default dark theme in syntect
    let theme = &ts.themes["base16-ocean.dark"];
    let mut h = HighlightLines::new(syntax, theme);

    let mut output = String::new();
    for line in LinesWithEndings::from(code) {
        if let Ok(ranges) = h.highlight_line(line, ps) {
            let escaped = as_24_bit_terminal_escaped(&ranges[..], false);
            output.push_str(&escaped);
        } else {
            output.push_str(line);
        }
    }

    output.push_str("\x1b[0m"); // reset
    output
}
