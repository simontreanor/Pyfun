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
- **Active-pattern fast-follows** (M) — the MVP shipped total / partial-Option / partial-bool /
  parameterized recognizers, but with shape rules: an AP only as an arm's *whole* pattern (no nesting under
  ctors / or- / as-patterns), binder-only case arguments (no nested literals like `case Rect 0 b:`), no
  guards or structural-pattern arms in an AP match, and module-local (no cross-module export). Also **eager,
  not lazy** recognizer evaluation (every distinct recognizer runs once up front). Lazy evaluation +
  sub-pattern case args + guards all need the same smarter fall-through lowering, so they'd land together.
  `DESIGN.md` §7.2.1.
- **Effect subsumption** (M) — declared effects are exact (two closed effect sets unify only when
  equal), so a *pure* function does not satisfy a declared `->{io}`/`->{async}` parameter. Sound
  subsumption (pure ≤ io) is *directional* — safe only at contravariant argument positions — so it needs
  **polarity threaded through the unifier** (`unify` currently unifies `Ty::Fun` arg/result/effect
  symmetrically) plus a directional effect-coerce. Low demand today, and done carelessly it lets effects
  slip past `let pure`, so it wants its own careful pass. `DESIGN.md` §4.
- **Separate effect line in LSP hover** (S) — hover shows `->{io}` inline on arrows; a dedicated effect
  line is the remaining display polish. (Doc-comment hover already shipped.)
- **Persistent-process REPL** (M) — today's REPL re-runs the accumulated definitions on each eval, so pure
  defs feel persistent but top-level effects / `let mut` don't carry across entries. A long-lived Python
  process would make state genuinely persistent.
- **List lexicographic ordering** (S) — user ADTs / records / tuples and built-in `Option`/`Result` order
  structurally; `List` could get a lexicographic `<` as a follow-on. `Set`/`Map` stay unordered (no natural
  order).
- **`Format` module — dates follow-on** (S) — the numeric/string first cut shipped (`Format.fixed`/
  `thousands`/`percent`/`currency`/`grouped`/`padLeft`/`padRight`). `formatDate` is still open: it needs a
  date type or a Python `datetime` `extern`, so it was left out of the pure-stdlib first cut.
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
