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
fn unit_literal_lowers_to_none() {
    let py = pyfun::compile("let nothing = ()").unwrap();
    assert!(py.contains("nothing = None"), "{py}");
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
fn list_zip_lowers_to_a_zip_helper() {
    let py = pyfun::compile("let ps = List.zip [1, 2] [3, 4]").unwrap();
    assert!(py.contains("def _pf_zip(xs, ys):"), "{py}");
    assert!(py.contains("return list(zip(xs, ys))"), "{py}");
}

#[test]
fn map_of_list_lowers_to_dict_and_to_list_to_items() {
    // `Map.ofList` is a bare `dict` over the pair list; `Map.toList` is a helper.
    let py = pyfun::compile("let m = Map.ofList [(1, 2)]\nlet ps = Map.toList m").unwrap();
    assert!(py.contains("m = dict([(1, 2)])"), "{py}");
    assert!(py.contains("def _pf_map_to_list(m):"), "{py}");
    assert!(py.contains("return list(m.items())"), "{py}");
}

#[test]
fn e2e_zip_into_a_map_and_back() {
    run_and_check(
        "
        let m = Map.ofList (List.zip [\"a\", \"b\"] [1, 2])
        let a = Option.withDefault 0 (Map.tryFind \"a\" m)
        let n = Map.len m
        let pairs = Map.toList m
        ",
        &[("a", "1"), ("n", "2"), ("pairs", "[('a', 1), ('b', 2)]")],
    );
}

#[test]
fn try_lowers_to_a_try_except_yielding_ok_or_error() {
    let py = pyfun::compile(
        "extern parseInt : string -> int = int\n\
         let safe s = try (parseInt s)",
    )
    .unwrap();
    // The Exception record is emitted as `_Exception` (not shadowing the builtin).
    assert!(py.contains("class _Exception:"), "{py}");
    assert!(py.contains("try:"), "{py}");
    assert!(py.contains("= Ok(int(s))"), "{py}");
    assert!(py.contains("except Exception as"), "{py}");
    // The handler builds Error(_Exception(type(e).__name__, str(e))).
    assert!(py.contains("_Exception(type("), "{py}");
    assert!(py.contains(").__name__, str("), "{py}");
}

#[test]
fn e2e_try_catches_and_recovers() {
    run_and_check(
        "
        extern parseInt : string -> int = int
        let orZero s = Result.withDefault 0 (try (parseInt s))
        let good = orZero \"42\"
        let bad = orZero \"oops\"
        ",
        &[("good", "42"), ("bad", "0")],
    );
}

#[test]
fn e2e_try_exposes_exception_kind_and_message() {
    run_and_check(
        "
        extern parseInt : string -> int = int
        let kindOf s =
          match try (parseInt s):
            case Ok n: \"ok\"
            case Error e: e.errorKind
        let k = kindOf \"nope\"
        let matched =
          match try (parseInt \"x\"):
            case Error (Exception { errorKind = \"ValueError\" }): \"caught-value-error\"
            case _: \"other\"
        ",
        &[("k", "ValueError"), ("matched", "caught-value-error")],
    );
}

#[test]
fn interpolated_string_lowers_to_a_python_fstring() {
    let py = pyfun::compile(
        "let name = \"Ada\"\n\
         let a = 3\n\
         let b = 4\n\
         let g = f\"hi {name}: {a + b}\"\n\
         let u = f\"upper {String.upper name}\"\n\
         let e = f\"brace {{ {a}\"",
    )
    .unwrap();
    assert!(py.contains("g = f\"hi {name}: {a + b}\""), "{py}");
    // A module member in a hole lowers to its helper call, inside the f-string.
    assert!(py.contains("u = f\"upper {_pf_str_upper(name)}\""), "{py}");
    // `{{` stays a literal brace in the emitted f-string.
    assert!(py.contains("e = f\"brace {{ {a}\""), "{py}");
}

#[test]
fn e2e_interpolated_strings() {
    run_and_check(
        "
        let name = \"Ada\"
        let a = 3
        let b = 4
        let p = (1, 2)
        let greet = f\"Hello, {name}!\"
        let math = f\"{a} + {b} = {a + b}\"
        let up = f\"upper: {String.upper name}\"
        let brace = f\"{{literal}} {a}\"
        let point = f\"point {p}\"
        ",
        &[
            ("greet", "Hello, Ada!"),
            ("math", "3 + 4 = 7"),
            ("up", "upper: ADA"),
            ("brace", "{literal} 3"),
            ("point", "point (1, 2)"),
        ],
    );
}

#[test]
fn debug_hole_lowers_to_an_echoed_literal_plus_hole() {
    // `{x=}` resolves at lex time to a literal chunk (the raw hole text incl. the
    // `=`) followed by an ordinary hole, so the emitted Python is `f"x={x}"`.
    let py = pyfun::compile(
        "let x = 3\n\
         let y = 4\n\
         let a = f\"{x=}\"\n\
         let b = f\"{x = }\"\n\
         let c = f\"sum {x + y=} end\"\n\
         let d = f\"{x==y}\"",
    )
    .unwrap();
    assert!(py.contains("a = f\"x={x}\""), "{py}");
    // Whitespace around the marker is echoed verbatim.
    assert!(py.contains("b = f\"x = {x}\""), "{py}");
    assert!(py.contains("c = f\"sum x + y={x + y} end\""), "{py}");
    // A trailing `==` is a comparison, not a debug marker: nothing is echoed.
    assert!(py.contains("d = f\"{x == y}\""), "{py}");
}

#[test]
fn list_completeness_ops_lower_to_helpers() {
    let py = pyfun::compile(
        "let a = List.get 0 [1, 2]\n\
         let b = List.isEmpty [1]\n\
         let c = List.contains 2 [1, 2]\n\
         let d = List.concat [1] [2]\n\
         let e = List.sort [2, 1]",
    )
    .unwrap();
    // get is total (bounds-checked, no raw IndexError) and yields Some/None.
    assert!(py.contains("Some(xs[i]) if 0 <= i < len(xs) else None_()"), "{py}");
    assert!(py.contains("len(xs) == 0"), "{py}"); // isEmpty
    assert!(py.contains("return x in xs"), "{py}"); // contains
    assert!(py.contains("return xs + ys"), "{py}"); // concat
    assert!(py.contains("return sorted(xs)"), "{py}"); // sort
}

#[test]
fn e2e_list_completeness_ops() {
    run_and_check(
        "
        let xs = [3, 1, 2]
        let a = Option.withDefault 0 (List.get 0 xs)
        let b = Option.withDefault 0 (List.get 9 xs)
        let c = List.contains 2 xs
        let d = List.concat xs [4]
        let e = List.sort xs
        let f = Option.withDefault 0 (List.find (fun x -> x > 1) xs)
        let g = List.isEmpty []
        ",
        &[
            ("a", "3"),
            ("b", "0"),
            ("c", "True"),
            ("d", "[3, 1, 2, 4]"),
            ("e", "[1, 2, 3]"),
            ("f", "3"),
            ("g", "True"),
        ],
    );
}

#[test]
fn modulo_lowers_to_python_percent() {
    let py = pyfun::compile("let r = 10 % 3\nlet even n = n % 2 == 0").unwrap();
    assert!(py.contains("r = 10 % 3"), "{py}");
    assert!(py.contains("return n % 2 == 0"), "{py}");
}

#[test]
fn e2e_modulo() {
    run_and_check(
        "
        let a = 10 % 3
        let b = 5.5 % 2.0
        let c = -7 % 3
        let d = (%) 17 5
        ",
        // `-7 % 3 == 2` (Python modulo takes the divisor's sign).
        &[("a", "1"), ("b", "1.5"), ("c", "2"), ("d", "2")],
    );
}

#[test]
fn non_ascii_string_is_not_double_encoded() {
    // The emitted Python must contain the real characters, not the mojibake a
    // per-byte `b as char` produced (`café` -> `cafÃ©`).
    let py = pyfun::compile("let s = \"café → 🎉\"").unwrap();
    assert!(py.contains("s = \"café → 🎉\""), "{py}");
    assert!(!py.contains("Ã"), "double-encoded output: {py}");
}

#[test]
fn e2e_non_ascii_string_length() {
    // Encoding-independent (output is a plain integer, no console-encoding
    // dependency): a correctly-lexed "café" has 4 characters; the old per-byte bug
    // gave 5+. Also covers a multi-byte f-string literal chunk.
    run_and_check(
        "
        let n = String.len \"café\"
        let m = String.len f\"→🎉 {n}\"
        ",
        &[("n", "4"), ("m", "4")],
    );
}

#[test]
fn chained_comparison_lowers_to_native_python_form() {
    // Lowering to Python's own `a < b < c` is what gives evaluate-once +
    // short-circuit for free — not a desugaring to `x < y and y < z`.
    let py = pyfun::compile("let f x = 1 < x < 10\nlet g = 1 == 1 == 1").unwrap();
    assert!(py.contains("return 1 < x < 10"), "{py}");
    assert!(py.contains("g = 1 == 1 == 1"), "{py}");
    assert!(!py.contains("and"), "should not desugar to `and`: {py}");
}

#[test]
fn e2e_chained_comparisons() {
    run_and_check(
        "
        let a = 1 < 2 < 3
        let b = 3 < 2 < 1
        let c =
            let x = 5
            1 < x < 10
        let d = 1 <= 1 < 2
        ",
        &[("a", "True"), ("b", "False"), ("c", "True"), ("d", "True")],
    );
}

#[test]
fn unary_minus_lowers_with_python_precedence() {
    let py = pyfun::compile(
        "let a = -5\n\
         let b = 2 * -3\n\
         let c = -(4 + 1)\n\
         let d = 0 - -7",
    )
    .unwrap();
    assert!(py.contains("a = -5"), "{py}");
    // Unary minus binds tighter than `*`, so no parens are needed around `-3`.
    assert!(py.contains("b = 2 * -3"), "{py}");
    // A looser `+` operand is parenthesized.
    assert!(py.contains("c = -(4 + 1)"), "{py}");
    assert!(py.contains("d = 0 - -7"), "{py}");
}

#[test]
fn e2e_unary_minus() {
    run_and_check(
        "
        let a = -5
        let b = abs (-5)
        let c = 2 * -3
        let d = -(4 + 1)
        let sign =
            match -1:
                case -1: \"neg\"
                case _: \"other\"
        ",
        &[("a", "-5"), ("b", "5"), ("c", "-6"), ("d", "-5"), ("sign", "neg")],
    );
}

#[test]
fn operator_section_lowers_to_a_curried_lambda() {
    // `(*)` lowers to the binary lambda; a partial application curries via
    // `functools.partial` (arity 2 is known), like any 2-arity function.
    let py = pyfun::compile("let mul = (*)\nlet double = (*) 2").unwrap();
    assert!(py.contains("mul = lambda a, b: a * b"), "{py}");
    assert!(
        py.contains("double = functools.partial(lambda a, b: a * b, 2)"),
        "{py}"
    );
}

#[test]
fn e2e_operator_sections() {
    run_and_check(
        "
        let mul = (*)
        let double = (*) 2
        let total = List.fold (+) 0 [1, 2, 3, 4]
        let cmp = (<) 2 3
        let a = mul 3 4
        let b = double 5
        ",
        &[("a", "12"), ("b", "10"), ("total", "10"), ("cmp", "True")],
    );
}

#[test]
fn fstring_is_an_application_argument() {
    // An f-string juxtaposed as a call argument (`print f"..."`, no parens) is a
    // single application, not two statements — `starts_atom` must accept `FStr`.
    let py = pyfun::compile("let x = 5\nlet main = print f\"x is {x}\"").unwrap();
    assert!(py.contains("main = print(f\"x is {x}\")"), "{py}");
}

#[test]
fn e2e_interpolation_debug_holes() {
    run_and_check(
        "
        let x = 3
        let y = 4
        let a = f\"{x=}\"
        let b = f\"{x = }\"
        let c = f\"{x + y=}\"
        let d = f\"{x == y}\"
        let e = f\"{x <= y}\"
        ",
        &[
            ("a", "x=3"),
            ("b", "x = 3"),
            ("c", "x + y=7"),
            ("d", "False"),
            ("e", "True"),
        ],
    );
}

#[test]
fn e2e_interpolation_hole_with_nested_string() {
    // A hole may contain a string literal (with its own quotes/braces); on the
    // Python 3.12+ target these reuse the outer quote cleanly.
    run_and_check(
        "
        let s = \"a}b\"
        let r = f\"contains: {String.contains \"}\" s}\"
        ",
        &[("r", "contains: True")],
    );
}

#[test]
fn string_functions_lower_to_builtins_and_helpers() {
    let py = pyfun::compile(
        "let n = String.len \"hi\"\n\
         let up = String.upper \"hi\"\n\
         let parts = String.split \",\" \"a,b\"\n\
         let joined = String.join \"-\" parts\n\
         let has = String.contains \"a\" \"abc\"\n\
         let s = String.fromInt 7\n\
         let chars = String.toList \"hi\"",
    )
    .unwrap();
    // Bare-builtin routes (no helper).
    assert!(py.contains("len(\"hi\")"), "{py}");
    assert!(py.contains("str(7)"), "{py}");
    assert!(py.contains("list(\"hi\")"), "{py}");
    // Emitted helpers.
    assert!(py.contains("def _pf_str_upper(s):"), "{py}");
    assert!(py.contains("return s.upper()"), "{py}");
    assert!(py.contains("def _pf_str_split(sep, s):"), "{py}");
    assert!(py.contains("return s.split(sep)"), "{py}");
    assert!(py.contains("def _pf_str_join(sep, xs):"), "{py}");
    assert!(py.contains("return sep.join(xs)"), "{py}");
    assert!(py.contains("def _pf_str_contains(sub, s):"), "{py}");
    assert!(py.contains("return sub in s"), "{py}");
}

#[test]
fn string_to_int_lowers_to_a_try_except_and_pulls_in_the_option_prelude() {
    let py = pyfun::compile("let r = String.toInt \"42\"").unwrap();
    assert!(py.contains("class Some:"), "{py}");
    assert!(py.contains("class None_:"), "{py}");
    assert!(py.contains("def _pf_str_to_int(s):"), "{py}");
    assert!(py.contains("try:"), "{py}");
    assert!(py.contains("return Some(int(s))"), "{py}");
    assert!(py.contains("except ValueError:"), "{py}");
    assert!(py.contains("return None_()"), "{py}");
}

#[test]
fn e2e_string_operations() {
    run_and_check(
        "
        let g = String.concat \"Hello, \" \"World\"
        let up = String.upper g
        let n = String.len g
        let parts = String.split \", \" g
        let joined = String.join \"/\" parts
        let has = String.contains \"World\" g
        let r = String.replace \"o\" \"0\" g
        ",
        &[
            ("up", "HELLO, WORLD"),
            ("n", "12"),
            ("parts", "['Hello', 'World']"),
            ("joined", "Hello/World"),
            ("has", "True"),
            ("r", "Hell0, W0rld"),
        ],
    );
}

#[test]
fn e2e_string_to_int_parses_or_yields_none() {
    run_and_check(
        "
        let ok = Option.withDefault 0 (String.toInt \"41\")
        let bad = Option.withDefault 0 (String.toInt \"nope\")
        let neg = Option.withDefault 0 (String.toInt \"-5\")
        ",
        &[("ok", "41"), ("bad", "0"), ("neg", "-5")],
    );
}

#[test]
fn unused_collection_helpers_are_not_emitted() {
    let py = pyfun::compile("let s = Set.empty").unwrap();
    assert!(!py.contains("_pf_"), "{py}");
}

#[test]
fn result_map_lowers_to_a_helper_pulling_in_the_result_prelude() {
    let py = pyfun::compile("let r = Result.map (fun x -> x) (Ok 1)").unwrap();
    assert!(py.contains("class Ok:"), "{py}");
    assert!(py.contains("def _pf_result_map(f, r):"), "{py}");
    assert!(py.contains("return Ok(f(r._0))"), "{py}");
}

#[test]
fn seq_map_filter_lower_to_pythons_lazy_builtins() {
    // Unlike `List.map` (eager, wrapped in `_pf_map`), `Seq.map`/`filter` route to
    // Python's own lazy builtins with no wrapper.
    let py =
        pyfun::compile("let r = Seq.map (fun x -> x) (Seq.filter (fun x -> true) (Seq.range 0 3))")
            .unwrap();
    assert!(py.contains("map(lambda x: x"), "{py}");
    assert!(py.contains("filter(lambda x: True"), "{py}");
    assert!(py.contains("range(0, 3)"), "{py}");
    assert!(
        !py.contains("_pf_map"),
        "Seq.map must not use the eager helper: {py}"
    );
}

#[test]
fn seq_take_lowers_to_itertools_islice() {
    let py = pyfun::compile("let r = Seq.take 2 (Seq.range 0 9)").unwrap();
    assert!(py.contains("import itertools"), "{py}");
    assert!(py.contains("def _pf_seq_take(n, xs):"), "{py}");
    assert!(py.contains("return itertools.islice(xs, n)"), "{py}");
}

#[test]
fn module_members_lower_to_mangled_top_level_names() {
    let py = pyfun::compile(
        "module Geometry =\n  let pi = 3\n  let area r = pi * r * r\nlet big = Geometry.area 10",
    )
    .unwrap();
    assert!(py.contains("Geometry_pi = 3"), "{py}");
    assert!(py.contains("def Geometry_area(r):"), "{py}");
    // A bare sibling reference (`pi` inside `area`) is rewritten to the mangled name.
    assert!(py.contains("return Geometry_pi * r * r"), "{py}");
    // Qualified access from outside lowers to the same mangled name.
    assert!(py.contains("big = Geometry_area(10)"), "{py}");
}

#[test]
fn partial_application_of_a_module_member() {
    let py = pyfun::compile("module M =\n  let add a b = a + b\nlet inc = M.add 1").unwrap();
    assert!(py.contains("inc = functools.partial(M_add, 1)"), "{py}");
}

#[test]
fn result_to_option_pulls_in_both_preludes() {
    let py = pyfun::compile("let o = Result.toOption (Ok 1)").unwrap();
    assert!(py.contains("class Ok:"), "Result prelude: {py}");
    assert!(py.contains("class Some:"), "Option prelude: {py}");
    assert!(py.contains("def _pf_result_to_option(r):"), "{py}");
    assert!(py.contains("return Some(r._0)"), "{py}");
    assert!(py.contains("return None_()"), "{py}");
}

#[test]
fn if_in_value_position_is_a_conditional_expression() {
    let py = pyfun::compile("let r = if true then 1 else 2").unwrap();
    assert!(py.contains("r = 1 if True else 2"), "{py}");
}

#[test]
fn exhaustive_match_without_wildcard_keeps_a_runtime_guard() {
    // Even a statically exhaustive ADT match emits a defensive `case _: raise`.
    let py = pyfun::compile("type Color = Red | Green | Blue\nlet f c = match c: case Red: 1 case Green: 2 case Blue: 3").unwrap();
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
fn adt_classes_get_structural_hash() {
    // A `__hash__` consistent with `__eq__` (type + fields) so ADTs/records can be
    // `Set` elements / `Map` keys — defining `__eq__` alone would make them
    // unhashable in Python.
    let py = pyfun::compile("type Opt a = Empty | Has a\nlet x = Has 1").unwrap();
    assert!(py.contains("def __hash__(self):"), "{py}");
    // Nullary hashes the type; a field hashes (type, field).
    assert!(py.contains("return hash(type(self))"), "{py}");
    assert!(py.contains("return hash((type(self), self._0))"), "{py}");
}

#[test]
fn record_lowers_to_class_with_named_fields() {
    let py =
        pyfun::compile("type Point = { x: int, y: int }\nlet p = Point { y = 4, x = 3 }").unwrap();
    assert!(py.contains("class Point:"), "{py}");
    assert!(py.contains("__match_args__ = ('x', 'y')"), "{py}");
    assert!(py.contains("def __init__(self, x, y):"), "{py}");
    // The literal is reordered to the declared field order for a positional call.
    assert!(py.contains("p = Point(3, 4)"), "{py}");
}

#[test]
fn record_update_copies_through_a_temp() {
    let py = pyfun::compile(
        "type Point = { x: int, y: int }\nlet p = Point { x = 1, y = 2 }\nlet q = { p with x = 9 }",
    )
    .unwrap();
    // `p` is bound to a temp so it is evaluated once; the unchanged field is read
    // from it, the changed one is the new value.
    assert!(py.contains("q = Point(9, _pf_t0.y)"), "{py}");
}

#[test]
fn record_field_access_lowers_to_attribute() {
    let py =
        pyfun::compile("type Point = { x: int }\nlet p = Point { x = 1 }\nlet s = p.x").unwrap();
    assert!(py.contains("s = p.x"), "{py}");
}

#[test]
fn record_pattern_lowers_to_keyword_class_pattern() {
    let py = pyfun::compile(
        "type Point = { x: int, y: int }\n\
         let f p = match p: case Point { x = 0, y }: y case Point { x }: x",
    )
    .unwrap();
    // Keyword class patterns name a subset of fields, in source order.
    assert!(py.contains("case Point(x=0, y=y):"), "{py}");
    assert!(py.contains("case Point(x=x):"), "{py}");
}

#[test]
fn tuple_literal_lowers_to_python_tuple() {
    let py = pyfun::compile("let pair = (1, 2)\nlet triple = (1, true, 3)").unwrap();
    assert!(py.contains("pair = (1, 2)"), "{py}");
    assert!(py.contains("triple = (1, True, 3)"), "{py}");
}

#[test]
fn tuple_pattern_lowers_to_sequence_pattern() {
    let py = pyfun::compile("let swap p = match p: case (a, b): (b, a)").unwrap();
    assert!(py.contains("case (a, b):"), "{py}");
    assert!(py.contains("return (b, a)"), "{py}");
}

#[test]
fn e2e_tuple_construct_and_destructure() {
    run_and_check(
        "
        let swap p =
          match p:
            case (a, b): (b, a)
        let fst t =
          match t:
            case (a, _): a
        let pair = (10, 20)
        let s = swap pair
        let first = fst pair
        ",
        &[("s", "(20, 10)"), ("first", "10")],
    );
}

#[test]
fn e2e_nested_tuple_pattern() {
    run_and_check(
        "
        let f t =
          match t:
            case ((a, b), c): a + b + c
        let r = f ((1, 2), 3)
        ",
        &[("r", "6")],
    );
}

#[test]
fn e2e_record_pattern_match() {
    run_and_check(
        "
        type Point = { x: int, y: int }
        let classify p =
          match p:
            case Point { x = 0, y = 0 }: 1
            case Point { x = 0 }: 2
            case Point { x, y }: x + y
        let a = classify (Point { x = 0, y = 0 })
        let b = classify (Point { x = 0, y = 9 })
        let c = classify (Point { x = 3, y = 4 })
        ",
        &[("a", "1"), ("b", "2"), ("c", "7")],
    );
}

#[test]
fn e2e_nested_record_and_constructor_pattern() {
    // A constructor sub-pattern inside a record pattern binds through both levels.
    run_and_check(
        "
        type Box = { item: Option int, tag: bool }
        let f b =
          match b:
            case Box { item = Some n, tag = true }: n
            case Box { item = Some n }: n + 100
            case _: 0
        let a = f (Box { item = Some 5, tag = true })
        let b = f (Box { item = Some 5, tag = false })
        let c = f (Box { item = None, tag = true })
        ",
        &[("a", "5"), ("b", "105"), ("c", "0")],
    );
}

#[test]
fn e2e_deep_exhaustive_match_without_wildcard() {
    // Deep exhaustiveness accepts this (every nested case covered), and it runs.
    run_and_check(
        "
        let f o =
          match o:
            case Some true: 1
            case Some false: 2
            case None: 3
        let a = f (Some true)
        let b = f (Some false)
        let c = f None
        ",
        &[("a", "1"), ("b", "2"), ("c", "3")],
    );
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
fn closure_reassigning_a_module_mut_emits_global() {
    // The enclosing block inlines at module level, so the captured `n` is global.
    let py = pyfun::compile(
        "let counter =\n  let mut n = 0\n  let bump x =\n    n <- n + x\n    n\n  bump 5",
    )
    .unwrap();
    assert!(py.contains("def bump(x):"), "{py}");
    assert!(py.contains("global n"), "{py}");
    assert!(!py.contains("nonlocal"), "{py}");
}

#[test]
fn closure_reassigning_an_enclosing_fn_mut_emits_nonlocal() {
    let py = pyfun::compile(
        "let make base =\n  let mut n = base\n  let bump x =\n    n <- n + x\n    n\n  bump 5",
    )
    .unwrap();
    assert!(py.contains("def make(base):"), "{py}");
    assert!(py.contains("nonlocal n"), "{py}");
    assert!(!py.contains("global"), "{py}");
}

#[test]
fn local_only_mut_needs_no_capture_declaration() {
    let py =
        pyfun::compile("let scaled a b =\n  let mut acc = a\n  acc <- acc + b\n  acc").unwrap();
    assert!(!py.contains("nonlocal"), "{py}");
    assert!(!py.contains("global"), "{py}");
}

#[test]
fn e2e_closure_reassigns_captured_mut() {
    // `global` path: the accumulator persists across calls (5 then +3 = 8).
    run_and_check(
        "
        let counter =
          let mut n = 0
          let bump x =
            n <- n + x
            n
          let a = bump 5
          bump 3
        ",
        &[("counter", "8")],
    );
}

#[test]
fn e2e_nonlocal_closure_in_a_function() {
    // `nonlocal` path: a fresh accumulator per `make` call.
    run_and_check(
        "
        let make base =
          let mut n = base
          let bump x =
            n <- n + x
            n
          let a = bump 5
          bump 3
        let r = make 100
        ",
        &[("r", "108")],
    );
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
    let err = pyfun::compile("let f o = match o: case Nope v: v").unwrap_err();
    assert!(err.to_string().contains("unknown constructor"), "{err}");
}

// ---------- end-to-end execution ----------

#[test]
fn recursive_call_lowers_to_a_direct_call() {
    let py = pyfun::compile("let fact n =\n  if n == 0 then 1\n  else n * fact (n - 1)").unwrap();
    assert!(py.contains("def fact(n):"), "{py}");
    // The self-call is a full application (arity known), not a functools.partial.
    assert!(py.contains("fact(n - 1)"), "{py}");
}

#[test]
fn e2e_recursive_functions() {
    run_and_check(
        "
        let fact n =
          if n == 0 then 1
          else n * fact (n - 1)
        let fib n =
          if n < 2 then n
          else fib (n - 1) + fib (n - 2)
        let a = fact 6
        let b = fib 10
        ",
        &[("a", "720"), ("b", "55")],
    );
}

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
fn e2e_elif_chain_selects_the_right_branch() {
    // `elif` is sugar for `else if`; the chain compiles to nested conditionals and
    // picks the first matching branch (here via nested ternaries, all-expression).
    run_and_check(
        "
        let grade n =
          if n >= 90 then \"A\"
          elif n >= 80 then \"B\"
          elif n >= 70 then \"C\"
          else \"F\"
        let a = grade 95
        let b = grade 85
        let c = grade 72
        let f = grade 50
        ",
        &[("a", "A"), ("b", "B"), ("c", "C"), ("f", "F")],
    );
}

#[test]
fn e2e_if_and_match() {
    run_and_check(
        "
        let classify n =
          match n:
            case 0: \"zero\"
            case 1: \"one\"
            case _: \"many\"
        let r1 = classify 0
        let r2 = classify 7
        let r3 = if true then 10 else 20
        ",
        &[("r1", "zero"), ("r2", "many"), ("r3", "10")],
    );
}

#[test]
fn e2e_blocks_in_match_arms_and_if_branches() {
    // Multi-statement blocks (with local `let` and `<-`) in arm and branch
    // positions, lowered to flat Python statement sequences.
    run_and_check(
        "
        let classify n =
          match n:
            case 0:
              let base = 100
              base
            case _:
              let mut acc = n
              acc <- acc * 2
              acc
        let absdiff a b =
          if a > b then
              let d = a - b
              d
          else
              let d = b - a
              d
        let r1 = classify 0
        let r2 = classify 5
        let r3 = absdiff 3 10
        ",
        &[("r1", "100"), ("r2", "10"), ("r3", "7")],
    );
}

#[test]
fn e2e_block_in_lambda_body() {
    run_and_check(
        "
        let doubler = fun x ->
          let y = x + x
          y
        let r = doubler 21
        ",
        &[("r", "42")],
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
        "let r = (match 1: case 1: 10 case _: 20) + 5",
        &[("r", "15")],
    );
}

#[test]
fn e2e_adt_construction_and_match() {
    run_and_check(
        "
        type Color = Red | Green | Blue
        let unwrap o = match o: case Some v: v case None: 0
        let r1 = unwrap (Some 5)
        let r2 = unwrap None
        let rank c = match c: case Red: 1 case Green: 2 case Blue: 3
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
        let p = Point { x = 3, y = 4 }
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
        let mk v = Box { item = v }
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
        let head d xs = match xs: case Nil: d case Cons h t: h
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
fn e2e_derived_measure_aliases_erase() {
    // Aliases are pure compile-time machinery; like base units they vanish at
    // runtime, leaving plain Python numbers.
    run_and_check(
        "
        measure kg
        measure m
        measure s
        measure N = kg m / s^2
        measure Pa = N / m^2
        let force = 10<N>
        let area = 2<m^2>
        let pressure = force / area
        ",
        &[("force", "10"), ("pressure", "5.0")],
    );
}

#[test]
fn e2e_result_ce_binds_and_short_circuits() {
    // Extract the result via a match so the assertions compare plain ints.
    run_and_check(
        "
        let safe ok v = result { let! x = if ok then Ok v else Error 9  return (x + 1) }
        let unwrap r = match r: case Ok n: n case Error e: e
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
fn e2e_user_monad_ce_binds_and_short_circuits() {
    // A user-defined `Maybe` builder desugars to bind/return_ calls and runs.
    run_and_check(
        "
        module Maybe =
          let bind m f = match m: case Some x: f x case None: None
          let return_ x = Some x
        let safe a b =
          Maybe {
            let! x = a
            let! y = b
            return x + y
          }
        let unwrap m = match m: case Some n: n case None: 0
        let r1 = unwrap (safe (Some 3) (Some 4))
        let r2 = unwrap (safe (Some 3) None)
        ",
        &[("r1", "7"), ("r2", "0")],
    );
}

#[test]
fn e2e_user_yield_ce_combines_via_delay() {
    // A yield builder exercising yield_/combine/delay (here, summation).
    run_and_check(
        "
        module Sum =
          let yield_ x = x
          let combine a b = a + b
          let delay f = f 0
        let total =
          Sum {
            yield 1
            yield 2
            yield 3
            yield 4
          }
        ",
        &[("total", "10")],
    );
}

#[test]
fn user_ce_lowers_to_plain_calls() {
    let py = pyfun::compile(
        "
        module M =
          let bind m f = f m
          let return_ x = x
        let r = M { let! x = 3  return x }
        ",
    )
    .unwrap();
    assert!(py.contains("r = M_bind(3, lambda x: M_return_(x))"), "{py}");
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
fn e2e_adts_and_records_as_keys_and_elements() {
    // The generated `__hash__` lets ADTs and records be `Set` elements / `Map`
    // keys, with structural identity (equal values collapse).
    run_and_check(
        "type Color = Red | Green | Blue\n\
         type Point = { x: int, y: int }\n\
         let cs = Set.ofList [Red, Green, Red, Blue]\n\
         let ncolors = Set.len cs\n\
         let hasGreen = Set.contains Green cs\n\
         let m = Map.add (Some 1) \"one\" Map.empty\n\
         let v = Option.withDefault \"?\" (Map.tryFind (Some 1) m)\n\
         let pts = Set.ofList [Point { x = 1, y = 2 }, Point { x = 1, y = 2 }, Point { x = 3, y = 4 }]\n\
         let npts = Set.len pts",
        &[
            ("ncolors", "3"),
            ("hasGreen", "True"),
            ("v", "one"),
            ("npts", "2"),
        ],
    );
}

#[test]
fn e2e_seq_module_is_lazy() {
    // `Seq.range 0 1000000` then `take`/`map` forces only a handful of elements —
    // laziness, not a million-element materialization.
    run_and_check(
        "let nats = Seq.range 0 1000000\n\
         let squares = Seq.toList (Seq.take 5 (Seq.map (fun x -> x * x) nats))\n\
         let evens = Seq.toList (Seq.take 3 (Seq.filter (fun x -> x // 2 * 2 == x) nats))\n\
         let total = Seq.fold (fun acc x -> acc + x) 0 (Seq.ofList [1, 2, 3, 4])",
        &[
            ("squares", "[0, 1, 4, 9, 16]"),
            ("evens", "[0, 2, 4]"),
            ("total", "10"),
        ],
    );
}

#[test]
fn e2e_in_file_module() {
    run_and_check(
        "module Geometry =\n\
        \x20 let pi = 3\n\
        \x20 let area r = pi * r * r\n\
        \x20 let double a = area a + area a\n\
         let a = Geometry.area 10\n\
         let d = Geometry.double 2\n\
         let p = Geometry.pi",
        &[("a", "300"), ("d", "24"), ("p", "3")],
    );
}

#[test]
fn e2e_result_module() {
    run_and_check(
        "let safeDiv a b = if b == 0 then Error \"div0\" else Ok (a // b)\n\
         let r1 = safeDiv 10 2\n\
         let r2 = safeDiv 10 0\n\
         let a = Result.withDefault 0 (Result.map (fun x -> x + 1) r1)\n\
         let b = Result.withDefault 0 r2\n\
         let c = Result.isOk r1\n\
         let d = Result.isError r2\n\
         let e = Option.withDefault 0 (Result.toOption r1)\n\
         let f = Result.withDefault 0 (Result.bind (fun x -> safeDiv x 3) r1)\n\
         let g = Result.isError (Result.mapError (fun s -> s) r2)",
        &[
            ("a", "6"),
            ("b", "0"),
            ("c", "True"),
            ("d", "True"),
            ("e", "5"),
            ("f", "1"),
            ("g", "True"),
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
