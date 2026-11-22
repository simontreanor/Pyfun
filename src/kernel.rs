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

use pyfun::traceback::Row;

use crate::repl::{
    Program, SourceLine, entry_lines, expression_is_unit, is_infrastructure, opening_line, table_of,
};

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
        let mut reply = match op.as_str() {
            "eval" => engine.eval_cell(UNLABELLED, &payload),
            // The cell under the name a traceback should give it, and its
            // blob's table back: the table is the blob frame's first line.
            "eval-in" => {
                let (label, cell) = payload.split_once('\n').unwrap_or(("", &payload));
                let label = if label.is_empty() { UNLABELLED } else { label };
                let mut reply = engine.eval_cell(label, cell);
                if reply.ok {
                    reply.blob = format!("{}\n{}", table_of(&reply.rows), reply.blob);
                }
                reply
            }
            "type" => engine.type_of(&payload),
            // The traceback hook's source, for the kernel to execute once.
            "hook" => Reply {
                ok: true,
                message: pyfun::traceback::HOOK_SOURCE.to_string(),
                blob: String::new(),
                rows: Vec::new(),
            },
            other => Reply::error(format!("unknown op `{other}`")),
        };
        reply.rows.clear();
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
    /// The rows of the blob's table (`DESIGN.md` §9): which statement of the
    /// cell each of its lines came from.
    rows: Vec<Row>,
}

/// What a traceback calls a cell the kernel did not name.
const UNLABELLED: &str = "<cell>";

impl Reply {
    fn error(message: String) -> Reply {
        Reply {
            ok: false,
            message,
            blob: String::new(),
            rows: Vec::new(),
        }
    }
}

/// The kernel-side session state: mirrors the REPL's `Session` minus the worker.
struct Engine {
    /// Accumulated definition source (the type-level state).
    defs: String,
    /// Where each line of `defs` was entered: one [`SourceLine`] per line.
    def_lines: Vec<SourceLine>,
    /// Emitted top-level chunks the caller has been given to exec (the diffing
    /// state) — the emitter is deterministic, so recompiling the accumulated
    /// program re-yields these byte-for-byte and they are skipped.
    executed: HashSet<String>,
}

impl Engine {
    fn new() -> Engine {
        Engine {
            defs: String::new(),
            def_lines: vec![opening_line()],
            executed: HashSet::new(),
        }
    }

    /// Evaluate one cell: definitions accumulate (echoing their inferred types),
    /// a trailing expression is split off and evaluated per entry.
    fn eval_cell(&mut self, label: &str, cell: &str) -> Reply {
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
        let mut rows = Vec::new();

        if !defs_src.trim().is_empty() {
            match self.add_definitions(defs_src, def_items, label) {
                Ok((echoes, defs_blob, defs_rows)) => {
                    message.push_str(&echoes);
                    blob.push_str(&defs_blob);
                    rows.extend(defs_rows);
                }
                Err(rendered) => return Reply::error(rendered),
            }
        }

        if let Some(expr) = expr_src {
            // The expression's lines are counted from the top of the cell, and
            // its blob follows the definitions' in the one the kernel runs.
            let first_line = 1 + defs_src.matches('\n').count();
            let above = blob.matches('\n').count();
            match self.eval_expression(expr, label, first_line) {
                Ok((warned, expr_blob, expr_rows)) => {
                    message.push_str(&warned);
                    blob.push_str(&expr_blob);
                    rows.extend(expr_rows.into_iter().map(|row| Row {
                        emitted: row.emitted + above,
                        ..row
                    }));
                }
                Err(rendered) => return Reply::error(rendered),
            }
        }

        Reply {
            ok: true,
            message,
            blob,
            rows,
        }
    }

    /// Type-check new definitions against the session; on success remember them,
    /// return each new binding's type echo and the not-yet-executed chunks.
    fn add_definitions(
        &mut self,
        entry: &str,
        new_count: usize,
        label: &str,
    ) -> Result<(String, String, Vec<Row>), String> {
        let combined = format!("{}\n{entry}", self.defs);
        let analysis = pyfun::analyze(&combined);
        if !analysis.diagnostics.is_empty() {
            return Err(render_errors(&combined, &analysis.diagnostics));
        }
        // The cell's warnings lead its output, ahead of the type echoes.
        let mut echoes = render_warnings(&combined, &analysis, combined.len() - entry.len());
        if let Some(module) = &analysis.module {
            let start = module.items.len().saturating_sub(new_count);
            for item in &module.items[start..] {
                if let Item::Let(binding) = item {
                    // A destructuring binding echoes one line per name it binds.
                    for (name, span) in binding.bound_vars() {
                        let ty = analysis
                            .types
                            .iter()
                            .find(|t| t.span == span)
                            .map(|t| t.ty.as_str());
                        match ty {
                            Some(ty) => echoes.push_str(&format!("{name} : {ty}\n")),
                            None => echoes.push_str(&format!("{name}\n")),
                        }
                    }
                }
            }
        }
        let mut lines = self.def_lines.clone();
        lines.extend(entry_lines(label, 1, entry));
        match Program::compile(&combined, lines) {
            Ok(program) => {
                let (blob, rows) = program.pending(&self.executed);
                self.executed.extend(program.chunks().cloned());
                self.defs = combined;
                self.def_lines = program.into_lines();
                Ok((echoes, blob, rows))
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
    fn eval_expression(
        &mut self,
        entry: &str,
        label: &str,
        first_line: usize,
    ) -> Result<(String, String, Vec<Row>), String> {
        let combined = format!("{}\n{entry}", self.defs);
        let analysis = pyfun::analyze(&combined);
        if !analysis.diagnostics.is_empty() {
            return Err(render_errors(&combined, &analysis.diagnostics));
        }
        let warned = render_warnings(&combined, &analysis, combined.len() - entry.len());
        let is_unit = expression_is_unit(&analysis);
        let program_src = if is_unit {
            combined
        } else {
            format!("{}\nprint ({entry})", self.defs)
        };
        let mut lines = self.def_lines.clone();
        lines.extend(entry_lines(label, first_line, entry));
        match Program::compile(&program_src, lines) {
            Ok(program) => {
                let (blob, rows) = program.pending(&self.executed);
                for chunk in program.chunks() {
                    if is_infrastructure(chunk) {
                        self.executed.insert(chunk.clone());
                    }
                }
                Ok((warned, blob, rows))
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
                rows: Vec::new(),
            },
            None => Reply::error(format!(
                "(could not determine a type — is `{expr}` an expression?)"
            )),
        }
    }
}

/// Render type errors against the combined source into one string (the offending
/// line is in the new entry — the accumulated definitions were already valid).
/// The rendered warnings of the cell text that begins at `entry_start`: the
/// ones about that cell alone (a notebook binds names again and imports ahead
/// of use, so those kinds are not reported).
fn render_warnings(source: &str, analysis: &pyfun::Analysis, entry_start: usize) -> String {
    let mut out = String::new();
    for w in pyfun::entry_warnings(analysis, entry_start) {
        out.push_str(&diagnostics::render(
            source,
            Level::Warning,
            &w.message,
            w.span,
        ));
        out.push('\n');
    }
    out
}

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
