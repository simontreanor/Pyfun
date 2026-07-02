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
fn accepts_unit_literal() {
    assert!(pyfun::check("let nothing = ()").is_ok());
    // A thunk forced with unit, as a CE `delay` would be written.
    assert!(pyfun::check("let force f = f ()").is_ok());
}

#[test]
fn rejects_unit_in_arithmetic() {
    // `()` has type `unit`, which is not numeric.
    assert_error_contains("let bad = () + 1", "unit");
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

// ---------- recursion (implicit, function bindings only) ----------

#[test]
fn function_binding_is_in_scope_in_its_own_body() {
    // A `let f x = …` sees itself, like Python's `def` — no `rec` keyword.
    assert!(
        pyfun::check("let fact n =\n  if n == 0 then 1\n  else n * fact (n - 1)\nlet r = fact 5")
            .is_ok()
    );
}

#[test]
fn recursive_function_has_the_expected_type() {
    // The recursive call constrains the argument, so `fact` is `int -> int`; calling
    // it on a bool is a type error.
    assert_error_contains(
        "let fact n =\n  if n == 0 then 1\n  else n * fact (n - 1)\nlet bad = fact true",
        "expected int, found bool",
    );
}

#[test]
fn value_binding_still_cannot_self_refer() {
    // Only *function* bindings are made self-visible; a plain value `let x = x` is
    // still unbound (as `x = x` is a NameError at Python's module level).
    assert_error_contains("let x = x + 1", "unbound name `x`");
}

#[test]
fn recursion_does_not_break_let_generalization() {
    // A recursive (monomorphic) function and an ordinary polymorphic one coexist.
    assert!(
        pyfun::check(
            "let fact n =\n  if n == 0 then 1\n  else n * fact (n - 1)\n\
             let id x = x\nlet a = id 1\nlet b = id true\nlet c = fact 4"
        )
        .is_ok()
    );
}

#[test]
fn mutual_recursion_across_bindings_is_unbound() {
    // Declare-before-use: `isEven` cannot see the later `isOdd` (no mutual recursion;
    // `DESIGN.md` §6.1).
    assert_error_contains(
        "let isEven n = if n == 0 then true else isOdd (n - 1)\n\
         let isOdd n = if n == 0 then false else isEven (n - 1)",
        "unbound name `isOdd`",
    );
}

#[test]
fn accepts_partial_application_and_pipe() {
    assert!(pyfun::check("let add a b = a + b\nlet inc = add 1\nlet r = 5 |> inc").is_ok());
}

#[test]
fn accepts_operator_sections() {
    // A section is a first-class function: applied fully, partially, and as an
    // argument to a higher-order function.
    assert!(pyfun::check("let r = (*) 3 4").is_ok());
    assert!(pyfun::check("let double = (*) 2\nlet r = double 5").is_ok());
    assert!(pyfun::check("let r = List.fold (+) 0 [1, 2, 3]").is_ok());
    // Comparison and equality sections keep their result type `bool`.
    assert!(pyfun::check("let r = (<) 1 2").is_ok());
    assert!(pyfun::check("let r = (==) 1 1").is_ok());
    // `(-)` and floor division `(//)`.
    assert!(pyfun::check("let r = (//) 7 2").is_ok());
}

#[test]
fn accepts_as_patterns() {
    // Both the inner var and the `as` name are bound.
    assert!(
        pyfun::check("let f o =\n  match o:\n    case Some v as w: v\n    case None: 0").is_ok()
    );
    // The `as` name is usable in the body.
    assert!(
        pyfun::check("let f o =\n  match o:\n    case Some v as w: w\n    case None: None").is_ok()
    );
    // `_ as x` is a catch-all.
    assert!(pyfun::check("let f n =\n  match n:\n    case 0: 0\n    case _ as x: x").is_ok());
}

#[test]
fn as_patterns_are_transparent_for_exhaustiveness() {
    // `Circle r as w` covers only Circle, so Rect is still unmatched.
    assert_error_contains(
        "type Shape = Circle float | Rect float float\n\
         let f s =\n  match s:\n    case Circle r as w: r",
        "not matched",
    );
    // With both constructors (each `as`-bound) it's exhaustive — no wildcard.
    assert!(
        pyfun::check(
            "type Shape = Circle float | Rect float float\n\
             let f s =\n  match s:\n    case Circle r as w: r\n    case Rect a b as w: a"
        )
        .is_ok()
    );
}

#[test]
fn accepts_discard_binding() {
    // `let _ = e` discards any-typed e (top-level and mid-block, where it lets a
    // non-unit result be dropped despite the "non-final statement is unit" rule).
    assert!(pyfun::check("let _ = 1 + 2").is_ok());
    assert!(pyfun::check("let f x =\n  let _ = x + 1\n  x").is_ok());
    // A discard takes no parameters and can't be `mut`.
    assert!(pyfun::check("let _ a = a").is_err());
    assert!(pyfun::check("let mut _ = 1").is_err());
}

#[test]
fn accepts_string_slice_and_index_of() {
    assert!(pyfun::check("let r = String.slice 0 3 \"hello\"").is_ok());
    assert!(pyfun::check("let r = String.tryIndexOf \"l\" \"hello\"").is_ok()); // : Option int
    assert!(pyfun::check("let r = Option.withDefault 0 (String.tryIndexOf \"l\" \"hi\")").is_ok());
    // slice bounds are ints, not strings.
    assert_error_contains("let r = String.slice \"a\" 3 \"hello\"", "int");
}

#[test]
fn accepts_exponentiation() {
    assert!(pyfun::check("let r = 2.0 ** 8.0").is_ok());
    assert!(pyfun::check("let r = 2 ** 10").is_ok()); // num literals coerce to float
    assert!(pyfun::check("let r = 2.0 ** -1.0").is_ok()); // negative exponent
    assert!(pyfun::check("let r = 2.0 ** 3.0 ** 2.0").is_ok()); // right-assoc chain
}

#[test]
fn exponentiation_is_float_only_and_dimensionless() {
    // Units can't ride through a runtime exponent.
    assert_error_contains("measure m\nlet r = 2.0<m> ** 2.0", "float<m>");
    // Non-numeric operands are rejected.
    assert_error_contains("let r = true ** 2.0", "float");
    // The result is a float, so an int context rejects it.
    assert_error_contains("let r = List.get (2 ** 3) [1, 2]", "float");
}

#[test]
fn accepts_option_bind_filter_to_result() {
    assert!(
        pyfun::check("let r = Option.bind (fun x -> Some (x + 1)) (Some 1)").is_ok(),
        "Option.bind"
    );
    assert!(pyfun::check("let r = Option.filter (fun x -> x > 0) (Some 3)").is_ok());
    assert!(pyfun::check("let r = Option.toResult \"none\" (Some 42)").is_ok());
    // bind chains: Option a -> Option b -> Option c.
    assert!(
        pyfun::check(
            "let r = Option.bind (fun x -> Some (x > 0)) (Option.bind (fun x -> Some (x + 1)) (Some 1))"
        )
        .is_ok()
    );
}

#[test]
fn option_bind_requires_an_option_returning_function() {
    // The classic misuse: passing a plain a -> b (should be a -> Option b).
    assert_error_contains("let r = Option.bind (fun x -> x + 1) (Some 1)", "Option");
}

#[test]
fn accepts_numeric_conversions() {
    // round/floor/ceil/truncate : float<'u> -> int<'u>, unit-preserving.
    assert!(pyfun::check("let r = round 3.7").is_ok());
    assert!(pyfun::check("let r = floor 3.2\nlet s = ceil 3.2\nlet t = truncate 3.9").is_ok());
    assert!(pyfun::check("measure m\nlet r = round 2.5<m>\nlet ok = r + 1<m>").is_ok());
    // The result is an int, so it mixes with int arithmetic.
    assert!(pyfun::check("let r = floor 3.7 + 1").is_ok());
    // String.toFloat : string -> Option float.
    assert!(pyfun::check("let r = Option.withDefault 0.0 (String.toFloat \"3.14\")").is_ok());
}

#[test]
fn round_preserves_units() {
    // The unit rides through the conversion, so a metre stays a metre.
    assert_error_contains(
        "measure m\nmeasure s\nlet r = round 2.5<m>\nlet bad = r + 1<s>",
        "int<s>",
    );
}

#[test]
fn accepts_scientific_notation() {
    assert!(pyfun::check("let x = 1e6").is_ok());
    assert!(pyfun::check("let x = 2.5e-3").is_ok());
    // A number with an exponent is a float, so it mixes with float arithmetic.
    assert!(pyfun::check("let x = 1e6 + 2.0").is_ok());
    // Scientific notation with a unit annotation (the physics case).
    assert!(pyfun::check("measure m\nlet x = 3.0e8<m>").is_ok());
    // It's a float, not num-polymorphic — an int context rejects it.
    assert_error_contains("let r = List.get 1e6 [1, 2]", "float");
}

#[test]
fn accepts_list_completeness_ops() {
    assert!(pyfun::check("let r = List.get 0 [1, 2, 3]").is_ok()); // : Option int
    assert!(pyfun::check("let r = List.isEmpty [1]").is_ok());
    assert!(pyfun::check("let r = List.contains 2 [1, 2]").is_ok());
    assert!(pyfun::check("let r = List.concat [1] [2, 3]").is_ok());
    assert!(pyfun::check("let r = List.sort [3, 1, 2]").is_ok());
    assert!(pyfun::check("let r = List.sort [\"b\", \"a\"]").is_ok()); // strings orderable
    assert!(pyfun::check("let r = List.find (fun x -> x > 1) [1, 2, 3]").is_ok());
    // `get`/`find` yield `Option`, consumed by the Option module.
    assert!(pyfun::check("let r = Option.withDefault 0 (List.get 0 [1, 2])").is_ok());
}

#[test]
fn rejects_ill_typed_list_ops() {
    // `sort` carries the `comparison` constraint: bools aren't orderable.
    assert_error_contains("let r = List.sort [true, false]", "comparison");
    // `get`'s index is an int.
    assert_error_contains("let r = List.get \"x\" [1, 2]", "int");
    // `contains` element must match the list's element type.
    assert_error_contains("let r = List.contains \"a\" [1, 2]", "string");
}

#[test]
fn accepts_modulo() {
    assert!(pyfun::check("let r = 10 % 3").is_ok());
    assert!(pyfun::check("let isEven n = n % 2 == 0").is_ok());
    assert!(pyfun::check("let r = 5.5 % 2.0").is_ok()); // float modulo
    // Unit-preserving like `+`/`-`: `10<m> % 3<m> : int<m>`.
    assert!(pyfun::check("measure m\nlet r = 10<m> % 3<m>\nlet ok = r + 1<m>").is_ok());
}

#[test]
fn rejects_ill_typed_modulo() {
    // Mixed units don't unify.
    assert_error_contains("measure m\nmeasure s\nlet r = 10<m> % 3<s>", "int<s>");
    // Non-numeric operands are rejected (the `num` constraint).
    assert_error_contains("let r = \"a\" % \"b\"", "int");
}

#[test]
fn accepts_chained_comparisons() {
    assert!(pyfun::check("let f x = 1 < x < 10").is_ok());
    assert!(pyfun::check("let g a b c = a <= b < c").is_ok());
    assert!(pyfun::check("let h = 1 == 1 == 1").is_ok());
    // Strings are orderable, so a string chain is fine.
    assert!(pyfun::check("let s = \"a\" < \"b\" < \"c\"").is_ok());
}

#[test]
fn rejects_ill_typed_chained_comparisons() {
    // A chain across types fails to unify.
    assert_error_contains("let x = 1 < 2 < true", "bool");
    // An ordering link needs the `comparison` constraint (bool isn't orderable).
    assert_error_contains("let x = true < false < true", "comparison");
}

#[test]
fn accepts_unary_minus() {
    // Negative literals, negation of expressions, and in argument position.
    assert!(pyfun::check("let a = -5").is_ok());
    assert!(pyfun::check("let a = -3 + 10\nlet b = 2 * -3").is_ok());
    assert!(pyfun::check("let a = abs (-5)").is_ok());
    assert!(pyfun::check("let neg x = -x\nlet a = neg 3").is_ok());
    // Unit-preserving: `-5<m>` stays `int<m>`, so it adds to `3<m>`.
    assert!(pyfun::check("measure m\nlet d = -5<m>\nlet e = d + 3<m>").is_ok());
    // A negative integer literal pattern.
    assert!(
        pyfun::check("let f n =\n  match n:\n    case -1: 0\n    case _: 1").is_ok(),
        "negative literal pattern should type-check"
    );
}

#[test]
fn rejects_negation_of_non_numeric() {
    assert_error_contains("let x = -true", "int");
    assert_error_contains("let x = -\"hi\"", "int");
}

#[test]
fn operator_section_carries_the_operators_constraint() {
    // `(+)` inherits the `num` constraint, so applying it to bools is rejected.
    assert_error_contains("let r = (+) true false", "int");
    // `(<)` inherits `comparison`: bool is not orderable.
    assert_error_contains("let r = (<) true false", "comparison");
}

#[test]
fn accepts_match_over_int_literals() {
    assert!(
        pyfun::check("let f n =\n  match n:\n    case 0: \"zero\"\n    case _: \"many\"").is_ok()
    );
}

#[test]
fn accepts_user_defined_adt() {
    // `Option`/`Some`/`None` are built-in (no user declaration needed).
    let src = "let unwrap o =\n\
               \x20 match o:\n\
               \x20 \x20 case Some v: v\n\
               \x20 \x20 case None: 0\n\
               let r = unwrap (Some 5)";
    assert!(pyfun::check(src).is_ok());
}

#[test]
fn accepts_parameterized_and_recursive_adts() {
    // `List` is a built-in collection now, so the recursive cons-list ADT here is
    // named `Lst`.
    let src = "type Either a b = Left a | Right b\n\
               type Lst a = Nil | Cons a (Lst a)\n\
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
        "let f n =\n  match n:\n    case 0: 1\n    case _: \"two\"",
        "expected int, found string",
    );
}

