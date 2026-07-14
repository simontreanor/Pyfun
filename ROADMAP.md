# Pyfun — Roadmap

The MVP showcase set (curried functions + `|>`, ADTs + exhaustive matching, computation expressions,
units of measure) **and** Phase 2 file-based modules are complete — the language is feature-complete for
its intended scope, and nothing below blocks normal use.

This is the single forward-looking list of what's **not** built, so nothing is drip-fed. Design mechanics
and rationale live in [`DESIGN.md`](./DESIGN.md); what shipped and when is in git history; current status
is in [`GUIDE.md`](./GUIDE.md). Effort is rough: **S** ≈ a sitting, **M** ≈ a focused day, **L** ≈ multi-day.
Keep this a *forward-looking* backlog — do not let it grow back into a changelog of shipped work.

## Deferred (real features, no current demand — say the word and I'll scope it)

- **Fold-pass residual shapes** (S per slice, demand-driven) — Tier B shipped 2026-07-13 (local named
  folders incl. `dedupLegs`, chained updates, fresh-reset slots with the store-then-reset idiom,
  `Map.remove`/`Set.remove`, defensive-copy/alias `Var` inits — `DESIGN.md` §5.1), so the known rejecting
  shapes are covered. What still falls back, honestly: ordered *inserts* (network-rail's `insertByDep` —
  list slicing/splicing, not an append), folds inside in-file `module`s (P8 mangling), and anything the
  occurrence discipline can't prove. Pick one up only when a real hot fold rejects on it. (A
  persistent-map/HAMT `Map` would kill the O(n²) generally but still loses to a bare `dict` on this
  pattern.) The ceiling framing stands and caps all perf work: Pyfun targets un-JIT'd CPython, so the goal
  is "as fast as idiomatic hand-written Python," and a genuinely hot inner loop still belongs behind an
  `extern` — the further lowering tiers (general inlining, fusion, micro-opts) remain **non-goals**
  (below).
- **Larger prelude / package manager** — added on demand: prelude functions when a real program misses
  one; the package/façade story (publish typed extern façades once, `import` many) is a whole axis that
  waits for actual users. A future Python-side runtime package could default to `uv`. (Macros are a
  non-goal, below — not part of this bucket.) (Decode specialization shipped 2026-07-13 — `DESIGN.md`
  §5.3: statically-known decoders deforest to direct dict/list access, byte-identical `Result`s, 2.8x
  measured on a decode-dominated workload; dynamic shapes (`andThen`, decoder-as-value) keep the
  interpreter.)

## Verification gaps (things shipped but not exercised locally — missing program, not missing code)

Everything here has a tested core (compiler-side tests, protocol tests, or upstream-docs fidelity);
what's missing is a run on the real third-party surface, because the program isn't installed on the
dev machine. Close each by running the listed check once; delete its line when it passes.

- **Neovim** (no `nvim`): the `editors/nvim/` regex syntax + ftdetect/ftplugin have never been loaded,
  and the `vim.lsp.config` / `vim.lsp.start` snippets in `editors/README.md` are unexercised. Check:
  open a `.pyfun` file, confirm highlighting + hover. Likewise the **nvim-treesitter** parser
  registration (compiles `src/parser.c` + `scanner.c` via `:TSInstall pyfun`).
- **Helix** (no `hx`): the `languages.toml` entry, the `[[grammar]] source = { git, subpath }` fetch
  of `editors/tree-sitter-pyfun`, and the runtime-queries copy are untested. Check: `hx --grammar
  fetch && hx --grammar build`, open a file, `:log-open` for LSP.
- **Emacs** (not installed): the eglot and lsp-mode snippets are unexercised.
- **PyCharm/IntelliJ** (not installed): the LSP4IJ user-defined-server steps and the TextMate-bundle
  import of `editors/vscode/` are transcribed from LSP4IJ's docs, not clicked through.
- **Zed**: named as a Tree-sitter beneficiary but no Zed extension exists — that's a small authored
  artifact (extension.toml + grammar ref), not just a config; unscoped.
- **Tree-sitter rendering**: captures are validated with `tree-sitter query`, but no themed
  highlight render (`tree-sitter highlight` needs a configured theme) and no `test/corpus/` golden
  trees — the gate is the zero-ERROR parse sweep + compiler-validated `test/stress.pyfun`.
- **Jupyter kernel**: verified end-to-end via `jupyter_client` on Windows/CPython 3.14 — including,
  against the released v0.0.9 wheel in a clean venv: the `[jupyter]` extra, `python -m
  pyfun_kernel.install --sys-prefix`, a full cell session on the installed kernelspec, and the
  binary-discovery fix (PYFUN_BIN → same-env → PATH; the 0.0.8 wheel had PATH-first, which a stale
  global `pyfun` could break). Still open: (a) no real JupyterLab/Notebook UI session yet, (b) the
  engine-death replay path in `kernel.py` is code-reviewed only, (c) KeyboardInterrupt during a
  long cell untested, (d) macOS/Linux untested.

## Non-goals (decided against — with the reason, so they're not re-litigated)

- **Type annotations (`let x : T`, `(x: T)`, return types)** — annotation-free code is a selling point,
  not a gap: HM inference is complete so the compiler needs none, types are already surfaced by LSP hover /
  `pyfun check` / REPL `:type`, and `extern` is the one place Pyfun asks for types on purpose (the boundary
  contract). The one concrete unlock they once offered — lifting field-name uniqueness — shipped *without*
  them (use-site multimap), and the syntax fights a load-bearing decision: a depth-0 `:` is the
  `match`/`case` block opener. **Sole revisit trigger:** error *localization* under pure inference becomes a
  real, recurring pain — and even then the first answer is better HM diagnostics (provenance / expected-vs-
  found notes), with param annotations `(x: T)` alone (inside brackets `:` is free) as the fallback slice,
  not full `let` annotations. `DESIGN.md` §3, §8.3.
