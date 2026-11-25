//! Interactive REPL: `pyfun repl` (or bare `pyfun` with no file arg later).
//!
//! Pyfun is compiled, so there is no interpreter to step — instead the REPL pairs
//! the Rust checker with one **long-lived Python worker process** holding a single
//! namespace for the whole session. Each entry is type-checked (via `analyze`)
//! against the session's accumulated definition source, and what reaches Python is
//! the *diff*: the emitter is deterministic, so the emitted program is split into
//! top-level statement chunks and only chunks not yet `exec`'d in the worker are
//! sent (length-framed over the worker's stdin/stdout) and run in the persistent
//! namespace.
//!
//! Consequences (python/ghci/fsi-like semantics): a definition's effects run
//! exactly **once, at entry**; an expression runs once per entry (re-entering it
//! re-runs it); state — including top-level `let mut` — persists across entries.
//! A definition echoes its inferred type (GHCi-style); an ill-typed entry is
//! rejected and changes nothing. Re-entering a definition with a different body
//! emits different chunks, which rebind the names in the worker. If the worker
//! dies it is respawned and the namespace rebuilt by re-running the accumulated
//! definitions (their effects re-run; the REPL warns). Entries are single-line
//! unless wrapped in `:{ … :}`. Known limitation: entered code that *reads* stdin
//! would consume the framing protocol's bytes.

