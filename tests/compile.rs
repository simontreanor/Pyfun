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
fn extern_lowers_to_its_python_target_with_import() {
    let py =
        pyfun::compile("extern pure sqrt: float -> float = math.sqrt\nlet r = sqrt 16.0").unwrap();
    assert!(py.contains("import math"), "{py}");
    assert!(py.contains("r = math.sqrt(16.0)"), "{py}");
}

#[test]
fn extern_with_name_equal_target_needs_no_import() {
    let py = pyfun::compile("extern show: a -> string = str\nlet r = show 42").unwrap();
    assert!(!py.contains("import"), "{py}");
    assert!(py.contains("r = str(42)"), "{py}");
}

#[test]
fn partial_application_of_extern_uses_functools_partial() {
    let py =
        pyfun::compile("extern pow: float -> float -> float = math.pow\nlet sq = pow 2.0").unwrap();
    assert!(py.contains("sq = functools.partial(math.pow, 2.0)"), "{py}");
}

#[test]
fn unused_extern_imports_nothing() {
    let py = pyfun::compile("extern pure sqrt: float -> float = math.sqrt\nlet r = 1").unwrap();
    assert!(!py.contains("import math"), "{py}");
}

#[test]
fn list_literal_lowers_to_a_python_list() {
    let py = pyfun::compile("let xs = [1, 2, 3]").unwrap();
    assert!(py.contains("xs = [1, 2, 3]"), "{py}");
}

#[test]
fn map_filter_lower_to_emitted_helpers() {
    let py = pyfun::compile("let r = List.map (fun x -> x) (List.filter (fun x -> true) [1, 2])")
        .unwrap();
    assert!(py.contains("def _pf_map(f, xs):"), "{py}");
    assert!(py.contains("return list(map(f, xs))"), "{py}");
    assert!(py.contains("def _pf_filter(f, xs):"), "{py}");
    assert!(py.contains("return list(filter(f, xs))"), "{py}");
}

#[test]
fn len_and_sum_lower_to_python_builtins_without_helpers() {
    let py = pyfun::compile("let n = List.len [1, 2]\nlet s = List.sum [1, 2]").unwrap();
    assert!(py.contains("n = len([1, 2])"), "{py}");
    assert!(py.contains("s = sum([1, 2])"), "{py}");
    assert!(!py.contains("_pf_"), "no helpers needed: {py}");
}

#[test]
fn fold_lowers_to_functools_reduce() {
    let py = pyfun::compile("let t = List.fold (fun a b -> a + b) 0 [1, 2, 3]").unwrap();
    assert!(py.starts_with("import functools\n"), "{py}");
    assert!(py.contains("def _pf_fold(f, acc, xs):"), "{py}");
    assert!(py.contains("return functools.reduce(f, xs, acc)"), "{py}");
}

#[test]
fn unused_list_helpers_are_not_emitted() {
    let py = pyfun::compile("let xs = [1, 2, 3]").unwrap();
    assert!(!py.contains("_pf_"), "{py}");
}

#[test]
fn pipe_becomes_application() {
    let py = pyfun::compile("let id x = x\nlet r = 5 |> id").unwrap();
    assert!(py.contains("r = id(5)"), "{py}");
}

#[test]
fn empty_collections_lower_to_set_and_dict() {
    let py = pyfun::compile("let s = Set.empty\nlet m = Map.empty").unwrap();
    assert!(py.contains("s = set()"), "{py}");
    assert!(py.contains("m = dict()"), "{py}");
}

#[test]
fn set_functions_lower_to_emitted_helpers() {
    let py = pyfun::compile("let r = Set.contains 1 (Set.add 1 (Set.ofList [2]))").unwrap();
    assert!(py.contains("def _pf_set_add(x, s):"), "{py}");
    assert!(py.contains("return s.union([x])"), "{py}");
    assert!(py.contains("def _pf_set_contains(x, s):"), "{py}");
    assert!(py.contains("return x in s"), "{py}");
    // `Set.ofList` is a bare builtin, no helper.
    assert!(py.contains("set([2])"), "{py}");
}

#[test]
fn map_add_and_find_or_lower_to_emitted_helpers() {
    let py =
        pyfun::compile("let m = Map.add \"a\" 1 Map.empty\nlet v = Map.findOr \"a\" 0 m").unwrap();
    assert!(py.contains("def _pf_map_add(k, v, m):"), "{py}");
    assert!(
        py.contains("return dict(list(m.items()) + [[k, v]])"),
        "{py}"
    );
    assert!(py.contains("def _pf_map_find_or(k, default, m):"), "{py}");
    assert!(py.contains("return m.get(k, default)"), "{py}");
}

#[test]
fn map_remove_lowers_to_a_copy_and_pop() {
    let py = pyfun::compile("let m = Map.remove \"a\" Map.empty").unwrap();
    assert!(py.contains("def _pf_map_remove(k, m):"), "{py}");
    assert!(py.contains("r = dict(m)"), "{py}");
    assert!(py.contains("r.pop(k, None)"), "{py}");
    assert!(py.contains("return r"), "{py}");
}

#[test]
fn try_find_lowers_to_an_option_and_pulls_in_the_option_prelude() {
    let py = pyfun::compile("let v = Map.tryFind \"a\" Map.empty").unwrap();
    assert!(py.contains("class Some:"), "{py}");
    assert!(py.contains("class None_:"), "{py}");
    assert!(py.contains("def _pf_map_try_find(k, m):"), "{py}");
    assert!(py.contains("return Some(m.get(k))"), "{py}");
    assert!(py.contains("return None_()"), "{py}");
}