- **Visibility (`pub`)** — all-public is the Python-natural model; enforced privacy fights the ethos.
- **Tail-call optimization** — CPython has none; the stack-safe path is the `List`/`Seq` combinators.
- **`Array` type** — redundant: `List` already *is* a Python list (O(1) index/len).
- **User-extensible type classes / SRTP** — `num` and `comparison` are deliberately *closed* constraints;
  Python dispatches operators at runtime.
- **Row polymorphism** — a whole type-system axis (row variables, open records, presence constraints) for
  *structural* records Pyfun deliberately doesn't have — its records are nominal. Field-name ambiguity was
  solved instead with a lazy **use-site multimap** (a bare `p.x` errors only when two visible records
  genuinely share `x`, never at declaration/import). `DESIGN.md` §8.3.
- **Effect subsumption (pure ≤ io subtyping)** — the wrong tool for the gap it would close. Declared
  effects are exact (two closed sets unify only when equal), which only ever bites at *declared* arrows —
  ordinary code is inference-first, and inferred higher-order functions are already effect-polymorphic, so
  pure and impure arguments both flow everywhere annotations aren't written. Sound subsumption is
  *directional* (safe only at contravariant positions), so it means threading polarity through a
  symmetric HM unifier — an invasive, permanent complication — and a variance slip lets an effect past
  `let pure`, the flagship guarantee. Where a declared arrow genuinely must accept any effect, the
  HM-native fix is an effect *variable* in the extern signature — **implemented** (`->{e}`,
  extern-only, 2026-07-13), not subtyping. `DESIGN.md` §4.
- **Active-pattern nesting & export** — three cutoffs keeping the feature honest to its lowering (an AP is
  a *function call*, not a structural test): **(1) nesting an AP under structural patterns** — under
  constructors (`case Some (Positive p):`), tuple scrutinees (`case (Positive p, Positive q):`), or
  as-patterns — needs recognizer application at projection paths plus Maranget usefulness recursing into
  hidden case sets at depth; the workaround is a nested `match` on the bound value. **(2) Nested
  destructuring case arguments** (`case Small (x, y):`) — the same soundness-sensitive usefulness recursion
  into the case's monomorphic field types, for ergonomics-only payoff: a nested *literal* is
  `case Small s if s == 0:` (guards, shipped), and a tuple payload is bound whole and destructured in the
  body. **(3) Cross-module export** — the hidden case-set type and its mono field vars can't cross a module
  boundary soundly. Re-open only on a concrete driver; F#-parity alone doesn't qualify. `DESIGN.md` §7.2.1.
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
  nothing enforces consistency. The Pyfun way is centralized formatting functions (the shipped `Format`
  module, `DESIGN.md` §6). Plain `f"{expr}"` interpolation stays; only the `:spec`/`!r` mini-language is
  excluded.
- **Further lowering tiers: general inlining, stream fusion, micro-opts (old perf tiers 2–4)** — measured
  out on the flagship workload; each also pressures the *readable-output* promise. **(2) General
  folder/call inlining:** the landed fold pass already splices the folder into the loop for every
  qualifying fold, and the residual per-element call overhead is wall-clock-small — inlining the hottest
  wrapper (1.87M calls) saved ~3%, after the cProfile line claiming 87% proved to be the profiler's own
  per-call overhead (`DESIGN.md` §5.2). **(3) Stream fusion / deforestation:** rests on a false premise
  here — `Seq` pipelines are already lazy iterators, nothing intermediate materializes — so fusion only
  removes per-element indirection (the same small bucket: network-rail's entire interpreter residual is
  ~0.6s of ~14s), while costing one of the hardest passes there is (effect ordering across fused stages)
  and replacing a visible source pipeline with a fused loop the source doesn't show. **(4) Micro-opts**
  (hoisting method lookups out of loops): noise-level wins, pure erosion of line-to-line correspondence.
  Reopen (3) only on a profiled real workload where combinator indirection itself — not IO or costs shared
  with native Python — dominates and an `extern` is inappropriate. `DESIGN.md` §5.1–5.2.
- **`extern` stub generator** (`pyfun stub <module.pyi>` emitting draft extern files) — it would optimize
  the part of the design that is deliberately small. The interop model is a *thin, curated* boundary — wrap
  the handful of functions you call and sign each effect deliberately; the largest boundary any shipped
  example needs is 10 externs (`http_fetch`). Bulk generation invites wide, untightened, `io`-by-default
  surfaces nobody really signed, automating the step that was never the bottleneck while diluting the one
  that matters (the trusted contract, §4). The mechanical drafting it offered is better done by an LLM
  assistant from docs/stubs (same human-signs step after); a dependency-free `.pyi`-subset parser is an L
  to build and a permanent second frontend to maintain, for inputs that are often absent, inline-only, or
  `Any`-ridden. Reopen only if a façade/package ecosystem emerges with demonstrated churn hand-writing
  *large* boundary files. `DESIGN.md` §6.
- **Built-in date type / `Format.formatDate`** — doubly against the design. A native date type means
  reimplementing calendar logic Python's `datetime` already has (the boundary-vs-engine thesis says call
  it, don't rebuild it), and a general `formatDate` takes a strftime pattern — `"%Y-%m-%d"` is exactly the
  stringly-typed mini-language the f-string-specifier non-goal rejects and the `Format` module exists to
  replace; a *typed* date-format DSL is out of scope. Dates belong at the boundary: `extern type Datetime`
  + instance-method externs, where the programmer signs the contract — shipped as
  `examples/interop/datetime.pyfun` (a fully *pure* FFI pipeline).
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
