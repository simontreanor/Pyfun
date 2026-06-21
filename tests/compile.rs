//! Phase 2 tests: lowering + Python emission.
//!
//! Two layers:
//! - String-level checks on the emitted Python (no interpreter needed).
//! - End-to-end execution: compile Pyfun, run the Python, assert on the result.
//!   These are skipped (not failed) when no `python`/`python3` is on PATH.

use std::io::Write;
use std::process::{Command, Stdio};

// ---------- string-level checks ----------

#[test]
fn curried_def_lowers_to_n_ary_def() {
    let py = pyfun::compile("let add a b = a + b").unwrap();
    assert!(py.contains("def add(a, b):"), "{py}");
    assert!(py.contains("return a + b"), "{py}");
}

#[test]
fn full_application_is_a_direct_call() {
    let py = pyfun::compile("let add a b = a + b\nlet r = add 1 2").unwrap();
    assert!(py.contains("r = add(1, 2)"), "{py}");
}

#[test]
fn partial_application_uses_functools_partial() {
    let py = pyfun::compile("let add a b = a + b\nlet inc = add 1").unwrap();
    assert!(py.starts_with("import functools\n"), "{py}");
    assert!(py.contains("inc = functools.partial(add, 1)"), "{py}");
}

#[test]
fn no_functools_import_when_unused() {
    let py = pyfun::compile("let add a b = a + b\nlet r = add 1 2").unwrap();
    assert!(!py.contains("import functools"), "{py}");
}

#[test]
fn pipe_becomes_application() {
    let py = pyfun::compile("let id x = x\nlet r = 5 |> id").unwrap();
    assert!(py.contains("r = id(5)"), "{py}");
}

#[test]
fn if_in_value_position_is_a_conditional_expression() {
    let py = pyfun::compile("let r = if true then 1 else 2").unwrap();
    assert!(py.contains("r = 1 if True else 2"), "{py}");
}

#[test]
fn exhaustive_match_without_wildcard_keeps_a_runtime_guard() {
    // Even a statically exhaustive ADT match emits a defensive `case _: raise`.
    let py = pyfun::compile("type Color = Red | Green | Blue\nlet f c = match c with | Red -> 1 | Green -> 2 | Blue -> 3").unwrap();
    assert!(py.contains("case _:"), "{py}");
    assert!(
        py.contains("raise RuntimeError(\"non-exhaustive match\")"),
        "{py}"
    );
}

#[test]
fn adt_lowers_to_classes_with_match_args() {
    let py = pyfun::compile("type Option a = None | Some a\nlet x = Some 1").unwrap();
    assert!(py.contains("class Some:"), "{py}");
    assert!(py.contains("__match_args__ = ('_0',)"), "{py}");
    // `None` is mangled to dodge the Python keyword, and is a nullary instance.
    assert!(py.contains("class None_:"), "{py}");
    assert!(py.contains("x = Some(1)"), "{py}");
}

#[test]
fn adt_classes_get_a_repr() {
    let py = pyfun::compile("type Option a = None | Some a\nlet x = Some 1").unwrap();
    assert!(py.contains("def __repr__(self):"), "{py}");
    // Nullary uses the bare class name; a field uses `!r`.
    assert!(py.contains("return \"None_\""), "{py}");
    assert!(py.contains("return f\"Some({self._0!r})\""), "{py}");
}

#[test]
fn adt_classes_get_structural_eq() {
    let py = pyfun::compile("type Option a = None | Some a\nlet x = Some 1").unwrap();
    assert!(py.contains("def __eq__(self, other):"), "{py}");
    assert!(
        py.contains("type(self) is type(other) and self.__dict__ == other.__dict__"),
        "{py}"
    );
}

#[test]
fn record_lowers_to_class_with_named_fields() {
    let py = pyfun::compile("type Point = { x: int, y: int }\nlet p = { y = 4, x = 3 }").unwrap();
    assert!(py.contains("class Point:"), "{py}");
    assert!(py.contains("__match_args__ = ('x', 'y')"), "{py}");
    assert!(py.contains("def __init__(self, x, y):"), "{py}");
    // The literal is reordered to the declared field order for a positional call.
    assert!(py.contains("p = Point(3, 4)"), "{py}");
}

#[test]
fn record_update_copies_through_a_temp() {
    let py = pyfun::compile(
        "type Point = { x: int, y: int }\nlet p = { x = 1, y = 2 }\nlet q = { p with x = 9 }",
    )
    .unwrap();
    // `p` is bound to a temp so it is evaluated once; the unchanged field is read
    // from it, the changed one is the new value.
    assert!(py.contains("q = Point(9, _pf_t0.y)"), "{py}");
}

