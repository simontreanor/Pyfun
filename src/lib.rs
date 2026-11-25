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
pub mod fmt;
pub mod imports;
pub mod lexer;
pub mod lint;
pub mod lowering;
pub mod lsp;
pub mod parser;
pub mod project;
pub mod python_emitter;
pub mod scope;
pub mod traceback;
pub mod types;

pub use parser::ast as syntax;

/// The compiler version, so an embedder can report which one it is running.
/// `pyfun version` reads the same constant through the binary crate, and the
/// WebAssembly playground shows it in the page footer.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

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
    /// Typed holes (`?` / `?name`) and their inferred types — surfaced by the LSP
    /// at `Information` severity (an intentional blank, not an error).
    pub holes: Vec<types::Hole>,
    /// Warnings (an unreachable arm, an unused binding or import, a top-level
    /// name bound twice), surfaced by the LSP at `Warning` severity.
    pub warnings: Vec<diagnostics::Warning>,
    /// What each non-exhaustive `match` leaves out, for the LSP's "add the
    /// missing cases".
    pub missing_cases: Vec<types::MissingCases>,
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
    let (type_errors, types, holes, warnings, missing_cases) =
        types::check_for_editor(&module, &imports);
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
        // Holes surface only once the file parses clean (like type errors).
        holes: if parse_ok { holes } else { Vec::new() },
        // So do warnings: a half-typed file has unused names everywhere.
        warnings: if parse_ok { warnings } else { Vec::new() },
        // And a fix for an error that is not being shown.
        missing_cases: if parse_ok { missing_cases } else { Vec::new() },
        parse_ok,
    }
}

/// The warnings for a single-file program that parses (none for one that does
/// not: its errors are what there is to say). `pyfun run` and `pyfun compile`
/// print these beside the program they build.
pub fn warnings(source: &str) -> Vec<diagnostics::Warning> {
    let analysis = analyze(source);
    if analysis.parse_ok && analysis.diagnostics.is_empty() {
        analysis.warnings
    } else {
        Vec::new()
    }
}

/// The warnings an interactive session shows for the entry that begins at byte
/// `entry_start` of the analyzed text: the ones inside the entry that stand on
/// their own ([`diagnostics::WarningKind::within_one_entry`]).
pub fn entry_warnings(analysis: &Analysis, entry_start: usize) -> Vec<&diagnostics::Warning> {
    analysis
        .warnings
        .iter()
        .filter(|w| w.span.start >= entry_start && w.kind.within_one_entry())
        .collect()
}

/// Compile `source` all the way to Python source text (default 3.12+ target).
///
/// The compiler is the gatekeeper (`DESIGN.md` §2): lowering only runs once the
/// program type-checks, so emitted Python is always well-typed Pyfun.
pub fn compile(source: &str) -> Result<String, CompileError> {
    compile_targeting(source, python_emitter::PyTarget::default())
}

/// [`compile`] with an explicit emission target (`--target 3.11` rewrites
/// PEP 701-dependent f-strings so the output parses on Python 3.11 / PyPy).
pub fn compile_targeting(
    source: &str,
    target: python_emitter::PyTarget,
) -> Result<String, CompileError> {
    compile_collecting(source, target).map(|(py, _)| py)
}

/// [`compile_targeting`], also returning the lowering **notes** — things worth
/// telling the author that are neither errors nor visible in the output (today,
/// a self tail call that kept its recursive form; `DESIGN.md` §5.4). The CLI
/// prints them; callers that only want the Python use [`compile`].
pub fn compile_collecting(
    source: &str,
    target: python_emitter::PyTarget,
) -> Result<(String, Vec<String>), CompileError> {
    compile_with(source, target, false)
}

/// [`compile_collecting`] with the **native** lowering on or off (`--native`,
/// `DESIGN.md` §5.6): a `match` on a user sum type becomes an `isinstance`
/// ladder, the form mypyc compiles and CPython runs faster, instead of
/// `match`/`case`.
pub fn compile_with(
    source: &str,
    target: python_emitter::PyTarget,
    native: bool,
) -> Result<(String, Vec<String>), CompileError> {
    compile_mapped(source, target, native, false, false, scope::Naming::File)
        .map(|(py, notes, _)| (py, notes))
}

