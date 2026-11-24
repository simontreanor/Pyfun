//! `pyfun test` and the `Test` module (`DESIGN.md` §9).

use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn compile(source: &str) -> String {
    pyfun::compile(source).unwrap_or_else(|e| panic!("compiles: {e}\n{source}"))
}

fn error_of(source: &str) -> String {
    match pyfun::compile(source) {
        Ok(python) => panic!("expected an error, compiled to:\n{python}"),
        Err(e) => e.message(),
    }
}

// ---------- The `Test` module ----------

#[test]
fn an_expectation_is_an_assert_where_it_stands() {
    // The tested value goes on the left, where pytest's report puts it.
    let python = compile(
        "let double x = x * 2\nlet test_double _ =\n    Test.equal 4 (double 2)\n    Test.is_true (double 1 > 1)",
    );
    assert!(
        python
            .contains("def test_double(_):\n    assert double(2) == 4\n    assert double(1) > 1\n"),
        "{python}"
    );
}

#[test]
fn a_failure_fits_the_branch_it_stands_in() {
    let python = compile(
        "let test_first _ =\n    match List.head [1, 2]:\n        case Some(n): Test.equal 1 n\n        case None: Test.fail \"an empty list\"",
    );
    assert!(python.contains("assert n == 1\n"), "{python}");
    assert!(
        python.contains("assert False, \"an empty list\"\n"),
        "{python}"
    );
    // It fits a branch of any type, not only `unit`.
    compile(
        "let test_value _ =\n    let n = match List.head [1]:\n        case Some(n): n\n        case None: Test.fail \"empty\"\n    Test.equal 1 n",
    );
}

#[test]
fn an_expectation_passed_as_a_value_keeps_a_helper() {
    let python = compile("let test_all_ones _ =\n    [1, 1] |> List.iter (Test.equal 1)");
    assert!(
        python.contains("def _pf_test_equal(expected, actual):\n    assert actual == expected\n"),
        "{python}"
    );
    assert!(
        python.contains("functools.partial(_pf_test_equal, 1)"),
        "{python}"
    );
}

#[test]
fn an_expectation_inside_a_tests_own_lambda_is_in_the_test() {
    compile("let test_each _ =\n    [1, 2] |> List.iter (fun x -> Test.is_true (x > 0))");
}

#[test]
fn a_helper_may_assert_and_its_type_says_so() {
    // An expectation is allowed anywhere. It performs `exn`, which a helper
    // that asserts inherits, so `let pure` keeps one out and `try` handles it.
    for source in [
        "let helper x = Test.is_true (x > 0)",
        "let check = List.iter (Test.equal 1)",
        "Test.equal 1 1",
        "let positive x = Test.is_true (x > 0)\nlet test_one _ = positive 1",
        "let positive x = Test.is_true (x > 0)\nlet pure caught x = try (positive x)",
    ] {
        compile(source);
    }
    let message = error_of("let pure positive x = Test.is_true (x > 0)");
    assert!(
        message.contains("is declared `pure` but performs `exn`"),
        "{message}"
    );
    // A helper keeps the `assert`, where pytest explains the failure.
    let py = compile("let positive x = Test.is_true (x > 0)");
    assert!(py.contains("def positive(x):\n    assert x > 0"), "{py}");
}

// ---------- The command ----------

/// The interpreter `pyfun test` will use, if it has pytest.
fn python_with_pytest() -> Option<String> {
    let candidates: Vec<String> = std::env::var("PYFUN_PYTHON")
        .ok()
        .into_iter()
        .chain(["python".to_string(), "python3".to_string()])
        .collect();
    let python = candidates
        .into_iter()
        .find(|p| Command::new(p).arg("--version").output().is_ok())?;
    Command::new(&python)
        .args(["-m", "pytest", "--version"])
        .output()
        .is_ok_and(|out| out.status.success())
        .then_some(python)
}

fn workdir(tag: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("pyfun_testcmd_{tag}_{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    for (name, source) in files {
        fs::write(dir.join(name), source).unwrap();
    }
    dir
}

/// Run `pyfun test <args>` in `dir`; `(succeeded, stdout, stderr)`.
fn pyfun_test(dir: &PathBuf, args: &[&str]) -> (bool, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_pyfun"))
        .arg("test")
        .args(args)
        .current_dir(dir)
        // Plain output, whatever the terminal running the tests is.
        .env("NO_COLOR", "1")
        .env_remove("FORCE_COLOR")
        .env_remove("PYTEST_ADDOPTS")
        .output()
        .expect("spawn pyfun test");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).replace("\r\n", "\n"),
        String::from_utf8_lossy(&out.stderr).replace("\r\n", "\n"),
    )
}

const SHAPES: &str = "\
type Shape = Circle(float) | Rect { width: float, height: float }

let area shape =
    match shape:
        case Circle(r): 3.0 * r * r
        case Rect { width, height }: width * height
";

#[test]
fn passing_tests_pass() {
    if python_with_pytest().is_none() {
        eprintln!("skipping: no python with pytest");
        return;
    }
    let dir = workdir(
        "pass",
        &[
            ("shapes.pyfun", SHAPES),
            (
                "test_shapes.pyfun",
                "import Shapes\n\n## A rectangle is its sides multiplied.\nlet test_rect _ =\n    Test.equal 6.0 (Shapes.area (Shapes.Rect { width = 2.0, height = 3.0 }))\n\nlet test_circle unused =\n    Test.is_true (Shapes.area (Shapes.Circle 1.0) > 2.0)\n",
            ),
        ],
    );
    // No path: the test files of the current directory.
    let (ok, stdout, stderr) = pyfun_test(&dir, &[]);
    let _ = fs::remove_dir_all(&dir);
    assert!(ok, "{stdout}\n{stderr}");
    assert!(stdout.contains("2 passed"), "{stdout}");
}

