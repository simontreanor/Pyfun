//! Phase 1 acceptance tests.
//!
//! The headline guarantee is the parse→print→parse roundtrip: printing an AST
//! and reparsing it must yield a structurally identical AST. A handful of
//! shape assertions pin down the parts most likely to regress — currying and
//! operator precedence.

use pyfun::parse;
use pyfun::syntax::{BinOp, Expr, Item};

/// Programs exercising every Phase 1 construct.
const PROGRAMS: &[&str] = &[
    "let x = 1",
    "let mut y = 2",
    "let pi = 3.14",
    "let greeting = \"hello\\nworld\"",
    "let yes = true",
    "let add a b = a + b",
    "let r = 1 + 2 * 3 - 4 / 2",
    "let curried = f a b c",
    "let piped = x |> f |> g a",
    "let choose = if cond then a else b",
    "let compose = fun f g x -> f (g x)",
    "let describe n = match n with | 0 -> \"zero\" | _ -> \"many\"",
    "let unwrap o = match o with | Some v -> v | None -> 0",
    "let nested = match p with | Pair (Some a) b -> a | _ -> b",
    // A small multi-item module mixing definitions and a trailing expression.
    "let id x = x\nlet k = id 42\nk |> id",
];

#[test]
fn parse_print_parse_is_idempotent() {
    for src in PROGRAMS {
        let ast1 = parse(src).unwrap_or_else(|e| panic!("failed to parse {src:?}: {e}"));
        let printed = pyfun::ast::print_module(&ast1);
        let ast2 = parse(&printed).unwrap_or_else(|e| {
            panic!("failed to reparse {src:?}\nprinted as:\n{printed}\nerror: {e}")
        });
        assert_eq!(
            ast1, ast2,
            "roundtrip changed the AST for {src:?}\nprinted as:\n{printed}"
        );
    }
}

#[test]
fn application_is_left_associative_and_curried() {
    // `f a b` must be `App(App(f, a), b)`.
    let module = parse("let r = f a b").unwrap();
    let Item::Let(binding) = &module.items[0] else {
        panic!("expected a let binding")
    };
    let Expr::App { func, arg } = &binding.value else {
        panic!("expected an application")
    };
    assert_eq!(**arg, Expr::Var("b".to_string()));
    let Expr::App {
        func: inner_func,
        arg: inner_arg,
    } = &**func
    else {
        panic!("expected a nested application")
    };
    assert_eq!(**inner_func, Expr::Var("f".to_string()));
    assert_eq!(**inner_arg, Expr::Var("a".to_string()));
}

#[test]
fn pipe_binds_looser_than_application() {
    // `x |> f a` must be `Pipe(x, App(f, a))`, not `App(Pipe(x, f), a)`.
    let module = parse("let r = x |> f a").unwrap();
    let Item::Let(binding) = &module.items[0] else {
        panic!("expected a let binding")
    };
    let Expr::Pipe { lhs, rhs } = &binding.value else {
        panic!("expected a pipe")
    };
    assert_eq!(**lhs, Expr::Var("x".to_string()));
    assert!(
        matches!(**rhs, Expr::App { .. }),
        "rhs of pipe should be an application"
    );
}

#[test]
fn arithmetic_precedence() {
    // `1 + 2 * 3` must be `1 + (2 * 3)`.
    let module = parse("let r = 1 + 2 * 3").unwrap();
    let Item::Let(binding) = &module.items[0] else {
        panic!("expected a let binding")
    };
    let Expr::Binary {
        op: BinOp::Add,
        rhs,
        ..
    } = &binding.value
    else {
        panic!("expected an addition at the root")
    };
    assert!(
        matches!(**rhs, Expr::Binary { op: BinOp::Mul, .. }),
        "right operand of + should be the multiplication"
    );
}

#[test]
fn reports_errors_for_malformed_input() {
    assert!(parse("let = 1").is_err());
    assert!(parse("if x then y").is_err()); // missing else
    assert!(parse("match x with").is_err()); // no arms
    assert!(parse("(1 + 2").is_err()); // unbalanced paren
}
