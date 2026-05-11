//! Markdown rendering for the terminal.
//!
//! Converts markdown text into ANSI-escaped strings for display in the client dashboard.
//! Uses `pulldown-cmark` for parsing and lightweight ANSI styling for code blocks.

use pulldown_cmark::{Event, Parser, Tag, TagEnd};

/// Renders markdown text into an ANSI-escaped terminal string.
///
/// # Arguments
///
/// * `input` - The raw markdown string.
/// * `enable_markdown` - If false, returns the input unmodified.
/// * `enable_syntax_highlighting` - If true, applies lightweight ANSI code styling.
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
                    let highlighted = highlight_code_block(&code_buffer);
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

/// Applies a dependency-light style to fenced code blocks.
fn highlight_code_block(code: &str) -> String {
    let mut output = String::new();
    output.push_str("\x1b[38;5;152m");
    output.push_str(code);
    if !code.ends_with('\n') {
        output.push('\n');
    }
    output.push_str("\x1b[0m");
    output
}

#[cfg(test)]
mod tests {
    use super::render_markdown;

    #[test]
    fn markdown_disabled_returns_input_unchanged() {
        let input = "**hi** `there`";
        assert_eq!(render_markdown(input, false, true), input);
    }

    #[test]
    fn fenced_code_blocks_are_styled_without_external_syntax_assets() {
        let rendered = render_markdown("```rust\nfn main() {}\n```", true, true);

        assert!(rendered.contains("[rust]"));
        assert!(rendered.contains("fn main() {}"));
        assert!(rendered.ends_with('\n'));
    }
}
