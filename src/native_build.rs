//! `pyfun build --native <file.pyfun> -o <dir>`: compile a program to a native
//! extension with mypyc (`DESIGN.md` §5.6).
//!
//! The program is compiled with `--native` (every match an `isinstance` ladder,
//! top-level functions annotated from their inferred types) into `<dir>`, mypyc
//! builds that module into a C extension beside it, the Python source is removed
//! so an import finds the extension, and a `__main__.py` imports it, so the
//! result runs as `python <dir>`. Needs `mypy` (which ships mypyc) and a C
//! compiler in the interpreter's environment; on Windows that means the MSVC
//! Build Tools.

use std::path::Path;
use std::process::{Command, ExitCode};

use pyfun::python_emitter::PyTarget;

/// Parse `build` arguments: `--native` (required for now, since native is the
/// only build there is), a path, and `-o <dir>`.
pub fn run(args: &[String]) -> ExitCode {
    let mut native = false;
    let mut path = None;
    let mut out = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--native" => native = true,
            "-o" | "--output" => {
                i += 1;
                out = args.get(i).cloned();
            }
            p if path.is_none() => path = Some(p.to_string()),
            other => return fail(&format!("unexpected argument `{other}`")),
        }
        i += 1;
    }
    if !native {
        return fail(
            "`build` compiles with mypyc and needs `--native` (pyfun build --native <file> -o <dir>)",
        );
    }
    let Some(path) = path else {
        return fail("`build` needs a file path");
    };
    let Some(out) = out else {
        return fail("`build` needs an output directory (-o <dir>)");
    };
    build(Path::new(&path), Path::new(&out))
}

fn build(path: &Path, out: &Path) -> ExitCode {
    let source = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => return fail(&format!("cannot read {}: {e}", path.display())),
    };
    if let Ok(module) = pyfun::parse(&source)
        && crate::has_imports(&module)
    {
        return fail("`build --native` supports a single file so far, not a project");
    }
    let python = match pyfun::compile_with(&source, PyTarget::default(), true) {
        Ok((py, notes)) => {
            crate::report_notes(&notes);
            py
        }
        Err(e) => {
            eprintln!(
                "{}",
                pyfun::diagnostics::render(
                    &source,
                    pyfun::diagnostics::Level::Error,
                    &e.message(),
                    e.span()
                )
            );
            return ExitCode::FAILURE;
        }
    };
    let Some(module) = pyfun::project::module_name_from_path(path).map(|m| m.to_lowercase()) else {
        return fail("the file needs a name");
    };
    if let Err(e) = std::fs::create_dir_all(out) {
        return fail(&format!("cannot create {}: {e}", out.display()));
    }
    let source_file = out.join(format!("{module}.py"));
    if let Err(e) = std::fs::write(&source_file, &python) {
        return fail(&format!("cannot write {}: {e}", source_file.display()));
    }
    let Some(interpreter) = crate::python_cmd() else {
        return fail("no Python interpreter found (set PYFUN_PYTHON)");
    };
    // Pyfun reuses a Python local across match arms, so a name can hold
    // differently narrowed types in different arms: mypy's newer redefinition
    // rule accepts that where the default first-assignment rule does not.
    let status = Command::new(&interpreter)
        .args([
            "-m",
            "mypyc",
            "--allow-redefinition-new",
            "--local-partial-types",
            &format!("{module}.py"),
        ])
        .current_dir(out)
        .status();
    match status {
        Ok(s) if s.success() => {}
        Ok(_) => {
            return fail(
                "mypyc could not build the program (see its output above); \
                 `pip install mypy` provides it, and it needs a C compiler",
            );
        }
        Err(e) => return fail(&format!("cannot run {interpreter}: {e}")),
    }
    // The extension now sits beside the source; remove the source so an import
    // finds the extension, and give the directory an entry point.
    let _ = std::fs::remove_file(&source_file);
    let _ = std::fs::remove_dir_all(out.join("build"));
    let main = format!(
        "# Runs the mypyc-compiled Pyfun program `{module}`.\nimport {module}  # noqa: F401\n"
    );
    if let Err(e) = std::fs::write(out.join("__main__.py"), main) {
        return fail(&format!("cannot write __main__.py: {e}"));
    }
    println!(
        "built {} natively: run it with `python {}`",
        path.display(),
        out.display()
    );
    ExitCode::SUCCESS
}

fn fail(msg: &str) -> ExitCode {
    eprintln!("error: {msg}");
    ExitCode::FAILURE
}
