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

// ---------- cross-module records (construct + access + pattern + update) ----------

#[test]
fn a_cross_module_record_constructs_and_routes_to_the_exporting_class() {
    let files = compile(
        "Main",
        &[
            ("Geometry", "type Point = { x: int, y: int }"),
            (
                "Main",
                "import Geometry\n\
                 let p = Geometry.Point { x = 1, y = 2 }\n\
                 let sx = p.x\n\
                 let shifted = { p with x = 10 }\n\
                 let describe q =\n  match q:\n    case Geometry.Point { x = 0, y }: y\n    case Geometry.Point { x, y }: x + y\n\
                 let d = describe p",
            ),
        ],
    );
    let main = file(&files, "main.py");
    // Construction, update, and the class pattern all reference the *imported*
    // class (`geometry.Point`) — the consumer never redefines it.
    assert!(main.contains("import geometry"), "{main}");
    assert!(main.contains("p = geometry.Point(1, 2)"), "{main}");
    assert!(main.contains("geometry.Point(10,"), "update: {main}");
    assert!(main.contains("case geometry.Point(x=0, y=y):"), "{main}");
    assert!(
        !main.contains("class Point"),
        "consumer must not redefine the record class: {main}"
    );
    assert!(
        file(&files, "geometry.py").contains("class Point"),
        "the class lives in its defining module"
    );
}

#[test]
fn e2e_cross_module_record_round_trips() {
    // Construct, bare-access, update, and pattern-match an imported record — and
    // pass a locally-constructed one back into an imported function that accesses
    // its fields (so both sides agree on the class identity).
    let files = compile(
        "Main",
        &[
            (
                "Geometry",
                "type Point = { x: int, y: int }\n\
                 let origin = Point { x = 0, y = 0 }\n\
                 let mag p = p.x + p.y",
            ),
            (
                "Main",
                "import Geometry\n\
                 let p = Geometry.Point { x = 3, y = 4 }\n\
                 let shifted = { p with x = 10 }\n\
                 let sum q =\n  match q:\n    case Geometry.Point { x, y }: x + y\n\
                 print (sum p)\n\
                 print shifted.x\n\
                 print (Geometry.mag p)\n\
                 print (Geometry.mag Geometry.origin)",
            ),
        ],
    );
    let dir = Scratch::new("e2e_record");
    if let Some(out) = run_project(&dir, &files, "main.py") {
        // sum p = 7 ; shifted.x = 10 ; mag p = 7 ; mag origin = 0
        assert_eq!(out.replace("\r\n", "\n").trim(), "7\n10\n7\n0");
    }
}

#[test]
fn e2e_a_parameterized_cross_module_record_round_trips() {
    // A polymorphic record `Box a` crosses the boundary: the closed-scheme transplant
    // instantiates its parameter afresh in the consumer.
    let files = compile(
        "Main",
        &[
            ("Store", "type Box a = { item: a, label: string }"),
            (
                "Main",
                "import Store\n\
                 let b = Store.Box { item = 42, label = \"answer\" }\n\
                 print b.item\n\
                 print b.label",
            ),
        ],
    );
    let dir = Scratch::new("e2e_box");
    if let Some(out) = run_project(&dir, &files, "main.py") {
        assert_eq!(out.replace("\r\n", "\n").trim(), "42\nanswer");
    }
}

#[test]
fn e2e_a_local_type_declaration_can_name_an_imported_type() {
    // A record field and an ADT variant typed with an imported record, one
    // qualified and one bare. Both spellings mean the same type, so the value
    // built by the exporter flows into the locally-declared shapes and back into
    // an imported function.
    let files = compile(
        "Main",
        &[
            (
                "Shapes",
                "type Placed = { row: int, col: int }\n\
                 let mk r = Placed { row = r, col = r }\n\
                 let rowOf p = p.row",
            ),
            (
                "Main",
                "import Shapes\n\
                 type Holder = { item: Shapes.Placed, tag: string }\n\
                 type Slot = Empty | Filled Placed\n\
                 let describe s =\n  match s:\n    case Empty: 0\n    case Filled p: Shapes.rowOf p\n\
                 let h = Holder { item = Shapes.mk 3, tag = \"a\" }\n\
                 print h.tag\n\
                 print (describe (Filled h.item))\n\
                 print (describe Empty)",
            ),
        ],
    );
    let dir = Scratch::new("e2e_imported_type_in_decl");
    if let Some(out) = run_project(&dir, &files, "main.py") {
        assert_eq!(out.replace("\r\n", "\n").trim(), "a\n3\n0");
    }
}

