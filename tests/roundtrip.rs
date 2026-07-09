//! Phase 1 acceptance tests.
//!
//! The headline guarantee is the parse→print→parse roundtrip: printing an AST
//! and reparsing it must yield a structurally identical AST. A handful of
//! shape assertions pin down the parts most likely to regress — currying and
//! operator precedence.

use pyfun::parse;
use pyfun::syntax::{BinOp, ExprKind, Item, UnOp};

/// Programs exercising every Phase 1 construct.
const PROGRAMS: &[&str] = &[
    "let x = 1",
    "let mut y = 2",
    "let pi = 3.14",
    // Scientific-notation float literals.
    "let big = 1e6",
    "let small = 2.5e-3",
    "let grav = 6.674e-11",
    "let greeting = \"hello\\nworld\"",
    "let yes = true",
    // The unit value `()`.
    "let nothing = ()",
    "let noop x = ()",
    "let force f = f ()",
    "let add a b = a + b",
    // Standard combinators — ordinary identifiers, no new syntax.
    "let r = flip add 3 10",
    "let z = List.map (const 0) xs",
    "let r = 1 + 2 * 3 - 4 / 2",
    "let r = 7 // 2",
    "let r = 10 % 3",
    "let r = n % 2 == 0",
    "let m = (%)",
    // Exponentiation: right-associative, tighter than unary minus.
    "let r = 2.0 ** 8.0",
    "let r = 2.0 ** 3.0 ** 2.0",
    "let r = -2.0 ** 2.0",
    "let p = (**)",
    // Comparison & equality (looser than arithmetic).
    "let r = 1 + 1 < 3",
    "let r = a == b",
    "let r = x <= y",
    "let cmp a b = a < b",
    // Chained comparisons (Python-style), a single node distinct from `(a < b) < c`.
    "let r = 1 < x < 10",
    "let r = a < b < c",
    "let r = a <= b < c",
    "let r = 1 == 1 == 1",
    // Boolean operators and prefix `not`.
    "let r = a or b and c",
    "let r = not a",
    "let r = not a == b",
    // Prefix arithmetic negation.
    "let a = -5",
    "let b = abs (-5)",
    "let c = -3 + 10",
    "let d = 2 * -3",
    "let e = -(4 + 1)",
    "let f = 0 - -7",
    "let g = -x",
    "let chk lo hi x = lo <= x and x <= hi",
    // Operator sections `(op)` — a binary operator as a curried function.
    "let mul = (*)",
    "let add = (+)",
    "let sub = (-)",
    "let flr = (//)",
    "let lt = (<)",
    "let eq = (==)",
    "let ne = (!=)",
    "let double = (*) 2",
    "let total = List.fold (+) 0 xs",
    "let first = List.get 0 xs",
    "let joined = List.concat xs ys",
    "let sorted = List.sort xs",
    "let hit = List.find (fun x -> x > 0) xs",
    // `5<m>` (adjacent, in the units section below) is a unit annotation, whereas
    // `5 < m` (spaced) is a comparison — the printer keeps them distinct.
    "let r = 5 < m",
    "let curried = f a b c",
    "let piped = x |> f |> g a",
    // Backward pipe `<|` — right-associative (`f <| g <| x` = `f (g x)`).
    "let back = f <| x",
    "let chain = f <| g <| x",
    // Function composition `>>` / `<<`: left-associative, tighter than `|>`.
    "let h = f >> g",
    "let h = f << g",
    "let h = f >> g >> h",
    "let piped = x |> f >> g",
    "let mapped = List.map (double >> inc) xs",
    "let choose = if cond then a else b",
    // Typed holes: anonymous `?` and named `?name`, in any expression position.
    "let f x = ?",
    "let g x = ?body + 1",
    "let h = List.map ?fn xs",
    "let i = f ? ?other",
    // `let _ = e` discards a result (top-level and in a block).
    "let _ = f 1",
    "let g x =\n    let _ = h x\n    x",
    // `elif` is sugar for `else if` (a nested `If`); the printer canonicalizes an
    // else-if chain to `elif` and it reparses to the same AST.
    "let grade n =\n  if n >= 90 then \"A\"\n  elif n >= 80 then \"B\"\n  else \"F\"",
    "let compose = fun f g x -> f (g x)",
    "let describe n =\n  match n:\n    case 0: \"zero\"\n    case _: \"many\"",
    // A negative integer literal pattern.
    "let sign n =\n  match n:\n    case -1: \"neg\"\n    case 0: \"zero\"\n    case _: \"pos\"",
    // as-patterns: bind the whole matched value alongside destructuring.
    "let f s =\n  match s:\n    case Some v as w: w\n    case None: None",
    "let g p =\n  match p:\n    case (a, b) as both: both",
    "let unwrap o =\n  match o:\n    case Some v: v\n    case None: 0",
    "let nested =\n  match p:\n    case Pair (Some a) b: a\n    case _: b",
    // Record patterns: shorthand, explicit, subset, and nested sub-patterns. The
    // `{ x }` shorthand must print back as shorthand (not `{ x = x }`).
    "let f p =\n  match p:\n    case Point { x, y }: x",
    "let f p =\n  match p:\n    case Point { x = a, y = b }: a",
    "let f p =\n  match p:\n    case Point { x = 0, y }: y\n    case Point { x }: x",
    "let f b =\n  match b:\n    case Box { item = Some n }: n\n    case _: 0",
    // A qualified (cross-module) record pattern.
    "let f q =\n  match q:\n    case Geometry.Point { x, y }: x",
    // List/sequence patterns: empty, fixed, prefix+star, lone star, nested elements.
    "let f xs =\n  match xs:\n    case []: 0\n    case [x, *rest]: x",
    "let f xs =\n  match xs:\n    case []: 0\n    case [x]: x\n    case [x, y]: y\n    case [a, *rest]: a",
    "let f xs =\n  match xs:\n    case [*all]: all",
    "let f xs =\n  match xs:\n    case [Some x, *rest]: x\n    case _: 0",
    "let f xs =\n  match xs:\n    case [0, y]: y\n    case _: 0",
    // Non-last (suffix) stars: `[*init, last]` and `[a, *mid, z]`, incl. nested
    // element patterns on both sides of the star.
    "let f xs =\n  match xs:\n    case []: 0\n    case [*init, last]: last",
    "let f xs =\n  match xs:\n    case []: 0\n    case [x]: x\n    case [a, *mid, z]: a + z",
    "let f xs =\n  match xs:\n    case [Some x, *rest, None]: x\n    case _: 0",
    "let f xs =\n  match xs:\n    case [*_, z]: z\n    case []: 0",
    // A small multi-item module mixing definitions and a trailing expression.
    "let id x = x\nlet k = id 42\nk |> id",
    // Offside rule: an indented continuation keeps a multi-line item together.
    "let classify n =\n  match n:\n    case 0: \"zero\"\n    case _: \"many\"",
    // Blocks in match arms / if branches / lambda bodies (opened after `->`,
    // `then`/`else`). A single-statement block is unwrapped, so only multi-stmt
    // bodies stay blocks; the printer renders them with offside layout.
    "let f n =\n  match n:\n    case 0:\n        let x = 1\n        x\n    case _: 0",
    "let f c =\n  if c then\n      let x = 1\n      x\n  else 0",
    "let f c =\n  if c then 1\n  else\n      let y = 2\n      y",
    "let g = fun x ->\n  let y = x\n  y",
    "let f n =\n  match n:\n    case 0:\n        let a = 1\n        a\n    case _:\n        let b = 2\n        b",
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
    "let p = Point { x = 3, y = 4 }",
    // A qualified (cross-module) record literal for an imported record.
    "let p = Geometry.Point { x = 3, y = 4 }",
    "let q = { p with y = 9 }",
    // Nested record-update sugar: dotted field paths, mixed with plain updates.
    "let q = { p with a.b = 1 }",
    "let q = { p with a.b.c = 1 }",
    "let q = { p with a.b = 1, x = 2, a.c = 3 }",
    "let s = p.x",
    "let d = obj.inner.value",
    "let n = (mk a).field",
    // Type declarations: nullary, parameterized, and recursive.
    "type Color = Red | Green | Blue",
    "type Option a = None | Some a",
    "type Result a b = Ok a | Err b",
    "type List a = Nil | Cons a (List a)",
    "type Option a = None | Some a\nlet unwrap o =\n  match o:\n    case Some v: v\n    case None: 0",
    // Offside union layout: one `| Variant` per line after `=` (the F#/ML form).
    // Prints back to the single-line canonical form, so roundtrip holds.
    "type Color =\n  | Red\n  | Green\n  | Blue",
    "type Shape =\n  | Circle float\n  | Rect float float\n  | Square float",
    // A record body may likewise sit on the next indented line.
    "type Point =\n  { x: int, y: int }",
    // Opaque handle types (`extern type`): no body, optional type parameters. They
    // print back with the `extern type` keyword and no `=`.
    "extern type Conn",
    "extern type Ref a",
    "extern type Pair a b",
    // List literals: empty, simple, nested, and compound elements.
    "let xs = [1, 2, 3]",
    "let e = []",
    "let m = [[1, 2], [3, 4]]",
    "let c = [a + 1, f b, x]",
    "let mapped = map f [1, 2, 3]",
    // Tuple literals (2+ elements), nesting, compound elements; `()` stays unit and
    // `(x)` stays grouping (no 0- or 1-tuples).
    "let pair = (1, 2)",
    "let triple = (1, \"a\", true)",
    "let nested = ((1, 2), 3)",
    "let mixed = (f a, x + 1, [1, 2])",
    "let grouped = (x + 1)",
    // Tuple patterns in match arms, including nested ones.
    "let swap p =\n  match p:\n    case (a, b): (b, a)",
    "let fst t =\n  match t:\n    case (a, _): a",
    "let deep p =\n  match p:\n    case ((a, b), c): a",
    // Tuple types in declarations and externs.
    "type Pair = { both: (int, string) }",
    "extern pure mk: a -> b -> (a, b) = builtins.tuple",
    // Computation expressions (built-in builders).
    "let a = seq { yield 1 yield! xs }",
    "let a = result { let! x = m return x }",
    "let a = async { let! x = m do! n return! r }",
    // User-defined CE builders (an uppercase module name before `{`). The CE-item
    // lookahead keeps `Some { x = 1 }` (a record argument) parsing as application.
    "let a = Maybe { let! x = m return x }",
    "let a = Build { yield 1 yield 2 }",
    "let a = M { let x = 1 do! e return! r }",
    "let a = Some (Cell { x = 1 })",
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
    // A dotted target inside a submodule (imported as the submodule at lowering).
    "extern quote: string -> string = urllib.parse.quote",
    // Instance-access externs. A method (`= .method()`) is called on the first
    // argument; a property (`= .attr`) is read. Includes the case where the member
    // name equals the Pyfun name, which must still print the leading `.` to
    // round-trip, and must keep the method-vs-property distinction.
    "extern readBody: a -> string = .read()",
    "extern read: a -> string = .read()",
    "extern execute: a -> string -> b = .execute()",
    "extern jsonBody: a -> b = .json()",
    "extern scheme: a -> string = .scheme",
    "extern text: a -> string = .text",
    // Effect annotations on declared arrows (`DESIGN.md` §4): labels print back
    // as written, so single, multi, and argument-position annotations roundtrip.
    "extern fetch: string ->{async} string = httpx.get",
    "extern send: string -> string ->{io, async} unit = hub.send",
    "extern register: (string ->{io} unit) -> unit = hub.register",
    "type Handler = H (string ->{io} unit)",
    "type Callbacks = { onData: string ->{io, async} unit }",
    // Interpolated strings `f"..."`: literal chunks, holes with full expressions,
    // `{{`/`}}` escapes. Printing re-escapes and reparses to the same AST.
    "let g = f\"hello {name}\"",
    "let g = f\"{a} + {b} = {a + b}\"",
    "let g = f\"upper {String.upper name}\"",
    "let g = f\"a literal brace {{ and {x}\"",
    "let g = f\"no holes at all\"",
    // Triple-quoted strings: embedded newlines are literal content. The canonical
    // print escapes them (`"a\nb"`), which reparses to the same string value —
    // roundtrip holds on value equality.
    "let doc = \"\"\"line one\nline two\"\"\"",
    "let quo = \"\"\"a \"quoted\" word\"\"\"",
    "let g = f\"\"\"hi {name}\nsum {a + b}\"\"\"",
    "let p = r\"\"\"C:\\path\nnext\"\"\"",
    // Self-documenting debug holes `{x=}` resolve at lex time into an echoed
    // literal chunk + an ordinary hole, so `f"{x=}"` prints as `f"x={x}"` and
    // reparses to the same AST (whitespace around the `=` is preserved).
    "let g = f\"{x=}\"",
    "let g = f\"{x = }\"",
    "let g = f\"val {a + b=} end\"",
    // A trailing `==`/`<=` is an operator, not a debug marker.
    "let g = f\"{a == b}\"",
    "let g = f\"{a <= b}\"",
    // `try e` — catch an exception into a `Result` (`DESIGN.md` §6). Binds looser
    // than `+` but tighter than `|>`, so the result pipes out.
    "let r = try (parseInt s)",
    "let r = try parseInt s",
    "let n = Result.withDefault 0 (try (parseInt s))",
    // String literal patterns, and a record pattern over the reserved `Exception`.
    "let f s =\n  match s:\n    case \"yes\": 1\n    case \"no\": 0\n    case _: 2",
    "let g r =\n  match r:\n    case Ok n: n\n    case Error e: e.errorKind\n    case _: \"?\"",
    "let h r =\n  match r:\n    case Error (Exception { errorKind = \"ValueError\" }): 1\n    case _: 0",
    // Built-in `String` module: qualified access is the ordinary field path.
    "let g = String.concat \"a\" \"b\"",
    "let parts = String.split \",\" line",
    "let n = String.toInt s",
    // In-file modules: members and qualified access.
    "module Geometry =\n    let pi = 3\n    let area r = pi * r * r",
    "module M =\n    let add a b = a + b",
    "let big = Geometry.area 10",
    // File-based module imports (`DESIGN.md` §6.1). The name is a single
    // capitalized identifier; access is the ordinary `Name.member` field path.
    "import Geometry",
    "import Geometry\nlet big = Geometry.area 10",
    "import Geometry\nimport Physics\nlet x = 1",
    // Qualified constructor patterns (an imported sum type, `DESIGN.md` §6.1).
    "let f s =\n  match s:\n    case Geometry.Circle r: r\n    case Geometry.Rect w h: w",
    "let g k =\n  match k:\n    case Color.Red: 1\n    case Color.Other: 2",
    // Doc comments (`## …` at column 0) attach to the following declaration and
    // are re-emitted by the printer, so they survive the roundtrip.
    "## Doubles a number.\nlet double x = x * 2",
    "## Line one.\n## Line two.\nlet x = 1",
    "## A shape.\ntype Shape = Circle float | Rect float float",
    "## Square root from Python.\nextern sqrt : float -> float = math.sqrt",
    "## Documented.\nlet a = 1\nlet b = 2",
    // Active patterns (`DESIGN.md` §7.2): total, partial (Option and bool),
    // parameterized partial — and matches using the cases (ordinary ctor
    // patterns, so they need no new pattern syntax).
    "let (|Even|Odd|) n = if n % 2 == 0 then Even else Odd",
    "let (|Small|Big|) n = if n < 10 then Small n else Big (n - 10)",
    "let (|Prime|_|) n = if n > 1 then Some n else None",
    "let (|Blank|_|) s = s == \"\"",
    "let (|DivisibleBy|_|) d n = n % d == 0",
    "## Splits parity.\nlet (|Even|Odd|) n = if n % 2 == 0 then Even else Odd",
    "let (|Even|Odd|) n = if n % 2 == 0 then Even else Odd\nlet f n =\n  match n:\n    case Even: 1\n    case Odd: 2",
    "let f n =\n  match n:\n    case DivisibleBy 3: 1\n    case Prime p: p\n    case _: 0",
];

