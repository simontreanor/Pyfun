//! Phase 3 CLI: type-check, compile to Python, or echo the canonical form.
//!
//! Hand-rolled argument handling; the full clap subcommand surface (`fmt`,
//! `lsp`, …) arrives later — see `DESIGN.md` §10.
//!
//! Usage:
//!   pyfun check   <file.pyfun> [--strict]       # type-check, report diagnostics
//!   pyfun compile <file.pyfun> [-o <out>]       # type-check then lower to Python
//!   pyfun run     <file.pyfun> [--] [args...]   # compile then execute with Python
//!   pyfun parse   <file.pyfun>                  # canonical pretty-print
//!   pyfun <file.pyfun>                          # shorthand for `compile`
//!
//! `check`/`compile`/`run` operate over the **whole import graph** when the entry
//! file has `import`s (`DESIGN.md` §6.1): every module is checked/emitted, the
//! shared `_pyfun_rt.py` is produced, and `run` materializes the tree to a temp
//! directory. A file with **no imports behaves exactly as before** (single-file
//! path: `compile` to stdout/one file, `run` piped to the interpreter).

use std::path::Path;
use std::process::{Command, ExitCode, Stdio};

use pyfun::diagnostics::{self, Level};
use pyfun::project::{self, ProjectError};
use pyfun::python_emitter::PyTarget;
use pyfun::syntax::{Item, Module};
use pyfun::traceback::{self, Traced};

mod bundle;
mod kernel;
mod native_build;
mod packages;
mod repl;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None | Some("-h") | Some("--help") => {
            help();
            ExitCode::SUCCESS
        }
        Some("-V") | Some("--version") | Some("version") => {
            println!("pyfun {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Some("check") => {
            // `--strict` (either side of the path) makes a warning a failure,
            // for a CI job that keeps a codebase free of them.
            let strict = args[1..].iter().any(|a| a == "--strict");
            match args[1..].iter().find(|a| *a != "--strict") {
                Some(path) => check(path, strict),
                None => fail("`check` needs a file path"),
            }
        }
        Some("compile") => match parse_compile_args(&args[1..]) {
            Ok((path, out, target, native)) => compile(path, out.as_deref(), target, native),
            Err(msg) => fail(&msg),
        },
        Some("run") => match args.get(1) {
            Some(path) => {
                // Everything after the path belongs to the program (even a
                // `-o` or `--help`), so it reaches `sys.argv` exactly as it
                // would when the compiled file is run with `python` directly.
                // One leading `--` is an optional separator and is consumed.
                let prog_args = match args.get(2).map(String::as_str) {
                    Some("--") => &args[3..],
                    _ => &args[2..],
                };
                run(path, prog_args)
            }
            None => fail("`run` needs a file path"),
        },
        Some("parse") => match args.get(1) {
            Some(path) => parse_only(path),
            None => fail("`parse` needs a file path"),
        },
        Some("bundle") => match bundle::parse_args(&args[1..]) {
            Ok(parsed) => bundle::run(parsed),
            Err(msg) => fail(&msg),
        },
        Some("build") => native_build::run(&args[1..]),
        Some("add") => packages::add(&args[1..]),
        Some("install") => packages::install(),
        Some("remove") => packages::remove(&args[1..]),
        Some("lsp") => lsp_server(),
        Some("repl") => repl::run(),
        Some("kernel-engine") => kernel::run(),
        // Shorthand: a bare path means `compile <path>` to stdout.
        Some(path) => compile(path, None, PyTarget::default(), false),
    }
}