#[test]
fn an_imported_type_is_the_same_type_qualified_or_bare() {
    // The qualifier is a readability aid at the use site, not a distinct type, so
    // a value annotated one way unifies with the other. `extern` is the only place
    // that takes a written type, so it is where the two spellings meet.
    let project = build_mem(
        "Main",
        &[
            ("Shapes", "type Placed = { row: int, col: int }"),
            (
                "Main",
                "import Shapes\n\
                 extern pure widen : Shapes.Placed -> Placed = builtins.id\n\
                 let same p = widen p",
            ),
        ],
    );
    assert!(
        project::check(&project).is_empty(),
        "the qualified and bare spellings must unify"
    );
}

#[test]
fn an_unknown_qualified_type_is_reported() {
    // The qualifier is checked, not stripped and ignored: a module that exports no
    // such type is an unknown type, not a silent resolution to the bare name.
    let project = build_mem(
        "Main",
        &[
            ("Shapes", "type Placed = { row: int, col: int }"),
            (
                "Main",
                "import Shapes\ntype Holder = { item: Shapes.Widget }",
            ),
        ],
    );
    let errors = project::check(&project);
    assert_eq!(errors.len(), 1);
    assert!(
        errors[0].errors[0].message.contains("unknown type"),
        "got: {}",
        errors[0].errors[0].message
    );
}

#[test]
fn a_bare_tag_does_not_construct_an_imported_record() {
    // An imported record must be tagged qualified (like an imported sum ctor); a
    // bare `Point { … }` is rejected rather than miscompiling to a NameError.
    let project = build_mem(
        "Main",
        &[
            ("Geometry", "type Point = { x: int, y: int }"),
            ("Main", "import Geometry\nlet p = Point { x = 1, y = 2 }"),
        ],
    );
    let errors = project::check(&project);
    assert_eq!(errors.len(), 1);
    assert!(
        errors[0].errors[0].message.contains("not a record type"),
        "got: {}",
        errors[0].errors[0].message
    );
}

#[test]
fn a_cross_module_shared_field_is_ambiguous_at_the_access_site() {
    // A local record and an imported one both declare `x`; a bare `p.x` cannot
    // resolve by name — the ambiguity is reported at that access, not at import.
    let project = build_mem(
        "Main",
        &[
            ("Geometry", "type Point = { x: int, y: int }"),
            (
                "Main",
                "import Geometry\ntype Vec = { x: int, z: int }\nlet get p = p.x",
            ),
        ],
    );
    let errors = project::check(&project);
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].name, "Main");
    assert!(
        errors[0].errors[0].message.contains("ambiguous"),
        "got: {}",
        errors[0].errors[0].message
    );
}

#[test]
fn an_unexported_record_member_is_not_a_member() {
    // Geometry has no record `Widget`; the qualified tag reports the ordinary
    // not-a-member error.
    let project = build_mem(
        "Main",
        &[
            ("Geometry", "type Point = { x: int, y: int }"),
            ("Main", "import Geometry\nlet w = Geometry.Widget { x = 1 }"),
        ],
    );
    let errors = project::check(&project);
    assert_eq!(errors.len(), 1);
    assert!(
        errors[0].errors[0]
            .message
            .contains("not a member of `Geometry`"),
        "got: {}",
        errors[0].errors[0].message
    );
}

// ---------- cross-module externs ----------

#[test]
fn a_cross_module_extern_binds_in_its_module_and_routes_from_the_consumer() {
    let files = compile(
        "Main",
        &[
            ("Mathx", "extern fabs : float -> float = math.fabs"),
            ("Main", "import Mathx\nlet r = Mathx.fabs 8.0\nprint r"),
        ],
    );
    // The defining module binds the extern at top level (so it is referenceable as
    // an attribute), with the Python module imported.
    let mathx = file(&files, "mathx.py");
    assert!(mathx.contains("import math"), "{mathx}");
    assert!(mathx.contains("fabs = math.fabs"), "{mathx}");
    // The consumer routes the qualified reference to the module attribute.
    let main = file(&files, "main.py");
    assert!(main.contains("import mathx"), "{main}");
    assert!(main.contains("r = mathx.fabs(8.0)"), "{main}");
}

#[test]
fn e2e_a_cross_module_extern_runs() {
    let files = compile(
        "Main",
        &[
            ("Mathx", "extern fabs : float -> float = math.fabs"),
            ("Main", "import Mathx\nlet r = Mathx.fabs 8.0\nprint r"),
        ],
    );
    let dir = Scratch::new("e2e_extern");
    if let Some(out) = run_project(&dir, &files, "main.py") {
        assert_eq!(out.trim(), "8.0");
    }
}

