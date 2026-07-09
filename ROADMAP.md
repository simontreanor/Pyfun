# Pyfun — Roadmap

The MVP showcase set (curried functions + `|>`, ADTs + exhaustive matching, computation expressions,
units of measure) **and** Phase 2 file-based modules are complete — the language is feature-complete for
its intended scope, and nothing below blocks normal use.

This is the single forward-looking list of what's **not** built, so nothing is drip-fed. Design mechanics
and rationale live in [`DESIGN.md`](./DESIGN.md); what shipped and when is in git history; current status
is in [`GUIDE.md`](./GUIDE.md). Effort is rough: **S** ≈ a sitting, **M** ≈ a focused day, **L** ≈ multi-day.
Keep this a *forward-looking* backlog — do not let it grow back into a changelog of shipped work.

## Deferred (real features, no current demand — say the word and I'll scope it)

- **Type annotations** (L) — `let x : T = …`, params `(x: T)`, return types. Parked: HM inference is
  complete, so the compiler needs none, and types are already surfaced by LSP hover / `pyfun check` / REPL
  `:type`. The one concrete unlock it once offered — lifting field-name uniqueness — shipped *without* it
  (use-site multimap). Fights a load-bearing syntax decision: a depth-0 `:` is the `match`/`case` block
  opener, so `let x : T` needs a disambiguating rule. **Revisit on a concrete driver:** error localization
  becomes a real pain (better: improve HM *diagnostics* directly), or a deliberate F#-parity call. Cheap
  partial slice if wanted: param annotations `(x: T)` alone (inside brackets `:` is free). `DESIGN.md` §8.3.
- **Active-pattern fast-follows (residual)** (M) — **guards + lazy recognizer eval shipped** (the
  fall-through `if`-sequence lowering, `lower_ap_match_seq`). Remaining shape rules: an AP only as an arm's
  *whole* pattern (no nesting under ctors / or- / as-patterns); **binder-only case arguments** — no nested
  *destructuring* like `case Small (x, y):` (a nested *literal* `case Small 0:` is already expressible as
  `case Small s if s == 0:` via guards); structural non-AP arms restricted to literals/vars/`_`; and
  module-local (cross-module export is a **non-goal** — the hidden type and mono field vars can't cross a
  module boundary soundly). Nested destructuring case args need the usefulness algorithm to recurse into
  AP-case field types (soundness-sensitive), for low marginal value now that guards cover literals.
  `DESIGN.md` §7.2.1.
- **Effect subsumption** (M) — declared effects are exact (two closed effect sets unify only when
  equal), so a *pure* function does not satisfy a declared `->{io}`/`->{async}` parameter. Sound
  subsumption (pure ≤ io) is *directional* — safe only at contravariant argument positions — so it needs
  **polarity threaded through the unifier** (`unify` currently unifies `Ty::Fun` arg/result/effect
  symmetrically) plus a directional effect-coerce. Low demand today, and done carelessly it lets effects
  slip past `let pure`, so it wants its own careful pass. `DESIGN.md` §4.
- **Persistent-process REPL** (M) — today's REPL re-runs the accumulated definitions on each eval, so pure
  defs feel persistent but top-level effects / `let mut` don't carry across entries. A long-lived Python
  process would make state genuinely persistent.
- **`Format` module — dates follow-on** (S) — the numeric/string first cut shipped (`Format.fixed`/
  `thousands`/`percent`/`currency`/`grouped`/`padLeft`/`padRight`). `formatDate` is still open: it needs a
  date type or a Python `datetime` `extern`, so it was left out of the pure-stdlib first cut.
- **`extern` stub generator** (L) — a `pyfun stub <module.pyi>` tool that reads a Python type stub (PEP 484
  `.pyi`) and emits a *starting-point* `.pyfun` `extern` file: an `extern type` handle per class, an
  `extern` signature per function/method (instance methods as the `= .method()` receiver form, §6). Cuts the
  mechanical bulk of wrapping a library and complements the deferred façade/package-manager story below
  (generate bindings, refine, publish once, `import` many). **Explicitly a scaffold, not an oracle:** a
  `.pyi` cannot express what Pyfun most wants — effects (`io`/`async`), units, ADT/`Result` totality — and
  hints are frequently absent or `Any`, so every generated arrow would be `io`-by-default and every unmapped
  type an opaque `extern type` or an open `-> a` for the human to tighten (cf. widening the HTTP body to a
  named `Bytes` handle in `examples/interop/http_fetch.pyfun`). This keeps the trusted-contract model (§4):
  the tool proposes a signature, the programmer still signs it. Needs a small dependency-free `.pyi`-subset
  parser. Note it does **not** remove extern annotations — the boundary contract is the one place Pyfun asks
  for types on purpose; the generator only drafts them. `DESIGN.md` §6.
