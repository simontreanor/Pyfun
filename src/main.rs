//! Phase 3 CLI: type-check, compile to Python, or echo the canonical form.
//!
//! Hand-rolled argument handling; the full clap subcommand surface (`fmt`,
//! `lsp`, …) arrives later — see `DESIGN.md` §10.
//!
//! Usage:
//!   pyfun check   <file.pyfun>                  # type-check, report diagnostics
//!   pyfun compile <file.pyfun> [-o <out>]       # type-check then lower to Python
//!   pyfun run     <file.pyfun>                  # compile then execute with Python
//!   pyfun parse   <file.pyfun>                  # canonical pretty-print
//!   pyfun <file.pyfun>                          # shorthand for `compile`
//!
//! `check`/`compile`/`run` operate over the **whole import graph** when the entry
//! file has `import`s (`DESIGN.md` §6.1): every module is checked/emitted, the
//! shared `_pyfun_rt.py` is produced, and `run` materializes the tree to a temp
//! directory. A file with **no imports behaves exactly as before** (single-file
//! path: `compile` to stdout/one file, `run` piped to the interpreter).

use std::io::Write;
use std::path::Path;
use std::process::{Command, ExitCode, Stdio};

use pyfun::diagnostics::{self, Level};
use pyfun::project::{self, ProjectError};
use pyfun::python_emitter::PyTarget;
use pyfun::syntax::{Item, Module};

mod kernel;
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
        Some("check") => match args.get(1) {
            Some(path) => check(path),
            None => fail("`check` needs a file path"),
        },
        Some("compile") => match parse_compile_args(&args[1..]) {
            Ok((path, out, target)) => compile(path, out.as_deref(), target),
            Err(msg) => fail(&msg),
        },
        Some("run") => match args.get(1) {
            Some(path) => run(path),
            None => fail("`run` needs a file path"),
        },
        Some("parse") => match args.get(1) {
            Some(path) => parse_only(path),
            None => fail("`parse` needs a file path"),
        },
        Some("lsp") => lsp_server(),
        Some("repl") => repl::run(),
        Some("kernel-engine") => kernel::run(),
        // Shorthand: a bare path means `compile <path>` to stdout.
        Some(path) => compile(path, None, PyTarget::default()),
    }
}