#[test]
fn rejects_unknown_constructor() {
    assert_error_contains(
        "let f o =\n  match o:\n    case Nope v: v",
        "unknown constructor `Nope`",
    );
}

#[test]
fn rejects_non_exhaustive_adt_match() {
    // `Option` is built-in; matching only `Some` misses `None`.
    let src = "let f o =\n  match o:\n    case Some v: v";
    assert_error_contains(src, "non-exhaustive match: `None` is not matched");
}

#[test]
fn rejects_non_exhaustive_int_match() {
    assert_error_contains("let f n =\n  match n:\n    case 0: 1", "add a wildcard");
}

// ---------- deep (nested) exhaustiveness ----------

#[test]
fn accepts_deep_exhaustive_nested_adt() {
    // Covering every nested combination is exhaustive without a wildcard.
    let src = "let f o =\n\
               \x20 match o:\n\
               \x20 \x20 case Some true: 1\n\
               \x20 \x20 case Some false: 2\n\
               \x20 \x20 case None: 3";
    assert!(pyfun::check(src).is_ok());
}

#[test]
fn accepts_deep_exhaustive_record_with_nested_ctors() {
    // The motivating case: `{ item = Some n } | { item = None }` is complete.
    let src = "type Box = { item: Option int }\n\
               let f b =\n\
               \x20 match b:\n\
               \x20 \x20 case Box { item = Some n }: n\n\
               \x20 \x20 case Box { item = None }: 0";
    assert!(pyfun::check(src).is_ok());
}

