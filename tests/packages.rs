//! Nested packages (`DESIGN.md` §6.1): `import Shapes.Geometry` names the module
//! in `shapes/geometry.pyfun`, and the code then says `Geometry`.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use pyfun::project;

const GEOMETRY: &str = "import Shapes.Util\n\
     type Shape = Circle(float) | Rect { width: float, height: float }\n\
     type Pt = { x: int, y: int }\n\
     let pi = 3.0\n\
     let area shape =\n    match shape:\n        case Circle(r): pi * r * r\n        case Rect { width, height }: Util.double (width * height) / 2.0\n";

/// A project in memory: `Main`, and three modules in two packages.
fn build(main: &str) -> Result<project::Project, project::ProjectError> {
    let files = [
        ("Main".to_string(), main.to_string()),
        ("Shapes.Geometry".to_string(), GEOMETRY.to_string()),
        (
            "Shapes.Util".to_string(),
            "let double x = x * 2.0\n".to_string(),
        ),
        (
            "Solids.Geometry".to_string(),
            "from Shapes.Util import double\nlet volume side = double (side * side * side)\n"
                .to_string(),
        ),
    ];
    project::build("Main", move |name| {
        files
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, s)| s.clone())
    })
}

/// The emitted files of a project that type-checks, by name.
fn compiled(main: &str) -> Vec<(String, String)> {
    let project = build(main).expect("the project resolves");
    let (errors, _) = project::check_reporting(&project);
    assert!(
        errors.is_empty(),
        "type-checks: {:?}",
        errors.iter().flat_map(|m| &m.errors).collect::<Vec<_>>()
    );
    project::compile(&project).expect("lowers").files
}

fn file<'a>(files: &'a [(String, String)], name: &str) -> &'a str {
    &files
        .iter()
        .find(|(n, _)| n == name)
        .unwrap_or_else(|| panic!("no {name} in {:?}", names(files)))
        .1
}

fn names(files: &[(String, String)]) -> Vec<&str> {
    files.iter().map(|(n, _)| n.as_str()).collect()
}

/// The errors of `Main`, as messages.
fn main_errors(main: &str) -> Vec<String> {
    let project = build(main).expect("the project resolves");
    project::check_reporting(&project)
        .0
        .into_iter()
        .filter(|m| m.name == "Main")
        .flat_map(|m| m.errors)
        .map(|e| e.message)
        .collect()
}

#[test]
fn a_dotted_import_parses_and_prints_back() {
    for source in [
        "import Shapes.Geometry",
        "from Shapes.Geometry import area, Circle",
        "import Company.Shapes.Geometry",
    ] {
        assert_eq!(pyfun::format(source).unwrap().trim_end(), source);
    }
    // Each part of the path is a module's name, so it is capitalized.
    let lower = pyfun::parse("import Shapes.geometry")
        .unwrap_err()
        .message();
    assert!(lower.contains("a module name after the `.`"), "{lower}");
    // `from` still opens a statement only ahead of a module path and `import`.
    assert!(pyfun::parse("let from = 1\nlet to = from + 1").is_ok());
}

#[test]
fn a_name_maps_to_a_path_under_the_top_folder() {
    assert_eq!(project::module_file_name("Geometry"), "geometry.pyfun");
    assert_eq!(
        project::module_file_name("Shapes.Geometry"),
        "shapes/geometry.pyfun"
    );
    assert_eq!(
        project::module_py_name("Rail.TrainTimes"),
        "rail/train_times.py"
    );
    assert_eq!(project::module_stem("Rail.TrainTimes"), "rail.train_times");
    assert_eq!(project::module_short_name("Rail.TrainTimes"), "TrainTimes");
    assert_eq!(
        project::module_name_from_rel("rail/train_times.pyfun").as_deref(),
        Some("Rail.TrainTimes")
    );
}