fn help() {
    eprintln!("pyfun {}", env!("CARGO_PKG_VERSION"));
    eprintln!();
    eprintln!("usage:");
    eprintln!("  pyfun check   <file.pyfun>                type-check, report diagnostics");
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
    eprintln!("  pyfun run     <file.pyfun>                compile then execute with Python");
    eprintln!("  pyfun parse   <file.pyfun>                canonical pretty-print");
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

fn check(path: &str) -> ExitCode {
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
        return check_project(path);
    }
    let (errors, _types, holes, _ordered) = pyfun::types::check_collecting(&module);
    if errors.is_empty() && holes.is_empty() {
        eprintln!("ok: no type errors");
        return ExitCode::SUCCESS;
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

fn compile(path: &str, out: Option<&str>, target: PyTarget) -> ExitCode {
    let Some(source) = read(path) else {
        return ExitCode::FAILURE;
    };
    if let Ok(module) = pyfun::parse(&source)
        && has_imports(&module)
    {
        return compile_project(path, out, target);
    }
    let python = match pyfun::compile_targeting(&source, target) {
        Ok(py) => py,
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

/// Compile `path` to Python (gated on type-checking), then execute it by piping
/// the emitted source to the interpreter's stdin (`python -`). The program's
/// stdout/stderr are inherited so its output appears directly; piping via stdin
/// sidesteps temp-file cleanup and Windows path-translation pitfalls.
fn run(path: &str) -> ExitCode {
    let Some(source) = read(path) else {
        return ExitCode::FAILURE;
    };
    if let Ok(module) = pyfun::parse(&source)
        && has_imports(&module)
    {
        return run_project(path);
    }
    let python = match pyfun::compile(&source) {
        Ok(py) => py,
        Err(e) => {
            eprintln!(
                "{}",
                diagnostics::render(&source, Level::Error, &e.message(), e.span())
            );
            return ExitCode::FAILURE;
        }
    };
    let Some(interp) = python_cmd() else {
        return fail("no Python interpreter found on PATH (tried `python`, `python3`)");
    };
    let mut child = match Command::new(&interp).arg("-").stdin(Stdio::piped()).spawn() {
        Ok(c) => c,
        Err(e) => return fail(&format!("cannot start `{interp}`: {e}")),
    };
    if let Some(mut stdin) = child.stdin.take()
        && let Err(e) = stdin.write_all(python.as_bytes())
    {
        return fail(&format!("cannot send program to `{interp}`: {e}"));
    }
    match child.wait() {
        Ok(status) if status.success() => ExitCode::SUCCESS,
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
                Err(_) => eprintln!("error: in module `{name}`: {}", error.message()),
            }
        }
        other => eprintln!("error: {other}"),
    }
    ExitCode::FAILURE
}

/// Type-check every module of the project, rendering each module's errors against
/// its own source. Returns `true` if the project type-checks.
fn check_project_ok(project: &project::Project) -> bool {
    let groups = project::check(project);
    if groups.is_empty() {
        return true;
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
    false
}

fn check_project(entry: &str) -> ExitCode {
    let project = match resolve_project(entry) {
        Ok(p) => p,
        Err(code) => return code,
    };
    if check_project_ok(&project) {
        eprintln!("ok: no type errors ({} modules)", project.modules.len());
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// Lower a checked project to its Python files, or render a lowering error.
fn lower_project(
    project: &project::Project,
    target: PyTarget,
) -> Result<Vec<(String, String)>, ExitCode> {
    match project::compile_targeting(project, target) {
        Ok(compiled) => Ok(compiled.files),
        Err(e) => {
            eprintln!("error: lowering failed: {}", e.message);
            Err(ExitCode::FAILURE)
        }
    }
}

fn compile_project(entry: &str, out: Option<&str>, target: PyTarget) -> ExitCode {
    let project = match resolve_project(entry) {
        Ok(p) => p,
        Err(code) => return code,
    };
    // The compiler is the gatekeeper: a project must type-check before it lowers.
    if !check_project_ok(&project) {
        return ExitCode::FAILURE;
    }
    let files = match lower_project(&project, target) {
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

fn run_project(entry: &str) -> ExitCode {
    let project = match resolve_project(entry) {
        Ok(p) => p,
        Err(code) => return code,
    };
    if !check_project_ok(&project) {
        return ExitCode::FAILURE;
    }
    let files = match lower_project(&project, PyTarget::default()) {
        Ok(f) => f,
        Err(code) => return code,
    };
    let Some(interp) = python_cmd() else {
        return fail("no Python interpreter found on PATH (tried `python`, `python3`)");
    };
    // Materialize the tree to a temp dir and run `python <entry>.py` there, so the
    // emitted `import geometry` / `_pyfun_rt` lines resolve as sibling modules.
    let dir = std::env::temp_dir().join(format!("pyfun_run_{}", std::process::id()));
    if let Err(e) = std::fs::create_dir_all(&dir) {
        return fail(&format!("cannot create temp dir: {e}"));
    }
    for (name, source) in &files {
        if let Err(e) = std::fs::write(dir.join(name), source) {
            let _ = std::fs::remove_dir_all(&dir);
            return fail(&format!("cannot stage {name}: {e}"));
        }
    }
    let entry_py = project::module_py_name(&project.entry().name);
    let status = Command::new(&interp)
        .arg(&entry_py)
        .current_dir(&dir)
        .status();
    let _ = std::fs::remove_dir_all(&dir);
    match status {
        Ok(s) if s.success() => ExitCode::SUCCESS,
        Ok(_) => ExitCode::FAILURE,
        Err(e) => fail(&format!("`{interp}` did not finish: {e}")),
    }
}

/// The first available Python interpreter command, if any.
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
fn parse_compile_args(args: &[String]) -> Result<(&str, Option<String>, PyTarget), String> {
    let mut path = None;
    let mut out = None;
    let mut target = PyTarget::default();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-o" | "--output" => {
                i += 1;
                out = Some(args.get(i).ok_or("`-o` needs a path")?.clone());
            }
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
    Ok((path.ok_or("`compile` needs a file path")?, out, target))
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
