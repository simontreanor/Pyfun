# Pyfun — Design

The language/semantics design spec. `GUIDE.md` is the lean operational pointer for working in
this repo; this file is the detailed reference. **Status: MVP showcase complete** — lexer, parser,
span-carrying AST, pretty-printer, Hindley–Milner type inference with parameterized + recursive
**algebraic data types**, constructor patterns, exhaustiveness checking, the three **computation
expressions** (`async`/`seq`/`result`) with monadic typing, and **units of measure** (abelian-group
unit unification with unit polymorphism, erased at lowering), rustc-style diagnostics (`pyfun
check`), and lowering to a Python-AST IR + runnable Python emission gated on type-checking, over the
`measure`/`type`/`let`/`if`/`match`/`fun` subset with curried application and `|>` (see §8, §10).
Programs are now executable: a small **prelude** of Python-builtin-backed functions
(`print`/`abs`/`min`/`max`, plus a `unit` type) makes output observable, `pyfun run` compiles-and-
runs, and a **lightweight offside rule** separates top-level statements. Still deferred until its
enabling syntax exists: effect inference (and a general offside rule for nested blocks).

## 1. Identity

**Pyfun is to Python as F# is to C#.** An FP-first language that compiles to Python and
interoperates with the Python ecosystem the way F# does with C#:

- **Shared runtime + ecosystem** — runs on CPython, imports Python libraries directly; Python can
  consume Pyfun-compiled modules.
- **Different philosophy** — immutable-by-default, expression-oriented, type-rich (inference +
  ADTs + exhaustive matching + effect tracking), where Python is mutable, statement-oriented,
  dynamic.
- **Rust compiler** — language-tooling-grade front end, shipped as a standalone binary.

**Novelty & precedent.** No existing project does "F# for Python." **Hy** (Lisp → Python AST,
~12 years to 1.0) is the closest *architectural* precedent and the blueprint for lowering — but
Hy is a syntax change, not a philosophy change. Pyfun's hard parts are **semantic design and
Python interop**, not parser complexity. (Related but not Pyfun: Cython = Python→C; ty/mypy/
pyright = type checkers; RustPython = a Python interpreter in Rust; Pyrs = Python→Rust.)

## 2. The central idea: the compiler is the gatekeeper

Python compiles to *untyped* bytecode — the runtime gives no compile-time guarantees. Pyfun gets
F#-level safety the way TypeScript, Elm, and Haskell do: **the Rust compiler enforces everything
before any Python is emitted.** A failed check stops compilation and produces rustc-style
diagnostics; Python never runs.

Pipeline (each stage gated):

```
parse → type-infer/check → exhaustiveness check → immutability check → effect check
      → lower to Python-AST IR → emit readable Python
```

## 3. Safety model

What the compiler enforces, mirroring (and exceeding) F#:

- **Type safety** — Hindley–Milner inference (no annotations required). *Optional* type annotations
  (`let x : T`, `(x: T)`) are **not yet implemented** — a deferred feature (see ROADMAP); today
  everything is inferred.
- **Exhaustive pattern matching** — all ADT variants must be handled.
- **Immutable-by-default** (implemented) — `let` is immutable; `<-` reassignment of a non-`mut`
  binding is a compile error; `let mut` is the explicit opt-in. `mut` bindings are monomorphic and
  cannot take parameters. Reassignment requires statement **sequencing**, which Pyfun gets from
  indentation **blocks** (an indented `let … =` body); see §7's offside note. A closure that
  reassigns a `mut` captured from an enclosing scope lowers with a `nonlocal` (enclosing function) or
  `global` (module-level) declaration — Python otherwise treats the assigned name as a fresh local and
  the closure would miscompile. This mirrors F# 4.0, which auto-boxes a captured mutable into a
  reference cell (Python's `nonlocal`/cell mechanism is the same idea).
- **Effect discipline** — first-class (see §4).

Example diagnostics the compiler must produce (rustc-style, with spans, codes, and `help` notes):
type mismatch (`add "hello" 5`), non-exhaustive `match` (missing `None` case), reassignment of an
immutable binding.

## 4. Effect system — first-class MVP goal (implemented)

Unlike F# (which has no real effect system, only computation expressions), Pyfun treats **purity
and effects as part of the type system from the MVP.** This is a defining feature, not a
later add-on, and it shapes inference and lowering — so it must be designed in from the start.

**Implemented (inference-first, zero pollution).** Function arrows (`Ty::Fun`) carry a latent
[`Effect`] — one concrete label `io` (printing, mutation via `<-`) plus effect *variables* for
polymorphism. Effects are **inferred and never written in ordinary code**: a pure function reads
exactly as before (`let add a b = a + b`); `print : 'a ->{io} unit` and impurity **propagate
automatically** (calling an impure function makes you impure). Defining a function is pure — its
body's effect is the *latent* effect on its innermost arrow. Effect variables generalize/instantiate
alongside type/unit/num variables, so higher-order functions stay effect-polymorphic (`let pure
apply f x = f x` is pure *up to its argument*: `apply print` is impure at the call site, `apply`
itself is not). The one **opt-in, definition-level** assertion is `let pure f … = …`, which is a
compile error if the binding introduces `io`. Effects are **fully erased at lowering** (zero runtime
residue, like units); `pure` produces no Python. The sources beyond `print` are `<-` (§3) and the
Python FFI boundary — a plain `extern` is effectful-by-default (§6), the third `io` source. Surfacing
inferred effects in `pyfun check`/hover output (beyond violation messages) and adding more labels
(e.g. `async`) are later refinements; declared function types (`a -> b` in a `type`) are still pure.

Original design intent (now realized):

- **Pure by default.** A function with no observable side effects has a pure type. Purity is
  inferred, not just annotated, and propagates: a function that calls an impure function is impure
  unless the effect is discharged.
- **Effects are tracked in the type.** Start coarse — at minimum an `IO`-style effect for
  side-effecting operations (printing, mutation of `mut` state, file/network) — with room to grow
  toward an effect-row system (Koka/Eff style) rather than a single monolithic `IO`. Avoid
  over-engineering: a small, well-defined effect lattice beats a sprawling one in the MVP.
- **The Python boundary is inherently effectful.** Any call into arbitrary Python code is treated
  as impure / `unsafe` at the boundary unless the programmer asserts otherwise (see §6). Interop
  cannot be silently pure.
- **Effects must lower cleanly.** Pure code and effectful code both compile to ordinary Python;
  the effect tracking exists at compile time and leaves little or no runtime residue, the same way
  types do. Decide early how (or whether) effect wrappers appear in emitted Python — the bias is
  toward zero-cost, readable output.

**Why inference-first (the chosen model).** Effects follow Koka/Flix/Unison — **inferred, never
written in ordinary code** — rather than effects-as-values (Haskell `IO`: `do`/`<-`/wrapper types) or
effects-as-keywords (Rust/Python `async` *coloring*, the very pain we avoid). The Python gradual-typing
mindset: tooling reports the property, the source stays clean. This is why the only surface syntax is
the opt-in, definition-level `let pure` assertion — never expression-body pollution.

Still open: the exact discharge story (is `io` terminal until a runtime boundary?); whether
`async`/`Async` joins the effect lattice or stays typed via its value form; and effect annotations in
declared function types. Surfacing inferred effects in hover output is now **done** — the LSP (§9)
shows `->{io}` on arrows when you hover an expression or binding name.

**Relationship to computation expressions (§8).** Effects and CEs are distinct but related:
effects track side effects *in types*; CEs provide *monadic sugar*. They coexist (F# has CEs and
no effect system; Pyfun has both). Concretely, `async { }` introduces an `Async` effect, `seq { }`
is lazy and pure, and `result { }` is pure but short-circuiting. Keep the two mechanisms separable
in the implementation even though `async` touches both.

## 5. Lowering strategy

Lower Pyfun into a **Python-AST IR represented in Rust**, then emit readable Python source — not
by string-splicing. Rationale (the Hy lesson): accurate span mapping (Pyfun → output) for errors,
cleaner interop, and a structured target later passes can operate on. Emitted Python must stay
human-readable for debugging.

Representative mappings: `let x = e` → assignment; expression `if` → ternary `IfExp`; `match` →
Python `match` (3.10+) or an if-chain; `x |> f |> g` → `g(f(x))` (the pipe is pure
parse/lowering-time sugar, no runtime cost).

**Currying lowering (curried in the type system, n-ary in the output).** Functions are curried by
default (§7), but naive currying would emit `add(1)(2)` everywhere — unreadable and slow. Because
arities are known statically, the compiler **collapses fully-applied calls to a direct n-ary Python
call** (`add(1, 2)`) and **only synthesizes a closure** (`functools.partial` or a small curry shim)
for a *genuine* partial application (`add 1`). This keeps emitted Python idiomatic (§ goal) and
avoids per-application closure allocation — the same optimization F# performs at the IL level.

**Representation contracts.** ADTs, records, tuples, options/results, and curried/partially-
applied functions each need a *stable* Python representation. That representation is a public
contract — emitted code and interop both depend on it — so changing it is a breaking change, not
an implementation detail.

## 6. Python interop — the hard boundary

Every functional guarantee is either enforced *before* lowering or consciously *relaxed* at the
Python boundary. Python is object-centric and mutation-friendly and can defeat static checks via
`eval`/`getattr`/etc. When Pyfun calls Python, the compiler's guarantees stop at the call, and the
call is effectful by default (§4).

Mitigations to design for (not all MVP):
- Emit Python **type annotations** so `mypy`/`pyright`/`ty` can check the boundary.
- Optional, configurable **runtime type guards** at interop edges.
- A clear model for mapping Pyfun types to/from arbitrary Python objects.

**Currying at the boundary.** Currying is a Pyfun-side view; the boundary stays n-ary. Imported
Python functions are treated as uncurried and called with normal Python call syntax. A Pyfun
function *exposed* to Python presents a plain n-ary `def` signature, so Python callers never see
`f(a)(b)`. (This is exactly where the F#↔C# analogy thins: F# emits ordinary .NET methods; Pyfun
emits ordinary `def`s.)

Treat interop type-mapping and FFI surfaces as load-bearing architecture.

**The prelude (first realized interop surface).** A small set of built-in functions gives programs
something to call. The MVP prelude is `print : 'a -> unit` and the unit-polymorphic numerics
`abs`/`min`/`max : int<'u> -> …`, plus the **unit-preserving numeric conversions**
`round`/`floor`/`ceil`/`truncate : float<'u> -> int<'u>` (`round` is a bare Python builtin; `floor`/`ceil`/
`truncate` lower to `math.floor`/`ceil`/`trunc` with `import math` — the extern dotted-target path — while
staying *unqualified* Pyfun names), plus a `unit` type whose one value is written `()` (both lower to
Python `None` — the honest result of an effectful call). Each is a *typed view over a Python builtin*: the single
source of truth is `types::PRELUDE` (names + arities, read by lowering so a partial application like
`max 0` still lowers to `functools.partial`) alongside `seed_prelude` (the type schemes). Pyfun
names equal their Python names, so there is no call-site renaming — the simplest honest interop
mapping. User definitions shadow prelude names. This is deliberately tiny; collections/option/
result helpers are the obvious next increments.

**`extern` — the general FFI surface (implemented).** The "import and type an arbitrary Python
function" story is now a first-class declaration:

```
extern len : string -> int                  # Pyfun name = Python name
extern show : a -> string = str             # aliased to a Python builtin
extern pure sqrt : float -> float = math.sqrt   # dotted path; module auto-imported
```

`extern [pure] name : type [= a.b.c]` binds `name` to a Python callable (or value) at a declared
Pyfun type. Type variables are bare lowercase identifiers (as in `type` declarations) and are
generalized, so `show : a -> string` is polymorphic. The optional `= a.b.c` clause is the dotted
Python target; omitted, it defaults to the Pyfun name (the prelude convention). A reference lowers
directly to its target (`math.sqrt`), and any module prefix of a *used* extern is emitted as an
`import` (deduplicated, sorted). Arity is the number of leading arrows, so partial application of an
extern still lowers to `functools.partial` exactly like a prelude builtin. Calls are still
type-checked at the boundary (`sqrt "x"` is rejected) — but only against the *declared* type; Pyfun
trusts the annotation, which is where the §4 "effectful/unsafe at the boundary" relaxation bites.

This makes the boundary's effectful-by-default rule (§4) concrete: a plain `extern`'s innermost
arrow carries `io` (the Python call is the effect, performed on full application), so an impure
`extern` cannot be called from a `let pure` binding. `extern pure` asserts the call is effect-free
("pure up to its arguments", like `let pure`) — used for the likes of `math.sqrt`. Externs are
erased to nothing themselves; only their reference sites and imports survive lowering. The prelude
(`print`/`abs`/`min`/`max`) remains separately seeded because it needs `num`/unit polymorphism the
`extern` type syntax can't yet express.

**Lists — the eager collection (implemented).** `List a` is a built-in type that **lowers to a
Python `list`** (a dynamic array), with literal syntax `[1, 2, 3]` (comma-separated, like Python and
like Pyfun records and tuples). The big-O is Python's, *not*
F#'s linked `list`: index/`len` are O(1), append-end O(1) amortized, prepend/concat O(n). `List` is
therefore the analogue of F#'s **array**, not F#'s linked `list`. So a singly-linked `list` and its
idioms (`cons`/`head`/`tail`, `x :: xs` cons-decomposition in `match`) are a **non-goal**: a cons-cell
type would lower to un-Pythonic linked nodes, and its recursive idiom is stack-unsafe without TCO (also
a non-goal — CPython has none). Python has no built-in singly-linked list either (`deque` is
doubly-ended). The array-appropriate, Python-native counterpart — **sequence patterns** `case []` /
`case [x]` / `case [first, *rest]` over `List` — is deferred (real, not a non-goal). For now the bulk
operations are the API. The list operations are
`List.map`/`List.filter`/`List.fold`/`List.len`/`List.sum`/`List.rev`/`List.range`/`List.zip` — **module-
qualified** (see *Built-in modules* below), single source of truth `types::LIST_PRELUDE` +
`seed_list_prelude`. `List.len`/`List.sum` map name-for-name onto the Python builtins; the rest lower
to small **emitted helpers** (`_pf_map` = `list(map(...))`, `_pf_fold` = `functools.reduce(...)`, etc.)
emitted on demand like the `Result` prelude — wrappers are needed because Python's `map`/`filter` are
lazy and we want eager lists, and because a first-class curried function must be a single callable (so
partial application still lowers to `functools.partial`). The higher-order functions are
**effect-polymorphic**: `List.map : (a ->{e} b) -> List a ->{e} List b`, so mapping an impure function
makes the whole call `io` and that flows out (a single bound effect variable links the function arrow
to the traversal arrow). The lazy counterpart already exists as the `seq {}` computation expression
(§8.1).

The **completeness ops** — `get`/`isEmpty`/`contains`/`concat`/`sort`/`find` — round out the array,
each with big-O honest to a Python `list`: `get : int -> List a -> Option a` is **O(1)** and
**bounds-checked → total** (there is deliberately *no* `xs[i]` surface syntax, since bare indexing would
risk a Python `IndexError`, violating the no-runtime-surprises rule); `isEmpty` is O(1); `contains` is
**O(n)** linear (use `Set` for O(1) membership); `concat` is O(n+m) returning a fresh list; `sort :
comparison a => List a -> List a` is O(n log n) (`sorted`, so it carries the `comparison` constraint —
ADT ordering is out of scope); and `find : (a ->{e} bool) -> List a ->{e} Option a` is O(n),
**first-match/lazy** (`next(map(Some, filter …))`) and effect-polymorphic like `filter`. There is
deliberately **no cheap-looking prepend/`cons`** (O(n) on an array — the linked-list non-goal); and
because the ops are immutable-style, building a list by repeated `concat` is O(n²), so construction
stays `map`/`fold`/comprehension/`Seq`. `get`/`find` return the built-in `Option` (setting `needs_option`),
and `get` introduced a `PyExpr::Subscript` node.

**Tuples — the structural product (implemented).** `(a, b, c)` is a tuple: an anonymous, **structural**
product of two or more values — Pyfun's first structural type (records are nominal, resolved by a field
registry; a tuple type is just its element list, `Ty::Tuple(Vec<Ty>)`, unified element-wise by arity
then pairwise). The surface forms are symmetric: literal `(a, b)` (`ExprKind::Tuple`), pattern `(a, b)`
(`Pattern::Tuple`), and type annotation `(a, b)` (`TypeExpr::Tuple`), all printed and displayed with
parentheses. **Disambiguation is by precedent, no new tokens:** `()` is the unit value (not a 0-tuple —
unit *is* the empty product), `(x)` is grouping (not a 1-tuple), and `(a, b)` (a comma after the first
element) is a tuple — so a tuple always has ≥2 elements. The parser checks for a comma after the first
parenthesized element in all three positions (expression, pattern, type). Tuples **lower ~1:1 to Python
tuples** (`PyExpr::Tuple` → `(a, b)`; `Pattern::Tuple` → a sequence pattern `case (a, b):` via
`PyPattern::Sequence`). A tuple is a **single-constructor** type, so a tuple pattern of variables is
exhaustive on its own, and **deep exhaustiveness recurses into the element columns** (`Tag::Tuple(arity)`
in the Maranget matrix), reporting witnesses like `` `(false, _)` is not matched ``. Tuples unblock
multi-value return and pair lists; the stdlib follow-ons that need them — `List.zip : List a -> List b ->
List (a, b)` and `Map.ofList`/`Map.toList` (to/from a `List (k, v)`) — have landed (see the list and map
sections above).