#[test]
fn reports_nested_witness_for_missing_inner_constructor() {
    // Missing `Some false` — the witness names the precise nested hole.
    assert_error_contains(
        "let f o =\n\
         \x20 match o:\n\
         \x20 \x20 case Some true: 1\n\
         \x20 \x20 case None: 2",
        "`Some false` is not matched",
    );
}

#[test]
fn reports_record_witness_for_missing_field_combination() {
    assert_error_contains(
        "type P = { x: bool, y: bool }\n\
         let f p =\n\
         \x20 match p:\n\
         \x20 \x20 case P { x = true, y = true }: 1\n\
         \x20 \x20 case P { x = true, y = false }: 2\n\
         \x20 \x20 case P { x = false, y = true }: 3",
        "`P { x = false, y = false }` is not matched",
    );
}

#[test]
fn deep_check_still_requires_wildcard_for_open_inner_type() {
    // The inner `int` is infinite, so listing some literals isn't exhaustive.
    assert_error_contains(
        "let f o =\n\
         \x20 match o:\n\
         \x20 \x20 case Some 0: 1\n\
         \x20 \x20 case None: 2",
        "non-exhaustive",
    );
}

#[test]
fn rejects_constructor_used_at_wrong_type() {
    // `Some` produces an `Option`, not an `int`.
    let src = "let r = (Some 1) + 1";
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

// ---------- user-defined CE builders ----------

const MAYBE_BUILDER: &str = "module Maybe =\n\
    \x20 let bind m f =\n\
    \x20 \x20 match m:\n\
    \x20 \x20 \x20 case Some x: f x\n\
    \x20 \x20 \x20 case None: None\n\
    \x20 let return_ x = Some x\n\
    \x20 let returnFrom m = m\n";

#[test]
fn accepts_user_monad_ce() {
    let src = format!(
        "{MAYBE_BUILDER}\
         let safe a b =\n\
         \x20 Maybe {{\n\
         \x20 \x20 let! x = a\n\
         \x20 \x20 let! y = b\n\
         \x20 \x20 return x + y\n\
         \x20 }}"
    );
    assert!(pyfun::check(&src).is_ok(), "{:?}", pyfun::check(&src).err());
}

#[test]
fn user_ce_threads_types_through_bind() {
    // `bind`/`return_` pin the result type: `safe : Option int -> Option int -> Option int`.
    let src = format!(
        "{MAYBE_BUILDER}\
         let safe a b =\n\
         \x20 Maybe {{\n\
         \x20 \x20 let! x = a\n\
         \x20 \x20 return x + 1\n\
         \x20 }}\n\
         let bad = safe (Some true) None"
    );
    // `x + 1` forces `x : int`, so `Some true` is a type error.
    assert_error_contains(&src, "int");
}

#[test]
fn rejects_user_ce_missing_builder_member() {
    let src = "module Bad =\n\
        \x20 let return_ x = Some x\n\
        let r =\n\
        \x20 Bad {\n\
        \x20 \x20 let! x = Some 1\n\
        \x20 \x20 return x\n\
        \x20 }";
    assert_error_contains(src, "`bind` is not a member of `Bad`");
}

#[test]
fn rejects_user_ce_return_before_end() {
    let src = format!(
        "{MAYBE_BUILDER}\
         let r =\n\
         \x20 Maybe {{\n\
         \x20 \x20 return 1\n\
         \x20 \x20 return 2\n\
         \x20 }}"
    );
    assert_error_contains(&src, "must be the final item");
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

// ---------- derived-measure aliases ----------

#[test]
fn accepts_derived_measure_alias() {
    // `N` expands to `kg m / s^2`; `force / area` is dimensionally `N / m^2`.
    let src = "measure kg\nmeasure m\nmeasure s\n\
               measure N = kg m / s^2\n\
               let force = 10<N>\n\
               let area = 2<m^2>\n\
               let pressure = force // area";
    assert!(pyfun::check(src).is_ok(), "{:?}", pyfun::check(src).err());
}

#[test]
fn alias_unifies_with_its_expansion() {
    // A value in `<N>` and one in `<kg m / s^2>` have the same dimension.
    let src = "measure kg\nmeasure m\nmeasure s\n\
               measure N = kg m / s^2\n\
               let a = 1<N>\n\
               let b = 1<kg m / s^2>\n\
               let same = a == b";
    assert!(pyfun::check(src).is_ok(), "{:?}", pyfun::check(src).err());
}

#[test]
fn alias_can_reference_another_alias() {
    let src = "measure kg\nmeasure m\nmeasure s\n\
               measure N = kg m / s^2\n\
               measure Pa = N / m^2\n\
               let p = 5<Pa>";
    assert!(pyfun::check(src).is_ok(), "{:?}", pyfun::check(src).err());
}

#[test]
fn derived_measure_dimension_mismatch_shows_expansion() {
    // The alias displays expanded (no abbreviation tracking in the MVP).
    assert_error_contains(
        "measure kg\nmeasure m\nmeasure s\nmeasure N = kg m / s^2\nlet bad = 1<N> + 1<m>",
        "expected int<kg m/s^2>, found int<m>",
    );
}

#[test]
fn rejects_unknown_measure_in_alias_body() {
    assert_error_contains("measure m\nmeasure Bad = m / xyz", "unknown measure `xyz`");
}

#[test]
fn rejects_alias_used_before_definition() {
    // Measures, like `let`s, must be declared before use.
    assert_error_contains(
        "measure m\nmeasure s\nmeasure Speed = Accel s\nmeasure Accel = m / s^2",
        "unknown measure `Accel`",
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
    let src = "let f n =\n  match n:\n    case 0: n\n    case _: n\nlet r = 1.0 + f 0";
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
    let src = "let r = Some 1 == Some 2";
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
               let p = Point { x = 3, y = 4 }\n\
               let q = { p with y = 9 }\n\
               let s = p.x + q.y";
    assert!(pyfun::check(src).is_ok());
}

#[test]
fn record_field_access_drives_function_type() {
    // `r.x + r.y` forces `r : Point` even without an annotation.
    let src = "type Point = { x: int, y: int }\n\
               let sumxy r = r.x + r.y\n\
               let t = sumxy (Point { x = 1, y = 2 })";
    assert!(pyfun::check(src).is_ok());
}

#[test]
fn parameterized_record_is_polymorphic() {
    // A `Box a` literal/access used at two element types in one program.
    let src = "type Box a = { item: a }\n\
               let mk v = Box { item = v }\n\
               let bi = (mk 5).item\n\
               let bs = (mk \"hi\").item";
    assert!(pyfun::check(src).is_ok());
}

#[test]
fn rejects_missing_record_field() {
    assert_error_contains(
        "type Point = { x: int, y: int }\nlet p = Point { x = 1 }",
        "missing field",
    );
}

#[test]
fn rejects_unknown_field_in_literal() {
    assert_error_contains(
        "type Point = { x: int, y: int }\nlet p = Point { x = 1, y = 2, z = 3 }",
        "has no field `z`",
    );
}

#[test]
fn rejects_unknown_field_access() {
    assert_error_contains(
        "type Point = { x: int }\nlet p = Point { x = 1 }\nlet z = p.nope",
        "unknown record field `nope`",
    );
}

#[test]
fn rejects_wrong_record_field_type() {
    assert_error_contains(
        "type Point = { x: int, y: int }\nlet p = Point { x = 1, y = true }",
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

// ---------- record patterns ----------

#[test]
fn accepts_record_pattern_match() {
    // `{ x, y }` shorthand binds both fields; field types flow into the body.
    let src = "type Point = { x: int, y: int }\n\
               let sumxy p =\n\
               \x20 match p:\n\
               \x20 \x20 case Point { x, y }: x + y";
    assert!(pyfun::check(src).is_ok());
}

#[test]
fn record_pattern_binds_fields_at_their_types() {
    // The bound `n` is an `int`, so `n + 1` checks; the catch-all is exhaustive.
    let src = "type Box = { item: int }\n\
               let f b =\n\
               \x20 match b:\n\
               \x20 \x20 case Box { item = n }: n + 1";
    assert!(pyfun::check(src).is_ok());
}

#[test]
fn record_pattern_may_mention_a_subset_of_fields() {
    let src = "type Point = { x: int, y: int }\n\
               let onlyx p =\n\
               \x20 match p:\n\
               \x20 \x20 case Point { x }: x";
    assert!(pyfun::check(src).is_ok());
}

#[test]
fn irrefutable_record_pattern_is_exhaustive() {
    // A record pattern whose fields are all irrefutable is itself a catch-all.
    let src = "type Point = { x: int, y: int }\n\
               let f p =\n\
               \x20 match p:\n\
               \x20 \x20 case Point { x = a, y = b }: a + b";
    assert!(pyfun::check(src).is_ok());
}

#[test]
fn rejects_refutable_record_pattern_without_wildcard() {
    // Shallow exhaustiveness: refutable record patterns don't cover the type.
    assert_error_contains(
        "type Point = { x: int, y: int }\n\
         let f p =\n\
         \x20 match p:\n\
         \x20 \x20 case Point { x = 0 }: 1\n\
         \x20 \x20 case Point { x = 1 }: 2",
        "non-exhaustive",
    );
}

#[test]
fn rejects_unknown_field_in_pattern() {
    // `x` resolves the owner record; `z` is then rejected against it.
    assert_error_contains(
        "type Point = { x: int, y: int }\n\
         let f p =\n\
         \x20 match p:\n\
         \x20 \x20 case Point { x = 0, z = 1 }: 1\n\
         \x20 \x20 case _: 0",
        "has no field `z`",
    );
}

#[test]
fn rejects_record_pattern_with_no_known_field() {
    // When even the first field is unknown, the owner can't be resolved.
    assert_error_contains(
        "type Point = { x: int, y: int }\n\
         let f p =\n\
         \x20 match p:\n\
         \x20 \x20 case Point { z = 0 }: 1\n\
         \x20 \x20 case _: 0",
        "has no field `z`",
    );
}

#[test]
fn rejects_field_matched_twice_in_pattern() {
    assert_error_contains(
        "type Point = { x: int, y: int }\n\
         let f p =\n\
         \x20 match p:\n\
         \x20 \x20 case Point { x = 0, x = 1 }: 1\n\
         \x20 \x20 case _: 0",
        "matched twice",
    );
}

#[test]
fn rejects_record_pattern_field_type_mismatch() {
    // `x : int`, so a `true` sub-pattern is a type error.
    assert_error_contains(
        "type Point = { x: int, y: int }\n\
         let f p =\n\
         \x20 match p:\n\
         \x20 \x20 case Point { x = true }: 1\n\
         \x20 \x20 case _: 0",
        "int",
    );
}

#[test]
fn rejects_update_of_unrelated_field() {
    // `y` belongs to a different record than `Point`, so the update mixes types.
    assert_error_contains(
        "type Point = { x: int }\n\
         type Other = { y: int }\n\
         let p = Point { x = 1 }\n\
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

// ---------- effects (DESIGN §4) ----------

#[test]
fn accepts_pure_binding_with_no_effects() {
    assert!(pyfun::check("let pure add a b = a + b\nlet r = add 1 2").is_ok());
    assert!(pyfun::check("let pure clamp lo hi x = max lo (min hi x)").is_ok());
}

#[test]
fn rejects_pure_binding_that_prints() {
    assert_error_contains(
        "let pure greet n = print n",
        "declared `pure` but performs `io`",
    );
}

#[test]
fn rejects_pure_binding_that_mutates() {
    assert_error_contains(
        "let pure f x =\n    let mut acc = x\n    acc <- acc + 1\n    acc",
        "declared `pure` but performs `io`",
    );
}

#[test]
fn io_propagates_through_calls() {
    // `bad` performs no io directly, but calls an impure helper → impure.
    assert_error_contains(
        "let shout n = print n\nlet pure bad n = shout n",
        "declared `pure` but performs `io`",
    );
}

#[test]
fn pure_calling_a_pure_helper_is_ok() {
    assert!(pyfun::check("let dbl x = x + x\nlet pure good x = dbl x").is_ok());
}

#[test]
fn higher_order_function_is_effect_polymorphic() {
    // `apply` itself introduces no io (pure up to its argument), so it may be
    // declared `pure` even though `apply print` would be impure at the call site.
    assert!(pyfun::check("let pure apply f x = f x\nlet r = apply print 5").is_ok());
}

#[test]
fn pure_higher_order_with_impure_argument_at_call_site_is_fine() {
    // Effect polymorphism: the same pure `twice` works with a pure or impure
    // function (`log` prints then returns its argument, so it is `'a -> 'a` io).
    let src = "let pure twice f x = f (f x)\n\
               let inc a = a + 1\n\
               let log a =\n    print a\n    a\n\
               let r1 = twice inc 0\n\
               let r2 = twice log 0";
    assert!(pyfun::check(src).is_ok());
}

// ---------- lists (the eager `List` collection, `DESIGN.md` §6) ----------

#[test]
fn accepts_list_literal_and_functions() {
    let src = "let xs = [1, 2, 3]\n\
               let ys = List.map (fun x -> x + 1) xs\n\
               let zs = List.filter (fun x -> x < 2) xs\n\
               let t = List.fold (fun a b -> a + b) 0 xs\n\
               let n = List.len xs\n\
               let s = List.sum xs\n\
               let r = List.rev xs\n\
               let q = List.range 0 5";
    assert!(pyfun::check(src).is_ok());
}

#[test]
fn empty_list_is_polymorphic() {
    // The same `[]`-derived helper is usable at two element types.
    assert!(pyfun::check("let e = []\nlet a = List.len [1]\nlet b = List.len [\"x\"]").is_ok());
}

#[test]
fn rejects_heterogeneous_list() {
    assert_error_contains("let bad = [1, true]", "expected int, found bool");
}

// ---------- tuples ----------

#[test]
fn accepts_tuple_construct_and_destructure() {
    let src = "let pair = (1, true)\n\
               let swap p =\n\
               \x20 match p:\n\
               \x20 \x20 case (a, b): (b, a)\n\
               let s = swap pair";
    assert!(pyfun::check(src).is_ok());
}

#[test]
fn tuple_elements_keep_their_own_types() {
    // The first element is int, the second string; using the second as a number
    // is an error.
    assert_error_contains(
        "let t = (1, \"x\")\n\
         let second =\n\
         \x20 match t:\n\
         \x20 \x20 case (a, b): b\n\
         let bad = second + 1",
        "string",
    );
}

#[test]
fn rejects_tuple_arity_mismatch() {
    // A 2-tuple cannot unify with a 3-tuple.
    assert_error_contains(
        "let f p =\n\
         \x20 match p:\n\
         \x20 \x20 case (a, b): a\n\
         let bad = f (1, 2, 3)",
        "found",
    );
}

#[test]
fn tuple_type_displays_with_parentheses() {
    // A pair of ints prints as `(int, int)`.
    assert_error_contains("let bad = (1, 2) + 3", "(int, int)");
}

#[test]
fn tuple_match_is_exhaustive_without_wildcard() {
    // A single tuple pattern of variables covers all values (one constructor).
    assert!(pyfun::check("let fst p =\n  match p:\n    case (a, b): a").is_ok());
}

#[test]
fn rejects_non_exhaustive_tuple_match_with_witness() {
    // Deep exhaustiveness recurses into element columns and reports a witness.
    assert_error_contains(
        "let f p =\n  match p:\n    case (true, b): b",
        "`(false, _)` is not matched",
    );
}

#[test]
fn accepts_tuple_in_record_field_and_extern() {
    let src = "type Pair = { both: (int, string) }\n\
               extern pure mk : a -> b -> (a, b) = builtins.tuple\n\
               let p = Pair { both = (1, \"x\") }";
    assert!(pyfun::check(src).is_ok());
}

#[test]
fn rejects_len_on_a_non_list() {
    // `List.len : List a -> int`, so an int argument is a type error.
    assert_error_contains("let n = List.len 5", "List");
}

#[test]
fn map_preserves_element_type_change() {
    // `List.map (int -> string)` over a `List int` yields a `List string`, so
    // summing the result (which needs `List int`) is a type error.
    assert_error_contains(
        "extern show: a -> string = str\nlet bad = List.sum (List.map show [1, 2])",
        "string",
    );
}

#[test]
fn map_of_a_pure_function_stays_pure() {
    assert!(pyfun::check("let pure inc xs = List.map (fun x -> x + 1) xs").is_ok());
}

#[test]
fn map_of_an_impure_function_is_impure() {
    // Effect polymorphism: mapping `print` makes the whole call `io`, so a `pure`
    // binding must be rejected.
    assert_error_contains(
        "let pure shoutAll xs = List.map (fun x -> print x) xs",
        "declared `pure` but performs `io`",
    );
}

#[test]
fn rejects_redefining_builtin_list() {
    assert_error_contains("type List a = Empty | More a", "already defined");
}

// ---------- sets & maps (the hashed collections, `DESIGN.md` §6) ----------

#[test]
fn accepts_set_functions() {
    let src = "let s = Set.ofList [1, 2, 3]\n\
               let s2 = Set.add 4 s\n\
               let s3 = Set.remove 1 s2\n\
               let has = Set.contains 2 s3\n\
               let n = Set.len s3\n\
               let u = Set.union s s2\n\
               let i = Set.intersect s s2\n\
               let d = Set.difference s s2\n\
               let xs = Set.toList (Set.union s Set.empty)";
    assert!(pyfun::check(src).is_ok());
}

#[test]
fn accepts_map_functions() {
    let src = "let m = Map.add \"a\" 1 (Map.add \"b\" 2 Map.empty)\n\
               let v = Map.findOr \"a\" 0 m\n\
               let o = Map.tryFind \"a\" m\n\
               let m2 = Map.remove \"b\" m\n\
               let has = Map.contains \"a\" m2\n\
               let n = Map.len m2\n\
               let ks = Map.keys m\n\
               let vs = Map.values m";
    assert!(pyfun::check(src).is_ok());
}

#[test]
fn try_yields_a_result_over_exception() {
    // `try e : Result <e> Exception`; the Ok payload is the body's type, the Error
    // payload is the reserved `Exception` record (accessed by field).
    let src = "extern parseInt : string -> int = int\n\
               let describe s =\n\
               \x20 match try (parseInt s):\n\
               \x20 \x20 case Ok n: n\n\
               \x20 \x20 case Error e: String.len e.errorKind";
    assert!(pyfun::check(src).is_ok());
}

#[test]
fn try_body_type_flows_into_ok() {
    // The Ok arm binds the body's type — here `string`, so String ops apply.
    let src = "extern readFile : string -> string = open\n\
               let firstChar path =\n\
               \x20 match try (readFile path):\n\
               \x20 \x20 case Ok contents: String.upper contents\n\
               \x20 \x20 case Error e: e.errorMessage";
    assert!(pyfun::check(src).is_ok());
}

#[test]
fn exception_type_is_reserved() {
    // A user `type Exception` collides with the reserved built-in.
    assert!(pyfun::check("type Exception = Boom").is_err());
}

#[test]
fn string_literal_patterns_typecheck_over_strings() {
    let src = "let classify s =\n\
               \x20 match s:\n\
               \x20 \x20 case \"yes\": 1\n\
               \x20 \x20 case \"no\": 0\n\
               \x20 \x20 case _: 2";
    assert!(pyfun::check(src).is_ok());
    // Matching a string literal against a non-string scrutinee is a type error.
    assert!(
        pyfun::check("let f n =\n  match n:\n    case \"x\": 1\n    case _: 0\nlet r = f 5").is_err()
    );
}

#[test]
fn string_match_needs_a_wildcard_to_be_exhaustive() {
    // `string` is infinite, so enumerating literals is never exhaustive.
    assert!(
        pyfun::check("let f s =\n  match s:\n    case \"a\": 1\n    case \"b\": 2").is_err()
    );
}

#[test]
fn interpolated_string_has_type_string() {
    // The whole f-string is a `string`; holes may be any type (int, tuple, ADT).
    let src = "let name = \"Ada\"\n\
               let n = 3\n\
               let p = (1, 2)\n\
               let msg = f\"hi {name}, n={n}, sum={n + 1}, p={p}\"\n\
               let len = String.len msg";
    assert!(pyfun::check(src).is_ok());
}

#[test]
fn interpolation_hole_must_be_well_typed() {
    // A hole is an ordinary expression, so an unbound name in it is an error.
    assert!(pyfun::check("let m = f\"value {missing}\"").is_err());
    // A type error inside a hole is caught too (adding a string to an int).
    assert!(pyfun::check("let m = f\"{1 + \"x\"}\"").is_err());
}

#[test]
fn interpolation_debug_hole_is_an_ordinary_hole() {
    // `{x=}` / `{x = }` echo their source text; the expression itself is checked
    // as usual, so the whole string stays a `string` (any hole type is fine).
    let src = "let x = 3\n\
               let a = 4\n\
               let b = 5\n\
               let m = f\"{x=} {a + b = }\"\n\
               let len = String.len m";
    assert!(pyfun::check(src).is_ok());
    // An unbound name in a debug hole is still an error.
    assert!(pyfun::check("let m = f\"{missing=}\"").is_err());
    // A trailing `==` is a comparison, not a debug marker: `x == y` needs `y`.
    assert!(pyfun::check("let x = 1\nlet m = f\"{x==y}\"").is_err());
    assert!(pyfun::check("let x = 1\nlet y = 2\nlet m = f\"{x==y}\"").is_ok());
}

#[test]
fn interpolation_propagates_hole_effects() {
    // A pure function may be asserted `pure` even when it builds an f-string from
    // pure holes...
    assert!(pyfun::check("let pure label n = f\"n = {n}\"").is_ok());
    // ...but a hole performing `io` (here `print`, via its unit result) makes the
    // whole expression impure, so `pure` is rejected.
    assert!(pyfun::check("let pure shout x = f\"{print x}\"").is_err());
}

#[test]
fn accepts_string_functions() {
    let src = "let g = String.concat \"Hello, \" \"World\"\n\
               let n = String.len g\n\
               let up = String.upper g\n\
               let lo = String.lower g\n\
               let t = String.strip \"  x  \"\n\
               let parts = String.split \",\" g\n\
               let joined = String.join \" | \" parts\n\
               let has = String.contains \"World\" g\n\
               let sw = String.startsWith \"He\" g\n\
               let ew = String.endsWith \"ld\" g\n\
               let r = String.replace \"l\" \"L\" g\n\
               let s1 = String.fromInt 7\n\
               let s2 = String.fromFloat 3.5\n\
               let chars = String.toList g";
    assert!(pyfun::check(src).is_ok());
}

#[test]
fn string_to_int_returns_an_option() {
    // `String.toInt : string -> Option int` — the result must match/destructure as an
    // Option, and its payload is an int (usable in arithmetic).
    let src = "let bump o =\n\
               \x20 match o:\n\
               \x20 \x20 case Some n: n + 1\n\
               \x20 \x20 case None: 0\n\
               let r = bump (String.toInt \"41\")";
    assert!(pyfun::check(src).is_ok());
}

#[test]
fn string_len_rejects_a_non_string() {
    // `String.len : string -> int`; applying it to an int is a type error.
    assert!(pyfun::check("let n = String.len 5").is_err());
}

#[test]
fn unknown_member_suggests_the_closest_name() {
    // Casing slip (camelCase), pure-casing shout, and abbreviation confusion all get
    // a "did you mean" pointing at the one canonical spelling.
    assert_error_contains(
        "let r = String.startswith \"a\" \"b\"",
        "did you mean `String.startsWith`?",
    );
    assert_error_contains("let r = String.UPPER \"x\"", "did you mean `String.upper`?");
    assert_error_contains("let n = String.length \"x\"", "did you mean `String.len`?");
}

#[test]
fn unknown_member_with_no_close_name_gives_no_suggestion() {
    // A genuinely absent function isn't force-fit to a bad suggestion.
    let msgs = errors("let r = String.frobnicate \"x\"");
    assert!(
        msgs.iter().any(|m| m.contains("is not a member of `String`")),
        "{msgs:?}"
    );
    assert!(
        !msgs.iter().any(|m| m.contains("did you mean")),
        "{msgs:?}"
    );
}

#[test]
fn did_you_mean_works_for_user_modules_too() {
    // The suggestion scans qualified env keys, so user modules benefit as well.
    assert_error_contains(
        "module Geom =\n  let area w h = w * h\nlet x = Geom.aera 2 3",
        "did you mean `Geom.area`?",
    );
}

#[test]
fn list_zip_pairs_elements_into_tuples() {
    // `List.zip : List a -> List b -> List (a, b)`. Destructuring a zipped pair
    // recovers both element types.
    let src = "let firstOf p =\n\
               \x20 match p:\n\
               \x20 \x20 case (n, s): n\n\
               let ps = List.zip [1, 2] [\"a\", \"b\"]\n\
               let firsts = List.map firstOf ps\n\
               let total = List.sum firsts";
    assert!(pyfun::check(src).is_ok());
}

#[test]
fn map_of_list_and_to_list_round_trip_through_pairs() {
    // `Map.ofList : List (k, v) -> Map k v` and `Map.toList` its inverse.
    let src = "let m = Map.ofList (List.zip [\"a\", \"b\"] [1, 2])\n\
               let v = Option.withDefault 0 (Map.tryFind \"a\" m)\n\
               let pairs = Map.toList m";
    assert!(pyfun::check(src).is_ok());
}

#[test]
fn rejects_map_of_list_from_a_non_pair_list() {
    // `Map.ofList` needs a list of pairs, not a list of ints.
    assert_error_contains("let bad = Map.ofList [1, 2, 3]", "found int");
}

#[test]
fn set_element_type_is_enforced() {
    // `Set.add : a -> Set a -> Set a`, so adding a string to a `Set int` fails.
    assert_error_contains(
        "let bad = Set.add \"x\" (Set.ofList [1, 2])",
        "expected string, found int",
    );
}

#[test]
fn map_find_or_default_must_match_the_value_type() {
    // `Map.findOr : k -> v -> Map k v -> v`; an int default against a string-valued
    // map is a type error.
    assert_error_contains(
        "let m = Map.add \"a\" \"x\" Map.empty\nlet bad = Map.findOr \"a\" 0 m",
        "string",
    );
}

#[test]
fn try_find_returns_an_option() {
    // `Map.tryFind : k -> Map k v -> Option v`, so `Option.withDefault` accepts it.
    assert!(
        pyfun::check(
            "let m = Map.add \"a\" 1 Map.empty\n\
             let v = Option.withDefault 0 (Map.tryFind \"a\" m)"
        )
        .is_ok()
    );
}

#[test]
fn empty_collections_are_polymorphic() {
    // The generalized nullary values work at independent element/key types.
    assert!(
        pyfun::check(
            "let a = Set.len (Set.add 1 Set.empty)\n\
             let b = Set.len (Set.add \"x\" Set.empty)\n\
             let c = Map.len (Map.add 1 true Map.empty)"
        )
        .is_ok()
    );
}

#[test]
fn bare_module_name_is_an_error() {
    // A module is not a value; using it without a member is a clear error.
    assert_error_contains("let x = List", "`List` is a module");
}

#[test]
fn unknown_module_member_is_rejected() {
    assert_error_contains("let x = List.nope [1]", "not a member");
}

#[test]
fn rejects_redefining_builtin_option() {
    assert_error_contains("type Option a = None | Some a", "already defined");
}

#[test]
fn accepts_result_module() {
    let src = "let r = Ok 1\n\
               let a = Result.map (fun x -> x + 1) r\n\
               let b = Result.mapError (fun e -> e) r\n\
               let c = Result.bind (fun x -> Ok (x + 1)) r\n\
               let d = Result.withDefault 0 r\n\
               let e = Result.isOk r\n\
               let f = Result.isError r\n\
               let g = Result.toOption r";
    assert!(pyfun::check(src).is_ok());
}

#[test]
fn result_map_changes_the_ok_type() {
    // `Result.map (int -> string)` over a `Result int e` yields `Result string e`,
    // so a later int default is a type error.
    assert_error_contains(
        "extern show: a -> string = str\n\
         let r = Ok 1\n\
         let bad = Result.withDefault 0 (Result.map show r)",
        "string",
    );
}

#[test]
fn result_to_option_bridges_to_option() {
    // `Result.toOption : Result a e -> Option a`, consumed by the `Option` module.
    assert!(pyfun::check("let v = Option.withDefault 0 (Result.toOption (Ok 1))").is_ok());
}

#[test]
fn accepts_seq_module() {
    let src = "let s = Seq.ofList [1, 2, 3]\n\
               let a = Seq.map (fun x -> x + 1) s\n\
               let b = Seq.filter (fun x -> x < 2) s\n\
               let c = Seq.take 2 s\n\
               let d = Seq.fold (fun acc x -> acc + x) 0 s\n\
               let e = Seq.toList (Seq.range 0 5)";
    assert!(pyfun::check(src).is_ok());
}

#[test]
fn seq_take_returns_a_seq_not_a_list() {
    // `Seq.take : int -> Seq a -> Seq a`; the result is still lazy, so `List.len`
    // (which needs a `List`) is a type error until `Seq.toList` forces it.
    assert_error_contains("let bad = List.len (Seq.take 2 (Seq.range 0 9))", "List");
}

#[test]
fn seq_and_list_are_distinct_types() {
    // The same member name resolves per-module: `Seq.map` keeps a `Seq`, so feeding
    // it to a `List` consumer is a type error.
    assert_error_contains(
        "let bad = List.len (Seq.map (fun x -> x) (Seq.range 0 3))",
        "List",
    );
}

#[test]
fn rejects_redefining_builtin_map_and_set() {
    assert_error_contains("type Set a = Empty", "already defined");
    assert_error_contains("type Map a = Empty", "already defined");
}

// ---------- in-file modules (`DESIGN.md` §6) ----------

#[test]
fn accepts_module_and_qualified_access() {
    let src = "module Geometry =\n  let pi = 3\n  let area r = pi * r * r\n\
               let big = Geometry.area 10";
    assert!(pyfun::check(src).is_ok());
}

#[test]
fn module_members_see_siblings_unqualified() {
    // `double` calls `area` (a sibling) bare; `area` reads `pi` bare.
    let src = "module M =\n  let pi = 3\n  let area r = pi * r * r\n  let double a = area a + area a\n\
               let r = M.double 2";
    assert!(pyfun::check(src).is_ok());
}

#[test]
fn module_members_are_not_visible_unqualified_outside() {
    // `area` is only `M.area` outside the module.
    assert_error_contains(
        "module M =\n  let area r = r\nlet bad = area 1",
        "unbound name `area`",
    );
}

#[test]
fn qualified_member_type_is_enforced() {
    // `M.inc : int -> int`, so applying it to a bool is a type error.
    assert_error_contains(
        "module M =\n  let inc n = n + 1\nlet bad = M.inc true",
        "expected int, found bool",
    );
}

#[test]
fn unknown_module_member_is_rejected_for_user_modules() {
    assert_error_contains(
        "module M =\n  let x = 1\nlet bad = M.nope",
        "not a member of `M`",
    );
}

#[test]
fn bare_user_module_reference_is_an_error() {
    assert_error_contains("module M =\n  let x = 1\nlet bad = M", "`M` is a module");
}

#[test]
fn rejects_redefining_a_builtin_module() {
    assert_error_contains(
        "module List =\n  let x = 1",
        "cannot redefine built-in module `List`",
    );
}

#[test]
fn rejects_duplicate_module() {
    assert_error_contains(
        "module M =\n  let x = 1\nmodule M =\n  let y = 2",
        "module `M` is already defined",
    );
}

// ---------- extern (typed Python imports, `DESIGN.md` §6) ----------

#[test]
fn accepts_extern_with_concrete_type() {
    assert!(pyfun::check("extern pure sqrt: float -> float = math.sqrt\nlet r = sqrt 2.0").is_ok());
}

#[test]
fn extern_type_is_generalized_over_its_variables() {
    // `show : a -> string` must be usable at two different argument types.
    let src = "extern show: a -> string = str\nlet a = show 1\nlet b = show true";
    assert!(pyfun::check(src).is_ok());
}

#[test]
fn extern_is_type_checked_at_the_call_site() {
    assert_error_contains(
        "extern pure sqrt: float -> float = math.sqrt\nlet bad = sqrt \"x\"",
        "expected float, found string",
    );
}

#[test]
fn extern_boundary_is_effectful_by_default() {
    // A plain `extern` is impure at full application, so a `pure` binding that
    // calls it must be rejected.
    assert_error_contains(
        "extern readLine: string -> string\nlet pure ask q = readLine q",
        "declared `pure` but performs `io`",
    );
}

#[test]
fn pure_extern_can_be_used_in_a_pure_binding() {
    assert!(
        pyfun::check("extern pure sqrt: float -> float = math.sqrt\nlet pure root x = sqrt x")
            .is_ok()
    );
}

#[test]
fn partial_application_of_an_extern_is_pure() {
    // The Python call only happens on full application, so a partial application
    // performs no effect — usable inside a `pure` binding.
    let src = "extern pow: float -> float -> float = math.pow\nlet pure twoTo = pow 2.0";
    assert!(pyfun::check(src).is_ok());
}

#[test]
fn rejects_extern_redefining_an_existing_name() {
    assert_error_contains(
        "extern print: string -> unit\nlet r = print \"hi\"",
        "already defined",
    );
}

// ---------- the compiler is the gatekeeper ----------

#[test]
fn compile_is_gated_on_type_checking() {
    // An ill-typed program must not produce Python.
    assert!(pyfun::compile("let add a b = a + b\nlet r = add 1 true").is_err());
    // A well-typed one still compiles.
    assert!(pyfun::compile("let add a b = a + b\nlet r = add 1 2").is_ok());
}