/// [`compile_collecting`] with **typed output** (`--typed`, `DESIGN.md` §5.7):
/// every top-level function, value and class carries its inferred type as an
/// annotation a Python type checker reads. The annotations use Python 3.12's
/// type parameter syntax.
pub fn compile_typed(source: &str) -> Result<(String, Vec<String>), CompileError> {
    compile_mapped(
        source,
        python_emitter::PyTarget::default(),
        false,
        false,
        true,
        scope::Naming::File,
    )
    .map(|(py, notes, _)| (py, notes))
}

/// Where each line of an emitted Python file came from: `(python line, source
/// span)` entries in line order, 1-based, the span being the statement the line
/// was lowered from (the header alone of a `match`, an `if` or a function). A Python line with no entry of its own belongs to the
/// nearest entry above it; lines above the first entry (imports, emitted
/// classes, helpers) come from no source line.
pub type LineMap = Vec<(usize, lexer::Span)>;

/// Compile `source` and build its [`LineMap`], for a traceback that names
/// `.pyfun` lines (`DESIGN.md` §9).
///
/// The map is read from a second, *traced* lowering whose statements carry
/// source-position markers. Markers can only be trusted if they changed
/// nothing else, so the traced emission, markers removed, must equal the real
/// one byte for byte; when it does not (some rewrite reacted to a marker),
/// there is no map and the traceback stays Python's own.
pub fn compile_with_line_map(source: &str) -> Result<(String, Option<LineMap>), CompileError> {
    compile_traced(source).map(|(py, _, map)| (py, map))
}

/// [`compile_with_line_map`], with the lowering notes [`compile_collecting`]
/// returns: everything `pyfun run` needs from one compilation.
pub fn compile_traced(
    source: &str,
) -> Result<(String, Vec<String>, Option<LineMap>), CompileError> {
    compile_mapped(
        source,
        python_emitter::PyTarget::default(),
        false,
        true,
        false,
        scope::Naming::File,
    )
}

/// [`compile_traced`] for a REPL or notebook session, whose source grows an
/// entry at a time. A name bound again keeps its earlier bindings under the
/// names they already run by, and the new one takes a name of its own
/// ([`scope::Naming::Session`]), so nothing that ran is emitted differently.
pub fn compile_session(
    source: &str,
) -> Result<(String, Vec<String>, Option<LineMap>), CompileError> {
    compile_mapped(
        source,
        python_emitter::PyTarget::default(),
        false,
        true,
        false,
        scope::Naming::Session,
    )
}

fn compile_mapped(
    source: &str,
    target: python_emitter::PyTarget,
    native: bool,
    want_map: bool,
    typed: bool,
    naming: scope::Naming,
) -> Result<(String, Vec<String>, Option<LineMap>), CompileError> {
    let module = parse(source)?;
    // One inference pass gives both the gate (errors) and the resolved types, from
    // which we mark the integer literals that resolved to `float` so lowering emits
    // them as `7.0` (matching their inferred type — see `float_literal_spans`).
    let (mut errors, types, holes, ordered, codecs) = types::check_collecting(&module);
    if !errors.is_empty() {
        return Err(CompileError::Type(errors.remove(0)));
    }
    // A hole has no value to lower — a program with holes is intentionally
    // incomplete, so compilation is blocked (with the hole's location + type).
    if let Some(hole) = holes.first() {
        return Err(CompileError::Type(types::TypeError {
            message: format!("cannot compile: unfilled {} (fill it in)", hole.message()),
            span: hole.span,
        }));
    }
    let floats = float_literal_spans(&types);
    // Single file: the whole program is visible, so emit ordering methods only for the
    // types actually compared (`DESIGN.md` §7.1).
    // Native mode annotates functions from their inferred types (`DESIGN.md` §5.6).
    let binding_types = if native {
        types.iter().map(|t| (t.span, t.raw.clone())).collect()
    } else {
        std::collections::HashMap::new()
    };
    let (py, notes) = lowering::lower_shaped(
        &module,
        &floats,
        lowering::OrderPolicy::OnDemand(ordered.clone()),
        &codecs,
        native,
        binding_types.clone(),
        lowering::Shape {
            trace: false,
            naming,
        },
    )
    .map_err(CompileError::Lower)?;
    let py = if typed {
        let all_types = types.iter().map(|t| (t.span, t.raw.clone())).collect();
        lowering::annotate_typed(&module, py, &all_types, &[])
    } else {
        py
    };
    let python = python_emitter::emit_for(&py, target);
    let map = if want_map {
        lowering::lower_shaped(
            &module,
            &floats,
            lowering::OrderPolicy::OnDemand(ordered),
            &codecs,
            native,
            binding_types,
            lowering::Shape {
                trace: true,
                naming,
            },
        )
        .ok()
        .and_then(|(traced, _)| line_map_of(&python_emitter::emit_for(&traced, target), &python))
    } else {
        None
    };
    Ok((python, notes, map))
}

