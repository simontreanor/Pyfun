//! Tests for the `pyfun` CLI over a multi-file project (`DESIGN.md` §6.1).
//!
//! These drive the built binary (cargo exposes its path via `CARGO_BIN_EXE_*`),
//! writing a small project to a temp directory and invoking `check`/`compile`/
//! `run` on its entry file. Execution cases skip (not fail) when no Python is on
//! PATH; the check/compile cases need no interpreter.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn pyfun_bin() -> &'static str {
    env!("CARGO_BIN_EXE_pyfun")
}

fn have_python() -> bool {
    ["python", "python3"]
        .iter()
        .any(|p| Command::new(p).arg("--version").output().is_ok())
}

/// A unique scratch project directory, cleaned up on drop.
struct Project(PathBuf);

impl Project {
    fn new(tag: &str, files: &[(&str, &str)]) -> Self {
        let dir = std::env::temp_dir().join(format!("pyfun_cli_{tag}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        for (name, source) in files {
            fs::write(dir.join(name), source).unwrap();
        }
        Project(dir)
    }

    fn path(&self, file: &str) -> PathBuf {
        self.0.join(file)
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

const GEOMETRY: (&str, &str) = ("geometry.pyfun", "let area w h = w * h");

#[test]
fn bundle_writes_a_page_a_loader_the_modules_and_the_assets() {
    // #111: a project entry, one asset and a page fragment become a static site.
    let project = Project::new(
        "bundle",
        &[
            ("main.pyfun", "import Geometry\nprint (Geometry.area 2 3)"),
            GEOMETRY,
            ("words.txt", "alpha\nbeta\n"),
            ("page.html", "<h1>Area</h1>"),
        ],
    );
    let site = project.path("site");
    let out = Command::new(pyfun_bin())
        .args(["bundle"])
        .arg(project.path("main.pyfun"))
        .arg("-o")
        .arg(&site)
        .arg("--asset")
        .arg(project.path("words.txt"))
        .arg("--page")
        .arg(project.path("page.html"))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    for name in [
        "index.html",
        "pyfun-bundle.js",
        "main.py",
        "geometry.py",
        "words.txt",
    ] {
        assert!(site.join(name).is_file(), "missing {name}");
    }
    let index = fs::read_to_string(site.join("index.html")).unwrap();
    assert!(index.contains("<h1>Area</h1>"), "{index}");
    assert!(index.contains("pyfun-bundle.js"), "{index}");
    assert!(index.contains("<title>main</title>"), "{index}");
    let loader = fs::read_to_string(site.join("pyfun-bundle.js")).unwrap();
    assert!(
        loader.contains("const FILES = [\"geometry.py\", \"main.py\"];"),
        "{loader}"
    );
    assert!(
        loader.contains("const ASSETS = [\"words.txt\"];"),
        "{loader}"
    );
    assert!(loader.contains("const ENTRY = \"main.py\";"), "{loader}");
    assert!(loader.contains("cdn.jsdelivr.net/pyodide/"), "{loader}");
    // A single file bundles as `main.py` with no page fragment.
    let single = Project::new("bundle_single", &[("hello.pyfun", "print 42")]);
    let site = single.path("site");
    let out = Command::new(pyfun_bin())
        .args(["bundle"])
        .arg(single.path("hello.pyfun"))
        .arg("-o")
        .arg(&site)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        fs::read_to_string(site.join("main.py"))
            .unwrap()
            .contains("print(42)")
    );
    let loader = fs::read_to_string(site.join("pyfun-bundle.js")).unwrap();
    assert!(loader.contains("const FILES = [\"main.py\"];"), "{loader}");
    // A type error stops the bundle before anything is written.
    let bad = Project::new("bundle_bad", &[("bad.pyfun", "let x = 1 + \"s\"")]);
    let out = Command::new(pyfun_bin())
        .args(["bundle"])
        .arg(bad.path("bad.pyfun"))
        .arg("-o")
        .arg(bad.path("site"))
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(!bad.path("site").exists());
}

#[test]
fn version_flag_prints_the_crate_version() {
    // The first command a cautious newcomer runs; all three spellings work.
    for flag in ["--version", "-V", "version"] {
        let out = Command::new(pyfun_bin()).arg(flag).output().unwrap();
        assert!(out.status.success(), "{flag}");
        let stdout = String::from_utf8(out.stdout).unwrap();
        assert_eq!(
            stdout.trim(),
            format!("pyfun {}", env!("CARGO_PKG_VERSION")),
            "{flag}"
        );
    }
}

#[test]
fn check_passes_over_the_whole_graph() {
    let proj = Project::new(
        "check_ok",
        &[
            GEOMETRY,
            (
                "main.pyfun",
                "import Geometry\nlet floor = Geometry.area 4 5",
            ),
        ],
    );
    let out = Command::new(pyfun_bin())
        .arg("check")
        .arg(proj.path("main.pyfun"))
        .output()
        .unwrap();
    assert!(out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("no type errors"), "{stderr}");
}

#[test]
fn check_reports_a_cross_module_error_in_the_right_module() {
    let proj = Project::new(
        "check_err",
        &[
            GEOMETRY,
            (
                "main.pyfun",
                "import Geometry\nlet bad = Geometry.area 4 \"five\"",
            ),
        ],
    );
    let out = Command::new(pyfun_bin())
        .arg("check")
        .arg(proj.path("main.pyfun"))
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("module `Main`"), "{stderr}");
    assert!(stderr.contains("type mismatch"), "{stderr}");
}

#[test]
fn check_reports_a_missing_import() {
    let proj = Project::new("check_missing", &[("main.pyfun", "import Nope\nlet x = 1")]);
    let out = Command::new(pyfun_bin())
        .arg("check")
        .arg(proj.path("main.pyfun"))
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("cannot find module `Nope`"), "{stderr}");
}

