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
                "import Store\nlet hit = Option.with_default 0 (Store.lookup 7)",
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
                 let hit = Option.with_default 0 (Store.lookup 7)\n\
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
                 extern get_argv : unit -> List str = sys.argv.copy\n\
                 print (Geometry.area 4 5)\n\
                 print (get_argv ())",
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

/// A program that builds and has two warnings: an unused local and an arm that
/// cannot match.
const WARNED: &str = "let f n =\n    let unused = 5\n    match n:\n        case 0: \"zero\"\n        case 0: \"again\"\n        case _: \"other\"\nprint (f 0)\n";

#[test]
fn check_reports_warnings_and_still_succeeds() {
    let project = Project::new("warn_check", &[("main.pyfun", WARNED)]);
    let out = Command::new(pyfun_bin())
        .arg("check")
        .arg(project.path("main.pyfun"))
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    assert!(
        stderr.contains("warning: `unused` is never used"),
        "{stderr}"
    );
    assert!(
        stderr.contains("warning: this `case` can never match"),
        "{stderr}"
    );
    assert!(
        stderr.contains("ok: no type errors (2 warnings)"),
        "{stderr}"
    );
}

#[test]
fn check_strict_makes_a_warning_a_failure() {
    let project = Project::new(
        "warn_strict",
        &[("main.pyfun", WARNED), ("clean.pyfun", "print 42\n")],
    );
    // The flag reads on either side of the path.
    for args in [
        ["check", "--strict", "main.pyfun"],
        ["check", "main.pyfun", "--strict"],
    ] {
        let out = Command::new(pyfun_bin())
            .args(args)
            .current_dir(&project.0)
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(!out.status.success(), "{stderr}");
        assert!(stderr.contains("no type errors (2 warnings)"), "{stderr}");
        assert!(
            stderr.contains("error: `--strict` makes a warning a failure"),
            "{stderr}"
        );
    }
    // A program without warnings passes under it.
    let out = Command::new(pyfun_bin())
        .args(["check", "--strict", "clean.pyfun"])
        .current_dir(&project.0)
        .output()
        .unwrap();
    assert!(out.status.success());
}

#[test]
fn check_reports_each_modules_warnings_in_a_project() {
    // `Geometry` is imported and never used; the unused local is in the other
    // module. Each is rendered under its own module's heading.
    let project = Project::new(
        "warn_project",
        &[
            (
                "main.pyfun",
                "import Geometry\nimport Extra\nprint (Extra.twice 2)\n",
            ),
            GEOMETRY,
            (
                "extra.pyfun",
                "let twice n =\n    let unused = 1\n    n * 2\n",
            ),
        ],
    );
    let out = Command::new(pyfun_bin())
        .arg("check")
        .arg(project.path("main.pyfun"))
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    assert!(
        stderr.contains("warning: `import Geometry` is never used"),
        "{stderr}"
    );
    // The module is headed with the file it was read from: beside the entry,
    // spelled the way the entry was given.
    let extra = project.path("extra.pyfun").display().to_string();
    assert!(
        stderr.contains(&format!("-- module `Extra` ({extra}) --")),
        "{stderr}"
    );
    assert!(
        stderr.contains("warning: `unused` is never used"),
        "{stderr}"
    );
    assert!(
        stderr.contains("ok: no type errors (3 modules) (2 warnings)"),
        "{stderr}"
    );
}

#[test]
fn run_and_compile_print_warnings_beside_the_program() {
    let project = Project::new("warn_run", &[("main.pyfun", WARNED)]);
    // `compile` writes the Python to stdout and the warnings to stderr.
    let out = Command::new(pyfun_bin())
        .arg("compile")
        .arg(project.path("main.pyfun"))
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stdout.contains("def f(n):"), "{stdout}");
    assert!(!stdout.contains("warning:"), "{stdout}");
    assert!(
        stderr.contains("warning: `unused` is never used"),
        "{stderr}"
    );
    if !have_python() {
        return;
    }
    // `run` prints them and then runs the program, whose output is untouched.
    let out = Command::new(pyfun_bin())
        .arg("run")
        .arg(project.path("main.pyfun"))
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "zero");
    assert!(
        stderr.contains("warning: this `case` can never match"),
        "{stderr}"
    );
}

// ---------- every error names its file, and a run reports them all ----------

/// `pyfun check <file>` run inside the project's folder, so the paths it
/// prints are the short ones a user sees.
fn check_in(proj: &Project, file: &str) -> (bool, String) {
    let out = Command::new(pyfun_bin())
        .current_dir(&proj.0)
        .arg("check")
        .arg(file)
        .output()
        .unwrap();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stderr).replace("\r\n", "\n"),
    )
}

#[test]
fn check_reports_every_old_spelling_of_a_file_in_one_run() {
    // What moving a 0.10 file to 0.11 meets: each of these used to stop the
    // compiler, one run per fix, and an `else` was hidden behind its `then`.
    let proj = Project::new(
        "all_syntax",
        &[(
            "old.pyfun",
            "let a = true\n\
             let b = false\n\
             let f x =\n\
             \x20 if x then\n\
             \x20   if a then\n\
             \x20     1\n\
             \x20   else\n\
             \x20     3\n\
             \x20 else\n\
             \x20   2\n\
             let g = f b\n",
        )],
    );
    let (ok, stderr) = check_in(&proj, "old.pyfun");
    assert!(!ok);
    for at in ["1:9", "2:9", "4:8", "5:10", "7:5", "9:3"] {
        assert!(
            stderr.contains(&format!(" --> old.pyfun:{at}\n")),
            "{stderr}"
        );
    }
    assert!(stderr.ends_with("\n6 errors\n"), "{stderr}");
    // `compile` and `run` say the same.
    for command in ["compile", "run"] {
        let out = Command::new(pyfun_bin())
            .current_dir(&proj.0)
            .arg(command)
            .arg("old.pyfun")
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(!out.status.success());
        assert!(stderr.contains("6 errors"), "{command}: {stderr}");
    }
}