- **Keyword arguments on `extern` bindings** (S) — pin fixed Python kwargs at the boundary, e.g.
  `extern openText : string -> Seq string = builtins.open(mode="rt", encoding="utf-8")`, so a call lowers
  to `open(path, mode="rt", encoding="utf-8")`. Purely a **readability** win, not a capability: the same is
  already expressible positionally (`open`'s mode, `write_text`'s encoding *are* positional args), as
  `examples/interop/network-rail` shows — a bare trailing `"utf-8"` just doesn't self-document the way
  `encoding="utf-8"` would. Needs: parse `(kw=lit, …)` after the extern target (parser/AST), a
  `PyExpr::CallKw` variant (emitter), and kwarg injection at the two extern call sites in lowering (the
  general and receiver-method paths). Fits the trusted-contract boundary — the programmer still signs the
  signature. `DESIGN.md` §6.
- **Performance-directed lowering — defunctionalize hot folds/pipelines** (L) — today `Seq.fold f init xs`
  lowers to `functools.reduce(f, xs, init)` (a Python call per element) and a `Seq.map`/`filter`/`fold`
  chain to nested lazy `map`/`filter`/`islice` (a call per element per stage), so a hot loop pays call
  overhead and immutable-accumulator churn a hand-written Python `for` loop doesn't. The
  `examples/interop/network-rail` pair measures it: the pure-Pyfun `Seq.fold` over ~20M lines runs ~15x
  slower than the equivalent native Python loop — and that gap is entirely a *lowering-shape* artifact
  (per-element calls, a `match`-destructure + rebuild of the tuple accumulator each iteration, an O(n²)
  `List.concat` append), not anything fundamental. This is the **same defunctionalization instinct the
  compiler already applies to currying** (fully-applied calls collapse to direct `f(a, b)`, closures only
  for real partial application; `DESIGN.md` §5–6), extended to iteration.

  Tiered plan, readable wins first. **(1)** Recognize `Seq.fold`/`Seq.iter` with an *inlinable* folder
  (lambda literal or small local) and emit an imperative `for` loop with the body spliced in; when `init`
  is a tuple threaded *linearly* (a fold accumulator always is), split it into **mutable locals** the body
  updates in place (`d[k] = v`, `xs.append(e)`) instead of rebuilding — this alone should recover most of
  the network-rail gap, since the result is essentially the Python the helper hand-wrote. **(2)** Stream-
  fuse `map`/`filter`/`take`/`fold` pipelines into a single loop (deforestation) so no intermediate
  iterators are built. **(3)** Micro-opts (hoist method lookups out of loops, etc.) — gated, since they
  erode the line-to-line source correspondence Pyfun's *readable-output* promise depends on.

  The hard parts are the enabling analyses, not the emission: an **inlinability** check on the folder, a
  **linearity/aliasing** check to license in-place accumulator mutation soundly (a fold's acc qualifies; a
  captured/aliased value does not), and preserved effect ordering. Falls back to today's `reduce`/lazy-
  combinator lowering whenever those don't hold, so it is strictly additive. Lives in `src/lowering` (see
  the `_pf_fold` helper and the `Seq.*` cases).

  Framing / ceiling: unlike F# — whose elegant `Seq` pipelines lean on the .NET **JIT** to become fast IL
  at runtime — Pyfun targets un-JIT'd CPython, so *the compiler must be the AOT optimizer itself*. That
  caps the target at "as fast as idiomatic hand-written Python," never F#-on-.NET; for merely-warm loops
  that is plenty. It **complements, not replaces, the `extern` boundary**: better lowering raises the floor
  so the pure version is viable more often, while a genuinely hot inner loop still belongs behind an
  `extern` to an already-optimized library (numpy/polars/C) — you never out-codegen "call the fast thing
  that already exists." Scope narrowly to start (tier 1, `Seq.fold`/`iter`), measure against the
  network-rail example, and let that decide whether tier 2 earns its keep.
