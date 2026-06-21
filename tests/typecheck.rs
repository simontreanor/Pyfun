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
    // `Option`/`Some`/`None` are built-in (no user declaration needed).
    let src = "let unwrap o = match o with | Some v -> v | None -> 0\n\
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
        "let f n = match n with | 0 -> 1 | _ -> \"two\"",
        "expected int, found string",
    );
}

#[test]
fn rejects_unknown_constructor() {
    assert_error_contains(
        "let f o = match o with | Nope v -> v",
        "unknown constructor `Nope`",
    );
}

#[test]
fn rejects_non_exhaustive_adt_match() {
    // `Option` is built-in; matching only `Some` misses `None`.
    let src = "let f o = match o with | Some v -> v";
    assert_error_contains(src, "non-exhaustive match: missing `None`");
}

#[test]
fn rejects_non_exhaustive_int_match() {
    assert_error_contains("let f n = match n with | 0 -> 1", "add a wildcard");
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