#[test]
fn e2e_a_cross_module_extern_partial_application_curries() {
    // `List.map Mathx.fabs xs` partially applies the imported extern — its arity is
    // threaded so the map still curries and runs.
    let files = compile(
        "Main",
        &[
            ("Mathx", "extern fabs : float -> float = math.fabs"),
            (
                "Main",
                "import Mathx\nlet xs = List.map Mathx.fabs [8.0, 27.0, 64.0]\nprint xs",
            ),
        ],
    );
    let dir = Scratch::new("e2e_extern_partial");
    if let Some(out) = run_project(&dir, &files, "main.py") {
        assert_eq!(out.replace(' ', "").trim(), "[8.0,27.0,64.0]");
    }
}

#[test]
fn an_unexported_extern_is_not_a_member() {
    let project = build_mem(
        "Main",
        &[
            ("Mathx", "extern fabs : float -> float = math.fabs"),
            ("Main", "import Mathx\nlet r = Mathx.hypotenuse 8.0"),
        ],
    );
    let errors = project::check(&project);
    assert_eq!(errors.len(), 1);
    assert!(
        errors[0].errors[0]
            .message
            .contains("not a member of `Mathx`"),
        "got: {}",
        errors[0].errors[0].message
    );
}

#[test]
fn an_imported_impure_extern_keeps_its_effect_across_the_boundary() {
    // An `extern` is effectful-by-default; its scheme carries `io` on the innermost
    // arrow, which must survive the transplant — so calling it from a `let pure` in
    // the consumer is rejected. A `pure` extern is fine.
    let project = build_mem(
        "Main",
        &[
            (
                "Logx",
                "extern log : string -> unit = builtins.print\n\
                 extern pure ident : int -> int = builtins.abs",
            ),
            ("Main", "import Logx\nlet pure bad s = Logx.log s"),
        ],
    );
    let errors = project::check(&project);
    assert_eq!(errors.len(), 1);
    assert!(
        errors[0].errors[0].message.contains("pure") && errors[0].errors[0].message.contains("io"),
        "got: {}",
        errors[0].errors[0].message
    );

    // The `pure` extern imposes no effect, so it is callable from a `let pure`.
    let ok = build_mem(
        "Main",
        &[
            (
                "Logx",
                "extern log : string -> unit = builtins.print\n\
                 extern pure ident : int -> int = builtins.abs",
            ),
            ("Main", "import Logx\nlet pure ok n = Logx.ident n"),
        ],
    );
    assert!(project::check(&ok).is_empty());
}

// ---------- cross-module measures ----------

#[test]
fn a_shared_measure_module_is_used_across_files() {
    // `Units` defines base measures + a derived alias; two modules import it and use
    // `<m>` / `<N>`. The shared re-import of the same base measures must not conflict.
    let project = build_mem(
        "Main",
        &[
            ("Units", "measure m\nmeasure s\nmeasure N = m / s"),
            ("Dist", "import Units\nlet far d = d + 1<m>"),
            (
                "Main",
                "import Units\nimport Dist\nlet here = 100<m>\nlet f = 10<N>\nlet g = Dist.far 5<m>",
            ),
        ],
    );
    let errors = project::check(&project);
    assert!(errors.is_empty(), "unexpected errors: {errors:?}");
}

#[test]
fn e2e_a_cross_module_measure_annotated_value_runs() {
    // Measures erase at lowering, so a `<m>`-annotated value round-trips to plain
    // Python numerics.
    let files = compile(
        "Main",
        &[
            ("Units", "measure m"),
            (
                "Main",
                "import Units\nlet d = 100<m>\nlet e = d + 50<m>\nprint e",
            ),
        ],
    );
    let dir = Scratch::new("e2e_measure");
    if let Some(out) = run_project(&dir, &files, "main.py") {
        assert_eq!(out.trim(), "150");
    }
}

#[test]
fn using_a_measure_without_importing_its_module_is_an_error() {
    let project = build_mem(
        "Main",
        &[("Units", "measure m"), ("Main", "let d = 100<m>")],
    );
    let errors = project::check(&project);
    assert_eq!(errors.len(), 1);
    assert!(
        errors[0].errors[0].message.contains("unknown measure `m`"),
        "got: {}",
        errors[0].errors[0].message
    );
}

#[test]
fn a_cross_module_measure_alias_conflict_is_reported() {
    // Two modules define a *different* expansion for the same alias name `N`; a
    // consumer importing both hits a genuine conflict (a shared base measure would
    // NOT — that is the common case and is idempotent).
    let project = build_mem(
        "Main",
        &[
            ("Ua", "measure m\nmeasure s\nmeasure N = m / s"),
            ("Ub", "measure m\nmeasure s\nmeasure N = m s"),
            ("Main", "import Ua\nimport Ub\nlet x = 1"),
        ],
    );
    let errors = project::check(&project);
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].name, "Main");
    assert!(
        errors[0].errors[0].message.contains("alias `N`")
            && errors[0].errors[0].message.contains("conflicts"),
        "got: {}",
        errors[0].errors[0].message
    );
}

