//! Rendering source-spanned diagnostics in a rustc-like style (`DESIGN.md` §3).
//!
//! Phase 3 keeps this deliberately small: a level, a message, and a single
//! underlined span. Diagnostic codes and multi-span notes can come later.

use crate::lexer::Span;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Error,
    Warning,
}

impl Level {
    fn label(self) -> &'static str {
        match self {
            Level::Error => "error",
            Level::Warning => "warning",
        }
    }
}

/// Render one diagnostic against `source`, e.g.
///
/// ```text
/// error: type mismatch: expected int, found bool
///   --> 1:18
///    |
///  1 | let r = add 1 true
///    |                  ^^^^
/// ```
pub fn render(source: &str, level: Level, message: &str, span: Span) -> String {
    let (line_no, col_no, line_start) = locate(source, span.start);
    let line_text = source[line_start..]
        .split('\n')
        .next()
        .unwrap_or("")
        .trim_end_matches('\r');

    let gutter = format!("{line_no}");
    let pad = " ".repeat(gutter.len());

    // Underline length, clamped to the remainder of the line.
    let span_len = span.end.saturating_sub(span.start).max(1);
    let max_len = line_text.len().saturating_sub(col_no - 1).max(1);
    let caret = "^".repeat(span_len.min(max_len));
    let indent = " ".repeat(col_no - 1);

    format!(
        "{level}: {message}\n\
         {pad}--> {line_no}:{col_no}\n\
         {pad} |\n\
         {gutter} | {line_text}\n\
         {pad} | {indent}{caret}",
        level = level.label(),
    )
}

/// Map a byte offset to `(1-based line, 1-based column, byte offset of line start)`.
fn locate(source: &str, offset: usize) -> (usize, usize, usize) {
    let offset = offset.min(source.len());
    let mut line = 1;
    let mut line_start = 0;
    for (i, b) in source.bytes().enumerate() {
        if i >= offset {
            break;
        }
        if b == b'\n' {
            line += 1;
            line_start = i + 1;
        }
    }
    let col = offset - line_start + 1;
    (line, col, line_start)
}