#[test]
fn check_reports_an_import_cycle() {
    let proj = Project::new(
        "check_cycle",
        &[
            ("a.pyfun", "import B\nlet a = 1"),
            ("b.pyfun", "import A\nlet b = 1"),
        ],
    );
    let out = Command::new(pyfun_bin())
        .arg("check")
        .arg(proj.path("a.pyfun"))
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("import cycle"), "{stderr}");
}

#[test]
fn compile_writes_the_python_tree_to_a_directory() {
    let proj = Project::new(
        "compile_dir",
        &[
            ("store.pyfun", "let lookup k = Some k"),
            (
                "main.pyfun",
                "import Store\nlet hit = Option.withDefault 0 (Store.lookup 7)",
            ),
        ],
    );
    let out_dir = proj.path("out");
    let out = Command::new(pyfun_bin())
        .arg("compile")
        .arg(proj.path("main.pyfun"))
        .arg("-o")
        .arg(&out_dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    // Module files plus the shared runtime (an Option crosses the boundary).
    for f in ["main.py", "store.py", "_pyfun_rt.py"] {
        assert!(out_dir.join(f).exists(), "missing {f}");
    }
    let main = fs::read_to_string(out_dir.join("main.py")).unwrap();
    assert!(main.contains("import store"), "{main}");
    assert!(main.contains("from _pyfun_rt import"), "{main}");
}

#[test]
fn run_executes_a_multi_file_project() {
    if !have_python() {
        eprintln!("skipping multi-file `run`: no python interpreter");
        return;
    }
    let proj = Project::new(
        "run_multi",
        &[
            GEOMETRY,
            ("store.pyfun", "let lookup k = Some k"),
            (
                "main.pyfun",
                "import Geometry\nimport Store\n\
                 let floor = Geometry.area 4 5\n\
                 let hit = Option.withDefault 0 (Store.lookup 7)\n\
                 print floor\nprint hit",
            ),
        ],
    );
    let out = Command::new(pyfun_bin())
        .arg("run")
        .arg(proj.path("main.pyfun"))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(stdout.replace("\r\n", "\n").trim(), "20\n7");
}

#[test]
fn run_forwards_arguments_to_a_multi_file_project() {
    if !have_python() {
        eprintln!("skipping multi-file `run` argv test: no python interpreter");
        return;
    }
    // Arguments after the entry file reach the program's `sys.argv` on the
    // project path too, with and without the `--` separator (issue #73).
    let proj = Project::new(
        "run_argv",
        &[
            GEOMETRY,
            (
                "main.pyfun",
                "import Geometry\n\
                 extern import sys\n\
                 extern getArgv : unit -> List string = sys.argv.copy\n\
                 print (Geometry.area 4 5)\n\
                 print (getArgv ())",
            ),
        ],
    );
    for extra in [["hello", "42"].as_slice(), ["--", "hello", "42"].as_slice()] {
        let out = Command::new(pyfun_bin())
            .arg("run")
            .arg(proj.path("main.pyfun"))
            .args(extra)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains("'hello', '42'") && !stdout.contains("'--'"),
            "arguments {extra:?} should reach sys.argv, got:\n{stdout}"
        );
        assert!(
            stdout.contains("main.py"),
            "sys.argv[0] should be the staged entry script, got:\n{stdout}"
        );
    }
}

#[test]
fn the_committed_modules_example_runs() {
    // Keep the shipped multi-file example (`examples/modules/`) working end-to-end.
    if !have_python() {
        eprintln!("skipping example run: no python interpreter");
        return;
    }
    let entry = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/modules/main.pyfun");
    let out = Command::new(pyfun_bin())
        .arg("run")
        .arg(&entry)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout).replace("\r\n", "\n");
    assert_eq!(stdout.trim(), "20\n9\n20\n100\n0");
}

#[test]
fn single_file_without_imports_still_inlines_classes() {
    // Back-compat: a no-import file uses the single-file path — classes inlined,
    // emitted to stdout, no shared runtime.
    let proj = Project::new("solo", &[("solo.pyfun", "let x = Some 1")]);
    let out = Command::new(pyfun_bin())
        .arg("compile")
        .arg(proj.path("solo.pyfun"))
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("class Some"), "{stdout}");
    assert!(!stdout.contains("_pyfun_rt"), "{stdout}");
}