#[test]
fn a_shared_base_measure_across_two_imports_does_not_conflict() {
    // Two imported modules both declare `measure m`; importing both is fine (base
    // measures are nominal-by-name and erase — the shared-`Units` pattern).
    let project = build_mem(
        "Main",
        &[
            ("Ua", "measure m\nlet a = 1<m>"),
            ("Ub", "measure m\nlet b = 2<m>"),
            ("Main", "import Ua\nimport Ub\nlet x = 1"),
        ],
    );
    let errors = project::check(&project);
    assert!(errors.is_empty(), "unexpected errors: {errors:?}");
}

// ---------- cross-module opaque types (newtypes) ----------

#[test]
fn a_cross_module_newtype_erases_in_the_consumer_too() {
    // `Ids.UserId` constructs, matches, and references first-class from Main —
    // all erased: no class anywhere, wraps are bare values, a first-class
    // reference is the `_pf_id` identity helper.
    let files = compile(
        "Main",
        &[
            (
                "Ids",
                "opaque type UserId = string\n\
                 let greet u =\n  match u:\n    case UserId s: String.concat \"hi \" s",
            ),
            (
                "Main",
                "import Ids\n\
                 let uid = Ids.UserId \"ana\"\n\
                 let g = Ids.greet uid\n\
                 let unwrapped =\n  match uid:\n    case Ids.UserId s: s\n\
                 let mapped = List.map Ids.UserId [\"a\", \"b\"]",
            ),
        ],
    );
    let main = file(&files, "main.py");
    assert!(main.contains("uid = \"ana\""), "{main}");
    assert!(main.contains("_pf_map(_pf_id,"), "{main}");
    assert!(!main.contains("class UserId"), "{main}");
    assert!(!main.contains("UserId("), "{main}");
    assert!(
        !file(&files, "ids.py").contains("class UserId"),
        "the exporting module erases the newtype as well"
    );
}

#[test]
fn e2e_a_cross_module_newtype_round_trips() {
    let files = compile(
        "Main",
        &[
            (
                "Ids",
                "opaque type UserId = string\n\
                 let greet u =\n  match u:\n    case UserId s: String.concat \"hi \" s",
            ),
            (
                "Main",
                "import Ids\n\
                 let uid = Ids.UserId \"ana\"\n\
                 let unwrapped =\n  match uid:\n    case Ids.UserId s: s\n\
                 let mapped = List.map Ids.UserId [\"a\", \"b\"]\n\
                 print (Ids.greet uid)\n\
                 print unwrapped\n\
                 print (List.len mapped)",
            ),
        ],
    );
    let dir = Scratch::new("e2e_newtype");
    if let Some(out) = run_project(&dir, &files, "main.py") {
        assert_eq!(out.replace("\r\n", "\n").trim(), "hi ana\nana\n2");
    }
}

#[test]
fn a_cross_module_newtype_stays_distinct_from_its_underlying() {
    // The nominal wall crosses the boundary: a raw string is not an `Ids.UserId`.
    let project = build_mem(
        "Main",
        &[
            (
                "Ids",
                "opaque type UserId = string\n\
                 let greet u =\n  match u:\n    case UserId s: s",
            ),
            ("Main", "import Ids\nlet bad = Ids.greet \"raw\""),
        ],
    );
    let errors = project::check(&project);
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].name, "Main");
    assert!(
        errors[0].errors[0]
            .message
            .contains("expected UserId, found string"),
        "got: {}",
        errors[0].errors[0].message
    );
}

// ---------- transitive interface closure (issues #72 / #75) ----------

/// The three-module shape of issue #75: `Inner` declares a record, `Middle`
/// wraps it, and the consumer imports only `Middle`.
const INNER: &str = "type Config = { cSize: int }\nlet make n = Config { cSize = n }";
const MIDDLE: &str = "import Inner\n\
     type Wrap = { wConf: Inner.Config }\n\
     let mk n = Wrap { wConf = Inner.make n }\n\
     let size w = w.wConf.cSize";