#[test]
fn record_field_access_lowers_to_attribute() {
    let py = pyfun::compile("type Point = { x: int }\nlet p = { x = 1 }\nlet s = p.x").unwrap();
    assert!(py.contains("s = p.x"), "{py}");
}

#[test]
fn block_body_lowers_to_statement_sequence() {
    let py = pyfun::compile(
        "let sum3 a b c =\n    let mut acc = 0\n    acc <- acc + a\n    acc <- acc + b\n    acc",
    )
    .unwrap();
    assert!(py.contains("def sum3(a, b, c):"), "{py}");
    assert!(py.contains("    acc = 0"), "{py}");
    assert!(py.contains("    acc = acc + a"), "{py}");
    assert!(py.contains("    return acc"), "{py}");
}

#[test]
fn top_level_assignment_lowers_to_plain_assign() {
    let py = pyfun::compile("let mut x = 0\nx <- x + 1").unwrap();
    assert!(py.contains("x = 0"), "{py}");
    assert!(py.contains("x = x + 1"), "{py}");
    // No bare `None` line from the unit-valued assignment statement.
    assert!(!py.contains("\nNone"), "{py}");
}

#[test]
fn nested_local_let_lowers_to_nested_assignments() {
    let py = pyfun::compile(
        "let f x =\n    let y =\n        let mut t = x\n        t <- t + 1\n        t\n    y",
    )
    .unwrap();
    assert!(py.contains("def f(x):"), "{py}");
    assert!(py.contains("t = x"), "{py}");
    assert!(py.contains("t = t + 1"), "{py}");
    assert!(py.contains("y = t"), "{py}");
    assert!(py.contains("return y"), "{py}");
}

#[test]
fn comparison_operators_lower_to_python() {
    let py =
        pyfun::compile("let a = 1 < 2\nlet b = 1 == 2\nlet c = 1 != 2\nlet d = 1 >= 2").unwrap();
    assert!(py.contains("a = 1 < 2"), "{py}");
    assert!(py.contains("b = 1 == 2"), "{py}");
    assert!(py.contains("c = 1 != 2"), "{py}");
    assert!(py.contains("d = 1 >= 2"), "{py}");
}

#[test]
fn boolean_operators_lower_to_python_keywords_with_precedence() {
    // `and`/`or`/`not` lower to the same Python keywords. Precedence mirrors
    // Python, so no redundant parentheses, but looser operands under `not` get them.
    assert!(
        pyfun::compile("let r = true and false")
            .unwrap()
            .contains("r = True and False")
    );
    assert!(
        pyfun::compile("let r = true or false")
            .unwrap()
            .contains("r = True or False")
    );
    assert!(
        pyfun::compile("let r = not true")
            .unwrap()
            .contains("r = not True")
    );
    // `not (1 == 2)` needs no parens (not is looser than ==, as in Python).
    assert!(
        pyfun::compile("let r = not 1 == 2")
            .unwrap()
            .contains("r = not 1 == 2")
    );
    // `(not true) == false` does need them.
    assert!(
        pyfun::compile("let r = (not true) == false")
            .unwrap()
            .contains("r = (not True) == False")
    );
}

#[test]
fn prelude_partial_application_uses_partial() {
    // A partially applied builtin must close over its arg, not call `max(0)`.
    let py = pyfun::compile("let clamp0 = max 0").unwrap();
    assert!(py.contains("clamp0 = functools.partial(max, 0)"), "{py}");
}

#[test]
fn unknown_constructor_is_rejected() {
    let err = pyfun::compile("let f o = match o with | Some v -> v | None -> 0").unwrap_err();
    assert!(err.to_string().contains("unknown constructor"), "{err}");
}

// ---------- end-to-end execution ----------

#[test]
fn e2e_currying_full_partial_and_over_application() {
    run_and_check(
        "
        let add a b = a + b
        let inc = add 1
        let twice f x = f (f x)
        let r1 = add 1 2
        let r2 = inc 41
        let r3 = twice inc 5
        ",
        &[("r1", "3"), ("r2", "42"), ("r3", "7")],
    );
}

#[test]
fn e2e_pipe_and_composition() {
    run_and_check(
        "
        let add a b = a + b
        let inc = add 1
        let compose = fun f g x -> f (g x)
        let r = 4 |> inc |> inc
        let r2 = (compose inc inc) 10
        ",
        &[("r", "6"), ("r2", "12")],
    );
}

