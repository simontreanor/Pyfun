//! Pyfun — an FP-first language that compiles to Python.
//!
//! Phase 1 scaffolding: the front-end pipeline entry points wired together.
//! See `DESIGN.md` for the full architecture and roadmap.
//!
//! Pipeline implemented so far: source → [`lexer`] → tokens → [`parser`] → AST
//! → [`lowering`] → Python IR → [`python_emitter`] → Python source. The
//! pretty-printer ([`ast`]) drives the parse→print→parse roundtrip tests.

pub mod ast;
pub mod desugar;
pub mod diagnostics;
pub mod lexer;
pub mod lowering;
pub mod lsp;
pub mod parser;
pub mod project;
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

/// A resilient analysis of a source document, for the editor (the LSP,
/// `DESIGN.md` §9). Produced by [`analyze`] and consumed by every LSP feature.
///
/// "Resilient" means a half-typed file still yields results: parsing recovers at
/// item boundaries, so the items that *do* parse populate `module` (and its hover
/// `types` and navigation), while the broken ones surface as `diagnostics`.
pub struct Analysis {
    /// The recovered module. Always present (lexing and parsing both recover), but
    /// may be *partial* — missing items that did not parse — when `parse_ok` is
    /// false. Kept an `Option` so the API tolerates a future fatal-parse path.
    pub module: Option<syntax::Module>,
    /// Diagnostics to publish: syntax errors (lex + parse) when the file does not
    /// fully parse, otherwise the type/effect/unit errors.
    pub diagnostics: Vec<types::TypeError>,
    /// The span→type table for hover (best-effort, even for a partial module).
    pub types: Vec<types::TypeSpan>,
    /// Whether the document lexed and parsed with no syntax errors. Mutating
    /// features (rename) require this — a partial module could hide occurrences.
    pub parse_ok: bool,
}

/// Analyze `source` for the editor (the LSP, `DESIGN.md` §9).
///
/// Unlike [`check`], this never short-circuits. Both the lexer and the parser
/// recover from errors: a bad character / unterminated string is skipped, and a
/// broken item is skipped to the next boundary, so the parts that *do* lex and
/// parse still populate the module (driving hover and navigation). Until the syntax
/// is clean, only the *syntax* errors are reported (type errors over a partial
/// module are noise); once it is, the type errors take over. This is the one entry
/// point the LSP server needs.
pub fn analyze(source: &str) -> Analysis {
    analyze_in_dir(source, None)
}

/// Like [`analyze`], but resolves `import`s relative to `dir` so a multi-module
/// file checks cleanly in the editor (`DESIGN.md` §6.1). When `dir` is `None`
/// (or no imports resolve) this is exactly [`analyze`]. Imported modules are
/// read from sibling `<name>.pyfun` files (best-effort — missing/broken imports
/// are skipped).
pub fn analyze_in_dir(source: &str, dir: Option<&std::path::Path>) -> Analysis {
    analyze_with_imports(source, |module| match dir {
        Some(d) => project::resolve_imports(d, module),
        None => std::collections::HashMap::new(),
    })
}

/// Like [`analyze`], but with import resolution injected: once `source` is
/// (recovering-)parsed, `resolve` maps the module to its imports' export
/// interfaces, which seed the type check
/// (`types::check_collecting_with_imports`; an empty map is exactly
/// [`analyze`]). [`analyze_in_dir`] passes the disk-reading
/// `project::resolve_imports`; the LSP server passes its cached,
/// open-buffer-aware resolver (`DESIGN.md` §9), which also records the files it
/// consulted so the analysis can be invalidated when an imported file changes.
pub fn analyze_with_imports(
    source: &str,
    resolve: impl FnOnce(&syntax::Module) -> std::collections::HashMap<String, types::ModuleExports>,
) -> Analysis {
    let (tokens, lex_errors) = lexer::lex_recover(source);
    let (module, parse_errors) = parser::parse_recover(tokens);
    let imports = resolve(&module);
    let (type_errors, types) = types::check_collecting_with_imports(&module, &imports);
    // Syntax errors (lex + parse) take precedence and suppress type errors; sorted
    // by position so cascading errors read top-to-bottom.
    let mut syntax_errors: Vec<_> = lex_errors
        .iter()
        .map(|e| to_type_error(&CompileError::Lex(e.clone())))
        .chain(
            parse_errors
                .iter()
                .map(|e| to_type_error(&CompileError::Parse(e.clone()))),
        )
        .collect();
    syntax_errors.sort_by_key(|e| (e.span.start, e.span.end));
    let parse_ok = syntax_errors.is_empty();
    Analysis {
        module: Some(module),
        diagnostics: if parse_ok { type_errors } else { syntax_errors },
        types,
        parse_ok,
    }
}

/// Compile `source` all the way to Python source text.
///
/// The compiler is the gatekeeper (`DESIGN.md` §2): lowering only runs once the
/// program type-checks, so emitted Python is always well-typed Pyfun.
pub fn compile(source: &str) -> Result<String, CompileError> {
    let module = parse(source)?;
    // One inference pass gives both the gate (errors) and the resolved types, from
    // which we mark the integer literals that resolved to `float` so lowering emits
    // them as `7.0` (matching their inferred type — see `float_literal_spans`).
    let (mut errors, types) = types::check_collecting(&module);
    if !errors.is_empty() {
        return Err(CompileError::Type(errors.remove(0)));
    }
    let floats = float_literal_spans(&types);
    let py = lowering::lower(&module, &floats).map_err(CompileError::Lower)?;
    Ok(python_emitter::emit(&py))
}

/// The spans of expressions whose inferred type is `float` (dimensionless or
/// unit-carrying), collected from a [`types::check_collecting`] type table.
/// Lowering consults this only for value-position integer literals, so a `float`
/// entry for a non-literal node is harmless (spans of distinct nodes never
/// collide). See [`compile`] and `lowering`'s `float_literals`.
pub(crate) fn float_literal_spans(
    types: &[types::TypeSpan],
) -> std::collections::HashSet<lexer::Span> {
    types
        .iter()
        .filter(|t| t.ty == "float" || t.ty.starts_with("float<"))
        .map(|t| t.span)
        .collect()
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
