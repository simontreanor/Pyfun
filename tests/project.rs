//! End-to-end tests for the multi-file driver's filesystem entry point
//! (`project::build_from_path`). The in-memory graph/cycle/topo logic is unit
//! tested in `src/project/mod.rs`; here we exercise the real
//! `<root>/<name>.pyfun` resolution against temp files on disk.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use pyfun::project::{self, ProjectError};

/// A unique scratch directory for one test, cleaned up on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("pyfun_project_{tag}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }

    fn write(&self, file: &str, source: &str) {
        fs::write(self.0.join(file), source).unwrap();
    }

    fn path(&self, file: &str) -> PathBuf {
        self.0.join(file)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn resolves_a_two_file_project_from_disk() {
    let dir = Scratch::new("two_file");
    dir.write("main.pyfun", "import Geometry\nlet a = Geometry.area 2 3");
    dir.write("geometry.pyfun", "let area w h = w * h");

    let project = project::build_from_path(&dir.path("main.pyfun")).unwrap();
    let names: Vec<&str> = project.modules.iter().map(|m| m.name.as_str()).collect();
    // Dependency first, entry last.
    assert_eq!(names, ["Geometry", "Main"]);
    assert_eq!(project.entry().name, "Main");
}

#[test]
fn a_missing_imported_file_is_reported() {
    let dir = Scratch::new("missing");
    dir.write("main.pyfun", "import Geometry\nlet x = 1");

    let err = project::build_from_path(&dir.path("main.pyfun")).unwrap_err();
    match err {
        ProjectError::Missing { name, importer } => {
            assert_eq!(name, "Geometry");
            assert_eq!(importer.as_deref(), Some("Main"));
        }
        other => panic!("expected a missing-module error, got {other:?}"),
    }
}

// ---------- slice 4: multi-file lowering & emit ----------

/// Resolve a project from in-memory `(name, source)` files via the graph driver.
fn build_mem(entry: &str, files: &[(&str, &str)]) -> project::Project {
    let owned: Vec<(String, String)> = files
        .iter()
        .map(|(n, s)| (n.to_string(), s.to_string()))
        .collect();
    project::build(entry, move |name| {
        owned
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, s)| s.clone())
    })
    .unwrap()
}

/// Build a project from in-memory files (entry first by convention), assert it
/// type-checks, and compile it to its emitted Python files.
fn compile(entry: &str, files: &[(&str, &str)]) -> Vec<(String, String)> {
    let project = build_mem(entry, files);
    assert!(
        project::check(&project).is_empty(),
        "project should type-check"
    );
    project::compile(&project).unwrap().files
}

/// The emitted source of a particular file, or panic.
fn file<'a>(files: &'a [(String, String)], name: &str) -> &'a str {
    files
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, s)| s.as_str())
        .unwrap_or_else(|| panic!("no emitted file `{name}` in {:?}", names(files)))
}

fn names(files: &[(String, String)]) -> Vec<&str> {
    files.iter().map(|(n, _)| n.as_str()).collect()
}

#[test]
fn each_module_emits_its_own_file_with_unmangled_names() {
    let files = compile(
        "Main",
        &[
            ("Main", "import Geometry\nlet floor = Geometry.area 4 5"),
            ("Geometry", "let area w h = w * h"),
        ],
    );
    assert_eq!(names(&files), ["geometry.py", "main.py"]);
    // The imported module's top-level names are un-mangled real Python defs.
    assert!(file(&files, "geometry.py").contains("def area(w, h):"));
}

#[test]
fn a_cross_module_call_imports_and_qualifies() {
    let files = compile(
        "Main",
        &[
            ("Main", "import Geometry\nlet floor = Geometry.area 4 5"),
            ("Geometry", "let area w h = w * h"),
        ],
    );
    let main = file(&files, "main.py");
    assert!(main.contains("import geometry"), "{main}");
    assert!(main.contains("floor = geometry.area(4, 5)"), "{main}");
}

#[test]
fn a_cross_module_partial_application_still_curries() {
    // Geometry.area's arity is threaded through, so a partial application lowers
    // to functools.partial rather than a wrong one-arg call.
    let files = compile(
        "Main",
        &[
            ("Main", "import Geometry\nlet scale = Geometry.area 2"),
            ("Geometry", "let area w h = w * h"),
        ],
    );
    let main = file(&files, "main.py");
    assert!(
        main.contains("scale = functools.partial(geometry.area, 2)"),
        "{main}"
    );
}

