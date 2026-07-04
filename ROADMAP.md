# Pyfun — Roadmap

Possible next steps now that the MVP showcase set (curried functions + `|>`, ADTs +
exhaustive matching, computation expressions, units of measure) is complete. Each entry notes
what it is, what it unlocks, and rough effort/risk. See [`DESIGN.md`](./DESIGN.md) for the full
design and [`GUIDE.md`](./GUIDE.md) for current status.

## Backlog — the full remaining picture

The single forward-looking list of **everything not yet built**, so nothing is drip-fed. Four
buckets: **overlooked essentials** (table-stakes gaps — now ✅ all cleared, kept as the audit record),
**non-goals** (decided against), **deferred** (real features, no current demand — build on request), and
**warts** (small polish — now ✅ all cleared). The narrative sections below record what *has* shipped.
As of 2026-07-03 the only genuinely-open work lives in **deferred**; essentials and warts are done.
Nothing here blocks normal use; the language is feature-complete for its MVP showcase + Phase 2
file-based modules. Effort is rough: **S** ≈ a sitting, **M** ≈ a focused day, **L** ≈ multi-day.

### Overlooked essentials (2026-07-02 audit — table-stakes) — ✅ all 12 cleared
Found by a gap-audit after the unary-minus miss (the same root cause: lexer + prelude basics *assumed*
rather than checked — almost none of this is in the type system). Each was verified with a failing
`pyfun check`; none was previously tracked. **All twelve are now done** (the last, raw strings, landed
2026-07-03); kept here as the audit record. Ordered by original priority.
1. ~~**Non-ASCII string literals are double-UTF-8-encoded**~~ — ✅ **fixed 2026-07-02**. Was a silent
   correctness bug: `lex_string`/`lex_fstring` did `b as char` on raw UTF-8 bytes, so `"café"` emitted
   mojibake. Now a shared `push_char` decodes the whole UTF-8 sequence (via `utf8_len`) in both the string
   and f-string-literal paths; covered by lexer + compile (string-level + encoding-independent e2e) tests.
2. ~~**Modulo `%`**~~ — ✅ **done 2026-07-02**. `Tok::Percent`/`BinOp::Mod` at the `*`/`/` precedence
   tier, → Python `%`. Numeric (`num`-constrained), works on int and float, and **unit-preserving like
   `+`/`-`** (`10<m> % 3<m> : int<m>`; mixed units rejected). `(%)` operator section works. Covered by
   roundtrip/typecheck/compile tests + hello.pyfun.
3. ~~**`List` is transform-only**~~ — ✅ **done 2026-07-02**. Added `get`/`isEmpty`/`contains`/`concat`/
   `sort`/`find` to `LIST_PRELUDE`, each with honest big-O: **`get : int -> List a -> Option a`** O(1)
   bounds-checked total (no raw `xs[i]`, no `IndexError`); **`isEmpty`** O(1); **`contains`** O(n) linear
   (`Set` is the O(1) alternative); **`concat`** O(n+m) fresh list; **`sort : comparison a => List a ->
   List a`** O(n log n); **`find : (a ->{e} bool) -> List a ->{e} Option a`** O(n), lazy/first-match,
   effect-poly. **No `xs[0]` surface syntax** (would risk `IndexError`; `get` is the total path) and **no
   cheap-looking prepend/`cons`** (O(n) on an array — the linked-list non-goal). NB immutable-style, so
   repeated `concat` to build a list is O(n²) — use `map`/`fold`/`Seq`.
4. ~~**Scientific-notation float literals**~~ — ✅ **done 2026-07-02**. `1e6`, `2.5e-3`, `1E3`, `1e+4`,
   `6.674e-11<m^3 / kg s^2>`. Lexer-only (`lex_number`): the exponent sign is consumed in the lexer (not
   left to unary minus), a number with an exponent is a float even without a `.`, and `e` is only consumed
   when a valid exponent follows (so `2exp`/`1e` stay int-then-identifier — back-compat).
5. ~~**Numeric conversions**~~ — ✅ **done 2026-07-02**. `round`/`floor`/`ceil`/`truncate : float<'u> ->
   int<'u>` (unqualified prelude, **unit-preserving** like `abs`/`min`/`max`; `round` is a bare builtin,
   the rest lower to `math.floor`/`ceil`/`trunc` + `import math`), and `String.toFloat : string -> Option
   float` (a total parse mirroring `toInt`, closing the fromFloat/toFloat asymmetry).
6. ~~**`Option.bind`**~~ — ✅ **done 2026-07-02**. Added `Option.bind : (a ->{e} Option b) -> Option a
   ->{e} Option b` (effect-poly like `Result.bind`), plus the cheap siblings `Option.filter : (a ->{e}
   bool) -> Option a ->{e} Option a` and `Option.toResult : e -> Option a -> Result a e` (the inverse of
   `Result.toOption`), completing the Option↔Result symmetry.
7. ~~**Exponentiation `**`**~~ — ✅ **done 2026-07-02**. `BinOp::Pow`, float-only + dimensionless
   (`float -> float -> float`, sidestepping the int**negative→float trap and units-through-a-runtime-
   exponent), right-associative and tighter than unary minus (`-2.0 ** 2.0 == -4`, `2.0 ** 3.0 ** 2.0 ==
   512`), lowering to Python `**`. Num literals coerce to float, and the `(**)` section works.
8. ~~**String slice / substring / indexOf**~~ — ✅ **done 2026-07-02**. `String.slice : int -> int ->
   string -> string` (Python `s[start:end]` — total, end-exclusive, clamps out-of-range; via a new
   `PyExpr::Slice` node so it emits readable `s[start:end]`) and `String.tryIndexOf : string -> string ->
   Option int` (via `str.find`, `None` when absent — total, no `IndexError`, like `List.get`).