#[test]
fn the_module_is_used_by_its_last_name_and_emitted_by_its_path() {
    let files = compiled(
        "import Shapes.Geometry\n\
         let box = Geometry.Rect { width = 2.0, height = 3.0 }\n\
         print (Geometry.area box)\n\
         print (List.map Geometry.area [Geometry.Circle 1.0, box])\n\
         print (Geometry.Pt { x = 1, y = 2 })\n",
    );
    // Each module lands in its package's folder, which gets an `__init__.py`.
    assert_eq!(
        names(&files),
        [
            "shapes/util.py",
            "shapes/geometry.py",
            "main.py",
            "shapes/__init__.py"
        ]
    );
    assert_eq!(file(&files, "shapes/__init__.py"), "");
    let main = file(&files, "main.py");
    assert!(main.starts_with("import shapes.geometry\n"), "{main}");
    assert!(
        main.contains("box = shapes.geometry.Rect(width=2.0, height=3.0)"),
        "{main}"
    );
    assert!(main.contains("print(shapes.geometry.area(box))"), "{main}");
    assert!(
        main.contains("_pf_map(shapes.geometry.area, [shapes.geometry.Circle(1.0), box])"),
        "{main}"
    );
    assert!(main.contains("print(shapes.geometry.Pt(1, 2))"), "{main}");
    // A module in a package names its neighbour from the top, as Python does.
    let geometry = file(&files, "shapes/geometry.py");
    assert!(geometry.contains("import shapes.util\n"), "{geometry}");
    assert!(
        geometry.contains("return shapes.util.double(width * height) / 2.0"),
        "{geometry}"
    );
}

#[test]
fn constructors_and_record_tags_match_through_the_last_name() {
    let files = compiled(
        "import Shapes.Geometry\n\
         let describe shape =\n    match shape:\n        case Geometry.Circle(r): f\"circle {r}\"\n        case Geometry.Rect { width, height }: f\"rect {width}x{height}\"\n\
         let x_of p =\n    match p:\n        case Geometry.Pt { x }: x\n",
    );
    let main = file(&files, "main.py");
    assert!(main.contains("case shapes.geometry.Circle(r):"), "{main}");
    assert!(
        main.contains("case shapes.geometry.Rect(width=width, height=height):"),
        "{main}"
    );
    assert!(main.contains("case shapes.geometry.Pt(x=x):"), "{main}");
}

#[test]
fn a_type_is_written_through_the_last_name_too() {
    let files = compiled(
        "import Shapes.Geometry\n\
         type Scene = Scene(List Geometry.Shape)\n\
         let total scene =\n    match scene:\n        case Scene(shapes): List.map Geometry.area shapes\n",
    );
    // A local called `shapes` takes the package's name, so the import goes by
    // an alias and every reference goes through it.
    let main = file(&files, "main.py");
    assert!(
        main.contains("import shapes.geometry as _pf_shapes_geometry\n"),
        "{main}"
    );
    assert!(main.contains("_pf_shapes_geometry.area"), "{main}");
    assert!(!main.contains("import shapes.geometry\n"), "{main}");
}

#[test]
fn names_are_asked_of_a_module_in_a_package() {
    let files = compiled(
        "from Shapes.Geometry import area, Circle\n\
         from Solids.Geometry import volume\n\
         print (area (Circle 1.0))\n\
         print (volume 2.0)\n",
    );
    let main = file(&files, "main.py");
    assert!(
        main.contains("print(shapes.geometry.area(shapes.geometry.Circle(1.0)))"),
        "{main}"
    );
    assert!(
        main.contains("print(solids.geometry.volume(2.0))"),
        "{main}"
    );
    // Two packages, each with its `__init__.py`.
    assert!(names(&files).contains(&"solids/__init__.py"));
    assert!(names(&files).contains(&"shapes/__init__.py"));
}

#[test]
fn one_module_by_each_last_name() {
    let errors = main_errors(
        "import Shapes.Geometry\nimport Solids.Geometry\nprint (Geometry.volume 2.0)\n",
    );
    assert!(
        errors.iter().any(|e| e.contains(
            "`Geometry` already means `Shapes.Geometry` in this file, so `Solids.Geometry` \
             cannot be imported beside it"
        ) && e.contains("`from Solids.Geometry import …`")),
        "{errors:?}"
    );
    // Asking the second one for names is the way to have both.
    let files = compiled(
        "import Shapes.Geometry\nfrom Solids.Geometry import volume\n\
         print (Geometry.area (Geometry.Circle (volume 1.0)))\n",
    );
    assert!(file(&files, "main.py").contains(
        "print(shapes.geometry.area(shapes.geometry.Circle(solids.geometry.volume(1.0))))"
    ));
}

