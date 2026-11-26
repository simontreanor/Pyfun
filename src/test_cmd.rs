//! `pyfun test` (`DESIGN.md` §9): run a project's tests with pytest.
//!
//! A test file is `test_*.pyfun`, and a test is a top-level function in it whose
//! name opens with `test_`: pytest's own two conventions. Each test file is
//! type-checked and compiled with the modules it imports, the tests are emitted
//! as the zero-argument functions pytest collects, and pytest is run over the
//! test files themselves, each read as the Python it compiled to. What a test
//! expects is said with the `Test` module, which lowers to `assert`, so a
//! failure is explained by pytest in its own way, at the `.pyfun` line it came
//! from.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use pyfun::diagnostics::{self, Level};
use pyfun::lexer::Span;
use pyfun::project;
use pyfun::traceback::{self, Traced};

use crate::{
    check_project_ok, fail, python_cmd, report_notes, resolve_project, traced_files, traced_names,
};

/// Run `pyfun test [path...] [-- pytest args]`.
pub fn run(args: &[String]) -> ExitCode {
    let split = args.iter().position(|a| a == "--").unwrap_or(args.len());
    let (paths, pytest_args) = (&args[..split], &args[(split + 1).min(args.len())..]);
    let targets = match test_targets(paths) {
        Ok(targets) => targets,
        Err(message) => return fail(&message),
    };
    let Some(interp) = python_cmd() else {
        return fail("no Python interpreter found on PATH (tried `python`, `python3`)");
    };
    let pytest = Command::new(&interp)
        .args(["-m", "pytest", "--version"])
        .output()
        .ok()
        .filter(|out| out.status.success());
    let Some(pytest) = pytest else {
        return fail(&format!(
            "`pyfun test` runs the tests with pytest, which `{interp}` does not have: \
             install it with `{interp} -m pip install pytest`, or beside Pyfun with \
             `{interp} -m pip install \"pyfun-lang[test]\"`"
        ));
    };
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&pytest.stdout),
        String::from_utf8_lossy(&pytest.stderr)
    );
    if let Some(version) = pytest_version(&said)
        && version < OLDEST_PYTEST
    {
        return fail(&format!(
            "`pyfun test` needs pytest {}.{} or newer, and `{interp}` has {}.{}: upgrade it \
             with `{interp} -m pip install -U pytest`",
            OLDEST_PYTEST.0, OLDEST_PYTEST.1, version.0, version.1
        ));
    }

    // Every test file's project, compiled into one directory: modules two
    // test files share come out the same for both.
    let mut staged: Vec<(String, String)> = Vec::new();
    let mut built = Vec::new();
    // Each test file beside the staged module it compiled to.
    let mut tests: Vec<(String, String)> = Vec::new();
    let mut files: Vec<&str> = Vec::new();
    // Whether a test is a property, which Hypothesis runs.
    let mut properties = false;
    for target in &targets {
        if !files.contains(&target.file.as_str()) {
            files.push(&target.file);
        }
    }
    for file in files {
        let project = match resolve_project(file) {
            Ok(project) => project,
            Err(code) => return code,
        };
        if !check_project_ok(&project) {
            return ExitCode::FAILURE;
        }
        if !tests_have_a_shape(&project) {
            return ExitCode::FAILURE;
        }
        let compiled = match project::compile_tests(&project) {
            Ok(compiled) => compiled,
            Err(e) => match &e.at {
                Some((name, span)) => {
                    report_at(&project, name, &e.message, *span);
                    return ExitCode::FAILURE;
                }
                None => return fail(&format!("lowering failed: {}", e.message)),
            },
        };
        report_notes(&compiled.notes);
        properties |= compiled.has_properties;
        for (name, python) in &compiled.files {
            match staged.iter().find(|(staged_name, _)| staged_name == name) {
                None => staged.push((name.clone(), python.clone())),
                Some((_, same)) if same == python => {}
                Some(_) => {
                    return fail(&format!(
                        "two different modules compile to `{name}`: test files run together \
                         share one directory, so run these paths separately"
                    ));
                }
            }
        }
        let entry = &project.entry().file;
        let absolute = std::path::absolute(entry).unwrap_or_else(|_| PathBuf::from(entry));
        tests.push((
            absolute.display().to_string(),
            project::module_stem(&project.entry().name),
        ));
        let names = traced_names(&project, &compiled);
        built.push((project, compiled, names));
    }

    if properties && let Err(message) = hypothesis_is_there(&interp) {
        return fail(&message);
    }

    // Where each emitted line came from, for a failure in `.pyfun` lines.
    let mut traced: Vec<Traced> = Vec::new();
    for (project, compiled, names) in &built {
        for file in traced_files(project, compiled, names) {
            if !traced.iter().any(|known| known.py_name == file.py_name) {
                traced.push(file);
            }
        }
    }
    let modules: Vec<String> = staged
        .iter()
        .filter_map(|(name, _)| name.strip_suffix(".py"))
        .filter(|stem| !stem.ends_with("__init__") && !stem.starts_with("_pyfun_"))
        .map(|stem| stem.replace('/', "."))
        .collect();
    let support = [
        (traceback::PYTEST_FILE, traceback::PYTEST_SOURCE.to_string()),
        (traceback::HOOK_FILE, traceback::HOOK_SOURCE.to_string()),
        (traceback::TABLE_FILE, traceback::table(&traced)),
        (
            traceback::TESTS_FILE,
            traceback::test_manifest(&tests, &modules),
        ),
        (
            traceback::PYTEST_CONFIG_FILE,
            traceback::PYTEST_CONFIG.to_string(),
        ),
    ];

    let dir = std::env::temp_dir().join(format!("pyfun_test_{}", std::process::id()));
    if let Err(e) = std::fs::create_dir_all(&dir) {
        return fail(&format!("cannot create temp dir: {e}"));
    }
    let to_stage = staged
        .iter()
        .map(|(name, python)| (name.as_str(), python))
        .chain(support.iter().map(|(name, text)| (*name, text)));
    for (name, text) in to_stage {
        if let Err(e) = project::write_file(&dir, name, text) {
            let _ = std::fs::remove_dir_all(&dir);
            return fail(&format!("cannot stage {name}: {e}"));
        }
    }
    // The working directory stays the user's, so a test's relative paths mean
    // what they mean on the command line, and pytest is given the test files
    // by the paths they have there: the staged script tells it which staged
    // module each one is, and to leave nothing behind in the project.
    let mut ids: Vec<String> = Vec::new();
    for target in &targets {
        let id = target.id();
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    let status = Command::new(&interp)
        .arg(dir.join(traceback::PYTEST_FILE))
        .args(&ids)
        .args(pytest_args)
        .status();
    let _ = std::fs::remove_dir_all(&dir);
    match status {
        Ok(s) if s.success() => ExitCode::SUCCESS,
        Ok(_) => ExitCode::FAILURE,
        Err(e) => fail(&format!("`{interp}` did not finish: {e}")),
    }
}