fn help() {
    eprintln!("pyfun {}", env!("CARGO_PKG_VERSION"));
    eprintln!();
    eprintln!("usage:");
    eprintln!(
        "  pyfun check   <file.pyfun> [--strict]     type-check; --strict fails on a warning"
    );
    eprintln!("  pyfun compile <file.pyfun> [-o <out>]     type-check then lower to Python");
    eprintln!(
        "                                            (-o is a file for one module, a dir for a project)"
    );
    eprintln!(
        "                [--target 3.11|3.12]        emission target (default 3.12; 3.11 avoids"
    );
    eprintln!(
        "                                            PEP 701 f-strings so the output runs on PyPy)"
    );
    eprintln!(
        "                [--native]                  lower matches on your own types to isinstance"
    );
    eprintln!("                                            ladders, the form mypyc compiles");
    eprintln!("  pyfun run     <file.pyfun> [--] [args...] compile then execute with Python");
    eprintln!(
        "  pyfun build   --native <file.pyfun> -o <dir>  compile to a C extension with mypyc;"
    );
    eprintln!("                                            run the result with `python <dir>`");
    eprintln!(
        "                                            (args after the path go to the program's sys.argv)"
    );
    eprintln!("  pyfun parse   <file.pyfun>                canonical pretty-print");
    eprintln!("  pyfun bundle  <file.pyfun> -o <dir>       a static web page running the program");
    eprintln!(
        "                [--asset <file>]... [--page <fragment.html>]   on Pyodide (CPython in WebAssembly)"
    );
    eprintln!(
        "  pyfun add     <package>...                install a Pyfun façade package and vendor"
    );
    eprintln!(
        "                                            its .pyfun files (records it in pyfun.toml)"
    );
    eprintln!("  pyfun install                             install every façade pyfun.toml lists");
    eprintln!(
        "  pyfun remove  <package>...                drop a façade from pyfun.toml and the project"
    );
    eprintln!("  pyfun lsp                                 run the language server (stdio)");
    eprintln!("  pyfun repl                                interactive read-eval-print loop");
    eprintln!(
        "  pyfun kernel-engine                       session engine for the Jupyter kernel (stdio)"
    );
    eprintln!("  pyfun --version                           print the compiler version");
    eprintln!("  pyfun <file.pyfun>                        shorthand for `compile`");
}

/// Run the language server, speaking LSP over stdin/stdout until `exit`.
fn lsp_server() -> ExitCode {
    match pyfun::lsp::run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => fail(&format!("lsp server error: {e}")),
    }
}

/// Print `warnings` against `source`, one rendered diagnostic each.
fn report_warnings(source: &str, warnings: &[pyfun::diagnostics::Warning]) {
    for w in warnings {
        eprintln!(
            "{}\n",
            diagnostics::render(source, Level::Warning, &w.message, w.span)
        );
    }
}

/// The outcome of a check that found no errors: success, unless `--strict` was
/// given and there were warnings.
fn checked_ok(summary: &str, warnings: usize, strict: bool) -> ExitCode {
    if strict && warnings > 0 {
        eprintln!(
            "{}{}",
            summary.trim_start_matches("ok: "),
            warning_count(warnings)
        );
        eprintln!("error: `--strict` makes a warning a failure");
        return ExitCode::FAILURE;
    }
    eprintln!("{summary}{}", warning_count(warnings));
    ExitCode::SUCCESS
}

fn check(path: &str, strict: bool) -> ExitCode {
    let Some(source) = read(path) else {
        return ExitCode::FAILURE;
    };
    let module = match pyfun::parse(&source) {
        Ok(m) => m,
        Err(e) => {
            eprintln!(
                "{}",
                diagnostics::render(&source, Level::Error, &e.message(), e.span())
            );
            return ExitCode::FAILURE;
        }
    };
    // A file with imports is the entry of a multi-module project.
    if has_imports(&module) {
        return check_project(path, strict);
    }
    let (errors, _types, holes, warnings) =
        pyfun::types::check_reporting(&module, &std::collections::HashMap::new());
    // Warnings come first and never change the outcome: the program below them
    // still compiles, or still has the errors that follow.
    report_warnings(&source, &warnings);
    if errors.is_empty() && holes.is_empty() {
        return checked_ok("ok: no type errors", warnings.len(), strict);
    }
    let mut first = true;
    for e in &errors {
        if !first {
            eprintln!();
        }
        first = false;
        eprintln!(
            "{}",
            diagnostics::render(&source, Level::Error, &e.message, e.span)
        );
    }
    // Typed holes are reported informatively (a "note", not an error), but they
    // keep `check` non-zero so a leftover hole is caught.
    for h in &holes {
        if !first {
            eprintln!();
        }
        first = false;
        eprintln!(
            "{}",
            diagnostics::render(&source, Level::Note, &h.message(), h.span)
        );
    }
    let n = errors.len();
    let hn = holes.len();
    if n > 0 {
        eprintln!("\n{n} error{}", if n == 1 { "" } else { "s" });
    }
    if hn > 0 {
        eprintln!("{hn} unfilled hole{}", if hn == 1 { "" } else { "s" });
    }
    ExitCode::FAILURE
}

