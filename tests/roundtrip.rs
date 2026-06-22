//! Phase 1 acceptance tests.
//!
//! The headline guarantee is the parse→print→parse roundtrip: printing an AST
//! and reparsing it must yield a structurally identical AST. A handful of
//! shape assertions pin down the parts most likely to regress — currying and
//! operator precedence.

use pyfun::parse;
use pyfun::syntax::{BinOp, ExprKind, Item};

/// Programs exercising every Phase 1 construct.
const PROGRAMS: &[&str] = &[
    "let x = 1",
    "let mut y = 2",
    "let pi = 3.14",
    "let greeting = \"hello\\nworld\"",
    "let yes = true",
    // The unit value `()`.
    "let nothing = ()",
    "let noop x = ()",
    "let force f = f ()",
    "let add a b = a + b",
    "let r = 1 + 2 * 3 - 4 / 2",
    "let r = 7 // 2",
    // Comparison & equality (looser than arithmetic).
    "let r = 1 + 1 < 3",
    "let r = a == b",
    "let r = x <= y",
    "let cmp a b = a < b",
    // Boolean operators and prefix `not`.
    "let r = a or b and c",
    "let r = not a",
    "let r = not a == b",
    "let chk lo hi x = lo <= x and x <= hi",
    // `5<m>` (adjacent, in the units section below) is a unit annotation, whereas
    // `5 < m` (spaced) is a comparison — the printer keeps them distinct.
    "let r = 5 < m",
    "let curried = f a b c",
    "let piped = x |> f |> g a",
    "let choose = if cond then a else b",
    "let compose = fun f g x -> f (g x)",
    "let describe n = match n with | 0 -> \"zero\" | _ -> \"many\"",
    "let unwrap o = match o with | Some v -> v | None -> 0",
    "let nested = match p with | Pair (Some a) b -> a | _ -> b",
    // Record patterns: shorthand, explicit, subset, and nested sub-patterns. The
    // `{ x }` shorthand must print back as shorthand (not `{ x = x }`).
    "let f p = match p with | { x, y } -> x",
    "let f p = match p with | { x = a, y = b } -> a",
    "let f p = match p with | { x = 0, y } -> y | { x } -> x",
    "let f b = match b with | { item = Some n } -> n | _ -> 0",
    // A small multi-item module mixing definitions and a trailing expression.
    "let id x = x\nlet k = id 42\nk |> id",
    // Offside rule: an indented continuation keeps a multi-line item together.
    "let classify n =\n  match n with\n  | 0 -> \"zero\"\n  | _ -> \"many\"",
    // Blocks in match arms / if branches / lambda bodies (opened after `->`,
    // `then`/`else`). A single-statement block is unwrapped, so only multi-stmt
    // bodies stay blocks; the printer renders them with offside layout.
    "let f n =\n  match n with\n  | 0 ->\n      let x = 1\n      x\n  | _ -> 0",
    "let f c =\n  if c then\n      let x = 1\n      x\n  else 0",
    "let f c =\n  if c then 1\n  else\n      let y = 2\n      y",
    "let g = fun x ->\n  let y = x\n  y",
    "let f n =\n  match n with\n  | 0 ->\n      let a = 1\n      a\n  | _ ->\n      let b = 2\n      b",
    // Offside rule: consecutive bare statements are separate items.
    "print a\nprint b",
    // Blocks: indented `let` bodies with local bindings, sequencing, and `<-`.
    "let f x =\n    let y = x\n    y",
    "let sum3 a b c =\n    let mut acc = 0\n    acc <- acc + a\n    acc <- acc + b\n    acc",
    "let nested x =\n    let y =\n        let mut t = x\n        t <- t * 2\n        t\n    y",
    // Top-level mutable binding and reassignment (already-sequenced items).
    "let mut counter = 0\ncounter <- counter + 1",
    // `pure` modifier (effect assertion).
    "let pure add a b = a + b",
    "let pure apply f x = f x",
    // Records: declaration, literal, functional update, field access.
    "type Point = { x: int, y: int }",
    "type Box a = { item: a, tag: string }",
    "let p = { x = 3, y = 4 }",
    "let q = { p with y = 9 }",
    "let s = p.x",
    "let d = obj.inner.value",
    "let n = (mk a).field",
    // Type declarations: nullary, parameterized, and recursive.
    "type Color = Red | Green | Blue",
    "type Option a = None | Some a",
    "type Result a b = Ok a | Err b",
    "type List a = Nil | Cons a (List a)",
    "type Option a = None | Some a\nlet unwrap o = match o with | Some v -> v | None -> 0",
    // List literals: empty, simple, nested, and compound elements.
    "let xs = [1, 2, 3]",
    "let e = []",
    "let m = [[1, 2], [3, 4]]",
    "let c = [a + 1, f b, x]",
    "let mapped = map f [1, 2, 3]",
    // Computation expressions (built-in builders).
    "let a = seq { yield 1 yield! xs }",
    "let a = result { let! x = m return x }",
    "let a = async { let! x = m do! n return! r }",
    // User-defined CE builders (an uppercase module name before `{`). The CE-item
    // lookahead keeps `Some { x = 1 }` (a record argument) parsing as application.
    "let a = Maybe { let! x = m return x }",
    "let a = Build { yield 1 yield 2 }",
    "let a = M { let x = 1 do! e return! r }",
    "let a = Some { x = 1 }",
    // Units of measure.
    "measure m",
    // Derived-measure aliases (`measure N = <unit body>`, no `<>` brackets).
    "measure kg\nmeasure m\nmeasure s\nmeasure N = kg m / s^2",
    "measure m\nmeasure s\nmeasure Hz = 1 / s\nmeasure Speed = m / s",
    "measure m\nmeasure s\nlet speed = 100<m> / 10<s>",
    "let a = 5<m>",
    "let a = 3<m/s>",
    "let a = 2<m^2>",
    "let a = 9<kg m / s^2>",
    "let a = 7<1>",
    // Denominator-only unit: `</s>` prints (and reparses) as `1/s`.
    "let a = 5</s>",
    // Externs (typed Python imports). The `= target` clause prints only when the
    // Python target differs from the Pyfun name.
    "extern len: string -> int",
    "extern show: a -> string = str",
    "extern pure sqrt: float -> float = math.sqrt",
    "extern pure pow: float -> float -> float = math.pow",
    // In-file modules: members and qualified access.
    "module Geometry =\n    let pi = 3\n    let area r = pi * r * r",
    "module M =\n    let add a b = a + b",
    "let big = Geometry.area 10",
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
    let ExprKind::App { func, arg } = &binding.value.kind else {
        panic!("expected an application")
    };
    assert_eq!(arg.kind, ExprKind::Var("b".to_string()));
    let ExprKind::App {
        func: inner_func,
        arg: inner_arg,
    } = &func.kind
    else {
        panic!("expected a nested application")
    };
    assert_eq!(inner_func.kind, ExprKind::Var("f".to_string()));
    assert_eq!(inner_arg.kind, ExprKind::Var("a".to_string()));
}

