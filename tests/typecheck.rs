//! Phase 3 tests: Hindley–Milner type inference.

/// Messages of all type errors for `source` (panics if it actually type-checks).
fn errors(source: &str) -> Vec<String> {
    pyfun::check(source)
        .err()
        .unwrap_or_else(|| panic!("expected a type error in:\n{source}"))
        .into_iter()
        .map(|e| e.message)
        .collect()
}

fn assert_error_contains(source: &str, needle: &str) {
    let msgs = errors(source);
    assert!(
        msgs.iter().any(|m| m.contains(needle)),
        "no error containing {needle:?}\nsource:\n{source}\nerrors: {msgs:?}"
    );
}

// ---------- well-typed programs ----------

#[test]
fn accepts_basic_and_curried_functions() {
    assert!(pyfun::check("let add a b = a + b\nlet r = add 1 2").is_ok());
}

#[test]
fn let_generalization_makes_bindings_polymorphic() {
    // `id` must be usable at two different types in the same program.
    assert!(pyfun::check("let id x = x\nlet a = id 1\nlet b = id true").is_ok());
}

#[test]
fn accepts_higher_order_composition() {
    assert!(pyfun::check("let compose = fun f g x -> f (g x)\nlet twice f x = f (f x)").is_ok());
}

#[test]
fn accepts_partial_application_and_pipe() {
    assert!(pyfun::check("let add a b = a + b\nlet inc = add 1\nlet r = 5 |> inc").is_ok());
}

#[test]
fn accepts_match_over_int_literals() {
    assert!(pyfun::check("let f n = match n with | 0 -> \"zero\" | _ -> \"many\"").is_ok());
}

#[test]
fn accepts_user_defined_adt() {
    let src = "type Option a = None | Some a\n\
               let unwrap o = match o with | Some v -> v | None -> 0\n\
               let r = unwrap (Some 5)";
    assert!(pyfun::check(src).is_ok());
}

#[test]
fn accepts_parameterized_and_recursive_adts() {
    let src = "type Either a b = Left a | Right b\n\
               type List a = Nil | Cons a (List a)\n\
               let xs = Cons 1 (Cons 2 Nil)\n\
               let e = Left 1";
    assert!(pyfun::check(src).is_ok());
}

#[test]
fn rejects_redefining_builtin_result() {
    // `Result`, `Ok`, and `Error` are reserved by the result computation expression.
    assert_error_contains("type Result a b = Ok a | Bad b", "already defined");
}

// ---------- ill-typed programs ----------

#[test]
fn rejects_argument_type_mismatch() {
    assert_error_contains(
        "let add a b = a + b\nlet r = add 1 true",
        "expected int, found bool",
    );
}

#[test]
fn rejects_non_bool_condition() {
    assert_error_contains("let r = if 1 then 2 else 3", "expected bool, found int");
}

#[test]
fn rejects_branch_type_mismatch() {
    assert_error_contains(
        "let r = if true then 1 else \"x\"",
        "expected int, found string",
    );
}

#[test]
fn rejects_unbound_name() {
    assert_error_contains("let r = nope", "unbound name `nope`");
}

#[test]
fn rejects_infinite_type() {
    // Self-application `x x` has no finite type.
    assert_error_contains("let bad = fun x -> x x", "infinite type");
}

#[test]
fn rejects_match_arm_result_mismatch() {
    assert_error_contains(
        "let f n = match n with | 0 -> 1 | _ -> \"two\"",
        "expected int, found string",
    );
}

#[test]
fn rejects_unknown_constructor() {
    assert_error_contains(
        "let f o = match o with | Some v -> v | None -> 0",
        "unknown constructor `Some`",
    );
}

#[test]
fn rejects_non_exhaustive_adt_match() {
    let src = "type Option a = None | Some a\nlet f o = match o with | Some v -> v";
    assert_error_contains(src, "non-exhaustive match: missing `None`");
}

#[test]
fn rejects_non_exhaustive_int_match() {
    assert_error_contains("let f n = match n with | 0 -> 1", "add a wildcard");
}

#[test]
fn rejects_constructor_used_at_wrong_type() {
    // `Some` produces an `Option`, not an `int`.
    let src = "type Option a = None | Some a\nlet r = (Some 1) + 1";
    assert_error_contains(src, "expected int, found Option");
}

#[test]
fn rejects_unknown_type_in_field() {
    assert_error_contains("type Bad = Mk Nope", "unknown type `Nope`");
}

// ---------- computation expressions ----------

#[test]
fn accepts_result_computation_expression() {
    let src = "let f ok v = result { let! x = if ok then Ok v else Error 0  return (x + 1) }";
    assert!(pyfun::check(src).is_ok());
}

