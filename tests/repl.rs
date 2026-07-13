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

// ---- persistent-worker semantics (one long-lived Python process per session) ----

#[test]
fn repl_definition_effect_runs_once_at_entry() {
    if !have_python() {
        eprintln!("skipping repl persistent test: no python interpreter");
        return;
    }
    // An effectful definition runs exactly once, when entered — later expression
    // entries must not re-run it (the old model re-ran all defs on every eval).
    let out = repl("let g = print \"boot\"\n1 + 1\n2 + 2\n:quit\n");
    assert_eq!(
        out.matches("boot").count(),
        1,
        "definition effect ran exactly once:\n{out}"
    );
    assert!(out.contains('2'), "1 + 1 evaluated:\n{out}");
    assert!(out.contains('4'), "2 + 2 evaluated:\n{out}");
}

#[test]
fn repl_top_level_mut_state_persists_across_entries() {
    if !have_python() {
        eprintln!("skipping repl persistent test: no python interpreter");
        return;
    }
    // Top-level `let mut` state carries across entries: two reassignment entries
    // accumulate, and a later expression observes the result.
    let out = repl("let mut n = 0\nn <- n + 1\nn <- n + 1\nn * 10\n:quit\n");
    assert!(out.contains("20"), "n reached 2, n * 10 is 20:\n{out}");
}

#[test]
fn repl_pure_definitions_still_evaluate() {
    if !have_python() {
        eprintln!("skipping repl persistent test: no python interpreter");
        return;
    }
    let out = repl("let inc x = x + 1\ninc 41\n:quit\n");
    assert!(out.contains("inc : int -> int"), "type echo:\n{out}");
    assert!(out.contains("42"), "inc 41:\n{out}");
}

#[test]
fn repl_reentered_expression_runs_again() {
    if !have_python() {
        eprintln!("skipping repl persistent test: no python interpreter");
        return;
    }
    // An expression is one-shot: entering it twice runs (and prints) it twice.
    let out = repl("let x = 20\nx + 22\nx + 22\n:quit\n");
    assert_eq!(
        out.matches("42").count(),
        2,
        "expression re-ran on re-entry:\n{out}"
    );
}

#[test]
fn repl_redefining_a_function_changes_behavior() {
    if !have_python() {
        eprintln!("skipping repl persistent test: no python interpreter");
        return;
    }
    // A redefinition emits a different chunk, which rebinds the name in the worker.
    let out = repl("let f x = x + 1\nf 5\nlet f x = x * 10\nf 5\n:quit\n");
    assert!(out.contains('6'), "original f 5 = 6:\n{out}");
    assert!(out.contains("50"), "redefined f 5 = 50:\n{out}");
}

#[test]
fn repl_reset_forgets_definitions() {
    if !have_python() {
        eprintln!("skipping repl persistent test: no python interpreter");
        return;
    }
    let out = repl("let inc x = x + 1\n:reset\ninc 1\n:quit\n");
    assert!(out.contains("(session reset)"), "reset confirmed:\n{out}");
    assert!(
        out.contains("unbound name `inc`"),
        "inc forgotten after :reset:\n{out}"
    );
}

#[test]
fn repl_rejected_definition_does_not_poison_session() {
    if !have_python() {
        eprintln!("skipping repl persistent test: no python interpreter");
        return;
    }
    // An ill-typed definition is rejected outright; the session keeps working.
    let out = repl("let bad = 1 + true\nlet ok = 2\nok + 40\n:quit\n");
    assert!(out.contains("found bool"), "type error shown:\n{out}");
    assert!(out.contains("42"), "session still evaluates:\n{out}");
}

#[test]
fn repl_adt_round_trips_across_entries() {
    if !have_python() {
        eprintln!("skipping repl persistent test: no python interpreter");
        return;
    }
    // A value stored under an ADT class in one entry must still match its
    // constructor pattern in a later entry — i.e. the class is exec'd once and
    // never resent (a re-exec'd class would break `isinstance` identity).
    let out = repl(
        "type Color = Red | Green\n\
         let c = Red\n\
         :{\n\
         match c:\n\
         \x20   case Red: print \"red branch\"\n\
         \x20   case Green: print \"green branch\"\n\
         :}\n\
         :quit\n",
    );
    assert!(out.contains("red branch"), "Red matched:\n{out}");
    assert!(
        !out.contains("non-exhaustive"),
        "class identity stable (match did not fall through):\n{out}"
    );
    assert!(
        !out.contains("green branch"),
        "only the Red arm ran:\n{out}"
    );
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
