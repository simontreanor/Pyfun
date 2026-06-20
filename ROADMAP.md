# Pyfun — Roadmap

Possible next steps now that the MVP showcase set (curried functions + `|>`, ADTs +
exhaustive matching, computation expressions, units of measure) is complete. Each entry notes
what it is, what it unlocks, and rough effort/risk. See [`DESIGN.md`](./DESIGN.md) for the full
design and [`GUIDE.md`](./GUIDE.md) for current status.

## Language features (the remaining vision)

### 1. Effect inference (`DESIGN.md` §4)
The last first-class showcase pillar. Today the type system tracks *what* values are (`int<m>`,
`Option a`) but not *what a function does* — whether it prints, mutates, awaits, or can fail.
Effect inference makes purity part of the type: a function is pure unless it performs an effect,
and impurity propagates (a caller of an impure function is impure unless the effect is discharged).
F# notably does *not* have this; it's where Pyfun would out-design its inspiration.
- **Unlocks:** "this function is pure" guarantees; principled boundaries (the Python FFI becomes
  effectful-by-default); ties into `async` (an `Async` effect).
- **Effort/risk:** High and somewhat open-ended. Hard parts: designing the effect lattice (start
  coarse — pure vs `IO`), threading effects through inference (an effect-row system à la Koka),
  and inference-vs-annotation at function boundaries. Needs effectful *operations* to track, so it
  pairs naturally with adding `print`/IO or mutation — really "effects + something that has effects."

### 2. Records
Named-field product types: `type Point = { x: int, y: int }`, construction `{ x = 1, y = 2 }`,
field access `p.x`, and functional update `{ p with x = 3 }`. ADTs give *sum* types (tagged
variants); records give ergonomic *product* types with named instead of positional fields.
- **Unlocks:** Real data modeling — structs, config objects, returning multiple values. Lowers
  cleanly to Python (dataclasses or named tuples).
- **Effort/risk:** Medium. New syntax (`{...}` collides with CE blocks — needs disambiguation),
  AST/parser, type inference for field access (row polymorphism for generic `.x`, or nominal
  records to keep it simple), and lowering. Nominal records are the bounded MVP.

### 3. Mutability checking (`let mut`)
The parser already accepts `let mut x = …`, and DESIGN promises immutable-by-default with a checked
`mut` opt-in — but there's **no reassignment syntax** yet (`x <- 5`), so there's nothing to check.
This adds assignment plus the rule "reassigning a non-`mut` binding is a compile error."
- **Unlocks:** The "immutable by default, mutation explicit and tracked" guarantee central to the
  F# pitch.
- **Effort/risk:** Low–medium. Needs assignment syntax + statement sequencing (Pyfun has no
  statement blocks yet — you can't currently sequence a mutation then continue), the check, and
  lowering. The sequencing requirement makes it bigger than it first looks.

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

### 9b. Lightweight offside rule — ✅ done (fell out of #9)
A line break returning to ≤ the first item's indentation column (outside `()`/`{}`) separates
top-level items, so `print a` / `print b` are two statements, not one juxtaposition; deeper-indented
lines and breaks inside brackets are continuations. Lives in the lexer (`Tok::Sep`).
- **Still to do:** a **general** offside rule for *nested* blocks (local `let`-sequencing,
  indentation-delimited bodies) — the prerequisite #3 (mutability) calls out.

### 10. LSP / editor support (`DESIGN.md` §9, v2)
A language server: inline type errors, hover-for-type, go-to-definition. DESIGN always scoped this
as later, "rust-analyzer-style front-end-first." The span-carrying AST and diagnostics
infrastructure already exist as the foundation.
- **Effort/risk:** High (a whole subsystem), high payoff for real-world usability.

## Suggested sequencing

The project is now *usable*: with **#8 (`run`)**, the **#9 MVP prelude** (`print`/`abs`/`min`/`max`),
and the **#9b offside rule**, you can write and observe self-contained programs. Highest leverage
next:

1. **Broaden #9 (prelude + interop)** — collections and option/result helpers, plus name-aliased
   Python imports, to flesh out the standard library and the general FFI surface.
2. **#1 (effects)** — the most intellectually significant remaining feature, but large; bundles
   naturally with adding IO (and the prelude now gives it effectful operations to track).
3. **#2 (records)** and **#4 (floats)** — the biggest everyday-ergonomics wins.
4. A **general offside rule** for nested blocks (extends #9b) unlocks #3 (mutability sequencing).
4. **#5–#7** — satisfying, lower-stakes polish increments.