#[test]
fn e2e_an_extern_type_can_be_named_across_the_module_boundary() {
    // issue #72: the *name* of an `extern type` crosses with its module's
    // interface, so a consumer can type a record field `Gen.Rng` (values of the
    // type always crossed through the imported schemes).
    let files = compile(
        "Main",
        &[
            (
                "Gen",
                "extern type Rng\n\
                 extern newRng : int -> Rng = random.Random\n\
                 let make seed = newRng seed",
            ),
            (
                "Main",
                "import Gen\n\
                 type Holder = { h: Gen.Rng }\n\
                 let x = Holder { h = Gen.make 1 }\n\
                 print \"ok\"",
            ),
        ],
    );
    let dir = Scratch::new("e2e_extern_type_name");
    if let Some(out) = run_project(&dir, &files, "main.py") {
        assert_eq!(out.trim(), "ok");
    }
}

#[test]
fn an_unknown_type_qualified_to_an_opaque_exporting_module_reports_once() {
    // A genuinely unknown qualified type still reports cleanly — one error, no
    // cascade — now that the module also exports opaque type names.
    let project = build_mem(
        "Main",
        &[
            (
                "Gen",
                "extern type Rng\nextern newRng : int -> Rng = random.Random",
            ),
            ("Main", "import Gen\ntype Holder = { h: Gen.Nope }"),
        ],
    );
    let errors = project::check(&project);
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].errors.len(), 1, "got: {:?}", errors[0].errors);
    assert!(
        errors[0].errors[0]
            .message
            .contains("unknown type `Gen.Nope`"),
        "got: {}",
        errors[0].errors[0].message
    );
}

#[test]
fn an_opaque_type_carried_transitively_can_be_named_bare() {
    // `Middle.fresh : int -> Rng` mentions Gen's handle type, so `Rng` crosses
    // with Middle's interface and the consumer can name it (bare — constructing
    // or qualifying still asks for the declaring module).
    let project = build_mem(
        "Main",
        &[
            (
                "Gen",
                "extern type Rng\n\
                 extern newRng : int -> Rng = random.Random\n\
                 let make seed = newRng seed",
            ),
            ("Middle", "import Gen\nlet fresh n = Gen.make n"),
            (
                "Main",
                "import Middle\n\
                 type H = { h: Rng }\n\
                 let x = H { h = Middle.fresh 1 }",
            ),
        ],
    );
    let errors = project::check(&project);
    assert!(errors.is_empty(), "unexpected errors: {errors:?}");
}

#[test]
fn a_local_type_clashing_with_an_imported_opaque_type_is_reported() {
    // A handle type now registers like any imported type, so a same-named local
    // declaration is the ordinary import clash (it would otherwise silently
    // conflate two distinct nominal types).
    let project = build_mem(
        "Main",
        &[
            (
                "Gen",
                "extern type Rng\nextern newRng : int -> Rng = random.Random",
            ),
            ("Main", "import Gen\ntype Rng = A | B\nlet x = A"),
        ],
    );
    let errors = project::check(&project);
    assert_eq!(errors.len(), 1);
    assert!(
        errors[0].errors[0]
            .message
            .contains("imported type `Rng` (from `Gen`) clashes"),
        "got: {}",
        errors[0].errors[0].message
    );
}

#[test]
fn e2e_a_transitive_record_field_is_accessible() {
    // issue #75: `Wrap`'s field is typed `Inner.Config`, so `Config` crosses
    // with Middle's interface and the consumer can read `x.wConf.cSize` without
    // importing `Inner` directly.
    let files = compile(
        "Main",
        &[
            ("Inner", INNER),
            ("Middle", MIDDLE),
            (
                "Main",
                "import Middle\nlet x = Middle.mk 5\nprint x.wConf.cSize",
            ),
        ],
    );
    let dir = Scratch::new("e2e_transitive_field");
    if let Some(out) = run_project(&dir, &files, "main.py") {
        assert_eq!(out.trim(), "5");
    }
}

#[test]
fn e2e_a_record_update_through_a_transitive_boundary_routes_to_the_declaring_class() {
    // An update reconstructs the record, so the consumer's emitted Python must
    // reference the *declaring* module's class (`inner.Config`), with its import
    // hoisted, even though the consumer never writes `import Inner`.
    let files = compile(
        "Main",
        &[
            ("Inner", INNER),
            ("Middle", MIDDLE),
            (
                "Main",
                "import Middle\n\
                 let x = Middle.mk 5\n\
                 let y = { x with wConf.cSize = 9 }\n\
                 let z = { x.wConf with cSize = 7 }\n\
                 print y.wConf.cSize\n\
                 print z.cSize",
            ),
        ],
    );
    let main = file(&files, "main.py");
    assert!(main.contains("import inner"), "{main}");
    assert!(main.contains("inner.Config("), "{main}");
    let dir = Scratch::new("e2e_transitive_update");
    if let Some(out) = run_project(&dir, &files, "main.py") {
        assert_eq!(out.replace("\r\n", "\n").trim(), "9\n7");
    }
}

