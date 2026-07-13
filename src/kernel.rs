//! Machine protocol powering the Jupyter kernel: `pyfun kernel-engine`.
//!
//! This is the REPL session turned inside out. The REPL (`src/repl.rs`) drives a
//! Python worker: it accumulates definition source, type-checks each entry against
//! it, recompiles the accumulated program, and sends only the not-yet-executed
//! emitted chunks to a persistent namespace. Here the *caller* (the `pyfun_kernel`
//! Python package, running inside Jupyter) owns the namespace: the engine does the
//! same accumulate → analyze → compile → chunk-diff bookkeeping, but *returns* the
//! new-chunk blob for the caller to `exec`. The kernel process is itself a Python
//! process, so no worker subprocess exists — Jupyter's own stdout capture routes
//! cell output, and the two processes live and die together.
//!
//! Protocol, both directions framed as 8 ASCII digits (payload byte length) then
//! that many UTF-8 bytes — the same framing as the REPL's worker driver:
//!
//!   request:  frame(op) frame(payload)
//!     op = "eval"  payload = a cell's source
//!     op = "type"  payload = an expression (`:type` — no evaluation, empty blob)
//!   response: frame(status) frame(message) frame(blob)
//!     status  = "ok" | "error"
//!     message = definition type echoes (`n : int`, one per line) on ok;
//!               rendered rustc-style diagnostics on error
//!     blob    = Python statements for the caller to exec in its namespace
//!               (empty when everything was already executed, or on error)
//!
//! EOF on stdin ends the loop — a normal exit.
//!
//! Semantics mirror the REPL with one Jupyter-shaped addition: a cell may mix
//! definitions with a *trailing* expression (`let x = 1` ⏎ `x * 2`), which is
//! split off, so the definitions accumulate and the expression is evaluated
//! (wrapped in `print (…)` unless `unit`-typed) — re-running the cell re-runs
//! the expression but not the definitions' effects.

use std::collections::HashSet;
use std::io::{self, Read, Write};
use std::process::ExitCode;

use pyfun::diagnostics::{self, Level};
use pyfun::syntax::Item;

use crate::repl::{blob_of_new, chunk_python, expression_is_unit, is_infrastructure};

pub fn run() -> ExitCode {
    let mut engine = Engine::new();
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut input = stdin.lock();
    let mut output = stdout.lock();
    loop {
        let Ok(Some(op)) = read_frame(&mut input) else {
            return ExitCode::SUCCESS; // EOF or a broken pipe: the kernel went away
        };
        let Ok(Some(payload)) = read_frame(&mut input) else {
            return ExitCode::SUCCESS;
        };
        let reply = match op.as_str() {
            "eval" => engine.eval_cell(&payload),
            "type" => engine.type_of(&payload),
            other => Reply::error(format!("unknown op `{other}`")),
        };
        let status = if reply.ok { "ok" } else { "error" };
        if write_frame(&mut output, status)
            .and_then(|()| write_frame(&mut output, &reply.message))
            .and_then(|()| write_frame(&mut output, &reply.blob))
            .and_then(|()| output.flush())
            .is_err()
        {
            return ExitCode::SUCCESS;
        }
    }
}

fn read_frame(input: &mut impl Read) -> io::Result<Option<String>> {
    let mut header = [0u8; 8];
    let mut filled = 0;
    while filled < 8 {
        match input.read(&mut header[filled..])? {
            0 if filled == 0 => return Ok(None), // clean EOF between frames
            0 => return Err(io::ErrorKind::UnexpectedEof.into()),
            n => filled += n,
        }
    }
    let len: usize = std::str::from_utf8(&header)
        .ok()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "bad frame header"))?;
    let mut payload = vec![0u8; len];
    input.read_exact(&mut payload)?;
    Ok(Some(String::from_utf8_lossy(&payload).into_owned()))
}

fn write_frame(output: &mut impl Write, payload: &str) -> io::Result<()> {
    write!(output, "{:08}", payload.len())?;
    output.write_all(payload.as_bytes())
}

struct Reply {
    ok: bool,
    message: String,
    blob: String,
}

impl Reply {
    fn error(message: String) -> Reply {
        Reply {
            ok: false,
            message,
            blob: String::new(),
        }
    }
}

/// The kernel-side session state: mirrors the REPL's `Session` minus the worker.
struct Engine {
    /// Accumulated definition source (the type-level state).
    defs: String,
    /// Emitted top-level chunks the caller has been given to exec (the diffing
    /// state) — the emitter is deterministic, so recompiling the accumulated
    /// program re-yields these byte-for-byte and they are skipped.
    executed: HashSet<String>,
}

impl Engine {
    fn new() -> Engine {
        Engine {
            defs: String::new(),
            executed: HashSet::new(),
        }
    }