**Sets and maps — the hashed collections (implemented).** `Set a` and `Map k v` are built-in types
that **lower to a Python `set` / `dict`**. They have **no literal syntax** (`{…}` is already records
and CE builders) and **no constructors** — built entirely from module functions, so adding them needed
no lexer/parser/AST changes, only seeded schemes + emitted helpers. The two modules (single source of
truth `types::SET_PRELUDE` / `MAP_PRELUDE` + `seed_set_prelude` / `seed_map_prelude`) are all **pure**
(unlike `List`'s higher-order trio, none take a function):
`Set.empty`/`Set.add`/`Set.remove`/`Set.contains`/`Set.len`/`Set.union`/`Set.intersect`/
`Set.difference`/`Set.ofList`/`Set.toList`, and `Map.empty`/`Map.add`/`Map.remove`/`Map.contains`/
`Map.findOr`/`Map.tryFind`/`Map.len`/`Map.keys`/`Map.values`. `Set.len`/`Set.ofList`/`Set.toList`/
`Map.len` route to bare Python builtins (`len`/`set`/`list`); the nullary `Set.empty`/`Map.empty` lower
to `set()`/`dict()`; the rest lower to small **emitted helpers** (`_pf_set_add` = `s.union([x])`,
`_pf_map_add` = `dict(list(m.items()) + [[k, v]])`, …) so the curried function is one callable (partial
application → `functools.partial`). The collections are **immutable-style**: every operation returns a
fresh container. `Map.findOr key default m` is a **total lookup with a fallback** (`dict.get`);
`Map.tryFind key m : Option v` is the optional form. `Map.ofList : List (k, v) -> Map k v` and
`Map.toList : Map k v -> List (k, v)` convert to/from a list of key/value **tuples** (`Map.ofList` lowers
to a bare `dict(pairs)`; `Map.toList` to `list(m.items())`), mirroring `Set.ofList`/`toList`. Element/key
types are **unconstrained polymorphic**
but must be **hashable at runtime** — Pyfun primitives are, and ADT/record values are too: generated
classes get a structural `__hash__` (a tuple of the type and field values, consistent with the
structural `__eq__`), so `Set Color` and `Map (Point) v` work and equal values collapse. A field that
is itself unhashable raises at hash time, the same way Python rejects an unhashable key. `Array` is
**deferred** as redundant — `List` already *is* a Python list (dynamic array).

**Option and Result — the built-in sum helpers (implemented).** `Option a` (constructors `Some`/`None`)
is seeded exactly like `Result a e` (`Ok`/`Error`): a reserved type with global constructors that lower
to `Some`/`None_` (resp. `Ok`/`Error`) classes (`None` is mangled off the Python keyword), emitted on
demand. Each has a module of combinators: `Option.map`/`bind`/`filter`/`withDefault`/`isSome`/`isNone`/
`toResult`, and `Result.map`/`mapError`/`bind`/`withDefault`/`isOk`/`isError`/`toOption`. The
mapping/binding/filtering ones are **effect-polymorphic** (like `List.map`). `Map.tryFind` returns an
`Option`; the two bridge **both ways** — `Result.toOption` (`Ok v → Some v`, `Error _ → None`) and
`Option.toResult e` (`Some v → Ok v`, `None → Error e`). A user `type Option`/`Result` is rejected (reserved).

**Strings — the `String` module (implemented).** Text operations over the built-in `string` type (which
lowers to a Python `str`), module-qualified like the collections: `String.len`/`concat`/`join`/`split`/
`upper`/`lower`/`strip`/`contains`/`startsWith`/`endsWith`/`replace`/`fromInt`/`fromFloat`/`toInt`/
`toFloat`/`toList`/`slice`/`tryIndexOf` (single source of truth `types::STRING_PRELUDE` +
`seed_string_prelude`). `slice start end s` → `s[start:end]` (total Python slicing, end-exclusive,
clamps out-of-range; emitted via a `PyExpr::Slice` node so the output reads `s[a:b]`); `tryIndexOf sub s
: … -> Option int` uses `str.find` and returns `None` when absent (total — no `IndexError`, like
`List.get`). **Naming follows the
`List` precedent** — use Python's word where it has a natural one (`len`/`upper`/`lower`/`strip`/`split`/
`join`/`replace`, matching Python's `str` methods, and consistent with `List.len`), and Pyfun's own
convention otherwise (the `contains`/`ofList`-style `toInt`/`toList`/`fromInt` family, and camelCase for
multi-word `startsWith`/`endsWith` like `tryFind`/`withDefault`). Unlike the collection preludes these
schemes are **monomorphic** (concrete over `string`/`int`/`float`/`bool`, no type variables) and all
**pure**. There is **no `char` type** — a character is a length-1 string, so `String.toList : string ->
List string` yields single-character strings and `String.join`/`concat` compose them back. Separator-first
argument order (`String.join ", " xs`, `String.split "," s`) keeps partial application natural. Lowering
mirrors the other modules: `len`/`fromInt`/`fromFloat`/`toList` route to bare Python builtins
(`len`/`str`/`list`); the rest lower to emitted `_pf_str_*` helpers (`_pf_str_upper` = `s.upper()`,
`_pf_str_split` = `s.split(sep)`, …) so each curried function is one callable. The one total parse,
`String.toInt : string -> Option int`, lowers to a `try`/`except ValueError` helper returning
`Some(int(s))`/`None_` (the first use of the general `PyStmt::Try` IR node) and pulls in the `Option`
prelude. Overloading `+` for strings is deferred — `String.concat` is the concatenation path.

**String interpolation — `f"..."` (implemented).** Python-style interpolated strings: an `f` prefix
(adjacent to the quote — `f "x"` with a space stays ordinary application, as in Python) with `{expr}`
holes holding **full Pyfun expressions**, and `{{`/`}}` for literal braces. The whole string is a
`string`; a hole may be **any type** — the emitted Python f-string stringifies it (so `f"{p}"` for a
record uses the generated `__repr__`, `Point { x = 1 }` → `Point(1)`), which is the one place Pyfun
relaxes to Python's implicit `str()` rather than requiring an explicit conversion. Holes' **effects
propagate** (`f"{impure x}"` is `io`). *Lexing* is the crux: the lexer (`lex_fstring`) splits an
`f"..."` into a `Tok::FStr(Vec<FStrPart>)` of literal chunks and holes, finding each hole's matching
`}` by balancing nested `{}` and skipping string literals, then **pre-lexes each hole's tokens with
absolute spans** (`lex_subrange`, bracket depth pre-set so no layout tokens leak in). The parser's
`parse_interp` re-parses those hole tokens with the ordinary expression grammar into
`ExprKind::Interp { parts: Vec<InterpPart> }`, so diagnostics and LSP navigation reach inside holes.
*Lowering* is 1:1: `PyExpr::FStr` emits a real Python f-string, holes verbatim, literal chunks with
their specials and braces re-escaped. This **targets Python 3.12+** (PEP 701), so a hole may freely
contain a string literal reusing the outer quote (`f"{String.contains "}" s}"`). **Self-documenting
debug holes `{x=}`** work as in Python: a single `=` as the hole's last non-whitespace character
echoes the raw hole text (everything the user typed, `=` and surrounding whitespace included) before
the value, so `f"{x = }"` prints `x = 3`. The `=` must be a genuine marker, not an operator tail —
the character before it may not be one of `=`/`!`/`<`/`>`, so `{x==y}`/`{x != y}`/`{x <= 1}`/`{x >= 1}`
stay ordinary holes. Resolved entirely at lex time (`debug_marker` in `lex_hole`): the echo joins the
preceding literal chunk and the hole's tokens exclude the marker, so parser, checker, lowering, and
emitter see an ordinary literal + hole (the value is `str()`ed like any hole, not Python's `repr`;
the pretty-printer renders `f"{x=}"` as the equivalent `f"x={x}"`). Format specifiers (`:.2f`, `!r`)
and multi-line `f"""..."""` are deferred. Covered by
lexer/roundtrip/typecheck/compile tests + `examples/hello.pyfun`.

**`try` — catching exceptions into a `Result` (implemented).** Pyfun's own code never raises (it returns
`Error`); the only reason to catch is the **Python FFI boundary** — an `extern` call can throw. So rather
than importing Python's imperative `try/except/finally/raise` (and an exception class hierarchy Pyfun has
no types for), the feature is a single **expression**: `try e : Result <e> Exception` (`ExprKind::Try`). It
runs `e`, and a thrown Python exception becomes `Error`; success is `Ok`. `try` is a prefix keyword binding
looser than `+`/comparison but tighter than `|>`, so `try parse s` is `try (parse s)` while `try parse s
|> Result.withDefault 0` pipes the `Result` out; parens capture a wider body. It does **not** change
effects — the body's `io` still happens (`try` catches a *thrown* exception, it doesn't suppress the
call). The `Error` payload is a **reserved built-in record `Exception`** with `errorKind : string` (the
class name, `type(e).__name__`) and `errorMessage : string` (`str(e)`) — read by field access (`e.errorKind`)
or matched (`case Error (Exception { errorKind = "ValueError" }): …`). It reuses the ordinary record
machinery (so its two fields join the global field registry, and a user `type Exception` is rejected as
reserved), lowered to a class emitted as **`_Exception`** so it does not shadow Python's builtin
`Exception` that the handler catches. Lowering reuses the `PyStmt::Try` node (extended with an `as <name>`
binding): `try:  t = Ok(<body>)  except Exception as e:  t = Error(_Exception(type(e).__name__, str(e)))`.
There is deliberately **no `raise`, no `finally`, no exception hierarchy** (Pyfun signals failure with
`Error`; the `result {}` CE + `Result` module compose the rest). Enabled by **string-literal patterns**
(`case "yes":`, `Pattern::Str` — a refutable leaf over the infinite `string` type, so a string `match`
still needs a wildcard), which landed alongside. Covered by roundtrip/typecheck/compile/run tests +
`examples/hello.pyfun`.

