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
fn numeric_literals_adapt_to_int_or_float() {
    // Integer literals are polymorphic (`num 'a => 'a`), so they mix with floats
    // the Python way; `/` yields float and `//` yields int, both fine here.
    assert!(pyfun::check("let r = 1 + 2.0").is_ok());
    assert!(pyfun::check("let r = (7 / 2) + 1").is_ok());
    assert!(pyfun::check("let r = (7 // 2) + 1").is_ok());
}

#[test]
fn arithmetic_is_numeric_polymorphic() {
    // `add` is usable at int *and* float in one program — the win from `num`.
    let src = "let add a b = a + b\nlet i = add 1 2\nlet f = add 1.5 2.5";
    assert!(pyfun::check(src).is_ok());
    // The prelude numerics span int and float too.
    assert!(pyfun::check("let a = max 1 2\nlet b = min 1.5 2.5\nlet c = abs 3").is_ok());
}

#[test]
fn rejects_mixing_concrete_int_and_float() {
    // Literals adapt, but two *concrete* numeric bases don't implicitly coerce:
    // `f` is forced to `int` by its int-literal pattern, so `1.0 + f 0` clashes.
    let src = "let f n = match n with | 0 -> n | _ -> n\nlet r = 1.0 + f 0";
    assert_error_contains(src, "found int");
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

// ---------- comparison & equality (DESIGN §7.1) ----------

#[test]
fn accepts_comparison_and_equality() {
    assert!(pyfun::check("let a = 1 < 2\nlet b = 2.5 >= 1.0\nlet c = \"x\" < \"y\"").is_ok());
    assert!(pyfun::check("let a = 1 == 1\nlet b = true != false\nlet c = \"x\" == \"y\"").is_ok());
}

#[test]
fn comparison_and_equality_produce_bool() {
    // The result is a bool, usable as an `if` condition.
    assert!(pyfun::check("let pick a b = if a < b then a else b").is_ok());
}

#[test]
fn generic_comparison_function_is_constrained() {
    // `lt` infers `comparison 'a => 'a -> 'a -> bool`: usable at int, float, string.
    let src = "let lt a b = a < b\nlet x = lt 1 2\nlet y = lt 1.5 2.5\nlet z = lt \"a\" \"b\"";
    assert!(pyfun::check(src).is_ok());
}

#[test]
fn rejects_comparison_of_unorderable_type() {
    // Booleans and functions are not comparable with `<`.
    assert_error_contains("let r = true < false", "does not support comparison");
    // `id` is concretely a function here, so comparing it is rejected.
    assert_error_contains(
        "let id x = x\nlet bad = id < id",
        "does not support comparison",
    );
}

#[test]
fn rejects_equality_across_different_types() {
    // Equality requires both sides to have the same type.
    assert_error_contains("let r = 1 == \"x\"", "expected int, found string");
}

#[test]
fn accepts_structural_equality_of_adts() {
    let src = "type Option a = None | Some a\nlet r = Some 1 == Some 2";
    assert!(pyfun::check(src).is_ok());
}

#[test]
fn unit_annotation_and_less_than_are_distinguished() {
    // `5<m>` (adjacent) is a unit; `5 < x` (spaced) is a comparison.
    assert!(pyfun::check("measure m\nlet d = 5<m>\nlet ok = d < 9<m>").is_ok());
    assert!(pyfun::check("let r = 5 < 9").is_ok());
}

// ---------- boolean operators ----------

#[test]
fn accepts_boolean_operators() {
    assert!(
        pyfun::check("let a = true and false\nlet b = true or false\nlet c = not true").is_ok()
    );
    // Mixed with comparisons, producing a bool condition.
    assert!(pyfun::check("let between lo hi x = lo <= x and x <= hi").is_ok());
}

#[test]
fn rejects_non_bool_logical_operands() {
    assert_error_contains("let r = 1 and true", "expected bool, found int");
    assert_error_contains("let r = true or 2", "expected bool, found int");
    assert_error_contains("let r = not 5", "expected bool, found int");
}

#[test]
fn not_binds_looser_than_comparison() {
    // `not 1 == 2` is `not (1 == 2)` (bool), which type-checks.
    assert!(pyfun::check("let r = not 1 == 2").is_ok());
}

// ---------- records ----------

#[test]
fn accepts_record_decl_literal_update_and_access() {
    let src = "type Point = { x: int, y: int }\n\
               let p = { x = 3, y = 4 }\n\
               let q = { p with y = 9 }\n\
               let s = p.x + q.y";
    assert!(pyfun::check(src).is_ok());
}

#[test]
fn record_field_access_drives_function_type() {
    // `r.x + r.y` forces `r : Point` even without an annotation.
    let src = "type Point = { x: int, y: int }\n\
               let sumxy r = r.x + r.y\n\
               let t = sumxy { x = 1, y = 2 }";
    assert!(pyfun::check(src).is_ok());
}

#[test]
fn parameterized_record_is_polymorphic() {
    // A `Box a` literal/access used at two element types in one program.
    let src = "type Box a = { item: a }\n\
               let mk v = { item = v }\n\
               let bi = (mk 5).item\n\
               let bs = (mk \"hi\").item";
    assert!(pyfun::check(src).is_ok());
}

#[test]
fn rejects_missing_record_field() {
    assert_error_contains(
        "type Point = { x: int, y: int }\nlet p = { x = 1 }",
        "missing field",
    );
}

#[test]
fn rejects_unknown_field_in_literal() {
    assert_error_contains(
        "type Point = { x: int, y: int }\nlet p = { x = 1, y = 2, z = 3 }",
        "has no field `z`",
    );
}

#[test]
fn rejects_unknown_field_access() {
    assert_error_contains(
        "type Point = { x: int }\nlet p = { x = 1 }\nlet z = p.nope",
        "unknown record field `nope`",
    );
}

#[test]
fn rejects_wrong_record_field_type() {
    assert_error_contains(
        "type Point = { x: int, y: int }\nlet p = { x = 1, y = true }",
        "expected int, found bool",
    );
}

#[test]
fn rejects_field_name_reused_across_records() {
    assert_error_contains(
        "type A = { x: int }\ntype B = { x: int }",
        "field `x` is already defined in record `A`",
    );
}

#[test]
fn rejects_field_declared_twice_in_one_record() {
    assert_error_contains("type P = { x: int, x: int }", "field `x` is declared twice");
}

#[test]
fn rejects_update_of_unrelated_field() {
    // `y` belongs to a different record than `Point`, so the update mixes types.
    assert_error_contains(
        "type Point = { x: int }\n\
         type Other = { y: int }\n\
         let p = { x = 1 }\n\
         let bad = { p with y = 2 }",
        "type mismatch",
    );
}

// ---------- blocks & mutability ----------

#[test]
fn accepts_block_with_local_bindings() {
    let src = "let f x =\n    let y = x + 1\n    let z = y + 1\n    z";
    assert!(pyfun::check(src).is_ok());
}

#[test]
fn accepts_local_mut_and_reassignment() {
    let src = "let sum3 a b c =\n    let mut acc = 0\n    acc <- acc + a\n    acc <- acc + b\n    acc <- acc + c\n    acc";
    assert!(pyfun::check(src).is_ok());
}

#[test]
fn accepts_top_level_mut_and_reassignment() {
    assert!(pyfun::check("let mut x = 0\nx <- x + 1").is_ok());
}

#[test]
fn local_bindings_stay_in_scope_polymorphically() {
    // A local `let` is generalized like a top-level one (usable at two types).
    let src = "let f u =\n    let id a = a\n    let p = id 1\n    let q = id true\n    p";
    assert!(pyfun::check(src).is_ok());
}

#[test]
fn rejects_assigning_immutable_binding() {
    assert_error_contains(
        "let f x =\n    let y = 0\n    y <- x\n    y",
        "it is immutable",
    );
}

#[test]
fn rejects_assigning_unbound_binding() {
    assert_error_contains("nope <- 1", "unbound name `nope`");
}

#[test]
fn rejects_reassignment_with_wrong_type() {
    assert_error_contains("let mut x = 0\nx <- true", "expected int, found bool");
}

#[test]
fn rejects_mutable_binding_with_parameters() {
    assert_error_contains("let mut f x = x", "cannot take parameters");
}

#[test]
fn rejects_non_unit_intermediate_statement() {
    // A non-final statement that yields a value must be bound, not dropped.
    assert_error_contains("let f x =\n    x + 1\n    x", "must be `unit`");
}

// ---------- the compiler is the gatekeeper ----------

#[test]
fn compile_is_gated_on_type_checking() {
    // An ill-typed program must not produce Python.
    assert!(pyfun::compile("let add a b = a + b\nlet r = add 1 true").is_err());
    // A well-typed one still compiles.
    assert!(pyfun::compile("let add a b = a + b\nlet r = add 1 2").is_ok());
}