/// The traced emission of `source` with its markers removed, for finding out why
/// a program has no line map: diff it against [`compile`]'s output.
#[doc(hidden)]
pub fn traced_python(source: &str) -> Result<String, CompileError> {
    let module = parse(source)?;
    let (mut errors, types, _holes, ordered, codecs) = types::check_collecting(&module);
    if !errors.is_empty() {
        return Err(CompileError::Type(errors.remove(0)));
    }
    let floats = float_literal_spans(&types);
    let (traced, _) = lowering::lower_native_typed_tracing(
        &module,
        &floats,
        lowering::OrderPolicy::OnDemand(ordered),
        &codecs,
        false,
        std::collections::HashMap::new(),
        true,
    )
    .map_err(CompileError::Lower)?;
    Ok(python_emitter::split_line_markers(&python_emitter::emit(&traced)).0)
}

/// The line map a traced emission describes, provided that removing its
/// markers gives exactly `python`, the program that will run.
///
/// A marker holds for the lines after it at its own depth or deeper. When the
/// emitted code dedents past it (the end of a hoisted function's body, say),
/// the lines that follow belong again to the marker in force at their depth,
/// so the map gets an entry there too.
pub(crate) fn line_map_of(traced: &str, python: &str) -> Option<LineMap> {
    let (stripped, marks) = python_emitter::split_line_markers(traced);
    if stripped != python {
        return None;
    }
    let mut map = LineMap::new();
    // The markers in force, outermost first: `(indentation, source span)`.
    let mut scopes: Vec<(usize, lexer::Span)> = Vec::new();
    let mut marks = marks.into_iter().peekable();
    let mut current = None;
    for (index, line) in python.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        while scopes.last().is_some_and(|scope| scope.0 > indent) {
            scopes.pop();
        }
        while let Some((_, start, end)) = marks.next_if(|(at, _, _)| *at <= index + 1) {
            if scopes.last().is_some_and(|scope| scope.0 == indent) {
                scopes.pop();
            }
            scopes.push((indent, lexer::Span::new(start as usize, end as usize)));
        }
        let now = scopes.last().map(|scope| scope.1);
        if now != current
            && let Some(span) = now
        {
            map.push((index + 1, span));
            current = now;
        }
    }
    Some(map)
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
/// uniform, span-carrying error list. The message is the bare one: the span
/// travels in its own field, so every consumer (the LSP publishes it as a
/// range, the REPL/kernel render it against the source) locates the error
/// without byte offsets in the text.
fn to_type_error(error: &CompileError) -> types::TypeError {
    let span = match error {
        CompileError::Lex(e) => e.span,
        CompileError::Parse(e) => e.span,
        // `parse` only ever yields Lex/Parse, so this is unreachable in practice.
        _ => lexer::Span::new(0, 0),
    };
    types::TypeError {
        message: error.message(),
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

    /// The bare message, without the `… error:` prefix from `Display` and
    /// without the `(at N..M)` byte-span suffix that `LexError`/`ParseError`
    /// append there. Callers pair it with [`Self::span`] (usually through
    /// `diagnostics::render`, which points into the source itself), so the raw
    /// offsets would only repeat what the rendering already shows.
    pub fn message(&self) -> String {
        match self {
            CompileError::Lex(e) => e.message.clone(),
            CompileError::Parse(e) => e.message.clone(),
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