#[test]
fn the_path_is_written_once() {
    let errors = main_errors("import Shapes.Geometry\nprint (Shapes.Geometry.pi)\n");
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(
        errors[0].starts_with(
            "write `Geometry.pi`: the `import` line gives the module's whole name once"
        ),
        "{errors:?}"
    );
}

#[test]
fn a_module_only_asked_for_names_is_not_bound() {
    let errors = main_errors("from Shapes.Geometry import area\nprint (Geometry.pi)\n");
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(
        errors[0].contains("`Geometry` is not imported as a module")
            && errors[0].contains("Add `import Shapes.Geometry` to write `Geometry.…`"),
        "{errors:?}"
    );
}

#[test]
fn a_member_the_module_lacks_is_named_with_the_whole_path() {
    let errors = main_errors("import Shapes.Geometry\nprint (Geometry.areaa 1)\n");
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(
        errors[0].contains("`areaa` is not a member of `Shapes.Geometry`")
            && errors[0].contains("did you mean `Shapes.Geometry.area`"),
        "{errors:?}"
    );
    let errors = main_errors("from Shapes.Geometry import areaa\n");
    assert_eq!(
        errors,
        vec!["module `Shapes.Geometry` has no `areaa` to import"]
    );
}

#[test]
fn an_unused_package_import_is_a_warning() {
    let project = build("import Shapes.Geometry\nprint 1\n").unwrap();
    let (errors, warned) = project::check_reporting(&project);
    assert!(errors.is_empty());
    let warnings: Vec<String> = warned
        .into_iter()
        .filter(|m| m.name == "Main")
        .flat_map(|m| m.warnings)
        .map(|w| w.message)
        .collect();
    assert_eq!(warnings, vec!["`import Shapes.Geometry` is never used"]);
}

#[test]
fn a_cycle_through_packages_is_named_by_whole_names() {
    let files = [
        ("Main", "import Shapes.A\n"),
        ("Shapes.A", "import Shapes.B\nlet a = 1\n"),
        ("Shapes.B", "import Shapes.A\nlet b = 1\n"),
    ];
    let err = project::build("Main", |name| {
        files
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, s)| s.to_string())
    })
    .unwrap_err();
    assert_eq!(
        err.to_string(),
        "import cycle: Shapes.A -> Shapes.B -> Shapes.A"
    );
}

#[test]
fn a_test_module_is_one_whose_file_is_named_for_pytest() {
    assert!(project::is_test_module("TestGeometry"));
    assert!(project::is_test_module("Shapes.TestGeometry"));
    assert!(!project::is_test_module("Testing.Geometry"));
}

// ---- on disk, through the CLI ----

fn pyfun_bin() -> &'static str {
    env!("CARGO_BIN_EXE_pyfun")
}

fn have_python() -> bool {
    ["python", "python3"]
        .iter()
        .any(|p| Command::new(p).arg("--version").output().is_ok())
}

/// A scratch project directory, removed on drop. A file's name may carry its
/// folders (`shapes/geometry.pyfun`).
struct OnDisk(PathBuf);

impl OnDisk {
    fn new(tag: &str, files: &[(&str, &str)]) -> Self {
        let dir = std::env::temp_dir().join(format!("pyfun_pkg_{tag}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        for (name, source) in files {
            let path = dir.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, source).unwrap();
        }
        OnDisk(dir)
    }

    fn path(&self, file: &str) -> PathBuf {
        self.0.join(file)
    }