#[test]
fn multiline_union_parses_like_inline() {
    // The offside `| Variant`-per-line layout must produce exactly the same AST
    // as the single-line form — the difference is pure surface whitespace. A
    // leading `|` on each line is optional, and a record body may also be
    // indented onto the next line.
    let cases = [
        (
            "type Color =\n  | Red\n  | Green\n  | Blue",
            "type Color = Red | Green | Blue",
        ),
        (
            "type Color =\n  Red\n  | Green\n  | Blue",
            "type Color = Red | Green | Blue",
        ),
        (
            "type Shape =\n  | Circle float\n  | Rect float float",
            "type Shape = Circle float | Rect float float",
        ),
        // Same-line `|` and newline `|` may be mixed within the block.
        (
            "type M =\n  | A | B\n  | C",
            "type M = A | B | C",
        ),
        (
            "type Point =\n  { x: int, y: int }",
            "type Point = { x: int, y: int }",
        ),
    ];
    for (multiline, inline) in cases {
        let a = parse(multiline).unwrap_or_else(|e| panic!("failed to parse {multiline:?}: {e}"));
        let b = parse(inline).unwrap();
        assert_eq!(a, b, "multi-line and inline forms differ for {multiline:?}");
    }
}

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
    let ExprKind::Pipe { lhs, rhs, .. } = &binding.value.kind else {
        panic!("expected a pipe")
    };
    assert_eq!(lhs.kind, ExprKind::Var("x".to_string()));
    assert!(
        matches!(rhs.kind, ExprKind::App { .. }),
        "rhs of pipe should be an application"
    );
}