fn compile(path: &str, out: Option<&str>, target: PyTarget, native: bool) -> ExitCode {
    let Some(source) = read(path) else {
        return ExitCode::FAILURE;
    };
    if let Ok(module) = pyfun::parse(&source)
        && has_imports(&module)
    {
        return compile_project(path, out, target, native);
    }
    let python = match pyfun::compile_with(&source, target, native) {
        Ok((py, notes)) => {
            report_warnings(&source, &pyfun::warnings(&source));
            report_notes(&notes);
            py
        }
        Err(e) => {
            eprintln!(
                "{}",
                diagnostics::render(&source, Level::Error, &e.message(), e.span())
            );
            return ExitCode::FAILURE;
        }
    };
    match out {
        Some(out_path) => match std::fs::write(out_path, python) {
            Ok(()) => {
                eprintln!("wrote {out_path}");
                ExitCode::SUCCESS
            }
            Err(e) => fail(&format!("cannot write {out_path}: {e}")),
        },
        None => {
            print!("{python}");
            ExitCode::SUCCESS
        }
    }
}

/// Compile `path` to Python (gated on type-checking), then execute it from a
/// temp file (`python <file>.py`), inheriting stdin/stdout/stderr so the program
/// reads the terminal and its output appears directly.
///
/// The obvious alternative, piping the emitted source to `python -`, spends the
/// program's *own* stdin on its source text, so the first `input` in an
/// interactive program raises `EOFError`. This mirrors what [`run_project`]
/// already does. The staged file is named `_pyfun_main.py` rather than after the
/// source: Python puts the script's directory first on `sys.path`, so a source
/// stem like `json.pyfun` would otherwise shadow the stdlib module an `extern`
/// in that very program imports.
///
/// `prog_args` are handed to the interpreter after the script path, so the
/// program sees them as `sys.argv[1..]`; `sys.argv[0]` is the staged script
/// path, matching what `python out/main.py args...` gives the compiled form.
///
/// An uncaught error is reported in `.pyfun` lines ([`stage_and_run`]).
fn run(path: &str, prog_args: &[String]) -> ExitCode {
    let Some(source) = read(path) else {
        return ExitCode::FAILURE;
    };
    if let Ok(module) = pyfun::parse(&source)
        && has_imports(&module)
    {
        return run_project(path, prog_args);
    }
    let (python, map) = match pyfun::compile_traced(&source) {
        Ok((py, notes, map)) => {
            report_warnings(&source, &pyfun::warnings(&source));
            report_notes(&notes);
            (py, map)
        }
        Err(e) => {
            eprintln!(
                "{}",
                diagnostics::render(&source, Level::Error, &e.message(), e.span())
            );
            return ExitCode::FAILURE;
        }
    };
    let entry_py = "_pyfun_main.py";
    let traced: Vec<Traced> = map
        .iter()
        .map(|map| Traced {
            py_name: entry_py,
            source_path: path,
            source: &source,
            map,
        })
        .collect();
    stage_and_run(
        &[(entry_py.to_string(), python)],
        entry_py,
        &traced,
        prog_args,
    )
}

