# Pyfun — Roadmap

The MVP showcase set (curried functions + `|>`, ADTs + exhaustive matching, computation expressions,
units of measure) **and** Phase 2 file-based modules are complete — the language is feature-complete for
its intended scope, and nothing below blocks normal use.

This is the single forward-looking list of what's **not** built, so nothing is drip-fed. Design mechanics
and rationale live in [`DESIGN.md`](./DESIGN.md); what shipped and when is in git history. Effort is
rough: **S** ≈ a sitting, **M** ≈ a focused day, **L** ≈ multi-day.
Keep this a *forward-looking* backlog — do not let it grow back into a changelog of shipped work.

## Deferred (real features, no current demand — say the word and I'll scope it)

- **Fold-pass residual shapes** (S per slice, demand-driven) — Tier B shipped 2026-07-13 (local named
  folders incl. `dedupLegs`, chained updates, fresh-reset slots with the store-then-reset idiom,
  `Map.remove`/`Set.remove`, defensive-copy/alias `Var` inits — `DESIGN.md` §5.1), so the known rejecting
  shapes are covered. What still falls back, honestly: ordered *inserts* (network-rail's `insertByDep` —
  list slicing/splicing, not an append), folds inside in-file `module`s (P8 mangling), and anything the
  occurrence discipline can't prove. Pick one up only when a real hot fold rejects on it. (A
  persistent-map/HAMT `Map` would kill the O(n²) generally but still loses to a bare `dict` on this
  pattern.) The ceiling framing stands and caps all *emitted-code* perf work: Pyfun targets un-JIT'd CPython, so
  the goal is "as fast as idiomatic hand-written Python," and a genuinely hot inner loop still belongs
  behind an `extern` — the further lowering tiers (general inlining, fusion, micro-opts) remain
  **non-goals** (below). What runs the output is a separate axis — see **Performance beyond CPython**.
- **Larger prelude / package manager** — added on demand: prelude functions when a real program misses
  one; the package/façade story (publish typed extern façades once, `import` many) is a whole axis that
  waits for actual users. A future Python-side runtime package could default to `uv`. (Macros are a
  non-goal, below — not part of this bucket.) (Decode specialization shipped 2026-07-13 — `DESIGN.md`
  §5.3: statically-known decoders deforest to direct dict/list access, byte-identical `Result`s, 2.8x
  measured on a decode-dominated workload; dynamic shapes (`andThen`, decoder-as-value) keep the
  interpreter.)

## Performance beyond CPython (scoped 2026-07-18)

The lowering work above closed the *emitted-code* axis: output within ~1.3× of hand-written Python,
further tiers measured out (non-goals below). This section is the other axis — changing what runs the
output. Ordered by effort; each entry carries its own gate. Draft write-up:
`local/article-draft-how-fast-could-it-get.md`. Measurement infrastructure: `bench/` (added
2026-07-18) — three compute-bound benchmarks (expr_eval / collatz / map_build), each paired with a
hand-written Python baseline as the ceiling reference, `bench/run.py` wall-clock runner
(median-of-N, output-equivalence-checked, `--python` selects the interpreter — the same harness
measures every option below). CPython 3.14.6 status quo: expr_eval 2.37×, collatz 1.18×,
map_build 1.64× vs hand-written.

- **Faster host runtimes** (S for the PyPy switch) — **GraalPy VERIFIED 2026-07-18** (3.12.8 /
  GraalVM CE 25.1.3, container; artifacts `local/graalpy-verification/`): emitted output runs
  *unchanged* — PEP 701 nested-quote f-strings, class-pattern `match`, dataclass ADTs/records, full
  bench suite byte-identical. Performance is **workload-dependent, not a blanket win**: collatz
  1.7× faster than CPython 3.14, map_build ~1.6× slower, expr_eval ~4× slower. Warmup probes show
  why: the hand-written *tuple*-based baseline JIT-warms to 2× faster than CPython, while every
  ADT-as-classes variant (match or isinstance, dataclass or `__slots__`) stays flat or degrades —
  GraalPy currently punishes allocation-heavy trees of small class instances, which is Pyfun's core
  data shape. Docs line: GraalPy runs Pyfun unchanged; try it for long-running arithmetic-heavy
  work; measure with `bench/run.py --python graalpy`, don't assume. CPython 3.14 is the best
  all-round host measured. **PyPy** still tops out at 3.11 (v7.3.22, 2026-04); the only 3.12
  feature the emitter relies on is PEP 701 (match/case is 3.10), so a `--target 3.11` switch that
  escapes nested quotes in f-strings unlocks it — untested and worth testing (different GC; the
  GraalPy ADT result does not transfer automatically). **CPython's own JIT** (experimental since
  3.13) accrues to every program for free.
