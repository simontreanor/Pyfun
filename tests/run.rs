//! Tests for the `pyfun run` CLI command.
//!
//! These drive the built binary (cargo exposes its path via `CARGO_BIN_EXE_*`)
//! rather than the library, since `run` spawns a Python interpreter. The
//! execution cases are skipped (not failed) when no `python`/`python3` is on
//! PATH; the type-error case needs no interpreter and always runs.

use std::path::PathBuf;
use std::process::Command;

/// Path to the freshly-built `pyfun` binary for this test run.
fn pyfun_bin() -> &'static str {
    env!("CARGO_BIN_EXE_pyfun")
}

/// Whether a Python interpreter is available to execute emitted code.
fn have_python() -> bool {
    ["python", "python3"]
        .iter()
        .any(|p| Command::new(p).arg("--version").output().is_ok())
}

/// Write `source` to a uniquely-named temp `.pyfun` file and return its path.
fn write_temp(name: &str, source: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("pyfun_run_{name}.pyfun"));
    std::fs::write(&path, source).expect("write temp pyfun file");
    path
}

#[test]
fn run_executes_a_valid_program() {
    if !have_python() {
        eprintln!("skipping `run` execution test: no python interpreter found");
        return;
    }
    // No prelude/`print` yet, so a valid program runs silently and exits 0.
    let file = write_temp(
        "valid",
        "let add a b = a + b\nlet r = add 1 2\nlet x = Some r",
    );
    let status = Command::new(pyfun_bin())
        .arg("run")
        .arg(&file)
        .status()
        .expect("spawn pyfun run");
    assert!(status.success(), "valid program should run cleanly");
    let _ = std::fs::remove_file(&file);
}

#[test]
fn run_propagates_a_python_runtime_error() {
    if !have_python() {
        eprintln!("skipping `run` runtime-error test: no python interpreter found");
        return;
    }
    // A nested match the shallow checker accepts but that has no arm for
    // `Some None` hits the emitted runtime guard, so Python exits non-zero.
    let file = write_temp(
        "runtime_error",
        "let deep o = match o with | Some (Some x) -> x | None -> 0\n\
         let boom = deep (Some None)",
    );
    let status = Command::new(pyfun_bin())
        .arg("run")
        .arg(&file)
        .status()
        .expect("spawn pyfun run");
    assert!(
        !status.success(),
        "an uncaught runtime error must make `run` exit non-zero"
    );
    let _ = std::fs::remove_file(&file);
}

#[test]
fn run_refuses_to_execute_ill_typed_code() {
    // The compiler is the gatekeeper: a type error stops `run` before any Python
    // executes. Needs no interpreter, so this always runs.
    let file = write_temp("ill_typed", "let add a b = a + b\nlet r = add 1 true");
    let output = Command::new(pyfun_bin())
        .arg("run")
        .arg(&file)
        .output()
        .expect("spawn pyfun run");
    assert!(!output.status.success(), "ill-typed code must not run");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("expected int, found bool"),
        "expected a type diagnostic, got:\n{stderr}"
    );
    let _ = std::fs::remove_file(&file);
}
