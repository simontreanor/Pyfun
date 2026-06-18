# Pyfun — Design

The language/semantics design spec. `GUIDE.md` is the lean operational pointer for working in
this repo; this file is the detailed reference. **Status: Phases 1–2 implemented** (lexer, parser,
AST, pretty-printer, plus lowering to a Python-AST IR and runnable Python emission over the
`let`/`if`/`match`/`fun` subset with curried application and `|>`; see §10). Phase 3+ (type/effect/
unit checking, ADTs, computation expressions, units) is not yet built.

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
- **Immutable-by-default** — `let` is immutable; reassignment is a compile error; `let mut` is the
  explicit opt-in.
- **Effect discipline** — first-class (see §4).

Example diagnostics the compiler must produce (rustc-style, with spans, codes, and `help` notes):
type mismatch (`add "hello" 5`), non-exhaustive `match` (missing `None` case), reassignment of an
immutable binding.

## 4. Effect system — first-class MVP goal

Unlike F# (which has no real effect system, only computation expressions), Pyfun treats **purity
and effects as part of the type system from the MVP.** This is a defining feature, not a
later add-on, and it shapes inference and lowering — so it must be designed in from the start.

Design intent (to be refined as the type checker takes shape):

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

Open design questions to resolve while building the checker: inference vs. mandatory annotation at
function boundaries; the exact effect lattice; how `mut` state interacts with the effect system;
and how much of the effect machinery is allowed to surface in generated Python.

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

MVP language features: immutable bindings by default, expression `if`/`match`, **curried
functions + partial application**, **pipe `|>`**, ADT declarations, effect-tracked functions, the
three computation expressions of §8, units of measure (§8), readable Python output.

Lexical conventions (decided in Phase 1): line comments start with `//` (F#-style); identifiers are
ASCII alpha + `_`; capitalized identifiers denote constructors in pattern position (§ patterns).

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
  lsp/             (v2) front-end-first, rust-analyzer style
prelude/           Pyfun/Python runtime support (Result/Option ADTs, etc.)
tests/             parser tests, compile tests, .pyfun fixtures (favor snapshot/golden tests)
```

**Build order:** `lexer` + `parser` + `ast` → `desugar` → `types` (incl. `units`) →
`lowering` + `python_emitter` → `diagnostics` + `cli` → `lsp`.

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
- **Phase 4+ — tooling:** formatter (`pyfun fmt`), then LSP / editor support; user-defined CE
  builders and unit polymorphism if not already in.

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