#[test]
fn e2e_if_and_match() {
    run_and_check(
        "
        let classify n =
          match n with
          | 0 -> \"zero\"
          | 1 -> \"one\"
          | _ -> \"many\"
        let r1 = classify 0
        let r2 = classify 7
        let r3 = if true then 10 else 20
        ",
        &[("r1", "zero"), ("r2", "many"), ("r3", "10")],
    );
}

#[test]
fn e2e_division_operators_match_python() {
    // `/` is true division (float), `//` floors (int) — like Python 3.
    run_and_check("let q = 7 / 2", &[("q", "3.5")]);
    run_and_check("let d = 7 // 2", &[("d", "3")]);
}

#[test]
fn division_operators_lower_to_matching_python_operators() {
    assert!(
        pyfun::compile("let q = 7 / 2")
            .unwrap()
            .contains("q = 7 / 2")
    );
    assert!(
        pyfun::compile("let d = 7 // 2")
            .unwrap()
            .contains("d = 7 // 2")
    );
}

#[test]
fn e2e_match_in_value_position_is_hoisted() {
    // The match must be evaluated into a temp, then added to 5.
    run_and_check(
        "let r = (match 1 with | 1 -> 10 | _ -> 20) + 5",
        &[("r", "15")],
    );
}

#[test]
fn e2e_adt_construction_and_match() {
    run_and_check(
        "
        type Option a = None | Some a
        type Color = Red | Green | Blue
        let unwrap o = match o with | Some v -> v | None -> 0
        let r1 = unwrap (Some 5)
        let r2 = unwrap None
        let rank c = match c with | Red -> 1 | Green -> 2 | Blue -> 3
        let r3 = rank Green
        ",
        &[("r1", "5"), ("r2", "0"), ("r3", "2")],
    );
}

#[test]
fn e2e_records_construct_access_and_update() {
    run_and_check(
        "
        type Point = { x: int, y: int }
        let p = { x = 3, y = 4 }
        let moved = { p with x = 10 }
        let sx = p.x
        let sy = moved.y
        let mx = moved.x
        let sumxy r = r.x + r.y
        let total = sumxy p
        ",
        &[("sx", "3"), ("sy", "4"), ("mx", "10"), ("total", "7")],
    );
}

