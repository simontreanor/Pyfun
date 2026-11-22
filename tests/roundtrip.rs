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
    "let yes = True",
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
    "let kept = List.choose f xs",
    "let flat = List.collect f xs",
    "let name = input \"who? \"",
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
    // Destructuring parameters: tuple, nested tuple, `_`, and mixed with names.
    "let tile_score (t, sq) = t * sq",
    "let swap (a, b) = (b, a)",
    "let nested ((a, b), c) = a + b + c",
    "let ignore_it _ = 42",
    "let mix a (b, c) _ = a + b + c",
    "let f = fun (a, b) -> a + b",
    "let total = List.fold (fun acc (a, b) -> acc + a * b) 0 pairs",
    "let partial (a, _) = a",
    // Record patterns in parameters: shorthand, explicit, nested, subset, mixed.
    "let describe (Cell { letter, row }) = letter",
    "let explicit (Cell { letter = l }) = l",
    "let nested (Pair { left = Cell { letter } }) = letter",
    "let mixed n (Cell { letter }) _ = letter",
    "let in_tuple (a, Cell { letter }) = letter",
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
    "let f c =\n  if c:\n      let x = 1\n      x\n  else 0",
    "let f c =\n  if c then 1\n  else:\n      let y = 2\n      y",
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
    "type Box a = { item: a, tag: str }",
    "let p = Point { x = 3, y = 4 }",
    // A qualified (cross-module) record literal for an imported record.
    "let p = Geometry.Point { x = 3, y = 4 }",
    // A module-qualified type *name*, in every position a written type appears:
    // a record field, an ADT variant's payload, and an extern signature.
    "type Holder = { item: Geometry.Point }",
    "type Slot = Empty | Filled Geometry.Point",
    "extern pure widen : Geometry.Point -> Geometry.Point = builtins.id",
    "extern pure boxed : Store.Box int -> int = builtins.id",
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
    // A sole constructor's pattern in a parameter and a `let`.
    "let unwrap (Wrap v) = v",
    "let (Wrap a (b, _)) = w",
    "let f = fun (Wrap v) -> v",
    // Record-bodied constructors: a declaration, the literal, the pattern.
    "type Shape = Circle { radius: float } | Rect { width: float, height: float } | Dot",
    "type Tree a = Leaf | Node { left: Tree a, value: a, right: Tree a }",
    "type Slot = Empty | Filled { at: Geometry.Point, by: str }",
    "let r = Rect { width = 2.0, height = 3.0 }",
    "let f s =\n  match s:\n    case Circle { radius }: radius\n    case Rect { width, height = h }: width * h\n    case Dot {}: 0.0",
    "let g s =\n  match s:\n    case Shapes.Rect {}: 1\n    case _: 0",
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
    // Opaque types (zero-cost newtypes): plain, parameterized, a compound
    // underlying, a wrap, and a match with newtype patterns. `opaque` is a
    // contextual keyword — only before `type` — so a binding named `opaque`
    // still parses.
    "opaque type UserId = str",
    "opaque type Tag a = List a",
    "opaque type Pairz = (int, str)",
    "opaque type UserId = str\nlet uid = UserId \"u\"",
    "opaque type Cents = int\nlet f c =\n  match c:\n    case Cents 0: \"free\"\n    case Cents n: \"costs\"",
    "let opaque = 1",
    // List literals: empty, simple, nested, and compound elements.
    "let xs = [1, 2, 3]",
    "let e = []",
    "let m = [[1, 2], [3, 4]]",
    "let c = [a + 1, f b, x]",
    "let mapped = map f [1, 2, 3]",
    // Tuple literals (2+ elements), nesting, compound elements; `()` stays unit and
    // `(x)` stays grouping (no 0- or 1-tuples).
    "let pair = (1, 2)",
    "let triple = (1, \"a\", True)",
    "let nested = ((1, 2), 3)",
    "let mixed = (f a, x + 1, [1, 2])",
    "let grouped = (x + 1)",
    // Tuple patterns in match arms, including nested ones.
    "let swap p =\n  match p:\n    case (a, b): (b, a)",
    "let fst t =\n  match t:\n    case (a, _): a",
    "let deep p =\n  match p:\n    case ((a, b), c): a",
    // Tuple types in declarations and externs.
    "type Pair = { both: (int, str) }",
    "extern pure mk: a -> b -> (a, b) = builtins.tuple",
    // Computation expressions (built-in builders).
    "let a = seq { yield 1 yield! xs }",
    "let a = result { let! x = m return x }",
    "let a = option { let! x = m return x }",
    "let a = async { let! x = m do! n return! r }",
    "let a = async { let! x = m do! n }",
    "let a = result { let! x = m let _ = print x return x }",
    // User-defined CE builders (an uppercase module name before `{`). The CE-item
    // lookahead keeps `Some { x = 1 }` (a record argument) parsing as application.
    "let a = Maybe { let! x = m return x }",
    "let a = Build { yield 1 yield 2 }",
    "let a = M { let x = 1 do! e return! r }",
    // `for` items: a one-line body is one item; a longer body lays the block out
    // on lines, which is what the printer emits.
    "let a = seq { for x in xs: yield x }",
    "let a = seq { for x in xs: yield x yield 2 }",
    "let a = seq { for (a, b) in ps: yield a for c in cs: yield c }",
    "let a = Html { yield h for r in rows: yield r yield f }",
    "let a = seq {\n    for x in xs:\n        let y = x\n        yield y\n    yield 2\n}",
    "let a = async {\n    for x in xs:\n        do! f x\n        for y in ys:\n            do! g y\n    return 1\n}",
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
    "extern len: str -> int",
    "extern show: a -> str = str",
    "extern pure sqrt: float -> float = math.sqrt",
    "extern pure pow: float -> float -> float = math.pow",
    // A dotted target inside a submodule (imported as the submodule at lowering).
    "extern quote: str -> str = urllib.parse.quote",
    // `extern import` (`DESIGN.md` §6): a boundary module declaration mirroring
    // Python's import statement — bare, dotted, aliased, and alongside the
    // extern whose target it roots.
    "extern import datetime",
    "extern import urllib.request",
    "extern import numpy as np",
    "extern import datetime\nextern now: unit -> a = datetime.datetime.now",
    // Instance-access externs. A method (`= .method()`) is called on the first
    // argument; a property (`= .attr`) is read. Includes the case where the member
    // name equals the Pyfun name, which must still print the leading `.` to
    // round-trip, and must keep the method-vs-property distinction.
    "extern read_body: a -> str = .read()",
    "extern read: a -> str = .read()",
    "extern execute: a -> str -> b = .execute()",
    "extern json_body: a -> b = .json()",
    "extern scheme: a -> str = .scheme",
    "extern text: a -> str = .text",
    // Pinned keyword arguments on an `extern` target (`DESIGN.md` §6): a plain
    // target, a receiver-method target, multiple kwargs, and int / negative-int /
    // bool / float literal values all round-trip. The kwargs also force the `= …`
    // clause to print even when the Python name equals the Pyfun name.
    "extern open_text: str -> Seq str = builtins.open(mode=\"rt\", encoding=\"utf-8\")",
    "extern write_text: a -> str -> int = .write_text(encoding=\"utf-8\")",
    "extern connect: str -> a = sqlite3.connect(timeout=5, check_same_thread=False)",
    "extern gzip_open: str -> a = gzip.open(compresslevel=-1, ratio=2.5, mode=\"rt\")",
    "extern open: str -> a = open(mode=\"rt\")",
    // A `...` slot takes its value from the caller instead of the declaration, and
    // mixes freely with pinned literals on all three target forms.
    "extern parse_int: str -> int -> int = int(base=...)",
    "extern open_text: str -> str -> Seq str = builtins.open(mode=\"rt\", encoding=...)",
    "extern write_text: a -> str -> str -> int = .write_text(encoding=...)",
    "extern mix: str -> int -> bool -> a = m.f(a=1, b=..., c=\"x\", d=...)",
    // Effect annotations on declared arrows (`DESIGN.md` §4): labels print back
    // as written, so single, multi, and argument-position annotations roundtrip.
    "extern fetch: str ->{async} str = httpx.get",
    "extern send: str -> str ->{io, async} unit = hub.send",
    "extern register: (str ->{io} unit) -> unit = hub.register",
    // Effect *variables* (`->{e}`, extern-only): a bare var and a label+var mix
    // print back verbatim (the effect list is just names to the printer).
    "extern each: (a ->{e} unit) -> List a ->{io, e} unit = mymod.each",
    "extern pure app: (a ->{e} b) -> a ->{e} b = mymod.app",
    "type Handler = H (str ->{io} unit)",
    "type Callbacks = { on_data: str ->{io, async} unit }",
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
    "let r = try (parse_int s)",
    "let r = try parse_int s",
    "let n = Result.with_default 0 (try (parse_int s))",
    // String literal patterns, and a record pattern over the reserved `Exception`.
    "let f s =\n  match s:\n    case \"yes\": 1\n    case \"no\": 0\n    case _: 2",
    "let g r =\n  match r:\n    case Ok n: n\n    case Error e: e.error_kind\n    case _: \"?\"",
    "let h r =\n  match r:\n    case Error (Exception { error_kind = \"ValueError\" }): 1\n    case _: 0",
    // Built-in `String` module: qualified access is the ordinary field path.
    "let g = String.concat \"a\" \"b\"",
    "let parts = String.split \",\" line",
    "let n = String.to_int s",
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
    // An or-pattern of binder-free active-pattern cases as the whole arm, and a
    // structural arm sitting beside an active-pattern arm.
    "let (|Even|Odd|) n = if n % 2 == 0 then Even else Odd\nlet f n =\n  match n:\n    case Even | Odd: \"eo\"",
    "let f o =\n  match o:\n    case Answer: 1\n    case Some x: x\n    case None: 0",
    // Destructuring `let` targets (`DESIGN.md` §7): tuples, nesting, records,
    // the discard, and the same targets on a computation expression's `let`/`let!`.
    "let (a, b) = (1, 2)",
    "let (a, (b, c)) = (1, (2, 3))",
    "let (a, _) = (1, 2)",
    "let _ = print \"hi\"",
    "type Point = { x: int, y: int }\nlet Point { x, y } = Point { x = 1, y = 2 }",
    "type Point = { x: int, y: int }\nlet Point { x = a, y = b } = Point { x = 1, y = 2 }",
    "let f p =\n  let (a, b) = p\n  a + b",
    "let f r = result {\n  let! (a, b) = r\n  return a + b\n}",
    "let f r = result {\n  let (a, b) = r\n  return a + b\n}",
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
        ("type M =\n  | A | B\n  | C", "type M = A | B | C"),
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
fn a_refutable_parameter_pattern_is_rejected_with_a_hint() {
    // A parameter has no second arm to fall through to, so only always-matching
    // shapes are admitted. Each rejection names what it saw.
    for (src, what) in [
        ("let f ([a, b]) = a", "a list pattern"),
        ("let f (0) = 1", "a literal pattern"),
        ("let f (a | b) = a", "an or-pattern"),
        ("let f (p as q) = q", "an as-pattern"),
        ("let f (a, Some 1) = a", "a literal pattern"),
        ("let f (Wrap [a]) = a", "a list pattern"),
    ] {
        let err = parse(src).unwrap_err();
        assert!(
            err.message().contains(what) && err.message().contains("must always match"),
            "{src} -> {}",
            err.message()
        );
    }
    // A constructor pattern over always-matching parts has a shape that may
    // always match, so it parses; whether its type has another constructor is
    // the type checker's question.
    for src in [
        "let f (Some x) = x",
        "let f (a, Wrap b) = a",
        "let f (Box { v }) = v",
    ] {
        assert!(parse(src).is_ok(), "{src}");
    }
}

