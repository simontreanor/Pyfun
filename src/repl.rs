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

/// The driver program fed to `python -c`: a tiny framed-message loop that
/// `exec`s each received blob in one persistent dict namespace, capturing
/// stdout/stderr during the exec (any exception's traceback included) and sending
/// the captured text back. Framing, both directions: 8 ASCII digits (the payload
/// byte length) followed by that many UTF-8 bytes. EOF on stdin ends the loop.
const DRIVER: &str = r#"
import io, sys, traceback
ns = {}
inp = sys.stdin.buffer
out = sys.stdout.buffer
while True:
    header = inp.read(8)
    if len(header) < 8:
        break
    code = inp.read(int(header)).decode('utf-8')
    buf = io.StringIO()
    saved = sys.stdout, sys.stderr
    sys.stdout = sys.stderr = buf
    try:
        exec(code, ns)
    except BaseException:
        traceback.print_exc()
    finally:
        sys.stdout, sys.stderr = saved
    payload = buf.getvalue().encode('utf-8')
    out.write(b'%08d' % len(payload))
    out.write(payload)
    out.flush()
"#;

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

        if is_expression {
            self.eval_expression(entry);
        } else {
            self.add_definitions(entry, module.items.len());
        }
    }

    /// Type-check and evaluate an expression against the accumulated definitions,
    /// printing its value (nothing for a `unit`-typed expression — its effect, if
    /// any, still runs). Only the emitted chunks not already in the worker's
    /// namespace are sent; the expression's own statements are *not* remembered,
    /// so re-entering the same expression re-runs it. Infrastructure chunks it
    /// pulled in (imports, `_pf_*` helpers, classes) *are* remembered — they are
    /// idempotent, and resending a class would break `isinstance` identity.
    fn eval_expression(&mut self, entry: &str) {
        let combined = format!("{}\n{entry}", self.defs);
        let analysis = pyfun::analyze(&combined);
        if !analysis.diagnostics.is_empty() {
            render_errors(&combined, &analysis.diagnostics);
            return;
        }
        // A `unit`-typed expression is run bare (its effect only); anything else is
        // wrapped in `print (…)` so its value is shown (and `None`/unit isn't printed).
        let is_unit = expression_is_unit(&analysis);
        let program_src = if is_unit {
            combined
        } else {
            format!("{}\nprint ({entry})", self.defs)
        };
        match pyfun::compile(&program_src) {
            Ok(python) => {
                let chunks = chunk_python(&python);
                let output = self.run_program(&chunks);
                print!("{output}");
                let _ = io::stdout().flush();
                for chunk in chunks {
                    if is_infrastructure(&chunk) {
                        self.executed.insert(chunk);
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
    fn add_definitions(&mut self, entry: &str, new_count: usize) {
        let combined = format!("{}\n{entry}", self.defs);
        let analysis = pyfun::analyze(&combined);
        if !analysis.diagnostics.is_empty() {
            render_errors(&combined, &analysis.diagnostics);
            return;
        }
        // Echo the type of each newly-added top-level `let` binding (GHCi-style).
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
                        Some(ty) => println!("{} : {ty}", binding.name),
                        None => println!("{}", binding.name),
                    }
                }
            }
        }
        // Compile the whole accumulated program; only the chunks the worker hasn't
        // seen (the new definitions and anything they pulled in) are executed.
        match pyfun::compile(&combined) {
            Ok(python) => {
                let chunks = chunk_python(&python);
                let output = self.run_program(&chunks);
                print!("{output}");
                let _ = io::stdout().flush();
                self.executed.extend(chunks);
                self.defs = combined;
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
    fn run_program(&mut self, chunks: &[String]) -> String {
        let blob = blob_of_new(chunks, &self.executed);
        if blob.is_empty() {
            return String::new();
        }
        let had_worker = self.worker.is_some();
        match self.try_exec(&blob) {
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
                    let Ok(python) = pyfun::compile(&self.defs) else {
                        // Cannot happen: `defs` only ever grows through successful compiles.
                        return output;
                    };
                    let def_chunks = chunk_python(&python);
                    match self.try_exec(&blob_of_new(&def_chunks, &self.executed)) {
                        Ok(out) => output.push_str(&out),
                        Err(e) => {
                            eprintln!("cannot restart python worker: {e}");
                            return output;
                        }
                    }
                    self.executed.extend(def_chunks);
                }
                // Retry the entry, re-diffed against the rebuilt namespace (it may
                // need chunks that only earlier *expressions* had pulled in).
                let blob = blob_of_new(chunks, &self.executed);
                if !blob.is_empty() {
                    match self.try_exec(&blob) {
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
    fn try_exec(&mut self, blob: &str) -> io::Result<String> {
        if self.worker.is_none() {
            self.worker = Some(Worker::spawn(&self.interp)?);
        }
        let result = self.worker.as_mut().expect("just spawned").exec(blob);
        if result.is_err() {
            self.worker = None; // drop kills the dead/wedged process
        }
        result
    }
}

/// Whether the trailing expression of the analyzed module has type `unit`.
fn expression_is_unit(analysis: &pyfun::Analysis) -> bool {
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

/// Split emitted Python into top-level chunks: a chunk starts at a column-0 line
/// and includes every following blank or indented line; column-0 decorator lines
/// (`@dataclass(...)`) attach to the *following* `class`/`def` chunk. Chunks are
/// stored with trailing whitespace trimmed (so the same statement compares equal
/// whether or not later statements follow it) and no trailing newline.
fn chunk_python(src: &str) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();
    // True while `current` holds only decorator lines — the next column-0 line
    // (the decorated def/class) then continues the chunk instead of opening one.
    let mut decorators_open = false;
    for line in src.lines() {
        let col0 = !line.is_empty() && !line.starts_with(' ') && !line.starts_with('\t');
        if col0 && !decorators_open {
            push_chunk(&mut chunks, &mut current);
        }
        current.push_str(line);
        current.push('\n');
        if col0 {
            decorators_open = line.starts_with('@');
        }
    }
    push_chunk(&mut chunks, &mut current);
    chunks
}

/// Finish the chunk in `current` (trimmed; empty chunks are dropped).
fn push_chunk(chunks: &mut Vec<String>, current: &mut String) {
    let trimmed = current.trim_end();
    if !trimmed.is_empty() {
        chunks.push(trimmed.to_string());
    }
    current.clear();
}

/// The not-yet-executed chunks of a program, joined into one exec-able blob (in
/// program order). Empty when the worker has already run everything.
fn blob_of_new(chunks: &[String], executed: &HashSet<String>) -> String {
    let new: Vec<&str> = chunks
        .iter()
        .filter(|c| !executed.contains(*c))
        .map(|s| s.as_str())
        .collect();
    if new.is_empty() {
        String::new()
    } else {
        new.join("\n") + "\n"
    }
}

/// Whether a chunk is session infrastructure — imports, `_pf_*` helper defs, and
/// (decorated) class definitions — which an *expression* entry may permanently
/// contribute to the namespace. Everything else an expression emits is its own
/// one-shot statements, which must re-run if the expression is re-entered.
fn is_infrastructure(chunk: &str) -> bool {
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
            .args(["-c", DRIVER])
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

    /// Send one blob of Python statements to be `exec`'d in the worker's
    /// namespace; return the output (stdout+stderr, tracebacks included) it
    /// captured while running them.
    fn exec(&mut self, blob: &str) -> io::Result<String> {
        let bytes = blob.as_bytes();
        write!(self.stdin, "{:08}", bytes.len())?;
        self.stdin.write_all(bytes)?;
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