use std::collections::HashSet;
use std::io::{self, Read, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use pyfun::diagnostics::{self, Level};
use pyfun::syntax::Item;
use pyfun::traceback::Row;

/// The driver program fed to `python -c`, after the traceback hook's source
/// ([`driver`]): a tiny framed-message loop that `exec`s each received blob in
/// one persistent dict namespace, capturing stdout/stderr during the exec and
/// sending the captured text back. Each blob arrives with its table (which
/// source statement each of its lines came from; empty when there is none), so
/// an exception's traceback is rendered in the lines the user entered
/// (`DESIGN.md` §9). Framing, both directions: 8 ASCII digits (the payload byte
/// length) followed by that many UTF-8 bytes. EOF on stdin ends the loop.
const DRIVER_LOOP: &str = r#"
import io, json, traceback
ns = {}
tables = {}
count = 0
inp = sys.stdin.buffer
out = sys.stdout.buffer

def lookup(filename):
    if filename in tables:
        return tables[filename]
    # This driver's own frames (it is the `-c` program).
    return HIDE if filename == "<string>" else None

def read():
    header = inp.read(8)
    if len(header) < 8:
        return None
    return inp.read(int(header)).decode('utf-8')

while True:
    table = read()
    code = read()
    if table is None or code is None:
        break
    count += 1
    name = "<pyfun-%d>" % count
    if table:
        tables[name] = json.loads(table)
    buf = io.StringIO()
    saved = sys.stdout, sys.stderr
    sys.stdout = sys.stderr = buf
    try:
        exec(compile(code, name, "exec"), ns)
    except BaseException:
        try:
            buf.write(render(*sys.exc_info(), lookup))
        except Exception:
            traceback.print_exc()
    finally:
        sys.stdout, sys.stderr = saved
    payload = buf.getvalue().encode('utf-8')
    out.write(b'%08d' % len(payload))
    out.write(payload)
    out.flush()
"#;

/// The whole `-c` program: the traceback hook's definitions, then the loop.
fn driver() -> String {
    format!("{}\n{DRIVER_LOOP}", pyfun::traceback::HOOK_SOURCE)
}

/// Run the REPL loop. Returns success unless the environment is unusable (no
/// Python interpreter, or stdin closes — which is a normal exit).
pub fn run() -> std::process::ExitCode {
    let Some(interp) = python_cmd() else {
        eprintln!("no Python interpreter found on PATH (tried `python`, `python3`)");
        return std::process::ExitCode::FAILURE;
    };
    println!("Pyfun REPL — :help for commands, :quit to exit.");

    let mut session = Session::new(interp);
    let stdin = io::stdin();
    loop {
        print!("pyfun> ");
        let _ = io::stdout().flush();
        let mut line = String::new();
        match stdin.read_line(&mut line) {
            Ok(0) => {
                println!();
                break; // EOF (Ctrl-D)
            }
            Ok(_) => {}
            Err(e) => {
                eprintln!("input error: {e}");
                break;
            }
        }
        let entry = line.trim();
        if entry.is_empty() {
            continue;
        }
        // `:{` opens a multi-line entry, read until a line that is `:}` — needed to
        // enter mutually-recursive functions (both must be checked together) or any
        // multi-line construct.
        if entry == ":{" {
            let block = read_block(&stdin);
            if !block.trim().is_empty() {
                session.eval_entry(&block);
            }
            continue;
        }
        if let Some(cmd) = entry.strip_prefix(':') {
            if session.handle_command(cmd) {
                break; // a quit command
            }
            continue;
        }
        session.eval_entry(entry);
    }
    // `session` dropping here kills the Python worker (no zombie interpreters).
    std::process::ExitCode::SUCCESS
}

/// Read a multi-line block opened by `:{`, up to a line that is exactly `:}`
/// (or EOF). Returns the block's source (lines joined with newlines).
fn read_block(stdin: &io::Stdin) -> String {
    let mut block = String::new();
    loop {
        print!("...... ");
        let _ = io::stdout().flush();
        let mut line = String::new();
        match stdin.read_line(&mut line) {
            Ok(0) => break, // EOF ends the block
            Ok(_) => {}
            Err(_) => break,
        }
        if line.trim() == ":}" {
            break;
        }
        block.push_str(&line);
    }
    block
}

fn print_help() {
    println!("Commands:");
    println!("  :type <expr>   show an expression's inferred type (no evaluation)");
    println!("  :{{ … :}}        enter a multi-line block (e.g. mutually-recursive defs)");
    println!("  :reset         forget all definitions entered this session");
    println!("  :help          show this help");
    println!("  :quit          exit (or Ctrl-D)");
    println!("Enter a `let`/`type`/`extern` definition to remember it, or an");
    println!("expression to evaluate it.");
}

/// One REPL session: the accumulated definition source (the type-level state),
/// the set of emitted top-level Python chunks already `exec`'d in the worker
/// (the diffing state), and the worker itself (the runtime state).
struct Session {
    interp: String,
    /// Accumulated definition source (functions, types, values entered so far).
    defs: String,
    /// Where each line of `defs` was entered: one [`SourceLine`] per line.
    def_lines: Vec<SourceLine>,
    /// How many entries have been evaluated; the next one's number.
    entries: usize,
    /// Top-level Python chunks (see [`chunk_python`]) already executed in the
    /// worker's namespace — the emitter is deterministic, so recompiling the
    /// accumulated program re-yields these byte-for-byte and they are skipped.
    executed: HashSet<String>,
    /// The persistent Python process; spawned lazily, respawned after `:reset`
    /// or death.
    worker: Option<Worker>,
}

impl Session {
    fn new(interp: String) -> Session {
        Session {
            interp,
            defs: String::new(),
            def_lines: vec![opening_line()],
            entries: 0,
            executed: HashSet::new(),
            worker: None,
        }
    }

    /// Handle a `:command`. Returns `true` if the REPL should quit.
    fn handle_command(&mut self, cmd: &str) -> bool {
        let (name, arg) = match cmd.split_once(char::is_whitespace) {
            Some((n, a)) => (n, a.trim()),
            None => (cmd.trim(), ""),
        };
        match name {
            "q" | "quit" | "exit" => return true,
            "h" | "help" | "?" => print_help(),
            "reset" => {
                self.defs.clear();
                self.def_lines = vec![opening_line()];
                self.executed.clear();
                self.worker = None; // drop kills the process; a fresh one spawns on demand
                println!("(session reset)");
            }
            "t" | "type" => {
                if arg.is_empty() {
                    println!("usage: :type <expression>");
                } else {
                    self.show_type(arg);
                }
            }
            "" => {}
            other => println!("unknown command `:{other}` — :help for the list"),
        }
        false
    }

    /// Evaluate one non-command entry: classify as an expression or definition(s),
    /// type-check against the accumulated definitions, then run it in the worker.
    fn eval_entry(&mut self, entry: &str) {
        // Parse the entry alone to classify it (a lone expression vs definitions).
        let module = match pyfun::parse(entry) {
            Ok(m) => m,
            Err(e) => {
                eprintln!(
                    "{}",
                    diagnostics::render(entry, Level::Error, &e.message(), e.span())
                );
                return;
            }
        };
        let is_expression = module.items.len() == 1 && matches!(module.items[0], Item::Expr(_));

        // What a traceback calls this entry (`DESIGN.md` §9).
        self.entries += 1;
        let label = format!("<input {}>", self.entries);
        if is_expression {
            self.eval_expression(entry, &label);
        } else {
            self.add_definitions(entry, module.items.len(), &label);
        }
    }

    /// Type-check and evaluate an expression against the accumulated definitions,
    /// printing its value (nothing for a `unit`-typed expression — its effect, if
    /// any, still runs). Only the emitted chunks not already in the worker's
    /// namespace are sent; the expression's own statements are *not* remembered,
    /// so re-entering the same expression re-runs it. Infrastructure chunks it
    /// pulled in (imports, `_pf_*` helpers, classes) *are* remembered — they are
    /// idempotent, and resending a class would break `isinstance` identity.
    fn eval_expression(&mut self, entry: &str, label: &str) {
        let combined = format!("{}\n{entry}", self.defs);
        let analysis = pyfun::analyze(&combined);
        if !analysis.diagnostics.is_empty() {
            render_errors(&combined, &analysis.diagnostics);
            return;
        }
        render_warnings(&combined, &analysis, combined.len() - entry.len());
        // A `unit`-typed expression is run bare (its effect only); anything else is
        // wrapped in `print (…)` so its value is shown (and `None`/unit isn't printed).
        let is_unit = expression_is_unit(&analysis);
        let program_src = if is_unit {
            combined
        } else {
            format!("{}\nprint ({entry})", self.defs)
        };
        let mut lines = self.def_lines.clone();
        lines.extend(entry_lines(label, 1, entry));
        match Program::compile(&program_src, lines) {
            Ok(program) => {
                let output = self.run_program(&program);
                print!("{output}");
                let _ = io::stdout().flush();
                for chunk in program.chunks() {
                    if is_infrastructure(chunk) {
                        self.executed.insert(chunk.clone());
                    }
                }
            }
            Err(e) => eprintln!(
                "{}",
                diagnostics::render(&program_src, Level::Error, &e.message(), e.span())
            ),
        }
    }

    /// Type-check the new definition(s) against the session; on success, remember
    /// them, echo each new binding's inferred type, and execute the definitions'
    /// not-yet-run chunks in the worker — so a definition's effects run exactly
    /// once, at entry.
    fn add_definitions(&mut self, entry: &str, new_count: usize, label: &str) {
        let combined = format!("{}\n{entry}", self.defs);
        let analysis = pyfun::analyze(&combined);
        if !analysis.diagnostics.is_empty() {
            render_errors(&combined, &analysis.diagnostics);
            return;
        }
        render_warnings(&combined, &analysis, combined.len() - entry.len());
        // Echo the type of each newly-added top-level `let` binding (GHCi-style).
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
                            Some(ty) => println!("{name} : {ty}"),
                            None => println!("{name}"),
                        }
                    }
                }
            }
        }
        // Compile the whole accumulated program; only the chunks the worker hasn't
        // seen (the new definitions and anything they pulled in) are executed.
        let mut lines = self.def_lines.clone();
        lines.extend(entry_lines(label, 1, entry));
        match Program::compile(&combined, lines) {
            Ok(program) => {
                let output = self.run_program(&program);
                print!("{output}");
                let _ = io::stdout().flush();
                self.executed.extend(program.chunks().cloned());
                self.defs = combined;
                self.def_lines = program.into_lines();
            }
            Err(e) => eprintln!(
                "{}",
                diagnostics::render(&combined, Level::Error, &e.message(), e.span())
            ),
        }
    }

    /// Show an expression's inferred type without evaluating it (`:type`).
    fn show_type(&self, expr: &str) {
        let combined = format!("{}\n{expr}", self.defs);
        let analysis = pyfun::analyze(&combined);
        if !analysis.diagnostics.is_empty() {
            render_errors(&combined, &analysis.diagnostics);
            return;
        }
        let ty = analysis
            .module
            .as_ref()
            .and_then(|m| m.items.last())
            .and_then(|item| match item {
                Item::Expr(e) => {
                    let span = e.span();
                    // The outermost node's type is recorded last (children first), and a
                    // desugared expr (e.g. an operator section) reuses its span for the
                    // synthetic sub-nodes — so take the last match, not the first.
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
            Some(ty) => println!("{expr} : {ty}"),
            None => println!("(could not determine a type — is `{expr}` an expression?)"),
        }
    }

    /// Execute a compiled program's not-yet-run chunks in the worker and return
    /// the captured output. If the worker died, respawn it, rebuild the namespace
    /// by re-running the accumulated definitions (warning that their effects
    /// re-ran), and retry the entry once against the rebuilt state.
    fn run_program(&mut self, program: &Program) -> String {
        let (blob, rows) = program.pending(&self.executed);
        if blob.is_empty() {
            return String::new();
        }
        let had_worker = self.worker.is_some();
        match self.try_exec(&table_of(&rows), &blob) {
            Ok(output) => output,
            Err(e) => {
                if had_worker {
                    eprintln!("python worker error: {e}");
                } else {
                    eprintln!("cannot start python worker: {e}");
                    return String::new();
                }
                self.executed.clear();
                // Rebuild the namespace from the accumulated definitions.
                let mut output = String::new();
                if !self.defs.trim().is_empty() {
                    eprintln!(
                        "(python worker restarted — definitions re-executed to rebuild state)"
                    );
                    let Ok(defs) = Program::compile(&self.defs, self.def_lines.clone()) else {
                        // Cannot happen: `defs` only ever grows through successful compiles.
                        return output;
                    };
                    let (blob, rows) = defs.pending(&self.executed);
                    match self.try_exec(&table_of(&rows), &blob) {
                        Ok(out) => output.push_str(&out),
                        Err(e) => {
                            eprintln!("cannot restart python worker: {e}");
                            return output;
                        }
                    }
                    self.executed.extend(defs.chunks().cloned());
                }
                // Retry the entry, re-diffed against the rebuilt namespace (it may
                // need chunks that only earlier *expressions* had pulled in).
                let (blob, rows) = program.pending(&self.executed);
                if !blob.is_empty() {
                    match self.try_exec(&table_of(&rows), &blob) {
                        Ok(out) => output.push_str(&out),
                        Err(e) => eprintln!("python worker failed again: {e}"),
                    }
                }
                output
            }
        }
    }

    /// One protocol round-trip with the worker (spawning it if needed). On any
    /// I/O failure the worker is discarded (and killed) so the caller can retry
    /// against a fresh one.
    fn try_exec(&mut self, table: &str, blob: &str) -> io::Result<String> {
        if self.worker.is_none() {
            self.worker = Some(Worker::spawn(&self.interp)?);
        }
        let result = self
            .worker
            .as_mut()
            .expect("just spawned")
            .exec(table, blob);
        if result.is_err() {
            self.worker = None; // drop kills the dead/wedged process
        }
        result
    }
}

/// Whether the trailing expression of the analyzed module has type `unit`.
/// (Shared with the Jupyter kernel engine, `src/kernel.rs`, as are the chunk
/// helpers below — the engine is this session logic minus the worker.)
pub(crate) fn expression_is_unit(analysis: &pyfun::Analysis) -> bool {
    let Some(module) = &analysis.module else {
        return false;
    };
    let Some(Item::Expr(e)) = module.items.last() else {
        return false;
    };
    let span = e.span();
    analysis
        .types
        .iter()
        .any(|t| t.span == span && t.ty == "unit")
}

/// Render the warnings of the entry that begins at `entry_start`: the ones
/// about that entry alone. Binding a name again, or importing before use, is
/// how a session goes, and neither is reported here.
fn render_warnings(source: &str, analysis: &pyfun::Analysis, entry_start: usize) {
    for w in pyfun::entry_warnings(analysis, entry_start) {
        eprintln!(
            "{}",
            diagnostics::render(source, Level::Warning, &w.message, w.span)
        );
    }
}

/// Render type errors against the combined source (the offending line is in the
/// new entry, since the accumulated definitions were already validated).
fn render_errors(source: &str, errors: &[pyfun::types::TypeError]) {
    for e in errors {
        eprintln!(
            "{}",
            diagnostics::render(source, Level::Error, &e.message, e.span)
        );
    }
}

/// One line of a session's source, as its author entered it: what a
/// traceback names when a statement on that line fails.
#[derive(Debug, Clone)]
pub(crate) struct SourceLine {
    /// The entry the line belongs to (`<input 3>`, `In[3]`); empty for the
    /// blank line a session's source opens with.
    pub(crate) label: String,
    /// The line's number within what the author sees of that entry, 1-based.
    pub(crate) line: usize,
    pub(crate) text: String,
}

/// The line a session's source opens with: the definitions start empty, and
/// every entry is joined to them by a newline.
pub(crate) fn opening_line() -> SourceLine {
    SourceLine {
        label: String::new(),
        line: 0,
        text: String::new(),
    }
}

/// The lines of one entry. `first_line` is the line it starts on as its author
/// sees it: 1, or further down for the expression that closes a notebook cell.
pub(crate) fn entry_lines(label: &str, first_line: usize, entry: &str) -> Vec<SourceLine> {
    entry
        .split('\n')
        .enumerate()
        .map(|(i, text)| SourceLine {
            label: label.to_string(),
            line: first_line + i,
            text: text.trim_end_matches('\r').to_string(),
        })
        .collect()
}

/// A compiled session program: the accumulated definitions with one entry on
/// the end, as emitted Python chunks that know where they came from.
pub(crate) struct Program {
    /// The emitted Python's top-level chunks ([`chunk_python`]).
    chunks: Vec<(usize, String)>,
    /// Where the emitted lines came from, when the compiler could say.
    map: Option<pyfun::LineMap>,
    /// The source that was compiled.
    source: String,
    /// One [`SourceLine`] for each line of `source`.
    lines: Vec<SourceLine>,
}

impl Program {
    /// Compile `source`, whose lines were entered as `lines` says.
    pub(crate) fn compile(
        source: &str,
        lines: Vec<SourceLine>,
    ) -> Result<Program, pyfun::CompileError> {
        let (python, _notes, map) = pyfun::compile_session(source)?;
        Ok(Program {
            chunks: chunk_python(&python),
            map,
            source: source.to_string(),
            lines,
        })
    }

    /// Every chunk of the program, in order.
    pub(crate) fn chunks(&self) -> impl Iterator<Item = &String> {
        self.chunks.iter().map(|(_, chunk)| chunk)
    }

    /// The lines the program was compiled from.
    pub(crate) fn into_lines(self) -> Vec<SourceLine> {
        self.lines
    }

    /// The chunks not yet in `executed`, joined into one exec-able blob (in
    /// program order), with the rows of the blob's table: which entered
    /// statement each of its lines came from. The blob is empty when the
    /// namespace has already run everything; the rows are empty when the
    /// program has no line map.
    pub(crate) fn pending(&self, executed: &HashSet<String>) -> (String, Vec<Row>) {
        let mut blob = String::new();
        let mut rows = Vec::new();
        let mut blob_line = 1;
        for (start, chunk) in &self.chunks {
            if executed.contains(chunk) {
                continue;
            }
            let count = chunk.lines().count();
            if let Some(map) = &self.map {
                // A chunk is cut from the program and joined to others, so its
                // first line always opens a row, even one that says "no source".
                let mut current = None;
                for i in 0..count {
                    let span = statement_of(map, start + i);
                    if current != Some(span) {
                        rows.push(self.row(blob_line + i, span));
                        current = Some(span);
                    }
                }
            }
            blob.push_str(chunk);
            blob.push('\n');
            blob_line += count;
        }
        (blob, rows)
    }

    /// The row for blob lines from `emitted` on, lowered from the statement at
    /// `span` (or from no statement).
    fn row(&self, emitted: usize, span: Option<pyfun::lexer::Span>) -> Row {
        let none = Row {
            emitted,
            line: 0,
            text: Vec::new(),
            label: None,
        };
        let Some(span) = span else {
            return none;
        };
        let (first, last) = pyfun::traceback::statement_lines(&self.source, span);
        let Some(head) = self.lines.get(first - 1).filter(|l| !l.label.is_empty()) else {
            return none;
        };
        Row {
            emitted,
            line: head.line,
            text: self
                .lines
                .iter()
                .skip(first - 1)
                .take(last + 1 - first)
                .take_while(|l| l.label == head.label)
                .map(|l| l.text.clone())
                .collect(),
            label: Some(head.label.clone()),
        }
    }
}

/// The statement the emitted line `py_line` came from: the nearest entry of
/// the line map at or above it.
fn statement_of(map: &pyfun::LineMap, py_line: usize) -> Option<pyfun::lexer::Span> {
    map.iter()
        .take_while(|(line, _)| *line <= py_line)
        .last()
        .map(|(_, span)| *span)
}

/// The table that travels with a blob: one line of JSON, or nothing when there
/// are no rows.
pub(crate) fn table_of(rows: &[Row]) -> String {
    if rows.is_empty() {
        String::new()
    } else {
        pyfun::traceback::chunk_table(rows)
    }
}

/// Split emitted Python into top-level chunks, each with the line it starts
/// on (1-based): a chunk starts at a column-0 line and includes every
/// following blank or indented line; column-0 decorator lines
/// (`@dataclass(...)`) attach to the *following* `class`/`def` chunk. Chunks are
/// stored with trailing whitespace trimmed (so the same statement compares equal
/// whether or not later statements follow it) and no trailing newline.
pub(crate) fn chunk_python(src: &str) -> Vec<(usize, String)> {
    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut start = 1;
    // True while `current` holds only decorator lines — the next column-0 line
    // (the decorated def/class) then continues the chunk instead of opening one.
    let mut decorators_open = false;
    for (index, line) in src.lines().enumerate() {
        let col0 = !line.is_empty() && !line.starts_with(' ') && !line.starts_with('\t');
        if col0 && !decorators_open {
            push_chunk(&mut chunks, &mut current, start);
            start = index + 1;
        }
        current.push_str(line);
        current.push('\n');
        if col0 {
            decorators_open = line.starts_with('@');
        }
    }
    push_chunk(&mut chunks, &mut current, start);
    chunks
}

/// Finish the chunk in `current`, which started on line `start` (trimmed;
/// empty chunks are dropped).
fn push_chunk(chunks: &mut Vec<(usize, String)>, current: &mut String, start: usize) {
    let trimmed = current.trim_end();
    if !trimmed.is_empty() {
        chunks.push((start, trimmed.to_string()));
    }
    current.clear();
}

/// Whether a chunk is session infrastructure — imports, `_pf_*` helper defs, and
/// (decorated) class definitions — which an *expression* entry may permanently
/// contribute to the namespace. Everything else an expression emits is its own
/// one-shot statements, which must re-run if the expression is re-entered.
pub(crate) fn is_infrastructure(chunk: &str) -> bool {
    ["import ", "from ", "def _pf_", "@", "class "]
        .iter()
        .any(|prefix| chunk.starts_with(prefix))
}

/// The persistent Python worker: `python -c DRIVER`, spoken to over stdin/stdout
/// with 8-digit-length-framed UTF-8 messages. Dropping it kills the process.
struct Worker {
    child: Child,
    stdin: ChildStdin,
    stdout: ChildStdout,
}

impl Worker {
    fn spawn(interp: &str) -> io::Result<Worker> {
        let mut child = Command::new(interp)
            .args(["-c", &driver()])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // stderr inherited: a crash of the driver itself surfaces to the user.
            .spawn()?;
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        Ok(Worker {
            child,
            stdin,
            stdout,
        })
    }

    /// Send one blob of Python statements, with its table, to be `exec`'d in
    /// the worker's namespace; return the output (stdout+stderr, tracebacks
    /// included) it captured while running them.
    fn exec(&mut self, table: &str, blob: &str) -> io::Result<String> {
        for part in [table, blob] {
            let bytes = part.as_bytes();
            write!(self.stdin, "{:08}", bytes.len())?;
            self.stdin.write_all(bytes)?;
        }
        self.stdin.flush()?;
        let mut header = [0u8; 8];
        self.stdout.read_exact(&mut header)?;
        let len: usize = std::str::from_utf8(&header)
            .ok()
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "bad response header"))?;
        let mut payload = vec![0u8; len];
        self.stdout.read_exact(&mut payload)?;
        Ok(String::from_utf8_lossy(&payload).into_owned())
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait(); // reap — no zombie interpreters
    }
}

/// The first available Python interpreter, if any.
fn python_cmd() -> Option<String> {
    for candidate in ["python", "python3"] {
        if Command::new(candidate)
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok()
        {
            return Some(candidate.to_string());
        }
    }
    None
}