#[test]
fn e2e_a_transitive_record_crosses_any_depth() {
    // The record is declared three imports away; each hop's interface carries it
    // onward, so the chain works at any depth.
    let files = compile(
        "Main",
        &[
            (
                "Deep",
                "type Core = { cv: int }\nlet mkCore n = Core { cv = n }",
            ),
            (
                "Mid",
                "import Deep\n\
                 type Two = { w1: Deep.Core }\n\
                 let mk2 n = Two { w1 = Deep.mkCore n }",
            ),
            (
                "Top",
                "import Mid\n\
                 type Three = { w2: Mid.Two }\n\
                 let mk3 n = Three { w2 = Mid.mk2 n }",
            ),
            ("Main", "import Top\nlet x = Top.mk3 7\nprint x.w2.w1.cv"),
        ],
    );
    let dir = Scratch::new("e2e_transitive_depth");
    if let Some(out) = run_project(&dir, &files, "main.py") {
        assert_eq!(out.trim(), "7");
    }
}

#[test]
fn mutually_referencing_records_cross_transitively() {
    // Two records that reference each other (via Option) are legal; the export
    // closure must follow the cycle and terminate rather than recurse forever.
    let project = build_mem(
        "Main",
        &[
            (
                "Inner",
                "type A = { av: int, ab: Option B }\n\
                 type B = { bv: int, ba: Option A }\n\
                 let mkA n = A { av = n, ab = None }\n\
                 let mkB n = B { bv = n, ba = Some (mkA n) }",
            ),
            ("Middle", "import Inner\nlet make n = Inner.mkB n"),
            (
                "Main",
                "import Middle\n\
                 let b = Middle.make 3\n\
                 let v = b.bv\n\
                 let w = Option.map (fun a -> a.av) b.ba",
            ),
        ],
    );
    let errors = project::check(&project);
    assert!(errors.is_empty(), "unexpected errors: {errors:?}");
}

#[test]
fn a_transitive_record_participates_in_field_ambiguity() {
    // A local record and a transitively carried one both declare `cSize`; a bare
    // access on an untyped base is ambiguous at the access site, exactly as with
    // a directly imported record.
    let project = build_mem(
        "Main",
        &[
            ("Inner", INNER),
            ("Middle", MIDDLE),
            (
                "Main",
                "import Middle\n\
                 type Local = { cSize: int, other: int }\n\
                 let get w = w.cSize",
            ),
        ],
    );
    let errors = project::check(&project);
    assert_eq!(errors.len(), 1);
    assert!(
        errors[0].errors[0].message.contains("ambiguous"),
        "got: {}",
        errors[0].errors[0].message
    );
}

#[test]
fn a_transitive_record_shadowed_by_a_local_name_hints_at_its_home() {
    // A local type already holds the name `Config`, so the carried record stays
    // out of scope (local and direct declarations win the bare name, and the
    // consumer never asked for the carried one — no clash error). Its fields
    // still feed the diagnostics: both the known-base and the untyped-base
    // failure name the record and the module that declares it.
    let with_known_base = build_mem(
        "Main",
        &[
            ("Inner", INNER),
            ("Middle", MIDDLE),
            (
                "Main",
                "import Middle\n\
                 type Config = { other: int }\n\
                 let x = Middle.mk 5\n\
                 let bad = x.wConf.cSize",
            ),
        ],
    );
    let errors = project::check(&with_known_base);
    assert_eq!(errors.len(), 1);
    let message = &errors[0].errors[0].message;
    assert!(
        message.contains("has no field `cSize`")
            && message.contains("module `Inner`")
            && message.contains("hidden"),
        "got: {message}"
    );

    let with_untyped_base = build_mem(
        "Main",
        &[
            ("Inner", INNER),
            ("Middle", MIDDLE),
            (
                "Main",
                "import Middle\n\
                 type Config = { other: int }\n\
                 let get w = w.cSize",
            ),
        ],
    );
    let errors = project::check(&with_untyped_base);
    assert_eq!(errors.len(), 1);
    let message = &errors[0].errors[0].message;
    assert!(
        message.contains("unknown record field `cSize`") && message.contains("module `Inner`"),
        "got: {message}"
    );
}

