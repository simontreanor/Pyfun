# Pyfun — Roadmap

Possible next steps now that the MVP showcase set (curried functions + `|>`, ADTs +
exhaustive matching, computation expressions, units of measure) is complete. Each entry notes
what it is, what it unlocks, and rough effort/risk. See [`DESIGN.md`](./DESIGN.md) for the full
design and [`GUIDE.md`](./GUIDE.md) for current status.

## Language features (the remaining vision)

### 1. Effect inference (`DESIGN.md` §4) — ✅ done (inference-first, with `let pure`)
The last first-class showcase pillar, and where Pyfun out-designs F# (which has no effect system).
The type system now tracks *what a function does*, not just what values are.
- **Shipped (low-pollution, inference-first — Koka/Flix/Unison model).** Function arrows (`Ty::Fun`)
  carry a latent `Effect` — one `io` label (printing, mutation via `<-`) plus effect *variables* for
  polymorphism. Effects are **inferred and never written in ordinary code**: `let add a b = a + b`
  is unchanged; `print : 'a ->{io} unit` and impurity **propagate automatically** through calls.
  Defining a function is pure (its body's effect is latent on the innermost arrow); effect vars
  generalize/instantiate alongside type/unit/num vars, so higher-order functions stay
  effect-polymorphic. The one opt-in, definition-level assertion is **`let pure f … = …`** — a
  compile error if the binding introduces `io` (effect variables are fine: "pure up to its
  arguments", so `let pure apply f x = f x` is accepted while `apply print` is impure at the call
  site). Implemented with an effect accumulator (`cur_eff`) + open-row effect unification; **fully
  erased at lowering** (`pure` produces no Python). Covered across typecheck/compile/roundtrip tests.
- **Still to do:** more labels (e.g. `async`); effect annotations in declared function types
  (currently treated as pure). **Done since:** the FFI boundary is now effectful-by-default (#9), and
  inferred effects now surface on **LSP hover** (`->{io}` on arrows, #10) — the display half.

### 2. Records — ✅ done (nominal MVP)
Named-field product types: `type Point = { x: int, y: int }`, construction `{ x = 1, y = 2 }`,
field access `p.x`, and functional update `{ p with x = 3 }`. ADTs give *sum* types (tagged
variants); records give ergonomic *product* types with named instead of positional fields.
- **Shipped:** **nominal** records reusing `Ty::Con`. A `{` after `=` in a `type` decl is a record
  body; a bare `{` atom is a literal (`{ ident = …` lookahead) or update (`{ expr with … }`); `.field`
  is a tight postfix. Records lower to Python classes (reusing the ADT class machinery — named
  fields, `__match_args__`, structural `__eq__`/`__hash__`, `__repr__`); literals/updates emit positional
  constructor calls in declared field order, an update binding its base to a temp first. Parameterized
  records (`type Box a = { item: a }`) are polymorphic; fields generalize/instantiate like ADT
  constructors. Covered across lexer/parser/typecheck/compile/roundtrip tests.
- **MVP limitation:** field names are **globally unique**, so `e.x` / `{ x = … }` resolves its record
  type from the field name alone — Pyfun has no type annotations to disambiguate, and row polymorphism
  is out of scope. Reusing a field name across records is a compile error.
- **Still to do:** record *patterns* in `match`, derived ordering, and lifting the unique-field-name
  restriction (needs annotations or row polymorphism / type-directed field resolution).

### 3. Mutability checking (`let mut`) — ✅ done (with blocks + general offside)
Immutable-by-default with a checked `mut` opt-in: `let mut x = …` and `x <- v` reassignment, where
reassigning a non-`mut` binding is a compile error.
- **Shipped (the "go big" version):** this required real **statement sequencing**, so it landed
  together with a **general offside rule** and **block expressions**:
  - **General offside rule** (lexer): a layout stack emits `Indent`/`Dedent`/`Sep`. The one block
    opener is an indented `let … =` body; a deeper line, or one led by a continuation token (infix
    op, `|`, `then`/`else`/…), continues the current statement, so multi-line `match`/`if`/CE still
    parse. Replaces the old top-level-only `Sep` rule (#9b).
  - **Blocks** (`ExprKind::Block`): an indented `let` body is a sequence of statements — local
    `let`/`let mut`, `<-` reassignments, and expression statements — whose final expression is the
    value. A single-expression block is unwrapped, so existing bodies keep their plain AST. Local
    `let`s are scoped and generalized (let-polymorphism); non-final statements must be `unit` (no
    silently dropped values).
  - **`<-`** (`ExprKind::Assign`, type `unit`): checked against a `Scheme.mutable` flag; `let mut`
    bindings are monomorphic and can't take parameters. Lowers to plain Python assignment (blocks →
    flat statement sequences; the curried n-ary lowering and expression-bridging handle the rest).
  - Covered across lexer/parser/typecheck/compile/roundtrip tests; `hello.pyfun` shows a block with
    local mutation. The indentation-aware pretty-printer round-trips blocks.
- **Carrier for effects (#1):** `<-` is the first real `io`-style effect source beyond `print`, so
  effect inference can now be bundled here (decided 2026-06-21).
- **Blocks in every tail position — ✅ done:** blocks now open after `=`/`->`/`then`/`else`, so
  `match` arms, `if` branches, and lambda bodies take multi-statement blocks (lexer primes on those
  tokens; parser uses `parse_block_or_expr`; lowering/typing were already position-agnostic; the
  pretty-printer gained an offside `print_layout`/`print_body` path since blocks can't be
  parenthesized).
- **Closure capture of a reassigned `mut` — ✅ done:** a closure that `<-`-reassigns a captured `mut`
  now emits `nonlocal` (enclosing function) or `global` (module-level) instead of silently
  miscompiling to a Python `UnboundLocalError`. `lower_fn_body` + `scan_scope` detect captured
  reassignments; classification uses a `fn_local_stack`. Mirrors F# 4.0's auto-ref-cell for captured
  mutables.

### 4. Float arithmetic / numeric constraint — ✅ done (`DESIGN.md` §7.1)
Python-familiar numerics via a single closed built-in constraint. Both steps shipped:
- **✅ Step (a):** `/` is true division → `float` (`7 / 2 == 3.5`); new `//` floors → `int`. To free
  `//`, line comments moved to `#` (Python-style). Each operator maps 1:1 to a Python operator, so
  lowering stays syntactic.
- **✅ Step (b):** one built-in `num` constraint with **polymorphic integer literals** (so `1 + 2.0`
  works; an unresolved numeric defaults to/displays as `int`). `let add a b = a + b` infers
  `num 'a => 'a -> 'a -> 'a` and runs at both int and float; `abs`/`min`/`max` and `area` stay
  polymorphic over int/float **and units**. Implemented as `Ty::Num(var, unit)` + a `num` union-find,
  generalized/instantiated like type and unit vars. No annotations; no user-extensible type classes;
  no F# `inline`/SRTP (Python dispatches operators at runtime). `+ - *` stay numeric.
- **Unlocks:** real numeric programming; makes units genuinely useful (physics is floats — they get
  dimensional checking for free, e.g. `10.5<m> / 2.0<s> : float<m/s>`).
- **Remaining nearby work:** a guiding error for `+` on strings. Minor wart: a literal unified to
  `float` still emits as an int literal, so a *bare* such value prints `7` not `7.0` (arithmetic
  coerces, so values are unaffected).

### 4b. Comparison & equality operators — ✅ done (`DESIGN.md` §7.1)
`< > <= >= == !=`, the everyday gap after numbers. Comparison (`< > <= >=`) carries a closed
`comparison` constraint (int/float/string), built like `num` (an `ord` set on type vars, propagated
and generalized), so `let lt a b = a < b : comparison 'a => 'a -> 'a -> bool`; bools/functions are
rejected. Equality (`== !=`) is `'a -> 'a -> bool` (any type, unconstrained), with structural
`__eq__` generated on ADT classes (`Some 1 == Some 1`). `<` disambiguates from unit annotations by
adjacency (`5<m>` unit vs `5 < m` comparison — the F# rule). Covered across lexer/parser/typecheck/
compile/roundtrip tests.
- **Remaining:** chained comparisons are left-assoc (not Python-style chaining); `<=`/`>=` on ADTs
  would need a derived ordering (only `comparison`-constrained primitives for now).

### 4c. Logical operators — ✅ done
`and` / `or` / `not` — all keywords, lowering to the same Python keywords. Spelled the Python way
(not F#'s `&&`/`||`) to match the §7.1 Python-familiarity theme. New `ExprKind::Unary`/`UnOp` model
the prefix `not` (reusable for a future unary minus). Precedence mirrors Python — `or` < `and` <
`not` < comparison — so emitted code needs minimal parentheses (`not a == b` stays bare; `(not a) ==
b` gets parens). Covered across lexer/parser/typecheck/compile/roundtrip tests.

## Polish on existing features

### 5. Deep exhaustiveness — DONE
Match exhaustiveness now analyzes nested patterns fully via Maranget's usefulness algorithm
(`check_exhaustive`: matrix `useful` + `specialize`/`default_matrix` in `src/types/`), replacing the
old shallow head-constructor scan. `Some true | Some false | None` and `{ item = Some n } | { item =
None }` are recognized as complete without a `_`; a non-exhaustive `match` reports a concrete witness
(`` `None` ``, `` `Some false` ``, `` `{ x = _, y = true }` ``). Infinite types (`int`/`string`) and
unmatchable `Con`s still need a wildcard. Lowering keeps its defensive `case _: raise` guard.

### 6. User-defined computation expressions — ✅ done
A builder is any in-file `module` providing the protocol functions; `Builder { … }` (an uppercase
module name before `{`) desugars (`src/desugar.rs`) to calls on `bind`/`return_`/`returnFrom`/`yield_`/
`yieldFrom`/`combine`/`delay`/`zero`, after which ordinary HM inference and lowering handle it — the
type-directedness falls out of inferring the desugared calls, so no per-builder rules were needed. The
three built-ins keep their bespoke native lowering. Parser disambiguates `Maybe { let! … }` (a CE) from
`Some { x = 1 }` (a ctor applied to a record) by CE-keyword lookahead. Covered by typecheck/compile/
roundtrip tests; `hello.pyfun` shows a `Maybe` monad.
- **Effort/risk:** was Medium–high. **Status:** landed via desugaring (the elegant path — reuses
  inference + lowering wholesale).

### 7. Derived-measure aliases — ✅ done
`measure N = kg m / s^2` names a compound of base measures; aliases may build on earlier aliases
(`measure Pa = N / m^2`). `Item::Measure.definition: Option<UnitExpr>`; resolved at `build_decls` into
`Decls::measure_aliases` (expansion over base measures) via the shared `resolve_unit_against`, used by
both alias declaration and `<…>` annotations — so an alias expands and `<N>` unifies with `<kg m /
s^2>`. The type *displays* expanded (no abbreviation/conversion tracking — F#'s richer model stays out
of scope). The body reuses the unit grammar (`parse_unit_body`, factored out; now also accepts `1/s`,
which fixed a latent denominator-only roundtrip). Aliases must precede use. Covered by typecheck/
compile/roundtrip; `hello.pyfun` shows newton/pascal.
- **Effort/risk:** was Low–medium. **Status:** landed.

## Tooling & consolidation (make it usable, not just correct)

### 8. End-to-end `run` command — ✅ done
`pyfun run foo.pyfun` compiles (gated on type-checking) then executes the emitted Python by piping
it to `python`/`python3` via stdin, inheriting the program's stdout/stderr and propagating its exit
status. A REPL is the natural follow-on. Note: without a prelude (#9) there is still no `print`, so
a valid program runs silently — `run`'s observable value today is exit status and propagated runtime
errors (e.g. the non-exhaustive-match guard). Covered by `tests/run.rs`.
- **Effort/risk:** Low. **Status:** landed.

### 9. Standard library / prelude — ✅ MVP prelude + general FFI (`extern`) + lists landed
A set of built-in functions Pyfun programs can call. The MVP prelude has landed: `print : 'a ->
unit` and unit-polymorphic `abs`/`min`/`max : int<'u> -> …`, each a typed view over a Python builtin
(single source of truth `types::PRELUDE` + `seed_prelude`), plus a `unit` type. This made programs
observable and forced the first concrete slice of the **Python interop story** (`DESIGN.md` §6:
Pyfun name = Python name, partial application via known arities). Shipping it surfaced that
consecutive statements need separation, which prompted the **lightweight offside rule** (9b below).
Covered by `tests/{typecheck,compile,roundtrip}.rs`.
**The general FFI surface has landed (`extern`).** `extern [pure] name : type [= a.b.c]` imports an
arbitrary Python callable/value at a declared Pyfun type: type variables generalize (`show : a ->
string`), the optional dotted target is auto-imported (`= math.sqrt` emits `import math`), partial
application still lowers to `functools.partial`, and the boundary is effectful-by-default — a plain
`extern` carries `io` (the third source after `print`/`<-`), `extern pure` opts out. This made the
effect system's "Python boundary is effectful-by-default" rule (`DESIGN.md` §6) concrete. Covered by
`tests/{typecheck,compile,roundtrip}.rs`.
**Lists, sets, maps, and options have landed — as built-in modules.** `List a` / `Set a` / `Map k v`
lower to a Python `list` / `set` / `dict`; `Option a` (`Some`/`None`) is seeded like `Result`. Their
operations are **module-qualified** (`List.map`, `Set.add`, `Map.tryFind`, `Option.withDefault`) —
which is what lets `len`/`contains`/`map` reuse one name across collections without overloading. The
modules are **built-in namespaces only** (no `module` declarations / files / imports — deferred), and
needed **no parser change**: `Module.member` reuses the field-access node, disambiguated by casing
(`Upper.x` = module member, `lower.x` = field access) and resolved in the checker + lowering via
`types::qualified_name`. Single source of truth `MODULES` + `MODULE_PRELUDES`. `List.map`/`filter`/`fold`
and `Option.map` are **effect-polymorphic**; `Map.tryFind` returns `Option`; `Map.findOr` is a total
`dict.get`. Lists keep `[1,2,3]` literals; the hashed collections have no literals (`{…}` is taken) and
no constructors. Keys/elements must be hashable at runtime — primitives and ADT/record values both are
(generated structural `__hash__`). `cons`/`head`/
`tail` + list patterns deferred; the lazy counterpart is the `seq {}` CE. Covered by
`tests/{typecheck,compile,roundtrip,run}.rs` + `examples/hello.pyfun`.
  The `Option`, `Result`, and `Seq` modules have landed too: `Option.map`/`withDefault`/`isSome`/
  `isNone`; `Result.map`/`mapError`/`bind`/`withDefault`/`isOk`/`isError`/`toOption`; and the lazy
  `Seq.map`/`filter`/`take`/`fold`/`toList`/`ofList`/`range` (the map/bind ones effect-poly;
  `Result.toOption` bridges to `Option`; `Seq` routes to Python's lazy `map`/`filter`/`islice`/`range`).
  **In-file user modules have landed**: `module Name = <indented let bindings>` declares a namespace;
  members see siblings unqualified inside and are `Name.member` outside, lowering to mangled top-level
  names (`Geometry_area`). Reuses the same `Module.member` access mechanism as the built-ins (no parser
  change for access). MVP: `let`-only bodies, no nested modules.
- **Still to do (a larger prelude):** `Array` is **deferred** as redundant (`List` already *is* a
  Python dynamic array); and the full *file-based* module system (one module per file, `import`, a
  resolver + dependency graph, visibility, multi-file LSP) — a separate, larger initiative. (Generated
  `__hash__` on ADT/record classes and **in-file modules** are **done**.)
- **Effort/risk:** Medium. **Status:** MVP prelude + FFI + lists + sets/maps + options + results + lazy
  seq + built-in & in-file modules + ADT `__hash__` done; file-based modules open.

### 9b. Lightweight offside rule — ✅ done, then generalized by #3
Originally a top-level-only rule (a line break back to the first item's column emitted `Tok::Sep`).
**Superseded by the general offside rule in #3**, which adds a layout stack with `Indent`/`Dedent` for
nested blocks (indented `let` bodies) while keeping the same continuation behavior for multi-line
`match`/`if`/CE. Lives in the lexer.

### 10. LSP / editor support (`DESIGN.md` §9) — ✅ diagnostics + hover + go-to-def + find-refs + rename + completion + document symbols, over resilient (lex + parse recovery) / version-cached analysis
A language server (`pyfun lsp`, stdio JSON-RPC). DESIGN always scoped this as later,
"rust-analyzer-style front-end-first"; the span-carrying AST and diagnostics infrastructure were the
foundation.
- **Done:** **diagnostics** (existing type/effect/unit/exhaustiveness errors streamed as
  `publishDiagnostics` on open/change); **hover-for-type-and-effect** (the inferred type of the
  narrowest expression / binding name / **parameter / pattern variable** under the cursor, with
  `->{io}` shown on arrows — the display half of #1); **go-to-definition** (**module-level *and*
  local** — params, block `let`s, pattern vars — via a dependency-free AST name resolver
  `src/lsp/resolve.rs` that tracks lexical scopes, resolving each reference to a `Local(span)` or
  `Module(name)` target and never mis-jumping on shadowing — params, block `let`s, pattern vars, and
  computation-expression `let`/`let!` all resolvable); **find-references** (the inverse of go-to-def,
  reusing the resolver: `symbol_at` maps the cursor to a `Target`, `find_references` returns every
  matching occurrence plus the declaration when `includeDeclaration` is set — works from a use or from
  the definition/binder); **rename** (a `WorkspaceEdit` rewriting every occurrence + declaration, with
  `prepareRename` validation; restricted to locals and top-level `let` values, whose occurrences are
  all precise — ctors/types/externs are refused as unsound); and **completion** (in-scope module
  symbols + prelude + builtins + keywords, contributed from the recovered partial module); and
  **document symbols** (the outline — every module-level definition as a flat `DocumentSymbol[]` from
  `resolve::definitions`, each with a precise range + LSP `SymbolKind`). The JSON/JSON-RPC layer is
  **hand-rolled** (`src/lsp/json.rs`) to keep the crate dependency-free; the handler core is a pure
  function, unit-tested, plus a real-binary stdio integration test (`tests/lsp.rs`). To enable local
  navigation, params became `Param { name, span }`, pattern vars `Pattern::Var { name, span }`, and
  `CeItem::Let`/`LetBang` gained a `name_span` (spans are `NodeSpan`, invisible to roundtrip).
  **Resilient & incremental analysis:** *both* the lexer (`lex_recover` — skip a bad char /
  unterminated string and continue) and the parser (`parse_recover` — synchronize to the next item
  boundary at block depth 0) recover, so a bad character or single broken `let` no longer blanks the
  whole file: the parts that lex+parse still drive hover/navigation/completion/outline, only syntax
  errors are reported until the file is clean (then type errors take over), and rename stays
  conservative (requires a fully-parsing file). The compiler keeps the strict `lex`/`parse`. A
  per-document version-keyed analysis cache means repeated requests on an unchanged document share one
  parse + type-check. A thin VS Code client lives in `editors/vscode/`.
- **Still to do (lower-value tail):** *truly* incremental reparsing (a red-green-tree subsystem — the
  version cache already avoids redundant re-analysis between requests; partial reparse on edit is
  disproportionate at this file size); workspace symbols (project-wide, vs. today's per-document
  outline); richer hover (needs doc-comment *syntax* — a language feature — or a separate effect line).
- **Effort/risk:** the headline features all landed; the remaining tail is low-value at current scale.

## Suggested sequencing

All four language pillars beyond the MVP core are now done: **#1 (effects)**, **#2 (records)**,
**#3 (mutability + blocks)**, **#4 (floats)** — on top of `run` + prelude + the general offside rule.
The remaining work is breadth and polish, not new pillars. Highest leverage next:

The general FFI surface (`extern`) and the eager `List` collection (both #9), and the **#10 LSP**
(diagnostics + hover-for-type/effect + go-to-def/find-refs/rename/completion/document-symbols over
resilient, cached analysis + a VS Code client) are now done. Remaining, in rough priority:

1. **Prelude breadth (#9 cont.)** — lists/sets/maps/options/results/lazy-seq + built-in & in-file
   modules + ADT/record `__hash__` landed; remaining: the full *file-based* module system (multi-file,
   `import`, resolver, multi-file LSP). `Array` deferred as redundant with `List`.
2. **#5–#7 — all landed**: deep exhaustiveness (full Maranget usefulness with witnesses),
   user-defined CE builders (module-based, desugared), derived-measure aliases. Plus the #2/#3
   follow-ups: record patterns **landed**, blocks in `match`/`if`/lambda positions **landed**.
   Closure capture of a reassigned `mut` (`nonlocal`/`global`) **landed**. Remaining in this band:
   list patterns + `cons`/`head`/`tail` (awaiting a big-O-honest representation).
3. **#10 LSP tail (optional, low-value at this scale)** — workspace symbols, truly incremental
   reparse, doc-comment hover.