#[test]
fn a_bare_uppercase_parameter_is_still_two_parameters() {
    // Only a `(`-led parameter takes the pattern grammar: `let f Some x = …` has
    // always meant two parameters named `Some` and `x`, not one ctor pattern.
    let module = parse("let f Some x = x").unwrap();
    let Item::Let(binding) = &module.items[0] else {
        panic!("expected a let binding")
    };
    assert_eq!(binding.params.len(), 2);
}

#[test]
fn a_capitalised_let_name_is_rejected_as_a_constructor() {
    // `let Some x = Some 1` used to define a function called `Some` that shadowed
    // the constructor. A capitalised binding name is a mistake every time.
    for src in [
        "let Some x = Some 1",
        "let Pi = 3.14",
        "let f x =\n    let Ok v = Ok x\n    v",
    ] {
        let err = parse(src).unwrap_err();
        assert!(
            err.message().contains("reads as a constructor"),
            "{src}: {}",
            err.message()
        );
    }
    assert!(parse("let some x = x").is_ok());
}

#[test]
fn a_type_annotation_on_a_let_is_answered() {
    // An editor shows an inferred type beside a binding the way Python
    // annotates one (`total: int`, `-> float`). Typed in, either is answered
    // with what a `let` looks like, at the mark that opens the annotation.
    for (source, mark, at) in [
        ("let total: int = 5", ":", 9),
        ("let area shape: Shape = 2.0", ":", 14),
        ("let add a b -> int = a + b", "->", 12),
    ] {
        let err = parse(source).unwrap_err();
        assert!(
            err.message()
                .contains("a `let` is written without a type: Pyfun infers it"),
            "{source}: {}",
            err.message()
        );
        assert!(
            err.message()
                .contains(&format!("Remove the `{mark}` and the type after it")),
            "{source}: {}",
            err.message()
        );
        assert_eq!(err.span().start, at, "{source}");
    }
}

