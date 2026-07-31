//! Tests for the `pyfun run` CLI command.
//!
//! These drive the built binary (cargo exposes its path via `CARGO_BIN_EXE_*`)
//! rather than the library, since `run` spawns a Python interpreter. The
//! execution cases are skipped (not failed) when no `python`/`python3` is on
//! PATH; the type-error case needs no interpreter and always runs.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

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
    // A well-typed program can still fail at runtime: the checker types division
    // but does not evaluate it, so this reaches Python and raises there.
    // (This case used to be a nested match with no arm for `Some None`, which
    // deep exhaustiveness now rejects at compile time — that is the *other*
    // test's job, so the runtime path needs an error types cannot see.)
    let file = write_temp(
        "runtime_error",
        "let divide a b = a / b\nlet boom = divide 1 0",
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
fn run_feeds_the_program_its_own_stdin() {
    if !have_python() {
        eprintln!("skipping `run` stdin test: no python interpreter found");
        return;
    }
    // `run` used to pipe the emitted source to `python -`, which spent the
    // program's stdin on its own source text: the first read raised `EOFError`
    // and no interactive program could be run by the tool that runs programs.
    let file = write_temp(
        "stdin",
        "extern readLine : string -> string = builtins.input\n\
         let main =\n\
         \x20   let name = readLine \"name? \"\n\
         \x20   print (f\"hi {name}\")\n\
         main",
    );
    let mut child = Command::new(pyfun_bin())
        .arg("run")
        .arg(&file)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn pyfun run");
    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(b"simon\n")
        .expect("write to the program's stdin");
    let output = child.wait_with_output().expect("wait for pyfun run");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success() && stdout.contains("hi simon"),
        "the program should read the line it was given, got:\n{stdout}"
    );
    let _ = std::fs::remove_file(&file);
}

#[test]
fn run_leaves_the_working_directory_alone() {
    if !have_python() {
        eprintln!("skipping `run` cwd test: no python interpreter found");
        return;
    }
    // The program is staged in a temp dir, but it runs *as if* invoked from the
    // command line, so a relative path in the program resolves against the
    // user's working directory rather than the staging dir.
    let dir = std::env::temp_dir().join(format!("pyfun_run_cwd_{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create working dir");
    std::fs::write(dir.join("data.txt"), "payload").expect("write data file");
    let file = write_temp(
        "cwd",
        "extern readFile : string -> string = pathlib.Path.read_text\n\
         extern pure path : string -> string = pathlib.Path\n\
         let main = print (readFile (path \"data.txt\"))\n\
         main",
    );
    let output = Command::new(pyfun_bin())
        .arg("run")
        .arg(&file)
        .current_dir(&dir)
        .output()
        .expect("spawn pyfun run");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success() && stdout.contains("payload"),
        "a relative path should resolve against the caller's cwd, got:\n{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let _ = std::fs::remove_file(&file);
    let _ = std::fs::remove_dir_all(&dir);
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
