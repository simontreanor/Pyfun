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
  fields, `__match_args__`, structural `__eq__`, `__repr__`); literals/updates emit positional
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
- **Still to do:** blocks in `match`-arm / `then` / `else` positions (only `let` bodies open blocks
  today); `nonlocal` for a closure that reassigns an outer `mut` (cross-function mutation).

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

### 5. Deep exhaustiveness
Current match exhaustiveness is *shallow* — it checks only the top-level constructor set, so
`match o with | Some (Some x) -> … | None -> …` is accepted even though `Some None` is unhandled
(the runtime guard catches it). Deep exhaustiveness analyzes nested patterns fully.
- **Effort/risk:** Medium; the classic usefulness-of-pattern-matrices algorithm. Pure checker work,
  no new syntax.

### 6. User-defined computation expressions
Today `async`/`seq`/`result` are the only builders, hard-coded. F#'s real power is letting *users*
define builders (a `Bind`/`Return` protocol) so anyone can build their own monadic DSL.
- **Effort/risk:** Medium–high; requires general builder resolution and type-directed desugaring
  instead of the current fixed lowering.

### 7. Derived-measure aliases
You can declare base measures (`measure m`) but not named derived ones (`measure N = kg m / s^2`).
This adds measure *expressions* in declarations so compound units can be named.
- **Effort/risk:** Low–medium; a measure-expression parser plus alias expansion in the unit resolver.

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
**Lists have landed.** `List a` is a built-in type lowering to a Python `list` (eager, dynamic-array
big-O: O(1) index/`len`, O(n) prepend/concat), with `[1, 2, 3]` literal syntax. The list prelude is
`map`/`filter`/`fold`/`len`/`sum`/`rev`/`range` (single source of truth `types::LIST_PRELUDE` +
`seed_list_prelude`): `len`/`sum` map onto Python builtins, the rest lower to emitted helpers
(`_pf_*`, on demand). `map`/`filter`/`fold` are **effect-polymorphic** (mapping an impure function is
`io`). The lazy counterpart is the existing `seq {}` CE. `List` is reserved like `Result`/`Seq`. Note:
`cons`/`head`/`tail` and list patterns in `match` are deferred (poor fit for a dynamic array). Covered
by `tests/{typecheck,compile,roundtrip}.rs`.
- **Still to do (a larger prelude):** `Array`/`Map`/`Set` (each its own type + functions + big-O),
  option/result helpers, and a value-level library over `seq {}`.
- **Effort/risk:** Medium. **Status:** MVP prelude + FFI + lists done; other collections/helpers open.

### 9b. Lightweight offside rule — ✅ done, then generalized by #3
Originally a top-level-only rule (a line break back to the first item's column emitted `Tok::Sep`).
**Superseded by the general offside rule in #3**, which adds a layout stack with `Indent`/`Dedent` for
nested blocks (indented `let` bodies) while keeping the same continuation behavior for multi-line
`match`/`if`/CE. Lives in the lexer.

### 10. LSP / editor support (`DESIGN.md` §9) — ✅ diagnostics + hover + go-to-def + find-refs + rename + completion + resilient/cached analysis done
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
  symbols + prelude + builtins + keywords, contributed from the recovered partial module). The
  JSON/JSON-RPC layer is **hand-rolled** (`src/lsp/json.rs`) to keep the crate dependency-free; the
  handler core is a pure function, unit-tested, plus a real-binary stdio integration test
  (`tests/lsp.rs`). To enable local navigation, params became `Param { name, span }`, pattern vars
  `Pattern::Var { name, span }`, and `CeItem::Let`/`LetBang` gained a `name_span` (spans are
  `NodeSpan`, invisible to roundtrip). **Resilient & incremental analysis:** the parser has an
  error-recovering entry point (`parse_recover`, synchronizing to the next item boundary at block
  depth 0) so a single broken `let` no longer blanks the whole file — the items that parse still drive
  hover/navigation/completion, only syntax errors are reported until the file is clean (then type
  errors take over), and rename stays conservative (requires a fully-parsing file). The compiler keeps
  the strict `parse`. A per-document version-keyed analysis cache means repeated requests on an
  unchanged document share one parse + type-check. A thin VS Code client lives in `editors/vscode/`.
- **Still to do (next slices):** *truly* incremental reparsing (the cache only avoids redundant
  re-analysis between requests on the same version, not partial reparse on edit); resilience to
  *lexing* errors (only parse errors recover today); document/workspace symbols (outline); richer
  hover (docs, a separate effect line).
- **Effort/risk:** the headline features landed; remaining slices are medium effort, high payoff.

## Suggested sequencing

All four language pillars beyond the MVP core are now done: **#1 (effects)**, **#2 (records)**,
**#3 (mutability + blocks)**, **#4 (floats)** — on top of `run` + prelude + the general offside rule.
The remaining work is breadth and polish, not new pillars. Highest leverage next:

The general FFI surface (`extern`) and the eager `List` collection (both #9), and the **#10 LSP**
(diagnostics + hover-for-type/effect + go-to-definition + completion + a VS Code client), are now done.
Remaining, in rough priority:

1. **#10 (LSP) cont.** — resilient parse-recovery + a version-keyed analysis cache landed; remaining
   slices: document/workspace symbols (outline), lex-error recovery, and truly incremental reparse.
2. **More collections / prelude (#9 cont.)** — `Array`/`Map`/`Set` (each its own type + big-O),
   option/result helpers, and a value-level library over the existing `seq {}` lazy type.
3. **#5–#7** — lower-stakes polish (deep exhaustiveness, user CE builders, derived measures), plus
   the #2/#3 follow-ups (record patterns; blocks in `match`/`if` arms; list patterns + `cons`/`head`/
   `tail` once a representation that honors their big-O is chosen).