    /// Run `pyfun <args>` in the project's top folder.
    fn pyfun(&self, args: &[&str]) -> (bool, String, String) {
        let out = Command::new(pyfun_bin())
            .args(args)
            .current_dir(&self.0)
            .output()
            .expect("pyfun runs");
        (
            out.status.success(),
            String::from_utf8_lossy(&out.stdout).replace("\r\n", "\n"),
            String::from_utf8_lossy(&out.stderr).replace("\r\n", "\n"),
        )
    }
}

impl Drop for OnDisk {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

const ON_DISK: &[(&str, &str)] = &[
    (
        "main.pyfun",
        "import Shapes.Geometry\nfrom Solids.Geometry import volume\n\nprint (Geometry.area (Geometry.Circle 1.0))\nprint (volume 2.0)\n",
    ),
    ("shapes/geometry.pyfun", GEOMETRY),
    ("shapes/util.pyfun", "let double x = x * 2.0\n"),
    (
        "solids/geometry.pyfun",
        "from Shapes.Util import double\nlet volume side = double (side * side * side)\n",
    ),
];

#[test]
fn check_reads_modules_from_their_folders() {
    let project = OnDisk::new("check", ON_DISK);
    let (ok, out, err) = project.pyfun(&["check", "main.pyfun"]);
    assert!(ok, "{out}{err}");
    assert!(format!("{out}{err}").contains("4 modules"), "{out}{err}");
}

#[test]
fn compile_writes_the_package_tree() {
    let project = OnDisk::new("compile", ON_DISK);
    let (ok, out, err) = project.pyfun(&["compile", "main.pyfun", "-o", "out"]);
    assert!(ok, "{out}{err}");
    for name in [
        "main.py",
        "shapes/geometry.py",
        "shapes/util.py",
        "shapes/__init__.py",
        "solids/geometry.py",
        "solids/__init__.py",
    ] {
        assert!(project.path("out").join(name).is_file(), "no out/{name}");
    }
}

#[test]
fn run_executes_a_program_spread_over_packages() {
    if !have_python() {
        return;
    }
    let project = OnDisk::new("run", ON_DISK);
    let (ok, out, err) = project.pyfun(&["run", "main.pyfun"]);
    assert!(ok, "{out}{err}");
    assert_eq!(out, "3.0\n16.0\n");
}

#[test]
fn json_is_one_type_across_the_files_of_a_project() {
    // The `Json` classes live in the shared runtime, so a value one module
    // builds is matched, compared and encoded by another.
    if !have_python() {
        return;
    }
    let scratch = OnDisk::new(
        "json",
        &[
            (
                "store/loader.pyfun",
                "type Doc = { id: int, extra: Json }\n\
                 let load text = Decode.decode_string Decode.value text\n\
                 let blank = JNull\n\
                 let wrap n = JList [JInt n, blank]\n",
            ),
            (
                "main.pyfun",
                "import Store.Loader\n\
                 let size j =\n  match j:\n    case JList xs: List.len xs\n    case JObject m: Map.len m\n    case _: 1\n\
                 match Loader.load \"[1, [2, 3], null]\":\n  case Ok j: print (size j)\n  case Error e: print e.error_message\n\
                 print (Loader.wrap 3 == JList [JInt 3, JNull])\n\
                 Loader.wrap 3 |> Encode.value |> print\n\
                 Loader.Doc { id = 1, extra = Loader.blank } |> Encode.auto |> print\n",
            ),
        ],
    );
    let (ok, out, err) = scratch.pyfun(&["run", "main.pyfun"]);
    assert!(ok, "{out}{err}");
    assert_eq!(
        out.replace("\r\n", "\n"),
        "3\nTrue\n[3, null]\n{\"id\": 1, \"extra\": null}\n"
    );
    // Each module imports the classes; neither defines them.
    let (ok, out, err) = scratch.pyfun(&["compile", "main.pyfun", "-o", "out"]);
    assert!(ok, "{out}{err}");
    let main = fs::read_to_string(scratch.path("out/main.py")).unwrap();
    let loader = fs::read_to_string(scratch.path("out/store/loader.py")).unwrap();
    let runtime = fs::read_to_string(scratch.path("out/_pyfun_rt.py")).unwrap();
    for module in [&main, &loader] {
        assert!(
            module.contains("from _pyfun_rt import JNull, JBool"),
            "{module}"
        );
        assert!(!module.contains("class JNull"), "{module}");
    }
    assert!(runtime.contains("class JObject:"), "{runtime}");
}

#[test]
fn the_shipped_example_runs() {
    // `examples/packages/` stays working end to end.
    if !have_python() {
        return;
    }
    let entry = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/packages/main.pyfun");
    let out = Command::new(pyfun_bin())
        .arg("run")
        .arg(&entry)
        .output()
        .expect("pyfun runs");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).replace("\r\n", "\n"),
        "a 4.0 by 5.0 rectangle\na circle of radius 1.0\n20.0\n1800.0\n[1800.0, 600.0]\n"
    );
}