#[test]
fn option_classes_come_from_the_shared_runtime() {
    // Both the producer and the consumer import Some/None_ from _pyfun_rt, so an
    // Option value crossing the boundary is one type (isinstance-compatible).
    let files = compile(
        "Main",
        &[
            (
                "Main",
                "import Store\nlet found = Option.isSome (Store.lookup 1)",
            ),
            ("Store", "let lookup k = Some k"),
        ],
    );
    assert!(names(&files).contains(&"_pyfun_rt.py"));
    assert!(file(&files, "store.py").contains("from _pyfun_rt import Some, None_"));
    assert!(file(&files, "main.py").contains("from _pyfun_rt import Some, None_"));
    // The classes are defined exactly once, in the runtime.
    assert!(file(&files, "_pyfun_rt.py").contains("class Some"));
    assert!(!file(&files, "store.py").contains("class Some"));
}

#[test]
fn no_runtime_file_when_no_option_or_result_is_used() {
    let files = compile(
        "Main",
        &[
            ("Main", "import Geometry\nlet floor = Geometry.area 4 5"),
            ("Geometry", "let area w h = w * h"),
        ],
    );
    assert!(
        !names(&files).contains(&"_pyfun_rt.py"),
        "{:?}",
        names(&files)
    );
}

/// Materialize the compiled files into `dir` and run `python <entry>.py` from
/// there, returning stdout. Returns `None` if no interpreter is on PATH.
fn run_project(dir: &Scratch, files: &[(String, String)], entry_py: &str) -> Option<String> {
    let python = ["python", "python3"]
        .into_iter()
        .find(|c| Command::new(c).arg("--version").output().is_ok())?;
    for (name, source) in files {
        dir.write(name, source);
    }
    let output = Command::new(python)
        .arg(entry_py)
        .current_dir(&dir.0)
        .output()
        .expect("run python");
    assert!(
        output.status.success(),
        "python failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Some(String::from_utf8(output.stdout).expect("utf-8 stdout"))
}

#[test]
fn e2e_runs_a_cross_module_program() {
    let files = compile(
        "Main",
        &[
            (
                "Main",
                "import Geometry\nlet floor = Geometry.area 4 5\nprint floor",
            ),
            ("Geometry", "let area w h = w * h"),
        ],
    );
    let dir = Scratch::new("e2e_cross");
    if let Some(out) = run_project(&dir, &files, "main.py") {
        assert_eq!(out.trim(), "20");
    }
}

// ---------- cross-module sum types (construction + matching) ----------

#[test]
fn a_cross_module_sum_type_constructs_and_matches() {
    let files = compile(
        "Main",
        &[
            ("Shape", "type Shape = Circle float | Rect float float"),
            (
                "Main",
                "import Shape\n\
                 let c = Shape.Circle 2.0\n\
                 let area s =\n  match s:\n    case Shape.Circle r: r\n    case Shape.Rect w h: w * h\n\
                 let a = area c",
            ),
        ],
    );
    let main = file(&files, "main.py");
    // Construction routes to the imported module's class; the match uses a dotted
    // class pattern against that same class.
    assert!(main.contains("c = shape.Circle(2.0)"), "{main}");
    assert!(main.contains("case shape.Circle(r):"), "{main}");
    assert!(main.contains("import shape"), "{main}");
}

#[test]
fn a_cross_module_nullary_enum_constructs_as_a_call() {
    let files = compile(
        "Main",
        &[
            ("Color", "type Color = Red | Green | Blue"),
            (
                "Main",
                "import Color\n\
                 let favourite = Color.Green\n\
                 let name k =\n  match k:\n    case Color.Red: \"r\"\n    case Color.Green: \"g\"\n    case Color.Blue: \"b\"\n\
                 let n = name favourite",
            ),
        ],
    );
    let main = file(&files, "main.py");
    // A nullary constructor used as a value is an instance: `color.Green()`.
    assert!(main.contains("favourite = color.Green()"), "{main}");
    assert!(main.contains("case color.Green():"), "{main}");
}

#[test]
fn a_cross_module_non_exhaustive_match_is_caught() {
    let project = build_mem(
        "Main",
        &[
            ("Shape", "type Shape = Circle float | Rect float float"),
            (
                "Main",
                "import Shape\nlet f s =\n  match s:\n    case Shape.Circle r: r",
            ),
        ],
    );
    let errors = project::check(&project);
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].name, "Main");
    assert!(
        errors[0].errors[0].message.contains("Shape.Rect"),
        "witness should name the missing qualified ctor: {}",
        errors[0].errors[0].message
    );
}