#[test]
fn pipe_binds_looser_than_application() {
    // `x |> f a` must be `Pipe(x, App(f, a))`, not `App(Pipe(x, f), a)`.
    let module = parse("let r = x |> f a").unwrap();
    let Item::Let(binding) = &module.items[0] else {
        panic!("expected a let binding")
    };
    let ExprKind::Pipe { lhs, rhs } = &binding.value.kind else {
        panic!("expected a pipe")
    };
    assert_eq!(lhs.kind, ExprKind::Var("x".to_string()));
    assert!(
        matches!(rhs.kind, ExprKind::App { .. }),
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
    let ExprKind::Binary {
        op: BinOp::Add,
        rhs,
        ..
    } = &binding.value.kind
    else {
        panic!("expected an addition at the root")
    };
    assert!(
        matches!(rhs.kind, ExprKind::Binary { op: BinOp::Mul, .. }),
        "right operand of + should be the multiplication"
    );
}

#[test]
fn offside_rule_separates_statements_but_joins_continuations() {
    // Two bare statements on separate lines are two items, not one juxtaposition.
    let module = parse("print a\nprint b").unwrap();
    assert_eq!(module.items.len(), 2, "statements should not merge");

    // An indented continuation stays part of the same item.
    let module = parse("let f n =\n  match n with\n  | 0 -> 1\n  | _ -> 2").unwrap();
    assert_eq!(
        module.items.len(),
        1,
        "continuation should not split the item"
    );
    let Item::Let(binding) = &module.items[0] else {
        panic!("expected a let binding")
    };
    assert!(matches!(binding.value.kind, ExprKind::Match { .. }));
}

#[test]
fn reports_errors_for_malformed_input() {
    assert!(parse("let = 1").is_err());
    assert!(parse("if x then y").is_err()); // missing else
    assert!(parse("match x with").is_err()); // no arms
    assert!(parse("(1 + 2").is_err()); // unbalanced paren
}