**Seq — the lazy module (implemented).** The `seq {}` CE produces a `Seq a` (a Python generator); the
`Seq` module is its lazy operation library, the counterpart to the eager `List`. `Seq.map`/`filter`/
`take`/`range` are **lazy** (they route to Python's own lazy `map`/`filter`/`itertools.islice`/`range`,
*not* the eager `_pf_*` wrappers `List` uses); `Seq.fold`/`toList` force the sequence (`Seq.fold` reuses
the list `_pf_fold` = `reduce`; `Seq.toList` = `list`). `Seq.ofList` = `iter`. `Seq.map`/`filter`/`fold`
are effect-polymorphic like `List.map` — since the effect system can't model *deferred* effects, the
function's effect is attributed at the call (sound, slightly conservative for the lazy ops). Caveat:
Python iterators are **single-pass**, unlike F#'s re-enumerable `seq` — consistent with the one-shot
generator the `seq {}` CE already produces.

**Modules — qualified namespaces.** Collection operations are **module-qualified** (`List.map`,
`Set.add`, `Map.tryFind`, `Option.withDefault`, `Seq.take`). This is what lets `len`/`contains`/`map`
reuse one name across collections without overloading or type classes (which the MVP rules out). The
built-in modules (`types::MODULES` = `List`/`Set`/`Map`/`Option`/`Result`/`Seq`/`String`; members paired in
`MODULE_PRELUDES`) and **user-declared in-file modules** (below) share one access syntax. The access
mechanism needed **no parser change**: `Module.member` is parsed as the ordinary field-access node
`Field { base: Var("List"), name: "map" }`; `types::qualified_name` recognizes an **uppercase** base
(value identifiers are lowercase, so `Upper.x` is only ever module access — a record-field base is a
lowercase value), and the checker + lowering resolve the dotted member against the module instead of as
record-field access. A genuinely global handful stay unqualified (`print`/`abs`/`min`/`max` in
`PRELUDE`), matching F# (`List.map` qualified, `abs` global). An unknown member gets a **"did you
mean"** hint — `` `startswith` is not a member of `String` (did you mean `String.startsWith`?) `` —
computed by `closest_member` (a case-insensitive match first, then edit distance ≤ ~⅓ the name, then a
prefix relation for abbreviation slips like `length`→`len`). It scans the env's qualified keys, so it
serves built-in *and* user modules, and rides the shared inference path so it surfaces in `pyfun check`
*and* LSP editor diagnostics. Names stay **single-spelling** (no camelCase/lowercase aliases) — casing is
load-bearing (`Upper.x` vs `lower.x`), so the hint is the forgiving path, not a second accepted name.

**In-file modules (implemented).** `module Name = <indented let bindings>` declares a namespace within
a file (`Item::Module`):
```
module Geometry =
  let pi = 3.14159
  let area r = pi * r * r          # siblings visible unqualified inside
let big = Geometry.area 10.0       # qualified outside
```
Members are typed in a cloned scope: each sees prior siblings **unqualified** (and qualified), but only
`Name.member` escapes to the outer env — so the bare names are not visible after the module. Lowering
flattens members to top-level defs/assignments with **mangled names** (`Geometry.area` → `Geometry_area`),
rewriting bare sibling references to the same names (`cur_module` in the lowerer); partial application
and the curry policy work unchanged (arity is registered under the qualified name). A module name can't
shadow a built-in module or duplicate another. **MVP limits:** the body holds only `let` bindings
(`type`/`measure`/`extern` inside a module are deferred), and there are no nested modules. The next
layer is the full *file-based* module system, scoped in §6.1.

### 6.1 File-based modules (Phase 2 — complete)

One module per file, referenced with an explicit `import`, compiled to a tree of readable Python files.
**All seven slices have landed** (each marked inline below); a runnable example lives in
`examples/modules/`.
The design optimizes for **Python familiarity** (the §7.1 theme): explicit imports, real Python modules,
no enforced visibility. All four shaping decisions were taken deliberately:

- **Explicit `import`, qualified use.** `import Geometry` declares the dependency edge; members are used
  qualified, `Geometry.area`. This is the core Python idiom (`import foo; foo.bar()`) and **reuses the
  existing access machinery unchanged** — `Geometry.area` is already the `Field { base: Var("Geometry"),
  name: "area" }` node that `types::qualified_name` resolves off an uppercase base. The *access* needs no
  parser change; only the `import` *statement* is new (`Item::Import { name, span }`, a new `import`
  keyword, the name a single capitalized identifier). **The syntax has landed (slice 1):** it lexes,
  parses (as an ordinary top-level item), pretty-prints, and round-trips; it is a no-op in single-file
  checking and lowering until the multi-file driver (slice 2) resolves it. (Enforcing "imports before
  other items" is left to that driver, matching the cross-file declare-before-use rule.)
  `from X import y` / `open` (unqualified import) are **deferred** (`open`-everything maps to Python's
  discouraged `import *`).
- **Parallel `.py` output.** Each `foo.pyfun` compiles to a sibling `foo.py` with a real `import`; member
  names stay **un-mangled** (`area`, not `Geometry_area` — the mangling is an in-file-module workaround we
  drop here), and a cross-module reference `Geometry.area` lowers to Python `geometry.area`. This matches
  Python expectations and the "readable Python / direct ecosystem interop" ethos, and enables
  Python↔Pyfun interop (a Python program can `import` a compiled Pyfun module and vice-versa).
- **All public.** Every top-level binding is exported; no `pub` keyword — Python has no enforced private
  (`_underscore` is convention only). Visibility control is **deferred**.
- **Implicit recursion** (landed — slice 0, independent of the rest): a *function* binding (`let f x =
  …`) is in scope in its own body, like Python's `def` — no `rec` keyword. A plain value binding still cannot
  self-refer (`let x = x` stays an error, as `x = x` is a module-level `NameError` in Python). Mechanism:
  pre-bind `f : α` (fresh) before inferring the body, unify, then generalize (standard monomorphic-
  recursion HM); lowering is unchanged (Python functions are already recursive). **Mutual recursion**
  (landed) extends this to *groups*: `run` builds the dependency graph among top-level `let` bindings
  (scope-accurate free variables, `collect_free`), finds cycles by SCC (`strongly_connected`), and infers
  each all-function cycle together (`infer_mutual_group`) — pre-bind every member monomorphically, infer
  all bodies (so `isEven` sees `isOdd` and vice versa, in any order), tie each knot, then generalize each
  against the *outer* env. Grouping only genuine cycles keeps let-polymorphism intact (an independent
  helper stays its own singleton SCC and generalizes normally). It's **implicit — no `and` keyword**
  (which would clash with the boolean `and`). A value cycle (`let a = b\nlet b = a`) is not a function
  group, so it stays rejected; and a one-way forward reference between *independent* (non-cyclic) bindings
  still requires declare-before-use. Lowering is unchanged (Python module-level `def`s resolve names at
  call time). **Tail-call optimization
  is deferred** — CPython does no TCO and caps recursion (~1000 frames), so deep recursion can
  `RecursionError`, exactly like hand-written Python; the **stack-safe path is the `List`/`Seq`
  combinators** (they lower to Python's iterative `reduce`/`map`). A future slice may rewrite
  self-tail-calls to a `while` loop in the emitted Python (the F#/Scala move; output stays readable).

**Module identity.** Source files are lowercase (`geometry.pyfun`), avoiding case-insensitive-filesystem
pitfalls; the **module name is the stem with its first letter uppercased** (`Geometry`), per Pyfun's
uppercase-identifier rule for types/modules; the emitted file keeps the lowercase stem (`geometry.py`).
Resolution maps `import Geometry` → `geometry.pyfun` by lowercasing. (Multi-word/snake_case stems and
nested/dotted packages are deferred — **flat, single-directory namespace** for the MVP.)

**Resolution & ordering** (landed — slice 2, `src/project`). A multi-file **driver**: from an entry file,
parse it, follow `import` edges (resolved relative to the entry's directory = the source root), and build
a dependency **graph**. The graph must be **acyclic** — a cycle is an error (Python tolerates import
cycles only fragilely; F# forbids them, and this is the cross-file face of declare-before-use). A
topological sort gives the compile/emit order. So "a module may only use modules declared before it"
falls out for free — no separate mechanism, and there is **no mutual recursion across modules** (merge
the files, as in F#). *Implementation:* `project::build(entry, load)` walks the graph depth-first with an
injected `load: Fn(&str) -> Option<String>` loader, so the graph/cycle/topo logic is **filesystem-free
and unit-testable**; a back-edge to a module on the DFS path is a `ProjectError::Cycle` (reported as the
path `A -> B -> A`), a `None` from the loader is a `ProjectError::Missing` (naming the importer), a
lex/parse failure is a `ProjectError::Compile` (naming the module), and the DFS post-order is the
returned topological order (dependencies first, entry last). `project::build_from_path(entry)` is the thin
`.pyfun`-file wrapper (module name = stem with first letter uppercased; `import Geometry` → `geometry.pyfun`
in the entry's directory). Cross-module *checking* and *emit* (the next slices) consume this `Project`.

**Cross-module checking** (landed — slice 3, `types::check_module` + `project::check`). Each module is
type-checked in topological order, its env seeded with every imported module's **exported value schemes**
under their qualified keys (`env.insert("Geometry.area", scheme)`) — reusing the qualified-key env the
checker already uses for built-in/in-file modules, so the existing `Field`-node access path resolves a
cross-module reference with no new lookup logic. A module's interface is its top-level **`let`
values** plus its **sum types** (since the cross-module-ADT follow-on; `ModuleExports` carries each public
sum type's name, arity, and constructors). A consumer can construct (`Geometry.Circle 2.0`) and
pattern-match (`| Geometry.Circle r ->`, a qualified constructor pattern) the imported type's values, with
**exhaustiveness checked across the boundary** (a missing arm reports the qualified witness `Geometry.Rect
_ _`). **Cross-module records / measures / externs are still deferred** (cross-module record field access
runs into the global field-uniqueness invariant; these are their own follow-on). Using a name a module does
not export is the ordinary "`x` is not a member of `Geometry`" error, located in the importing module.
*Implementation:* the single-file `run` was generalized to take the imports map and return the module's
exported value schemes **and** its exported sum types; `check_module(module, imports)` seeds imported
values under qualified keys and imported sum types into the decls under **qualified constructor keys**
(`merge_imported_types`: `decls.ctors["Geometry.Circle"]`, `type_ctors["Shape"] = ["Geometry.Circle", …]`),
so construction (the `Field` arm), qualified ctor patterns (`bind_pattern`), and exhaustiveness
(`ctor_signature`) all resolve with no special cases. Transplanting a scheme across modules is sound
because a top-level binding (and a constructor) generalizes against an env of closed schemes, so its own
scheme is closed and `instantiate` refreshes the quantified vars in the dependent module's id space.
`project::check` threads the `ModuleExports` map through the topological order, seeding each module from
only the modules it actually imports (so an unimported module's members/constructors stay invisible), and
returns errors grouped by module. *Lowering* routes a qualified constructor — in expression or pattern
position — to the imported module's class (`geometry.Circle`, dotted class pattern `case geometry.Circle(r):`,
with `import geometry` hoisted); a nullary imported constructor used as a value is called
(`palette.Red()`), and imported constructor arities are threaded so a partial application still curries.

**Output & the shared runtime** (landed — slice 4, `lowering::lower_in_project` + `project::compile`).
Each module lowers independently to its own `.py`; a cross-module `Geometry.area` emits `geometry.area`
(attribute access), with `import geometry` hoisted to the file header (reusing the lowerer's
`needed_imports` set), and imported members keep **un-mangled** names (a file module is lowered as an
ordinary top-level program — the `Geometry_area` mangling is only for *in-file* `module` declarations).
**One correctness constraint forces a shared runtime module:** the built-in `Option`/`Result`
constructors lower to *nominal* Python classes (`Some`/`None_`/`Ok`/`Error`); if each file defined its
own, an `Option` value crossing a module boundary would fail the receiver's `isinstance`/`match` checks.
So those classes are hoisted into a generated **`_pyfun_rt.py`** that every module needing them imports
(`from _pyfun_rt import Some, None_` / `Ok, Error`). `List`/`Set`/`Map`/tuples need no runtime — they are
native `list`/`set`/`dict`/`tuple`. The pure `_pf_*` helpers stay per-file for the MVP (duplication is
bloat, not bug); de-duplicating them into `_pyfun_rt.py` is a follow-on. **Single-file `compile`/`run`
output is unchanged** (`lowering::lower` still inlines the classes) — the runtime module appears only for
a multi-file program, and only when some module actually uses `Option`/`Result`. *Implementation:* the
`Lowerer` gained an `imported_modules` set (drives the `geometry.area` routing) and a `use_runtime` flag
(emit `from _pyfun_rt import …` vs inline); `lower_in_project(module, ctx)` sets them and threads
`ctx.member_arities` (the imported functions' arities) into the arity table so a **cross-module partial
application still lowers to `functools.partial`**. `project::compile` builds each module's `ImportContext`
from its imports, emits `<name>.py` per module, and appends `_pyfun_rt.py` (via `runtime_module()`) iff
any module used the nominal classes.

**CLI** (landed — slice 5, `src/main.rs`). `pyfun {check,compile,run} entry.pyfun` operate over the whole
graph: `check` checks all modules (errors rendered rustc-style against each module's own source, grouped
under a `-- module `Name` (name.pyfun) --` header); `compile -o <dir>` emits the `.py` tree (+
`_pyfun_rt.py`) into `<dir>` (no `-o` prints each file to stdout under a `# ==== name.py ====` banner);
`run` materializes the tree to a temp dir and executes `python entry.py` with the dir on the path (then
cleans up). Each command **detects imports** by parsing the entry: a file with **no imports takes the
single-file path exactly as before** (full back-compat — `compile` to stdout / one file with the classes
*inlined*, `run` piped to `python -`), and only a file that actually `import`s engages the graph driver.
The compiler stays the gatekeeper: `compile`/`run` over a project gate on a clean `project::check` first.
Graph errors (missing file, cycle, a lex/parse failure in some module) are rendered before any checking.

**LSP** (landed — slice 6). The editor analysis gains **minimal import awareness**: `analyze_in_dir(source,
dir)` resolves an imported file's export interface (via `project::resolve_imports`, a *forgiving* variant
that reads sibling `<name>.pyfun` files, resolves transitively, and silently omits a missing/broken/cyclic
import) and seeds the type-check (`types::check_collecting_with_imports`), so a multi-module file
type-checks `Geometry.area` cleanly instead of flagging "not a member" — while a genuine cross-module type
error is still reported. The server maps the document's `file:` URI to a directory (`uri_to_path`,
percent-decoding + the Windows `/C:/` fixup) and passes it in; a non-`file:` URI or a no-imports file is
analyzed exactly as before. **Limitations (acceptable for the MVP, documented):** imported modules are read
from *disk*, not the editor's unsaved buffers, and the per-URI/version cache is not invalidated when an
imported file changes (re-open/edit the dependent file to refresh). **Rich cross-file navigation is
deferred** (today's per-URI, version-cached analysis stays the core). *Cross-file navigation follow-on
(landed):* (1) **go-to-definition crosses files** — a qualified reference to an imported file module
(`Geometry.area`, `Geometry.Circle`) jumps to the definition in that module's `.pyfun`
(`resolve::qualified_at` records expression-position qualified refs with spans; the server resolves the
sibling URI and locates the member via `resolve::definitions`, reading an open buffer over disk); (2)
**workspace symbols** (`workspace/symbol`) search every definition across the project directory's `.pyfun`
files; and (3) **find-references and rename for top-level `let` values *and* constructors** span the whole
project — `Server::symbol_occurrences` scans the directory's `.pyfun` files and collects the symbol's
definition, its bare uses in the defining file, and every qualified use (`Geometry.area`,
`Geometry.Circle`) elsewhere (rewriting only the member identifier, via `member_subspan`, so the
`Geometry.` qualifier is preserved). A constructor's uses include both construction expressions *and*
patterns: `Pattern::Ctor` and the `type` variant declaration each carry a name span, and the resolver
records pattern constructors in the same reference channels as the expression forms, so the occurrence set
is complete. Rename is sound: it fires only for a top-level value or constructor (a value renames to a
value, a constructor to a constructor), and a *strict* scan **refuses** rather than do a partial rewrite if
any project file fails to parse. **Type names** also navigate and rename, but **in-file only** — there is
no qualified-type syntax, so a type name appears only in its own file's annotations (sum-variant and record
field types, `extern` types). `TypeExpr::Con` and the `type` declaration each carry a name span, the
resolver walks type annotations (`resolve::walk_type`) collecting uppercase-name occurrences, and
`resolve::type_at` / `type_use_references` drive go-to-definition, find-references, and rename (a type
renames to an uppercase type name; builtins are refused). **Still deferred:** a project-wide LSP cache.

**Post-Phase-2 follow-ons.** Landed after the seven slices: **cross-module sum-type ADTs**
(construct + qualified-pattern-match + cross-boundary exhaustiveness) and **cross-file LSP navigation**
(go-to-definition across files, workspace symbols, project-wide find-references + rename of top-level
values and constructors, and in-file find-references / rename of type names). **Explicit non-goals
(decided not to build):** visibility (`pub`) — Pyfun is
all-public by design, the Python-natural model, so enforced visibility would fight the ethos; and **TCO** —
CPython has none and the `List`/`Seq` combinators are the stack-safe path, so deep self-recursion matching
hand-written Python's `RecursionError` is acceptable. **Still deferred (no demonstrated need yet):** `from
X import y` / `open`; cross-module *records*/measures/externs (records hit the global field-uniqueness
invariant); nested/dotted packages & multi-word stem naming; de-duplicated `_pf_*` runtime; a project-wide
LSP cache.
**Implementation slices (ordered):** (0) implicit
recursion [**done**]; (1) `import` syntax + AST + pretty-print + roundtrip [**done**]; (2) multi-file
driver: graph, cycle/missing-file errors, topo sort [**done** — `src/project`, a loader-injected
DFS so the graph logic is filesystem-free and unit-testable; `build_from_path` is the `.pyfun`-file
wrapper]; (3) cross-module value checking [**done** — `types::check_module` (seeded env + exported
schemes) + `project::check` over the topo order]; (4) shared
`_pyfun_rt.py` + cross-module lowering + parallel-file emit [**done** — `lowering::lower_in_project` +
`lowering::runtime_module` + `project::compile`]; (5) CLI over the graph (temp-dir `run`,
`-o <dir>`) [**done** — import-detecting `check`/`compile`/`run` in `src/main.rs`, single-file
back-compat preserved]; (6) minimal-import-awareness LSP [**done** — `analyze_in_dir` +
`project::resolve_imports` + `types::check_collecting_with_imports`, URI→dir in the server]; (7)
docs/example [**done** — this section, plus the runnable `examples/modules/` project]. **All seven
slices have landed — Phase 2 file-based modules are complete**, plus the cross-module-ADT and
cross-file-LSP-navigation follow-ons (above).

## 7. Surface language (MVP)

Differences from Python that the MVP commits to:

| Concept        | Python                      | Pyfun (F#-style)                              |
|----------------|-----------------------------|-----------------------------------------------|
| Bindings       | `x = 1` (mutable)           | `let x = 1` (immutable); `let mut` to opt in  |
| Control flow   | `if`/`match` statements     | `if`/`match` as expressions                   |
| Pattern match  | limited `match` (3.10+)     | exhaustive, destructuring                     |
| Types          | optional, runtime-checked   | inferred, semantic; ADTs                      |
| Functions      | `def f(x): ...`             | `let f x = ...` (expression bodies), **curried** |
| Application    | `f(a, b)` n-ary             | `f a b`; `f a` is a partial application       |
| Pipe           | none                        | `x \|> f \|> g` (= `g(f(x))`)                  |
| Effects        | untracked                   | tracked in the type (§4)                      |
| Comp. exprs    | none (ad-hoc `async`/gens)  | `async {}` / `seq {}` / `result {}` (§8)      |
| Units          | none                        | units of measure, compile-time only (§8)      |

**Functions are curried by default** (F# style): `let add a b = a + b` has type
`int -> int -> int`, and `add 1` is a legal partial application of type `int -> int`. This is what
makes `|>` and point-free style pay off. Inference handles curried arrows and partial application
(standard HM); lowering keeps output readable via the n-ary-collapse strategy (§5); the Python
boundary stays n-ary (§6).

MVP language features: immutable bindings by default with checked `let mut`/`<-` and indentation
blocks (§3), expression `if`/`match`, **curried functions + partial application**, **pipe `|>`**, ADT
and **record** declarations, the three computation expressions of §8, units of measure (§8), readable
Python output. (Effect tracking is designed but deferred — §4.)

Lexical conventions: line comments start with `#` (Python-style — `//` is floor division, §7.1);
identifiers are ASCII alpha + `_`; capitalized identifiers denote constructors in pattern position
(§ patterns).

**Statement separation & blocks (general offside rule, implemented).** Indentation is turned into
block structure by a layout rule, not semicolons or braces. At lexing time a layout stack of block
columns (outside any `()`/`{}` brackets, where line breaks are always continuations) emits three
synthetic tokens: `Indent` opens a block, `Dedent` closes one, `Sep` separates two statements.
- A block opens after any **tail-position keyword** at bracket depth 0: a `let … =` body, a `match`
  arm or lambda `->`, or an `if`'s `then`/`else`. (An inline body crosses no newline, so the priming
  lapses and no block opens.) The top level is the outermost (implicit) block.
- A line on the current block's column starts a **new statement** (`Sep`) *unless* it leads with a
  continuation token (an infix operator, `|`, `then`/`else`/`with`/`and`/`or`/`in`) — none of which
  can begin a statement. A line indented *past* the block continues the current statement. So
  consecutive statements (`print a` then `print b`) are distinct, while multi-line `match`/`if` and
  CE blocks stay together.
- A **block** (any indented tail-position body) is a sequence of statements — local `let`/`let mut`,
  `<-` reassignments, expression statements — whose final expression is its value. A single-expression
  block is unwrapped, so existing one-expression bodies keep their plain form. This is what gives
  mutability (§3) the statement sequencing it needs. Blocks lower to flat Python statement sequences;
  in `match`-arm / `if`-branch / lambda positions the lowering recurses into the body, which already
  handled blocks, so they "just work". Because a block can't be parenthesized (the offside rule is off
  inside brackets), the canonical pretty-printer renders block-bearing `if`/`match`/`fun` with offside
  indentation rather than the inline parenthesized form.

The rule is orthogonal to the brace-delimited CEs and records (§8.1).

### 7.1 Numbers & arithmetic — Python-familiar (implemented)

The design for floats puts **familiarity to Python programmers first** — Pyfun brings functional
discipline, but numeric behaviour should not surprise someone coming from Python. A Python user never
sees the type machinery; they feel a few surface behaviours, and those are what this design pins
down. Both steps have shipped: the division semantics + `#` comments (step a), and the `num`
constraint with polymorphic literals (step b).

**Decisions:**

1. **`/` is true division; `//` floors; `%` is modulo; `**` exponentiates. ✅ implemented.** Pyfun `/`
   is Python `/` (result type `float`, `7 / 2 == 3.5`), `//` is Python floor division (`7 // 2 == 3`,
   result `int`), `%` is Python modulo (`10 % 3 == 1`, same `*`/`/` precedence tier), and `**` is Python
   exponentiation — **float-only and dimensionless** (`float -> float -> float`; a runtime exponent
   can't be dimensionally checked, and `int ** -1` isn't an int, so following F# it stays float),
   **right-associative**, and **tighter than unary minus** (`-2.0 ** 2.0 == -4`, `2.0 ** 3.0 ** 2.0 ==
   512`). This
   matches Python 3's most well-known numeric fact (the old floor-meaning `/` was the single most
   un-Pythonic thing in the language). To free the `//` spelling, **line comments moved from `//` to
   `#`** (Python-style — another familiarity win). `%` is num-constrained and **unit-preserving like
   `+`/`-`** (`10<m> % 3<m> : int<m>`; mixing units is an error). Bonus: because each operator maps
   1:1 to a Python
   operator, lowering stays purely syntactic — no need to consult inferred types to choose `/` vs
   `//` (the type-directed-lowering problem this would otherwise create disappears).
2. **One built-in numeric constraint, `num`. ✅ implemented.** `+ - * //` (and the prelude numerics)
   are typed with a single compiler-known constraint: `let add a b = a + b : num 'a => 'a -> 'a ->
   'a`. `int` and `float` (with any units) satisfy `num`; `bool`/`string` do not (→ "expected int,
   found bool"). Generic functions like `area`/`min`/`max` stay polymorphic over int *and* float
   *and* units — the property a hard-coded int-default would throw away. No type annotations are ever
   required (Pyfun has none anyway). Implemented as a `Ty::Num(var, unit)` variant resolved by a tiny
   `num` union-find, with `num` variables generalized/instantiated alongside type and unit variables.
   **Prefix negation `-e`** (`UnOp::Neg`) is `num`-constrained and **unit-preserving** (`-5<m> :
   int<m>`). It is a **parser-level prefix operator**, deliberately *not* a lexer negative-literal:
   a signed-literal token would make `x-1` lex as `x` applied to `-1` (the ML/F# whitespace trap), so
   instead `-` is subtraction when it has a left operand and negation when it doesn't. It binds tighter
   than `*`/`/` and looser than application (`-f x` = `-(f x)`, `2 * -3` = `2 * (-3)`), coexists with
   the `(-)` operator section, and enables **negative integer literal patterns** (`case -1:`, the sign
   folded into the pattern, as Python's `match` allows). Lowers to Python `-x`.
3. **Polymorphic numeric literals; default `int`. ✅ implemented.** An integer literal `1` has type
   `num 'a => 'a` and adapts to context, so mixed-literal arithmetic just works the Python way:
   `1 + 2.0 : float`. Float literals (`1.5`) are concretely `float`, and include **scientific notation**
   (`1e6`, `2.5e-3`, `1E3`, `6.674e-11<m^3 / kg s^2>`): the lexer consumes the exponent (including its
   sign, so `e-3` isn't handed to unary minus), a number carrying an exponent is `float` even with no
   `.`, and `e` is only consumed when a real exponent follows (so `2exp`/`1e` stay integer-then-identifier).
   An unresolved numeric defaults
   to `int` — operationally automatic rather than a separate pass: it *displays* as `int`, and since
   it lowers to an int literal that Python coerces in arithmetic, results stay correct. (Minor wart:
   a literal whose type unifies to `float` still emits as an int literal, so a *bare* such literal
   prints `7` not `7.0`; in any arithmetic Python coerces, so values are unaffected.)
4. **No implicit int→float coercion between *variables*.** Mixing two values of genuinely different
   concrete numeric type (an `int`-typed variable plus a `float`-typed one) is a (gentle) error
   rather than a silent widening. Full coercion would require subtyping (`int <: float`), which
   breaks HM principal types; literal polymorphism (decision 3) covers the cases Python users
   actually hit, so this stricter-than-Python corner is rare and is where the discipline pays off.
5. **`+ - *` stay numeric.** Python overloads `+` for string/list concatenation; Pyfun does not.
   String concatenation is a named function (or a distinct operator) later, with an error that
   steers users there. This is the one deliberate departure from Python familiarity — silent
   `+`-means-anything is exactly the dynamic mushiness Pyfun exists to replace.

6. **Comparison & equality. ✅ implemented.** `< > <= >=` carry a closed built-in **`comparison`**
   constraint (satisfied by `int`/`float`/`string`), implemented like `num` (an `ord` constraint set
   on type variables, propagated through unification and generalized), so `let lt a b = a < b`
   infers `comparison 'a => 'a -> 'a -> bool` and works at int/float/string but rejects bools and
   functions. `== !=` need **no** constraint — they're `'a -> 'a -> bool` (same type, every type has
   equality), and generated ADT classes get a structural `__eq__` so `Some 1 == Some 1`. Both produce
   `bool` and are looser than arithmetic, tighter than `|>`. Surface wrinkle: `<` opens a unit
   annotation only when *adjacent* to a literal (`5<m>`); spaced (`5 < m`) it is less-than — the F#
   rule. **Chained comparisons** are Python-style: `a < b < c` means `a < b and b < c` with each
   operand evaluated once and short-circuiting — *not* the left-associative `(a < b) < c` (a bool
   compared to `c`). A dedicated `ExprKind::Compare` node (produced when two or more comparison links
   chain; a single comparison stays `Binary`) **lowers 1:1 to Python's own chained comparison**, so
   evaluate-once and short-circuit come for free rather than via a desugaring to `and`. Each adjacent
   pair is typed independently (operands unify; ordering links carry `comparison`, equality links
   don't), and links may mix operators (`0 <= i < n`, `a == b == c`).
7. **Logical operators. ✅ implemented.** `and` / `or` / `not` — all keywords, lowering to the same
   Python keywords. Spelled the Python way rather than F#'s `&&`/`||` to stay consistent with the
   Python-familiarity theme of this section (and to lower 1:1). `not` is `bool -> bool`, `and`/`or`
   are `bool -> bool -> bool`. Precedence mirrors Python — `or` < `and` < `not` < comparison — so
   `not a == b` is `not (a == b)` and emitted Python needs minimal parentheses.
8. **Operator sections. ✅ implemented.** A binary operator wrapped in parentheses, `(op)`, is that
   operator as a first-class curried function — `(*)`, `(+)`, `(-)`, `(/)`, `(//)`, `(==)`, `(!=)`,
   `(<)`, `(>)`, `(<=)`, `(>=)` — and `(*) 2` partially applies it (F#-style). It parses to
   `ExprKind::OpFunc(BinOp)` (the parser's `(`-atom disambiguates a lone operator-then-`)` from unit
   `()`, grouping `(e)`, and tuples `(a, b)`) and **desugars to the lambda `fun a b -> a op b`**
   (`desugar::op_func`) at both inference and lowering, so the operator's own constraint
   (`num`/`comparison`), currying, and partial application all fall out with no bespoke checker or
   emitter code — the same desugar-at-use tactic the computation expressions use. The pretty-printer
   keeps the faithful `(op)` spelling. `and`/`or` are **excluded**: they are keywords, and a strict
   function value would silently drop their short-circuit evaluation (F# excludes `&&`/`||` for the
   same reason). This makes point-free style with the `List`/`Seq` combinators natural: `List.fold (+)
   0 xs`, `List.map ((*) 2) xs`.

**Why a *closed* set of built-in constraints, not user type classes.** `num` and `comparison` are
baked into the compiler; there is **no `class`/`instance` surface syntax**. The set stays closed,
which is itself the guardrail against sprawling into a Haskell-style class system (§11). Notably, **Pyfun
needs none of F#'s `inline`/SRTP machinery**: F# requires compile-time monomorphization for generic
arithmetic because `+` is a static per-type method on .NET, whereas **Python dispatches `+`/`<`/`==`
at runtime** (`__add__`/`__lt__`/`__eq__`). So a generic `add` lowers to one ordinary
`def add(a, b): return a + b` that works at runtime on whatever flows in — the constraint system
lives entirely in the type checker (for safety), and lowering stays trivial.

**What this loses** (vs a real, user-extensible type-class system): users can't declare their own
type `num`/`comparison` (e.g. a `Vector` supporting `+`), there are no custom classes or `deriving`,
and equality/ordering for user ADTs (when those land) is the compiler's call — it would auto-generate
`__eq__`/`__lt__` on emitted classes the way it already generates `__repr__`. What it keeps is the
thing that matters here: numeric and **unit** polymorphism, with Python-native surface behaviour.

**Implementation status (ROADMAP #4):** (a) ✅ `/` true division, `//` floor, comments → `#`;
(b) ✅ the `num` constraint with polymorphic literals; (c) `+ - *` stay numeric — string concat is
deferred to a later named function (no guiding error yet); plus ✅ comparison/equality operators
(`< > <= >= == !=`) with the `comparison` constraint and structural ADT equality; plus ✅ logical
`and` / `or` / `not`.

### 7.2 Pattern matching — Python-framed (implemented)

Pyfun's original `match e with | pat -> body` framing is F#. Python 3.10+ users now know a *native*
`match`/`case`, so the F# framing is the second false friend (§ discussion): the audience has muscle
memory for `match x:` / `case …:` and Pyfun spells it differently for no functional gain. This section
adopts Python's **framing** while keeping Pyfun's **pattern language** untouched — the distinction is the
whole point.

**Surface form.**
```
match <scrutinee>:
  case <pattern>: <block>
  case <pattern> if <guard>: <block>     # optional guard (see below)
  case <a> | <b>: <block>                # or-pattern (see below)
  ...
```
- `match e:` — the `:` opens an offside block of `case` arms (the scrutinee is any expression; the `:`
  at bracket depth 0 ends it).
- `case pat:` — the `:` opens the arm's **block** body (a sequence of statements whose final expression is
  the arm's value, per §7's block rule — so a `match` remains an **expression** yielding the taken arm's
  value). Inline (`case None: 0`) and indented multi-statement bodies both work, exactly as `then`/`else`
  bodies do today.
- Arms are `case`-delimited (new statements in the match block), **not** `|`-delimited. `with` and the
  leading `|` leave `match` entirely.

**What is deliberately *not* Python.** The **pattern language is unchanged**, because it is the part you
value and the part that carries Pyfun's FP surface:
- Constructor patterns stay **juxtaposition**: `case Some v:`, not Python's call-form `case Some(v):`.
  Juxtaposition is how application and construction are spelled everywhere else (§7); patterns mirror
  construction, and `Some(v)` would drag `( )`-application into patterns and fight currying.
- Tuple patterns `case (a, b):`, literal patterns, `_`, and **tagged record patterns** `case Point { x =
  0, y }:` (§8.3) are as before. Record patterns keep `{ … }` rather than becoming Python class/mapping
  patterns — consistent with tagged construction, and Pyfun has no `{ }` mapping-pattern to collide with.

**Two slots this framing frees (Python-identical), both implemented:**
- **Or-patterns.** With arms delimited by `case`, `|` inside a pattern means alternation, as in Python:
  `case 1 | 2 | 3:`. Parsed at the constructor-application level (`Some a | None` is `(Some a) | None`),
  modelled as `Pattern::Or(Vec<Pattern>)`. Every alternative must bind the **same variables at the same
  types** (checked in `bind_pattern` by binding each alternative into a temp scope and unifying); the
  exhaustiveness checker expands an or-pattern into its alternatives (`expand_first_column` in `useful`),
  and it lowers to a Python or-pattern `case a | b | c:`.
- **As-patterns.** `case p as x:` matches `p` and also binds the whole matched value to `x` (Python's
  spelling). `as` is a keyword binding looser than `|`, so `a | b as x` is `(a | b) as x`; modelled as
  `Pattern::As`. It binds `x` plus the inner pattern's variables, and is **transparent for
  exhaustiveness** — the usefulness algorithm peels it (delegating in `pattern_tag`/`row_head`/`expand_or`),
  so `Circle r as w` covers exactly `Circle` and `_ as x` is a catch-all. Lowers 1:1 to Python `case p as x`.
- **Guards.** `case pat if cond:` is a refutable arm condition, the Python spelling. The guard is checked
  in the arm's pattern-bound scope and must be `bool`; a **guarded arm never counts toward exhaustiveness**
  (`check_exhaustive` filters `guard.is_none()`, and lowering's `has_catch_all` treats a guarded arm as
  refutable). It lowers to `case pat if cond:`; because Python allows no statements in a guard, a guard
  that would need hoisted statements is a lowering error (realistic guards are pure expressions).

**Implementation scope.** The *framing* change is lexer + parser + pretty-printer only (mirroring the
"blocks in tail position" change); **guards and or-patterns** additionally reach the checker, lowering,
and Python IR (both are genuine new pattern power, not just spelling):
- **Lexer.** `case` becomes a keyword. `:` at bracket **depth 0** primes a pending block (a new
  tail-position opener alongside `=`/`->`/`then`/`else`). This is unambiguous: Pyfun puts `:` nowhere else
  at depth 0 — record field types (`x: int`) live inside `{ }` (offside off), and there are no `let`/param
  type annotations — so a depth-0 `:` is always a `match`/`case` block opener. (This quietly forbids ever
  adding a depth-0 `:` elsewhere, e.g. optional `let x : int` annotations; §8.3 decision 2 leans on their
  absence, so the dependency is already implied.)
- **Parser.** `match e:` then an indented (or, inside brackets, un-indented) `Sep`-separated sequence of
  `case` arms; each arm parses a pattern (with a top-level `|` folded into `Pattern::Or`), an optional `if`
  guard, then a block body via the existing `parse_block_or_expr`. `case` starts a new arm (default `Sep`,
  not a continuation token), so no continuation-token table change is needed.
- **Pretty-printer.** Renders `match e:` / `case p:` with offside indentation (an or-pattern parenthesized,
  a guard as ` if <cond>`), replacing the `with | … ->` rendering. A `match` embedded mid-expression still
  has an inline parenthesized form `(match e: case p: b …)`. Round-trip guarantee preserved.
- **Checker.** `MatchArm` gained `guard: Option<Expr>` (typed `bool` in the arm scope; excluded from
  coverage) and `Pattern::Or` (same-variables-same-types check; expanded for exhaustiveness).
- **Lowering / IR.** `PyCase` gained an optional guard; `PyPattern::Or` emits `a | b`. Arms still lower to
  a Python `match`/`case` statement — now an even closer 1:1 — with the defensive `case _: raise` guard.
  Witnesses still print in Pyfun pattern syntax (`` `Some false` ``, `` `Point { x = _ } ` ``).

**Migration.** This **replaces** `match … with | … ->`; the two spellings are not both supported (avoid
two ways to write one thing in an MVP). `->` is retained for lambdas (`fun x -> …`) and function types
(`int -> int`), where it does not compete with a Python form. Examples and `examples/hello.pyfun` move to
the new spelling in the same change.

**`if` is deliberately *not* `:`-framed** (unlike `match`). `if cond then a else b` stays — it is an
**expression** (Python's `if:`/`elif:`/`else:` is a statement, and Python's value-form is the backwards
`a if cond else b` ternary; neither is a good fit), it is frequently a one-liner where offside blocks
would be heavy, and `then` is not a false-friend (Python has no `then`, so nobody is *misled*). Block
branches already work via the offside opener after `then`/`else` (§3), so nothing is lost. The `:`-framing
suits multi-clause block constructs (`match`); `if/then/else` suits inline conditionals — a principled
split, not an inconsistency. The one additive familiarity win taken here is **`elif`**: pure sugar for
`else if`, parsed by `parse_if_rest` into a nested `If` in the else branch (no new AST node). The
pretty-printer canonicalizes any else-if chain (however written) to an `elif` chain, so it round-trips;
`elif` is a keyword and a statement-continuation lead (like `else`), so a chain spans lines cleanly.

**Example (with §8.3 tagged records — construction and pattern now mirror):**
```
let describe p =
  match p:
    case Point { x = 0, y = 0 }: "origin"
    case Point { x = 0 }:        "y-axis"
    case Point { x, y }:         "elsewhere"

let classify n =
  match n:
    case 0: "zero"
    case _:
      let positive = n > 0
      if positive then "positive" else "negative"

let grade n =
  if n >= 90 then "A"
  elif n >= 80 then "B"
  elif n >= 70 then "C"
  else "F"
```

## 8. Showcase features (MVP): computation expressions & units of measure

These two F# flagships are deliberately in the MVP — they are the clearest demonstrations of "what
Python can't do," and both reinforce the gatekeeper thesis (units in particular are pure
compile-time machinery erased at runtime). They are an intentional, bounded exception to the
"defer ambitious features" guardrail (§11); everything *outside* this list still gets deferred.

### 8.1 Computation expressions

F# CEs desugar `builder { ... }` into calls on a *builder* with methods like `Bind`, `Return`,
`ReturnFrom`, `Zero`, `Combine`, `Delay`, `For`, `While`. Pyfun follows the same model:

- The three built-ins (`async`/`seq`/`result`) keep **bespoke native lowerings** (await / generators /
  railway) — a generic bind/return desugar can't produce those idiomatically.
- **User-defined builders are now supported** (`src/desugar.rs`). A builder is an in-file `module`
  providing the protocol functions; `Builder { … }` (an uppercase module name before `{`) desugars to
  calls on them, after which ordinary HM inference and lowering take over — no per-builder type rules
  or codegen. The desugaring is type-directed *for free*: the builder's `bind`/`return_` signatures
  determine the types via normal inference on the desugared calls.

The protocol (F#'s, lowercased and keyword-safe); a builder need only define what its bodies use:

| item            | desugaring                                           |
|-----------------|------------------------------------------------------|
| `let! x = e` …  | `B.bind e (fun x -> …)`                              |
| `do! e` …       | `B.bind e (fun _ -> …)`   (trailing `do! e` → `e`)   |
| `let x = e` …   | `(fun x -> …) e`                                     |
| `return e`      | `B.return_ e`        (must be last)                  |
| `return! e`     | `B.returnFrom e`     (must be last)                  |
| `yield e` …     | `B.combine (B.yield_ e) (B.delay (fun _ -> …))`      |
| `yield! e` …    | `B.combine (B.yieldFrom e) (B.delay (fun _ -> …))`   |
| (empty)         | `B.zero`                                             |

`Builder { let! … }` is told from `Some { x = 1 }` (a constructor applied to a record) by one-token
lookahead: a CE body starts with a CE keyword, a record with `ident =`. `delay` receives a thunk
`unit -> m a` (force it with the unit value: `let delay f = f ()`).

The three built-ins and how they lower to Python:

| CE          | Semantics                          | Lowers to                                              |
|-------------|------------------------------------|--------------------------------------------------------|
| `async {}`  | asynchronous, `let!`/`do!` = await | Python `async def` + `await` (native coroutines); carries the `Async` effect (§4) |
| `seq {}`    | lazy sequence, `yield`/`yield!`    | Python generator functions (`yield` / `yield from`); pure, lazy |
| `result {}` | railway-oriented; short-circuit on `Error` | the `Result` ADT + early-return / nested-bind chain; pure but short-circuiting |

Notes:
- `result {}` depends on a `Result`/`Option` ADT in the prelude — its Python representation is a
  §5 representation contract.
- `async {}` is where CEs and the effect system meet; keep the `Async` effect and the CE
  desugaring as separate concerns that compose.
- Lowered output must stay readable (§5): `seq {}` should produce idiomatic generators, `async {}`
  idiomatic `async`/`await`.

**Why braces, not indentation (a deliberate choice).** F# is offside-sensitive yet still delimits
CEs with `{ }` — because braces and the offside rule solve *different* problems. The offside rule
delimits *declarations* (where a `let`/`match` body ends); the braces delimit a **builder applied
to a block** (in F#, `async`/`seq` are ordinary values, not keywords, and the braces tie the value
to the block; indentation still structures the items *inside* the braces). The deciding factor is
that a CE is an **expression in arbitrary position** — a function argument, a `let` RHS, nested in
another CE — and the offside rule is awkward at delimiting an expression embedded mid-expression.
Braces are a context-free delimiter that works identically everywhere. Python is the cautionary
case: being indentation-sensitive, it *forbids* blocks in expression position (hence the
single-expression `lambda`); an expression-oriented language that went indentation-only for CEs
would inherit exactly that limitation. So Pyfun keeps the braces deliberately, not by inheritance:

- Pyfun is currently whitespace-insensitive (no offside rule at all — `lexer/mod.rs`), so the `{ }`
  is the *only* thing delimiting a CE block today.
- The contextual-keyword scheme (`async`/`seq`/`result` are keywords *only* immediately before `{`)
  depends on the explicit brace as its disambiguator.
- A future offside rule for `let`/`match`/function bodies is **orthogonal** and composes with this
  (exactly as in F#): adding it would not require changing CE or record braces. Records (§8.3) reuse
  `{ }` as well, so the brace family stays consistent.

### 8.2 Units of measure

F#-style `[<Measure>]` types: dimensional analysis enforced at compile time, **fully erased at
runtime** — `1.0<m> / 2.0<s>` has type `float<m/s>` but compiles to a plain Python `float`. This is
the purest expression of the gatekeeper model: maximum safety, zero runtime cost, zero residue in
emitted Python.

Design intent:

- **Units are a type-system extension**, integrated with HM inference in `types/`. Units form a
  **free abelian group** (multiplication, division, integer powers, a dimensionless identity), so
  unit unification is **AC-unification / Gaussian elimination over rationals** — *not* ordinary
  syntactic unification. **This is the single hardest piece of the type checker** and should be
  designed as its own sub-module from the start.
- **Erasure at lowering:** units vanish; numeric literals/operations emit as ordinary Python
  numbers. No runtime unit objects.
- **MVP standard units:** a small SI base set + dimensionless, with **user-definable measures**
  (`type m`-style measure declarations). Keep the built-in set small (§11).
- **Derived-measure aliases (implemented):** `measure N = kg m / s^2` names a compound of base
  measures; aliases may build on earlier aliases (`measure Pa = N / m^2`). An alias **expands** to its
  base-measure unit at declaration time (stored in `Decls::measure_aliases`) and is substituted
  wherever it appears — so `<N>` and `<kg m / s^2>` are the *same* type and unify. Consequence: the
  inferred-type display shows the **expanded** form (`int<kg m/s^2>`, not `int<N>`) — there is no
  abbreviation/conversion tracking (F#'s richer model is out of scope). The alias body reuses the unit
  grammar (now also accepting `1/s` for a dimensionless numerator); aliases, like `let`s, must be
  declared before use.
- Open questions: measure-generic functions (unit polymorphism) in the MVP vs. later; how units
  interact with Python interop (units can't cross the boundary — they're erased, so the boundary
  sees plain numbers).

### 8.3 Records (implemented — constructor-tagged literals)

Named-field **product** types, complementing ADTs' sum types: `type Point = { x: int, y: int }`,
**construction `Point { x = 1, y = 2 }`**, access `p.x`, functional update `{ p with x = 3 }`.
Parameterized records (`type Box a = { item: a }`) are polymorphic.

**Motivation for the revision.** The original literal spelling `{ x = 1, y = 2 }` is a *false friend*
to Python readers: it reads as a `dict`, but a Python dict is `{ "x": 1 }` (colons, string keys) and a
Pyfun record is nominal with `=` and bare field names. Pyfun has no dict/map literal (maps are built
with `Map.ofList` / `Map.add`), so the collision is only against a reader's Python knowledge — but that
is exactly the "basic stuff should feel familiar" surface we care about (§7.1). Tagging the literal with
its type constructor kills the false friend, is **honest about nominality** (§ decision 1), matches
Haskell's record syntax (functional pedigree) *and* Python's dataclass call `Point(x=1, y=2)` (familiar),
and makes construction **mirror** its pattern (below).

Decisions:

1. **Nominal, not structural / row-polymorphic.** (Unchanged.) A record literal/access resolves to a
   *declared* record type. Records reuse the existing `Ty::Con` machinery (a record is a type constructor
   with a field registry), so no new `Ty` variant, and they unify and generalize exactly like ADTs.
2. **Field names are globally unique — retained.** Resolution of a bare `p.x` access is *still* by field
   name: an access carries no tag and no type annotation (Pyfun has none on `let`/params), so `x` must
   identify its record. Tagging construction and patterns does **not** by itself lift this — access
   remains the blocker. Reusing a field name across two records stays a compile error.

   **Why this is a real bind, not a lazy TODO — and why row polymorphism is a non-goal.** Lifting field
   uniqueness has exactly three routes, all flawed for Pyfun:
   - **Type-directed access** (resolve `p.x` from `p`'s inferred type, since construction/patterns are
     already tagged) — the "right" answer, but it *regresses* the accessor lambda `fun p -> p.x`: when
     `p` is a bare parameter its type is still a variable at the access point, so which record `x` belongs
     to is unknowable there. That case needs row polymorphism.
   - **Project-wide uniqueness** (export field registries, error on cross-module clashes) — makes
     cross-module records mechanical, but defeats module isolation: two unrelated modules couldn't both
     have a `name`/`id`/`x` field, and collisions become inevitable at scale. A PITA, rejected.
   - **Row polymorphism** — the mechanism that solves all of it cleanly (see below), but it's a whole new
     axis in the type system that Pyfun deliberately does not want.

   **Row polymorphism** is polymorphism over *the rest of a record's fields*: `fun p -> p.x` would type as
   `{ x : 'a | 'r } -> 'a`, where `'r` is a **row variable** standing for "whatever other fields are
   present," so the function works on *any* record carrying an `x`. It's what PureScript and Elm's
   extensible records are built on. Implementing it entails: a new kind of type variable (rows) alongside
   type/unit/num/effect vars; **open** record types `{ x: int | r }`; **row unification** (match common
   labels and unify tails, rewriting rows so labels line up regardless of order — ordinary syntactic
   unification isn't enough); **presence/absence ("lacks") constraints** so `{ p with x = … }` can't
   duplicate a field; reworked inference for every record operation; and noticeably noisier type errors
   (`{ x: int | r0 }` vs `Point`). Lowering is unaffected (rows erase — still a plain Python object). That
   is disproportionate machinery for **structural/extensible records the language deliberately doesn't
   have**: Pyfun records are *nominal* (mirroring Python's `dataclass`/named classes, the readable-Python
   target), and unique fields keep access dead simple while letting `fun p -> p.x` work with none of this
   apparatus. So row polymorphism is a **non-goal**, and lifting field-uniqueness remains parked behind it.

   **Cross-module records, precisely (current behavior).** Because a record's field registry is
   module-local (only *sum-type* ADTs are exported cross-module — construction, qualified patterns, and
   exhaustiveness for `Geometry.Circle`), a record **crosses a module boundary only as an opaque value**:
   the consumer can *receive, hold, and pass* a `Geometry.origin : Point` (it's an instance of a class
   emitted in `Geometry`, and an imported function like `Geometry.sumXY` can operate on it), but **cannot
   access a field (`p.x`), construct (`Geometry.Point { … }`), pattern-match, or update it** — those all
   need the record's fields in the consumer's registry, which hits the bind above. This is effectively
   ADT-style encapsulation across modules, and doing better waits on row polymorphism (the non-goal), so
   cross-module records are parked rather than deferred.
3. **Construction is constructor-tagged: `T { f = v, … }`.** A record literal in **expression position**
   always names its type: `Point { x = 1, y = 2 }`. There is **no bare `{ f = v }` literal** — that form
   is removed, which is what eliminates the dict false friend outright. The type-declaration body keeps
   bare braces (`type Point = { x: int, y: int }` — a *type* body, not an expression), and access `p.x`
   is unchanged.
4. **Update stays bare: `{ e with f = v }`.** The base expression `e` already fixes the record type, and
   the `with` keyword makes the form unambiguously an update (Python has no `{ … with … }`), so it is not
   a false friend and needs no tag. (Lowering unchanged: bind base to a temp, then a positional
   constructor call — `{ p with x = 3 }` → `_t = p; Point(3, _t.y)`.)
5. **Lowering reuses the ADT class machinery** (§5). A record type becomes a Python class with its real
   field names, `__match_args__`, structural `__eq__`/`__hash__`, `__repr__`. `Point { x = 1, y = 2 }`
   lowers to the positional call `Point(1, 2)` in declared field order — i.e. the *tag erases into the
   class name it already denotes*, so codegen is unchanged from today.
6. **Syntax disambiguation.** `{` participates in three constructs; the rule is now cleaner because bare
   expression-position literals are gone:
   - A `{` immediately after `=` in a `type` declaration is a **record-type body**.
   - `Name { … }` in expression position: peek the brace body. A computation-expression item
     (`let!`/`return`/`yield`/`do!` — the existing `starts_ce_item` lookahead) ⇒ a **CE block** and
     `Name` is a builder (§8.1). Otherwise `field = expr, …` ⇒ a **record construction** node
     `Record { ty: Name, fields }`; the checker verifies `Name` is a declared record type (error
     otherwise). This resolves the `Maybe { let! … }` vs `Point { x = 1 }` ambiguity by brace content, as
     today — only the record arm changes from "apply `Name` to a bare literal" to "construct `Name`".
   - A bare `{` in expression position must be a **functional update** (`{ e with … }`); anything else is
     a parse error (the old bare-literal path is removed). A data constructor applied to a record is now
     written with the tag explicit, `Some (Point { x = 1 })`, not `Some { x = 1 }`.
   - `.field` is a postfix binding tighter than application (`f p.x` is `f (p.x)`). (Unchanged.)

**Record patterns** in `match` are correspondingly **tagged**: `case Point { x = 0, y }:` (see §7.2). The
form is `T { name = pat, … }`, with `{ x }` shorthand binding field `x` to a same-named variable. Tagging
makes the pattern **mirror construction** and matches the scrutinee's record type explicitly. A pattern
may name a **subset** of fields (omitted fields go unmatched). It lowers to a Python keyword class pattern
(`case Point(x=0, y=y):`). A record pattern whose named sub-patterns are all irrefutable acts as a
catch-all for exhaustiveness.

**Exhaustiveness is deep** and is **entirely unaffected** by this revision — the check operates on the
`Pattern` AST (Maranget usefulness, matrix specialization), which is unchanged; only the surface spelling
of construction and record patterns moves. `Point { item = Some n } | Point { item = None }` is still
recognized as complete without a `_`, and witnesses print in the tagged form (`` `Point { x = _, y = true
} ` ``). Infinite types (`int`, `string`) and types without matchable constructors are exhaustive only via
a wildcard arm.

**Alternative considered — distinctive delimiters `{| … |}`** (OCaml/F#-anonymous flavour, e.g.
`{| x = 1, y = 2 |}`). It also removes the false friend with a simpler grammar (no type-name
classification), but is noisier per use, *looks* structural/anonymous (dishonest about Pyfun records being
nominal), and gains no symmetry with construction-vs-pattern. Rejected in favour of the tag, which buys
familiarity (dataclass/Haskell) and honesty. Neither option lifts field-uniqueness (decision 2).

Deferred: derived ordering on records, and lifting the unique-field-name restriction.

## 9. Project layout (planned Rust crate)

Keep modules small and single-purpose — exhaustiveness, type+effect inference, and codegen each
grow large and must not bleed together.

```
src/
  lexer/           tokenizer, token types, lex errors
  parser/          recursive-descent + precedence climbing; ast.rs = Expr/Pat/Ty/Stmt
  ast/             traversal + visitor utilities, pretty-printer
  desugar/         computation-expression desugaring (§8.1): builder{} → bind/return/…
  types/           HM inference + effect inference/checking, exhaustiveness
    units/         units-of-measure inference: abelian-group unit unification (§8.2)
  lowering/        Pyfun AST → Python-AST IR; scope/name-binding analysis; unit erasure
  python_emitter/  Python-AST IR → readable source
  diagnostics/     rustc-style errors: codes (E001…), levels, spans, notes
  cli/             clap-based; subcommands compile/check/fmt/lsp
  lsp/             front-end-first language server (stdio JSON-RPC) — IMPLEMENTED
    json.rs        hand-rolled, dependency-free JSON value + parser + serializer
prelude/           Pyfun/Python runtime support (Result/Option ADTs, etc.)
editors/vscode/    minimal VS Code client that launches `pyfun lsp`
tests/             parser tests, compile tests, .pyfun fixtures (favor snapshot/golden tests)
```

**Build order:** `lexer` + `parser` + `ast` → `desugar` → `types` (incl. `units`) →
`lowering` + `python_emitter` → `diagnostics` + `cli` → `lsp`.

**The LSP (implemented).** `pyfun lsp` runs a small language server over stdio. It
speaks LSP/JSON-RPC with `Content-Length` framing; to keep the crate
**dependency-free** (no `serde`/`lsp-types`), the JSON value type, parser, and
serializer are hand-rolled in `src/lsp/json.rs` — the same choice as the
hand-rolled lexer/parser. The message-handling core (`Server::handle`) is pure
(JSON in → JSON out) so it is unit-tested without spawning a process; a separate
integration test (`tests/lsp.rs`) drives the real binary over piped stdio. Four
features, all reusing the existing front end:

- **Diagnostics** — the existing type/effect/unit/exhaustiveness errors, streamed
  as `textDocument/publishDiagnostics` on open/change (full document sync).
- **Hover-for-type** — the inferred type of the narrowest expression, binding name,
  **parameter, or pattern variable** under the cursor, **with latent effects** shown
  on arrows (e.g. `string ->{io} unit`). This is the display half of the type+effect
  system: Pyfun types are inferred and never written, so hover is the only way to
  *see* one without provoking an error. It works because the checker, in a
  `record`-enabled pass (`types::check_collecting`, surfaced via `analyze`),
  accumulates a `(span, ty)` table for every expression node, binding name, function
  parameter, and pattern variable, then resolves each entry against the final
  substitution and renders it. Bindings carry a `name_span`, and parameters /
  pattern variables carry their own spans, so a function name hovers to its full
  inferred signature and a parameter hovers to its element type.
- **Go-to-definition** — jump from a reference to its definition, **module-level or
  local**. Backed by a dependency-free name resolver (`src/lsp/resolve.rs`) that
  walks the parsed AST (independent of the type checker, so it works on any program
  that *parses*): `definitions` collects module-level symbols (top-level `let`s with
  their precise name span; constructors / type / record decls / `extern`s at their
  declaration), and `references` resolves every identifier occurrence to a `Target`
  — either a `Local` binder (function parameter, block-local `let`, pattern
  variable, or computation-expression `let`/`let!`, resolved to the binder's own
  span) or a `Module` symbol (resolved by name against `definitions`). The walk
  tracks lexical scopes so an inner binding correctly shadows an outer one — every
  local binder now carries a span, so all are resolvable.
- **Find-references** — every occurrence of the symbol under the cursor (the
  inverse of go-to-definition, reusing the same resolver). The cursor may sit on a
  *use* or the *definition/binder* itself: `symbol_at` maps the offset to its
  occurrence span and a `Target` (the narrowest enclosing reference / local-binder /
  definition span wins), then `find_references` returns all references with that
  target plus, when the request's `context.includeDeclaration` is set, the
  declaration(s). Works for both locals (all binder spans are collected during the
  walk, so even an unused binder is recognized) and module symbols.
- **Rename** — rewrite every occurrence (declaration included) of the symbol under
  the cursor to a new name, returned as a `WorkspaceEdit`. Built directly on
  `symbol_at` + `find_references`. `prepareRename` validates first and returns the
  identifier's range. Only **locals** and top-level **`let` values** are renameable
  — their every occurrence is a precise span; constructors / types / `extern`s are
  refused, because their declaration span covers the whole declaration and their
  type-annotation uses aren't tracked as references, so a rename would be unsound.
  The new name must be a valid lowercase value identifier (not a keyword). No
  capture-avoidance check is done (renaming to a name already bound nearby can
  shadow) — the editor shows the diff for review.
- **Completion** — in-scope module symbols (from whatever the recovering parser
  produced — see below, so even a partially-typed file contributes its symbols)
  plus the always-available prelude (`PRELUDE` + `LIST_PRELUDE`), builtins
  (`Ok`/`Error`, the builtin/reserved type names), and keywords, each tagged with a
  `CompletionItemKind`. The static set is the fallback when nothing parses.
- **Document symbols** — the editor outline: every module-level definition as a flat
  `DocumentSymbol[]`, reusing the same `resolve::definitions` (each with a precise
  `range`/`selectionRange` and an LSP `SymbolKind` icon). Works on whatever parsed,
  so a partial file still outlines its good items.
- **Resilient & incremental analysis** — a half-typed file still yields results.
  The parser has an error-recovering entry point (`parser::parse_recover →
  (Module, Vec<ParseError>)`) used by the editor (the compiler keeps the strict
  `parse`, as it must reject any broken program): on a failed item it records the
  error, guarantees forward progress, then `synchronize`s to the next item
  boundary (a statement separator at block depth 0, tracking `Indent`/`Dedent` so a
  separator *inside* a broken block isn't mistaken for it). So one broken `let` no
  longer hides the rest of the file — the items that parse still drive hover and
  navigation, and only the *syntax* errors are reported until the file is clean (a
  type error over a partial module is noise), at which point the type errors take
  over. `analyze` returns an `Analysis { module, diagnostics, types, parse_ok }`
  bundle; **lexing errors remain fatal** (no AST) and **rename requires `parse_ok`**
  — a partial module could hide occurrences in the unparsed region, so the mutating
  feature stays conservative while the read-only ones degrade gracefully. The
  "incremental" half is a per-document analysis cache keyed on a monotonic version
  stamp: repeated requests on an unchanged document (hover, then go-to-def, then
  references) reuse one parse + type-check instead of redoing it each time.

The AST changes that enable local navigation: function/binding parameters are
`Param { name, span }` (was `Vec<String>`), `Pattern::Var { name, span }` (was
`Var(String)`), and the `CeItem::Let`/`LetBang` variants carry a `name_span`. The
spans are `NodeSpan` (which compares equal unconditionally), so roundtrip/structural
equality is unaffected; lowering erases them (`param_names`).

Deferred (next LSP slices, `ROADMAP` #10): *truly* incremental reparsing (today a
change re-analyzes the whole document — fine at this size; the version cache only
avoids redundant re-analysis between requests on the *same* version, not partial
reparse on edit); resilience to *lexing* errors (only parse errors recover today);
workspace symbols (project-wide, vs. today's per-document outline); and richer hover
(docs, separate effect line). The `editors/vscode/` client is intentionally thin —
all language smarts live in the Rust server.

**Syntax highlighting (TextMate grammar).** Separate from the LSP's semantic
smarts, `editors/vscode/pyfun.tmLanguage.json` gives static, parse-free
highlighting (keywords, declarations, types/constructors, numbers + adjacent unit
annotations, operators, strings/comments). One deliberate design choice: the
**escape-hatch tokens are flagged in a caution colour** to signal the opt-outs
from Pyfun's immutable-by-default / effect-checked defaults — `mut` (the
mutability opt-out), `<-` (the act of mutation), and `extern` (the untyped,
effectful-by-default Python FFI boundary). `pure` deliberately stays a neutral
`storage.modifier` (it's an *encouraged* assertion, the opposite of an escape
hatch), and `->` is scoped apart from `<-` so only the reassignment arrow is
flagged. The colour is applied via **honest TextMate scopes plus a pinned
foreground**, not by borrowing a "warning" scope: `mut` →
`storage.modifier.mutable.pyfun`, `<-` → `keyword.operator.mutation.pyfun`,
`extern` → `keyword.other.extern.pyfun` (each names what the token *is*), and the
extension pins all three to an amber `#CC5E00` (no italic) via
`contributes.configurationDefaults.editor.tokenColorCustomizations`. Pinning the
colour rather than relying on a theme's rendering of, say, `invalid` keeps the hue
consistent across themes and light/dark auto-switching, and avoids the semantic
lie that these valid keywords are errors (an earlier `invalid.deprecated` scoping
also picked up theme-specific italics). Users can still override the colour in
their own `editor.tokenColorCustomizations`.

## 10. Scope & phases

MVP = "immutable, expression-oriented, effect-tracked, FP-first syntax — with computation
expressions and units of measure — that compiles to readable Python," optimizing for
compiler-pipeline and diagnostics quality over feature breadth.

- **Phase 1 — parse:** lexer + AST + pretty-printer; tiny subset (`let`, `if`, `match`, `fn`);
  roundtrip test (parse → print → parse). Add CE-block and unit-literal *syntax* here so later
  phases have something to chew on.
- **Phase 2 — lower:** lowering + emitter; `pyfun compile foo.pyfun` produces a runnable `.py`.
  Includes CE desugaring (§8.1) and unit erasure (§8.2) into ordinary Python.
- **Phase 3 — check + CLI:** type **and effect** inference, exhaustiveness, immutability, **and
  unit inference**; `pyfun check`; good errors for reassignment, missing arms, type/effect/unit
  mismatches.
- **Phase 4+ — tooling:** formatter (`pyfun fmt`); LSP / editor support — **landed**
  (`pyfun lsp`: diagnostics + hover-for-type/effect + go-to-definition + completion over stdio, plus a
  thin VS Code client; see §9); then user-defined CE builders and unit polymorphism if not already in.

Because effects, CEs, and units are all MVP, their checking lands in Phase 3 alongside HM type
inference — not deferred. Units-of-measure unification (§8.2) is the highest-risk item in Phase 3;
spike it early.

## 11. Non-goals / guardrails

**Scope creep is the #1 project risk.** A neat transpiler becomes a multi-year language by
accreting features. The MVP showcase set (§8) is a *deliberate, fixed* exception — everything
outside it is deferred. Hold the line:

- **Do not fork CPython** — Pyfun is a front end targeting Python, full stop.
- Beyond the MVP (effects + the three CEs + units), defer **user-defined CE builders**, **unit
  polymorphism** (if not trivially free), macros, and a package manager until the core is solid.
- Ship **exactly three** built-in computation expressions (`async`/`seq`/`result`) — no more — and
  a **small** built-in unit set. Generality comes after the MVP proves out.
- Syntax is cheap; resist inventing more. Parser quality, error quality, and predictable lowering
  are what make the language usable — spend effort there.
- Keep the effect lattice small until real programs justify expanding it.

## 12. Naming (decided)

- Prose: **Pyfun** (capitalized like "Python"); never "PyFun" (reads as two words).
- Machine-facing: lowercase `pyfun` — CLI command, Rust crate, PyPI package, repo.
- File extension: **`.pyfun`**. CLI: `pyfun compile foo.pyfun`, `pyfun check foo.pyfun`, later
  `pyfun fmt`, `pyfun lsp`.
- `pyfun-lang` is the distribution fallback if PyPI/GitHub `pyfun` collide with abandoned
  existing projects; the crate and prose name stay `pyfun`/Pyfun regardless.