/// The oldest pytest the staged plugin is run against: the floor of the
/// wheel's `test` extra (`pyproject.toml`).
const OLDEST_PYTEST: (u32, u32) = (8, 4);

/// The oldest Hypothesis a property test is run against: the floor of the
/// wheel's `test` extra (`pyproject.toml`).
const OLDEST_HYPOTHESIS: (u32, u32) = (6, 100);

/// A property test is run by Hypothesis, which `interp` has to have: said
/// before pytest is started, where a missing one would be an `ImportError` in
/// the middle of collecting the tests.
fn hypothesis_is_there(interp: &str) -> Result<(), String> {
    let found = Command::new(interp)
        .args(["-c", "import hypothesis; print(hypothesis.__version__)"])
        .output()
        .ok()
        .filter(|out| out.status.success());
    let Some(found) = found else {
        return Err(format!(
            "a property test (a `test_*` function that reads its parameters) is run by \
             Hypothesis, which `{interp}` does not have: install it with `{interp} -m pip \
             install hypothesis`, or beside Pyfun with `{interp} -m pip install \
             \"pyfun-lang[test]\"`"
        ));
    };
    let said = String::from_utf8_lossy(&found.stdout);
    if let Some(version) = dotted_version(said.trim())
        && version < OLDEST_HYPOTHESIS
    {
        return Err(format!(
            "`pyfun test` runs a property test with Hypothesis {}.{} or newer, and `{interp}` \
             has {}.{}: upgrade it with `{interp} -m pip install -U hypothesis`",
            OLDEST_HYPOTHESIS.0, OLDEST_HYPOTHESIS.1, version.0, version.1
        ));
    }
    Ok(())
}

/// The `(major, minor)` of a version (`6.170.17`).
fn dotted_version(version: &str) -> Option<(u32, u32)> {
    let mut parts = version.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    Some((major, minor))
}

