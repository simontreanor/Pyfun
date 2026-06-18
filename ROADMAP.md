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

### 4. Float arithmetic / numeric type classes
Arithmetic is integer-only today (`+ - * /` are `int -> int -> int`), so `3.14 + 1.0` is a type
error even though floats exist as values. Doing this "properly" means an overloading mechanism —
either a small built-in numeric class (so `+` works on `int` and `float` but not `string`), or
duplicated operators. Units already ride on `int` and `float`, so float math inherits dimensional
checking for free.
- **Unlocks:** Real numeric programming; makes units far more useful (physics is floats).
- **Effort/risk:** Medium, and a genuine design fork — HM has no type classes, so either add a
  constrained-inference mechanism (bigger) or hard-code numeric overloading (simpler, less
  principled).

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

### 8. End-to-end `run` command
`pyfun compile` emits Python today; you run it yourself. `pyfun run foo.pyfun` would compile to a
temp module and execute it (a REPL is the natural follow-on). Small, but makes the language feel
real to use.
- **Effort/risk:** Low.

### 9. Standard library / prelude
A set of built-in functions Pyfun programs can call — `print`, list/option/result helpers, math.
A program can define functions today but has almost nothing to call (no `print`, no collections),
so you can't do anything observable without dropping to Python interop. Arguably the biggest gap
between "feature-complete compiler" and "usable language." Forces the **Python interop story**
(`DESIGN.md` §6) to get real — how Pyfun types map to/from Python library functions.
- **Effort/risk:** Medium, partly a design exercise (the interop surface; how Python functions are
  imported and typed).

### 10. LSP / editor support (`DESIGN.md` §9, v2)
A language server: inline type errors, hover-for-type, go-to-definition. DESIGN always scoped this
as later, "rust-analyzer-style front-end-first." The span-carrying AST and diagnostics
infrastructure already exist as the foundation.
- **Effort/risk:** High (a whole subsystem), high payoff for real-world usability.

## Suggested sequencing

The project is *correct* but not yet *usable* — you can't print or call a library. Highest leverage:

1. **#9 (prelude + interop)** or the quick **#8 (`run`)** — together make Pyfun something you can
   actually write programs in.
2. **#1 (effects)** — the most intellectually significant remaining feature, but large; bundles
   naturally with adding IO.
3. **#2 (records)** and **#4 (floats)** — the biggest everyday-ergonomics wins.
4. **#5–#7** — satisfying, lower-stakes polish increments.