    /// Evaluate one cell: definitions accumulate (echoing their inferred types),
    /// a trailing expression is split off and evaluated per entry.
    fn eval_cell(&mut self, cell: &str) -> Reply {
        let module = match pyfun::parse(cell) {
            Ok(m) => m,
            Err(e) => {
                return Reply::error(diagnostics::render(
                    cell,
                    Level::Error,
                    &e.message(),
                    e.span(),
                ));
            }
        };

        // Split a trailing expression off the cell (notebook cells routinely end
        // with one to display). Earlier `Item::Expr`s stay with the definitions:
        // they run once, at entry, exactly as in the REPL.
        let (defs_src, expr_src) = match module.items.last() {
            Some(Item::Expr(e)) => {
                let start = e.span().start;
                (&cell[..start], Some(&cell[start..]))
            }
            _ => (cell, None),
        };
        let def_items = module.items.len() - usize::from(expr_src.is_some());

        let mut message = String::new();
        let mut blob = String::new();

        if !defs_src.trim().is_empty() {
            match self.add_definitions(defs_src, def_items) {
                Ok((echoes, defs_blob)) => {
                    message.push_str(&echoes);
                    blob.push_str(&defs_blob);
                }
                Err(rendered) => return Reply::error(rendered),
            }
        }

        if let Some(expr) = expr_src {
            match self.eval_expression(expr) {
                Ok(expr_blob) => blob.push_str(&expr_blob),
                Err(rendered) => return Reply::error(rendered),
            }
        }

        Reply {
            ok: true,
            message,
            blob,
        }
    }

    /// Type-check new definitions against the session; on success remember them,
    /// return each new binding's type echo and the not-yet-executed chunks.
    fn add_definitions(
        &mut self,
        entry: &str,
        new_count: usize,
    ) -> Result<(String, String), String> {
        let combined = format!("{}\n{entry}", self.defs);
        let analysis = pyfun::analyze(&combined);
        if !analysis.diagnostics.is_empty() {
            return Err(render_errors(&combined, &analysis.diagnostics));
        }
        let mut echoes = String::new();
        if let Some(module) = &analysis.module {
            let start = module.items.len().saturating_sub(new_count);
            for item in &module.items[start..] {
                if let Item::Let(binding) = item {
                    let ty = analysis
                        .types
                        .iter()
                        .find(|t| t.span == binding.name_span.span())
                        .map(|t| t.ty.as_str());
                    match ty {
                        Some(ty) => echoes.push_str(&format!("{} : {ty}\n", binding.name)),
                        None => echoes.push_str(&format!("{}\n", binding.name)),
                    }
                }
            }
        }
        match pyfun::compile(&combined) {
            Ok(python) => {
                let chunks = chunk_python(&python);
                let blob = blob_of_new(&chunks, &self.executed);
                self.executed.extend(chunks);
                self.defs = combined;
                Ok((echoes, blob))
            }
            Err(e) => Err(diagnostics::render(
                &combined,
                Level::Error,
                &e.message(),
                e.span(),
            )),
        }
    }

    /// Type-check an expression against the accumulated definitions and return
    /// the chunks to run it once (wrapped in `print (…)` unless `unit`-typed).
    /// Infrastructure chunks it pulls in are remembered; its own statements are
    /// not, so re-running the cell re-runs the expression.
    fn eval_expression(&mut self, entry: &str) -> Result<String, String> {
        let combined = format!("{}\n{entry}", self.defs);
        let analysis = pyfun::analyze(&combined);
        if !analysis.diagnostics.is_empty() {
            return Err(render_errors(&combined, &analysis.diagnostics));
        }
        let is_unit = expression_is_unit(&analysis);
        let program_src = if is_unit {
            combined
        } else {
            format!("{}\nprint ({entry})", self.defs)
        };
        match pyfun::compile(&program_src) {
            Ok(python) => {
                let chunks = chunk_python(&python);
                let blob = blob_of_new(&chunks, &self.executed);
                for chunk in chunks {
                    if is_infrastructure(&chunk) {
                        self.executed.insert(chunk);
                    }
                }
                Ok(blob)
            }
            Err(e) => Err(diagnostics::render(
                &program_src,
                Level::Error,
                &e.message(),
                e.span(),
            )),
        }
    }

    /// `:type` — an expression's inferred type, without evaluating it.
    fn type_of(&self, expr: &str) -> Reply {
        let combined = format!("{}\n{expr}", self.defs);
        let analysis = pyfun::analyze(&combined);
        if !analysis.diagnostics.is_empty() {
            return Reply::error(render_errors(&combined, &analysis.diagnostics));
        }
        let ty = analysis
            .module
            .as_ref()
            .and_then(|m| m.items.last())
            .and_then(|item| match item {
                Item::Expr(e) => {
                    let span = e.span();
                    // The outermost node's type is recorded last (children first);
                    // desugared exprs reuse their span — take the last match.
                    analysis
                        .types
                        .iter()
                        .rev()
                        .find(|t| t.span == span)
                        .map(|t| t.ty.clone())
                }
                _ => None,
            });
        match ty {
            Some(ty) => Reply {
                ok: true,
                message: format!("{expr} : {ty}"),
                blob: String::new(),
            },
            None => Reply::error(format!(
                "(could not determine a type — is `{expr}` an expression?)"
            )),
        }
    }
}

/// Render type errors against the combined source into one string (the offending
/// line is in the new entry — the accumulated definitions were already valid).
fn render_errors(source: &str, errors: &[pyfun::types::TypeError]) -> String {
    let mut out = String::new();
    for e in errors {
        out.push_str(&diagnostics::render(
            source,
            Level::Error,
            &e.message,
            e.span,
        ));
        out.push('\n');
    }
    out
}