- **Typed-emit + mypyc AOT (`--native`)** (M to measure, L to ship; **gated on the measurement**) —
  the checker knows every binding's inferred type, so the emitter could produce fully annotated
  Python whose annotations cannot lie, then compile it with mypyc into a C extension — native speed
  with the interop story intact (the result is still an ordinary extension module). Real blockers
  make this a feature, not a flag: mypyc does not yet compile `match` statements
  (python/mypy#12362) and every Pyfun pattern match lowers to one, so native mode needs an alternate
  `if`/`elif` match lowering; nested closures (partial application), generators (`seq`), and
  `_pyfun_rt.py` all need a compatibility audit; and mypyc needs a C toolchain on the user's
  machine, so this is opt-in only — `pip install pyfun` stays toolchain-free. **Gate MEASURED
  2026-07-18** (hand-made `--native` mock-up of `bench/expr_eval` — annotations + `if`/`isinstance`
  match lowering + monomorphized fold; mypyc 1.19 in a python:3.12 container, gcc; artifacts in
  `local/mypyc-experiment/`): vs the hand-written baseline, emitted **4.25×** → rewrite-only
  (interpreted) **2.16×** → mypyc-compiled **1.26×**. Net: **~3.4× faster than today's emitted
  output** on the ADT-heavy workload, landing near hand-written speed — the L is justified on these
  numbers. Two riders: (1) roughly half the gap closed *before* compilation — CPython's
  class-pattern `match` dispatch is expensive (Windows 3.14 ablation: 2.31× → 1.44× from the
  rewrite alone), so the `if`/`isinstance` lowering mypyc forces is also a candidate lever on its
  own, though it trades away the readable `match`/`case` output the default emitter promises —
  native-mode-only unless a real workload demands otherwise; (2) frozen-dataclass ADTs compiled
  fine — mypyc's remaining headroom (native classes vs dataclasses, boxed union fields) is upside
  not yet claimed.
- **Native backend** (not planned — recorded as a design-space note so the property it rests on
  stays deliberate) — the semantics are AOT-compilable: static HM types (no dynamic dispatch),
  default immutability (aggressive optimization is sound), tracked effects (pure code may be
  reordered), exhaustive ADTs (matches become jump tables), units already erase. That is OCaml's
  profile; nothing in the language *requires* a dynamic runtime, and that stays true by design. The
  cost center is the boundary: a native Pyfun embeds CPython and every `extern` crosses worlds,
  where cost = crossing *frequency* × data marshalling, not callee speed (bulk data can share
  zero-copy via the buffer protocol; chatty per-element crossings are fatal). Pyfun's edge if ever
  built: externs are typed and effect-tracked, so every crossing is statically known — the compiler
  could warn on chatty boundaries inside hot loops, or batch them. Two-tier precedent: Codon, Mojo —
  both multi-year funded-team efforts. Rewriting Python libraries in Pyfun to remove the boundary is
  rejected outright (the ecosystem is the asset). Reopen only with a funded reason.

## Verification gaps (things shipped but not exercised on the real surface)

Sweep completed 2026-07-14: Neovim 0.12 (5/5 headless checks: filetype/syntax/LSP attach/hover/
diagnostics), Helix 25.07 (health + the `[[grammar]] git+subpath` fetch AND build + highlights),
Emacs 30.2 (eglot attach + hover; note `eglot-ensure` needs interactive Emacs — batch tests must
call `eglot--connect` directly), Tree-sitter (40 corpus goldens + themed render audit), and the
Jupyter kernel — interrupt (CPU-bound cell aborts in ~50ms; a cell blocked in a C call does not
interrupt promptly on Windows, verified identical in the stock python3 kernel), engine-death
replay, and macOS/Linux/Windows via the `kernel.yml` CI matrix running `tests/kernel_e2e.py` on
every push (all green). JupyterLab UI session user-confirmed (if cells show empty `[ ]` with no
output, restart the Jupyter server before suspecting the kernel). Wheel/install/discovery chain
verified against the released v0.1.0 in a clean venv.

Zed user-confirmed 2026-07-14 (dev-extension install; needs `rustup target add wasm32-wasip2` —
documented in `editors/zed/README.md`). PyCharm user-confirmed 2026-07-14 (LSP4IJ + TextMate
bundle; hover is noticeably slower than in VS Code — LSP4IJ behavior, not the server). **No open
gaps.** Post-launch follow-ups that came out of the sweep: publish the Zed extension to the
registry (PR to zed-industries/extensions), and consider shipping Helix indent/textobject queries
(`hx --health` reports them missing; highlights ship today).

## Distribution (marketplace/registry presence — post-launch except where noted)

- **Open VSX** — DONE 2026-07-14: `pyfun.pyfun` 0.1.0 published and indexed (covers
  VSCodium/code-server/Gitpod/Theia). Future releases: `ovsx publish <vsix> -p <token>`.
- **JetBrains Marketplace** — plugin uploaded 2026-07-14 (`editors/jetbrains/`, thin: file type
  + TextMate grammar + LSP4IJ wiring, free mode + legacy CE, 2024.2+); **in moderation**
  (~1–3 business days — check plugins.jetbrains.com for approval or reviewer feedback).
  Later releases automate via `gradle publishPlugin`.
- **Zed extensions registry** — PR open
  ([zed-industries/extensions#6814](https://github.com/zed-industries/extensions/pull/6814)):
  the main repo as a submodule with `path = editors/zed` (no dedicated repo needed; the
  registry required a LICENSE file inside the extension dir — added). On merge, Pyfun is
  one-click in Zed's extension panel.
- **Upstream registry PRs** — status after the 2026-07-14 sweep:
  - **Helix**: PR open ([helix-editor/helix#16036](https://github.com/helix-editor/helix/pull/16036)) —
    languages.toml + git/rev/subpath grammar + Helix-scope queries; their query-check/docgen run
    clean locally; CI awaits first-contributor approval. May face an "established language" test.
  - **nvim-lspconfig**: PR #4476 CLOSED by maintainers — new languages need adoption evidence
    (~100 stars informally). **Resubmit post-launch with downloads/installs/stars in hand**; until
    then the manual `vim.lsp.config` snippet in `editors/README.md` is the documented path.
  - **Mason registry**: PR #16012 withdrawn by us (its acceptance path was lspconfig approval).
    Same resubmission trigger as lspconfig.
  - **nvim-treesitter**: upstream repo ARCHIVED 2026-04, no successor yet (candidates: the
    neovim-treesitter fork org, or parser management in Neovim core — neovim/neovim#39006). A
    fully validated branch is parked at `simontreanor/nvim-treesitter` (`add-pyfun`, parser entry
    + queries, their linter clean) ready to retarget when the ecosystem settles.
  - **MELPA** `pyfun-mode` recipe: PR open
    ([melpa/melpa#10094](https://github.com/melpa/melpa/pull/10094)); their process asked for an
    `Assisted-by:` header on the elisp (added). MELPA reviews code, not popularity — expect
    interactive review comments.
- **Sublime Text Package Control** (M, new audience) and a **Pygments lexer** on PyPI (S–M;
  improves JupyterLab/nbconvert/Sphinx rendering — kernel currently declares the `fsharp`
  lexer as an approximation) — both demand-gated.

## Docs & education site (live at simontreanor.github.io/Pyfun — what remains)

The mdBook site shipped 2026-07-15 (learner track, educator pack, internals tour, in-page runnable
code blocks; the playground moved to `/playground/` with `#code=` permalinks). Teaching prose is
CC BY 4.0. When lessons change, re-verify with `python docs/verify_lessons.py` (checks every deep
link decodes to its displayed starter and every solution's output matches). Still open:

- **Notebook-format lessons** (M, demand-gated) — the same lessons as `.ipynb` files riding the
  shipped Jupyter kernel, so instructors can distribute them through existing course
  infrastructure. Wait for an educator to ask.
- **CONTRIBUTING.md + curated good-first-issues** (S) — point new contributors at the internals
  tour's "Where you would add..." notes; label a handful of well-scoped issues.
- **Printable educator pack** (S, demand-gated) — a PDF export of the five session docs for
  departments that circulate paper.

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
