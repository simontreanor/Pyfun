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
        return build_project(path, out);
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
    finish(path, out, &module, std::slice::from_ref(&module))
}

/// A project: every module compiled natively into `out`, and every one but the
/// shared `_pyfun_rt.py` built by mypyc (the runtime stays Python, which the
/// compiled modules import like any other module).
fn build_project(path: &Path, out: &Path) -> ExitCode {
    let entry = path.to_string_lossy().to_string();
    let project = match crate::resolve_project(&entry) {
        Ok(p) => p,
        Err(code) => return code,
    };
    if !crate::check_project_ok(&project) {
        return ExitCode::FAILURE;
    }
    let files = match crate::lower_project(&project, PyTarget::default(), true) {
        Ok(f) => f,
        Err(code) => return code,
    };
    if let Err(e) = std::fs::create_dir_all(out) {
        return fail(&format!("cannot create {}: {e}", out.display()));
    }
    let mut modules = Vec::new();
    for (name, source) in &files {
        if let Err(e) = std::fs::write(out.join(name), source) {
            return fail(&format!("cannot write {name}: {e}"));
        }
        if let Some(stem) = name.strip_suffix(".py")
            && stem != "_pyfun_rt"
        {
            modules.push(stem.to_string());
        }
    }
    let Some(entry_module) = pyfun::project::module_name_from_path(path).map(|m| m.to_lowercase())
    else {
        return fail("the file needs a name");
    };
    finish(path, out, &entry_module, &modules)
}

/// Run mypyc over `modules` in `out`, drop their sources so imports find the
/// extensions, and write the `__main__.py` that imports `entry`.
fn finish(path: &Path, out: &Path, entry: &str, modules: &[String]) -> ExitCode {
    let Some(interpreter) = crate::python_cmd() else {
        return fail("no Python interpreter found (set PYFUN_PYTHON)");
    };
    // Pyfun reuses a Python local across match arms, so a name can hold
    // differently narrowed types in different arms: mypy's newer redefinition
    // rule accepts that where the default first-assignment rule does not.
    let mut args = vec![
        "-m".to_string(),
        "mypyc".to_string(),
        "--allow-redefinition-new".to_string(),
        "--local-partial-types".to_string(),
        // Pyfun binds a unit call's result (`_ = f(x)`), which mypy reports
        // when `f` is annotated `-> None`; the binding is harmless.
        "--disable-error-code".to_string(),
        "func-returns-value".to_string(),
        // At the Python boundary the extern declarations are the contract, and
        // Pyfun's checker has already held the program to them; mypy's view of
        // an unannotated module-level value or a stub's union adds nothing.
        "--disable-error-code".to_string(),
        "var-annotated".to_string(),
        "--disable-error-code".to_string(),
        "union-attr".to_string(),
    ];
    args.extend(modules.iter().map(|m| format!("{m}.py")));
    let status = Command::new(&interpreter)
        .args(&args)
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
    for m in modules {
        let _ = std::fs::remove_file(out.join(format!("{m}.py")));
    }
    let _ = std::fs::remove_dir_all(out.join("build"));
    let main = format!(
        "# Runs the mypyc-compiled Pyfun program `{entry}`.\nimport {entry}  # noqa: F401\n"
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