#[test]
fn a_diamond_of_carried_records_does_not_clash() {
    // Two imports both carry `Inner.Config` transitively; the consumer must see
    // one record, not a spurious clash for a name it never asked for.
    let project = build_mem(
        "Main",
        &[
            ("Inner", INNER),
            (
                "Left",
                "import Inner\n\
                 type LWrap = { lc: Inner.Config }\n\
                 let mkL n = LWrap { lc = Inner.make n }",
            ),
            (
                "Right",
                "import Inner\n\
                 type RWrap = { rc: Inner.Config }\n\
                 let mkR n = RWrap { rc = Inner.make n }",
            ),
            (
                "Main",
                "import Left\n\
                 import Right\n\
                 let a = (Left.mkL 1).lc.cSize\n\
                 let b = (Right.mkR 2).rc.cSize",
            ),
        ],
    );
    let errors = project::check(&project);
    assert!(errors.is_empty(), "unexpected errors: {errors:?}");
}

#[test]
fn importing_the_declaring_module_alongside_the_carrier_is_fine() {
    // Direct import and transitive carry of the same record coexist: one record,
    // full access, and construction through the direct import.
    let project = build_mem(
        "Main",
        &[
            ("Inner", INNER),
            ("Middle", MIDDLE),
            (
                "Main",
                "import Inner\n\
                 import Middle\n\
                 let x = Middle.mk 5\n\
                 let a = x.wConf.cSize\n\
                 let mine = Inner.Config { cSize = 1 }",
            ),
        ],
    );
    let errors = project::check(&project);
    assert!(errors.is_empty(), "unexpected errors: {errors:?}");
}

#[test]
fn a_carried_record_cannot_be_constructed_without_a_direct_import() {
    // The carried record crosses as an identity (naming, unification, field
    // access), not as a member: constructing it still asks for `import Inner`.
    let project = build_mem(
        "Main",
        &[
            ("Inner", INNER),
            ("Middle", MIDDLE),
            (
                "Main",
                "import Middle\nlet bad = Inner.Config { cSize = 1 }",
            ),
        ],
    );
    let errors = project::check(&project);
    assert_eq!(errors.len(), 1);
    assert!(
        errors[0].errors[0]
            .message
            .contains("not a member of `Inner`"),
        "got: {}",
        errors[0].errors[0].message
    );
}

#[test]
fn a_binding_colliding_with_a_module_alias_gets_a_mangled_import() {
    // `import Ids` + `let ids = …` used to emit `ids = …` after `import ids`,
    // clobbering the module object. The collided import is now aliased and every
    // qualified reference routes through the alias.
    let files = compile(
        "Main",
        &[
            (
                "Ids",
                "let greet s = String.concat \"hi \" s\nlet tag = \"x\"",
            ),
            (
                "Main",
                "import Ids\n\
                 let ids = [1, 2, 3]\n\
                 let g = Ids.greet \"ana\"\n\
                 print g\n\
                 print (List.len ids)",
            ),
        ],
    );
    let main = file(&files, "main.py");
    assert!(main.contains("import ids as _pf_ids"), "{main}");
    assert!(main.contains("_pf_ids.greet"), "{main}");
    let dir = Scratch::new("alias_collision");
    if let Some(out) = run_project(&dir, &files, "main.py") {
        assert_eq!(out.replace("\r\n", "\n").trim(), "hi ana\n3");
    }
}

#[test]
fn local_binders_colliding_with_a_module_alias_also_get_the_mangled_import() {
    // Python hoists locals, so ANY binder in a function scope named like the
    // import alias shadows it for the whole function: parameters, block `let`s
    // (even after the qualified call), lambda parameters, match-pattern
    // captures, top-level pattern captures, and CE binders (which become
    // captures inside the generated def). Each shape used to break at runtime.
    let files = compile(
        "Main",
        &[
            ("Ids", "let greet s = String.concat \"hi \" s"),
            (
                "Main",
                "import Ids\n\
                 let viaParam ids = List.map (fun n -> Ids.greet n) ids\n\
                 let hoisted u =\n    let g = Ids.greet u\n    let ids = [1, 2]\n    List.len ids + String.len g\n\
                 let viaLambda xs = List.map (fun ids -> Ids.greet ids) xs\n\
                 let fromMatch xs =\n  match xs:\n    case [ids]: Ids.greet ids\n    case _: \"none\"\n\
                 let fromCe =\n  result {\n    let! ids = Ok [1]\n    return Ids.greet \"g\"\n  }\n\
                 let topLevel =\n  match [\"f\"]:\n    case [ids]: Ids.greet ids\n    case _: \"none\"\n\
                 print (List.len (viaParam [\"a\", \"b\"]))\n\
                 print (hoisted \"c\")\n\
                 print (List.len (viaLambda [\"d\"]))\n\
                 print (fromMatch [\"e\"])\n\
                 print (Result.withDefault \"err\" fromCe)\n\
                 print topLevel",
            ),
        ],
    );
    let main = file(&files, "main.py");
    assert!(main.contains("import ids as _pf_ids"), "{main}");
    assert!(!main.contains("return ids.greet"), "{main}");
    let dir = Scratch::new("alias_local_collision");
    if let Some(out) = run_project(&dir, &files, "main.py") {
        assert_eq!(
            out.replace("\r\n", "\n").trim(),
            "2\n6\n1\nhi e\nhi g\nhi f"
        );
    }
}

