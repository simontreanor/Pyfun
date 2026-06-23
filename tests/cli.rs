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