#[test]
fn a_parse_error_inside_a_block_is_one_error() {
    // The broken line is two blocks deep. What is left of the function is
    // skipped with it, so nothing follows from it, and the next function's
    // own error is still found.
    let proj = Project::new(
        "resync",
        &[(
            "deep.pyfun",
            "let f x =\n\
             \x20 let y =\n\
             \x20   let z = x +\n\
             \x20   z\n\
             \x20 y + 1\n\
             let g = )\n",
        )],
    );
    let (ok, stderr) = check_in(&proj, "deep.pyfun");
    assert!(!ok);
    // The line that stops short is reported where it stops.
    assert!(stderr.contains(" --> deep.pyfun:3:16\n"), "{stderr}");
    assert!(stderr.contains(" --> deep.pyfun:6:9\n"), "{stderr}");
    assert!(stderr.ends_with("\n2 errors\n"), "{stderr}");
}

#[test]
fn a_syntax_error_in_an_imported_module_names_its_file() {
    // The entry and a module it imports both fail to parse. Each is headed
    // with its file, each error line names it, and one run has them all.
    let proj = Project::new(
        "syntax_files",
        &[
            ("helper.pyfun", "let a = true\nlet b = 1\n"),
            (
                "main.pyfun",
                "import Helper\nprint Helper.a\nlet z = false\n",
            ),
        ],
    );
    let (ok, stderr) = check_in(&proj, "main.pyfun");
    assert!(!ok);
    let helper = stderr
        .find("-- module `Helper` (helper.pyfun) --")
        .expect(&stderr);
    let main = stderr
        .find("-- module `Main` (main.pyfun) --")
        .expect(&stderr);
    assert!(helper < main, "{stderr}");
    assert!(stderr.contains(" --> helper.pyfun:1:9\n"), "{stderr}");
    assert!(stderr.contains(" --> main.pyfun:3:9\n"), "{stderr}");
    assert!(stderr.ends_with("\n2 errors\n"), "{stderr}");
}

#[test]
fn a_module_is_headed_with_the_file_it_was_read_from() {
    // `_tool.pyfun` is the module `Tool`, and the module's name does not say
    // where the leading underscore went.
    let proj = Project::new(
        "real_file",
        &[
            ("helper.pyfun", "let one = 1\n"),
            (
                "_tool.pyfun",
                "import Helper\nlet unused = 1\nlet total = Helper.one + \"x\"\n",
            ),
        ],
    );
    let (ok, stderr) = check_in(&proj, "_tool.pyfun");
    assert!(!ok);
    assert!(
        stderr.contains("-- module `Tool` (_tool.pyfun) --"),
        "{stderr}"
    );
    assert!(stderr.contains(" --> _tool.pyfun:3:26\n"), "{stderr}");
    assert!(!stderr.contains("(tool.pyfun)"), "{stderr}");
}

#[test]
fn a_failed_declaration_is_not_reported_again_by_the_modules_that_use_it() {
    let proj = Project::new(
        "one_cause",
        &[
            (
                "net.pyfun",
                "extern connect: string -> int -> int = max\n\
                 type Hello = { name: string, seed: int }\n\
                 type Wire = Greeting Hello | Line Nope\n\
                 let version = 2\n",
            ),
            (
                "main.pyfun",
                "import Net\n\
                 let a = Net.connect \"host\" 7777\n\
                 let b = Net.Hello { name = \"Ann\", seed = 1 }\n\
                 let show m =\n\
                 \x20 match m:\n\
                 \x20   case Net.Line l: l\n\
                 \x20   case Net.Greeting h: h.name\n\
                 print Net.version\n",
            ),
        ],
    );
    let (ok, stderr) = check_in(&proj, "main.pyfun");
    assert!(!ok);
    assert!(
        stderr.contains("-- module `Net` (net.pyfun) --"),
        "{stderr}"
    );
    assert!(!stderr.contains("module `Main`"), "{stderr}");
    assert!(stderr.ends_with("\n3 errors\n"), "{stderr}");
}

#[test]
fn a_parse_error_names_the_token_it_met() {
    for (source, found) in [
        ("let f x =\n  let y = x\n  elif y\n", "found `elif`"),
        ("let g x = x ** case\n", "found `case`"),
        ("let k x = [x, as]\n", "found `as`"),
        ("let m x = (% 2)\n", "found `%`"),
        ("let p x = x +\nlet q = 1\n", "found end of statement"),
    ] {
        let proj = Project::new("token_names", &[("t.pyfun", source)]);
        let (ok, stderr) = check_in(&proj, "t.pyfun");
        assert!(!ok);
        assert!(stderr.contains(found), "{source}: {stderr}");
        assert!(!stderr.contains("`token`"), "{source}: {stderr}");
    }
}