/// Write `files` to a fresh temp directory and run `entry_py` there with the
/// Python interpreter, handing it `prog_args`.
///
/// The interpreter is given an absolute script path and *no* `current_dir`: the
/// program keeps the user's working directory, so its relative file paths mean
/// what they meant on the command line, while Python still puts the script's
/// own directory first on `sys.path` (which is how a project's sibling modules
/// and `_pyfun_rt` resolve).
///
/// With line maps in `traced`, the script is a launcher that runs the entry
/// program under the traceback hook (`pyfun::traceback`, `DESIGN.md` §9), so an
/// uncaught error names `.pyfun` lines. `PYFUN_TRACEBACK=python` runs the entry
/// program directly, for Python's own traceback over the emitted code.
fn stage_and_run(
    files: &[(String, String)],
    entry_py: &str,
    traced: &[Traced],
    prog_args: &[String],
) -> ExitCode {
    let Some(interp) = python_cmd() else {
        return fail("no Python interpreter found on PATH (tried `python`, `python3`)");
    };
    let dir = std::env::temp_dir().join(format!("pyfun_run_{}", std::process::id()));
    if let Err(e) = std::fs::create_dir_all(&dir) {
        return fail(&format!("cannot create temp dir: {e}"));
    }
    let python_traceback = std::env::var("PYFUN_TRACEBACK").is_ok_and(|v| v == "python");
    let hooked = !traced.is_empty() && !python_traceback;
    let mut staged: Vec<(&str, std::borrow::Cow<str>)> = files
        .iter()
        .map(|(name, source)| (name.as_str(), source.as_str().into()))
        .collect();
    if hooked {
        staged.push((
            traceback::LAUNCHER_FILE,
            traceback::launcher(entry_py).into(),
        ));
        staged.push((traceback::HOOK_FILE, traceback::HOOK_SOURCE.into()));
        staged.push((traceback::TABLE_FILE, traceback::table(traced).into()));
    }
    for (name, source) in &staged {
        if let Err(e) = std::fs::write(dir.join(name), source.as_bytes()) {
            let _ = std::fs::remove_dir_all(&dir);
            return fail(&format!("cannot stage {name}: {e}"));
        }
    }
    let script = if hooked {
        traceback::LAUNCHER_FILE
    } else {
        entry_py
    };
    let status = Command::new(&interp)
        .arg(dir.join(script))
        .args(prog_args)
        .status();
    let _ = std::fs::remove_dir_all(&dir);
    match status {
        Ok(s) if s.success() => ExitCode::SUCCESS,
        Ok(_) => ExitCode::FAILURE,
        Err(e) => fail(&format!("`{interp}` did not finish: {e}")),
    }
}

/// Whether `module` opens a multi-module project (has at least one `import`).
fn has_imports(module: &Module) -> bool {
    module
        .items
        .iter()
        .any(|i| matches!(i, Item::Import { .. }))
}

/// Resolve the import graph rooted at `entry`, rendering any graph error
/// (missing file, cycle, or a lex/parse failure in some module) and returning
/// the exit code to use on failure.
fn resolve_project(entry: &str) -> Result<project::Project, ExitCode> {
    project::build_from_path(Path::new(entry)).map_err(|e| render_project_error(entry, &e))
}

/// Render a graph-resolution error. A lex/parse failure is shown rustc-style
/// against the offending module's source; the rest are one-line messages.
fn render_project_error(entry: &str, error: &ProjectError) -> ExitCode {
    match error {
        ProjectError::Compile { name, error } => {
            let root = Path::new(entry).parent().unwrap_or_else(|| Path::new("."));
            let file = root.join(project::module_file_name(name));
            match std::fs::read_to_string(&file) {
                Ok(src) => eprintln!(
                    "{}",
                    diagnostics::render(&src, Level::Error, &error.message(), error.span())
                ),
                // The module's source could not be re-read, so nothing renders
                // the span; the error's `Display` keeps the byte offsets as the
                // only location information this path has.
                Err(_) => eprintln!("error: in module `{name}`: {error}"),
            }
        }
        other => eprintln!("error: {other}"),
    }
    ExitCode::FAILURE
}

/// Type-check every module of the project, rendering each module's errors against
/// its own source. Returns `true` if the project type-checks.
fn check_project_ok(project: &project::Project) -> bool {
    check_project_reporting(project).is_some()
}

