//! `pyfun test` (`DESIGN.md` §9): run a project's tests with pytest.
//!
//! A test file is `test_*.pyfun`, and a test is a top-level function in it whose
//! name opens with `test_`: pytest's own two conventions. Each test file is
//! type-checked and compiled with the modules it imports, the tests are emitted
//! as the zero-argument functions pytest collects, and pytest is run over the
//! result. What a test expects is said with the `Test` module, which lowers to
//! `assert`, so a failure is reported by pytest in its own way.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use pyfun::project;
use pyfun::syntax::Item;

use crate::{check_project_ok, fail, python_cmd, report_notes, resolve_project};

/// Run `pyfun test [path...] [-- pytest args]`.
pub fn run(args: &[String]) -> ExitCode {
    let split = args.iter().position(|a| a == "--").unwrap_or(args.len());
    let (paths, pytest_args) = (&args[..split], &args[(split + 1).min(args.len())..]);
    let files = match test_files(paths) {
        Ok(files) => files,
        Err(message) => return fail(&message),
    };
    let Some(interp) = python_cmd() else {
        return fail("no Python interpreter found on PATH (tried `python`, `python3`)");
    };
    let has_pytest = Command::new(&interp)
        .args(["-m", "pytest", "--version"])
        .output()
        .is_ok_and(|out| out.status.success());
    if !has_pytest {
        return fail(&format!(
            "`pyfun test` runs the tests with pytest, which `{interp}` does not have: \
             install it with `{interp} -m pip install pytest`"
        ));
    }

    // Every test file's project, compiled into one directory: modules two
    // test files share come out the same for both.
    let mut staged: Vec<(String, String)> = Vec::new();
    for file in &files {
        let entry = file.to_string_lossy();
        let project = match resolve_project(&entry) {
            Ok(project) => project,
            Err(code) => return code,
        };
        if !check_project_ok(&project) {
            return ExitCode::FAILURE;
        }
        if let Err(message) = tests_take_one_parameter(&project) {
            return fail(&message);
        }
        let compiled = match project::compile_tests(&project) {
            Ok(compiled) => compiled,
            Err(e) => return fail(&format!("lowering failed: {}", e.message)),
        };
        report_notes(&compiled.notes);
        for (name, python) in compiled.files {
            match staged.iter().find(|(staged_name, _)| *staged_name == name) {
                None => staged.push((name, python)),
                Some((_, same)) if *same == python => {}
                Some(_) => {
                    return fail(&format!(
                        "two different modules compile to `{name}`: test files run together \
                         share one directory, so run these paths separately"
                    ));
                }
            }
        }
    }

    let dir = std::env::temp_dir().join(format!("pyfun_test_{}", std::process::id()));
    if let Err(e) = std::fs::create_dir_all(&dir) {
        return fail(&format!("cannot create temp dir: {e}"));
    }
    for (name, python) in &staged {
        if let Err(e) = project::write_file(&dir, name, python) {
            let _ = std::fs::remove_dir_all(&dir);
            return fail(&format!("cannot stage {name}: {e}"));
        }
    }
    // The working directory stays the user's, so a test's relative paths mean
    // what they mean on the command line; pytest is told where the tests are,
    // and to leave no cache behind in a directory that is about to go.
    let status = Command::new(&interp)
        .args(["-m", "pytest", "-p", "no:cacheprovider", "--rootdir"])
        .arg(&dir)
        .arg(&dir)
        .args(pytest_args)
        .status();
    let _ = std::fs::remove_dir_all(&dir);
    match status {
        Ok(s) if s.success() => ExitCode::SUCCESS,
        Ok(_) => ExitCode::FAILURE,
        Err(e) => fail(&format!("`{interp}` did not finish: {e}")),
    }
}

/// The test files `paths` name: a file is itself, a directory is the
/// `test_*.pyfun` files in it, and no path at all is the current directory.
fn test_files(paths: &[String]) -> Result<Vec<PathBuf>, String> {
    let here = [".".to_string()];
    let paths = if paths.is_empty() { &here[..] } else { paths };
    let mut files = Vec::new();
    for path in paths {
        let path = Path::new(path);
        if path.is_dir() {
            let mut found: Vec<PathBuf> = std::fs::read_dir(path)
                .map_err(|e| format!("cannot read {}: {e}", path.display()))?
                .flatten()
                .map(|entry| entry.path())
                .filter(|p| is_test_file(p))
                .collect();
            found.sort();
            if found.is_empty() {
                return Err(format!(
                    "no test files in {}: a test file is named `test_*.pyfun`",
                    path.display()
                ));
            }
            files.extend(found);
        } else if path.is_file() {
            if !is_test_file(path) {
                return Err(format!(
                    "{} is not a test file: pytest collects a file named `test_*`, so name it \
                     `test_{}`",
                    path.display(),
                    path.file_name().unwrap_or_default().to_string_lossy()
                ));
            }
            files.push(path.to_path_buf());
        } else {
            return Err(format!("cannot find {}", path.display()));
        }
    }
    Ok(files)
}

fn is_test_file(path: &Path) -> bool {
    path.extension().is_some_and(|x| x == "pyfun")
        && path
            .file_stem()
            .is_some_and(|stem| stem.to_string_lossy().starts_with("test_"))
}

/// A test is called with nothing, so it is the function of one ignored
/// parameter: `let test_name _ = …`.
fn tests_take_one_parameter(project: &project::Project) -> Result<(), String> {
    for module in &project.modules {
        if !project::is_test_module(&module.name) {
            continue;
        }
        for item in &module.ast.items {
            if let Item::Let(binding) = item
                && let Some(name) = binding.name()
                && pyfun::types::is_test_name(name)
                && binding.params.len() != 1
            {
                return Err(format!(
                    "in `{}`: `{name}` is named as a test, and a test takes one parameter that \
                     it ignores: write `let {name} _ = …`",
                    project::module_file_name(&module.name)
                ));
            }
        }
    }
    Ok(())
}
