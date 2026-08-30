# Pyfun — Roadmap

The MVP showcase set (curried functions + `|>`, ADTs + exhaustive matching, computation expressions,
units of measure) **and** Phase 2 file-based modules are complete — the language is feature-complete for
its intended scope, and nothing below blocks normal use.

This is the single forward-looking list of what's **not** built, so nothing is drip-fed. Design mechanics
and rationale live in [`DESIGN.md`](./DESIGN.md); what shipped and when is in git history. Effort is
rough: **S** ≈ a sitting, **M** ≈ a focused day, **L** ≈ multi-day.
Keep this a *forward-looking* backlog — do not let it grow back into a changelog of shipped work.

## Dogfooding findings (2026-07-31, first real interactive program)

A dogfooded interactive terminal game (private repo) hit six gaps that a compiler test suite does not
reach: the first Pyfun program to be *interactive*, *stateful across turns*, and written to a module
layout chosen before the compiler had an opinion. Each root cause below is verified in the source, not
inferred from the symptom. All six are accepted work, and the two that carried an open decision (the
size of the standard-library sweep, and which of three shapes answers the recursion gap) were settled
on 2026-07-31; each entry records what was chosen and what was turned down with it.

1. **Imported types cannot appear in a `type` declaration** (S) — `types::run` calls `build_decls`
   (which resolves every record field and ADT variant against `type_arity`) *before*
   `merge_imported_types`, so an imported type is not registered yet when local type bodies resolve.
   `type Holder = { item: Placed }` fails with "unknown type" even though the bare name is exactly how
   imported records register; the qualified spelling `Shapes.Placed` additionally does not parse
   (`parse_type_atom` accepts a bare `Ident`). Fix: register imported names and arities in a pre-pass,
   then allow dotted names in type position. **Highest language impact of the six**: it forces every
   record mentioning another module's type into a single file, which collapsed a board module and a
   rules module into one 340-line engine in the dogfooded program. This is the only finding that
   changed a program's architecture rather than its phrasing. **Follow-up it creates** (S, LSP): a type
   name can now be written in a file other than the one declaring it, so rename and find-references
   for *type* names, which are in-file only, can leave stale references behind. Cross-file type nav was
   never built because qualified type syntax did not exist; it does now, and `resolve::type_at` needs
   the cross-file dimension the value and constructor paths already have (`symbol_occurrences`).
2. **Field access picks its record before the base type is known** (S) — `Infer::infer_field` calls
   `record_of_field` (the name-only multimap) and infers the base one line later, so two records
   sharing a field name collide at *every* use site even where the base's type is already solved. The
   dogfooded program paid for it in Hungarian prefixes (`cRow`, `cCol`, `cLetter`, plus nine
   `g`-prefixed fields on the game state), which is a wart in an ML-family language, and the suggested
   workaround (pattern-match to disambiguate) means destructuring at every use site. Fix: infer the
   base first and use its solved `Ty::Con` when that record declares the field, keeping the multimap
   as the fallback for a still-unsolved base. That is F#'s own rule, and it needs no new syntax.
3. **Single-file `pyfun run` cannot feed the program stdin** (S) — `main::run` pipes the emitted
   source to `python -`, so the program's stdin *is* its own source text and the first read raises
   `EOFError`. Interactive programs are therefore un-runnable by the tool whose job is running
   programs. The project path (`main::run_project`) already materializes to a temp directory and
   inherits stdio, so a multi-module interactive program works today: the fix is making the
   single-file path do what the project path does. Tooling, not language design.