#[test]
fn composition_binds_tighter_than_pipe() {
    // `x |> f >> g` must be `Pipe(x, Compose(f, g))`, not `Compose(Pipe(x, f), g)`.
    let module = parse("let r = x |> f >> g").unwrap();
    let Item::Let(binding) = &module.items[0] else {
        panic!("expected a let binding")
    };
    let ExprKind::Pipe { lhs, rhs, .. } = &binding.value.kind else {
        panic!("expected a pipe")
    };
    assert_eq!(lhs.kind, ExprKind::Var("x".to_string()));
    assert!(
        matches!(rhs.kind, ExprKind::Compose { .. }),
        "rhs of pipe should be a composition"
    );
}

#[test]
fn backward_pipe_is_right_associative() {
    // `f <| g <| x` = `f <| (g <| x)` — the rhs is itself a backward pipe.
    let module = parse("let r = f <| g <| x").unwrap();
    let Item::Let(binding) = &module.items[0] else {
        panic!("expected a let binding")
    };
    let ExprKind::Pipe { rhs, backward, .. } = &binding.value.kind else {
        panic!("expected a pipe")
    };
    assert!(*backward, "outer pipe should be backward");
    assert!(
        matches!(rhs.kind, ExprKind::Pipe { backward: true, .. }),
        "rhs should be the nested backward pipe (right-associative)"
    );
}

