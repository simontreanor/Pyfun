//! Tests for the `pyfun repl` command. These drive the built binary with piped
//! stdin (the REPL spawns Python to evaluate expressions), so the eval cases skip
//! when no `python`/`python3` is on PATH; type-echo and error cases always run.

use std::io::Write;
use std::process::{Command, Stdio};

fn pyfun_bin() -> &'static str {
    env!("CARGO_BIN_EXE_pyfun")
}

fn have_python() -> bool {
    ["python", "python3"]
        .iter()
        .any(|p| Command::new(p).arg("--version").output().is_ok())
}

/// Run the REPL feeding `input` on stdin, returning combined stdout+stderr.
fn repl(input: &str) -> String {
    let mut child = Command::new(pyfun_bin())
        .arg("repl")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn pyfun repl");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let out = child.wait_with_output().expect("repl finished");
    let mut s = String::from_utf8_lossy(&out.stdout).into_owned();
    s.push_str(&String::from_utf8_lossy(&out.stderr));
    s
}

#[test]
fn repl_echoes_definition_types() {
    // A definition is remembered and its inferred type echoed (no Python needed).
    let out = repl("let double = (*) 2\n:type double\n:quit\n");
    assert!(out.contains("double : int"), "definition type echo:\n{out}");
}

#[test]
fn repl_reports_type_errors_without_committing() {
    // A type error is shown and the bad binding is not remembered.
    let out = repl("let x = 5\nlet y = x + true\ny\n:quit\n");
    assert!(out.contains("found bool"), "type error shown:\n{out}");
    assert!(out.contains("unbound name `y`"), "y not committed:\n{out}");
}

#[test]
fn repl_evaluates_expressions() {
    if !have_python() {
        eprintln!("skipping repl eval test: no python interpreter");
        return;
    }
    let out = repl("let double = (*) 2\ndouble 21\n1 + 2 * 3\n:quit\n");
    assert!(out.contains("42"), "double 21:\n{out}");
    assert!(out.contains('7'), "1 + 2 * 3:\n{out}");
}

#[test]
fn repl_multiline_block_enables_mutual_recursion() {
    if !have_python() {
        eprintln!("skipping repl multiline test: no python interpreter");
        return;
    }
    // Both mutually-recursive functions are entered in one `:{ … :}` block, so
    // they type-check as a group; then `isEven 10` evaluates to True.
    let out = repl(
        ":{\n\
         let isEven n = if n == 0 then true else isOdd (n - 1)\n\
         let isOdd n = if n == 0 then false else isEven (n - 1)\n\
         :}\n\
         isEven 10\n\
         :quit\n",
    );
    assert!(
        out.contains("isEven : int -> bool"),
        "grouped types:\n{out}"
    );
    assert!(out.contains("True"), "isEven 10:\n{out}");
}

#[test]
fn repl_unit_expression_prints_no_none() {
    if !have_python() {
        eprintln!("skipping repl unit test: no python interpreter");
        return;
    }
    // A `unit`-typed expression runs its effect but does not print a `None` value.
    let out = repl("print \"hello repl\"\n:quit\n");
    assert!(out.contains("hello repl"), "effect ran:\n{out}");
    assert!(!out.contains("None"), "no None printed:\n{out}");
}