/// The `(major, minor)` in what `pytest --version` printed (`pytest 9.2.0`).
fn pytest_version(said: &str) -> Option<(u32, u32)> {
    let version = said
        .split_whitespace()
        .skip_while(|word| *word != "pytest")
        .find(|word| word.starts_with(|c: char| c.is_ascii_digit()))?;
    dotted_version(version)
}

/// One thing to run: a test file, or one test of it.
struct Target {
    /// The file's path, as a failure shows it.
    file: String,
    /// The test's name, when the path was a test's id (`file.pyfun::test_name`).
    test: Option<String>,
}

impl Target {
    /// What pytest is asked for: the file, or the test's id.
    fn id(&self) -> String {
        match &self.test {
            Some(test) => format!("{}::{test}", self.file),
            None => self.file.clone(),
        }
    }
}

/// A path as it is shown: with no `./` ahead of it.
fn shown(path: &Path) -> String {
    let text = path.display().to_string();
    let mut rest = text.as_str();
    while let Some(inner) = rest.strip_prefix("./").or_else(|| rest.strip_prefix(".\\")) {
        rest = inner;
    }
    rest.to_string()
}

/// What `paths` name: a file is itself, a directory is the `test_*.pyfun`
/// files in it, a test's id (`test_shapes.pyfun::test_rect`, as a failure's
/// summary line prints it) is that one test, and no path at all is the
/// current directory.
fn test_targets(paths: &[String]) -> Result<Vec<Target>, String> {
    let here = [".".to_string()];
    let paths = if paths.is_empty() { &here[..] } else { paths };
    let mut targets = Vec::new();
    for given in paths {
        let (path, test) = match given.split_once("::") {
            Some((path, test)) => (Path::new(path), Some(test.to_string())),
            None => (Path::new(given), None),
        };
        if path.is_dir() && test.is_none() {
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
            targets.extend(found.iter().map(|file| Target {
                file: shown(file),
                test: None,
            }));
        } else if path.is_file() {
            if !is_test_file(path) {
                return Err(format!(
                    "{} is not a test file: pytest collects a file named `test_*`, so name it \
                     `test_{}`",
                    path.display(),
                    path.file_name().unwrap_or_default().to_string_lossy()
                ));
            }
            targets.push(Target {
                file: shown(path),
                test,
            });
        } else {
            return Err(format!("cannot find {}", path.display()));
        }
    }
    Ok(targets)
}

fn is_test_file(path: &Path) -> bool {
    path.extension().is_some_and(|x| x == "pyfun")
        && path
            .file_stem()
            .is_some_and(|stem| stem.to_string_lossy().starts_with("test_"))
}

/// Each test has a shape `pyfun test` can run: an example takes the one
/// parameter it ignores (`let test_name _ = …`), and a property reads its
/// parameters, which are generated (`DESIGN.md` §9). A test of neither shape
/// is reported at its name, and the result is whether there was none.
fn tests_have_a_shape(project: &project::Project) -> bool {
    let mut ok = true;
    for module in &project.modules {
        if !project::is_test_module(&module.name) {
            continue;
        }
        if let Err((span, message)) = pyfun::lowering::test_kinds(&module.ast) {
            report_at(project, &module.name, &message, span);
            ok = false;
        }
    }
    ok
}

/// An error at `span` of the module `name`, rendered against its source as
/// `check` renders one.
fn report_at(project: &project::Project, name: &str, message: &str, span: Span) {
    match project.modules.iter().find(|m| m.name == name) {
        Some(module) => eprintln!(
            "{}",
            diagnostics::render_in(&module.file, &module.source, Level::Error, message, span)
        ),
        None => eprintln!("error: {message}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_version_pytest_prints_is_read() {
        assert_eq!(pytest_version("pytest 9.2.0\n"), Some((9, 2)));
        assert_eq!(pytest_version("pytest 8.4.0.dev12\n"), Some((8, 4)));
        assert_eq!(
            pytest_version("This is pytest version 7.1.3, imported from x"),
            Some((7, 1))
        );
        assert_eq!(pytest_version("no such module"), None);
        assert!(pytest_version("pytest 8.3.5").unwrap() < OLDEST_PYTEST);
        assert!(pytest_version("pytest 10.0.1").unwrap() >= OLDEST_PYTEST);
    }

    #[test]
    fn the_version_hypothesis_says_is_read() {
        assert_eq!(dotted_version("6.170.17"), Some((6, 170)));
        assert!(dotted_version("6.99.0").unwrap() < OLDEST_HYPOTHESIS);
        assert!(dotted_version("7.0.0").unwrap() >= OLDEST_HYPOTHESIS);
        assert_eq!(dotted_version("unknown"), None);
    }
}