/// ` (2 warnings)`, or nothing when there are none: the tail of `check`'s
/// closing line.
fn warning_count(n: usize) -> String {
    match n {
        0 => String::new(),
        1 => " (1 warning)".to_string(),
        n => format!(" ({n} warnings)"),
    }
}

/// Type-check the project, printing each module's warnings and errors against
/// its own source. `Some(count of warnings)` if it type-checks.
fn check_project_reporting(project: &project::Project) -> Option<usize> {
    let (groups, warned) = project::check_reporting(project);
    let source_of = |name: &str| {
        project
            .modules
            .iter()
            .find(|m| m.name == name)
            .map(|m| m.source.as_str())
            .unwrap_or("")
    };
    let mut shown = 0;
    for group in &warned {
        eprintln!(
            "-- module `{}` ({}) --",
            group.name,
            project::module_file_name(&group.name)
        );
        report_warnings(source_of(&group.name), &group.warnings);
        shown += group.warnings.len();
    }
    if groups.is_empty() {
        return Some(shown);
    }
    let mut total = 0;
    for group in &groups {
        let source = project
            .modules
            .iter()
            .find(|m| m.name == group.name)
            .map(|m| m.source.as_str())
            .unwrap_or("");
        eprintln!(
            "-- module `{}` ({}) --",
            group.name,
            project::module_file_name(&group.name)
        );
        for e in &group.errors {
            eprintln!(
                "{}",
                diagnostics::render(source, Level::Error, &e.message, e.span)
            );
            total += 1;
        }
    }
    eprintln!("\n{total} error{}", if total == 1 { "" } else { "s" });
    None
}

fn check_project(entry: &str, strict: bool) -> ExitCode {
    let project = match resolve_project(entry) {
        Ok(p) => p,
        Err(code) => return code,
    };
    match check_project_reporting(&project) {
        Some(warnings) => checked_ok(
            &format!("ok: no type errors ({} modules)", project.modules.len()),
            warnings,
            strict,
        ),
        None => ExitCode::FAILURE,
    }
}

/// Print lowering notes to stderr. These are neither errors nor part of the
/// emitted program: they say something the author would otherwise have to read
/// the generated Python to discover.
fn report_notes(notes: &[String]) {
    for note in notes {
        eprintln!("note: {note}");
    }
}

/// Lower a checked project to its Python files, or render a lowering error.
fn lower_project(
    project: &project::Project,
    target: PyTarget,
    native: bool,
) -> Result<Vec<(String, String)>, ExitCode> {
    match project::compile_with(project, target, native) {
        Ok(compiled) => {
            report_notes(&compiled.notes);
            Ok(compiled.files)
        }
        Err(e) => {
            eprintln!("error: lowering failed: {}", e.message);
            Err(ExitCode::FAILURE)
        }
    }
}

fn compile_project(entry: &str, out: Option<&str>, target: PyTarget, native: bool) -> ExitCode {
    let project = match resolve_project(entry) {
        Ok(p) => p,
        Err(code) => return code,
    };
    // The compiler is the gatekeeper: a project must type-check before it lowers.
    if !check_project_ok(&project) {
        return ExitCode::FAILURE;
    }
    let files = match lower_project(&project, target, native) {
        Ok(f) => f,
        Err(code) => return code,
    };
    match out {
        // For a project, `-o` names a directory holding the whole `.py` tree.
        Some(dir) => {
            if let Err(e) = std::fs::create_dir_all(dir) {
                return fail(&format!("cannot create {dir}: {e}"));
            }
            for (name, source) in &files {
                let path = Path::new(dir).join(name);
                if let Err(e) = std::fs::write(&path, source) {
                    return fail(&format!("cannot write {}: {e}", path.display()));
                }
            }
            eprintln!("wrote {} files to {dir}", files.len());
            ExitCode::SUCCESS
        }
        // No `-o`: print each file to stdout under a header banner.
        None => {
            for (name, source) in &files {
                println!("# ==== {name} ====");
                print!("{source}");
                println!();
            }
            ExitCode::SUCCESS
        }
    }
}