#[test]
fn let_rec_is_rejected_with_a_helpful_hint() {
    // Pyfun has no `rec` keyword (functions are implicitly recursive). The F#/ML
    // `let rec f x = …` would otherwise silently define a function named `rec`; we
    // catch it and point at the fix.
    let err = parse("let rec f x = x").unwrap_err();
    assert!(
        err.message().contains("`rec` is not a keyword"),
        "{}",
        err.message()
    );
    // A binding genuinely named `rec` with no params is still fine (a plain value).
    assert!(parse("let rec = 1").is_ok());
    // …and so is the corrected form.
    assert!(parse("let f x = x").is_ok());
}

#[test]
fn def_is_answered_with_the_let_spelling() {
    // `def` is Python muscle memory; the hint rewrites the line's own shape.
    for (source, expected) in [
        ("def area w h = w * h", "write `let area w h = …`"),
        ("def area(w, h):\n  w * h", "write `let area w h = …`"),
        (
            "def area(w: int, h: int) -> int:\n  w * h",
            "write `let area w h = …`",
        ),
        ("def greet():\n  print \"hi\"", "write `let greet _ = …`"),
        ("let f x =\n  def g y = y + 1\n  g x", "write `let g y = …`"),
    ] {
        let err = parse(source).unwrap_err();
        assert!(
            err.message().contains(expected),
            "{source} -> {}",
            err.message()
        );
    }
    // Every suggested spelling parses.
    assert!(parse("let area w h = w * h").is_ok());
    assert!(parse("let greet _ =\n  print \"hi\"").is_ok());
    // `def` is still an ordinary identifier: a function the program calls `def`.
    assert!(parse("let def x = x + 1\nprint (def 2)").is_ok());
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
    let params: Vec<&str> = decl.params.iter().filter_map(|p| p.name()).collect();
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

#[test]
fn booleans_print_capitalized_in_every_position() {
    // Expression, pattern, and a pinned extern keyword argument.
    for source in [
        "let yes = True",
        "let f b =\n  match b:\n    case True: 1\n    case False: 0",
        "extern connect: str -> a = sqlite3.connect(check_same_thread=False)",
    ] {
        let printed = pyfun::ast::print_module(&parse(source).unwrap());
        assert_eq!(
            parse(&printed).unwrap(),
            parse(source).unwrap(),
            "{printed}"
        );
        assert!(
            printed.contains("True") || printed.contains("False"),
            "{printed}"
        );
        assert!(
            !printed.contains("true") && !printed.contains("false"),
            "{printed}"
        );
    }
}

#[test]
fn a_lowercase_boolean_names_the_capitalized_spelling() {
    // `case true:` must never parse as a variable pattern, which would match
    // everything and leave the `false` arm silently dead.
    for (source, expected) in [
        ("let ok = true", "Pyfun spells this `True`, as Python does"),
        (
            "let f b =\n  match b:\n    case true: 1\n    case false: 0",
            "Pyfun spells this `True`, as Python does",
        ),
        (
            "let ok = not false",
            "Pyfun spells this `False`, as Python does",
        ),
    ] {
        let err = parse(source).unwrap_err();
        assert_eq!(err.message(), expected, "{source}");
    }
}

#[test]
fn suggests_keyword_operators_for_symbolic_logical_operator_typos() {
    for (source, expected) in [
        ("let ok = ready && valid", "use `and` instead of `&&`"),
        ("let ok = ready || valid", "use `or` instead of `||`"),
    ] {
        let err = parse(source).unwrap_err();
        assert!(
            err.message().contains(expected),
            "{source} -> {}",
            err.message()
        );
    }
    // The message ends at the suggestion, with no `, found` tacked on.
    assert_eq!(
        parse("let ok = ready || valid").unwrap_err().message(),
        "use `or` instead of `||`"
    );
}

#[test]
fn return_outside_computation_expression_suggests_dropping_return() {
    let err = parse("let double x = return x * 2").unwrap_err();
    let message = err.to_string();
    assert!(
        message.contains("a function's value is its last expression, so drop `return`"),
        "unexpected message: {message}"
    );
}

#[test]
fn a_python_shaped_statement_names_the_pyfun_form() {
    for (source, expected) in [
        ("x = 5", "Pyfun binds a name with `let`: write `let x = …`"),
        (
            "let f n =\n  total = n + 1\n  total",
            "write `let total = …` (to change a `let mut`, write `total <- …`)",
        ),
        (
            "let mut n = 0\nn += 1",
            "Pyfun has no `+=`: write `n <- n + …`",
        ),
        (
            "let mut n = 9\nn //= 2",
            "Pyfun has no `//=`: write `n <- n // …`",
        ),
        (
            "for x in [1, 2]:\n  print x",
            "Pyfun has no `for` statement",
        ),
        ("while True:\n  print 1", "Pyfun has no `while` loop"),
        ("class Point:\n  x = 1", "Pyfun declares data with `type`"),
        ("from math import sqrt", "write `extern import module`"),
        ("import math", "write `extern import math`"),
        (
            "let f = lambda x, y: x + y",
            "Pyfun writes a lambda as `fun x y -> …`",
        ),
        ("let g = lambda: 1", "Pyfun writes a lambda as `fun _ -> …`"),
        ("let h = List.map (lambda x: x + 1) xs", "`fun x -> …`"),
        (
            "let s = f\"{pi:.2f}\"",
            "Pyfun has no `{x:.2f}` format specs. Format the value with the `Format` module",
        ),
        (
            "let s = f\"{name!r}\"",
            "an f-string hole holds an expression only",
        ),
    ] {
        match parse(source) {
            Err(err) => assert!(
                err.message().contains(expected),
                "{source}\n-> {}",
                err.message()
            ),
            Ok(_) => panic!("{source} should not parse"),
        }
    }
}

#[test]
fn python_words_stay_ordinary_names() {
    // Each hint needs the whole Python shape, so a program that only uses one
    // of these words as a name parses as it always did.
    for source in [
        "let lambda = 3",
        "let for = lambda + 1",
        "let r = List.map (fun from -> from + while) [class]",
        "let total = for x",
        "print (lambda x)",
        "let s = f\"{a} and {b}\"",
    ] {
        assert!(parse(source).is_ok(), "{source}: {:?}", parse(source).err());
    }
}

#[test]
fn an_if_branch_is_spelled_by_its_shape() {
    // A branch on its own indented lines opens with `:`, as Python writes it;
    // a branch on the same line follows `then` (or a bare `else`). The printer
    // picks per branch, so a chain may mix them.
    let block = "let sign x =\n  if x > 0:\n    print \"positive\"\n    1\n  elif x < 0:\n    print \"negative\"\n    2\n  else:\n    print \"zero\"\n    0";
    let module = parse(block).unwrap();
    let printed = pyfun::ast::print_module(&module);
    assert_eq!(
        printed.trim_end(),
        "let sign x =\n    if (x > 0):\n        (print \"positive\")\n        1\n    elif (x < 0):\n        (print \"negative\")\n        2\n    else:\n        (print \"zero\")\n        0",
    );
    assert_eq!(parse(&printed).unwrap(), module, "{printed}");

    let mixed = "let f x =\n  if x > 0:\n    let y = x * 2\n    y + 1\n  else 0";
    let module = parse(mixed).unwrap();
    let printed = pyfun::ast::print_module(&module);
    assert_eq!(
        printed.trim_end(),
        "let f x =\n    if (x > 0):\n        let y = (x * 2)\n        (y + 1)\n    else 0",
    );
    assert_eq!(parse(&printed).unwrap(), module, "{printed}");

    // A one-expression branch written as a block is the same AST as the inline
    // form, and prints inline.
    let lone = "let f x =\n  if x > 0:\n    1\n  else:\n    2";
    assert_eq!(
        parse(lone).unwrap(),
        parse("let f x = if x > 0 then 1 else 2").unwrap()
    );
    let printed = pyfun::ast::print_module(&parse(lone).unwrap());
    assert!(printed.contains("then 1 else 2"), "{printed}");
}

#[test]
fn a_crossed_if_branch_names_the_other_opener() {
    for (source, expected) in [
        (
            "let f x =\n  if x > 0 then\n    1\n  else 2",
            "a branch on its own indented lines opens with `:`, as in Python: \
             write `if cond:`, not `then`",
        ),
        (
            "let f x =\n  if x > 0:\n    1\n  else\n    2",
            "a branch on its own indented lines opens with `:`, as in Python: \
             write `else:`, not `else`",
        ),
        (
            "let f x =\n  if x > 0 then 1\n  elif x < 0 then\n    2\n  else 3",
            "write `if cond:`, not `then`",
        ),
        (
            "let f x = if x > 0: 1 else 2",
            "`:` opens a branch on its own indented lines; \
             a branch on the same line is written `if cond then value`",
        ),
        (
            "let f x =\n  if x > 0 then 1\n  else: 2",
            "a branch on the same line is written `else value`",
        ),
        // Inside brackets the offside rule is off, so there is no block to open.
        (
            "let f x = (if x > 0:\n  1\n  else 2)",
            "a branch on the same line is written `if cond then value`",
        ),
        ("let f x = if x > 0 1 else 2", "expected `then` or `:`"),
    ] {
        let err = parse(source).unwrap_err();
        assert!(
            err.message().contains(expected),
            "{source}\n-> {}",
            err.message()
        );
    }
    // Inside brackets a multi-line `if` keeps `then`, its lines being continuations.
    assert!(parse("let f x = (if x > 0 then\n  1\n  else\n  2)").is_ok());
}

#[test]
fn collection_literals_print_as_written() {
    // `{k: v}` is a Map, `{a, b}` a Set, `{}` the empty Map, and a bare brace
    // with `with` is still a record update. Each prints back in its own form.
    for (source, printed) in [
        (
            "let ages = {\"Ada\": 36, \"Alan\": 41}",
            "let ages = {\"Ada\": 36, \"Alan\": 41}",
        ),
        ("let primes = {2, 3, 5}", "let primes = {2, 3, 5}"),
        ("let one = {1}", "let one = {1}"),
        ("let empty = {}", "let empty = {}"),
        // A trailing comma and line breaks are layout only.
        (
            "let ages = {\n  \"Ada\": 36,\n  \"Alan\": 41,\n}",
            "let ages = {\"Ada\": 36, \"Alan\": 41}",
        ),
        ("let s = {\n  1,\n  2,\n}", "let s = {1, 2}"),
        (
            "let nested = {\"evens\": {2, 4}}",
            "let nested = {\"evens\": {2, 4}}",
        ),
        ("let moved = {p with x = 3}", "let moved = { p with x = 3 }"),
    ] {
        let module = parse(source).unwrap_or_else(|e| panic!("{source}: {}", e.message()));
        let out = pyfun::ast::print_module(&module);
        assert_eq!(out.trim_end(), printed, "{source}");
        assert_eq!(parse(&out).unwrap(), module, "{out}");
    }
}

#[test]
fn a_malformed_collection_literal_says_which_forms_exist() {
    for (source, expected) in [
        (
            "let bad = {1, \"a\": 2}",
            "a `{…}` literal is a `Map` (every entry `key: value`) or a `Set` (no `:`), not a mix",
        ),
        ("let bad = {\"a\": 1, 2}", "expected `:` after a map key"),
        (
            "let bad = {p x = 1}",
            "expected `:` (a `Map` literal), `,` (a `Set` literal) or `with` (a record update)",
        ),
        // The old bare record literal keeps its own migration message.
        (
            "let bad = {x = 1}",
            "record literals are now constructor-tagged",
        ),
    ] {
        let err = parse(source).unwrap_err();
        assert!(
            err.message().contains(expected),
            "{source}\n-> {}",
            err.message()
        );
    }
}

#[test]
fn an_if_without_an_else_says_what_to_write() {
    // Python's statement `if` may stop after one branch; an expression cannot.
    for source in [
        "let f x =\n  if x > 0:\n    print \"positive\"\nprint 1",
        "let g x = if x > 0 then print \"positive\"",
    ] {
        let err = parse(source).unwrap_err();
        assert_eq!(
            err.message(),
            "an `if` is an expression, so it needs an `else` (or `elif`) branch: \
             write `else ()` for one that does nothing",
            "{source}"
        );
    }
    assert!(parse("let g x = if x > 0 then print \"positive\" else ()").is_ok());
}

#[test]
fn pythons_conditional_expression_names_the_pyfun_order() {
    for source in [
        "let x = 1 if ready else 2",
        "let xs = [x if x > 0 else 0]",
        "print (a if c else b)",
    ] {
        let err = parse(source).unwrap_err();
        assert_eq!(
            err.message(),
            "Pyfun puts the condition first: write `if cond then value else other`",
            "{source}"
        );
    }
    // A `case` guard follows a pattern, and a new statement may start with `if`.
    assert!(parse("let f n =\n  match n:\n    case x if x > 0: 1\n    case _: 0").is_ok());
    assert!(parse("let a = 1\nif a > 0 then print a else ()").is_ok());
}

#[test]
fn an_if_that_opens_a_line_in_a_block_is_the_next_item() {
    // Inside a computation expression's braces there is no separator token, so
    // the conditional-expression hint must look only at an `if` on the same line.
    let source = "let job = async {\n  print 1\n  if ready then print 2 else ()\n}";
    assert!(parse(source).is_ok(), "{:?}", parse(source).err());
}

#[test]
fn membership_parses_at_comparison_precedence_and_prints_back() {
    for (source, printed) in [
        ("let a = 2 in xs", "let a = (2 in xs)"),
        ("let b = x not in {1, 2}", "let b = (x not in {1, 2})"),
        // Looser than arithmetic and application, tighter than `not`/`and`.
        ("let c = f x + 1 in ys", "let c = (((f x) + 1) in ys)"),
        (
            "let d = not 1 in xs and ok",
            "let d = ((not (1 in xs)) and ok)",
        ),
    ] {
        let module = parse(source).unwrap_or_else(|e| panic!("{source}: {}", e.message()));
        let out = pyfun::ast::print_module(&module);
        assert_eq!(out.trim_end(), printed, "{source}");
        assert_eq!(parse(&out).unwrap(), module, "{out}");
    }
    // It does not chain with another comparison.
    for source in [
        "let bad = 1 < 2 in [True]",
        "let bad = a in b in c",
        "let bad = a in b == c",
    ] {
        let err = parse(source).unwrap_err();
        assert_eq!(
            err.message(),
            "`in` does not chain with another comparison: join the two tests with `and`",
            "{source}"
        );
    }
    // `in` is a keyword now, and a computation expression's `for` still takes it.
    assert!(parse("let s = seq { for x in xs: yield x }").is_ok());
    assert!(parse("let in = 3").is_err());
}

#[test]
fn a_constructor_declares_its_fields_by_position_or_by_name() {
    use pyfun::syntax::TypeDeclKind;
    let module =
        parse("type Shape = Circle float | Rect { width: float, height: float } | Dot").unwrap();
    let Item::Type(decl) = &module.items[0] else {
        panic!("expected a type declaration")
    };
    let TypeDeclKind::Sum(variants) = &decl.kind else {
        panic!("expected a sum type")
    };
    assert_eq!(variants[0].field_names, None);
    assert_eq!(variants[0].fields.len(), 1);
    assert_eq!(
        variants[1].field_names,
        Some(vec!["width".to_string(), "height".to_string()])
    );
    assert_eq!(variants[1].fields.len(), 2);
    assert_eq!(variants[2].field_names, None);
    assert!(variants[2].fields.is_empty());
    // The printer writes each the way it was declared.
    assert_eq!(
        pyfun::ast::print_module(&module).trim(),
        "type Shape = Circle float | Rect { width: float, height: float } | Dot"
    );
    // A body needs a field, as a record declaration does.
    assert!(parse("type Shape = Rect {}").is_err());
    // `Rect {}` is a pattern (any `Rect`), printed without the inner space.
    let m = parse("let f s =\n  match s:\n    case Rect {}: 1\n    case _: 0").unwrap();
    assert!(pyfun::ast::print_module(&m).contains("Rect {}"));
}