#[test]
fn a_failing_test_is_reported_by_pytest_with_both_values() {
    if python_with_pytest().is_none() {
        eprintln!("skipping: no python with pytest");
        return;
    }
    let dir = workdir(
        "fail",
        &[(
            "test_lists.pyfun",
            "let test_right _ =\n    Test.equal [1, 2] (List.map (fun x -> x + 1) [0, 1])\n\nlet test_wrong _ =\n    Test.equal [1, 2, 3] (List.map (fun x -> x + 1) [0, 1, 3])\n",
        )],
    );
    let (ok, stdout, stderr) = pyfun_test(&dir, &["test_lists.pyfun"]);
    assert!(!ok, "a failing test fails the command:\n{stdout}\n{stderr}");
    assert!(stdout.contains("1 failed, 1 passed"), "{stdout}");
    // pytest's own account of the assertion: the expression, then the values.
    assert!(stdout.contains("def test_wrong():"), "{stdout}");
    assert!(stdout.contains("assert [1, 2, 4] == [1, 2, 3]"), "{stdout}");

    // What follows `--` is pytest's: here, pick one test by name.
    let (ok, stdout, stderr) = pyfun_test(&dir, &["test_lists.pyfun", "--", "-k", "test_right"]);
    let _ = fs::remove_dir_all(&dir);
    assert!(ok, "{stdout}\n{stderr}");
    assert!(stdout.contains("1 passed, 1 deselected"), "{stdout}");
}

#[test]
fn a_failure_inside_a_helper_is_explained_there() {
    if python_with_pytest().is_none() {
        eprintln!("skipping: no python with pytest");
        return;
    }
    // A helper shared by tests asserts for them, and one test handles a
    // failed expectation with `try`.
    let dir = workdir(
        "helper",
        &[(
            "test_sorted.pyfun",
            "let is_sorted xs = List.sort xs == xs\n\n\
             let expect_sorted xs = Test.is_true (is_sorted xs)\n\n\
             let test_sorted _ =\n    expect_sorted [1, 2, 3]\n\n\
             let test_unsorted _ =\n    expect_sorted [3, 1]\n\n\
             let test_caught _ =\n    match try (expect_sorted [2, 1]):\n        case Ok _: Test.fail \"it should have failed\"\n        case Error e: Test.equal \"AssertionError\" e.error_kind\n",
        )],
    );
    let (ok, stdout, stderr) = pyfun_test(&dir, &[]);
    let _ = fs::remove_dir_all(&dir);
    assert!(!ok, "{stdout}\n{stderr}");
    assert!(stdout.contains("1 failed, 2 passed"), "{stdout}");
    // pytest reads the helper's `assert` as it reads a test's.
    assert!(stdout.contains("def expect_sorted(xs):"), "{stdout}");
    assert!(
        stdout.contains("where False = is_sorted([3, 1])"),
        "{stdout}"
    );
}

#[test]
fn an_ill_typed_test_file_is_stopped_by_the_compiler() {
    if python_with_pytest().is_none() {
        eprintln!("skipping: no python with pytest");
        return;
    }
    let dir = workdir(
        "illtyped",
        &[(
            "test_bad.pyfun",
            "let test_bad _ =\n    Test.equal 1 \"one\"\n",
        )],
    );
    let (ok, stdout, stderr) = pyfun_test(&dir, &[]);
    let _ = fs::remove_dir_all(&dir);
    assert!(!ok);
    assert!(stderr.contains("type mismatch"), "{stderr}");
    assert!(
        !stdout.contains("test session starts"),
        "pytest never ran:\n{stdout}"
    );
}

#[test]
fn what_is_and_is_not_a_test_is_said_plainly() {
    if python_with_pytest().is_none() {
        eprintln!("skipping: no python with pytest");
        return;
    }
    let dir = workdir(
        "shape",
        &[
            ("checks.pyfun", "let test_one _ =\n    Test.equal 1 1\n"),
            (
                "test_two_params.pyfun",
                "let test_add a b =\n    Test.equal 2 (a + b)\n",
            ),
        ],
    );
    fs::create_dir_all(dir.join("empty")).unwrap();
    let (ok, _, stderr) = pyfun_test(&dir, &["checks.pyfun"]);
    assert!(!ok);
    assert!(
        stderr.contains("is not a test file") && stderr.contains("`test_checks.pyfun`"),
        "{stderr}"
    );
    let (ok, _, stderr) = pyfun_test(&dir, &["test_two_params.pyfun"]);
    assert!(!ok);
    assert!(
        stderr.contains("a test takes one parameter that it ignores: write `let test_add _ = …`"),
        "{stderr}"
    );
    let (ok, _, stderr) = pyfun_test(&dir, &["empty"]);
    let _ = fs::remove_dir_all(&dir);
    assert!(!ok);
    assert!(
        stderr.contains("a test file is named `test_*.pyfun`"),
        "{stderr}"
    );
}
