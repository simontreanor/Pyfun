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

- **Type safety** — Hindley–Milner inference; annotations optional but semantic.
- **Exhaustive pattern matching** — all ADT variants must be handled.
- **Immutable-by-default** (implemented) — `let` is immutable; `<-` reassignment of a non-`mut`
  binding is a compile error; `let mut` is the explicit opt-in. `mut` bindings are monomorphic and
  cannot take parameters. Reassignment requires statement **sequencing**, which Pyfun gets from
  indentation **blocks** (an indented `let … =` body); see §7's offside note.
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
`abs`/`min`/`max : int<'u> -> …`, plus a `unit` type (one value, lowers to Python `None` — the
honest result of an effectful call). Each is a *typed view over a Python builtin*: the single
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
like Pyfun records — there are no tuples, so commas are unambiguous). The big-O is Python's, *not*
F#'s linked `list`: index/`len` are O(1), append-end O(1) amortized, prepend/concat O(n). So the
linked-list idioms (`cons`/`head`/`tail`, `match`-on-cons) are a poor fit and are deferred along with
list patterns; the bulk operations are the API. The list operations are
`List.map`/`List.filter`/`List.fold`/`List.len`/`List.sum`/`List.rev`/`List.range` — **module-
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
`Map.tryFind key m : Option v` is the optional form. There is **no `Map.ofList`** (no tuples to express
a pair list — build with `Map.empty` + `Map.add`). Element/key types are **unconstrained polymorphic**
but must be **hashable at runtime** — Pyfun primitives are, and ADT/record values are too: generated
classes get a structural `__hash__` (a tuple of the type and field values, consistent with the
structural `__eq__`), so `Set Color` and `Map (Point) v` work and equal values collapse. A field that
is itself unhashable raises at hash time, the same way Python rejects an unhashable key. `Array` is
**deferred** as redundant — `List` already *is* a Python list (dynamic array).

**Option and Result — the built-in sum helpers (implemented).** `Option a` (constructors `Some`/`None`)
is seeded exactly like `Result a e` (`Ok`/`Error`): a reserved type with global constructors that lower
to `Some`/`None_` (resp. `Ok`/`Error`) classes (`None` is mangled off the Python keyword), emitted on
demand. Each has a module of combinators: `Option.map`/`withDefault`/`isSome`/`isNone`, and
`Result.map`/`mapError`/`bind`/`withDefault`/`isOk`/`isError`/`toOption`. The mapping/binding ones are
**effect-polymorphic** (like `List.map`). `Map.tryFind` returns an `Option`; `Result.toOption` bridges
the two (`Ok v → Some v`, `Error _ → None`). A user `type Option`/`Result` is rejected (reserved).

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
built-in modules (`types::MODULES` = `List`/`Set`/`Map`/`Option`/`Result`/`Seq`; members paired in
`MODULE_PRELUDES`) and **user-declared in-file modules** (below) share one access syntax. The access
mechanism needed **no parser change**: `Module.member` is parsed as the ordinary field-access node
`Field { base: Var("List"), name: "map" }`; `types::qualified_name` recognizes an **uppercase** base
(value identifiers are lowercase, so `Upper.x` is only ever module access — a record-field base is a
lowercase value), and the checker + lowering resolve the dotted member against the module instead of as
record-field access. A genuinely global handful stay unqualified (`print`/`abs`/`min`/`max` in
`PRELUDE`), matching F# (`List.map` qualified, `abs` global).

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
(`type`/`measure`/`extern` inside a module are deferred), and there are no nested modules. Remaining next
layer: the full *file-based* module system (one module per file, `import`, a resolver + dependency
graph, visibility, multi-file LSP) — a separate, larger initiative.

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

1. **`/` is true division; `//` floors. ✅ implemented.** Pyfun `/` is Python `/` (result type
   `float`, `7 / 2 == 3.5`), and `//` is Python floor division (`7 // 2 == 3`, result `int`). This
   matches Python 3's most well-known numeric fact (the old floor-meaning `/` was the single most
   un-Pythonic thing in the language). To free the `//` spelling, **line comments moved from `//` to
   `#`** (Python-style — another familiarity win). Bonus: because each operator maps
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
3. **Polymorphic numeric literals; default `int`. ✅ implemented.** An integer literal `1` has type
   `num 'a => 'a` and adapts to context, so mixed-literal arithmetic just works the Python way:
   `1 + 2.0 : float`. Float literals (`1.5`) are concretely `float`. An unresolved numeric defaults
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
   rule.