- **Larger prelude / package manager / macros** — added on demand. A future Python-side runtime package
  could default to `uv`.

## Non-goals (decided against — with the reason, so they're not re-litigated)

- **Visibility (`pub`)** — all-public is the Python-natural model; enforced privacy fights the ethos.
- **Tail-call optimization** — CPython has none; the stack-safe path is the `List`/`Seq` combinators.
- **`Array` type** — redundant: `List` already *is* a Python list (O(1) index/len).
- **User-extensible type classes / SRTP** — `num` and `comparison` are deliberately *closed* constraints;
  Python dispatches operators at runtime.
- **Row polymorphism** — a whole type-system axis (row variables, open records, presence constraints) for
  *structural* records Pyfun deliberately doesn't have — its records are nominal. Field-name ambiguity was
  solved instead with a lazy **use-site multimap** (a bare `p.x` errors only when two visible records
  genuinely share `x`, never at declaration/import). `DESIGN.md` §8.3.
- **Singly-linked `list` + `cons`/`head`/`tail` patterns** (F#'s `list`) — Pyfun's `List` *is* F#'s *array*
  (a Python `list`). A cons-cell type would lower to un-Pythonic linked nodes, and its recursive `x :: xs`
  idiom is stack-unsafe without TCO. Sequence patterns on the existing `List` (`case [x, *rest]`, done) are
  the Python-native, big-O-honest answer.
- **Imperative loops (`while` / `for … in`)** — iteration is the `List`/`Seq` combinators plus recursion;
  `let mut` is for local accumulation inside an expression, not to drive a loop.
- **Else-less `if`** — `if` is an *expression*, so both branches are required; a conditional side effect is
  `if c then eff else ()`.
- **Imperative `raise` / `finally` / exception hierarchy** — Pyfun signals failure with `Error`; the
  `try e : Result a Exception` expression catches at the FFI boundary and `result {}` + the `Result` module
  compose the rest. A `raise`/`finally` form would duplicate `Result` and import a class hierarchy Pyfun has
  no types for.
- **f-string format specifiers (`{x:.2f}`, `{v!r}`)** — an unchecked, stringly-typed sublanguage smuggled
  inside a string literal: the compiler can't see into it, so `.2f`→`.f2` misformats only at runtime and
  nothing enforces consistency. The Pyfun way is centralized formatting functions (the `Format` module
  above). Plain `f"{expr}"` interpolation stays; only the `:spec`/`!r` mini-language is excluded.
- **Unicode / symbol measure names (`<Ω>`, `<μ>`, superscript `m²`)** — measure names are ordinary
  identifiers, so this can't be scoped to units; it's language-wide Unicode identifiers (which would leak
  into Python names). Safe homoglyph handling (µ U+00B5 vs μ U+03BC) needs Unicode *normalization*, which
  isn't in std — violating the **dependency-free** constraint. Use ASCII names (`ohm`, `deg`, `celsius`).
  Explored + dropped 2026-07-04.
- **Higher unit-aware roots beyond `sqrt`/`cbrt`** — a general `root n x` needs dependent types (runtime
  `n`, the same wall as `x<'u> ** y`). √ and ∛ map to physical area/volume and are the principled cutoff;
  `**` stays dimensionless, and integer powers-with-units are covered by `*`.
- **Macros** — out of scope for the compiler.
- **Truly incremental LSP reparse** — whole-file lex + parse + check is milliseconds at realistic sizes,
  and the fingerprint-validated caches already remove redundant whole-file work; region reparse would
  complicate the offside lexer + recovering parser for no perceptible win.

---

*A 2026-07-02 table-stakes gap audit found 12 overlooked essentials (silent non-ASCII string double-encoding,
`%`, `List` completeness ops, scientific notation, numeric conversions, `Option.bind`, `**`, `String`
slice/`tryIndexOf`, mutual recursion, `as`-patterns, `let _ =` discard, literal ergonomics) — all cleared.
Everything across the MVP showcase, effects, records, mutability, numerics, the standard library, file-based
modules, and the LSP has shipped. See `DESIGN.md` for mechanics and git history for the timeline.*