#[test]
fn a_constructor_of_an_unimported_type_is_unavailable() {
    // Without `import Shape`, `Shape.Circle` is not in scope.
    let project = build_mem(
        "Main",
        &[
            ("Shape", "type Shape = Circle float | Rect float float"),
            ("Main", "let c = Shape.Circle 2.0"),
        ],
    );
    let errors = project::check(&project);
    assert_eq!(errors.len(), 1);
    assert!(
        errors[0].errors[0]
            .message
            .contains("not a member of `Shape`"),
        "got: {}",
        errors[0].errors[0].message
    );
}

#[test]
fn e2e_cross_module_adt_round_trips() {
    let files = compile(
        "Main",
        &[
            ("Shape", "type Shape = Circle int | Rect int int"),
            (
                "Main",
                "import Shape\n\
                 let area s =\n  match s:\n    case Shape.Circle r: r * r\n    case Shape.Rect w h: w * h\n\
                 print (area (Shape.Circle 3))\n\
                 print (area (Shape.Rect 4 5))",
            ),
        ],
    );
    let dir = Scratch::new("e2e_adt");
    if let Some(out) = run_project(&dir, &files, "main.py") {
        assert_eq!(out.replace("\r\n", "\n").trim(), "9\n20");
    }
}

// ---------- slice 6: import-aware editor analysis ----------

#[test]
fn editor_analysis_resolves_imports_from_disk() {
    // With the directory known, a `Geometry.area` reference checks cleanly.
    let dir = Scratch::new("analyze_ok");
    dir.write("geometry.pyfun", "let area w h = w * h");
    let main = "import Geometry\nlet floor = Geometry.area 4 5";

    let analysis = pyfun::analyze_in_dir(main, Some(dir.0.as_path()));
    assert!(
        analysis.diagnostics.is_empty(),
        "imported member should resolve: {:?}",
        analysis.diagnostics
    );
}

#[test]
fn editor_analysis_without_a_dir_cannot_resolve_imports() {
    // The dir-less analysis (no filesystem) can't see Geometry, so the qualified
    // reference is unresolved — the behavior before import-awareness.
    let main = "import Geometry\nlet floor = Geometry.area 4 5";
    let analysis = pyfun::analyze(main);
    assert!(
        analysis
            .diagnostics
            .iter()
            .any(|d| d.message.contains("not a member of `Geometry`")),
        "expected an unresolved-member diagnostic: {:?}",
        analysis.diagnostics
    );
}

#[test]
fn editor_analysis_still_flags_a_genuine_cross_module_error() {
    // Import resolution must not mask a real type error against the imported API.
    let dir = Scratch::new("analyze_err");
    dir.write("geometry.pyfun", "let area w h = w * h");
    let main = "import Geometry\nlet bad = Geometry.area 4 \"five\"";

    let analysis = pyfun::analyze_in_dir(main, Some(dir.0.as_path()));
    assert!(
        !analysis.diagnostics.is_empty(),
        "a cross-module type error should still be reported"
    );
}

#[test]
fn e2e_an_option_crosses_the_module_boundary() {
    // The decisive runtime test: an Option built in Store and inspected in Main
    // must match against the *same* Some class — only possible via the shared
    // runtime. A per-file class would make `Option.isSome` see a foreign type.
    let files = compile(
        "Main",
        &[
            (
                "Main",
                "import Store\nlet hit = Option.withDefault 0 (Store.lookup 7)\nprint hit",
            ),
            ("Store", "let lookup k = Some k"),
        ],
    );
    let dir = Scratch::new("e2e_option");
    if let Some(out) = run_project(&dir, &files, "main.py") {
        assert_eq!(out.trim(), "7");
    }
}