9. **Mutual recursion** — ✅ **done 2026-07-02**. Mutually-recursive top-level functions type-check
   together, in any order (`isEven`/`isOdd`) — **implicit, no `and` keyword** (which would clash with the
   boolean `and`). `run` finds cycles among top-level `let`s via scope-accurate free-variable analysis
   (`collect_free`) + SCC (`strongly_connected`); each all-function cycle is inferred as a group
   (`infer_mutual_group`: pre-bind mono, infer all bodies, tie knots, generalize each against the outer
   env — so the group is monomorphic within itself but polymorphic to the rest, preserving `id`-style
   let-polymorphism). Value cycles stay rejected. Lowers unchanged (Python defs resolve names at call
   time). **Limitation:** one-way forward references between *independent* (non-cyclic) top-level bindings
   still need declare-before-use — only genuine cycles are grouped.
10. ~~**`as`-patterns**~~ — ✅ **done 2026-07-02**. `case p as x:` binds the whole matched value to `x`
    alongside destructuring (`Pattern::As`, `as` a keyword binding looser than `|`). **Transparent for
    exhaustiveness** (peeled in the usefulness algorithm — `Circle r as w` covers exactly Circle, `_ as x`
    is a catch-all); binds the name + the inner pattern's vars; lowers 1:1 to Python `case p as x`.
11. ~~**`let _ = e` discard**~~ — ✅ **done 2026-07-02**. `let _ = e` discards any-typed `e` (lets a
    non-unit result be dropped mid-block despite the "non-final statement is `unit`" rule), lowering to
    Python's idiomatic `_ = e`. A discard takes no parameters and can't be `mut`. Parser-only change.
12. **Literal ergonomics** — ✅ **done 2026-07-02**. Numeric: `1_000_000` digit separators and
    `0xFF`/`0o17`/`0b101` alternate bases (incl. hex with separators `0xDEAD_BEEF`; `_` only between
    digits, values normalize to decimal). String escapes: added `\r` and Rust-style **`\u{HEX}`** (1–6
    hex digits; decodes at lex time, and the emitter now re-escapes `\r`); factored a shared `lex_escape`
    used by both string and f-string lexing. **Raw strings** (`r"C:\path"`) **landed 2026-07-03** — a
    string-prefix lexer mode like `f"` (`lex_raw_string`), lexer-only (yields a plain `Tok::Str`).