#[test]
fn e2e_polymorphic_record_field() {
    run_and_check(
        "
        type Box a = { item: a }
        let mk v = { item = v }
        let i = (mk 42).item
        let s = (mk \"hi\").item
        ",
        &[("i", "42"), ("s", "hi")],
    );
}

#[test]
fn e2e_blocks_and_mutation() {
    run_and_check(
        "
        let sum3 a b c =
            let mut acc = 0
            acc <- acc + a
            acc <- acc + b
            acc <- acc + c
            acc
        let nested x =
            let y =
                let mut t = x
                t <- t * 2
                t
            y + 1
        let r1 = sum3 1 2 3
        let r2 = nested 10
        ",
        &[("r1", "6"), ("r2", "21")],
    );
}

#[test]
fn e2e_top_level_mutation() {
    run_and_check(
        "
        let mut counter = 0
        counter <- counter + 1
        counter <- counter + 5
        ",
        &[("counter", "6")],
    );
}

#[test]
fn e2e_recursive_adt() {
    // A cons-list: length via recursion-free folding isn't available, but nested
    // construction and matching exercise recursive types end to end.
    run_and_check(
        "
        type List a = Nil | Cons a (List a)
        let head d xs = match xs with | Nil -> d | Cons h t -> h
        let r = head 0 (Cons 7 (Cons 8 Nil))
        ",
        &[("r", "7")],
    );
}

#[test]
fn units_are_erased_in_emitted_python() {
    let py = pyfun::compile("measure m\nmeasure s\nlet speed = 100<m> / 10<s>").unwrap();
    assert!(!py.contains('<'), "units should be erased: {py}");
    assert!(py.contains("speed = 100 / 10"), "{py}");
}

#[test]
fn e2e_units_compute_after_erasure() {
    run_and_check(
        "
        measure m
        measure s
        let dist = 100<m>
        let time = 10<s>
        let speed = dist / time
        ",
        // `/` is true division, so the unit-bearing result is a float.
        &[("speed", "10.0")],
    );
}

#[test]
fn e2e_result_ce_binds_and_short_circuits() {
    // Extract the result via a match so the assertions compare plain ints.
    run_and_check(
        "
        let safe ok v = result { let! x = if ok then Ok v else Error 9  return (x + 1) }
        let unwrap r = match r with | Ok n -> n | Error e -> e
        let r1 = unwrap (safe true 10)
        let r2 = unwrap (safe false 10)
        ",
        &[("r1", "11"), ("r2", "9")],
    );
}

#[test]
fn e2e_seq_ce_produces_a_generator() {
    let Some(python) = python_cmd() else { return };
    let mut program =
        pyfun::compile("let xs = seq { yield 1  yield! (seq { yield 2  yield 3 }) }").unwrap();
    program.push_str("\nprint(list(xs))\n");
    assert_eq!(run_python(&python, &program).trim(), "[1, 2, 3]");
}

#[test]
fn e2e_async_ce_produces_a_coroutine() {
    let Some(python) = python_cmd() else { return };
    let mut program =
        pyfun::compile("let twice = async { let! x = async { return 21 }  return (x + x) }")
            .unwrap();
    program.push_str("\nimport asyncio\nprint(asyncio.run(twice))\n");
    assert_eq!(run_python(&python, &program).trim(), "42");
}

#[test]
fn e2e_prelude_print_and_numerics() {
    // Bare `print` statements (separated by the offside rule) and the numeric
    // builtins run end to end and produce observable output.
    let Some(python) = python_cmd() else {
        eprintln!("skipping end-to-end check: no python interpreter found");
        return;
    };
    let program = pyfun::compile(
        "type Option a = None | Some a\n\
         let a = 3\n\
         let b = 10\n\
         print (min a b)\n\
         print (max a b)\n\
         print (abs (a - b))\n\
         print (Some 7)",
    )
    .unwrap();
    let stdout = run_python(&python, &program);
    assert_eq!(
        stdout.lines().collect::<Vec<_>>(),
        ["3", "10", "7", "Some(7)"]
    );
}

#[test]
fn e2e_comparison_and_structural_equality() {
    let Some(python) = python_cmd() else {
        eprintln!("skipping end-to-end check: no python interpreter found");
        return;
    };
    let program = pyfun::compile(
        "type Option a = None | Some a\n\
         print (1 < 2)\n\
         print (\"a\" < \"b\")\n\
         print (3 == 3)\n\
         print (Some 1 == Some 1)\n\
         print (Some 1 == Some 2)",
    )
    .unwrap();
    let stdout = run_python(&python, &program);
    assert_eq!(
        stdout.lines().collect::<Vec<_>>(),
        ["True", "True", "True", "True", "False"]
    );
}

#[test]
fn e2e_boolean_logic() {
    let Some(python) = python_cmd() else {
        eprintln!("skipping end-to-end check: no python interpreter found");
        return;
    };
    let program = pyfun::compile(
        "let between lo hi x = lo <= x and x <= hi\n\
         print (true and not false)\n\
         print (1 < 2 or 5 < 3)\n\
         print (not (1 == 2))\n\
         print (between 0 10 5)\n\
         print (between 0 10 20)",
    )
    .unwrap();
    let stdout = run_python(&python, &program);
    assert_eq!(
        stdout.lines().collect::<Vec<_>>(),
        ["True", "True", "True", "True", "False"]
    );
}

// ---------- helpers ----------

/// Compile `source`, then run the emitted Python and assert that each named
/// top-level binding stringifies (`str(...)`) to the expected value.
fn run_and_check(source: &str, expected: &[(&str, &str)]) {
    let Some(python) = python_cmd() else {
        eprintln!("skipping end-to-end check: no python interpreter found");
        return;
    };
    let mut program =
        pyfun::compile(source).unwrap_or_else(|e| panic!("compile failed: {e}\n{source}"));
    program.push('\n');
    for (name, _) in expected {
        program.push_str(&format!("print({name})\n"));
    }
    let stdout = run_python(&python, &program);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(
        lines.len(),
        expected.len(),
        "unexpected output\nprogram:\n{program}\nstdout:\n{stdout}"
    );
    for (line, (name, want)) in lines.iter().zip(expected) {
        assert_eq!(line, want, "binding `{name}` mismatch\nprogram:\n{program}");
    }
}

/// The first available Python interpreter command, if any.
fn python_cmd() -> Option<String> {
    for candidate in ["python", "python3"] {
        if Command::new(candidate).arg("--version").output().is_ok() {
            return Some(candidate.to_string());
        }
    }
    None
}

/// Run `program` by piping it to `python -` and return its stdout. Panics if the
/// interpreter reports an error, so compile bugs surface as test failures.
fn run_python(python: &str, program: &str) -> String {
    let mut child = Command::new(python)
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn python");
    child
        .stdin
        .take()
        .expect("python stdin")
        .write_all(program.as_bytes())
        .expect("write program to python");
    let output = child.wait_with_output().expect("wait for python");
    assert!(
        output.status.success(),
        "python exited with {}\nprogram:\n{program}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("python stdout is utf-8")
}
