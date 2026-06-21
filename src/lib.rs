//! Pyfun — an FP-first language that compiles to Python.
//!
//! Phase 1 scaffolding: the front-end pipeline entry points wired together.
//! See `DESIGN.md` for the full architecture and roadmap.
//!
//! Pipeline implemented so far: source → [`lexer`] → tokens → [`parser`] → AST
//! → [`lowering`] → Python IR → [`python_emitter`] → Python source. The
//! pretty-printer ([`ast`]) drives the parse→print→parse roundtrip tests.

pub mod ast;
pub mod diagnostics;
pub mod lexer;
pub mod lowering;
pub mod lsp;
pub mod parser;
pub mod python_emitter;
pub mod types;

pub use parser::ast as syntax;

/// Lex and parse `source` into a [`syntax::Module`].
///
/// This is the convenience entry point used by the CLI and tests. Lexing and
/// parsing errors are surfaced as a single [`CompileError`].
pub fn parse(source: &str) -> Result<syntax::Module, CompileError> {
    let tokens = lexer::lex(source).map_err(CompileError::Lex)?;
    parser::parse(tokens).map_err(CompileError::Parse)
}

/// Lex, parse, then pretty-print `source` back to canonical Pyfun text.
///
/// The printer fully parenthesizes compound expressions, so its output is
/// canonical rather than a faithful reproduction of the original formatting.
pub fn format(source: &str) -> Result<String, CompileError> {
    Ok(ast::print_module(&parse(source)?))
}

/// Type-check `source`, returning every type error found.
///
/// A parse failure is reported as a single error in the returned vector.
pub fn check(source: &str) -> Result<(), Vec<types::TypeError>> {
    let module = parse(source).map_err(|e| vec![to_type_error(&e)])?;
    types::check(&module)
}

/// Analyze `source` for the editor (the LSP, `DESIGN.md` §9): return every
/// diagnostic plus a span→type table for hover.
///
/// Unlike [`check`], this never short-circuits — a parse failure becomes a single
/// diagnostic (with an empty hover table), and a module that type-checks with
/// errors still yields whatever inferred types were recovered. This is the one
/// entry point the LSP server needs.
pub fn analyze(source: &str) -> (Vec<types::TypeError>, Vec<types::TypeSpan>) {
    match parse(source) {
        Ok(module) => types::check_collecting(&module),
        Err(e) => (vec![to_type_error(&e)], Vec::new()),
    }
}

/// Compile `source` all the way to Python source text.
///
/// The compiler is the gatekeeper (`DESIGN.md` §2): lowering only runs once the
/// program type-checks, so emitted Python is always well-typed Pyfun.
pub fn compile(source: &str) -> Result<String, CompileError> {
    let module = parse(source)?;
    if let Err(mut errors) = types::check(&module) {
        return Err(CompileError::Type(errors.remove(0)));
    }
    let py = lowering::lower(&module).map_err(CompileError::Lower)?;
    Ok(python_emitter::emit(&py))
}

/// Turn a lex/parse `CompileError` into a `TypeError` so `check` can report a
/// uniform, span-carrying error list.
fn to_type_error(error: &CompileError) -> types::TypeError {
    let span = match error {
        CompileError::Lex(e) => e.span,
        CompileError::Parse(e) => e.span,
        // `parse` only ever yields Lex/Parse, so this is unreachable in practice.
        _ => lexer::Span::new(0, 0),
    };
    types::TypeError {
        message: error.to_string(),
        span,
    }
}

/// A front-end error, from any stage of the pipeline.
#[derive(Debug, Clone, PartialEq)]
pub enum CompileError {
    Lex(lexer::LexError),
    Parse(parser::ParseError),
    Type(types::TypeError),
    Lower(lowering::LowerError),
}

impl CompileError {
    /// The source span this error should be reported against. Lowering errors
    /// have no span yet, so they report at the start of the file.
    pub fn span(&self) -> lexer::Span {
        match self {
            CompileError::Lex(e) => e.span,
            CompileError::Parse(e) => e.span,
            CompileError::Type(e) => e.span,
            CompileError::Lower(_) => lexer::Span::new(0, 0),
        }
    }

    /// The bare message, without the `… error:` prefix from `Display`.
    pub fn message(&self) -> String {
        match self {
            CompileError::Lex(e) => e.to_string(),
            CompileError::Parse(e) => e.to_string(),
            CompileError::Type(e) => e.to_string(),
            CompileError::Lower(e) => e.to_string(),
        }
    }
}

impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CompileError::Lex(e) => write!(f, "lex error: {e}"),
            CompileError::Parse(e) => write!(f, "parse error: {e}"),
            CompileError::Type(e) => write!(f, "type error: {e}"),
            CompileError::Lower(e) => write!(f, "lowering error: {e}"),
        }
    }
}

impl std::error::Error for CompileError {}