### Non-goals (won't build unless a concrete need appears, with the reason)
- **Visibility (`pub`)** — Pyfun is all-public, the Python-natural model; enforced privacy fights the ethos.
- **Tail-call optimization** — CPython has none; the stack-safe path is the `List`/`Seq` combinators
  (deep self-recursion matching hand-written Python's `RecursionError` is acceptable).
- **`Array` type** — redundant: `List` already *is* a Python list (O(1) index/len).
- **User-extensible type classes / SRTP** — `num` and `comparison` are deliberately *closed* constraints;
  Python dispatches operators at runtime.
- **Row polymorphism** — out of scope, and **no longer needed** for the problems it was held in reserve
  for. It's the textbook way to type `fun p -> p.x : { x: 'a | 'r } -> 'a`, but it's a whole new
  type-system axis (row variables, open records, row unification, presence/absence constraints, noisier
  errors) for *structural* records Pyfun deliberately doesn't have — its records are nominal (Python
  `dataclass`-style). Field-name ambiguity (incl. **cross-module records**, done 2026-07-03) was instead
  solved with a **lazy, use-site multimap**: field names are non-unique, and a bare `p.x` errors only when
  two visible records genuinely share `x` (never at declaration/import). Full rationale: `DESIGN.md` §8.3.
- **A singly-linked `list` + `cons`/`head`/`tail` patterns** (F#'s `list`) — Pyfun's `List` *is* F#'s
  *array* (a Python `list`: O(1) index/len). A cons-cell type would lower to un-Pythonic linked-node
  classes (fighting the readable-Python ethos), and its signature idiom — recursive `x :: xs`
  decomposition — is stack-unsafe without TCO (a non-goal, since CPython has none). Same reasoning as
  the `Array` and TCO non-goals. Python has no built-in singly-linked list anyway (`deque` is
  doubly-ended, a different structure). What people actually want here is **sequence patterns on the
  existing `List`** (see Deferred) — Python-native and big-O-honest — not a new linked type.
- **Macros** and a **package manager** — out of scope for the compiler (a future Python runtime package
  could default to `uv`).
- **Imperative loops (`while` / `for … in`)** — FP-first: iteration is the `List`/`Seq` combinators
  (`map`/`filter`/`fold`) plus recursion. `let mut` exists for local accumulation inside an expression,
  not to drive a loop. (Decided 2026-07-02 during the gap audit.)
- **Else-less `if`** — `if` is an *expression*, so both branches are required; a conditional side effect
  is `if c then eff else ()` (the `else` branch is `unit`). No statement-form `if` without `else`.
- **Imperative `raise`/`finally`/exception hierarchy** — Pyfun signals failure with `Error`, not by
  raising; the `try e : Result a Exception` expression (done) catches at the FFI boundary, and
  `result {}` + the `Result` module compose the rest. A `raise`/`finally` statement form would duplicate
  `Result` and import a class hierarchy Pyfun has no types for.
- **f-string format specifiers (`{x:.2f}`, `{v!r}`)** — decided 2026-07-03. A format spec is an
  **unchecked, stringly-typed sublanguage smuggled inside a string literal**: the compiler can't see into
  it, so `.2f`→`.3f` is a *silent* behaviour change, `.2f`→`.f2` misformats or blows up only at runtime, and
  nothing enforces consistency (currency formatted `.2f` here, `.3f` there). That's exactly the class of
  surprise Pyfun refuses everywhere else — it rejects float patterns (unreliable `==`), unchecked field
  access, and unit mismatches *because* it won't bless stringly-typed footguns; a format mini-language would
  be a glaring exception to "the compiler is the gatekeeper." The FP answer, and the Pyfun way, is
  **centralized formatting functions** — `formatCurrency : float<gbp> -> string`, `formatPercent`,
  `formatDate` — defined once, checked at every call site, changed in one place: consistency + compile-
  checking + single-source-of-truth, and just functions. A future small `Format` module could ship these
  over the existing `String` ops. (The plain-hole `f"{expr}"` interpolation stays; only the `:spec`/`!r`
  mini-language is excluded. Multi-line `f"""` was a *separate*, uncontroversial feature — done 2026-07-03.)
- **Unicode / symbol units of measure (`<Ω>`, `<μ>`, `<°>`, superscript `m²`)** — explored + dropped
  2026-07-04. Measure names are ordinary identifiers (same lexer path as any name), so this can't be
  scoped to units — it's language-wide Unicode identifiers, which would also leak into Python names and
  void the "measures erase, so no interop cost" argument. Decisively: detecting Unicode letters is free
  (std `char::is_alphabetic`), but **normalization is not in std**, and NFC/NFKC is the *only* thing that
  closes the homoglyph footgun (µ micro U+00B5 vs μ mu U+03BC, Ω ohm U+2126 vs Ω omega U+03A9 render
  identically) — so it can't be made safe within the **dependency-free** constraint (a non-negotiable).
  The safe alternative — a **display alias** (`measure ohm = "Ω"`, ASCII source, symbol rendered only in
  hover/errors) — is doable and zero-dep-clean but mostly-cosmetic machinery, so it too is parked. And
  **runtime unit *printing* is a separate wall**: units erase to plain numbers, so the value carries no
  unit; `print f"{speed} m/s"` (explicit) is the answer, not compiler-injected units. The `°`/temperature
  case has an extra semantic block regardless: angles are dimensionless and `°C`/`°F` are affine, unsound
  in a multiplicative unit system. Use ASCII measure names (`ohm`, `deg`, `celsius`) with eyes open.

### Deferred (real, no current demand — say the word and I'll scope it)
*Language*
- **Active patterns** — ✅ **done 2026-07-04** (`DESIGN.md` §7.2.1). F#'s named recognizers, all four
  flavors: **total** `let (|Even|Odd|) n = …` (a hidden ADT + function; the case set is **closed for
  exhaustiveness** — `case Even:`/`case Odd:` needs no `_`, a missing case gets a witness), **partial
  Option** `let (|Positive|_|) n = … Some n … None` (the case binds the payload), **partial bool**
  `let (|Blank|_|) s = …` (a predicate — binds nothing, no `Some ()` ceremony; flavor inferred from the
  body type), and **parameterized partial** `let (|DivisibleBy|_|) d n = …` (`case DivisibleBy 3:`).
  Uses are ordinary ctor patterns (no new pattern syntax); the recognizer's effect is *performed* by a
  match that uses it (a `let pure` + impure AP is rejected); lowering is an honest **if/elif chain**
  with each distinct recognizer application hoisted once per match (side effects once), `isinstance`
  tests + field binds, truthiness for bool cases. **Deferred fast-follows (MVP shape rules, all clean
  checker errors):** an AP only as an arm's *whole* pattern (no nesting under ctors / or- / as-patterns),
  binder-only case arguments (no nested literal sub-patterns like `case Rect 0 b:`), no guards and no
  structural-pattern arms in an AP match, and no cross-module export of an AP (module-local). Also
  **eager, not lazy, recognizer evaluation**: every distinct recognizer in the match runs up front (once
  each), so all fire regardless of which arm wins — invisible for pure APs (just redundant work), a minor
  effect-timing quirk for impure ones (F# is lazy/short-circuit here). Making it lazy needs the same
  smarter fall-through lowering as guards + sub-pattern case args, so those three would land together.
- **Typed holes** — ✅ **done 2026-07-04** (`DESIGN.md` §9). `?` / `?name` in expression position — the
  type-driven-development tool from Haskell/Idris (past the F# model, which has none). Inferred as a
  **fresh type variable that unifies freely** (so it never causes a spurious error and takes the context's
  type — `?body + 1` ⇒ `int`, `List.map ? xs` ⇒ a function), collected and resolved into
  `types::Hole { name, ty, span }`. Reported **informatively, not as a red error**: `pyfun check` prints a
  `note` and exits non-zero; the LSP publishes at **Information severity** with hover showing the type
  (free — the hole's type is already in the span→type table). It **blocks `compile`/`run`** (no value to
  lower). `?` chosen over Haskell's `_` (already the wildcard + `let _ =` discard). **Valid hole fits**
  (added 2026-07-04): each note also lists in-scope binding names whose type could fill the hole — found by
  a *trial unification* of every visible scheme against the hole's type, rolled back so the checker's
  substitution is untouched (`Infer::hole_fits` + `subst_snapshot`/`restore`). Ranked most-specific (fewest
  generalized vars) first, unqualified names (the user's own bindings) before qualified module members,
  capped at 6; a fully-unconstrained hole lists none (everything would fit). Covered by
  `tests/{roundtrip,typecheck}.rs` + in-crate LSP tests. Fully done — no residual.
- **Sequence patterns on `List`** — ✅ **done 2026-07-03**. `case []`, `case [x]`, `case [x, y]`,
  `case [x, *rest]`, `case [*rest]` in `match` over the existing `List` (a Python array). Python-native and
  big-O-honest (`*rest` is a visible slice-copy). `Pattern::List { prefix, rest }` (rest = the trailing
  star's var/`_`, or `None`); **first cut: the star must be last** (non-last `[*init, last]` is a parse
  error — a follow-on). Exhaustiveness models `List` as `Nil | Cons a (List a)` **inside the usefulness
  algorithm only** (no ADT, no lowering change): `Tag::Nil`/`Tag::Cons`, `ctor_signature(List) = [Nil,
  Cons]`, and a lone star `[*r] ≡ r` delegates in `pattern_tag`/`row_head`/`default_matrix` — so
  `[] | [x, *rest]` is exhaustive without a wildcard and `case []:` alone reports witness `[_, *_]`. Lowers
  to a Python **list** sequence pattern (`PyPattern::ListSeq` → `case [x, *rest]:`, brackets — distinct from
  a tuple's paren `Sequence`). Nested element patterns (`case [Some x, *rest]:`) type-check + are
  deep-exhaustive. Covered by `tests/{roundtrip,typecheck,compile}.rs`. **Deferred follow-on:** a non-last
  star (`[*init, last]`, `[a, *mid, z]`). The linked-list `cons`/`head`/`tail` half is a non-goal (above).
- **Lift the unique-field-name restriction** — ✅ **done 2026-07-03** (with cross-module records). This
  was estimated **L** on the assumption it needed type annotations, type-directed field resolution, or row
  polymorphism (a non-goal) — but a **lazy, use-site multimap** solved it with none of those: `field_owner`
  is now `HashMap<String, Vec<String>>`, and a bare `p.x` resolves iff **exactly one** visible record
  declares `x` — **2+** is an ambiguity error *at that access site* (never at declaration/import), **0** is
  "unknown field". Two records (in- or cross-module) may freely share `x`/`name`/`id`; tagged
  construction/patterns (`Point { x = 1 }`) are never ambiguous since they name the type. See the
  cross-module-records entry below and `DESIGN.md` §8.3.
- **Derived ordering for ADTs** — ✅ **done 2026-07-03**. `< > <= >=` (and `List.sort`) now work on
  **user sum types, records, and tuples**, compared structurally: a sum orders by variant *declaration
  order* then field-by-field (`Red < Green < Blue`, any `Circle` < any `Rect`), a record field-by-field, a
  tuple lexicographically; nested/recursive types compose. Type side: `require_ord` recurses into a `Con`'s
  ctor/record field types (params substituted by the actual args) with a visiting-set recursion guard
  (keyed on `(name, args)`; `MAX_ORD_DEPTH` bounds non-regular recursion); the deferred-var `ord` mechanism
  flows a late-resolved `comparison 'a` through it. Codegen: each user variant/record class emits
  `_pf_order_key = (variant_index, fields…)` + `__lt__`/`__le__`/`__gt__`/`__ge__` (tuples need none — Python
  tuples already order). **Built-in `Option`/`Result` also order** (done 2026-07-03 as the follow-on):
  `None < Some`, `Ok < Error`, orderable when their payloads are (their prelude classes now emit the same
  ordering methods; nested `Some (Ok x)` composes). Still not ordered: `Set`/`Map`/`Exception` (no natural
  order) and `List` (a possible lexicographic follow-on). Covered by typecheck/compile tests +
  `examples/hello.pyfun`.
- **Unit-aware roots `sqrt`/`cbrt`** — ✅ **`sqrt` done 2026-07-03, `cbrt` 2026-07-04**. √area = length
  and ∛volume = length:
  `sqrt 16.0<m^2> : float<m>`, `sqrt x<m^4/s^2> : float<m^2/s>`, `cbrt 27.0<m^3> : float<m>`, and a non-square unit (`<m>`,
  `<m^3>`) is a compile-time dimensional error. Needed **neither** rational unit exponents **nor** a
  bespoke halving scheme: the scheme is seeded in the prelude with the existing integer-exponent
  representation (arg unit `'u^2` via `Unit::pow(2)`), and the existing abelian-group unifier's
  elimination step already halves even exponents / fails odd ones — the "perfect square" constraint
  falls out of unification, and it propagates (`let norm x = sqrt (x * x) : float<'u> -> float<'u>`).
  Pure; lowers to `math.sqrt` (+ `import math`, the `floor`/`ceil`/`truncate` routing); units erase.
  Bonus fix: `solve_unit`'s reduce step could previously recurse forever on `'u^2 ~ m^3` (a latent
  stack overflow reachable via `let sq x = x * x`); it now detects the no-progress case and reports a
  mismatch. `extern sqrt` now hits the ordinary "already defined" clash (drop the old dimensionless
  workaround); a user `let sqrt` still shadows. **`cbrt` (2026-07-04)** is the exact sibling with the
  exponent bumped to 3 (`Unit::pow(3)`): thirds a perfect-cube unit (`m^3 → m`, `m^6/s^3 → m^2/s`),
  rejects a non-cube (`m`, `m^2`, `m^4`), lowers to `math.cbrt`. It's justified *purely* by units —
  dimensionless `cbrt` is just `x ** (1.0/3.0)`, but `**` is dimensionless so only a unit-aware cube
  root keeps the dimension; `math.cbrt` also handles negatives correctly, which `** (1/3)` doesn't.
  **Where the family stops — `{sqrt, cbrt}` and no more:** each fixed-`n` root is its own monomorphic
  builtin (`float<'u^n> -> float<'u>`); a general `root n x` is **impossible** (runtime `n` → dependent
  types, the same wall as `x<'u> ** y`), so higher roots can't be unified into one function. Two is the
  principled cutoff — √ and ∛ map to physical *spatial dimensions* (2D area, 3D volume), the quantities
  people actually root; a 4th root of `m^4` isn't a measured thing (use a dimensionless `extern` if ever
  needed). `**` stays dimensionless, and integer powers-with-units are covered by `*`
  (`x<m> * x<m> : <m^2>`). Decided 2026-07-02 (sqrt), extended 2026-07-04 (cbrt).
- **Chained comparisons** — ✅ **done**: `a < b < c` is Python-style (means `a < b and b < c`, `b`
  evaluated once), a dedicated `ExprKind::Compare` node lowering 1:1 to Python's native chained
  comparison. A lone comparison stays `Binary`; links may mix `== != < > <= >=`.
- **Operator sections / operators as functions** — ✅ **done**: `(op)` (e.g. `(*)`, `(+)`, `(<)`) is a
  binary operator as a first-class curried function; `(*) 2` partially applies it. `ExprKind::OpFunc(BinOp)`
  desugars to the lambda `fun a b -> a op b` (`desugar::op_func`) at inference and lowering, so the
  operator's own constraints, currying, and partial application all fall out; the pretty-printer keeps the
  `(op)` spelling. `and`/`or` are excluded (keywords whose short-circuiting a strict function would drop).
- **More effect labels + effect annotations on declared arrows** — ✅ **done 2026-07-03**. The `Effect`
  is now a set of concrete labels (`EffLabel::Io`/`EffLabel::Async`, `labels: BTreeSet<EffLabel>`) plus
  effect vars, so it generalizes/instantiates/unifies over a *set* and displays sorted (`->{io, async}`).
  Declared function arrows in `type` decls and `extern` signatures take `->{label, …}` annotations
  (`extern fetch : string ->{async} string`), threaded into the seeded scheme (a bare `->` stays pure);
  `let pure` rejects any concrete label. Full backward compatibility — `io`-only code is unchanged, effects
  still erase at lowering. **Still deferred there:** wiring the `async {}` CE to actually *produce* the
  `async` label (it's representable/annotatable/inferrable now, not yet CE-linked), and effect subsumption
  (declared effects are exact, not lower bounds). Covered by typecheck (multi-label inference, `let pure`
  per label, declared-arrow flow, `io, async` display) + roundtrip (`->{...}` syntax) tests.
- **f-string extras** — ✅ **all landed**: the core `f"...{expr}..."` interpolation (targets Python
  3.12+), **`{x=}`** self-documenting holes, and (2026-07-03) **multi-line triple-quoted strings** —
  plain `"""..."""`, interpolated `f"""..."""`, and raw `r"""..."""`. Lexer-only (a `triple` flag on
  `lex_string`/`lex_raw_string`/`lex_fstring`; exactly three quotes at the open, so `""` stays the empty
  string): newlines and lone `"` are literal content, escapes/holes/debug-holes work exactly as in the
  single-line forms, and the whole literal is consumed in one pass so no layout token can leak from
  inside it. Emits the escaped single-line form (`"a\nb"` — value-identical; the emitter is line-based,
  so a real Python triple-quoted literal would need unindented continuation lines). `rf"""…"""` out of
  scope. **Format specifiers** (`{x:.2f}`, `{v!r}`) are a **non-goal** — see
  the Non-goals section for the reasoning (an unchecked stringly-typed sublanguage; centralized formatting
  functions are the Pyfun way).
- **Type annotations** (L) — **parked (deprioritized 2026-07-02).** `let x : T = …`, params `(x: T)`,
  return types. *Not necessary*: HM inference is complete, so the compiler needs none; the identity is
  "F#-level safety without ceremony," and types are already surfaced by LSP hover / `pyfun check` / REPL
  `:type`. **The strongest concrete driver has evaporated:** the field-name-uniqueness lift — once the one
  real unlock annotations offered — shipped 2026-07-03 via the use-site multimap, *without* any annotation.
  What remains is doc signatures, error *localization*, and `num` pinning — the least new capability for the
  most cost, and they fight a load-bearing syntax decision: a depth-0 `:` is the `match`/`case` block opener
  and §8.3 leans on `:` being unused elsewhere, so `let x : int` needs a disambiguating rule (hence L).
  **Revisit only on a concrete driver:** (a) error messages become a real pain — in which case improve HM
  *diagnostics* directly, cheaper and helps all code; (b) a deliberate F#-parity call. **Cheap partial slice
  if wanted:** param annotations `(x: T)` are feasible on their own (inside brackets `:` is free — record
  fields already use it), covering most of the doc/localization value.
- **Function composition `>>` / `<<`** — ✅ **done 2026-07-03**. `f >> g` = `fun x -> g (f x)`
  (left-to-right, f then g); `f << g` = `fun x -> f (g x)` (right-to-left / math ∘). Two-char lexer tokens
  (`GtGt`/`LtLt`) lexed before single `<`/`>`; a `parse_compose` precedence level between `|>` and `or`
  (tighter than `|>`, left-associative); `ExprKind::Compose` desugars to a composition lambda
  (`desugar::compose`) at inference + lowering, like the operator sections. Capture-safe: the lambda param
  is chosen free of both operands' free vars (`_pf_x`, …). Covered by roundtrip/typecheck/compile +
  `examples/hello.pyfun`.
- **Backward pipe `<|`** — ✅ **done 2026-07-03** (added for `|>`/`<|`/`>>`/`<<` symmetry). `f <| x` == `f x`
  (apply the left function to the right argument; F#'s `<|` / Haskell's `$`). `Tok::PipeLeft`; a `backward`
  flag on `ExprKind::Pipe` (forward applies rhs to lhs, backward applies lhs to rhs); **right-associative**
  (`f <| g <| x` = `f (g x)`), lowest precedence with `|>`. Lowers to plain application (flattens through
  `flatten_app` like `|>`). Covered by lexer/roundtrip/typecheck/compile tests.
- **Raw strings `r"C:\path"`** — ✅ **done 2026-07-03**. **Lexer-only** (`lex_raw_string`, mirrors the
  `f"` interception): an adjacent `r"` reads to the closing `"` with no escape processing (backslashes
  literal), following Python's rule that `\` keeps itself + the next char and `\"` does not terminate. It
  yields an ordinary `Tok::Str`; the emitter's `string_literal` escaper re-escapes on output, so there is
  no AST/type/lowering change. `rf"..."` out of scope. Covered by lexer + compile tests + `examples/hello.pyfun`.
*Cross-module (file-modules follow-ons)*
- **Cross-module externs / measures** — ✅ **done 2026-07-03**. An imported `extern` (`Mathx.cbrt`) now
  exports like a value (its name joins `run()`'s `exported`, so its scheme — `io` on the innermost arrow
  for a non-`pure` extern — joins `ModuleExports.schemes`) and, in the **project lowering path**, is also
  bound at top level in its own module (`cbrt = math.cbrt`, `import math` hoisted, via `Lowerer::project_mode`
  — single-file still erases externs); the consumer routes `Mathx.cbrt` → `mathx.cbrt` and `export_arities`
  includes externs so partial application (`List.map Mathx.cbrt xs`) curries. **Measures** merge *unqualified*
  (there is no qualified unit syntax — `<m>` is bare): `ModuleExports` carries `measures`/`measure_aliases`,
  `merge_imported_types` inserts base names idempotently (a shared `Units` module imported everywhere is the
  common case) and aliases with a **different-expansion conflict error** (two imports mapping `N` to
  different units errors; a shared base measure does not). Measures erase at lowering, so no lowering change.
  Covered by `tests/project.rs` (extern + measure e2e, effect transplant, alias conflict).
- **Cross-module records** — ✅ **done 2026-07-03**. Records now cross a module boundary on the *same
  rails as sum-type ADTs*: a consumer can **construct** (`Geometry.Point { x = 1, y = 2 }`),
  **pattern-match** (`case Geometry.Point { x, y }:`), **update** (`{ p with x = 3 }`), and **bare-access**
  a field (`p.x`) of an imported record. Export/merge mirrors ADTs (`ModuleExports.records`,
  `collect_exported_records`, `merge_imported_types` registering the record under its **bare identity
  name** with a qualified surface alias `Geometry.Point → Point`); lowering reuses the qualified-ctor
  dotted-class path (`geometry.Point(...)`, `import geometry` hoisted, the class defined in exactly one
  module). Field-name resolution was solved *without* row polymorphism (still a non-goal) or project-wide
  uniqueness: **field names are no longer globally unique** — `field_owner` is a multimap and a bare `p.x`
  resolves iff **exactly one** visible record declares `x`; **2+** is an ambiguity error *at that access
  site* only (never at declaration or import, so module isolation holds — two unrelated modules may both
  have an `x`/`name`/`id`). This also **relaxes the in-file field-reuse restriction**: two local records
  may share a field name and compile; only an ambiguous bare access errors (tagged construction/patterns
  disambiguate). The escape hatch (a qualified accessor / type-directed tiebreak) was deliberately *not*
  built — the error + hint is the whole feature. Full rationale in `DESIGN.md` §8.3. Covered by
  `tests/project.rs` (cross-module e2e) + `tests/typecheck.rs` (ambiguity + shared-field cases).
*Tooling*
- **REPL** — ✅ **done 2026-07-02**: `pyfun repl` (`src/repl.rs`). Keeps session **definitions** as
  accumulated Pyfun source; each entry is type-checked (via `analyze`) against them — a definition is
  remembered and echoes its inferred type (GHCi-style `name : type`), an expression is compiled with the
  accumulated defs and run once through Python, printing its value (nothing for a `unit`-typed expr, whose
  effect still runs). Commands: `:type`, `:{ … :}` (multi-line — needed to enter mutually-recursive
  functions as one group), `:reset`, `:help`, `:quit` (or Ctrl-D). **MVP limitations:** entries are
  single-line unless in a `:{` block; state = the definitions, which re-run on each expression eval, so
  *pure* defs feel persistent but a top-level effect or `let mut` doesn't carry across entries (a
  persistent-Python-process design is the future step). Covered by `tests/repl.rs`.
- **Project-wide LSP cache + truly incremental reparse** — ✅ **cache half done 2026-07-03**;
  reparse half **decided against**. Import-aware analysis is now cached at two fingerprint-validated
  levels (`DESIGN.md` §9): each per-document entry records the imported files it consulted (content
  fingerprints, `None` = absent), so a hit needs the doc version *and* all deps to match — **editing an
  imported file (open buffer or on disk) re-analyzes dependents** on their next request, fixing the old
  "cache not invalidated on import change" MVP limitation; and a **project-wide exports cache**
  (`ModuleExports` per module-file URI + its own dep list) shares one parse + check of a common import
  across all open documents. Imported sources read open-buffer-first (else disk); cycle-context
  interfaces are "tainted" and stay out of the shared cache. No behavior change — same diagnostics/
  hover/nav/rename, just not recomputed. **Truly incremental reparse is not worth it here**: whole-file
  lex + parse + check is milliseconds at realistic file sizes, the caches remove all redundant
  whole-file work, and region reparse would complicate the offside lexer + recovering parser for no
  perceptible win. Covered by `src/lsp` unit tests (invalidation via buffer + disk, fingerprint reuse,
  shared exports, cycle taint) + a `tests/lsp.rs` e2e (hover reflects an imported buffer's edit).
- **Doc-comment syntax + richer hover** — ✅ **done 2026-07-03**: `##` doc comments (column 0, one or
  more lines) attach to the following top-level `let`/`type`/`extern` as AST metadata
  (`doc: Option<String>`), are re-emitted by the pretty-printer (roundtrip-safe), erase at lowering,
  and surface in LSP hover — appended below the inferred type on the declaration name or any
  reference to it; a documented `type`/`extern` name with no recorded type hovers to the doc alone.
  Ordinary `#` comments (and `##` indented/trailing/bracketed) are untouched. Syntax + rationale in
  `DESIGN.md` §7; hover in §9. (A *separate effect line* in hover remains the deferred remainder.)

### Warts (small, low priority) — ✅ all cleared
All three original warts were **fixed 2026-07-03** (one sitting):
- ~~**No guiding error for `+` on strings**~~ — ✅ the numeric mismatch from `expect_num` now becomes
  `` `+` is numeric and does not concatenate strings — use `String.concat a b` `` when either operand is a
  `string` (`Infer::string_concat_hint`, `Add`-on-`string` only; every other numeric mismatch keeps its
  message). `+` stays numeric; overloading it for strings is still deferred.
- ~~**A bare literal unified to `float` prints `7` not `7.0`**~~ — ✅ an integer literal that inference
  *monomorphically* resolved to `float` (e.g. the `2` in `[1.0, 2, 3.0]`, or the `1` in `if b then 1 else
  1.5`) now lowers to a Python float (`2.0`/`1.0`), so `print` shows it as a float. `compile` runs one
  `check_collecting` pass, `lib::float_literal_spans` collects the `float`-typed spans, and lowering emits
  `PyExpr::Float` for a value-position `ExprKind::Int` whose span is in that set (`Span` gained `Hash`; the
  project path threads per-module spans via `types::check_module_collecting` in topo order). NB a
  *generalized* `let x = 7` stays `7` — `x` is polymorphic `num`, not float, so that was never the bug.
- ~~**Float literal patterns give a parse error, not a guiding one**~~ — ✅ `case 1.5:` / `case -1.5:` now
  report `` float literals can't be matched — float equality is unreliable; bind a variable and use a guard
  instead, e.g. `case x if x == 1.5:` `` (`parse_atom_pattern` → `float_pattern_error`). Matching floats
  stays intentionally unsupported (int/string literal patterns are the leaves); the error now *teaches* the
  guard alternative. Covered by `tests/{typecheck,compile}.rs`.

---

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
- **Field resolution (updated 2026-07-03):** field names are **no longer globally unique**. A bare
  `e.x` / `{ e with x = … }` resolves iff **exactly one** visible record declares `x`; two or more is an
  ambiguity error *at that access site* (never at declaration/import). Tagged construction and patterns
  carry their record type, so they are never ambiguous. This dropped the old "reusing a field name is a
  compile error" rule *without* annotations or row polymorphism — see §Records / cross-module below.
- **Cross-module (done 2026-07-03):** records export like sum-type ADTs — construct/pattern/update/access
  an imported record via a qualified tag (`Geometry.Point { … }`). See the cross-module entry in the
  backlog section for the full design.
- **Still to do:** (nothing major). Record *patterns* in `match` **landed**; cross-module records
  **landed**; the unique-field-name restriction is **lifted** (above); **derived ordering landed** (record
  values compare structurally — see the backlog entry).

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
- **✅ Prefix negation `-e`** (`UnOp::Neg`): `num`-constrained, **unit-preserving** (`-5<m> : int<m>`).
  A parser-level prefix operator (not a lexer negative-literal, avoiding the `x-1` whitespace trap):
  `-` is subtraction with a left operand, negation without; binds tighter than `*`/`/`, looser than
  application. Coexists with the `(-)` section; enables negative integer literal patterns (`case -1:`).
  Lowers to Python `-x`. (This closed a real gap — negatives were previously only reachable via
  `0 - n`.)
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
- **Chained comparisons — ✅ done:** `a < b < c` is Python-style (a single `ExprKind::Compare` lowering
  to Python's native chained comparison — evaluate-once, short-circuit — not the left-assoc `(a < b) < c`).
- **Derived ordering — ✅ done 2026-07-03:** `< > <= >=` (and `List.sort`) now work on user sum types,
  records, and tuples, compared structurally (variant declaration order then fields; lexicographic tuples),
  plus built-in `Option`/`Result` (`None < Some`, `Ok < Error`). `Set`/`Map`/`List` stay unordered. See the
  backlog entry.

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
status. Covered by `tests/run.rs`.
- **Effort/risk:** Low. **Status:** landed. **Follow-on `pyfun repl` also landed** (see the Deferred
  list's REPL entry) — interactive read-eval-print built on the same compile-and-run-Python pipeline.

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
`dict.get`. `List.zip : List a -> List b -> List (a, b)` and `Map.ofList`/`Map.toList` (to/from a
`List (k, v)`) bridge through **tuples**. Lists keep `[1,2,3]` literals; the hashed collections have no
literals (`{…}` is taken) and
no constructors. Keys/elements must be hashable at runtime — primitives and ADT/record values both are
(generated structural `__hash__`). A singly-linked `list` + `cons`/`head`/`tail` is a non-goal (`List`
*is* F#'s array); **sequence patterns on `List` (`case [x, *rest]`) have landed** (see the Deferred
section's now-done entry); the lazy counterpart is the `seq {}` CE. Covered by
`tests/{typecheck,compile,roundtrip,run}.rs` + `examples/hello.pyfun`.
  The `Option`, `Result`, and `Seq` modules have landed too: `Option.map`/`withDefault`/`isSome`/
  `isNone`; `Result.map`/`mapError`/`bind`/`withDefault`/`isOk`/`isError`/`toOption`; and the lazy
  `Seq.map`/`filter`/`take`/`fold`/`toList`/`ofList`/`range` (the map/bind ones effect-poly;
  `Result.toOption` bridges to `Option`; `Seq` routes to Python's lazy `map`/`filter`/`islice`/`range`).
  **In-file user modules have landed**: `module Name = <indented let bindings>` declares a namespace;
  members see siblings unqualified inside and are `Name.member` outside, lowering to mangled top-level
  names (`Geometry_area`). Reuses the same `Module.member` access mechanism as the built-ins (no parser
  change for access). MVP: `let`-only bodies, no nested modules.
  **The standard combinators have landed** (unqualified prelude): `id : 'a -> 'a`,
  `const : 'a -> 'b -> 'a`, `ignore : 'a -> unit`, `flip : (a -> b -> c) -> b -> a -> c` — the natural
  companions to the composition/pipe operators. Fully type-polymorphic; `id`/`const`/`ignore` are pure,
  `flip` is **effect-polymorphic** (it calls its function argument, so flipping an impure function is
  `io` — soundly rejected inside a `let pure`). None can lower name-for-name (Python's `id` returns a
  memory address; the rest have no builtin), so each routes in `lower_var` to an emitted
  `_pf_id`/`_pf_const`/`_pf_ignore`/`_pf_flip` helper (`combinator_prelude`, the on-demand mechanism the
  collection helpers use); `_pf_flip(f, x, y)` = `f(y, x)` n-ary, exactly what a hand-written
  `let flip f x y = f y x` compiles to (so no more/less capable than that). Covered by
  `tests/{typecheck,compile,roundtrip}.rs` + `examples/hello.pyfun`.
- **Still to do (a larger prelude):** `Array` is **deferred** as redundant (`List` already *is* a
  Python dynamic array); and the full *file-based* module system (one module per file, `import`, a
  resolver + dependency graph, visibility, multi-file LSP) — a separate, larger initiative. (Generated
  `__hash__` on ADT/record classes and **in-file modules** are **done**.)
- **Effort/risk:** Medium. **Status:** MVP prelude + FFI + lists + sets/maps + options + results + lazy
  seq + **the `String` module** (text ops; `String.toInt` via the new `PyStmt::Try` node) + built-in &
  in-file modules + ADT `__hash__` + **standard combinators** (`id`/`const`/`ignore`/`flip`) done;
  file-based modules done.

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
  outline); a separate effect line in hover (doc-comment hover — `##` docs — shipped 2026-07-03).
- **Effort/risk:** the headline features all landed; the remaining tail is low-value at current scale.

## Suggested sequencing

All four language pillars beyond the MVP core are now done: **#1 (effects)**, **#2 (records)**,
**#3 (mutability + blocks)**, **#4 (floats)** — on top of `run` + prelude + the general offside rule.
The remaining work is breadth and polish, not new pillars. Highest leverage next:

The general FFI surface (`extern`) and the eager `List` collection (both #9), and the **#10 LSP**
(diagnostics + hover-for-type/effect + go-to-def/find-refs/rename/completion/document-symbols over
resilient, cached analysis + a VS Code client) are now done. Remaining, in rough priority:

1. **Prelude breadth (#9 cont.)** — lists/sets/maps/options/results/lazy-seq + built-in & in-file
   modules + ADT/record `__hash__` + **tuples** + the tuple-enabled stdlib (`List.zip`,
   `Map.ofList`/`toList`) landed. `Array` deferred as redundant with `List`.
   **File-based modules (Phase 2) are now COMPLETE** — see `DESIGN.md` §6.1, with a runnable example in
   `examples/modules/`. Decisions (all
   Python-natural): explicit `import Geometry` + qualified `Geometry.area` (reuses `qualified_name`, no
   parser change for access); **parallel `.py` output** (un-mangled names, `geometry.area`, real Python
   `import`); **all public** (no `pub`); a small **implicit-recursion** precursor (function bindings see
   themselves like Python `def`; TCO deferred — CPython has none, use `List`/`Seq` combinators for
   stack-safe bulk work). Flat single-dir namespace + **acyclic** import graph (cross-file
   declare-before-use). A generated **`_pyfun_rt.py`** holds the `Option`/`Result` classes so those
   values stay `isinstance`-compatible across files; MVP exports **values only** (cross-module
   types/ctors deferred). Seven ordered slices; **all seven have landed** — (0) implicit recursion (a
   function binding is in scope in its own body like Python `def`, no `rec`, monomorphic, value bindings
   excluded); (1) `import` syntax (`import Geometry` → `Item::Import`; lexes/parses/pretty-prints/
   round-trips, a no-op until the driver resolves it); (2) the multi-file driver (`src/project`:
   loader-injected DFS building an acyclic graph with cycle/missing-file/parse errors and a topo order,
   `build_from_path` the `.pyfun` wrapper); (3) cross-module value checking (`types::check_module` seeds
   a module's env with its imports' exported value schemes under qualified keys and returns its own
   exports; `project::check` threads them through the topo order, reporting "not a member" for unexported
   uses); (4) multi-file lowering + emit (`lowering::lower_in_project`: `Geometry.area` → `geometry.area`
   + hoisted `import geometry`, un-mangled names, cross-module partial application curries; nominal
   `Option`/`Result` classes hoisted to a shared `_pyfun_rt.py` via `lowering::runtime_module` so values
   stay `isinstance`-compatible across files; `project::compile` emits the `.py` tree); (5) the CLI over
   the graph (`check`/`compile`/`run` in `src/main.rs` detect imports and drive the project — whole-graph
   check with per-module diagnostics, `compile -o <dir>` writes the tree, `run` materializes to a temp dir
   and executes `python main.py`; a no-import file keeps the exact single-file behavior); (6) minimal
   import-awareness in the LSP (`analyze_in_dir` resolves an entry's imports from sibling files via
   `project::resolve_imports` and seeds `types::check_collecting_with_imports`, so a multi-module file
   checks `Geometry.area` cleanly in the editor; the server maps the `file:` URI to a directory); (7)
   docs + the runnable `examples/modules/` project. **Phase 2 is complete.** Follow-on landed:
   **cross-module sum-type ADTs** (construct `Geometry.Circle 2.0`, qualified pattern `| Geometry.Circle r
   ->`, cross-boundary exhaustiveness; `merge_imported_types` + qualified ctor patterns + dotted-class
   lowering). Dropped as **explicit non-goals**: visibility (`pub`, all-public is the Python-natural model)
   and TCO (`List`/`Seq` combinators are the stack-safe path). **Cross-file LSP navigation** also landed:
   go-to-definition across files (`resolve::qualified_at` + `locate_cross_file`), `workspace/symbol`, and
   **project-wide find-references + rename of top-level values and constructors** (`symbol_occurrences`;
   constructor occurrences incl. patterns via `Pattern::Ctor`/variant `name_span` + `resolve::walk_pattern`;
   sound — kind-matched, strict scan refuses on parse failure), and **in-file find-references / rename of
   type names** (`TypeExpr::Con`/`TypeDecl` `name_span` + `resolve::walk_type` + `type_at`; types have no
   cross-file dimension), **plus cross-file go-to-def on a qualified record tag** (`Geometry.Point` — the
   `Record`/`Pattern::Record` arms of the resolver now push a `QualRef` for a dotted tag). **Cross-module
   records, externs, and measures all landed 2026-07-03** (construct/pattern/update/access an imported
   record with a use-site field-ambiguity multimap; imported externs bound + routed; measures merged
   unqualified with an alias-conflict check). The **project-wide LSP cache landed 2026-07-03** (see
   *Tooling* above).
2. **#5–#7 — all landed**: deep exhaustiveness (full Maranget usefulness with witnesses),
   user-defined CE builders (module-based, desugared), derived-measure aliases. Plus the #2/#3
   follow-ups: record patterns **landed**, blocks in `match`/`if`/lambda positions **landed**.
   Closure capture of a reassigned `mut` (`nonlocal`/`global`) **landed**. **Tuples** (structural
   `(a, b)` literals/patterns/types, deep-exhaustive) **landed**. **Sequence patterns on `List`**
   (`case [x, *rest]`, `Nil | Cons` exhaustiveness modeling, `PyPattern::ListSeq`) **landed 2026-07-03**;
   the only remaining slice is a non-last star (`[*init, last]`). The linked-list `cons`/`head`/`tail`
   variant is a non-goal.
3. **#10 LSP tail** — workspace symbols **landed** (with cross-file nav); the project-wide cache
   **landed 2026-07-03**; doc-comment hover **landed 2026-07-03**; truly incremental reparse
   **decided against** (whole-file analysis is milliseconds at this scale — see *Tooling* above).