#[test]
fn unused_collection_helpers_are_not_emitted() {
    let py = pyfun::compile("let s = Set.empty").unwrap();
    assert!(!py.contains("_pf_"), "{py}");
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
    // A user-defined ADT (Option/Some/None are now built-in, so use a fresh type).
    let py = pyfun::compile("type Opt a = Empty | Has a\nlet x = Has 1").unwrap();
    assert!(py.contains("class Has:"), "{py}");
    assert!(py.contains("__match_args__ = ('_0',)"), "{py}");
    assert!(py.contains("class Empty:"), "{py}");
    assert!(py.contains("x = Has(1)"), "{py}");
}

#[test]
fn adt_classes_get_a_repr() {
    let py = pyfun::compile("type Opt a = Empty | Has a\nlet x = Has 1").unwrap();
    assert!(py.contains("def __repr__(self):"), "{py}");
    // Nullary uses the bare class name; a field uses `!r`.
    assert!(py.contains("return \"Empty\""), "{py}");
    assert!(py.contains("return f\"Has({self._0!r})\""), "{py}");
}

#[test]
fn adt_classes_get_structural_eq() {
    let py = pyfun::compile("type Opt a = Empty | Has a\nlet x = Has 1").unwrap();
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
fn pure_modifier_is_erased_at_lowering() {
    // Effects (and the `pure` assertion) leave no runtime residue.
    let py = pyfun::compile("let pure add a b = a + b\nlet r = add 1 2").unwrap();
    assert!(py.contains("def add(a, b):"), "{py}");
    assert!(!py.contains("pure"), "{py}");
    assert!(!py.contains("io"), "{py}");
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
    let err = pyfun::compile("let f o = match o with | Nope v -> v").unwrap_err();
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
        // `List` is now a built-in collection type, so this cons-list ADT uses a
        // distinct name (`Lst`).
        "
        type Lst a = Nil | Cons a (Lst a)
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
        "let a = 3\n\
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
        "print (1 < 2)\n\
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

#[test]
fn e2e_extern_calls_python() {
    run_and_check(
        "extern show: a -> string = str\n\
         extern ord: string -> int\n\
         extern pure sqrt: float -> float = math.sqrt\n\
         let label = show 42\n\
         let code = ord \"A\"\n\
         let root = sqrt 16.0",
        &[("label", "42"), ("code", "65"), ("root", "4.0")],
    );
}

#[test]
fn e2e_list_functions() {
    run_and_check(
        "let xs = [1, 2, 3, 4]\n\
         let doubled = List.map (fun x -> x * 2) xs\n\
         let small = List.filter (fun x -> x < 3) xs\n\
         let total = List.fold (fun a b -> a + b) 0 xs\n\
         let n = List.len xs\n\
         let s = List.sum xs\n\
         let r = List.rev xs\n\
         let q = List.range 1 4",
        &[
            ("doubled", "[2, 4, 6, 8]"),
            ("small", "[1, 2]"),
            ("total", "10"),
            ("n", "4"),
            ("s", "10"),
            ("r", "[4, 3, 2, 1]"),
            ("q", "[1, 2, 3]"),
        ],
    );
}

#[test]
fn e2e_set_functions() {
    run_and_check(
        "let s = Set.ofList [1, 2, 3, 3]\n\
         let n = Set.len s\n\
         let has = Set.contains 2 s\n\
         let u = Set.len (Set.union s (Set.ofList [3, 4]))\n\
         let i = Set.len (Set.intersect s (Set.ofList [3, 4]))\n\
         let d = Set.len (Set.difference s (Set.ofList [3, 4]))\n\
         let e = Set.len Set.empty",
        &[
            ("n", "3"),
            ("has", "True"),
            ("u", "4"),
            ("i", "1"),
            ("d", "2"),
            ("e", "0"),
        ],
    );
}

#[test]
fn e2e_map_functions() {
    run_and_check(
        "let m = Map.add \"a\" 1 (Map.add \"b\" 2 Map.empty)\n\
         let found = Map.findOr \"a\" 0 m\n\
         let dflt = Map.findOr \"z\" 99 m\n\
         let sz = Map.len m\n\
         let mc = Map.contains \"b\" m\n\
         let rm = Map.len (Map.remove \"b\" m)\n\
         let ks = List.len (Map.keys m)\n\
         let hit = Option.withDefault 0 (Map.tryFind \"a\" m)\n\
         let miss = Option.withDefault 0 (Map.tryFind \"z\" m)",
        &[
            ("found", "1"),
            ("dflt", "99"),
            ("sz", "2"),
            ("mc", "True"),
            ("rm", "1"),
            ("ks", "2"),
            ("hit", "1"),
            ("miss", "0"),
        ],
    );
}

#[test]
fn e2e_option_module() {
    run_and_check(
        "let some = Some 5\n\
         let none = None\n\
         let a = Option.withDefault 0 some\n\
         let b = Option.withDefault 0 none\n\
         let c = Option.isSome some\n\
         let d = Option.isNone none\n\
         let e = Option.map (fun x -> x + 1) some",
        &[
            ("a", "5"),
            ("b", "0"),
            ("c", "True"),
            ("d", "True"),
            ("e", "Some(6)"),
        ],
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