#[test]
fn a_traceback_names_a_package_module_by_its_source_path() {
    if !have_python() {
        return;
    }
    let project = OnDisk::new(
        "trace",
        &[
            (
                "main.pyfun",
                "import Rail.Times\nprint (Times.share 10 0)\n",
            ),
            ("rail/times.pyfun", "let share total n =\n    total // n\n"),
        ],
    );
    let (ok, _, err) = project.pyfun(&["run", "main.pyfun"]);
    assert!(!ok);
    let sep = std::path::MAIN_SEPARATOR;
    assert!(
        err.contains(&format!("File \"rail{sep}times.pyfun\", line 2, in share")),
        "{err}"
    );
    assert!(err.contains("    total // n\n"), "{err}");
    assert!(!err.contains("times.py\""), "{err}");
}

#[test]
fn a_neighbour_imported_by_its_bare_name_is_explained() {
    // Python 2's habit: a module in a package naming the one beside it bare.
    let project = OnDisk::new(
        "beside",
        &[
            ("main.pyfun", "import Shapes.Geometry\nprint Geometry.two\n"),
            (
                "shapes/geometry.pyfun",
                "import Util\nlet two = Util.double 1.0\n",
            ),
            ("shapes/util.pyfun", "let double x = x * 2.0\n"),
        ],
    );
    let (ok, _, err) = project.pyfun(&["check", "main.pyfun"]);
    assert!(!ok);
    assert!(
        err.contains(
            "cannot find module `Util` (expected `util.pyfun`), imported by `Shapes.Geometry`"
        ),
        "{err}"
    );
    assert!(
        err.contains("note: `shapes/util.pyfun` sits beside `Shapes.Geometry`")
            && err.contains("so write `import Shapes.Util`"),
        "{err}"
    );
}

#[test]
fn a_file_inside_a_package_is_checked_through_its_program() {
    let project = OnDisk::new("inside", ON_DISK);
    let (ok, _, err) = project.pyfun(&["check", "shapes/geometry.pyfun"]);
    assert!(!ok);
    assert!(
        err.contains("cannot find module `Shapes.Util` (expected `shapes/util.pyfun`)"),
        "{err}"
    );
    assert!(
        err.contains("note: `shapes/util.pyfun` is found from")
            && err.contains("run or check the program's main file there"),
        "{err}"
    );
}

#[test]
fn tests_at_the_top_reach_into_packages() {
    if !have_python() {
        return;
    }
    let has_pytest = ["python", "python3"].iter().any(|p| {
        Command::new(std::env::var("PYFUN_PYTHON").unwrap_or(p.to_string()))
            .args(["-m", "pytest", "--version"])
            .output()
            .is_ok_and(|out| out.status.success())
    });
    if !has_pytest {
        return;
    }
    let mut files = ON_DISK.to_vec();
    files.push((
        "test_shapes.pyfun",
        "import Shapes.Geometry\nfrom Solids.Geometry import volume\n\nlet test_area _ = Test.equal 3.0 (Geometry.area (Geometry.Circle 1.0))\nlet test_volume _ = Test.equal 16.0 (volume 2.0)\n",
    ));
    let project = OnDisk::new("tests", &files);
    let (ok, out, err) = project.pyfun(&["test"]);
    assert!(ok, "{out}{err}");
    assert!(out.contains("2 passed"), "{out}");
}