#[test]
fn an_uncollided_module_import_stays_unmangled() {
    // No collision → the readable plain `import ids` is preserved.
    let files = compile(
        "Main",
        &[
            ("Ids", "let greet s = String.concat \"hi \" s"),
            ("Main", "import Ids\nprint (Ids.greet \"ana\")"),
        ],
    );
    let main = file(&files, "main.py");
    assert!(main.contains("import ids\n"), "{main}");
    assert!(!main.contains("_pf_ids"), "{main}");
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

#[test]
fn an_importing_module_keeps_its_own_functions_polymorphic() {
    // An imported scheme arrives holding the *exporting* module's variable ids,
    // which this module allocates too. Before those ids were refreshed at the
    // boundary, one collision let `env_free_vars` rewrite the imported scheme and
    // leak local variables into the environment's free set — blocking
    // generalization of Main's own bindings, so `f1` went monomorphic and its
    // second use was rejected. Main does not even use the import; the exact shapes
    // matter only in that they make the two modules' id ranges overlap.
    let project = build_mem(
        "Main",
        &[
            (
                "Main",
                "import Helper\nlet f0 a b = (a, b)\nlet f1 a b = (a, b)\nprint (f1 1 2)\nprint (f1 \"x\" \"y\")",
            ),
            ("Helper", "let id x = x\nlet id2 y = y"),
        ],
    );
    let errors = project::check(&project);
    assert!(
        errors.is_empty(),
        "an importing module's own function must stay polymorphic: {errors:?}"
    );
}

#[test]
fn an_under_generalized_export_does_not_cascade_to_a_consumer() {
    // The contagious half, one module further along: `Middle` imports `Helper`,
    // and the blocked generalization above made `Middle.f1` *export* with nothing
    // quantified — so `Main` received one shared type variable for every use site
    // and its second use was rejected, naming a function two modules away.
    let project = build_mem(
        "Main",
        &[
            (
                "Main",
                "import Middle\nprint (Middle.f1 1 2)\nprint (Middle.f1 \"x\" \"y\")",
            ),
            (
                "Middle",
                "import Helper\nlet f0 a b = (a, b)\nlet f1 a b = (a, b)",
            ),
            ("Helper", "let id x = x\nlet id2 y = y"),
        ],
    );
    let errors = project::check(&project);
    assert!(
        errors.is_empty(),
        "an exported polymorphic function must instantiate freshly per use: {errors:?}"
    );
}

#[test]
fn a_module_that_only_matches_an_imported_option_imports_it() {
    // The producer is in the other module, so nothing in Main constructs an
    // Option — but Main's emitted patterns still name Some/None_, which must come
    // from the shared runtime or the module raises NameError at import time.
    let files = compile(
        "Main",
        &[
            (
                "Main",
                "import Helper
let probe xs =
  match Helper.firstOf xs:
    case None: \"none\"
    case Some k: k
print (probe [\"A\", \"B\"])",
            ),
            ("Helper", "let firstOf xs = List.get 0 xs"),
        ],
    );
    assert!(
        file(&files, "main.py").contains("from _pyfun_rt import Some, None_"),
        "{}",
        file(&files, "main.py")
    );
    let dir = Scratch::new("e2e_option_match_only");
    if let Some(out) = run_project(&dir, &files, "main.py") {
        assert_eq!(out.trim(), "A");
    }
}

#[test]
fn a_value_named_after_a_python_builtin_crosses_a_module_boundary() {
    // The mangling is a pure function of the name, so the defining module and
    // every reference to it agree without coordination.
    let files = compile(
        "Main",
        &[
            (
                "Main",
                "import Helper\nprint (Helper.set + 1)\nprint (Helper.len [\"a\", \"b\"])",
            ),
            ("Helper", "let set = 41\nlet len xs = List.len xs"),
        ],
    );
    assert!(
        file(&files, "helper.py").contains("set_ = 41"),
        "{}",
        file(&files, "helper.py")
    );
    assert!(
        file(&files, "main.py").contains("helper.set_"),
        "{}",
        file(&files, "main.py")
    );
    let dir = Scratch::new("e2e_builtin_name_cross_module");
    if let Some(out) = run_project(&dir, &files, "main.py") {
        assert_eq!(out.replace("\r\n", "\n").trim(), "42\n2");
    }
}
