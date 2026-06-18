//! Pyfun — an FP-first language that compiles to Python.
//!
//! Phase 1 scaffolding: the front-end pipeline entry points wired together.
//! See `DESIGN.md` for the full architecture and roadmap.
//!
//! Pipeline implemented so far: source → [`lexer`] → tokens → [`parser`] → AST
//! → [`lowering`] → Python IR → [`python_emitter`] → Python source. The
//! pretty-printer ([`ast`]) drives the parse→print→parse roundtrip tests.

pub mod ast;
pub mod lexer;
pub mod lowering;
pub mod parser;
pub mod python_emitter;

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

/// Compile `source` all the way to Python source text.
pub fn compile(source: &str) -> Result<String, CompileError> {
    let module = parse(source)?;
    let py = lowering::lower(&module).map_err(CompileError::Lower)?;
    Ok(python_emitter::emit(&py))
}

/// A front-end error, from any stage of the pipeline.
#[derive(Debug, Clone, PartialEq)]
pub enum CompileError {
    Lex(lexer::LexError),
    Parse(parser::ParseError),
    Lower(lowering::LowerError),
}

impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CompileError::Lex(e) => write!(f, "lex error: {e}"),
            CompileError::Parse(e) => write!(f, "parse error: {e}"),
            CompileError::Lower(e) => write!(f, "lowering error: {e}"),
        }
    }
}

impl std::error::Error for CompileError {}