#[test]
fn accepts_seq_computation_expression() {
    assert!(pyfun::check("let xs = seq { yield 1  yield 2 }").is_ok());
    // yield! splices a sub-sequence of the same element type.
    assert!(pyfun::check("let xs = seq { yield 1  yield! (seq { yield 2 }) }").is_ok());
}

#[test]
fn accepts_async_computation_expression() {
    assert!(pyfun::check("let f = async { let! x = async { return 1 }  return (x + 1) }").is_ok());
}

#[test]
fn rejects_yield_in_result_block() {
    assert_error_contains(
        "let a = result { yield 1 }",
        "`yield` is not allowed in a `result` block",
    );
}

#[test]
fn rejects_result_without_return() {
    assert_error_contains("let a = result { let! x = Ok 1 }", "must end with `return`");
}

#[test]
fn rejects_return_before_end() {
    assert_error_contains(
        "let a = result { return 1  return 2 }",
        "must be the final item",
    );
}

#[test]
fn rejects_seq_with_return() {
    assert_error_contains(
        "let a = seq { return 5 }",
        "only `yield`, `yield!`, and `let`",
    );
}

#[test]
fn rejects_binding_wrong_monad() {
    // `let!` in an async block must bind an Async, not a Seq.
    let src = "let a = async { let! x = (seq { yield 1 })  return x }";
    assert_error_contains(src, "expected Async");
}

// ---------- units of measure ----------

#[test]
fn accepts_dimensional_arithmetic() {
    let src = "measure m\nmeasure s\nlet dist = 100<m>\nlet time = 10<s>\nlet speed = dist / time";
    assert!(pyfun::check(src).is_ok());
}

#[test]
fn accepts_unit_polymorphic_function() {
    // `area` works at different unit combinations.
    let src = "measure m\nmeasure s\n\
               let area w h = w * h\n\
               let rect = area 2<m> 3<m>\n\
               let flow = area 2<m> 3<s>";
    assert!(pyfun::check(src).is_ok());
}

#[test]
fn rejects_adding_different_units() {
    assert_error_contains(
        "measure m\nmeasure s\nlet bad = 1<m> + 1<s>",
        "expected int<m>, found int<s>",
    );
}

#[test]
fn rejects_unit_result_used_at_wrong_unit() {
    // speed is m/s, so adding metres is a dimension error. Uses `//` so the
    // result stays `int` (`/` would make it `float`, a different mismatch).
    let src = "measure m\nmeasure s\nlet speed = 100<m> // 10<s>\nlet bad = speed + 1<m>";
    assert_error_contains(src, "expected int<m/s>, found int<m>");
}

#[test]
fn true_division_yields_float_floor_division_yields_int() {
    // `/` produces a float, so adding an int to it is rejected (arithmetic is
    // integer-only until the `num` constraint lands); `//` keeps it an int.
    assert_error_contains("let r = (7 / 2) + 1", "expected int, found float");
    assert!(pyfun::check("let r = (7 // 2) + 1").is_ok());
}

#[test]
fn rejects_unknown_measure() {
    assert_error_contains("let x = 5<furlong>", "unknown measure `furlong`");
}

#[test]
fn dimensionless_units_unify_with_plain_ints() {
    // An explicit `<1>` is dimensionless and interoperates with bare literals.
    assert!(pyfun::check("let x = 5<1> + 3").is_ok());
}

// ---------- prelude (built-in functions, DESIGN §6) ----------

#[test]
fn accepts_print_of_any_type() {
    // `print : 'a -> unit` is parametrically polymorphic.
    assert!(pyfun::check("let a = print 1\nlet b = print true\nlet c = print \"x\"").is_ok());
}

#[test]
fn accepts_unit_polymorphic_numeric_builtins() {
    let src = "measure m\n\
               let big = max 3<m> 5<m>\n\
               let small = min 1 2\n\
               let d = abs (10<m> - 4<m>)";
    assert!(pyfun::check(src).is_ok());
}

#[test]
fn rejects_min_across_different_units() {
    assert_error_contains(
        "measure m\nmeasure s\nlet bad = min 3<m> 5<s>",
        "expected int<m>, found int<s>",
    );
}

#[test]
fn user_definition_shadows_a_prelude_name() {
    // A user `min` overrides the builtin, here at a non-numeric type.
    assert!(pyfun::check("let min a b = a\nlet r = min true false").is_ok());
}

// ---------- the compiler is the gatekeeper ----------

#[test]
fn compile_is_gated_on_type_checking() {
    // An ill-typed program must not produce Python.
    assert!(pyfun::compile("let add a b = a + b\nlet r = add 1 true").is_err());
    // A well-typed one still compiles.
    assert!(pyfun::compile("let add a b = a + b\nlet r = add 1 2").is_ok());
}