fn run_project(entry: &str, prog_args: &[String]) -> ExitCode {
    let project = match resolve_project(entry) {
        Ok(p) => p,
        Err(code) => return code,
    };
    if !check_project_ok(&project) {
        return ExitCode::FAILURE;
    }
    let compiled = match project::compile_traced(&project) {
        Ok(compiled) => compiled,
        Err(e) => {
            eprintln!("error: lowering failed: {}", e.message);
            return ExitCode::FAILURE;
        }
    };
    report_notes(&compiled.notes);
    // Each module's frames name its own source file, spelled the way the entry
    // was given on the command line.
    let root = Path::new(entry).parent().unwrap_or_else(|| Path::new(""));
    let entry_name = &project.entry().name;
    let names: Vec<(String, String)> = compiled
        .line_maps
        .iter()
        .map(|(module, _)| {
            let path = if module == entry_name {
                entry.to_string()
            } else {
                let file = project::locate_module(root, module)
                    .unwrap_or_else(|| root.join(project::module_file_name(module)))
                    .display()
                    .to_string();
                if entry.contains('/') {
                    file.replace('\\', "/")
                } else {
                    file
                }
            };
            (project::module_py_name(module), path)
        })
        .collect();
    let traced: Vec<Traced> = compiled
        .line_maps
        .iter()
        .zip(&names)
        .filter_map(|((module, map), (py_name, source_path))| {
            let source = &project.modules.iter().find(|m| &m.name == module)?.source;
            Some(Traced {
                py_name,
                source_path,
                source,
                map,
            })
        })
        .collect();
    let entry_py = project::module_py_name(entry_name);
    stage_and_run(&compiled.files, &entry_py, &traced, prog_args)
}

/// The Python interpreter to use: `PYFUN_PYTHON` when set (so `run` and `add`
/// can target a virtual environment), else the first of `python`/`python3`.
fn python_cmd() -> Option<String> {
    if let Ok(python) = std::env::var("PYFUN_PYTHON")
        && !python.is_empty()
    {
        return Some(python);
    }
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

fn parse_only(path: &str) -> ExitCode {
    let Some(source) = read(path) else {
        return ExitCode::FAILURE;
    };
    match pyfun::format(&source) {
        Ok(canonical) => {
            print!("{canonical}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!(
                "{}",
                diagnostics::render(&source, Level::Error, &e.message(), e.span())
            );
            ExitCode::FAILURE
        }
    }
}

/// Parse `compile` arguments: a required path, an optional `-o <out>`, and an
/// optional `--target 3.11|3.12` (default 3.12 — see `python_emitter::PyTarget`).
fn parse_compile_args(args: &[String]) -> Result<(&str, Option<String>, PyTarget, bool), String> {
    let mut path = None;
    let mut out = None;
    let mut target = PyTarget::default();
    let mut native = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-o" | "--output" => {
                i += 1;
                out = Some(args.get(i).ok_or("`-o` needs a path")?.clone());
            }
            "--native" => native = true,
            "--target" => {
                i += 1;
                target = match args.get(i).map(String::as_str) {
                    Some("3.11") => PyTarget::Py311,
                    Some("3.12") => PyTarget::Py312,
                    Some(other) => {
                        return Err(format!("`--target` must be 3.11 or 3.12, got `{other}`"));
                    }
                    None => return Err("`--target` needs a version (3.11 or 3.12)".to_string()),
                };
            }
            p if path.is_none() => path = Some(p),
            other => return Err(format!("unexpected argument `{other}`")),
        }
        i += 1;
    }
    Ok((
        path.ok_or("`compile` needs a file path")?,
        out,
        target,
        native,
    ))
}

fn read(path: &str) -> Option<String> {
    match std::fs::read_to_string(path) {
        Ok(s) => Some(s),
        Err(e) => {
            eprintln!("error: cannot read {path}: {e}");
            None
        }
    }
}

fn fail(message: &str) -> ExitCode {
    eprintln!("error: {message}");
    ExitCode::FAILURE
}