7. **Logical operators. ✅ implemented.** `and` / `or` / `not` — all keywords, lowering to the same
   Python keywords. Spelled the Python way rather than F#'s `&&`/`||` to stay consistent with the
   Python-familiarity theme of this section (and to lower 1:1). `not` is `bool -> bool`, `and`/`or`
   are `bool -> bool -> bool`. Precedence mirrors Python — `or` < `and` < `not` < comparison — so
   `not a == b` is `not (a == b)` and emitted Python needs minimal parentheses.

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

## 8. Showcase features (MVP): computation expressions & units of measure

These two F# flagships are deliberately in the MVP — they are the clearest demonstrations of "what
Python can't do," and both reinforce the gatekeeper thesis (units in particular are pure
compile-time machinery erased at runtime). They are an intentional, bounded exception to the
"defer ambitious features" guardrail (§11); everything *outside* this list still gets deferred.

### 8.1 Computation expressions

F# CEs desugar `builder { ... }` into calls on a *builder* with methods like `Bind`, `Return`,
`ReturnFrom`, `Zero`, `Combine`, `Delay`, `For`, `While`. Pyfun follows the same model:

- **Build a general CE desugaring pass** (the `builder { e }` → bind/return/… transform) so the
  mechanism generalizes later — but **ship only three built-in builders in the MVP**. User-defined
  builders are **post-MVP**. This keeps scope bounded while proving the feature.
- The desugaring is **type-directed in spirit** (the builder determines `Bind`'s type), but with
  only three known builders the MVP can desugar against fixed, known signatures rather than a
  general builder-resolution algorithm.

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
- Open questions: measure-generic functions (unit polymorphism) in the MVP vs. later; how units
  interact with Python interop (units can't cross the boundary — they're erased, so the boundary
  sees plain numbers).

### 8.3 Records (implemented)

Named-field **product** types, complementing ADTs' sum types: `type Point = { x: int, y: int }`,
construction `{ x = 1, y = 2 }`, access `p.x`, functional update `{ p with x = 3 }`. Parameterized
records (`type Box a = { item: a }`) are polymorphic.

Decisions (all ✅ implemented):

1. **Nominal, not structural / row-polymorphic.** A record literal/access resolves to a *declared*
   record type. Records reuse the existing `Ty::Con` machinery (a record is just a type constructor
   with a field registry), so no new `Ty` variant, and they unify and generalize exactly like ADTs.
2. **Field names are globally unique.** Resolution is by field name: `e.x` and `{ x = … }` find their
   record type from the field(s) alone. Pyfun has **no type annotations** on `let`/params, so there is
   no other signal to resolve a bare `.x` against — and row polymorphism (inferring "some record with
   field `x`") is explicitly out of scope (§11, keep the MVP bounded). Reusing a field name across two
   records is therefore a compile error. This is the one real ergonomic limitation; lifting it needs
   either annotations or row polymorphism.
3. **Lowering reuses the ADT class machinery** (§5 representation contract): a record type becomes a
   Python class with its real field names, `__match_args__`, structural `__eq__`/`__hash__`, and
   `__repr__`.
   Literals and updates emit **positional** constructor calls in declared field order; an update binds
   its base to a temp first so it is evaluated once (`{ p with x = 3 }` → `_t = p; Point(3, _t.y)`).
4. **Syntax disambiguation.** `{` collides with computation-expression blocks, so: a `{` immediately
   after `=` in a `type` declaration is a record body; a CE block is always preceded by a builder name
   (`async`/`seq`/`result`); a bare `{` in expression position is a record literal (`{ ident = …`
   lookahead) or update (`{ expr with … }`). `.field` is a postfix that binds tighter than application
   (`f p.x` is `f (p.x)`).

**Record patterns** in `match` are supported: `match p with | { x = 0, y } -> …`. The form is `{ name =
pat, … }`, with `{ x }` shorthand binding field `x` to a same-named variable. The owning record type is
resolved from the (globally unique) field names, so a pattern may name a **subset** of fields (omitted
fields go unmatched). They lower to Python keyword class patterns (`case Point(x=0, y=y):`). A record
pattern whose fields are all irrefutable acts as a catch-all for exhaustiveness.

**Exhaustiveness is deep.** The checker uses Maranget's usefulness algorithm (matrix specialization),
not a top-level constructor scan, so it recurses into nested patterns: `Some true | Some false | None`
and `{ item = Some n } | { item = None }` are recognized as complete without a `_`. When a `match` is
non-exhaustive it reports a concrete witness — `` `None` ``, `` `Some false` ``, `` `{ x = _, y = true
} ` `` — naming an uncovered value. Infinite types (`int`, `string`) and types without matchable
constructors are exhaustive only via a wildcard arm.

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