#[test]
fn composition_is_left_associative() {
    // `f >> g >> h` = `(f >> g) >> h`, and `>>` sets `right_to_left = false`.
    let module = parse("let r = f >> g >> h").unwrap();
    let Item::Let(binding) = &module.items[0] else {
        panic!("expected a let binding")
    };
    let ExprKind::Compose {
        lhs,
        rhs,
        right_to_left,
    } = &binding.value.kind
    else {
        panic!("expected a composition")
    };
    assert!(!right_to_left, "`>>` is left-to-right");
    assert_eq!(rhs.kind, ExprKind::Var("h".to_string()));
    assert!(
        matches!(lhs.kind, ExprKind::Compose { .. }),
        "left-associative: lhs is itself a composition"
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
fn exponentiation_is_right_assoc_and_binds_tighter_than_unary_minus() {
    // `2 ** 3 ** 2` is `2 ** (3 ** 2)` — the right operand is another `**`.
    let module = parse("let r = 2.0 ** 3.0 ** 2.0").unwrap();
    let Item::Let(binding) = &module.items[0] else {
        panic!("expected a let binding")
    };
    let ExprKind::Binary {
        op: BinOp::Pow,
        rhs,
        ..
    } = &binding.value.kind
    else {
        panic!("expected `**` at the root, got {:?}", binding.value.kind)
    };
    assert!(
        matches!(rhs.kind, ExprKind::Binary { op: BinOp::Pow, .. }),
        "right operand should be the nested `**` (right-associative)"
    );

    // `-2 ** 2` is `-(2 ** 2)` — negation wraps the power, not the other way.
    let module = parse("let r = -2.0 ** 2.0").unwrap();
    let Item::Let(binding) = &module.items[0] else {
        panic!("expected a let binding")
    };
    let ExprKind::Unary {
        op: UnOp::Neg,
        expr,
    } = &binding.value.kind
    else {
        panic!(
            "expected negation at the root, got {:?}",
            binding.value.kind
        )
    };
    assert!(matches!(expr.kind, ExprKind::Binary { op: BinOp::Pow, .. }));
}

#[test]
fn chained_comparison_is_one_node_but_single_stays_binary() {
    // `a < b < c` is a single chained comparison, not `(a < b) < c`.
    let module = parse("let r = a < b < c").unwrap();
    let Item::Let(binding) = &module.items[0] else {
        panic!("expected a let binding")
    };
    let ExprKind::Compare { rest, .. } = &binding.value.kind else {
        panic!(
            "expected a chained comparison, got {:?}",
            binding.value.kind
        )
    };
    assert_eq!(rest.len(), 2, "two comparison links");

    // A lone comparison stays a plain `Binary`.
    let module = parse("let r = a < b").unwrap();
    let Item::Let(binding) = &module.items[0] else {
        panic!("expected a let binding")
    };
    assert!(matches!(binding.value.kind, ExprKind::Binary { .. }));
}

#[test]
fn operator_section_parses_to_op_func() {
    // `(*)` is an operator section, distinct from grouping `(x)` and unit `()`.
    let module = parse("let mul = (*)").unwrap();
    let Item::Let(binding) = &module.items[0] else {
        panic!("expected a let binding")
    };
    assert!(matches!(binding.value.kind, ExprKind::OpFunc(BinOp::Mul)));

    // `(*) 2` is ordinary application of the section to one argument.
    let module = parse("let double = (*) 2").unwrap();
    let Item::Let(binding) = &module.items[0] else {
        panic!("expected a let binding")
    };
    let ExprKind::App { func, .. } = &binding.value.kind else {
        panic!("expected an application")
    };
    assert!(matches!(func.kind, ExprKind::OpFunc(BinOp::Mul)));

    // A parenthesized expression is still grouping, not a section.
    let module = parse("let g = (1 + 2)").unwrap();
    let Item::Let(binding) = &module.items[0] else {
        panic!("expected a let binding")
    };
    assert!(matches!(binding.value.kind, ExprKind::Binary { .. }));
}

#[test]
fn offside_rule_separates_statements_but_joins_continuations() {
    // Two bare statements on separate lines are two items, not one juxtaposition.
    let module = parse("print a\nprint b").unwrap();
    assert_eq!(module.items.len(), 2, "statements should not merge");

    // An indented continuation stays part of the same item.
    let module = parse("let f n =\n  match n:\n    case 0: 1\n    case _: 2").unwrap();
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
fn import_parses_to_a_named_import_item() {
    let module = parse("import Geometry").unwrap();
    assert_eq!(module.items.len(), 1);
    let Item::Import { name, .. } = &module.items[0] else {
        panic!("expected an import item")
    };
    assert_eq!(name, "Geometry");
}

#[test]
fn import_requires_a_capitalized_module_name() {
    // The module name is a single uppercase identifier (like the in-file `module`
    // name); lowercase or missing names are errors.
    assert!(parse("import geometry").is_err());
    assert!(parse("import").is_err());
}

#[test]
fn reports_errors_for_malformed_input() {
    assert!(parse("let = 1").is_err());
    assert!(parse("if x then y").is_err()); // missing else
    assert!(parse("match x:").is_err()); // no arms
    assert!(parse("(1 + 2").is_err()); // unbalanced paren
}

#[test]
fn doc_comment_attaches_to_the_following_declaration() {
    // `## …` lines at column 0 join (with `\n`) onto the next top-level
    // `let`/`type`/`extern`; the printer re-emits them, one `## ` line each.
    let module = parse("## Adds one\n## to a number.\nlet inc x = x + 1").unwrap();
    let Item::Let(binding) = &module.items[0] else {
        panic!("expected a let binding")
    };
    assert_eq!(binding.doc.as_deref(), Some("Adds one\nto a number."));
    let printed = pyfun::ast::print_module(&module);
    assert!(
        printed.contains("## Adds one\n## to a number.\nlet inc"),
        "printed was:\n{printed}"
    );
}

#[test]
fn doc_comment_attaches_to_type_and_extern_declarations() {
    let module =
        parse("## A colour.\ntype Color = Red | Green\n## Python sqrt.\nextern sqrt : float -> float = math.sqrt")
            .unwrap();
    let Item::Type(decl) = &module.items[0] else {
        panic!("expected a type declaration")
    };
    assert_eq!(decl.doc.as_deref(), Some("A colour."));
    let Item::Extern(decl) = &module.items[1] else {
        panic!("expected an extern declaration")
    };
    assert_eq!(decl.doc.as_deref(), Some("Python sqrt."));
}

#[test]
fn ordinary_comments_and_non_doc_positions_stay_undocumented() {
    // A single-`#` comment is not a doc; an indented or trailing `##` stays an
    // ordinary comment; a doc before a second binding doesn't leak to the first.
    let module =
        parse("# plain comment\nlet a = 1 ## trailing\n## Documented.\nlet b = 2").unwrap();
    let Item::Let(a) = &module.items[0] else {
        panic!("expected a let binding")
    };
    assert_eq!(a.doc, None);
    let Item::Let(b) = &module.items[1] else {
        panic!("expected a let binding")
    };
    assert_eq!(b.doc.as_deref(), Some("Documented."));
}

#[test]
fn trailing_doc_lines_without_a_declaration_are_dropped() {
    // A doc at EOF (or before a non-documentable item) has nothing to attach to;
    // it is accepted and dropped like an ordinary comment.
    let module = parse("let a = 1\n## dangling note").unwrap();
    assert_eq!(module.items.len(), 1);
}

#[test]
fn a_list_pattern_allows_at_most_one_star() {
    // The star may sit anywhere (`[*init, last]`, `[a, *mid, z]`), but Python's
    // one-star rule holds: a second `*` is a parse error.
    let err = parse("let f xs =\n  match xs:\n    case [*a, *b]: 0").unwrap_err();
    assert!(
        err.message().contains("at most one `*`"),
        "unexpected message: {}",
        err.message()
    );
}

// ---------- active patterns (`DESIGN.md` §7.2) ----------

#[test]
fn active_pattern_declarations_parse_to_the_expected_shape() {
    let module = parse("let (|DivisibleBy|_|) d n = n % d == 0").unwrap();
    let Item::ActivePattern(decl) = &module.items[0] else {
        panic!("expected an active-pattern item")
    };
    assert!(decl.partial);
    assert_eq!(decl.cases.len(), 1);
    assert_eq!(decl.cases[0].name, "DivisibleBy");
    let params: Vec<&str> = decl.params.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(params, ["d", "n"]);

    let module = parse("let (|Even|Odd|) n = if n % 2 == 0 then Even else Odd").unwrap();
    let Item::ActivePattern(decl) = &module.items[0] else {
        panic!("expected an active-pattern item")
    };
    assert!(!decl.partial);
    let cases: Vec<&str> = decl.cases.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(cases, ["Even", "Odd"]);
}

#[test]
fn a_total_active_pattern_takes_exactly_one_parameter() {
    let err = parse("let (|Even|Odd|) d n = Even").unwrap_err();
    assert!(
        err.message().contains("exactly one parameter"),
        "unexpected message: {}",
        err.message()
    );
}

#[test]
fn an_active_pattern_is_top_level_only() {
    // Inside a block (a local `let`) the banana brackets are a guiding error.
    let err = parse("let f x =\n  let (|A|_|) n = n == 0\n  x").unwrap_err();
    assert!(
        err.message().contains("top-level"),
        "unexpected message: {}",
        err.message()
    );
    // And the `mut`/`pure` modifiers don't apply to one.
    let err = parse("let pure (|A|_|) n = n == 0").unwrap_err();
    assert!(
        err.message().contains("top-level"),
        "unexpected message: {}",
        err.message()
    );
}
