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
fn constructor_patterns_are_rejected_until_adts() {
    assert_error_contains(
        "let f o = match o with | Some v -> v | None -> 0",
        "constructor patterns",
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