4. **No tuple patterns in function or lambda parameters** (M) — `parse_param` is `parse_ident`, so
   `fun (t, sq) -> …` does not parse and anything folding over pairs needs a named helper wrapping a
   `match` (five such one-line functions in the dogfooded program). Widening `Param` to a pattern
   reaches the LSP, where `Param{name,span}` feeds hover, go-to-definition and rename. **Follow-ups**
   (S each, both wanted 2026-07-31): **record patterns in parameters**
   (`fun (Cell { letter }) -> …`), which are irrefutable and so belong in the admitted set, but need
   attribute-reading lowering rather than tuple unpacking; and **narrowing the self-tail-call capture
   guard** below to true free variables (see #6).
5. **Standard library completion** (L, sliced per module) — the dogfooded program wrote 11 scaffolding
   functions before it could start on the game, and defined `takeN`/`dropN` index-based over
   zip-with-indices because the natural recursive definitions are stack-unsafe (item 6). Confirmed
   missing across the prelude: `fst`/`snd` (no tuple accessors at all), `List.take`/`drop`/`head`/
   `tail`/`last`/`map2`/`indexed`/`exists`/`forall`/`sortBy`/`distinct`/`max`/`min`/`partition`/
   `updateAt` and more; `Seq` carries 7 members against `List`'s 16; `Map` and `Set` have no
   `map`/`filter`/`fold`; `Option` and `Result` have no `map2`/`orElse`/`iter`. The standing "prelude
   functions on demand" policy (under Deferred) is what produced this backlog one program at a time,
   so the decision is to complete the surface in one sweep instead. **Decided 2026-07-31**: the full
   sweep, about 90 members, F# core as the reference, then a member-by-member `FSharp.Core` audit to
   catch what this list missed; excluded is anything needing type classes or an un-Pythonic lowering.
   Five PRs split by module. Three conventions settled with it, and the first two were already the
   house style rather than new rules: **total functions** (`String.slice` clamps, so `take`/`drop`
   clamp), **bare names returning `Option`** for accessors that can fail (`head : List a -> Option a`,
   following the existing `List.get`/`List.find`, diverging from F# where `head` raises), and
   **`take`/`drop`** rather than F#'s `take`/`skip` (the `concat` divergence already set that
   precedent). `String.tryIndexOf` is the one member out of step with the accessor convention; it
   stays as-is (the audit turned up no others). The positional-update family
   (`updateAt`/`insertAt`/`removeAt`) is the most directly game-shaped gap: a board update has no
   vocabulary at all today. **Sweep COMPLETE 2026-07-31** — `List` (+`fst`/`snd`), `Seq`, `Set`/`Map`,
   `String`, `Option`/`Result`, then the `FSharp.Core` audit, which added 21 more members. Nearly all
   of those came from *internal* asymmetry rather than F# parity: `takeWhile`/`dropWhile` on `Seq` but
   not `List`, nine `List` members with no `Seq` counterpart, `iter` on every module except `Set` and
   `Map`, `Option.exists` with no `forall`, and `sign` as the one missing F# global. The audit's more
   valuable half was a bug: every multi-argument callback's scheme put the effect variable on *every*
   arrow, so an impure two-argument callback could never unify and `List.fold` could not print
   (`DESIGN.md`, "Effects through a multi-argument callback").
6. **Unbounded recursion has no stack-safe form** (M, decided) — an interactive turn loop is not a
   collection traversal, so the Non-goals answer below ("iteration is the `List`/`Seq` combinators
   plus recursion") does not cover it: every turn and every rejected input is a frame that never
   returns until the game ends, and the dogfooded program calls `setRecursionLimit 20000` at startup
   to survive a long game. This is the first genuine evidence against that non-goal, and it is
   narrower than the non-goal's scope: the shape at issue is a *self* tail call driving a loop, not
   general TCO and not `while`. **Decided 2026-07-31**: lower a direct, saturated self tail call to
   `while True` with parameter rebinding. It is contained in `src/lowering`, needs no surface change,
   emits the `while` loop a Python programmer would have written by hand (so it costs nothing in
   readability), and it also makes the natural recursive `take`/`drop` stack-safe, which pays part of
   item 5. The precision requirement is the work: the tail-position walk covers match arms, `if`
   branches, block tails and `let` bodies, and must not fire where the function is partially applied
   or captured. Rejected alongside it: a state-in/state-out `Loop` combinator (stack-safe for free and
   S effort, but a second loop idiom that goes redundant the moment this lands, and it does nothing
   for recursive list functions), and reopening `while` (it needs `let mut` to be useful and fights
   the expression orientation, and this covers the actual complaint without it). Mutual-recursion
   trampolining stays out: it costs the readable output that lowering exists to protect.
   **Known over-rejection** (S to fix): the capture precondition compares *every name mentioned*
   inside a nested function against the names this frame binds, so it also rejects two shapes that are
   in fact safe — a lambda whose own parameter merely shares a name with one of ours
   (`fun n -> n + 1` inside a function whose parameter is `n`: no capture at all), and a closure that
   genuinely captures but is consumed within the iteration and never escapes
   (`List.fold (fun acc x -> acc + x * n) 0 xs`). Subtracting the names bound *inside* the nested
   function fixes the first outright and is strictly sound; the second needs escape analysis and is
   not worth it. Rejections are silent, which is the real cost: a program keeps its recursion without
   saying why.

7. ~~**A dotted `extern` target imports the wrong prefix when a lowercase segment is not a module**~~
   **CLOSED 2026-07-31** (was S–M, reported the same day) — fixed with option (b): the undecidable
   shape is now a compile error naming the `extern import` to add (`types::undecidable_extern_segment`,
   checked in `build_decls` so `pyfun check` reports it, not only `compile`). Two shipped externs in
   `examples/interop/http_fetch.pyfun` needed the declaration the diagnostic asks for, which is the
   expected cost of the trade. Original report below. — `extern flush : unit -> unit = sys.stdout.flush` emits
   `import sys.stdout`, which raises `ImportError`. The *call* is always right; only the import line
   is wrong. `lowering::extern_import` takes the **maximal leading run of lowercase-initial segments**
   before the final name, following PEP 8 (packages lowercase, classes capitalised), so it succeeds
   exactly when that run happens to be a real submodule:

   | target | emitted | outcome |
   | --- | --- | --- |
   | `pathlib.Path.write_text` | `import pathlib` | works, `Path` is capitalised so the run stops |
   | `os.path.join` | `import os.path` | works, `os.path` really is a submodule |
   | `sys.stdout.flush` | `import sys.stdout` | fails, `stdout` is an object |

   **Workaround today, and it does work:** declare the module (`extern import sys`), which
   `extern_import_spec` consults before the heuristic and which emits `import sys`. The function's own
   doc comment already names this case, so it is a known limit rather than a surprise — but emitting a
   line that cannot import is still the wrong failure mode, and the fix should stop at a compile error
   at worst.

   **The obvious fix does not work as stated.** "Import the longest *importable* prefix" is not
   statically decidable: whether `urllib.request` or `sys.stdout` is a module is a property of the
   target environment, not of the text. Nor does "always import only the top-level package", which
   trades one broken case for another — verified on CPython 3.14: `import urllib` leaves
   `urllib.request` unbound (`AttributeError`), while `import os` does bind `os.path` and `import sys`
   does bind `sys.stdout`. So the candidates are: **(a)** emit the top-level import plus the deeper one
   guarded by `try/except ImportError`, which is correct everywhere and mildly ugly, once per module;
   **(b)** keep the heuristic but *reject* the ambiguous shape at compile time with a fix-it naming the
   `extern import` to add, which fits "the compiler is the gatekeeper, no runtime surprises" and costs
   nothing at runtime; or **(c)** both — (b) by default, with (a) where the shape is unambiguous.
   **Decided 2026-07-31: (b).** A compile error naming the one line to add beats an import that may or
   may not work in the target environment, and it keeps the emitted Python free of defensive
   `try/except` around something the author can state exactly. The check fires only on the shape the
   heuristic cannot see through — a target with a lowercase segment after the first, before the final
   name — and the message carries the fix (`add `extern import sys``). The `extern import` escape hatch
   already exists and already wins over the heuristic, so this turns a silent runtime failure into a
   diagnostic pointing at the existing answer.

## Dogfooding findings (2026-08-02, second session on the same program)

A second pass over the same private game found two gaps, both about the *shape* of code rather than a
missing capability: each had a working spelling already, and each cost the readable Python that lowering
exists to protect. **Both shipped on 2026-08-02**, in the order below reversed (destructuring first, so
the `option` example landed in its final form). Together they turn a chained-`Option` function into the
statements a Python programmer would have written:

```pyfun
let parseCoord tok =
  option {
    let! c0 = List.get 0 (String.toList tok)
    let! ci = List.findIndex ((==) (String.upper c0)) Render.colLabels
    let! rn = String.toInt (tok |> String.toList |> List.drop 1 |> String.join "")
    return! if 1 <= rn and rn <= Board.size then Some (rn - 1, ci) else None
  }
```

8. ~~**`Option` is the one short-circuit type with no computation expression**~~
   **CLOSED 2026-08-02** (was M, reported the same day) — `option { }` is the fourth built-in.
   The stdlib's accessor convention (item 5: bare names returning `Option`) makes `Option` the type it
   pushes you to chain, and a chain of them meant one nested `match` per step with an identical
   `case None: None` arm at every level. A user-defined builder could not close it, which was the
   point: `src/desugar.rs` is an *expression* transform, so `Opt { … }` compiled to nested `bind`
   lambdas on one line, where the bespoke lowering emits flat `match` statements with early returns.

   **What keeps a fourth built-in from becoming a fifth**, now recorded in `DESIGN.md` §8.1 and on
   `CeBuilder`: one per built-in *short-circuit type* (`Option`, `Result`) and one per Python
   *control-flow form* (`async`, `seq`). There is no fifth candidate, so the set closes at four.

   `result` and `option` turned out to be one lowering, not two: `lowering::ShortCircuit` names the
   success and failure constructors and whether the failure carries a payload, so neither can drift
   from the other. `option`'s is the simpler half — `None` carries nothing, so its failure arm binds
   nothing and returns a fresh one (`case None_(): return None_()`). Also shipped: the near-miss
   diagnostic (uppercase `Option { let! … }` is rejected with a message naming the lowercase form,
   instead of resolving against the prelude module and failing on `Option.bind`'s pipe-first argument
   order), the `a`/`an` fix in the CE diagnostics this exposed (`async` had it too), and the builder
   keyword across every editor target. Lesson 13 gains the `option` section and the
   two-or-more-binds threshold; lesson 20 keeps the `Option` builder as its worked example, now
   introduced as a rebuild of a battery you have, with the lowering difference as the reason the
   built-in exists.

9. ~~**A `let` binding cannot destructure**~~ **CLOSED 2026-08-02** (was M, reported the same day) —
   `let (r, c) = parseCoord tok`, `let Point { x, y } = origin` and `let (a, (b, c)) = nested` now parse
   at top level, in blocks, in an in-file `module`, and on a computation expression's `let`/`let!`.
   `LetBinding` and `CeItem::Let`/`LetBang` carry a `Pattern` where they carried a `String`, with
   `bound_names`/`bound_vars` on the AST as the one place every phase asks what a binding introduces.
   The irrefutability rule turned out to already exist — `parser::refutable_in_param`, renamed
   `refutable_shape` and now shared — so a `let` target admits exactly what a parameter does (a name,
   `_`, tuples, records, nested) and rejects the rest with a message naming what was written and
   pointing at `match`. A function binding and a `let mut` keep their single name, each with its own
   message. Lowering reuses `unpack_into`, so `let (r, c) = e` emits `r, c = e` with no temp, a nested
   target reads through a reserved base (`_pf_t0_1`, never a name derived from the user's), a record
   target reads one attribute per field named, and a destructuring `let!` in `result` rides inside the
   `Ok` pattern it already matches (`case Ok((r, c)):`) for no extra statement. Each bound name
   generalizes on its own, matching the single-name case. Canonical `DESIGN.md` §7; lesson 8 covers it;
   the tree-sitter grammar accepts the new targets (and destructuring *parameters*, which the language
   had but the grammar did not).

## Dogfooding findings (2026-08-30, third session: performance)

A third pass over the Scrabble program was about runtime cost rather than missing capability: the
program was complete and correct, and the question was how far the emitted Python sat from what a
person would write in the hot paths. Six issues came out of it (#84 to #89), each measured by
hand-editing the emitted module and timing the result, and all shipped the same day in PRs #90, #91,
#93 and #94; a seventh (#92), found while fixing #84, and its sibling #96 shipped in the PR that adds
this entry.

10. ~~**Two miscompiles the checker could not see**~~ **CLOSED 2026-08-30** (#84 in #91, #92 and #96
    in the PR carrying this entry). A block-local `let pair a b` applied to one argument was emitted as a
    full call with one argument, because lowering took arity from a top-level table only; the
    checker had typed it as a partial application. Now a block-scoped arity table sits beside the
    fold pass's local-folder registry with the same save/restore/shadow discipline, so `List.map
    (pair 10)` with a block-local `pair` prints `[11, 12]`, and two lambda workarounds in the
    program came out. The second was the mirror image: a match-arm capture named like a block-local
    `def` (`case Some pair: pair`, then `pair r 1`) is arm-scoped in Pyfun but a function-wide local
    in Python, so the later call found an integer, and a `let` in a nested block (#96, an `if`
    branch rebinding `x`) had the same shape. A capture or nested `let` that reuses a name the
    function uses elsewhere is now emitted as `_name` (`DESIGN.md` §5, "Arm-scoped captures and
    nested-block `let`s"). Downstream review found the first rule too eager (#97, closed the same
    day): it renamed a name reused across sequential matches, where nothing is live across the
    rename, so the rule was tightened to liveness and `case Error why:` in three matches of one
    function keeps its plain name. The same review turned up the last member of the family (#99,
    closed the same day): a root-level `let x` in a function that had already read an enclosing
    `x` made `x` local to the whole `def` and the earlier read raised `UnboundLocalError`; such a
    `let` now renames exactly when a read in the function means a binding outside it.

11. ~~**A destructuring folder went quadratic**~~ **CLOSED 2026-08-30** (#85 in #90). The in-place
    fold pass rejected `fun m (p, l) -> Map.add p l m` because the element parameter had no single
    name, and the `functools.reduce` fallback copies the accumulator on every step. The element
    parameter may now be any irrefutable pattern (Python's own `for (p, l) in steps:` header). On
    the program's automaton build: 2,000 words 478ms to 15ms, and the full 267,751-word list builds
    in 1.85s, the same as the `fst`/`snd` spelling it used to need.

12. ~~**Option was a tax on every accessor**~~ **CLOSED 2026-08-30** (#86 to #89 in #93 and #94).
    Four costs in the stdlib's `Option` convention, each cheap alone and paid millions of times per
    position: `List.findIndex ((==) x)` ran a Python-level scan with a fresh lambda; `match
    Map.tryFind k m:` built a `Some` only to take it apart; every miss constructed a new `None_()`;
    and every `Option` match went through CPython's class-pattern dispatch plus an unreachable
    `case _`. Now `List.findIndex/find/exists ((==) x)` route to `list.index`/`in`; a match that
    consumes `Map.tryFind`, `List.head` or `List.get` on the spot lowers to `if k in m: v = m[k]`
    with no `Option` built; nullary constructors are module-level singletons (`_None_`, `_Across`);
    and `Option`/`Result` matches are `if isinstance(o, Some): x = o._0` ladders. Downstream:
    `takeLetter` emits `_pf_index_of(l, rack)` at 3 sites and `allowsLetter` emits `if rc in checks:
    s = checks[rc]; return l in s`, output identical (46,271 candidates); generation on the Mega
    position went ERSTAING 1.26s to 1.01s, ERSTAIN? 9.84s to 7.63s, QU??AING 13.3s to 10.3s, 1.29x
    overall against the 1.27x predicted from the hand-patched output; and scoring through
    `Rules.validate` gained about 8% (4.5s to 4.1s) without being the target, because it uses
    `Option` and `Map.tryFind` too. 45 engine tests and 4 golden replays byte-identical, no source
    changes needed.

    Three decisions made this round, all now in `DESIGN.md` §5: the `isinstance` ladder is the
    default emitter's output for `Option`/`Result` (the rider in the performance section below that
    kept it native-mode-only is retired; user ADTs still emit `match`/`case`); `slots=True` on every
    emitted class, so `vars(instance)` no longer works from Python, the one visible interop change
    and one to name in the next release notes; and nullary singletons are spelled `_Ctor`, with a
    fallback to `Ctor()` when a program binds that name.

## Dogfooding findings (2026-08-30, fourth session: network play and the browser)

A fourth pass over the Scrabble program scoped its two remaining features, network play (its roadmap
item 4) and a browser interface (item 3), against Pyfun 0.7.0 and filed nine issues (#103 to #111).
Nothing here is a miscompile of code the program already runs; every item is a place the language
ran out when the program reached for concurrency, a wire format, or a page. The decisions below were
made on 2026-08-30 and each entry records what was chosen and what was turned down with it. Network
play comes first; the browser target is last because it depends on the async decisions.

13. ~~**An `->{async}` extern is checked but never awaited**~~ **CLOSED 2026-08-30** (#104, in the
    PR carrying this entry). The lowering had no hook for the `async` effect label: an extern typed
    `float ->{async} unit` was emitted as a plain call, the coroutine dropped with a
    `RuntimeWarning`, and lesson 18 plus `examples/interop/http_fetch.pyfun` taught exactly that
    spelling. **Decided and done:** the `Async` *type* is the only thing the lowering awaits, and an
    `->{async}` extern whose result is not `Async _` is rejected at the declaration with the working
    spelling named (`-> Async a`, bound with `let!`); lesson 18, `hello.pyfun` and `http_fetch`
    moved to it (`DESIGN.md` §4). Effect-directed lowering (emit `await` wherever the label says so)
    was turned down: an `async`-effect function handed to `List.map` or any effect-polymorphic
    higher-order function would propagate the label onto a call that cannot await, so the result is
    silently a list of coroutines, and the rule needed to forbid that is a new checker concept for
    the gain of one `let!`. F# chose a type for the same reason.

14. ~~**CE bodies reject expression and `match` items, and `async` rejects a trailing `do!`**~~
    **CLOSED 2026-08-30** (#105, in the PR carrying this section). A unit-typed expression on its own
    line (`print reply`) was a parse error inside any CE, so was a `match` or `if`, and an `async`
    block had to end with `return`. The parser now reads a bare expression item as `let _ = e` (what
    the checker requires of it and what the canonical pretty-print spells), which admits `match` and
    `if` as items; a trailing `do! e` ends a `result`/`option`/`async` block as its value (`M unit`,
    an `await` as the last statement of the `async def`; a `result` forwards the step as it is, no
    ladder), as the user-builder table already did. `DESIGN.md` §8.1. Still open: CE items *inside*
    match arms (F# allows `let!` in an arm), a separate desugaring change that waits for a program
    to need it.

15. **`for` inside computation expressions** (#103, S–M, spelling decided). `seq { }` has no `for`,
    so "one per element" is `yield! (List.map f xs)`, which allocates a list to feed a generator; a
    user builder has no `for_` row either, and `DESIGN.md` §8.1 lists `For` as part of the protocol
    being followed. **Decided:** the Python spelling, `for x in xs:` with an offside block, consistent
    with `match e:` / `case`; `for` is a contextual keyword inside CE braces only. Native `seq` lowers
    to Python's `for` statement; a user builder desugars to `B.for_ e (fun x -> ...)`. Its two small
    siblings shipped with item 14: the first-item near-miss (`Html { for ...`, `Html { class_ …`)
    is now diagnosed as "not a computation-expression item", naming what the position takes, instead
    of as a record literal missing `=`; and `docs/src/internals/03-desugaring.md` now quotes the whole
    protocol table (`yield!` and `zero` were missing, the two a list-shaped DSL needs).
    **Turned down:** custom operations (F#'s `[<CustomOperation>]`, `class' "board"` as a bare item);
    they need a per-builder name table, and a list-shaped markup DSL (`div [attrs] [kids]`) is where
    F#'s own community settled.

16. ~~**A `unit -> a` thunk handed to Python is miscalled**~~ **CLOSED 2026-08-30** (#107, in the PR
    carrying this entry). `fun _ -> 41` lowered to `lambda _: 41` and `asyncio.to_thread` called it
    with no arguments. **Done:** an argument to an extern whose declared parameter type is
    `unit -> a` is wrapped as `lambda: f(None)` at the call site (a literal `fun _ -> body` collapses
    to `lambda: body`), at every extern call path (plain, kwargs, receiver). The issue's second rule,
    spreading a tuple parameter (`(a, b) -> c` wrapped as `lambda a, b: f((a, b))`), was turned
    down: Python callbacks that receive one tuple are everywhere (`sorted(pairs, key=f)`,
    `map(f, d.items())`) and the boundary cannot tell the two conventions apart from the Pyfun type.
    The rule is "curry your callbacks", and lesson 12 now has a section on handing Python a
    function, with the callback's effects on the parameter arrow (`DESIGN.md` §6). Still to check:
    a partially applied function (`serve (handler cfg)`) crossing the same boundary.

17. **An `Async` module, `Async.catch`, and structured concurrency** (#106, #108, #109, M; the
    module and `catch` **shipped 2026-08-30** in the PR carrying this entry, the scope is what
    remains). `Async` was a type with `async { }` and nothing else: no `sleep`, `timeout`,
    `toThread`, `parallel`, `race`, and no way to catch an exception at the await (`try e` catches
    at call time, so it wraps the coroutine and the `TimeoutError` or `ConnectionResetError` escapes
    at the `let!`). Now `Async.sleep`/`timeout`/`toThread`/`parallel`/`race`/`catch` are prelude
    members (`DESIGN.md` §6, "Async combinators"): `sleep` is `asyncio.sleep` itself, the rest are
    emitted `_pf_async_*` helpers because the extern syntax has no spread for `gather(*xs)`, `wait`
    returns task sets, and `timeout`/`catch` build `Option`/`Result` values; `catch` builds the same
    `Exception` record `try` does and lets `CancelledError` through. Structured concurrency ships as
    a library next, `Task.scope : (Scope -> Async a) -> Async a` over `asyncio.TaskGroup` and
    `Task.start : Scope -> Async unit -> unit`, so a start outside a scope is a missing argument
    (needs an `async with` node in the Python IR). A `task { }` spelling is wanted; **decided:** try it
    as a *user builder* over those helpers first (the §8.1 mechanism, no fifth built-in) and, if the
    result is ugly, amend the §8.1 rule in `DESIGN.md` with the argument written down rather than
    admit `async with` as "a control-flow form the rule did not count" (by that reading `with`, `for`
    and `try` qualify too and the set reopens). Open inside this item: whether a scope types as `Async
    (Result a (List Exception))` so an `ExceptionGroup` is a value. **Deliberately absent:**
    `Async.start` and `Async.cancel` (a free start is the leak). The `spawn` effect label that only a
    scope discharges, Pyfun's first effect handler, is an aspiration and goes in Deferred. No new
    Python floor: `TaskGroup` and `asyncio.timeout` are 3.11, Pyfun targets 3.12.

18. **A mailbox `Agent`** (#108, M, decided). F#'s `MailboxProcessor` types the game exactly (a
    keyboard task and a socket task post to one loop that is a `match` over a `Msg` ADT). **Decided:**
    it lives in `examples/interop/` first, built from `asyncio.Queue` externs and the `Async` module,
    with the game as its consumer; it is promoted to the prelude once its signature stops moving (#109
    already says `Agent.start` may need to take a scope, which is the kind of change a prelude module
    must not make after the fact).

19. **`Encode` to mirror `Decode`, with derived codecs** (#110, M–L, shape decided). A program that
    speaks to itself over a wire writes values out with f-strings and reads them back with
    `Decode.field` by hand, and the two drift; `Decode.map2` scales to two fields and a hand-written
    decoder for a record holding a `Map (int, int) Placed` is forty lines. Two mechanisms: `Encode.auto
    : a -> Json` is a runtime helper (the emitted classes carry their fields and case names, the same
    knowledge `__repr__` uses); `Decode.auto : Decoder a` needs `a` known statically at the use site,
    so it is type-directed lowering after inference (precedent: Decode specialization, `DESIGN.md`
    §5.3) with a rejection when `a` is still a variable. Both ship together so both ends are one line,
    and the property the tests state once is `Decode.auto (Encode.auto v) == Ok v`. **Decided shape:**
    internally tagged objects, the convention serde (`tag = "type"`), Pydantic discriminated unions
    and System.Text.Json share: a case with a record payload is `{"type": "Move", "square": "K11"}`,
    positional payloads are `{"type": "Move", "fields": [...]}`, `Option` is `null` or the value,
    tuples are arrays, a `Map` with string keys is an object and any other key type is a list of
    `[k, v]` pairs. F#'s `{"Case", "Fields"}` is the outlier and was not copied. The game's `.replay`
    files are free to adopt the same encoding.

20. **A browser target** (#111, L, last). Three pieces once the async items are in: `pyfun bundle`
    (a static page: the compiled Python, the program's data files, and the Pyodide loader the
    playground already has, so a program is a shareable link with no server); a typed `Dom` façade in
    the interop cookbook (the first real consumer of the "publish a façade, import many" axis; it hits
    item 16's calling convention and Pyodide's `create_proxy` lifetime, which the façade should hide);
    and a `Promise` to `Async` bridge, which Pyodide already performs and the types only need to say
    (`-> Async a` on an extern over a JS async API). Signalling for a peer-to-peer transport is the
    game's problem, not Pyfun's.

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
- **A `spawn` effect label discharged only by a concurrency scope** (fourth dogfooding session,
  item 17): `Task.start` would perform `spawn` and only a `Task.scope` handles it, so a start outside
  a scope is "performs `spawn`" with nothing to discharge it. Pyfun's first effect *handler*, and the
  reason structured concurrency belongs in the language rather than a library; the value form
  (`Scope` as a capability argument) ships first and covers the use.
- **Larger prelude / package manager** — the *prelude* half is superseded by Dogfooding findings #5
  (complete the surface in one sweep; "on demand" is what accumulated that backlog). The package/façade story (publish typed extern façades once, `import` many) is a whole axis that
  waits for actual users. A future Python-side runtime package could default to `uv`. (Macros are a
  non-goal, below — not part of this bucket.) (Decode specialization shipped 2026-07-13 — `DESIGN.md`
  §5.3: statically-known decoders deforest to direct dict/list access, byte-identical `Result`s, 2.8x
  measured on a decode-dominated workload; dynamic shapes (`andThen`, decoder-as-value) keep the
  interpreter.)
- ~~Module-alias shadowing~~ **CLOSED 2026-07-27** — `import Ids` + any same-named binder (top-level
  `let`, parameter, block `let` anywhere in the function, lambda parameter, match-pattern capture at
  any level, native-CE binder) now emits `import ids as _pf_ids` at the affected sites
  (`lowering::py_module_ref` consults `user_defs` + `module_binders` + the `fn_local_stack` scope
  frames; plain and aliased imports coexist, so un-collided sites keep readable output). No known
  residual; the per-shape regression test is
  `tests/project.rs::local_binders_colliding_with_a_module_alias_also_get_the_mangled_import`.

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
  all-round *stock* host. **PyPy TESTED 2026-07-18 — the best host measured for emitted Pyfun**
  (7.3.23/3.11.15, docker `pypy:3.11`; artifacts `local/pypy-verification/`): the **`--target 3.11`
  switch SHIPPED** the same day (`src/python_emitter/py311.rs` — PEP 701-dependent f-strings rewrite
  to `"…".format(…)` calls, exact check on rendered holes, everything else Pyfun emits is
  3.10-compatible; `bench/run.py --target 3.11` compiles into `bench/out-3.11/`). Results, cold:
  emitted code runs **1.5–3.6× faster than CPython 3.14** (expr_eval 3.6×, map_build 2.7×, collatz
  1.5×), outputs byte-identical; the GraalPy ADT pathology does **not** transfer (steady ~0.4s/iter
  on the probe), and cold PyPy even beats the mypyc-compiled figure on expr_eval (0.525s vs 0.824s)
  with zero user toolchain. Weak spot: recursion (collatz 7.36× vs PyPy's own iterative baseline —
  absolute time still beats CPython). Docs line earned: "compute-bound? `--target 3.11` + PyPy."
  **CPython's own JIT** (experimental since 3.13) accrues to every program for free.
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
  rewrite alone), so the `if`/`isinstance` lowering mypyc forces is also a lever on its own. **It
  shipped in the default emitter for `Option`/`Result` scrutinees on 2026-08-30** (`DESIGN.md`
  §5.5, issues #87/#89): a real workload (a Scrabble move generator asking `Map.tryFind`/`List.findIndex`
  millions of times per position) demanded it, and for a two-case type with one payload the ladder
  *is* the readable form. User ADTs keep `match`/`case`, so native mode still needs the general
  `if`/`elif` lowering; (2) frozen-dataclass ADTs compiled
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

- **PyPI** — `pyfun-lang` at **0.7.0** (2026-08-30, published by the tag and verified by installing
  it into a clean venv and running the five 0.7.0 miscompile repros against it; the index lags the
  tag build by a few minutes, so a first `No matching distribution` is a retry, not a failure).
  Rides the tag; no manual step.
- **Open VSX** — DONE, **accepted**: `pyfun.pyfun` covers VSCodium/code-server/Gitpod/Theia.
  At **0.7.0** (2026-08-30, verified against the registry API; indexing lags the publish by a minute
  or two, so check twice before believing it failed). Every release:
  `ovsx publish <vsix> -p <token>` (scriptable, no moderation).
- **JetBrains Marketplace** — DONE, **accepted**: plugin `com.github.simontreanor.pyfun` (id
  32915) is live (`editors/jetbrains/`, thin: file type + TextMate grammar + LSP4IJ wiring, free
  mode + legacy CE, 2024.2+). At **0.7.0**, uploaded 2026-08-30 and **awaiting moderation**; the
  plugins API reports 0.6.0, so that one cleared (verified against the plugins API; an earlier
  "0.4.0 is not uploaded yet" note here was stale, as an "awaiting moderation" one had been before
  it — check the API, not this line). `editors/jetbrains/` has a
  **committed Gradle wrapper**: `./gradlew publishPlugin` (JDK 21 + `JETBRAINS_PERMANENT_TOKEN`)
  needs nothing installed, because Gradle is *not* on this machine and two releases running lost
  time rediscovering that before reaching for a bare `gradle`. Approval is not instant: the plugins
  API lists approved versions only, so it reads the previous one until moderation clears.
- **VS Code Marketplace** — accepted and live as `pyfun.pyfun`, at **0.6.0**; the **0.7.0 vsix is
  packaged and attached to the release but not yet uploaded** (2026-08-30, gallery API still reads
  0.6.0). Verification takes minutes, during which the API keeps reading the
  previous version and the publisher UI shows the pending one as "Verifying" — a lag, not a failed
  upload. The **only surface that cannot be scripted**: the vsix is uploaded by hand through
  the publisher web UI at `https://marketplace.visualstudio.com/manage/publishers/pyfun` (the CLI
  auth path is broken — see `editors/vscode/DEVELOPMENT.md` and RELEASING.md). It is therefore the
  one that silently falls behind; check it whenever a release goes out.
- **Third-party registries — PARKED until there is adoption evidence** (decided 2026-07-31). The
  surfaces Pyfun controls (PyPI, VS Code Marketplace, Open VSX, JetBrains) are the ones that get
  kept current every release; these do not, and their status is deliberately *not* re-checked each
  time. Two of them already told us the same thing in different words, which is what makes the rule
  rather than the exception. The rule is about *adoption gates*: a registry that closed on a process
  rule with a stated way back in is worth finishing when that condition is met, which is why MELPA
  below was resubmitted while the star-gated ones stay parked.
  - **nvim-lspconfig** (#4476, closed) and **Mason** (#16012, withdrawn — its path was lspconfig
    approval): new languages need adoption evidence, informally ~100 stars.
  - **MELPA** `pyfun-mode` (melpa/melpa#10094, closed 2026-07-19): *not* a rejection of the recipe,
    which they had already signed off; they require the Emacs package to live in a public repository
    for **one month or more** and `pyfun-mode.el` was five days old. The one-month gate passed on 2026-08-14
    (`pyfun-mode.el` public since 2026-07-14), and **resubmitted 2026-08-29 as melpa/melpa#10189**,
    recipe unchanged. GitHub refuses to reopen #10094, so a fresh PR from a branch off current
    `master` is the way back in. Their closing note also carried a *preference*, not a requirement:
    MELPA would rather a package not live in a monorepo, because their build machinery has to pull
    the whole thing. **Decided 2026-08-29 to stay in the monorepo** and say so in the PR: a full bare
    clone is 3.3 MB, the mode's version is bumped in lockstep with the compiler by `RELEASING.md`, and
    a separate repository would mean two places to keep in step for one file plus a reset of the very
    soak time the submission was waiting on. Split it out only if MELPA asks.
  - **Zed** (zed-industries/extensions#6814 — the main repo as a submodule at `editors/zed`, with
    the LICENSE the registry wanted inside the extension dir) had changes requested on 2026-08-10:
    the submodule pointed at a branch commit that stopped being reachable once that branch was
    squash-merged and deleted. Repointed at the v0.6.0 commit on 2026-08-29; checks green, awaiting
    re-review. Two traps a future pin bump walks straight into, both learned the hard way that day:
    **squash-merging deletes the commit a submodule pin names**, so a pin must always be a commit on
    `main` and wants rechecking whenever it is bumped; and **moving the pin moves every file inside
    it**, including `editors/zed/extension.toml`, whose `version` their `package-extensions` script
    checks against the `version` in *their* `extensions.toml`. The repoint carried the extension from
    0.1.0 to 0.2.0 and failed with `Incorrect version for extension pyfun`, fixed by bumping their
    entry to match. **Bump both numbers together.** Note the Zed extension is versioned on the
    grammar's schedule, not the compiler's (`RELEASING.md`), so the two are not the same number and
    the pairing has to be checked rather than assumed. When reading that failure, the loud
    `could not find Cargo.toml` error in the log is **benign**: it appears identically in the July run
    that passed, and the real error sits ten lines below it. **Helix**
    (helix-editor/helix#16036 — languages.toml, git/rev/subpath grammar, Helix-scope queries, their
    checks clean locally) is still open and needs nothing from us; if it merges, it merges.
  - **nvim-treesitter**: upstream ARCHIVED 2026-04 with no successor (candidates: the
    neovim-treesitter fork org, or parser management in Neovim core — neovim/neovim#39006). A fully
    validated branch is parked at `simontreanor/nvim-treesitter` (`add-pyfun`), ready to retarget
    when the ecosystem settles.
  - **Sublime Text Package Control** and a **Pygments lexer** on PyPI (the kernel declares the
    `fsharp` lexer as an approximation) were always demand-gated and stay that way.

  The documented fallback for every one of these already exists in `editors/README.md`, so a user
  on any of those editors is not blocked — they install by hand instead of by registry. Revisit the
  whole list when download or install numbers give the maintainers something to say yes to, rather
  than re-litigating each one per release.

## Docs & education site (live at simontreanor.github.io/Pyfun — what remains)

The mdBook site shipped 2026-07-15 (learner track, educator pack, internals tour, in-page runnable
code blocks; the playground moved to `/playground/` with `#code=` permalinks). Teaching prose is
CC BY 4.0. When lessons change, re-verify with `python docs/verify_lessons.py` (checks every deep
link decodes to its displayed starter and every solution's output matches); `ci.yml` runs it on
every PR, and it refuses a `target/debug` binary older than `Cargo.toml` rather than reporting
green against a stale compiler. Still open:

- **Notebook-format lessons** (M, demand-gated) — the same lessons as `.ipynb` files riding the
  shipped Jupyter kernel, so instructors can distribute them through existing course
  infrastructure. Wait for an educator to ask.
- **CONTRIBUTING.md + curated good-first-issues** (S) — point new contributors at the internals
  tour's "Where you would add..." notes; label a handful of well-scoped issues.
- **Printable educator pack** (S, demand-gated) — a PDF export of the five session docs for
  departments that circulate paper.

## Two surface gaps the 0.5.0 documentation audit found (2026-08-02)

Both surfaced while checking what the docs claim against what the compiler does, and neither is a
documentation problem, so they are recorded here rather than papered over in prose.

1. **An uppercase `let` binding silently defines a function** (S) — `let Some x = Some 1`
   type-checks. `parser::parse_binding_target` enters the pattern grammar only after `(` or
   `Ident {`, so a bare constructor name is read as the *function name* of `let f x = …`, and the
   program defines a function called `Some` that shadows the constructor. The irrefutability rule
   that exists for exactly this case (`refutable_shape`, which does reject `let (Some x) = …`) never
   sees it. Nothing downstream can use such a name as a constructor, so the shape is a mistake
   every time it is written. Fix: reject an uppercase-initial binding name, with a message pointing
   at the parenthesized pattern form when a constructor pattern was plainly intended. Lesson 8 had
   to work around it (it quotes the parenthesized spelling), which is how it was found.
2. **The hole-fit shortlist can hide the answer it exists to name** (S–M) — `hole_fits` ranks by
   generality, then by qualified-vs-bare, then **by name**, and truncates at `HOLE_FIT_CAP = 6`. For
   a common shape like `string -> string` the stdlib sweep left far more than six equally specific
   fits, so the tail is decided alphabetically: `String.upper` now falls off the end of a
   `string -> string` hole while `String.trimStart` stays. That is arbitrary from the reader's side,
   and it cost lesson 9 its worked example. Options, in increasing effort: rank the remaining tier
   by *shortest name* or by prelude-declaration order rather than alphabetically; say "and N more"
   when the list is truncated; or filter by the hole's own name against candidate names (`?upper`
   plainly wants `upper`), which is the one that would have kept the lesson working.

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
  **Partially reopened 2026-07-31**: the combinators answer holds for collection traversal and does not
  cover an unbounded interactive loop (Dogfooding findings #6). General and mutual TCO stay out; a
  direct, saturated *self* tail call is accepted work as a lowering-only transform (`while True` plus
  parameter rebinding), which is a change to emitted code, not to the language.
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
  `let mut` is for local accumulation inside an expression, not to drive a loop. (The interactive-loop
  gap this leaves is Dogfooding findings #6, where reopening `while` is option (c) of three.)
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
