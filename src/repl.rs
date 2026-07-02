//! Interactive REPL: `pyfun repl` (or bare `pyfun` with no file arg later).
//!
//! Pyfun is compiled, so there is no interpreter to step — instead the REPL keeps
//! the session's **definitions** as accumulated Pyfun source, and on each entry
//! re-checks and (for an expression) recompiles the accumulated program plus that
//! expression and runs it through Python once, showing the value. Definitions echo
//! their inferred type (GHCi-style) and are remembered; expressions are one-shot.
//!
//! Model + limitations (MVP): entries are single-line; state is the set of
//! definitions (they are re-run on each expression eval, so *pure* definitions feel
//! persistent, but a top-level effect or `let mut` does not carry across entries).
//! A persistent-Python-process design (true cross-entry state) is a future step.

use std::io::{self, Write};
use std::process::{Command, Stdio};

use pyfun::diagnostics::{self, Level};
use pyfun::syntax::Item;

/// Run the REPL loop. Returns success unless the environment is unusable (no
/// Python interpreter, or stdin closes — which is a normal exit).
pub fn run() -> std::process::ExitCode {
    let Some(interp) = python_cmd() else {
        eprintln!("no Python interpreter found on PATH (tried `python`, `python3`)");
        return std::process::ExitCode::FAILURE;
    };
    println!("Pyfun REPL — :help for commands, :quit to exit.");

    // Accumulated definition source (functions, types, values entered so far).
    let mut defs = String::new();
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
                eval_entry(&block, &mut defs, &interp);
            }
            continue;
        }
        if let Some(cmd) = entry.strip_prefix(':') {
            if handle_command(cmd, &mut defs) {
                break; // a quit command
            }
            continue;
        }
        eval_entry(entry, &mut defs, &interp);
    }
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

/// Handle a `:command`. Returns `true` if the REPL should quit.
fn handle_command(cmd: &str, defs: &mut String) -> bool {
    let (name, arg) = match cmd.split_once(char::is_whitespace) {
        Some((n, a)) => (n, a.trim()),
        None => (cmd.trim(), ""),
    };
    match name {
        "q" | "quit" | "exit" => return true,
        "h" | "help" | "?" => print_help(),
        "reset" => {
            defs.clear();
            println!("(session reset)");
        }
        "t" | "type" => {
            if arg.is_empty() {
                println!("usage: :type <expression>");
            } else {
                show_type(arg, defs);
            }
        }
        "" => {}
        other => println!("unknown command `:{other}` — :help for the list"),
    }
    false
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

/// Evaluate one non-command entry: classify as an expression or definition(s),
/// type-check against the accumulated definitions, then run or remember it.
fn eval_entry(entry: &str, defs: &mut String, interp: &str) {
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
        eval_expression(entry, defs, interp);
    } else {
        add_definitions(entry, module.items.len(), defs);
    }
}

/// Type-check and evaluate an expression against the accumulated definitions,
/// printing its value (nothing for a `unit`-typed expression — its effect, if any,
/// still runs).
fn eval_expression(entry: &str, defs: &str, interp: &str) {
    let combined = format!("{defs}\n{entry}");
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
        format!("{defs}\nprint ({entry})")
    };
    match pyfun::compile(&program_src) {
        Ok(python) => {
            let output = run_python(interp, &python);
            print!("{output}");
            let _ = io::stdout().flush();
        }
        Err(e) => eprintln!(
            "{}",
            diagnostics::render(&program_src, Level::Error, &e.message(), e.span())
        ),
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

/// Type-check the new definition(s) against the session; on success, remember them
/// and echo each new binding's inferred type.
fn add_definitions(entry: &str, new_count: usize, defs: &mut String) {
    let combined = format!("{defs}\n{entry}");
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
    *defs = combined;
}

/// Show an expression's inferred type without evaluating it (`:type`).
fn show_type(expr: &str, defs: &str) {
    let combined = format!("{defs}\n{expr}");
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

/// Run a complete Python program, capturing its stdout and stderr (a runtime
/// error's traceback lands on stderr). Returns the combined output.
fn run_python(interp: &str, program: &str) -> String {
    let mut child = match Command::new(interp)
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => return format!("cannot start `{interp}`: {e}\n"),
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(program.as_bytes());
    }
    match child.wait_with_output() {
        Ok(out) => {
            let mut s = String::from_utf8_lossy(&out.stdout).into_owned();
            s.push_str(&String::from_utf8_lossy(&out.stderr));
            s
        }
        Err(e) => format!("`{interp}` did not finish: {e}\n"),
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
