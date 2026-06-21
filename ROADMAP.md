# Pyfun — Roadmap

Possible next steps now that the MVP showcase set (curried functions + `|>`, ADTs +
exhaustive matching, computation expressions, units of measure) is complete. Each entry notes
what it is, what it unlocks, and rough effort/risk. See [`DESIGN.md`](./DESIGN.md) for the full
design and [`GUIDE.md`](./GUIDE.md) for current status.

## Language features (the remaining vision)

### 1. Effect inference (`DESIGN.md` §4) — deferred, bundled with #3/FFI (decided 2026-06-21)
The last first-class showcase pillar. Today the type system tracks *what* values are (`int<m>`,
`Option a`) but not *what a function does* — whether it prints, mutates, awaits, or can fail.
Effect inference makes purity part of the type: a function is pure unless it performs an effect,
and impurity propagates (a caller of an impure function is impure unless the effect is discharged).
F# notably does *not* have this; it's where Pyfun would out-design its inspiration.
- **Unlocks:** "this function is pure" guarantees; principled boundaries (the Python FFI becomes
  effectful-by-default); ties into `async` (an `Async` effect).
- **Decided direction (low-pollution).** Follow the **inference-first** model (Koka/Flix/Unison),
  *not* effects-as-values (Haskell `IO`) or effects-as-keywords (Rust `async` coloring). Effects are
  **inferred and never written in ordinary code** — a pure function looks exactly as it does today
  (`let add a b = a + b`); `print : 'a ->{io} unit` propagates automatically. Effects surface only in
  (a) `pyfun check`/hover output (like mypy reporting a type — the Python-gradual-typing mindset),
  (b) error messages at a violation. Any *written* purity assertion, if added, is **definition-level
  and opt-in** (decorator-shaped, like `@property`), never expression-body syntax. Start coarse: one
  `io` label + effect *variables* for polymorphism (so `compose`/`map` stay pure-polymorphic). Fully
  erased at lowering.
- **Why deferred.** With only `print` to track and no enforcement site, effect inference today is
  pure infrastructure. Holding it until **mutation (#3, `<-`)** or **real Python FFI** exists means
  shipping it as one payoff-bearing unit where purity has teeth on day one (mutation is an `io`-style
  effect; the FFI boundary is effectful-by-default). Do "effects + something that has effects"
  together, not effects first.
- **Effort/risk:** High and somewhat open-ended. The invasive part is mechanical: every `Ty::Fun`
  arrow and every `infer_expr` rule gains an effect component, generalized/instantiated like the
  existing unit/num/ord variables.

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

### 9. Standard library / prelude — ✅ started (MVP prelude landed)
A set of built-in functions Pyfun programs can call. The MVP prelude has landed: `print : 'a ->
unit` and unit-polymorphic `abs`/`min`/`max : int<'u> -> …`, each a typed view over a Python builtin
(single source of truth `types::PRELUDE` + `seed_prelude`), plus a `unit` type. This made programs
observable and forced the first concrete slice of the **Python interop story** (`DESIGN.md` §6:
Pyfun name = Python name, partial application via known arities). Shipping it surfaced that
consecutive statements need separation, which prompted the **lightweight offside rule** (9b below).
Covered by `tests/{typecheck,compile,roundtrip}.rs`.
- **Still to do (a larger prelude):** collections, option/result helpers, and name-aliased imports
  (`show` → Python `str`) — the general "import and type an arbitrary Python function" surface.
- **Effort/risk:** Medium, partly a design exercise. **Status:** MVP slice done; broader prelude open.

### 9b. Lightweight offside rule — ✅ done, then generalized by #3
Originally a top-level-only rule (a line break back to the first item's column emitted `Tok::Sep`).
**Superseded by the general offside rule in #3**, which adds a layout stack with `Indent`/`Dedent` for
nested blocks (indented `let` bodies) while keeping the same continuation behavior for multi-line
`match`/`if`/CE. Lives in the lexer.

### 10. LSP / editor support (`DESIGN.md` §9, v2)
A language server: inline type errors, hover-for-type, go-to-definition. DESIGN always scoped this
as later, "rust-analyzer-style front-end-first." The span-carrying AST and diagnostics
infrastructure already exist as the foundation.
- **Effort/risk:** High (a whole subsystem), high payoff for real-world usability.

## Suggested sequencing

The project is now *usable* and expressive: `run` + prelude + the general offside rule, plus
**#2 (records)**, **#3 (mutability + blocks)**, and **#4 (floats)**. Highest leverage next:

1. **Broaden #9 (prelude + interop)** — collections and option/result helpers, plus name-aliased
   Python imports, to flesh out the standard library and the general FFI surface.
2. **#1 (effects)** — now unblocked: `<-` (from #3) is the first real `io` effect source, so effect
   inference can land with teeth (FFI from #9 would add more). Inference-first, zero-pollution
   (decided 2026-06-21; see #1 above).
3. **#5–#7** — lower-stakes polish (deep exhaustiveness, user CE builders, derived measures), plus
   the #2/#3 follow-ups (record patterns; blocks in `match`/`if` arms).
