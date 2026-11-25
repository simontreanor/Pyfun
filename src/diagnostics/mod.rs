//! Rendering source-spanned diagnostics in a rustc-like style (`DESIGN.md` §3).
//!
//! Phase 3 keeps this deliberately small: a level, a message, and a single
//! underlined span. Diagnostic codes and multi-span notes can come later.

use crate::lexer::Span;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Error,
    Warning,
    /// An informational note — used for typed holes (`?` / `?name`), which report a
    /// type but aren't mistakes.
    Note,
}

/// A warning: something in a program that compiles and is very likely not what
/// its author meant (`DESIGN.md` §3). A warning never stops a build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Warning {
    pub kind: WarningKind,
    pub message: String,
    pub span: Span,
}

/// What a warning is about. The kind decides where it is shown: a session that
/// grows one entry at a time (the REPL, a notebook) binds a name again and
/// imports ahead of use as a matter of course, so it shows only the kinds that
/// are about one entry on its own ([`WarningKind::within_one_entry`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WarningKind {
    /// A `case` arm no value reaches.
    UnreachableArm,
    /// A local `let` whose name nothing reads.
    UnusedBinding,
    /// An `import` nothing in the file uses.
    UnusedImport,
    /// An `extern import` no `extern` is rooted at.
    UnusedExternImport,
    /// A top-level `let` binding a name an earlier one bound.
    Rebound,
}

impl Warning {
    /// The span is code that does nothing (an unused binding or import, an arm
    /// that cannot match), which an editor shows faded.
    pub fn unnecessary(&self) -> bool {
        self.kind != WarningKind::Rebound
    }
}

impl WarningKind {
    /// Whether the warning stands whatever is entered next: true of an arm
    /// that cannot match and of an unused local, false of the kinds a later
    /// entry may answer (an import used below) or that an interactive session
    /// does on purpose (binding a name again).
    pub fn within_one_entry(self) -> bool {
        matches!(
            self,
            WarningKind::UnreachableArm | WarningKind::UnusedBinding
        )
    }
}

impl Level {
    fn label(self) -> &'static str {
        match self {
            Level::Error => "error",
            Level::Warning => "warning",
            Level::Note => "note",
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
    render_at(None, source, level, message, span)
}

/// [`render`] for a diagnostic in a file: the location line names the file
/// (`--> shapes.pyfun:1:18`), the form a terminal and an editor open.
pub fn render_in(file: &str, source: &str, level: Level, message: &str, span: Span) -> String {
    render_at(Some(file), source, level, message, span)
}

fn render_at(file: Option<&str>, source: &str, level: Level, message: &str, span: Span) -> String {
    let (line_no, col_no, line_start) = locate(source, span.start);
    let file = file.map(|f| format!("{f}:")).unwrap_or_default();
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
         {pad}--> {file}{line_no}:{col_no}\n\
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
