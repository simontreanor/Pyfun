# Pyfun — Design

The language and semantics reference, and the source of truth for any compiler or language change.
`RATIONALE.md` is the short narrative for a reader deciding whether Pyfun is worth their time; this
file is the detailed design.
It describes the language as it stands: read it before changing semantics, and update it when a
design decision changes. What landed when is in git history, not here.

## 1. Identity

**An F#-inspired, functional-first language for the Python ecosystem.** Pyfun takes the role F#
plays on .NET — the typed, functional-first sibling to a dominant imperative language — and brings
it to Python, interoperating with the ecosystem much as F# does with C#:

- **Shared runtime + ecosystem** — runs on CPython, imports Python libraries directly; Python can
  consume Pyfun-compiled modules.
- **Different philosophy** — immutable-by-default, expression-oriented, type-rich (inference +
  ADTs + exhaustive matching + effect tracking), where Python is mutable, statement-oriented,
  dynamic.
- **Rust compiler** — language-tooling-grade front end, shipped as a standalone binary.

Mechanically Pyfun is a *transpiler* — the TypeScript-to-JavaScript relationship — not a co-equal
language on a shared VM: it compiles *to* Python source (F# does not compile to C#). The F#/C#
analogy is one of *role and philosophy*, not architecture.

**Prior work & positioning.** Pyfun enters a populated space. `RATIONALE.md` explains the design
decisions and names the languages Pyfun borrows from; `README.md` sets it beside Fable, Erg, and
Coconut feature by feature. In short, Pyfun does not out-feature Fable's F#, and its
bet is a narrower one: an ML-family, FP-first language with mandatory static checking (HM inference,
enforced exhaustiveness, inferred effects, units) that compiles to self-contained, idiomatic Python
(a `List` *is* a `list`) with no runtime library and no .NET. Architecturally, **Hy** is the closest
lowering precedent (source → Python AST), though it changes syntax, not philosophy, so Pyfun's hard
parts are semantic design and interop, not parsing. (Related but distinct: Cython = Python→C;
ty/mypy/pyright = type checkers; RustPython = a Python interpreter in Rust.)

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

- **Type safety** — Hindley–Milner inference (no annotations required). *Optional* type annotations
  (`let x : T`, `(x: T)`) are a **non-goal** (see ROADMAP non-goals): HM inference is complete so the
  compiler needs none, their strongest concrete driver (lifting the field-name uniqueness restriction)
  is handled by the use-site multimap *without* annotations, and annotation-free code is the point —
  `extern` (§6) is the one place types are asked for on purpose. Everything is inferred and surfaced by
  LSP hover / `pyfun check` / REPL `:type`. Sole revisit trigger: error localization under pure inference
  becoming a real recurring pain — answered first by better HM diagnostics, not syntax.
- **Exhaustive pattern matching** — all ADT variants must be handled.
- **Immutable-by-default** — `let` is immutable; `<-` reassignment of a non-`mut`
  binding is a compile error; `let mut` is the explicit opt-in. `mut` bindings are monomorphic and
  cannot take parameters. Reassignment requires statement **sequencing**, which Pyfun gets from
  indentation **blocks** (an indented `let … =` body); see §7's offside note. A closure that
  reassigns a `mut` captured from an enclosing scope lowers with a `nonlocal` (enclosing function) or
  `global` (module-level) declaration — Python otherwise treats the assigned name as a fresh local and
  the closure would miscompile. A CE body (`async { }`, `seq { }`, `result { }`, `option { }`) lowers
  to its own nested function, so a reassignment inside one gets the same declaration — the coroutine
  is a closure over the flag it flips. This mirrors F# 4.0, which auto-boxes a captured mutable into a
  reference cell (Python's `nonlocal`/cell mechanism is the same idea).
- **Effect discipline** — first-class (see §4).

Example diagnostics the compiler must produce (rustc-style, with spans, codes, and `help` notes):
type mismatch (`add "hello" 5`), non-exhaustive `match` (missing `None` case), reassignment of an
immutable binding.

**Warnings.** An error is a program the compiler will not build. A warning is a program it builds
while something in it does nothing, which is very likely not what its author meant. A warning never
stops a build. The set is small and each member is certain, since a warning that is sometimes wrong
teaches people to stop reading them:

- **A `case` that can never match**: the arms above it already take every value it would. It is
  the same usefulness question exhaustiveness asks (§7.2), put to each arm against the unguarded
  arms before it, so it sees through constructors, tuples, records, lists and or-patterns
  (`(True, True)` after `(True, _)`, `Some 3` after `Some x`, `_` after every constructor). A
  guarded arm above takes nothing for certain. A match over an active pattern, or with a list
  pattern that has elements after its star, is not judged.
- **A local `let` nothing reads**, in a block or a computation expression. A name beginning with
  `_` says the binding is unused on purpose. Parameters, pattern variables and loop targets are not
  reported, because they are often there for the shape and not for the name; neither is a top-level
  `let`, which another module may import. A program with a typed hole is unfinished on purpose, and
  the unused name is very likely what the hole is waiting for, so this warning waits until the holes
  are filled.
- **An `import` nothing uses.** A use is a `Module.member` anywhere (an expression, a pattern, a
  record tag, a type, a computation expression's builder) and also what crosses a module boundary
  without a qualifier: a type named bare, a record's field read by name. A module that exports
  measures is never reported, since a unit carries no mark of where it came from.
- **An `extern import` no `extern` in the file is rooted at.**
- **A top-level `let` that binds a name an earlier one bound.** The later one replaces it for the
  rest of the file.

An unused `extern` is deliberately not on the list: a module of `extern` declarations is a façade
written for other modules to import.

**Where they show.** `pyfun check` prints them and still succeeds (`ok: no type errors (2
warnings)`); `pyfun check --strict` makes any warning a failure, for a CI job. `pyfun run` and
`pyfun compile` print them on stderr beside the program they build. The language server publishes
them at `Warning` severity, tagging code that does nothing as `Unnecessary` so an editor fades it.
The playground and the lessons' runner list them with the output. The REPL and the notebook kernel
show the two kinds that are about one entry alone (an unreachable arm, an unused local): a session
binds a name again and imports ahead of use as it goes. Warnings are shown only for a file that
parses.

## 4. Effect system — first-class MVP goal
Unlike F# (which has no real effect system, only computation expressions), Pyfun treats **purity
and effects as part of the type system from the MVP.** This is a defining feature, not a
later add-on, and it shapes inference and lowering — so it must be designed in from the start.

**Implemented (inference-first, zero pollution).** Function arrows (`Ty::Fun`) carry a latent
[`Effect`] — a **set of concrete labels** (`EffLabel`: `io` — printing, observable mutation via `<-` —,
`async`, `spawn`, which `Task.start` performs and only a `Task.scope` discharges (§8), and `exn`, which
an `extern` declares and `try` discharges (§6)) plus effect *variables* for polymorphism. Effects are
**inferred and never written in ordinary code**: a pure function reads exactly as before (`let add a b
= a + b`); `print : a ->{io} unit` and impurity **propagate automatically** (calling an impure function
makes you impure), and labels from different calls **union** (a body that prints and fetches is `->{io,
async}`). Defining a function is pure — its body's effect is the *latent* effect on its innermost
arrow. Effect variables generalize/instantiate alongside type/unit/num variables, so higher-order
functions stay effect-polymorphic (`let pure apply f x = f x` is pure *up to its argument*: `apply
print` is impure at the call site, `apply` itself is not). The one **opt-in, definition-level**
assertion is `let pure f … = …`, which is a compile error if the binding introduces *any* concrete
label (the violation names the set: "performs `io, async`"). Effects are **fully erased at lowering**
(zero runtime residue, like units); `pure` produces no Python. The sources beyond `print` are `<-` (§3)
and the Python FFI boundary — a plain `extern` is effectful-by-default (§6), the third `io` source.
Display is **canonical and deterministic**: labels render in a fixed order, `io` first (`->{io}`,
`->{async}`, `->{io, async}`); a pure or purely-polymorphic arrow stays the familiar `->`.

**Effect annotations on declared arrows.** Function arrows in *declared* types — `type`
declarations (ADT ctor / record field types) and `extern` signatures — may carry an explicit
annotation `->{label, …}` (e.g. `type Handler = H (str ->{io} unit)`, `extern fetch : str
->{async} Async str = client.get`). A bare `->` stays pure; an unknown label is a compile error. This is
the *declaration-side* exception to "never written": ordinary code remains annotation-free. For an
`extern`, an annotation on the **innermost** arrow is trusted as written and replaces the
`io`-by-default boundary rule (that's how an async client binds as `->{async}` rather than `->{io}`);
an annotation elsewhere (say on a higher-order *argument* arrow) does not suppress the default — the
extern still calls Python. An `->{async}` extern must return the `Async` *type*: an async Python
function returns a coroutine, and the only thing the lowering awaits is an `Async` value bound with
`let!`/`do!` inside an `async { }` block, so `str ->{async} str` would create a coroutine and
drop it (issue #104). The checker rejects that declaration and names the working spelling; the
label stays an inferred, checkable fact and the type is what the emitter acts on. Effect-directed
lowering (emit `await` wherever the label says so) was considered and turned down: an
`async`-effect function passed to `List.map` or any effect-polymorphic combinator would carry the
label onto a call that cannot await, and the rule needed to forbid that is a checker concept F#
avoided by making async a type. Note declared effects are **exact** (no sub-effecting): a *pure* function
does not satisfy a declared `->{io}` parameter, because two closed effect sets unify only when equal.
Effect subsumption (pure ≤ io) is a **non-goal** (ROADMAP): sound subsumption is directional, so it
would thread polarity through the symmetric unifier and risk the `let pure` guarantee on a variance
slip. Where a declared arrow must accept any effect (a boundary callback), the HM-native answer is an
**effect variable, extern-only**: in an `extern` signature a lowercase `->{…}` name
that is not a known label is a variable (`extern each : (a ->{e} unit) -> List a ->{io, e} unit`),
collected like type variables (`collect_effect_vars`), generalized in the extern's scheme
(`Scheme::eff_vars`), and instantiated fresh per use site — exactly the stdlib's effect-polymorphic
`map`/`fold` mechanics. A pure and a printing callback now both flow; the callback's effect rides the
variable into the innermost `{io, e}`, so `let pure` still rejects an impure pipeline. A `->{e}` on
the innermost arrow counts as annotated (skips the io-by-default rule, asserting "exactly the
callback's effect"). In a `type` declaration effect variables remain an error — ADT/record fields
would need effect-*parameterized* types, which stays out of scope. A callback arrow written without a
variable is closed, and an `async { }` handler (which performs `async`) never fits one, so an argument
whose arrow meets a closed declared arrow is reported at the argument with both effects and the
`->{e}` fix (`` `handle` performs {io, async}, but `serve` takes this callback as `Writer -> Async
unit`, which performs nothing; … `Reader -> Writer ->{e} Async unit` ``, #180), not the inferred set,
which would accept only callbacks performing exactly that.

**A local accumulator is not an effect.** `x <- e` performs `io` when the state it changes can be
seen from outside, and a `let mut` assigned in the body of the function that declares it cannot:
every call gets a fresh one, and nothing can change it once the call has returned. So the
accumulator style the language offers (§3) leaves its function pure, and `let pure` accepts it:

```
let pure sum_to n =
  let mut acc = 0
  acc <- acc + n
  acc
```

The rule is by frame, and every other assignment keeps its `io`. A closure that assigns its
enclosing function's local runs in a deeper frame, during some other call, so the mutation
outlives or interleaves with its caller: the closure's own arrow carries `io`, which leaves the
function that only *builds* it pure and makes the one that *calls* it impure. A computation
expression's body counts as a frame too (it lowers to a nested `def`, and an `async` or `seq` body
may run later, or more than once). A module-level `let mut` is shared state wherever it is assigned.
The checker implements this with a function depth (`Infer::fn_depth`) and the depth at which each
`let mut` was declared (`Scheme::mut_frame`): an assignment is local exactly when the two are equal
and non-zero, which is sound because the functions enclosing any program point have distinct depths.

The rule has one cost, and it follows from declared effects being exact (above). A function that was
`io` *only* through its own accumulator is now pure, so it no longer fits an arrow declared
`->{io}` in an `extern` or a `type`, where before 0.11 it did. The existing message for a closed
callback arrow says so and names the way out: the effect variable `->{e}` for an `extern`, and, for a
`type` (which cannot take one), declaring the arrow with the effect its functions really have. A
callback that prints, or assigns state it does not own, is `io` as before and is unaffected.

**Why inference-first (the chosen model).** Effects follow Koka/Flix/Unison — **inferred, never
written in ordinary code** — rather than effects-as-values (Haskell `IO`: `do`/`<-`/wrapper types) or
effects-as-keywords (Rust/Python `async` *coloring*, the very pain we avoid). The Python gradual-typing
mindset: tooling reports the property, the source stays clean. This is why the only surface syntax is
the opt-in, definition-level `let pure` assertion — never expression-body pollution.

`async {}` now **produces** the `async` label: an async block performs `async` at its lexical site,
so a function whose body is an async block has an `->{async}` arrow and a `let pure` binding wrapping
one is rejected (the label was already representable, annotatable via `->{async}` externs, and
inferrable by propagation; the CE now contributes it too). Still open: the exact discharge story
(is `io` terminal until a runtime boundary?); effect subsumption is a **non-goal** (declared effects
stay exact — see above). Effect annotations in declared function types are **done** (`->{label, …}`), as is surfacing
inferred effects in hover output — the LSP (§9) shows `->{io}` / `->{io, async}` on arrows when you
hover an expression or binding name.

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
Python `match` (3.10+), or an `isinstance` ladder for the built-in two-case types (§5.5), or an
if-chain for active patterns; `x |> f |> g` → `g(f(x))` (the pipe is pure parse/lowering-time
sugar, no runtime cost).

**Currying lowering (curried in the type system, n-ary in the output).** Functions are curried by
default (§7), but naive currying would emit `add(1)(2)` everywhere — unreadable and slow. Because
arities are known statically, the compiler **collapses fully-applied calls to a direct n-ary Python
call** (`add(1, 2)`) and **only synthesizes a closure** (`functools.partial` or a small curry shim)
for a *genuine* partial application (`add 1`). This keeps emitted Python idiomatic (§ goal) and
avoids per-application closure allocation — the same optimization F# performs at the IL level.

A partially applied **lambda** goes one better and closes over the argument rather than being wrapped.
Operator sections lower to lambdas, so `List.map ((+) 2)` emits `lambda b: 2 + b`, not
`functools.partial(lambda a, b: a + b, 2)` — one fewer call layer, no `functools` pulled in for this
alone, and the idiomatic spelling stops emitting worse Python than the `fun y -> y + 2` it stands in
for. The wrapper stays wherever folding would change meaning: an argument that is not a name or a
literal keeps it, since `functools.partial` evaluates its arguments once and *now* while a lambda body
evaluates them on every call; and an argument whose name is also one of the remaining parameters keeps
it, since substituting would capture (`((+) b)` must not become `lambda b: b + b`). A fully applied
section is unchanged.

**Representation contracts.** ADTs, records, tuples, options/results, and curried/partially-
applied functions each need a *stable* Python representation. That representation is a public
contract — emitted code and interop both depend on it — so changing it is a breaking change, not
an implementation detail. The contract today: every ADT variant, record and prelude class is a
frozen, **slotted** dataclass (`@dataclass(frozen=True, slots=True)`), so an instance holds exactly
its fields and has no `__dict__`; a field-less variant is additionally backed by one module-level
instance (§5.5) that every use of the constructor as a value loads, though constructing through the
class by hand stays valid and equal to it.

**Names Python already owns.** A Pyfun name is emitted as itself, with one exception: the emitted
module is a Python namespace the compiler is also using. A binding whose name is a Python **keyword**
cannot be emitted at all (`lambda = 1` is a `SyntaxError`), and one that claims a **builtin the
emitter calls** shadows it at module scope, so an emitted `set([…])` or `len(xs)` would find the
user's value instead — a runtime `TypeError` far from the binding that caused it. Such names
therefore gain a trailing underscore in the output (`set` → `set_`, `lambda` → `lambda_`), the same
dodge that has always applied to the constructor `None` → `None_`, and PEP 8's own convention. The
rule is a pure function of the name, so a definition and every reference to it — including across a
module boundary — agree with no coordination. Record *fields* only dodge keywords: an attribute lives
in its object's namespace, so `q.set` needs nothing. Imported Python modules are a moving target
rather than a fixed list, so an `import` shadowed by a binding is **aliased** instead
(`import math as _pf_math`), leaving the user's `math` binding untouched. Programs that claim none of
these names — nearly all of them — emit byte-identical output either way.

**Arm-scoped captures and nested-block `let`s.** A match arm's capture (`case Some pair: …`) is
scoped to its arm, and a `let` inside a nested block (an `if` branch, an arm body) is scoped to that
block. The Python either lowers to (`case Some(pair):`, `pair = o._0` in an `isinstance` ladder, or
a plain `x = 10` in the branch) is an assignment to a local of the enclosing function, and Python
locals are function-wide: one slot per name per function. Left alone, a capture or nested `let` that
reuses a name the function also uses for something else (a block-local `def`, a parameter, an outer
`let`, a global it reads later) would rebind that slot for the rest of the function, and a later read
would see the inner value where the program meant the original binding. So the compiler guarantees
that neither ever rebinds another binding of the same name in the emitted function, and the rule is
liveness: when some reference to the name outside the arm or block resolves to a binding that is
live across it (a parameter, a `let` at the root of the function body, a global or builtin, a
binding of an enclosing function, or an arm or block that encloses this one), the binder is emitted
as `_name` (`case Some(_pair): return _pair`; `_x = 10`, bumped to `_name2` if that is taken) and
every reference to it follows; otherwise the name is emitted as itself. A reference bound by a
*disjoint* arm or block reads its own binding and does not force the rename, whether the two are
sibling arms of one match or arms of two matches in sequence, so the common `case Ok v: … case
Error why: …` keeps its names however many matches in a function spell it. The one exception is a
reference inside a nested closure, which always counts, since a closure can outlive its arm: a
lambda made in one arm that reads `why` forces a later arm capturing `why` to rename. A `let` at
the root of a function body has its own rule, because Python makes a name local to the whole `def`
the moment anything in the `def` assigns it: such a `let` is renamed only when some read of the
name in that function means a binding *outside* it (an enclosing function's, a module-level one, a
builtin) and sits before the `let` or in its own value or in a closure made before it, where Python
would raise `UnboundLocalError` or read the wrong slot. Rebinding a parameter or an earlier `let` of
the same function in sequence is exactly what Python does too, so that is emitted as itself, and a
top-level `let` is a global and never renames. A computation expression's body is its own Python
function, and its `let`/`let!` targets are root-level binders of it under the same rule: a
`let! n = …` in an `option { }` that had already read an outer `n` is emitted as `_n`. A nested `let x = x + 1` reads the outer `x` in its
value and binds the fresh name, as `_x = x + 1`. A renamed nested function is defined under its fresh name and recurses under
it, and a renamed `let mut` is assigned under it, including from a closure's `nonlocal`. Mechanics
in `INTERNALS.md`.

### 5.1 In-place linear accumulation (`Seq.fold`/`List.fold`)

Because the collections are immutable-style (every operation returns a fresh container, §6), building a
collection inside a fold by repeated `Map.add`/`List.concat`/`Set.add` is nominally O(n²): each
step copies the whole accumulator. Pyfun runs the idiomatic fold-with-`Map.add` construction in
**linear time** anyway: a `Seq.fold`/`List.fold` that threads its accumulator linearly and only grows it
lowers to an in-place `for`-loop over a mutable accumulator (`Map.add`—`m[k]=v`, and so on). This is
**sound because Pyfun exposes no object identity**: mutate-in-place and fresh-copy-each-step are
indistinguishable unless a retained reference to the accumulator escapes, and the pass conservatively
refuses whenever it cannot prove none does. The output is byte-identical to the naive lowering.
Mechanics and the full precondition set are in `INTERNALS.md`.

### 5.2 Inlining fully-applied pure stdlib predicates

A pure one-liner stdlib member that is **fully applied** lowers to the Python idiom directly rather than
through its `_pf_*` helper: `String.contains n s`—`n in s`, `List`/`Set`/`Map.contains`—`x in xs`,
`String.starts_with`/`ends_with`—`s.startswith(…)`/`s.endswith(…)`, `List.is_empty xs`—`not xs`. The
inline form is value-identical to the helper. A partial application (`String.contains "x"`) keeps the
helper, so `List.map (String.contains "x") xs` still works. This is a readability win first; mechanics
in `INTERNALS.md`.

The same lever covers two call-site shapes where the idiomatic Pyfun spelling and the fastest Python
answer differ by more than a function call.

**Equality predicates.** `List.find_index ((==) x) xs` is the way to ask "where is `x`", and Python
answers that in C with `list.index`. A fully-applied `List.find_index`, `List.find`, or `List.exists`
whose predicate is the section `(==) x` (or the lambda `fun y -> x == y` / `fun y -> y == x`, with `y`
not free in `x`) lowers to that scan: `_pf_index_of(x, xs)` (`xs.index(x)` under a `try`),
`_pf_find_eq(x, xs)` (`xs[xs.index(x)]`, so the *element* found is returned, as the generic helper
returns it), and `x in xs`. `x` is evaluated once, before `xs`, exactly as the helper call evaluated
its arguments. Any other predicate, and any partial application, keeps the generic Python-level scan.

**Match-consumed lookups.** `match Map.find k m:` with the arms `Some v` and `None` is the language's
way to say "look it up, and here is what to do in each case", and the `Option` in the middle exists only
to carry the answer from the helper to the `match`. When the two are adjacent it need not exist: the
match lowers to `if k in m:` with `v` bound from `m[k]` in the first branch and the `None` arm in the
`else`, in return and in value position alike. `List.head xs` (`if xs:`, `xs[0]`) and `List.get i xs`
(`if 0 <= i < len(xs):`, `xs[i]`, the helper's own bounds test) take the same route. The operands are
bound to temps unless they are names or literals, so each is evaluated once. The rewrite needs exactly
the two arms, unguarded, with a payload pattern a plain assignment can bind (a name, `_`, or a tuple of
those); anything else, including the `Option` escaping into a variable, keeps the helper and the
`match`.

### 5.3 Specializing statically-known `Decode` decoders

A `Decode.decode_string` whose decoder is a **syntactically-known composition** of the simple combinators
compiles to direct dict/list access with inline error handling — a validating parser that reads like
hand-written Python — instead of building and interpreting a runtime decoder value. The resulting
`Result` is **byte-identical** to the interpreter's on every input (same `Ok` payload, same error kind
and message). Anything dynamic (`and_then`, a decoder passed as a value, a non-literal `one_of`) falls
back to the interpreter. Measured ~2.8x end-to-end on a decode-dominated workload. Mechanics in
`INTERNALS.md`.

### 5.4 Self tail calls become loops

A function that calls **itself** in tail position lowers to a `while True:` that rebinds the
parameters and goes round again, rather than to a recursive call:

```python
def turn(state, deck):          # let turn state deck =
    while True:                 #   if over state then state
        if over(state):         #   else turn (step state) (rest deck)
            return state
        state, deck = (step(state), rest(deck))
        continue
```

CPython has no tail-call elimination and caps the stack near 1000 frames, so the recursive form
walks a stack it has no reason to build; an interactive loop written as recursion (every turn and
every rejected input a frame that never returns) is the shape that forced the issue. Several
parameters rebind **simultaneously** from one tuple, so an argument still reads the previous
iteration's values, exactly as the call it replaces evaluated its arguments before entering.

The scope is deliberately one shape: a **direct, saturated** call to the enclosing function in tail
position. General tail-call elimination and mutual recursion need a trampoline, which costs the
readable output lowering exists to protect; both stay non-goals (`ROADMAP.md`). Nothing about the
language changes — this is a property of the emitted code, and the recursive definition remains the
only way to write it.

The **async form** is the same rewrite one wrapper in. `let f a = async { … }` lowers to
`def f(a): async def g(): …; return _pf_Async(g)`, and its self tail call is `return await f(…)`
inside `g` —
each message of an agent loop then awaits a fresh coroutine *on the same stack*, so CPython's limit
is the loop's lifetime cap (measured: ~1,000 messages), which is fatal for exactly the thing an
agent is written to be: long-lived. When a body ends in that wrapper shape, `g` takes the
parameters as its own (`async def g(a): while True: …`, returned as `_pf_Async(g, a)`), and the
awaited tail calls rebind them and go round the loop inside the one coroutine. Because an `Async` is
cold (§8), every await of `f(…)` starts again from the arguments it was built with, and no
`nonlocal` is needed. For the tail call to be *visible*,
`return!` of a `match`/`if` in an `async { }` block lowers with a return per arm (`return await
loop(inbox, n)` in the arm) rather than assigning a temp and awaiting it once — the same descent a
sync function's tail `match` gets. The preconditions below apply to `g`'s body unchanged; the
`heard`-style helper that *computes* the next state stays sync (`case Said s: loop inbox (heard
count s)`), because a helper that returns the next `Async` instead would be mutual recursion, which
stays out of scope.

Four preconditions, each of which leaves the ordinary recursive def when it fails (always correct,
merely stack-bound): the body must not **rebind the function's own name** (a shadowing `let` would
make the loop jump somewhere the program meant to call); it must not be a **generator or coroutine**
(`return` in a generator raises `StopIteration` with a value, and an async tail call is an `await`);
the rewrite does not descend into a **`for` loop** (a `continue` there belongs to the `for`) or a
**`try`** (looping inside it would put every later iteration under a handler that covered one call);
and no **nested function may have one of this frame's names free** in it. That last one is the subtle
one: the loop reuses one cell per parameter where recursion gave each frame its own, so a closure
that outlives its iteration would see the final value rather than the one it was made with. It is
*free* variables, not mentions: a `fun n -> n + 1` inside a function whose parameter is also `n`
binds its own `n` and shares nothing. What it still gives up is a closure that captures but provably
cannot escape (`List.fold (fun acc x -> acc + x * n) 0 xs`, consumed within the iteration); proving
that needs escape analysis per callee, which is not worth the machinery.

Because a rejection is otherwise invisible — the program simply keeps recursing — a function that
*does* call itself in tail position but kept its recursive form emits a **note** saying which
precondition stopped it (`` note: `collect` calls itself in tail position but keeps its recursive
form: a closure in it captures `n` ``). Nothing is reported for the overwhelming majority of
functions, which have no self tail call at all. Mechanics in `INTERNALS.md`.

### 5.5 `Option`/`Result` matches as `isinstance` ladders; nullary constructors as singletons

`Option` is the return type of every stdlib accessor (`Map.find`, `List.head`, `List.find_index`),
so a `match` on one is the commonest match in the language, and a real program asks it millions of
times. A Python `match` resolves a class pattern through `__match_args__` and `isinstance` at
runtime, then binds by position, which is structural machinery a two-case type with one payload does
not need. A match whose arms are the constructors of one built-in family (`Some p`/`None`, or
`Ok p`/`Error e`) with irrefutable payload patterns, optionally ending in a catch-all, therefore
lowers to what a person writes:

```python
def allows_letter(checks, rc, l):      # match Map.find rc checks:
    _pf_t0 = _pf_map_find(rc, checks)
    if isinstance(_pf_t0, None_):     #   case None: True
        return True
    else:                             #   case Some s: Set.contains l s
        s = _pf_t0._0
        return l in s
```

The last arm of an exhaustive match is a plain `else`, and the `raise RuntimeError("non-exhaustive
match")` the `match` lowering keeps as a belt-and-braces guard is dropped: the checker has proved
the arms cover the type, and the ladder's own rule (both constructors unguarded, or an unguarded
catch-all) is the checker's rule for this shape. A guarded arm in return position is its own `if`,
so a failed guard falls through to the arms after it exactly as `case … if …:` does; in value
position a guard would need a matched-flag to do the same, so a guarded value-position match keeps
the `match` lowering. A refutable payload pattern (`case Some 0:`, `case Some (Some x):`) keeps it
too. The `result {}`/`option {}` computation expressions use the same ladder for every `let!`,
forwarding the failure as the value it already is. User ADTs still lower to `match`/`case`: the
readable structural output is the point of the emitter, and only the built-in family is hot enough,
and simple enough, to earn the exception.

A field-less constructor has one possible value, so building a fresh instance at every mention
(`rules.Across()`, `None_()` on every miss) is work for nothing. Each nullary variant, and the
prelude's `None`, is therefore built **once**, right after its class, as a module-level singleton
named after the class with a leading underscore (`_Across = Across()`, `_None_ = None_()`), and
every use of the constructor as a value loads it. Equality is untouched: the dataclass compares by
class and (absent) fields, so `_Across == Across()` holds and a hand-constructed instance matches
`case Across():` as before; what changes is that `d == Across` in a hot loop is now a name load and
an identity check that short-circuits in C. A program that binds the singleton's own name
(`let _Across = …`) keeps the call form for that constructor, so the rewrite can never shadow a
user binding. Mechanics in `INTERNALS.md`.

### 5.6 Native lowering (`--native`): every match as an `if` ladder

`pyfun compile --native` is the first step toward compiling Pyfun output with mypyc
(`ROADMAP.md`, "Typed-emit + mypyc AOT"). mypyc does not compile `match` statements, and every Pyfun
`match` lowers to one, so native mode lowers a match to an `if`/`elif` ladder of plain tests instead,
generalizing §5.5 from the `Option`/`Result` family to every pattern built from constructors,
records, tuples, literals, variables, wildcards and `as`:

```python
    elif isinstance(e, Add):                          # case Add a b:
        a = e._0
        b = e._1
        _pf_t2 = (simplify(a), simplify(b))           #   match (simplify a, simplify b):
        if isinstance(_pf_t2[0], Num) and _pf_t2[0]._0 == 0:
            sb = _pf_t2[1]                            #     case (Num 0, sb): sb
            return sb
        ...
        else:                                         #     case (sa, sb): Add sa sb
            sa = _pf_t2[0]
            sb = _pf_t2[1]
            return Add(sa, sb)
```

A pattern compiles to the tests that decide it (`isinstance` for a constructor or record tag, `==`
for a literal, `is` for a boolean), and-ed in order and short-circuiting before any field is read,
and the assignments that bind its names, which run once the arm is chosen. A newtype pattern erases
to its payload's, as everywhere. The checker has proved the unguarded arms cover the type, so the
last arm, when unguarded, is a plain `else` with no `raise` behind it. Guards follow §5.5: in return
position a guarded arm is its own `if` and a failed guard falls through; in value position a guard
keeps the `match` lowering. A top-level or-pattern becomes one ladder arm per alternative, each with
its own copy of the body, a nested or-pattern that binds no names (`Some (1 | 2)`) becomes one test
joined with `or` (one that binds names keeps `match`), and a list pattern becomes a length
test (`len(xs) == n`, or `>=` with a rest), positional reads with the suffix counted from the end
(`xs[-1]`), and the rest as a slice (`xs[1:len(xs) - 1]`). Active patterns already lower to their
recognizer and an `isinstance` ladder in both modes. The `return!` of a `match` in an `async { }`
block, which lowers through its own path so a self tail call stays visible (§5.4), takes the ladder
too, `Option`/`Result` shapes included; on the dogfooded Scrabble game (11 modules) native output
has no `match` statement left, and every recorded replay prints the same in both modes.

The ladder is also faster on CPython by itself, since a class pattern goes through `__match_args__`
and positional binding at run time: on `bench/expr_eval` (ADT allocation and matching) the output
goes from 2.30x to 1.63x of the hand-written baseline on CPython 3.14, before any compilation.

**Annotations.** Top-level functions, and functions defined inside them, carry annotations taken
from their inferred types where Python can spell them: `int`, `float`, `bool`, `str`, `None`, `list`/`set`/`dict`/`tuple` of those, a
record's class, and a union alias per user sum type (`Expr = Num | Add | Mul | Neg`, emitted after
its classes). A type variable, a function type or an async result stays unannotated, so the
annotations never claim more than the checker proved, and so does a `unit` result, since Pyfun
returns a unit call's value (`return f(x)`), which mypy rejects under `-> None`. Pyfun never
rebinds a parameter through `nonlocal` (the async self tail call loops over the coroutine's own
parameters, §5.4), which matters to mypyc: it copies an annotated `int` parameter into a closure
environment without boxing it, which gcc rejects and an MSVC build turns into a segfault
(python/mypy#22221). Data class fields follow suit: a field declared as a user sum type or record names it (`_0: "Expr"`,
a string because the alias comes later), and any other field without a Python class of its own
is `typing.Any`, since mypyc cannot build a dataclass with an `object` field. The output is
written for mypy's newer redefinition rule (`--allow-redefinition-new --local-partial-types`): a
Python local reused across match arms can hold differently narrowed types in different arms, which
the default first-assignment rule rejects. A
build also turns off three of mypy's error codes that only restate the Python boundary:
`func-returns-value` (Pyfun binds a unit call's result, `_ = f(x)`), and `var-annotated` and
`union-attr` (a module-level value mypy cannot infer, a typeshed union at an `extern` call), where
the `extern` declarations are the contract and Pyfun's checker has already held the program to
them.

**Comparisons.** A sum type the program compares writes `__eq__`, `__lt__`, `__le__`, `__gt__` and
`__ge__` out over its `_pf_order_key`, where the default output lets `@functools.total_ordering`
derive the last three from `__lt__` (§7.1). mypyc compiles `==` between two values of a class that
defines `__lt__` without its own `__eq__` as an identity test, so `Some 1 == Some 1` came out
`false`, and mypy cannot see the methods `total_ordering` derives, so `<=` and `>=` did not
type-check. The dataclass still generates `__hash__`.

**Building.** `pyfun build --native <file> -o <dir>` does the whole trip: it compiles natively, runs
mypyc with those flags, and leaves the extension and a `__main__.py` in `<dir>`, so the program runs
as `python <dir>`. It needs `mypy`, `setuptools` (which a fresh Python 3.12+ venv lacks) and a C
compiler in the environment. A project builds the same
way: every module is annotated from its own types and compiled by mypyc, and the shared
`_pyfun_rt.py` stays Python, imported by the compiled modules like any other. `bench/mypyc_run.py`
builds every benchmark this way and finds the mypyc build 1.3x to 13x faster than the same program
as plain Python; the int-arithmetic benchmark runs about 12x faster than hand-written Python.

Native mode is opt-in and applies to every module of a project alike. The default emitter keeps
`match`/`case`, because it reads as the program was written. Every example and the whole
end-to-end suite produce identical output in both modes.

### 5.7 Typed output (`--typed`): the inferred types as Python annotations

The plain output has the types it needs to run and no more: a `def` with bare parameters, a class whose
generic field is `object`. A Python program that imports it learns nothing from a type checker. `pyfun
compile --typed` writes the types the checker inferred into the same output, as annotations Python's
own type checkers read, and for a project puts the `py.typed` marker (PEP 561) beside the modules so
that the directory is a typed package. The code that runs is the plain output's: take the annotations
away and the two files are the same.

```python
type Shape = Circle | Rect

def area(shape: Shape) -> float: ...
def pair[A, B](x: A, y: B) -> tuple[A, B]: ...
def double[A: (int, float)](x: A) -> A: ...
def first[A](xs: list[A]) -> Some[A] | None_: ...
origin: Point = Point(0, 0)
```

Every top-level function has each parameter and its result annotated, every top-level value its type,
and every class its fields' declared types. The spelling follows what the lowering produces at run
time:

- **A type variable is a type parameter** of the function or class that is generic in it, in Python
  3.12's own syntax (`def pair[A, B]`, `class Box[A]`). A generalized *value* has nowhere to declare
  one, so its variables are `Any` (`nothing: list[Any] = []`).
- **A number a function is polymorphic in is a type parameter held to `int` and `float`** (`[A: (int,
  float)]`), so `double(2)` is an `int` and `double(2.5)` a `float`, as they are in Pyfun. `float`
  alone would accept both and return the wrong one. A number that only comes out is `int`, its default.
- **A sum type is an alias over its constructors' classes**, `type Shape = Circle | Rect`, generic when
  the type is (`type Tree[A] = Leaf | Node[A]`); a constructor's class takes the type's parameters its
  own fields mention. A type whose one constructor has its name is that class. A record is its class.
- **The built-in types are what they lower to:** `List a` is `list[A]`, `Map k v` is `dict[K, V]`, `Set
  a` is `set[A]`, a tuple is `tuple[...]`, `Option a` is `Some[A] | None_` and `Result a e` is `Ok[A] |
  Error[E]` (the prelude's classes become generic), `Seq a` is `Iterable[A]`, `Async a` is
  `Awaitable[A]`, `unit` is `None`.
- **A function is a `Callable` of all its arguments at once,** `Callable[[A, B], C]` for `a -> b -> c`,
  since that is how the emitted code calls it (§5).
- **What is erased is spelled as what is left:** a unit of measure is gone (`float<m>` is `float`), a
  newtype is its underlying type, an effect has no spelling. An opaque handle type, about which Pyfun
  knows nothing, is `Any`.
- **A type declared in another module of the project** is reached through that module (`geo.Shape`),
  which the file imports if the code had not already.

A typed file opens with `from __future__ import annotations`, so an annotation may name a class or an
alias defined further down, and a recursive type can name itself. The type parameter syntax is Python
3.12's, so `--typed` does not combine with `--target 3.11`; nor with `--native`, which annotates for
mypyc's needs instead.

What is promised is the surface a caller sees. `tests/typed_output.py` holds it to that with mypy: a
consumer's correct uses pass and its wrong ones are rejected at their lines, a typed project has
nothing reported at all, and no annotation in any lesson or example is malformed. The *bodies* of
emitted functions are not annotated, and a type checker pointed at them finds idioms it would write
differently (a temporary assigned an `Ok` in one branch and an `Error` in the other); a caller of the
package never sees those.

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

**The prelude (first realized interop surface).** A small set of built-in functions gives programs
something to call. The MVP prelude is `print : a -> unit` and the unit-polymorphic numerics
`abs`/`min`/`max : int<'u> -> …`, plus the **unit-preserving numeric conversions**
`round`/`floor`/`ceil`/`truncate : float<'u> -> int<'u>` (`round` is a bare Python builtin; `floor`/`ceil`/
`truncate` lower to `math.floor`/`ceil`/`trunc` with `import math` — the extern dotted-target path — while
staying *unqualified* Pyfun names), plus the **unit-aware roots `sqrt : float<'u^2> -> float<'u>`**
(√area = length) **and `cbrt : float<'u^3> -> float<'u>`** (∛volume = length; see §8.2; lower to
`math.sqrt`/`math.cbrt` like the conversions; as in Python, the result is the platform C library's, and glibc's
`cbrt` is not correctly rounded, so `cbrt 27.0` prints `3.0000000000000004` on Linux and `3.0` on Windows: the
last digit of a float maths function can differ between systems), plus a `unit` type whose one value is written `()` (both lower to
Python `None` — the natural result of an effectful call). It also seeds the **standard combinators**
`id : a -> a`, `const : a -> b -> a`, `ignore : a -> unit`, and
`flip : (a -> b -> c) -> b -> a -> c` (fully type-polymorphic; `id`/`const`/`ignore` are pure, while
`flip` is **effect-polymorphic** because it calls its function argument — flipping an impure function
is `io`). Unlike the numeric builtins these can't lower name-for-name (Python's `id` returns a memory
address; the others have no builtin), so each routes to a tiny emitted `_pf_*` helper in `lower_var`
(the same on-demand mechanism as the `List`/`Set`/`Map` helpers); `_pf_flip(f, x, y)` calls `f(y, x)`
n-ary, exactly as a hand-written `let flip f x y = f y x` compiles, so it is neither more nor less
capable than that definition. Each is a *typed view over a Python builtin or `_pf_*` helper*: the single
source of truth is `types::PRELUDE` (names + arities, read by lowering so a partial application like
`max 0` still lowers to `functools.partial`) alongside `seed_prelude` (the type schemes). Pyfun
names equal their Python names (or a routed helper), so there is no call-site renaming — the simplest interop
mapping. User definitions shadow prelude names. This is deliberately tiny; collections/option/
result helpers are the obvious next increments.

**`extern` — the general FFI surface.** The "import and type an arbitrary Python
function" story is now a first-class declaration:

```
extern len : str -> int                  # Pyfun name = Python name
extern show : a -> str = str             # aliased to a Python builtin
extern pure cbrt : float -> float = math.cbrt   # dotted path; module auto-imported
```

`extern [pure] name : type [= a.b.c]` binds `name` to a Python callable (or value) at a declared
Pyfun type. Type variables are bare lowercase identifiers (as in `type` declarations) and are
generalized, so `show : a -> str` is polymorphic. The optional `= a.b.c` clause is the dotted
Python target; omitted, it defaults to the Pyfun name (the prelude convention). A reference lowers
directly to its target (`math.cbrt`), and the module prefix of a *used* extern is emitted as an
`import` (deduplicated, sorted). The dotted target mixes a module path with an attribute path, and
only its shape separates them, so the emitted import follows PEP 8 (packages lowercase, classes
capitalized): it is the **maximal leading run of lowercase-initial segments** before the final
referenced name — always at least the top-level package. Thus `urllib.request.urlopen` imports the
submodule `urllib.request`, while `sqlite3.Connection.execute` imports only `sqlite3` (`Connection`
is a class attribute, not a submodule); a target rooted at a builtin type (`str.upper`,
`int.from_bytes`) imports nothing, since those names are always in scope. The one shape the heuristic
can't see through is a *lowercase* attribute that is a value or class rather than a submodule
(`sys.stdout.write`; classmethods on the lowercase class `datetime.datetime`). For those, declare the
module explicitly with **`extern import`** — Python's own import statement behind the `extern`
keyword, `as` and all:

```
extern import datetime
extern import numpy as np
extern now : unit -> Datetime = datetime.datetime.now    # import datetime; datetime.datetime.now()
extern zeros : int -> Arr = np.zeros                     # import numpy as np; np.zeros(n)
```

**An undecidable module prefix is a compile error, not a guess.** A lowercase segment *after the
first* could be a submodule (`os.path`, `urllib.request`) or an object (`sys.stdout`,
`datetime.datetime`), and which one it is belongs to the running environment, not to the text. The
compiler used to guess with PEP 8 (maximal leading run of lowercase segments), which emitted
`import sys.stdout` for `sys.stdout.flush` and raised `ImportError` at runtime. It now refuses the
shape and names the fix: `` cannot tell which part of `sys.stdout.flush` names the module … declare it
with `extern import sys` — or `extern import sys.stdout` if `stdout` really is a module ``. A declared
`extern import` settles it and the error goes away, which is the escape hatch that already existed.
Capitalised segments still settle themselves (`pathlib.Path.read_text` needs nothing, since `Path` is
a class), and a two-segment target has nothing to guess. The alternative — emitting the deeper import
guarded by `try/except ImportError` — was rejected: it puts defensive machinery in the output for
something the author can state exactly, and a compile error that names the one line to add is the
better trade (`ROADMAP.md` finding #7).

A used target rooted at a declared path (longest match) — or at its alias name — imports the module
exactly as declared (trusted as written, the same signed-contract stance as the rest of the
boundary), and only otherwise does the heuristic decide (`Lowerer::extern_import_spec`). One
declaration serves every extern in the file rooted at that module. It is purely an import/lowering
concern: no Pyfun name is bound, the checker ignores it (`Item::ExternImport`), an unused declaration
emits nothing, and the emitted reference is always the full dotted target as written. The `extern`
prefix keeps it distinct from Pyfun's own file-module `import Geometry` (§6.1) — the module graph and
LSP are untouched. Receiver-reachable members can of course still use the instance-access form below
instead. A `unit` **parameter contributes no argument**, wherever it sits along the arrow: a
**nullary** extern (`unit -> a`, e.g. `time.time`) applied to `()` lowers to a zero-argument call
(`time.time()`), not `time.time(None)`; a leading unit before real parameters drops the same way
(`extern my_abs : unit -> int -> int = abs` lowers `my_abs () (-5)` to `abs(-5)`, never the split
`abs()(-5)`); and a mid-list unit simply vanishes from the call (`int -> unit -> int -> int = max`
emits `max(3, 5)`). One rule, three shapes. The dropped argument is still evaluated when it could
have effects, and a partial application closes over a lambda rather than `functools.partial`, which
could not drop a `unit` that has not yet arrived. The mirror image is a **thunk parameter**:
an extern parameter typed `unit -> a` (`asyncio.to_thread`'s function, an event handler with no
arguments) is a Pyfun function of one parameter, the unit value, that Python will call with none, so
the call site wraps that argument as `lambda: f(None)` (a literal `fun _ -> body` collapses to
`lambda: body`). Only the slots the declared type marks are touched; a curried multi-parameter
callback is already a multi-parameter `def` and passes as it is, which is why callbacks are written
curried, not over a tuple: Python has no way to tell `(a, b) -> c` meant "call me with two
arguments" from "call me with one pair" (`sorted(pairs, key=f)` wants the pair), so the boundary
does not guess (issue #107). Arity is the number of leading arrows, so partial application of an
extern still lowers to `functools.partial` exactly like a prelude builtin. Calls are still
type-checked at the boundary (`cbrt "x"` is rejected) — but only against the *declared* type; Pyfun
trusts the annotation, which is where the §4 "effectful/unsafe at the boundary" relaxation bites.
(An extern may not redeclare an existing name, prelude builtins included — so the old dimensionless
`extern pure sqrt … = math.sqrt` workaround now errors, pointing at the built-in unit-aware `sqrt`.)

This makes the boundary's effectful-by-default rule (§4) concrete: a plain `extern`'s innermost
arrow carries `io` (the Python call is the effect, performed on full application), so an impure
`extern` cannot be called from a `let pure` binding. `extern pure` asserts the call is effect-free
("pure up to its arguments", like `let pure`) — used for the likes of `math.cbrt`; or an explicit
innermost-arrow annotation (`->{async}`, which must then return `Async _`) overrides the `io`
default (§4). Externs are
erased to nothing themselves; only their reference sites and imports survive lowering. The prelude
(`print`/`abs`/`min`/`max`) remains separately seeded because it needs `num`/unit polymorphism the
`extern` type syntax can't yet express.

**Opaque handle types (`extern type`).** A Python object Pyfun never looks inside — a `sqlite3`
connection, a `pathlib.Path`, an HTTP response — needs a Pyfun type only so it can cross the `extern`
boundary and be passed between calls. `extern type Conn` (optionally parameterized, `extern type Ref a`)
declares exactly that: a nominal type name with an arity but **no constructors, no fields, and no
runtime representation**. It registers like any user type (so `extern connect : str -> Conn` checks),
but has no way to be built or `match`ed — it is only ever produced and consumed by externs — and it
**erases** at lowering, emitting no Python class. It is preferable to a phantom-ADT workaround
(`type Conn = ConnH`, a nullary sum whose sole constructor exists only to be never used): the one-liner
carries the intent ("opaque, don't look inside"), drops the throwaway constructor that could be
mistakenly applied, and emits nothing. Parsing is disambiguated by lookahead — `extern type …` is the
handle form, `extern name : …` the value form. (For a value Pyfun *does* construct and inspect but
wants nominally distinct at zero cost — `UserId` over `str` — see **opaque types**, §7.3: same
erasure, but with a constructor and a single-case pattern.)

**Instance-access externs (`= .member`).** A leading dot marks the target as a *member path applied to
the first argument* — the receiver — rather than a module-qualified free function. Trailing `()` is the
"call" marker that separates the two forms: a **method** `= .read()` lowers `read resp` to `resp.read()`
and `extern execute : Conn -> str -> Cursor = .execute()` lowers `execute conn sql` to
`conn.execute(sql)`; a **property** `= .scheme` (no `()`) lowers `scheme url` to `url.scheme` — an
attribute read, no call. For a method, currying falls out of Python's own binding, so the receiver-only
partial `execute conn` is the bound method `conn.execute` (no `functools.partial`); a bare reference to
either form is a receiver-taking lambda (`lambda r: r.read()` / `lambda r: r.scheme`). This is the
idiomatic way to wrap object-oriented libraries: it never names the receiver's *class*, so it needs no
class-module import and no casing guess, and — unlike the alternative of an unbound `Class.method`
target — it reaches members that are **inherited or delegated** rather than defined directly on the
named class (e.g. `urllib.response.addinfourl.read`, which exists only on an instance). All three target
forms coexist; the plain dotted form stays right for genuine module functions and staticmethods.

**Pinned keyword arguments (`= target(kw = lit, …)`).** A trailing `(kw = lit, …)` on an
extern target pins **fixed Python keyword arguments**, appended to every emitted call:
`extern open_text : str -> Seq str = builtins.open(mode="rt", encoding="utf-8")` lowers `open_text path`
to `builtins.open(path, mode="rt", encoding="utf-8")`, and the receiver-method form
`extern write_text : Path -> str -> int = pathlib.Path.write_text(encoding="utf-8")` lowers `write_text p text`
to `p.write_text(text, encoding="utf-8")`. This reaches Python keyword-only / deep-positional arguments
(such as `open`'s `encoding`) *without* the ugly positional fillers that would otherwise be needed to
skip past intervening parameters. Each `kw` is a lowercase identifier and each `lit` a string / int
(optionally negated) / float / bool literal — the only forms the boundary needs; a duplicate keyword or
a non-literal value is a parse error. The pinned kwargs are **purely a lowering concern**: they never
appear in the Pyfun arrow type (the type above is still `str -> Seq str`), they are invisible to
inference and effects, and the parens compose with all three target forms (an empty `()` after a leading
dot is still just the method marker). The feature is **strictly additive** — an extern with no `(…)`
behaves exactly as before. Under-application never *drops* the kwargs: a partial or bare reference to a
kwargs-extern carries them through `functools.partial` (`open_text` → `functools.partial(builtins.open,
mode="rt", encoding="utf-8")`), so a later application still supplies them; only a full (or over-)
application emits the direct `f(a, kw=v)` call.

**Caller-supplied keyword slots (`= target(kw = ...)`).** Python's API culture is optional keyword
arguments with defaults, and pinning a literal only covers the value that is fixed for every call. A
`...` in place of the literal makes the keyword's value come from the **caller** instead:

```
extern parse_int : str -> int -> int = int(base = ...)          # parse_int "ff" 16 → int("ff", base=16)
extern open_text : str -> str -> Seq str = builtins.open(mode = "rt", encoding = ...)
extern write_text : Path -> str -> str -> int = .write_text(encoding = ...)
```

The spelling is Python's own stub-file placeholder (`def get(url, timeout=...)`), lexed as one token.
The **binding rule** is positional and mirrors a Python call: the target takes the leading arguments
positionally and the `...` slots take the **trailing** ones, in the order the keywords are written;
pinned literals consume no argument, so they may sit anywhere among the slots
(`m.f(a = 1, b = ..., c = "x", d = ...)` at arity 3 emits `m.f(s, a=1, b=i, c="x", d=b)`). A slot claims
one argument of the declared arrow, so the type must have one to spare: a receiver takes the first
argument and a `unit` parameter contributes none, so a type left with nothing for a slot to claim is
rejected with a diagnostic rather than silently mis-lowered.

A slot changes only *where* an argument lands in the emitted call, so like a pinned literal it stays
**invisible to the type** (`parse_int` is an ordinary `str -> int -> int`), to inference, and to
effects. Under-application still never drops anything, but it cannot use `functools.partial`, which has
no way to carry a keyword whose value has not arrived. An under-applied slot extern becomes a **lambda**
over the remaining arguments (`parse_int "ff"` → `lambda _pf_k0: int("ff", base=_pf_k0)`; a bare
`parse_int` → `lambda _pf_k0, _pf_k1: int(_pf_k0, base=_pf_k1)`). The arguments already supplied are
bound to temporaries first, so they evaluate at application time exactly as `functools.partial` would
have evaluated them, rather than once per later call; a receiver is bound the same way.

**Lists — the eager collection.** `List a` is a built-in type that **lowers to a
Python `list`** (a dynamic array), with literal syntax `[1, 2, 3]` (comma-separated, like Python and
like Pyfun records and tuples). The big-O is Python's, *not*
F#'s linked `list`: index/`len` are O(1), append-end O(1) amortized, prepend/concat O(n). `List` is
therefore the analogue of F#'s **array**, not F#'s linked `list`. So a singly-linked `list` and its
idioms (`cons`/`head`/`tail`, `x :: xs` cons-decomposition in `match`) are a **non-goal**: a cons-cell
type would lower to un-Pythonic linked nodes, and its recursive idiom is stack-unsafe without TCO (also
a non-goal — CPython has none). Python has no built-in singly-linked list either (`deque` is
doubly-ended). The array-appropriate, Python-native counterpart — **sequence patterns** `case []` /
`case [x]` / `case [first, *rest]` over `List` — is shipped (§7.2), beside the bulk
operations. The list operations are
`List.map`/`List.filter`/`List.fold`/`List.len`/`List.sum`/`List.rev`/`List.range`/`List.zip` — **module-
qualified** (see *Built-in modules* below), single source of truth `types::LIST_PRELUDE` +
`seed_list_prelude`. `List.len`/`List.sum` map name-for-name onto the Python builtins; the rest lower
to small **emitted helpers** (`_pf_map` = `list(map(...))`, `_pf_fold` = `functools.reduce(...)`, etc.)
emitted on demand like the `Result` prelude — wrappers are needed because Python's `map`/`filter` are
lazy and we want eager lists, and because a first-class curried function must be a single callable (so
partial application still lowers to `functools.partial`). The higher-order functions are
**effect-polymorphic**: `List.map : (a ->{e} b) -> List a ->{e} List b`, so mapping an impure function
makes the whole call `io` and that flows out (a single bound effect variable links the function arrow
to the traversal arrow). The lazy counterpart already exists as the `seq {}` computation expression
(§8.1).

**The rest of the `List` surface.** A dogfooded program should not have to write its own prelude
before it starts, so the module covers the F# core set rather than growing one function at a time
(`ROADMAP.md`). Three conventions decide the shapes, and the first two were already the house style:

- **Total, never raising.** Anything F# raises on, Pyfun answers with `Option` under the *plain* name,
  following `get`/`find`: `head`/`last`/`tail`, `max`/`min`/`max_by`/`min_by`, `average`, `reduce`,
  `find_index`. `sum_by` is not among them — an empty sum is zero, so it needs no `Option`.
- **Counts clamp** rather than erroring, like `String.slice`: `take`/`drop`/`split_at` past either end
  give everything or nothing, a negative count reads as none of it, and `update_at`/`remove_at` outside
  the list leave it unchanged (`insert_at` clamps to the ends). `windowed`/`chunk_by_size` answer `[]`
  for a non-positive size.
- **`take`/`drop`**, not F#'s `take`/`skip` — the same kind of divergence as `concat`, chosen for the
  more obvious word.

Access and slicing: `head`, `last`, `tail`, `take`, `drop`, `split_at`. Transform: `map2` (stopping at
the shorter input, where F# raises, matching the existing `zip`), `indexed`, `iter` (effect-carrying,
like `map`). Query: `exists`, `forall`, `find_index`. Order: `sort_by`, `sort_descending`, `distinct`
(first occurrence wins, order kept), `distinct_by`, `group_by` (in first-seen key order). Aggregate:
`max`, `min`, `max_by`, `min_by`, `sum_by`, `average`, `reduce`. Structure: `partition` (testing each
element once), `unzip`, `flatten`, `init`, `replicate`. Positional update: `update_at`, `insert_at`,
`remove_at`, each returning a fresh list. Windows: `pairwise`, `windowed`, `chunk_by_size`. The
`comparison` constraint lands on the *key* where there is one (`sort_by`/`max_by`/`min_by` constrain what
the function returns, not the element), and every higher-order member is effect-polymorphic like `map`.

`average : List float -> Option float` is deliberately float-in/float-out: averaging integers would
silently change type, and an integer literal that must be a float is already inferred as one.

**Tuple accessors.** `fst` and `snd` are global (like `abs`, and like F#), not module-qualified.
Destructuring in a parameter or a `match` covers most uses, but a projection you can *pass*
(`List.map fst pairs`) cannot be written point-free without them.

The **completeness ops** — `get`/`is_empty`/`contains`/`concat`/`sort`/`find` — round out the array,
each with big-O faithful to a Python `list`: `get : int -> List a -> Option a` is **O(1)** and
**bounds-checked → total** (there is deliberately *no* `xs[i]` surface syntax, since bare indexing would
risk a Python `IndexError`, violating the no-runtime-surprises rule); `is_empty` is O(1); `contains` is
**O(n)** linear (use `Set` for O(1) membership); `concat` is O(n+m) returning a fresh list; `sort :
comparison a => List a -> List a` is O(n log n) (`sorted`, so it carries the `comparison` constraint —
ADT ordering is out of scope); and `find : (a ->{e} bool) -> List a ->{e} Option a` is O(n),
**first-match/lazy** (`next(map(Some, filter …))`) and effect-polymorphic like `filter`. Two fused
higher-order ops complete the F# core set: `choose : (a ->{e} Option b) -> List a ->{e} List b` keeps
the payloads of the `Some` results (F#'s `List.choose` — `filter`+`map` in one O(n) pass; an
extend/append loop in the helper), and `collect : (a ->{e} List b) -> List a ->{e} List b` maps and
concatenates (F#'s `List.collect`, i.e. flatMap; `List.collect (fun x -> x)` flattens a list of
lists). Both are effect-polymorphic like `map`. **Naming note:** Pyfun's `concat` (append two lists)
diverges from F#, where `concat` *flattens* and `append` joins two — a deliberate cost: to a
Pythonista "concat two lists" is the natural reading (`+`). Flattening is the separate `flatten`. There is
deliberately **no cheap-looking prepend/`cons`** (O(n) on an array — the linked-list non-goal); and
because the ops are immutable-style, building a list by repeated `concat` is *nominally* O(n²) —
though a `Seq.fold`/`List.fold` that builds a collection linearly is recognized and lowered to an
in-place loop (§5.1), so the idiomatic `fold`-with-`concat`/`Map.add` construction runs in linear
time; construction otherwise stays `map`/`fold`/comprehension/`Seq`. `get`/`find` return the built-in `Option` (setting `needs_option`),
and `get` introduced a `PyExpr::Subscript` node.

**Tuples — the structural product.** `(a, b, c)` is a tuple: an anonymous, **structural**
product of two or more values — Pyfun's first structural type (records are nominal, resolved by a field
registry; a tuple type is just its element list, `Ty::Tuple(Vec<Ty>)`, unified element-wise by arity
then pairwise). The surface forms are symmetric: literal `(a, b)` (`ExprKind::Tuple`), pattern `(a, b)`
(`Pattern::Tuple`), and type annotation `(a, b)` (`TypeExpr::Tuple`), all printed and displayed with
parentheses. **Disambiguation is by precedent, no new tokens:** `()` is the unit value (not a 0-tuple —
unit *is* the empty product), `(x)` is grouping (not a 1-tuple), and `(a, b)` (a comma after the first
element) is a tuple — so a tuple always has ≥2 elements. The parser checks for a comma after the first
parenthesized element in all three positions (expression, pattern, type). Tuples **lower ~1:1 to Python
tuples** (`PyExpr::Tuple` → `(a, b)`; `Pattern::Tuple` → a sequence pattern `case (a, b):` via
`PyPattern::Sequence`). A tuple is a **single-constructor** type, so a tuple pattern of variables is
exhaustive on its own, and **deep exhaustiveness recurses into the element columns** (`Tag::Tuple(arity)`
in the Maranget matrix), reporting witnesses like `` `(False, _)` is not matched ``. Tuples unblock
multi-value return and pair lists; the stdlib follow-ons that need them — `List.zip : List a -> List b ->
List (a, b)` and `Map.of_list`/`Map.to_list` (to/from a `List (k, v)`) (see the list and map
sections above).

**Sets and maps — the hashed collections.** `Set a` and `Map k v` are built-in types
that **lower to a Python `set` / `dict`**. They have **no literal syntax** (`{…}` is already records
and CE builders) and **no constructors** — built entirely from module functions, so adding them needed
no lexer/parser/AST changes, only seeded schemes + emitted helpers. The two modules (single source of
truth `types::SET_PRELUDE` / `MAP_PRELUDE` + `seed_set_prelude` / `seed_map_prelude`) are all **pure**
(unlike `List`'s higher-order trio, none take a function):
`Set.empty`/`Set.add`/`Set.remove`/`Set.contains`/`Set.len`/`Set.union`/`Set.intersect`/
`Set.difference`/`Set.of_list`/`Set.to_list`, and `Map.empty`/`Map.add`/`Map.remove`/`Map.contains`/
`Map.find_or`/`Map.find`/`Map.len`/`Map.keys`/`Map.values`. `Set.len`/`Set.of_list`/`Set.to_list`/
`Map.len` route to bare Python builtins (`len`/`set`/`list`); the nullary `Set.empty`/`Map.empty` lower
to `set()`/`dict()`; the rest lower to small **emitted helpers** (`_pf_set_add` = `s.union([x])`,
`_pf_map_add` = `dict(list(m.items()) + [[k, v]])`, …) so the curried function is one callable (partial
application → `functools.partial`). The collections are **immutable-style**: every operation returns a
fresh container. `Map.find_or key default m` is a **total lookup with a fallback** (`dict.get`);
`Map.find key m : Option v` is the optional form. `Map.of_list : List (k, v) -> Map k v` and
`Map.to_list : Map k v -> List (k, v)` convert to/from a list of key/value **tuples** (`Map.of_list` lowers
to a bare `dict(pairs)`; `Map.to_list` to `list(m.items())`), mirroring `Set.of_list`/`to_list`. Element/key
types are **unconstrained polymorphic**
but must be **hashable at runtime** — Pyfun primitives are, and ADT/record values are too: generated
classes get a structural `__hash__` (a tuple of the type and field values, consistent with the
structural `__eq__`), so `Set Color` and `Map (Point) v` work and equal values collapse. A field that
is itself unhashable raises at hash time, the same way Python rejects an unhashable key. `Array` is
**deferred** as redundant — `List` already *is* a Python list (dynamic array).

**Option and Result — the built-in sum helpers.** `Option a` (constructors `Some`/`None`)
is seeded exactly like `Result a e` (`Ok`/`Error`): a reserved type with global constructors that lower
to `Some`/`None_` (resp. `Ok`/`Error`) classes (`None` is mangled off the Python keyword), emitted on
demand. Each has a module of combinators: `Option.map`/`bind`/`filter`/`with_default`/`is_some`/`is_none`/
`to_result`, and `Result.map`/`map_error`/`bind`/`with_default`/`is_ok`/`is_error`/`to_option`. The
mapping/binding/filtering ones are **effect-polymorphic** (like `List.map`). `Map.find` returns an
`Option`; the two bridge **both ways** — `Result.to_option` (`Ok v → Some v`, `Error _ → None`) and
`Option.to_result e` (`Some v → Ok v`, `None → Error e`). A user `type Option`/`Result` is rejected (reserved).

**Strings — the `String` module.** Text operations over the built-in `str` type (which
lowers to a Python `str`), module-qualified like the collections: `String.len`/`concat`/`join`/`split`/
`upper`/`lower`/`strip`/`contains`/`starts_with`/`ends_with`/`replace`/`from_int`/`from_float`/`to_int`/
`to_float`/`to_list`/`slice`/`index_of` (single source of truth `types::STRING_PRELUDE` +
`seed_string_prelude`). `slice start end s` → `s[start:end]` (total Python slicing, end-exclusive,
clamps out-of-range; emitted via a `PyExpr::Slice` node so the output reads `s[a:b]`); `indexOf sub s
: … -> Option int` uses `str.find` and returns `None` when absent (total — no `IndexError`, like
`List.get`). **A lookup is not marked `try`**: every one is total
and returns an `Option` (`List.find`, `List.head`, `List.get`, `Map.find`, `String.index_of`), so the
prefix would say nothing the type does not; `Map.tryFind` and `String.tryIndexOf`, the two that
carried it before 0.11, are answered with the new name. **Naming is one rule: Python's
word, snake_case spelling.** A name uses Python's word where it has a natural one (`len`/`upper`/`lower`/
`strip`/`lstrip`/`rstrip`/`split`/`join`/`replace`, matching Python's `str` methods, and consistent with
`List.len`) and Pyfun's own otherwise (the `contains`/`of_list`-style `to_int`/`to_list`/`from_int` family).
Every multi-word name is snake_case with an underscore at each word boundary (`sort_by`, `with_default`,
`find_or`), and that holds where Python itself runs the words together: `starts_with`, `split_lines`,
`is_subset` and `take_while` for Python's `startswith`, `splitlines`, `issubset` and `takewhile`. Mirroring
Python letter for letter would import its inconsistency (`group_by` beside `groupby`), and a reader could
not predict a name without having memorised Python's; the run-together spelling is a short edit from
the member, so the suggestion bridges it (`String.startswith` is answered with `String.starts_with`). The
camelCase names of releases before 0.11 are answered the same way, whatever the edit distance
(`List.sort_by` with `List.sort_by`, `String.lstrip` with `String.lstrip`). The rule reaches every
compiler-known value name: a computation-expression builder's `return_from` and `yield_from` (§8.1) and
the `Exception` record's `error_kind` and `error_message`. Types, constructors and modules stay
PascalCase, which the grammar relies on, and a measure keeps its author's case because SI symbols need
it (`N`, `Pa`, `mm`). An `extern` names its Pyfun side freely, so Python's own mixed casing stops at the
boundary: `extern get_logger: str -> Logger = logging.getLogger`. Unlike the collection preludes these
schemes are **monomorphic** (concrete over `str`/`int`/`float`/`bool`, no type variables) and all
**pure**. There is **no `char` type** — a character is a length-1 string, so `String.to_list : str ->
List str` yields single-character strings and `String.join`/`concat` compose them back. Separator-first
argument order (`String.join ", " xs`, `String.split "," s`) keeps partial application natural. Lowering
mirrors the other modules: `len`/`from_int`/`from_float`/`to_list` route to bare Python builtins
(`len`/`str`/`list`); the rest lower to emitted `_pf_str_*` helpers (`_pf_str_upper` = `s.upper()`,
`_pf_str_split` = `s.split(sep)`, …) so each curried function is one callable. The one total parse,
`String.to_int : str -> Option int`, lowers to a `try`/`except ValueError` helper returning
`Some(int(s))`/`None_` (the first use of the general `PyStmt::Try` IR node) and pulls in the `Option`
prelude. Overloading `+` for strings is deferred — `String.concat` is the concatenation path.

**Formatting — the `Format` module.** The typed alternative to Python's format specifiers
(the `:.2f`/`!r` mini-language, a non-goal below): checked functions that build the spec themselves, so
a `.2f`→`.f2` typo is impossible. First cut (single source of truth `types::FORMAT_PRELUDE` +
`seed_format_prelude`): `Format.fixed n x` (n decimals, no grouping), `Format.thousands n x` (decimals +
grouping), `Format.percent n x` (ratio → percent), `Format.currency sym n x` (symbol + grouped amount),
`Format.grouped x` (grouped integer), and `Format.pad_left`/`pad_right w fill s` (alignment, replacing
`:>N`/`:<N`). The numeric formatters are **unit-polymorphic** over `float<'u>`/`int<'u>` — the unit is
compile-time only and erases at lowering — so `Format.currency "£" 2 19.5<gbp>` checks; padding is
monomorphic over `str` (`fill` is a length-1 string, per the no-`char` rule). All **pure**; each
lowers to an emitted `_pf_fmt_*` helper wrapping `format(x, spec)` (the spec a nested f-string assembled
from the checked `int`) or `str.rjust`/`ljust`. Dates are deferred (no date type; they would need a
Python `datetime` `extern`).

**Cold async.** An `Async a` is **cold**, as F#'s `Async` is: building one runs nothing, and every
await is a fresh run, so the same value can be awaited twice, retried, or passed to
`Async.parallel` twice and run twice. A Python coroutine is single-shot (a second `await` raises
`RuntimeError: cannot reuse already awaited coroutine`), so the value is not one. It is a small
emitted `_pf_Async(fn, *args)` whose `__await__` calls `fn(*args)` afresh: `let job = async { … }`
lowers to `job = _pf_Async(_pf_fn0)`, and a saturated call of an `extern` whose result is `Async`
to `_pf_Async(f, a, b)` (function and arguments evaluated at once; pinned keywords ride a
`functools.partial`). Awaiting one where it is built prints as the direct call (`await
inbox.get()`). Where Python insists on a real coroutine, a value passes through `_pf_run(x)` (`async
def _pf_run(a): return await a`), a fresh coroutine of any awaitable: an `extern` parameter typed
`Async a`, a callback parameter returning `Async` (wrapped so each call Python makes returns one:
`asyncio.start_server` schedules a handler's result only if it is a coroutine, and would otherwise
drop it silently), a `unit -> Async a` thunk, and every element `Async.parallel`/`race` start. In a
project the exporting module's binding for such an extern does the adapting, since a dependent
knows only its arity, and a bare or partially applied reference is the same adapting lambda
(`lambda p0: _pf_Async(asyncio.sleep, p0)`, under a `functools.partial` for supplied arguments), so
`urls |> List.map fetch` builds cold values too. Python code handed a Pyfun `Async` can `await` it
any number of times, or call it for a fresh coroutine, as it would a zero-argument `async def`
(`asyncio.run(job())`): `_pf_Async.__call__` is `async def __call__(self): return await self`.
Python 3.14's `asyncio.run` accepts any awaitable, but 3.12 and 3.13 want a coroutine, so code that
must run on them calls the value. Cold also means an `extern` whose Python call starts work at once
(a call returning an `asyncio` Task or Future, or a JS promise under Pyodide) now starts it at the
await: two such calls bound with `let` and awaited afterwards run one after the other, where they
used to overlap, and awaiting one twice makes the call twice. A program overlaps work by handing it
to `Async.parallel` (or `race`) together, which starts every element before awaiting any.

**Async combinators — the `Async` module.** `async { }` builds an `Async a`; the module is the rest
of what F#'s `Async` offers that `asyncio` can supply, typed once
(single source of truth `types::ASYNC_PRELUDE` + `seed_async_prelude`): `Async.sleep secs`,
`Async.timeout secs c : Async (Option a)` (`asyncio.wait_for`, `None` on the deadline),
`Async.to_thread thunk` (a blocking call off the event loop, `asyncio.to_thread`), `Async.parallel
xs : Async (List a)` (`gather`, results in order), `Async.race xs : Async a` (`wait(FIRST_COMPLETED)`,
the losers cancelled; the list must be non-empty — racing nothing would wait forever, so the empty
list raises `ValueError("Async.race needs at least one value")` at the await, the module's one
partial member) and `Async.catch c : Async (Result a Exception)`, the await-time `try`: `try e`
catches at *call* time, and an async extern does not throw when called, it returns a coroutine that
throws when awaited, so `try` would wrap the coroutine and the `TimeoutError` would escape at the
`let!`. `catch` catches `Exception`, not `BaseException`, so `asyncio.CancelledError` passes through
and structured cancellation keeps working. Building any of these is pure (an `Async` is a value);
the `async` effect is attributed where the value is awaited, except `to_thread`, whose arrow carries
the thunk's effect. Each is an emitted helper returning a `_pf_Async` (a fully applied
`Async.sleep secs` builds `_pf_Async(asyncio.sleep, secs)` directly),
because `gather(*xs)` spreads a list the extern syntax cannot, `wait` returns task sets, and two of
them build `Option`/`Result` values. **Deliberately absent:** `Async.start` and `Async.cancel`. A task
should only start inside a scope that will join or cancel it, which is the structured-concurrency
item on the roadmap (`Task.scope`/`Task.start` over `asyncio.TaskGroup`); a free `start` is the leak.

**Structured concurrency — the `Task` module.** Every task is owned by a scope, the scope does not
exit until its children finish, one failure cancels the siblings, and leaving the scope cancels
everything. Python 3.11 has the primitive (`asyncio.TaskGroup`); Pyfun types the discipline with an
**effect**: `Task.start : Async unit ->{spawn} unit` performs `spawn`, and `Task.scope : Async a ->
Async a` is its **handler**, Pyfun's first. When `Task.scope body` is applied, the `spawn` that
evaluating `body` performs stops there (the rest of its effect passes through), so tasks started in
the body, directly or through a helper whose arrow carries `->{spawn}`, belong to the scope. A
top-level evaluation that still performs `spawn` is a compile error ("this starts a task outside any
`Task.scope`"), since nothing would own the task; defining a function that starts tasks is fine, as
its effect is latent until it is called. There is no `cancel` to forget. The discharge is a rule in
`Infer::infer_apply` for a saturated `Task.scope` application rather than a change to effect
unification: effects are sets, not rows, so `{spawn} ∪ e` meeting `{async, spawn}` widens `e`
rather than subtracting, and teaching the unifier subtraction would change every effect-polymorphic
signature. A bare `Task.scope` passed as a value is the plain `Async a -> Async a` and discharges
nothing. At run time `Task.scope` opens `async with asyncio.TaskGroup()` in a task of its own and
publishes the group through a `contextvars.ContextVar` (`_pf_scope`), which `Task.start` reads;
running the scope as its own task gives it a private copy of the context, so the innermost open scope
is always the one a start sees and nothing leaks to the caller when a scope ends. In a multi-file
project the variable lives in `_pyfun_rt.py`, so a module's helper can start a task in a scope
another module opened. This replaced an earlier **value form**, where the body received a `Scope`
capability argument and `Task.start` took it explicitly; the effect makes the argument redundant. `Async.parallel`/`race`/`timeout`
stay the one-shot combinators they are; a `task { }` spelling was considered and set aside: the §8.1
rule keeps the built-ins at four, and a user builder cannot open the scope around the whole block
(the protocol has no `run` member), while `Task.scope (async { … })` reads well enough
that adding `run` to the protocol waits for demand. A failed child raises an `ExceptionGroup` out of
the scope, which `Async.catch` reports as one `Exception` (kind `ExceptionGroup`); a
`Result a (List Exception)` reading of the scope stays open until a program needs the members. The
lowering is two emitted helpers over `asyncio` and one Python IR node, `PyStmt::AsyncWith`. A
blocking `input()` on a worker thread cannot be cancelled, so a keyboard task under `Async.to_thread`
outlives its scope until the process exits; that is the platform's limit, not the scope's.

A mailbox **agent** (F#'s `MailboxProcessor`) is deliberately *not* a prelude module: it is an
`asyncio.Queue` behind two externs and a recursive `async` loop that is a `match` over a message ADT,
which the cookbook shows (`examples/interop/structured_concurrency.pyfun`) and the game consumes; it
is promoted once its signature stops moving (whether `start` takes the scope, for one).

**JSON decoding — the `Decode` module.** An Elm-style (`elm/json`) decoder-combinator
library over an opaque built-in `Decoder a`, module-qualified like the others (single source of truth
`types::DECODE_PRELUDE` + `seed_decode_prelude`). It is the batteries-included form of the "parse, don't
validate" pattern: instead of casting an untyped `dict` into a record (which type-checks but crashes,
since `json` yields subscript-access dicts and a Pyfun record is an attribute-access class), you build a
**decoder** — a total recipe — and run it with `Decode.decode_string : Decoder a -> str ->
Result a Exception`, so malformed input is a value you `match`, never a crash. Primitives
`Decode.str`/`int`/`float`/`bool` are **strict** (a JSON number does not decode as a string;
`Decode.int` rejects a JSON bool — a Python `bool` is an `int` subclass — while `Decode.float` accepts a
JSON int); structure combinators `Decode.field name dec`, `Decode.list dec`, `Decode.nullable dec` (JSON
`null` → `Option`), `Decode.dict dec` (an object whose keys are data, every value alike: a `Map str a`
in the object's order) and `Decode.index i dec` (the element at one position of an array, counted from
0 like `List.get`, for an array whose positions each mean something else: `[1, [4, "10"]]`; an array
too short is an `Error` saying which index was wanted and how long the array is); and composition `Decode.map`/`map2`/`map3`/`map4` (the fan-in that builds a record
from several field decoders at once), `Decode.and_then` (a decoder chosen from an already-decoded value),
`Decode.one_of` (first decoder that succeeds — decode a union), `Decode.succeed`/`Decode.fail`. **Every
arrow is `pure`** — decoders are a pure, total sublanguage, so composing *and running* them introduces no
effect even though the runner parses JSON; a `let pure` over `Decode.decode_string` type-checks, where the
raw `extern json.loads` boundary is conservatively `io`. Representation: a `Decoder a` is a Python callable
`parsed -> a` that **raises** on a shape/type mismatch (the `Decoder` type erases, no runtime class); the
combinators build such callables (closures), and `decode_string` runs one inside `try`, catching any raise
into `Ok`/`Error(_Exception(kind, message))` exactly like the `try e` boundary form. Lowering routes each
member to an emitted `_pf_dec_*` helper (built from the IR, not string-spliced — the strict primitives use
the new general `PyStmt::Raise` node and the `is` comparison for `null`). User-registered combinators
stay deferred.

**JSON as data — the `Json` type.** A decoder turns JSON into the program's own types, which is right
where the shape is known. A tool that *edits* a file has to carry the parts it does not understand and
write them back as they were, and some JSON has no shape worth a type. For those there is one built-in
sum type:

```pyfun
type Json =
  | JNull
  | JBool bool
  | JInt int
  | JFloat float
  | JStr str
  | JList (List Json)
  | JObject (Map str Json)
```

`Decode.value : Decoder Json` takes any JSON as it is, and `Encode.value : Json -> str` writes a value
back. The round trip keeps what a JSON document says: an int stays an int and a float a float (`JInt 1`
and `JFloat 1.0` are different values, and `1` and `1.0` different texts), and an object keeps its keys
in the order they came, since a `Map` is a Python dict. It does not keep the document's spacing or its
string escapes. `Json` is a sum type to the rest of the language: `match` on it is checked for
exhaustiveness over the seven constructors, values are built with them, `==` is structural, and the
`Map` and `List` modules work on what `JObject` and `JList` hold. It has no order (`<` on two `Json`
values is an error, as it is on a `Map`). **Why constructors with a `J`.** They are bare names, like
`Some` and `Ok`, so they share the program's one constructor namespace; `Null`, `Bool`, `List` and
`Str` would take names a program wants or the language already uses. The names are reserved: a
program's own `type Json` or `JNull` constructor is an error that says the name is built in.
**Derived codecs.** A `Json` inside a type the derived codecs handle travels as the JSON it stands for,
so `type Doc = { id: int, extra: Json }` reads and writes `extra` untouched, and `Encode.auto` on a
`Json` value gives what `Encode.value` gives. **Representation.** One class per constructor, exactly
as a user sum type lowers (`JInt(1)`, `case JList(xs):`, `JNull` built once as `_JNull`), emitted only
in a file that builds or matches a `Json` or uses `Decode.value` / `Encode.value`, and placed in the
shared `_pyfun_rt.py` of a project so the type is one type across its files. `Decode.value` is a
helper that wraps the parsed document (a `bool` is tested before an `int`, since Python's `bool` is
one), so reading costs a copy of the tree. The alternative, a `Json` that *is* the parsed Python
value with no classes, was turned down: `1 == 1.0` and `True == 1` hold in Python, so equality and
`match` would not tell `JInt` from `JFloat` from `JBool`. The derived encoder tells a `Json` value by
its class's name, which the reservation makes sound, so a program without `Json` carries none of its
classes. Typed output names the type by an alias, `type Json = JNull | JBool | …`.

**Browser target — `pyfun bundle`.** The playground already runs the compiler under WASM and
CPython under Pyodide; `pyfun bundle <entry> -o <dir>` ships a *program* the same way: a static
page (`index.html`), the compiled Python (one `main.py`, or a project's whole `.py` tree), any
`--asset` files, and a loader that boots Pyodide from its CDN (the release the playground pins),
stages the files into the in-browser file system, and runs the entry with stdout and stderr in two
`<pre>` panes; `--page <fragment.html>` puts the program's own markup above them. A shareable link
with no server, which is how two players get a game whose interface is the browser. The page is
reachable through Pyodide's `js` module like any Python library: `extern import js`, then
`extern by_id: str -> Element = js.document.getElementById`, instance-access externs for methods
(`.replaceChildren`, `.addEventListener`), and `pyodide.ffi.create_proxy` for a callback the page
holds (a lifetime detail a façade hides). The cookbook's `browser/` entry is that façade, a
`Dom` module a program imports, with a counter page as its consumer. Two things fall out of work
already done: a click handler is an ordinary one-parameter Pyfun function (Python calls it with the
event, §6's calling convention), and a JS `Promise` is awaitable under Pyodide, so an extern over a
JS async API is typed `-> Async a` and awaited with `let!` like any other, no bridge to build. Since
an `Async` is cold (§8), the JS call is made at the await, so requests meant to overlap go to
`Async.parallel` together.
Signalling for a peer-to-peer transport is the program's problem, not the language's.

**Derived codecs — `Decode.auto` and `Encode.auto`.** A program that speaks to itself over a wire
(or a save file) has the same Pyfun type on both ends, and writing the two halves by hand is where
they drift. `Encode.auto : a -> str` and `Decode.auto : Decoder a` are derived from the type, so
both ends are one line and the property the tests state once is `Decode.decode_string Decode.auto
(Encode.auto v) == Ok v` for every type the derivation accepts: the primitives, records, sum types,
tuples, `List`, `Set`, `Map`, `Option`, `Result`, `Json` (which travels as the JSON it stands for,
above), newtypes (erased, so the wire carries the underlying value), and any recursion through them. **The shape** is the convention serde
(`#[serde(tag = "type")]`), Pydantic discriminated unions and System.Text.Json share: a record is an
object keyed by its Pyfun field names; a sum-type case is `{"type": "Move", "fields": [...]}` with
positional payloads (`{"type": "Resign"}` for a nullary case; F#'s `{"Case", "Fields"}` is the outlier
and was not copied), and a case with named fields (§8.3) writes `fields` as an object keyed by them,
`{"type": "Rect", "fields": {"width": 2.0, "height": 3.0}}`; `Option` is `null` or the value; tuples and `List`/`Set` are arrays; a `Map` with
string keys is an object and any other key type is a list of `[key, value]` pairs; `unit` is `null`.
**Two mechanisms.** `Encode.auto` is a run-time helper: the emitted classes already carry a record's
field names and a case's class name (the same knowledge `__repr__` uses), so `_pf_enc_value` reads
the value's shape (`dataclasses.fields`, a keyword-mangled `class_` travelling as `class`) and
`json.dumps` the result. `Decode.auto` is **type-directed lowering after inference**: the checker
records every `Decode.auto` site with its instantiated `Decoder a`, resolves `a` once inference is
complete, and derives a [`Codec`] from the declarations (records' fields, constructors' payloads at
the instantiated type, newtypes read through); lowering turns it into a descriptor the emitted
`_pf_dec_auto` interprets (`("record", Point, [("x", ("int",)), …])`, `("adt", {"Move": (Move,
[("str",)])})`), user-declared types living in a per-module `_pf_codecs` table under their displayed
type so a recursive `Tree` is a `("ref", "Tree")` back into it. The derived decoder is a `Decoder`
like any other (a callable that raises on a mismatch, strict like the primitives, `unknown case Nope`
/ `expected an object, got list` as the message), so it composes with `Decode.field`/`map2`/`and_then`
and runs under `decode_string`'s `try`. An `a` still open at the site is an error there, with the ways
to pin it named; a function, an extern type, a lazy `Seq` or an `Async` have no JSON form and say so.
Because the codec is derived from the *resolved* type, a `Decode.auto` site's type variable stays
**weak** at `let`-generalization (OCaml's `'_a`, the same discipline a deferred field access already
follows), so `let back = Decode.decode_string Decode.auto wire` at the top level is pinned by the
`match` that follows rather than each use getting a copy that pins nothing. Cross-module types work
when the declaring module is imported directly (the descriptor names `rules.View`); a transitively
carried record says to import its module. Type classes stay rejected: this is one derivation for one
built-in decoder, not a mechanism for user-defined derivations.

**String interpolation — `f"..."`.** Python-style interpolated strings: an `f` prefix
(adjacent to the quote — `f "x"` with a space stays ordinary application, as in Python) with `{expr}`
holes holding **full Pyfun expressions**, and `{{`/`}}` for literal braces. The whole string is a
`str`; a hole may be **any type** — the emitted Python f-string stringifies it (so `f"{p}"` for a
record uses the generated `__repr__`, `Point { x = 1 }` → `Point(1)`), which is the one place Pyfun
relaxes to Python's implicit `str()` rather than requiring an explicit conversion. Holes' **effects
propagate** (`f"{impure x}"` is `io`). *Lexing* is the crux: the lexer (`lex_fstring`) splits an
`f"..."` into a `Tok::FStr(Vec<FStrPart>)` of literal chunks and holes, finding each hole's matching
`}` by balancing nested `{}` and skipping string literals, then **pre-lexes each hole's tokens with
absolute spans** (`lex_subrange`, bracket depth pre-set so no layout tokens leak in). The parser's
`parse_interp` re-parses those hole tokens with the ordinary expression grammar into
`ExprKind::Interp { parts: Vec<InterpPart> }`, so diagnostics and LSP navigation reach inside holes.
*Lowering* is 1:1: `PyExpr::FStr` emits a real Python f-string, holes verbatim, literal chunks with
their specials and braces re-escaped. This **targets Python 3.12+** (PEP 701), so a hole may freely
contain a string literal reusing the outer quote (`f"{String.contains "}" s}"`). **Self-documenting
debug holes `{x=}`** work as in Python: a single `=` as the hole's last non-whitespace character
echoes the raw hole text (everything the user typed, `=` and surrounding whitespace included) before
the value, so `f"{x = }"` prints `x = 3`. The `=` must be a genuine marker, not an operator tail —
the character before it may not be one of `=`/`!`/`<`/`>`, so `{x==y}`/`{x != y}`/`{x <= 1}`/`{x >= 1}`
stay ordinary holes. Resolved entirely at lex time (`debug_marker` in `lex_hole`): the echo joins the
preceding literal chunk and the hole's tokens exclude the marker, so parser, checker, lowering, and
emitter see an ordinary literal + hole (the value is `str()`ed like any hole, not Python's `repr`;
the pretty-printer renders `f"{x=}"` as the equivalent `f"x={x}"`). Multi-line `f"""..."""` works — see the triple-quoted-strings paragraph below. **Format specifiers (`:.2f`, `!r`) are a non-goal**: a format spec is an *unchecked, stringly-typed sublanguage* inside a string literal — the
compiler can't see into it, so `.2f`→`.3f` silently changes output, `.2f`→`.f2` only fails at runtime,
and nothing enforces consistency across call sites. That is exactly the stringly-typed footgun Pyfun
refuses elsewhere (float patterns, unchecked field access, unit mismatches), so blessing a format
mini-language would contradict "the compiler is the gatekeeper." The FP alternative is **centralized
formatting functions** — the **`Format` module** above (`Format.currency "£" 2 19.5<gbp>`,
`Format.percent`, `Format.fixed`) — defined once, checked at every call, changed in one place.
The plain-hole `f"{expr}"` interpolation stays; only the `:spec`/`!r` mini-language is excluded.

**Raw strings — `r"..."`.** A raw string suppresses escape processing, so backslashes are
literal — handy for Windows paths (`r"C:\Users\pyfun"`) and regex via `extern`. **Lexer-only, no AST /
type / lowering change:** an *adjacent* `r"` (like `f"`; `r "x"` with a space stays `r` applied to a
string) opens `lex_raw_string`, which reads to the closing `"` **without** decoding escapes, following
Python's raw-string rule — a `\` keeps *both* itself and the following character literal, so `\"` is two
literal characters that do **not** terminate the string (`r"a\"b"` is the four chars `a \ " b`), and a raw
string cannot end in a lone backslash-before-quote (it just continues). It produces an ordinary
`Tok::Str` holding the raw content; from there it is an ordinary string literal, and the emitter's
existing `string_literal` escaper re-escapes on output (`C:\path` → Python `"C:\\path"` → reads back as
`C:\path`), so the round-trip is faithful with zero downstream changes. Combined `rf"..."` (raw +
interpolated) is out of scope.

**Triple-quoted (multi-line) strings — `"""..."""`, `f"""..."""`, `r"""..."""`.**
Python's multi-line string forms: embedded newlines (and lone `"`/`""`) are literal content, and only
`"""` terminates. **Lexer-only for the plain and raw forms** (the raw-string model): the string-opening
dispatch checks for **exactly three quotes at the open** — `""` stays the empty string (a following `"`
opens a new literal, Python's disambiguation rule), `""""""` is the empty triple string, and `"""`/`""""`
with no close are unterminated. `lex_string`/`lex_raw_string`/`lex_fstring` each take a `triple` flag
(shared `quotes_at`/`at_triple_quote` helpers): plain `"""..."""` processes escapes exactly like `"..."`
(Python's non-raw triple-quote rule, via the shared `lex_escape`), `r"""..."""` keeps backslashes
literal, and adjacency still gates the prefixes (`f """…"""` is application). `f"""..."""` reuses the
whole hole-splitting machinery unchanged — `{expr}` holes, `{{`/`}}`, `{x=}` debug holes, PEP 701
nested quotes (and `skip_string_in_hole` now skips a *nested triple* string so its braces can't
unbalance a hole). All three produce the ordinary tokens (`Tok::Str` / `Tok::FStr`), so there is **no
AST / type / lowering change**. **Offside:** the whole literal is consumed in one `lex_one` call, so
its internal newlines never reach the layout rule — no `Sep`/`Indent`/`Dedent` can leak from inside a
string, and a `"""` literal inside a `let` block leaves the block structure intact. **Emission is the
escaped single-line form** (`"a\nb"`, via the existing `string_literal`/`fstring_literal` escapers),
*not* a Python triple-quoted literal: the emitter is line-based (every statement line is indented to
its block depth), and a real multi-line literal would force unindented continuation lines through that
model — while `"a\nb"` is value-identical, self-contained, and keeps one escaping path. The Pyfun
pretty-printer likewise prints the escaped `"a\nb"` form, so the parse→print→parse roundtrip holds on
value equality. `rf"""…"""` is out of scope (as is `rf"…"`).

**`try` — catching exceptions into a `Result`.** Pyfun's own code never raises (it returns
`Error`); the only reason to catch is the **Python FFI boundary** — an `extern` call can throw. So rather
than importing Python's imperative `try/except/finally/raise` (and an exception class hierarchy Pyfun has
no types for), the feature is a single **expression**: `try e : Result <e> Exception` (`ExprKind::Try`). It
runs `e`, and a thrown Python exception becomes `Error`; success is `Ok`. `try` is a prefix keyword binding
looser than `+`/comparison but tighter than `|>`, so `try parse s` is `try (parse s)` while `try parse s
|> Result.with_default 0` pipes the `Result` out; parens capture a wider body. The body's `io` still
happens (`try` catches a *thrown* exception, it doesn't suppress the call); the one effect it takes
away is `exn`, below. The `Error` payload is a **reserved built-in record `Exception`** with `error_kind : str` (the
class name, `type(e).__name__`) and `error_message : str` (`str(e)`) — read by field access (`e.error_kind`)
or matched (`case Error (Exception { error_kind = "ValueError" }): …`). It reuses the ordinary record
machinery (so its two fields join the global field registry, and a user `type Exception` is rejected as
reserved), lowered to a class emitted as **`_Exception`** so it does not shadow Python's builtin
`Exception` that the handler catches. Lowering reuses the `PyStmt::Try` node (extended with an `as <name>`
binding): `try:  t = Ok(<body>)  except Exception as e:  t = Error(_Exception(type(e).__name__, str(e)))`.
There is deliberately **no `raise`, no `finally`, no exception hierarchy** (Pyfun signals failure with
`Error`; the `result {}` CE + `Result` module compose the rest). Enabled by **string-literal patterns**
(`case "yes":`, `Pattern::Str` — a refutable leaf over the infinite `str` type, so a string `match`
still needs a wildcard).

**`exn`: a call that may raise says so.** An exception is the one thing a Python call can do that its
Pyfun type did not show. The `exn` effect label shows it. It is **declared, never guessed**: an
`extern` carries it when its arrow says so, and from there it is inferred through every caller like any
other label.

```pyfun
extern pure to_number : str ->{exn} int = int
extern read_text : str ->{io, exn} str = pathlib.Path.read_text

let total texts = List.sum (List.map to_number texts)   # List str ->{exn} int
let safe text = try (to_number text)                    # str -> Result int Exception
let load path = try (read_text path)                    # str ->{io} Result str Exception
```

`try` is its **handler**, the second after `Task.scope` (§8): the exception raised while the body runs
becomes the `Error` of the result, so the `exn` the body performs stops at the `try` and the rest of
its effect passes through. `Async.catch` does the same for what is awaited. A function that `try` hands
back runs later, outside it, and keeps its own `exn`. `let pure` holds a function to not raising, and a
declared arrow without the label (`type Reader = Reader(str -> int)`) takes only a function that
cannot.

The label is opt-in at the boundary. An extern that does not mention it is typed as before, and may
still raise unseen; nothing in an existing program changes. The rule for an annotated arrow is also
unchanged: its effects are taken as written, so a file read that may fail is `->{io, exn}` and a
conversion that may fail is `->{exn}`. A program may perform `exn` at top level, as it may perform
`io`: an uncaught exception ends it with a traceback in its own lines (§9). Like every effect, the
label is erased from the emitted Python.

**Seq — the lazy module.** The `seq {}` CE produces a `Seq a` (a Python generator); the
`Seq` module is its lazy operation library, the counterpart to the eager `List`. `Seq.map`/`filter`/
`take`/`range` are **lazy** (they route to Python's own lazy `map`/`filter`/`itertools.islice`/`range`,
*not* the eager `_pf_*` wrappers `List` uses); `Seq.fold`/`to_list` force the sequence (`Seq.fold` reuses
the list `_pf_fold` = `reduce`; `Seq.to_list` = `list`). `Seq.of_list` = `iter`. `Seq.map`/`filter`/`fold`
are effect-polymorphic like `List.map` — since the effect system can't model *deferred* effects, the
function's effect is attributed at the call (sound, slightly conservative for the lazy ops). Caveat:
Python iterators are **single-pass**, unlike F#'s re-enumerable `seq` — consistent with the one-shot
generator the `seq {}` CE already produces.

The module splits cleanly in two, and each member's hover documentation says which half it is in,
because single-pass makes the difference matter. **Lazy** (a new sequence, nothing pulled yet):
`drop`, `take_while`, `drop_while`, `concat` (append, as in `List.concat`), `flatten`, `collect`, `zip`,
`indexed`, `distinct`, `pairwise`, `init`, `init_infinite`, `unfold`. Most route to Python's own lazy
machinery (`itertools.islice`/`takewhile`/`dropwhile`/`chain`/`pairwise`/`count`); `distinct` and
`unfold` are emitted **generators**, since they carry state while staying lazy. **Consuming** (each
pulls, and what it pulls is gone): `head`, `is_empty`, `len`, `exists`, `forall`, `contains`, `sum`,
`iter`, `find` — several short-circuit, so they consume only as far as the answer.

`init_infinite` and `unfold` are the members with no list behind them at all, which is the reason the
lazy module exists: `Seq.init_infinite (fun i -> i * 2) |> Seq.take 5 |> Seq.to_list` terminates, and
`Seq.find` over an endless sequence stops at the first match. Anything that forces the whole sequence
(`len`, `sum`, `to_list`) does not terminate on one, which its documentation says.

**Set and Map — the traversal half.** Both started as containers with membership and construction
but no way to *walk* them, so transforming either meant a round trip through `to_list`. They now carry
`is_empty`/`map`/`filter`/`fold`/`exists`/`forall`/`partition`, plus `is_subset`/`is_superset`/`max`/`min`
for `Set` (the last two `Option`-answering and `comparison`-constrained, like `List.max`) and `union`
for `Map` (the second map winning a shared key). Every `Map` member takes the **key and the value**,
since a map's element is the pair, and walks `m.items()` so each entry is read once — a loop over keys
with `m[k]` inside would hash every key twice. `Set.map` can return a *smaller* set, since two elements
may map onto one, and its result type is free (`Set a -> Set b`); `Map.map` keeps the keys and frees
only the value type (`Map k v -> Map k w`). `Set.fold` walks in Python's set order, which is not sorted
— a fold whose result depends on order should sort first. All the higher-order members are
effect-polymorphic like `List.map`.

**String — the text module.** Beyond the operations already there, the sweep added `is_empty` (the same
name every other container uses), `get` (one character by index, bounds-checked and `Option`-answering
exactly like `List.get` — there is no `char` type, so a character is a one-character string), `repeat`,
`lstrip`/`rstrip` (`strip` does both ends), `split_lines`, `rev`, and `of_list` (the inverse of
`to_list`). Padding is deliberately *not* here: `Format.pad_left`/`pad_right` already take a width and a
fill character, and a second spelling in `String` would be one way too many.

**Option and Result — combining and bridging.** Both had `map`/`bind`/`with_default` and the
predicates, which cover one value at a time, and now carry the members for *two* values and for
crossing into other types. `map2` combines a pair (`Some` only when both are; `Ok` only when both are,
otherwise the **first** `Error`, so the earliest failure is the one reported) — the shape for two
independent lookups without nesting two matches. `or_else` takes the **fallback first**, matching
`with_default`, so it reads as `primary |> Option.or_else fallback`. `iter` runs a function for its
effect on a payload that may not be there, `to_list` turns either into a zero-or-one-element list, and
`Option.flatten`/`Option.exists` finish the set. Every member of both modules is O(1) — they hold one
value, so there is nothing to traverse and nothing to say about cost. All the higher-order members are
effect-polymorphic like `List.map`.

**Effects through a multi-argument callback.** A function's latent effect lives on its **innermost**
arrow (§4), so a higher-order scheme must put the effect variable there and nowhere else. Writing
`(b ->{e} a ->{e} b)` looks more general but is strictly *less*: a pure folder forces `e` pure on both
arrows and an impure one wants `e` pure outside and `io` inside, so nothing effectful ever unified.
The FSharp.Core audit found every multi-argument member carrying that shape — `List.fold` among them,
which meant a fold could never print. They now read `(b -> a ->{e} b)`, which accepts both and lets the
effect flow out to the call, as the one-argument members (`List.map`) always did.

**Modules — qualified namespaces.** Collection operations are **module-qualified** (`List.map`,
`Set.add`, `Map.find`, `Option.with_default`, `Seq.take`). This is what lets `len`/`contains`/`map`
reuse one name across collections without overloading or type classes (which the MVP rules out). The
built-in modules (`types::MODULES` = `List`/`Set`/`Map`/`Option`/`Result`/`Seq`/`String`/`Format`; members paired in
`MODULE_PRELUDES`) and **user-declared in-file modules** (below) share one access syntax. The access
mechanism needed **no parser change**: `Module.member` is parsed as the ordinary field-access node
`Field { base: Var("List"), name: "map" }`; `types::qualified_name` recognizes an **uppercase** base
(value identifiers are lowercase, so `Upper.x` is only ever module access — a record-field base is a
lowercase value), and the checker + lowering resolve the dotted member against the module instead of as
record-field access. A genuinely global handful stay unqualified (`print`/`input`/`abs`/`min`/`max` in
`PRELUDE`), matching F# (`List.map` qualified, `abs` global). `input : str ->{io} str` is
Python's `input(prompt)` name-for-name (`input ""` for promptless) — the io effect coming *in*,
pairing with `print`'s going out; `input "n? " |> String.to_int : Option int` is the idiomatic total
parse. Environment caveats: the REPL's worker protocol runs over stdin, so `input` inside the REPL
consumes protocol bytes (the documented stdin hazard), and the Pyodide playground cannot block on
stdin — scripts via `pyfun run` (and Jupyter, whose kernel routes stdin) are its home turf. An unknown member gets a **"did you
mean"** hint — `` `startswith` is not a member of `String` (did you mean `String.starts_with`?) `` —
computed by `closest_member` (a case-insensitive match first, then edit distance ≤ ~⅓ the name, then a
prefix relation for abbreviation slips like `length`→`len`). It scans the env's qualified keys, so it
serves built-in *and* user modules, and rides the shared inference path so it surfaces in `pyfun check`
*and* LSP editor diagnostics. Names stay **single-spelling** (no camelCase/lowercase aliases) — casing is
load-bearing (`Upper.x` vs `lower.x`), so the hint is the forgiving path, not a second accepted name.

**In-file modules.** `module Name = <indented let bindings>` declares a namespace within
a file (`Item::Module`):
```
module Geometry =
  let pi = 3.14159
  let area r = pi * r * r          # siblings visible unqualified inside
let big = Geometry.area 10.0       # qualified outside
```
Members are typed in a cloned scope: each sees prior siblings **unqualified** (and qualified), but only
`Name.member` escapes to the outer env — so the bare names are not visible after the module. Lowering
flattens members to top-level defs/assignments with **mangled names** (`Geometry.area` → `Geometry_area`),
rewriting bare sibling references to the same names (`cur_module` in the lowerer); partial application
and the curry policy work unchanged (arity is registered under the qualified name). A module name can't
shadow a built-in module or duplicate another. **MVP limits:** the body holds only `let` bindings
(`type`/`measure`/`extern` inside a module are deferred), and there are no nested modules. The next
layer is the full *file-based* module system, scoped in §6.1.

### 6.1 File-based modules

One module per file, referenced with an explicit `import`, compiled to a tree of readable Python files.
a runnable example lives in
`examples/modules/`.
The design optimizes for **Python familiarity** (the §7.1 theme): explicit imports, real Python modules,
no enforced visibility. All four shaping decisions were taken deliberately:

- **Explicit `import`, qualified use.** `import Geometry` declares the dependency edge; members are used
  qualified, `Geometry.area`. This is the core Python idiom (`import foo; foo.bar()`) and **reuses the
  existing access machinery unchanged** — `Geometry.area` is already the `Field { base: Var("Geometry"),
  name: "area" }` node that `types::qualified_name` resolves off an uppercase base. The *access* needs no
  parser change; only the `import` *statement* is new (`Item::Import { name, span }`, a new `import`
  keyword, the name a single capitalized identifier). **The syntax** lexes,
  parses (as an ordinary top-level item), pretty-prints, and round-trips; it is a no-op in single-file
  checking and lowering until the multi-file driver resolves it. (Enforcing "imports before
  other items" is left to that driver, matching the cross-file declare-before-use rule.)
  `open` (import everything, unqualified) is **not offered**: it maps to Python's discouraged
  `import *`.
- **`from Geometry import area, Circle`: names written bare.** The statement is Python's and follows it
  strictly. It binds the names it lists (values, constructors, record tags; a type name may be listed
  too, though a type is already one flat namespace across the project) and nothing else: the module
  itself is not bound, so `Geometry.pi` still needs `import Geometry`, and the two lines can stand
  together. There is no `import *`, which would bring in names the file never shows. A local binder of
  the same name (a parameter, a `let`, a pattern variable) shadows the import, as in Python; a
  top-level definition of the same name is an error, as is the same name from two modules, since one
  name means one thing in a file. A listed name the module does not have is an error, and one nothing
  uses is a warning. `from` is a keyword only at the head of this statement, ahead of a module name.
  It is a rewrite ahead of everything else (`imports.rs`): each bare use becomes the `Geometry.area`
  it stands for, at the same span, so the checker, the lowering and the editor see a program written
  out in full. The emitted Python is therefore the same as for the qualified form, `import geometry`
  and `geometry.area(...)`, and go-to-definition, find-references and rename follow a bare name to its
  module.
- **Packages: `import Shapes.Geometry`, used as `Geometry.area`.** A module in a folder is named by its
  path from the top of the project, each part capitalized, so `import Shapes.Geometry` reads
  `shapes/geometry.pyfun`. The path is written once, on the import line, and the code then uses its
  last part: `Geometry.area`, `Geometry.Circle(r)` in a pattern, `Geometry.Shape` in a type, exactly as
  it reads for a module at the top. `from Shapes.Geometry import area` takes the same path. Python's
  own `import shapes.geometry` has every use say `shapes.geometry.area`; what its programmers reach for
  is `from shapes import geometry`, which binds the last part, and that is the binding this statement
  makes. Moving a module into a folder is then a change to one line of each file that imports it.
  Since the last part is the name in the code, a file imports one module by each last name. A second
  `import Solids.Geometry` is an error that points to `from Solids.Geometry import …`, which binds no
  module name and can stand beside the first. Writing the whole path at a use (`Shapes.Geometry.area`)
  is an error that gives the short form.
  **The top of the project is the folder of the file being run**, as the script's folder is for Python,
  and every module names its imports from there: `shapes/geometry.pyfun` imports its neighbour as
  `import Shapes.Util`. There are no relative imports. Two mistakes get a note of their own: a
  neighbour imported by its bare name (the note gives the full path to write), and a file inside a
  package checked on its own (the note names the folder its imports are found from).
  **Output.** `shapes/geometry.py` beside an empty `shapes/__init__.py` (a regular package, which is
  what pytest and mypy expect of a folder of modules), `import shapes.geometry` at the head of an
  importing file, and `shapes.geometry.area(box)` at each use. When a binding of the file takes the
  package's name (`let shapes = …`), the import goes by an alias, `import shapes.geometry as
  _pf_shapes_geometry`, as it does for a flat module.
  **How.** Past the parser a module's name is its dotted path: in the project graph, in the checker's
  environment (`Shapes.Geometry.area`), in the lowering context and in messages. The rewrite that
  serves `from` imports (`imports.rs`) also writes each short name out in full, so nothing downstream
  knows a short name exists, and a `Module.member` string is split at its last dot.
  `project::module_stem`, `module_file_name` and `module_py_name` turn the name into `shapes.geometry`,
  `shapes/geometry.pyfun` and `shapes/geometry.py`.
- **Parallel `.py` output.** Each `foo.pyfun` compiles to a sibling `foo.py` with a real `import`; member
  names stay **un-mangled** (`area`, not `Geometry_area` — the mangling is an in-file-module workaround we
  drop here), and a cross-module reference `Geometry.area` lowers to Python `geometry.area`. This matches
  Python expectations and the "readable Python / direct ecosystem interop" ethos, and enables
  Python↔Pyfun interop (a Python program can `import` a compiled Pyfun module and vice-versa).
- **All public.** Every top-level binding is exported; no `pub` keyword — Python has no enforced private
  (`_underscore` is convention only). Visibility control is **deferred**.
- **Implicit recursion** (independent of the rest): a *function* binding (`let f x =
  …`) is in scope in its own body, like Python's `def` — no `rec` keyword (an F#/ML `let rec f x = …` is
  rejected with a hint to drop `rec`, rather than silently binding a function named `rec`). A plain value
  binding still cannot self-refer (`let x = x` stays an error, as `x = x` is a module-level `NameError` in
  Python). Mechanism:
  pre-bind `f : α` (fresh) before inferring the body, unify, then generalize (standard monomorphic-
  recursion HM); lowering is unchanged (Python functions are already recursive). **Mutual recursion**
  extends this to *groups*: `run` builds the dependency graph among top-level `let` bindings
  (scope-accurate free variables, `collect_free`), finds cycles by SCC (`strongly_connected`), and infers
  each all-function cycle together (`infer_mutual_group`) — pre-bind every member monomorphically, infer
  all bodies (so `is_even` sees `is_odd` and vice versa, in any order), tie each knot, then generalize each
  against the *outer* env. Grouping only genuine cycles keeps let-polymorphism intact (an independent
  helper stays its own singleton SCC and generalizes normally). It's **implicit — no `and` keyword**
  (which would clash with the boolean `and`). A value cycle (`let a = b\nlet b = a`) is not a function
  group, so it stays rejected; and a one-way forward reference between *independent* (non-cyclic) bindings
  still requires declare-before-use. Lowering is unchanged (Python module-level `def`s resolve names at
  call time). **Lines between a cycle's members.** A cycle cannot run until its last member is defined,
  so everything written above that line is ready for it: a function, a value or an in-file module
  between the members is in scope for the whole cycle (`plan_items` finds what the cycle reads there, and
  `TopItems` infers it ahead of the cycle). Such a name still enters scope for every *other* line at the
  place it is written. The cycle's own names are in scope from its first member on, but Python defines
  each function where it is written, so a top-level value or expression between the members that uses one
  of them is an error (`is_even` cannot be used yet: it needs `is_odd`, which is defined below this
  line); it would be a `NameError` when the program ran. A function written between the members may call
  the cycle, and then waits with it: a value that uses that function before the cycle is whole gets the
  same error. The check reads names, not calls, so a line that only hands the function on (`let table =
  [is_even]`) is rejected too and has to move below the cycle. **Tail-call optimization is a non-goal** (below) — CPython does no TCO and caps recursion
  (~1000 frames), so deep recursion can `RecursionError` exactly like hand-written Python; the
  **stack-safe path is the `List`/`Seq` combinators** (they lower to Python's iterative `reduce`/`map`).

**Module identity.** Source files are lowercase (`geometry.pyfun`), avoiding case-insensitive-filesystem
pitfalls, and a multi-word one is snake_case (`train_times.pyfun`), the way PEP 8 names a module. The
**module name is the stem in PascalCase**, each underscore-separated word capitalized (`Geometry`,
`TrainTimes`), per Pyfun's uppercase-identifier rule for types/modules. Resolution maps a name to its
stem with the same snake_case rule every value name follows (§6), so `import TrainTimes` reads
`train_times.pyfun`, emits `train_times.py` and lowers a reference to `train_times.next_departure`.
One function (`project::module_stem`) serves all three, so the source file, the emitted file and the
Python `import` cannot disagree. An underscore before a digit is kept in the name (`day_1.pyfun` is
`Day_1`), which keeps the mapping reversible. Before 0.11 the stem was only lowercased, so
`import TrainTimes` read `traintimes.pyfun`; a missing-module error names the file it looked for.
A folder is named the same way, so `import Rail.TrainTimes` reads `rail/train_times.pyfun`.

**Resolution & ordering.** A multi-file **driver**: from an entry file,
parse it, follow `import` edges (resolved relative to the entry's directory = the source root), and build
a dependency **graph**. The graph must be **acyclic** — a cycle is an error (Python tolerates import
cycles only fragilely; F# forbids them, and this is the cross-file face of declare-before-use). A
topological sort gives the compile/emit order. So "a module may only use modules declared before it"
falls out for free — no separate mechanism, and there is **no mutual recursion across modules** (merge
the files, as in F#).

**Cross-module checking.** Each module is
type-checked in topological order, its env seeded with every imported module's **exported value schemes**
under their qualified keys (`env.insert("Geometry.area", scheme)`) — reusing the qualified-key env the
checker already uses for built-in/in-file modules, so the existing `Field`-node access path resolves a
cross-module reference with no new lookup logic. A module's interface is its top-level **`let`
values** plus its **sum types** (since the cross-module-ADT follow-on; `ModuleExports` carries each public
sum type's name, arity, and constructors) **and its records** (since the cross-module-record follow-on;
`ModuleExports` also carries each public record's name + fields) **and its opaque handle types** (name +
arity, so `extern type Rng` can be named across the boundary). A consumer can construct (`Geometry.Circle
2.0`) and pattern-match (`| Geometry.Circle r ->`, a qualified constructor pattern) the imported type's
values, with **exhaustiveness checked across the boundary** (a missing arm reports the qualified witness
`Geometry.Rect _ _`). **Records cross too** (`DESIGN.md` §8.3): construct `Geometry.Point { x = 1, y = 2 }`,
pattern `case Geometry.Point { x, y }:`, update `{ p with x = 3 }`, and bare-access `p.x` on an imported
value — the record class is emitted once (in its module) and referenced as `geometry.Point`. **An imported
type can be *named* in a local `type` declaration** — `type Holder = { item: Shapes.Placed }`, and likewise
an ADT variant payload or an `extern` signature. Both spellings are accepted, bare `Placed` and qualified
`Shapes.Placed`, and they denote the same type: an imported type registers under its bare identity name
(unique across everything visible, as the import clash check enforces) plus a qualified key, and a written
qualifier is validated and then folded back to that identity, so the two unify freely. Prefer the qualified
spelling where the reader benefits from knowing which module a type came from. **Opaque handle types
(`extern type Rng`, §6) cross like any other type name:** the interface carries the name + arity, so a
consumer can write `Gen.Rng` (or bare `Rng`) in a record field, an ADT payload, or an `extern` signature;
values of the type always crossed through the exported schemes. **Interfaces close transitively over what
they reference:** a record or opaque type that an exported scheme, constructor, or record field mentions is
carried in the exporter's interface, tagged with the module that declares it, and the pull repeats through
each hop (a worklist walk with a seen set, so mutually referencing records terminate). A third module
importing `Holder` without importing `Shapes` can therefore hold its `item`, read that value's fields, and
name its type bare (`Placed`). A carried type crosses as an *identity*, not as a member: constructing it,
pattern-matching it, or writing the qualified spelling still requires importing the declaring module
directly. A carried name already taken here (by a local type or a direct import) is skipped silently rather
than reported, since the consumer never wrote that name: local and direct declarations win the bare name,
the same record arriving along two import paths is recognized by its declaring module and admitted once,
and a genuinely different type shadows the carried record, whose fields then feed the "unknown record
field" diagnostic (the message names the hidden record and the module that declares it). Field lookup
mirrors the same precedence: in a by-name field access (a base whose type is not yet known), records
declared here or imported directly form the deciding tier, and a carried record owns a field only when no
record in that tier declares it, so editing a dependency two hops away can never make a working access
ambiguous. Two carried records that alone declare a field still tie, and the ambiguity message names
them. **Externs and
measures cross too:** an imported `extern` (`Mathx.cbrt`) is exported like a value (its scheme joins the
interface) and — in the project lowering path — also **bound at top level in its own module** (`cbrt =
math.cbrt`, `import math` hoisted) so a dependent module references it as `mathx.cbrt`; single-file lowering
still erases externs (references inline to their dotted target). **Measures** merge *unqualified* — there is
no qualified unit syntax (`<m>` is bare) — so a shared `Units` module's `measure m`/`measure s` and its
derived aliases become available wherever it is imported; a base measure re-imported under the same name is
idempotent (the shared-`Units` pattern), while the *same alias name mapped to a different expansion* across
two imports is a genuine conflict and errors. (Measures erase at lowering, so a `<m>`-annotated cross-module
value round-trips to plain numerics with no lowering change.) Using a name a module does not export is the
ordinary "`x` is not a member of `Geometry`" error, located in the importing module.

**Output & the shared runtime.**
Each module lowers independently to its own `.py`; a cross-module `Geometry.area` emits `geometry.area`
(attribute access), with `import geometry` hoisted to the file header (reusing the lowerer's
`needed_imports` set), and imported members keep **un-mangled** names (a file module is lowered as an
ordinary top-level program — the `Geometry_area` mangling is only for *in-file* `module` declarations).
**One correctness constraint forces a shared runtime module:** the built-in `Option`/`Result`
constructors lower to *nominal* Python classes (`Some`/`None_`/`Ok`/`Error`); if each file defined its
own, an `Option` value crossing a module boundary would fail the receiver's `isinstance`/`match` checks.
So those classes are hoisted into a generated **`_pyfun_rt.py`** that every module needing them imports
(`from _pyfun_rt import Some, None_` / `Ok, Error`). `List`/`Set`/`Map`/tuples need no runtime — they are
native `list`/`set`/`dict`/`tuple`. The pure `_pf_*` helpers stay per-file for the MVP (duplication is
bloat, not bug); de-duplicating them into `_pyfun_rt.py` is a follow-on. **Single-file `compile`/`run`
output is unchanged** (`lowering::lower` still inlines the classes) — the runtime module appears only for
a multi-file program, and only when some module actually uses `Option`/`Result`.

**CLI.** `pyfun {check,compile,run} entry.pyfun` operate over the whole
graph: `check` checks all modules (errors rendered rustc-style against each module's own source, grouped
under a `-- module `Name` (name.pyfun) --` header); `compile -o <dir>` emits the `.py` tree (+
`_pyfun_rt.py`) into `<dir>` (no `-o` prints each file to stdout under a `# ==== name.py ====` banner);
`run` materializes the tree to a temp dir and executes `python <dir>/entry.py` (the script's own directory
leads `sys.path`, so siblings resolve, and the *working* directory is left alone so the program's relative
paths still mean what they meant on the command line), then cleans up. Each command **detects imports** by
parsing the entry: a file with **no imports takes the single-file path** (`compile` to stdout / one file
with the classes *inlined*, `run` staging one `_pyfun_main.py` the same way), and only a file that actually
`import`s engages the graph driver. An error the program does not catch is reported in `.pyfun` lines (§9). `run` never pipes the program to `python -`: that would spend the
program's own stdin on its source text, so the first `input` in an interactive program would hit `EOFError`.
The compiler stays the gatekeeper: `compile`/`run` over a project gate on a clean `project::check` first.
Graph errors (missing file, cycle, a lex/parse failure in some module) are rendered before any checking.

**Built-in documentation.** Every built-in member carries a one-line description
(`types::MEMBER_DOCS`, keyed by the name a user writes), which the language server appends to hover
below the type and attaches to completion items alongside the rendered signature. Complexity is
stated wherever it is not obvious or not what a reader would assume — a `List` backed by a Python
list makes cost easy to misjudge (`List.contains` is a linear scan where `Set.contains` is O(1);
`update_at` copies). A user's own `##` doc always wins over a built-in of the same name. A test pins
the table to the prelude constants in both directions, so a new member cannot ship undocumented and a
renamed one cannot leave a stale entry behind.

**LSP.** The editor analysis gains **minimal import awareness**: `analyze_in_dir(source,
dir)` resolves an imported file's export interface (via `project::resolve_imports`, a *forgiving* variant
that reads sibling `<name>.pyfun` files, resolves transitively, and silently omits a missing/broken/cyclic
import) and seeds the type-check (`types::check_collecting_with_imports`), so a multi-module file
type-checks `Geometry.area` cleanly instead of flagging "not a member" — while a genuine cross-module type
error is still reported. The server maps the document's `file:` URI to a directory (`uri_to_path`,
percent-decoding + the Windows `/C:/` fixup) and passes it in; a non-`file:` URI or a no-imports file is
analyzed exactly as before. Both former MVP limitations here (disk-only reads, no invalidation when an
imported file changes) are fixed by the project-wide LSP cache (`INTERNALS.md`). *Cross-file navigation:*
(1) **go-to-definition crosses files** — a qualified reference to an imported file module
(`Geometry.area`, `Geometry.Circle`) jumps to the definition in that module's `.pyfun`
(`resolve::qualified_at` records expression-position qualified refs with spans; the server resolves the
sibling URI and locates the member via `resolve::definitions`, reading an open buffer over disk); (2)
**workspace symbols** (`workspace/symbol`) search every definition across the project directory's `.pyfun`
files; and (3) **find-references and rename for top-level `let` values *and* constructors** span the whole
project — `Server::symbol_occurrences` scans the directory's `.pyfun` files and collects the symbol's
definition, its bare uses in the defining file, and every qualified use (`Geometry.area`,
`Geometry.Circle`) elsewhere (rewriting only the member identifier, via `member_subspan`, so the
`Geometry.` qualifier is preserved). A constructor's uses include both construction expressions *and*
patterns: `Pattern::Ctor` and the `type` variant declaration each carry a name span, and the resolver
records pattern constructors in the same reference channels as the expression forms, so the occurrence set
is complete. Rename is sound: it fires only for a top-level value or constructor (a value renames to a
value, a constructor to a constructor), and a *strict* scan **refuses** rather than do a partial rewrite if
any project file fails to parse. **Type names** navigate and rename across the project too. A type is
written in annotations (sum-variant and record field types, `extern` types), bare or qualified
(`Shapes.Placed`), and a record's name is also its tag. The declaring module is found from the
occurrence under the cursor (a local declaration wins a bare-name clash, else the importing file's
imports decide), and `type_occurrences` collects the declaration and bare uses in the declaring file plus,
in each importer, qualified uses (member part only), bare uses where the importer declares no type of that
name, and a record's qualified tags. `TypeExpr::Con` and the `type` declaration each carry a name span,
and the resolver walks type annotations (`resolve::walk_type`) collecting uppercase-name occurrences. A
type renames to an uppercase type name; builtins are refused, and a document outside a project directory
keeps the in-file behaviour. The **project-wide LSP cache** is described in `INTERNALS.md`.

**The top of a project, for the editor** (`lsp/roots.rs`). The compiler is told which file is run; the
editor is told only which file is open, and that may be a module deep in a package. The server places
the top above the document's own folder on two kinds of evidence: what the document imports (`import
Shapes.Util` in `shapes/geometry.pyfun` is found one folder up), and who imports the document (a file
above that says `import Shapes.Geometry` and means this one, which settles a module that imports
nothing). The furthest of the two is the top. Every cross-file answer is given from there, with each
file named by its path from the top, and the scans behind find-references, rename and workspace symbols
walk the whole tree under it: bounded, inside the repository, past hidden folders and build output. A
file in that tree with a nearer module of the same path (a folder of separate programs beside the
project's modules) means that one and is left out. A file that never mentions the name being sought
need not parse for a rename to go ahead.

**Post-Phase-2 follow-ons (each detailed above):** cross-module sum-type ADTs, cross-module records
(§8.3), cross-module externs and measures, and cross-file LSP navigation. **Explicit non-goals
(decided not to build):** visibility (`pub`) — Pyfun is all-public by design, the Python-natural model,
so enforced visibility would fight the ethos; and **TCO** — CPython has none and the `List`/`Seq`
combinators are the stack-safe path, so deep self-recursion matching hand-written Python's
`RecursionError` is acceptable. **Still deferred (no demonstrated need yet):** relative imports;
de-duplicated `_pf_*` runtime.

### 6.2 Façade packages: `pyfun.toml`, `pyfun add`, `pyfun install`

The friction §6 leaves is distribution: a set of typed `extern` declarations for a library is
cheap to write (five to ten lines a library) but was copied between projects by hand. A **façade**
is those declarations published once as an **ordinary pip distribution** whose package data
includes `.pyfun` files, so the Python ecosystem's own tools (pip, uv, PyPI, private indexes)
carry them and Pyfun adds no registry of its own. `examples/facades/pyfun-textwrap/` is one.

A project is marked by a **manifest**, `pyfun.toml`, at its root:

```toml
[project]
name = "scrabble"

[dependencies]
pyfun-textwrap = "0.1.0"
```

`pyfun add <package>` (a name with an optional version specifier, or a local directory) installs
the package into the interpreter's environment (`uv pip install` when `uv` is on PATH, else
`python -m pip install`; `PYFUN_PYTHON` picks the interpreter, as it does for `pyfun run`), reads
the installed distribution's file list for its `.pyfun` files, copies them into
`<root>/.pyfun/facades/<dist>/`, and records the installed version in `[dependencies]`, creating
the manifest if there is none. `pyfun install` repeats that for every dependency listed, pinned to
its recorded version, which is how a fresh checkout or another machine gets the same façades, and
`pyfun remove <package>` drops the entry, deletes the vendored copy and uninstalls the distribution.

Resolution then gains one step: `import Name` looks for the sibling `name.pyfun` first (§6.1) and,
failing that, for the same file in each vendored façade directory under the nearest `pyfun.toml`,
so a project's own module always wins over a façade of the same name. Everything else is unchanged:
a façade module is a module in the graph like any other, type-checked, lowered to its own `.py` file,
and imported by the Python its dependents emit. **The compiler reads only the vendored copies**, so
checking and compiling need neither Python nor the network, and the vendored directory is safe to
commit for fully reproducible builds (or to leave out, since `pyfun install` recreates it).

A façade is ordinary Pyfun, so beside its `extern`s it can hold small functions that give a
Python API a pipe-friendly argument order (`let fill width text = fill_text text width`), which
keyword slots alone cannot, since a slot takes a trailing parameter. One naming rule matters: the
façade's module name must differ from the Python package it wraps, because the module lowers to a
`.py` file of its own name (`Wrap` becomes `wrap.py`, which would shadow a real `wrap` package).

The manifest reader is deliberately a small subset of TOML (`[table]` headers and `key = "string"`
entries, edited line by line so comments survive), keeping the compiler dependency-free. Deferred:
a lock file with hashes (the recorded version pins the façade, and pip's own resolver pins its
dependencies) and dependency resolution *between* façades.

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
| Comp. exprs    | none (ad-hoc `async`/gens)  | `async {}` / `seq {}` / `result {}` / `option {}` (§8) |
| Units          | none                        | units of measure, compile-time only (§8)      |

**Functions are curried by default** (F# style): `let add a b = a + b` has type
`int -> int -> int`, and `add 1` is a legal partial application of type `int -> int`. This is what
makes `|>` and point-free style pay off. Inference handles curried arrows and partial application
(standard HM); lowering keeps output readable via the n-ary-collapse strategy (§5); the Python
boundary stays n-ary (§6).

**A parameter may destructure**, so long as it cannot fail to match: a name, `_`, a tuple, or a
**record** pattern, over those, nested — `let tile_score (t, sq) = t * sq`,
`fun acc (a, b) -> acc + a * b`, `let const42 _ = 42`, `let describe (Cell { letter }) = letter`. A
record has exactly one shape, so naming a subset of its fields still matches every value of that
type. A parameter has no second arm to fall through to, which is why refutable shapes (a literal, a
list pattern, an or-pattern, an as-pattern) are rejected there with a message naming what was
written; match on the value in the body instead. **A constructor pattern binds there when its type
has no other constructor**, since it then cannot fail: `let unwrap (Wrap v) = v`, and the newtype
idiom `let raw (UserId s) = s` (F#'s single-case union, Haskell's irrefutable constructor). The
parser admits the shape and the type checker counts the constructors, because only it knows the
type: `let f (Some x) = x` is an error that names what else the value may be (`Option` also has
`None`). Positional and record-bodied constructors (§8.3) follow the one rule. An active-pattern
case is decided by running its recognizer, so it stays in `match`. Only a **parenthesized**
parameter takes the pattern grammar: a bare uppercase identifier stays a parameter *name*, so
`let f Some x = …` keeps meaning two parameters rather than silently becoming one constructor
pattern. Python 3 removed tuple parameters, so a destructuring parameter lowers to a synthetic
argument name unpacked on the body's first line (`def tile_score(_pf_arg0): t, sq = _pf_arg0`), one
statement per nesting level; a record parameter reads one attribute per field it names
(`letter = _pf_arg0.letter`) and nothing for the fields it does not; a constructor parameter reads
its fields the same way (`n = _pf_arg0._0` for a positional one). A destructuring *lambda*
therefore lowers to a named `def` rather than a Python `lambda`, which cannot hold a statement. A
newtype's pattern is the exception to all of this: its constructor is erased (§7.3), so the pattern
is rewritten to its payload before lowering and `let raw (UserId s) = s` is `def raw(s)`.

**A `let` binding may destructure too**, under exactly the rule above and for exactly the same
reason — `let (r, c) = parse_coord tok`, `let Point { x, y } = origin`, `let (a, (b, c)) = nested`.
Python spells this `a, b = f()`, and Pyfun emits precisely that: a tuple of plain names unpacks in a
single statement straight from the value, with no temp in the way. Anything deeper reads through a
reserved temp, one statement per level (`a, _pf_t0_1 = p` then `b, c = _pf_t0_1`), and a record
target reads one attribute per field it names. The target is held to irrefutability by the same
checks a parameter's is, so `let (Wrap v) = w` binds when `Wrap` is its type's only constructor and
`let (Some x) = …` is rejected with a message pointing at `match`, which has somewhere to fall
through to. The bracket-less `let Some x = …` would
otherwise parse as a *function* called `Some`, so a capitalized binding name is rejected outright:
it can only be a constructor written without its brackets (or a module name), never a value. Three positions take a target: a
top-level `let`, a block-local `let`, and a computation expression's `let`/`let!` — where a
destructuring bind is one unpacking statement from the payload `result` has already tested
(`r, c = x._0`). What cannot destructure is a binding that needs a single name to
*be*: a function binding (`params` are what make it one) and a `let mut` (whose name is what `<-`
reassigns) both reject a pattern target, each with its own message. Each name a destructuring
binding introduces is generalized on its own, the same let-generalization a single-name binding
gets; there is no value restriction to trip over, since `mut` bindings are monomorphic and named.
`let _ = e` is the degenerate case: it binds nothing and exists so a value-producing expression can
be run for its effect.

MVP language features: immutable bindings by default with checked `let mut`/`<-` and indentation
blocks (§3), expression `if`/`match`, **curried functions + partial application**, **pipe `|>`**, ADT
and **record** declarations, the computation expressions of §8, units of measure (§8), readable
Python output, and inferred effect tracking (§4).

Lexical conventions: line comments start with `#` (Python-style — `//` is floor division, §7.1);
identifiers are ASCII alpha + `_`; capitalized identifiers denote constructors in pattern position
(§ patterns).

**A type is displayed the way it is written.** Hover, the REPL, a hole's note and every message
print a type variable bare (`List.map : (a -> b) -> List a -> List b`), which is how an `extern`
signature and a `type` declaration spell one; the ML quote (`'a`) that releases before 0.11 printed
was a second notation for the same thing. A **unit** variable keeps its quote (`float<'u^2> ->
float<'u>`), because that is how a signature writes it: bare, it would read as a measure of that
name. A type application on the left of an arrow carries no parentheses, since application binds
tighter than `->` in the type grammar (`Map a b -> Option b`); a function there keeps them. And a
type inside a message is always in backticks (``expected `List a`, found `int` ``), like every other
piece of code a message quotes, so a bare `a` is never mistaken for the English word.

**A whole type is displayed with its constraints**, in front, the way this document has always
written them: `lt : comparison a => a -> a -> bool`, `add : num a => a<'u> -> a<'u> -> a<'u>`,
`has : a in b => a -> b -> bool`, and several in parentheses (`(comparison a, a in b) => a -> a ->
b -> bool`). Without them a hover says less than the checker knows: `a -> a -> bool` reads as "any
two values of one type", and the first call with a function argument is refused. "Whole" means the
places that show a binding's or an expression's type on its own: hover, the REPL's echo, `:type`
and a typed hole's note. A type quoted inside a message (``expected `List a`, found `int` ``) stays
bare, since a constraint belongs to a binding and not to a fragment of its type.

A **`num` variable is named only when the type is a function of it**, that is when it occurs to the
left of an arrow in a type a binding was generalized over: `twice : num a => a<'u> -> a<'u>`, since
the caller picks `int` or `float`. A number that only comes out displays as its default (`let x = 5`
is `x : int`, `twice 3 : int`, `[1, 2] : List int`), which is what the value is unless something
asks otherwise, and what a reader of lesson 1 should see. So does a literal's variable inside a body
(the lambda in `List.map (fun x -> x + 1) xs` hovers as `int -> int`), and so do the numbers in a
typed hole's note: `List.fold ? 0 ns` asks for `int -> int -> int`, because those numbers are the
hole's context's and not the hole's to leave open. A hole's note does carry `comparison` and `in`.
Unit variables are lettered from `'u` and type variables from `a`, so the two never share a letter
(`a<'u>`, never `a<'a>`).

**A Python habit is answered with the Pyfun form.** The audience arrives with Python in its
fingers, and most Python-shaped mistakes are errors anyway, at a token that says little (`expected an
expression, found =`). So the compiler reads the shape and names the form that replaces it: `x = 5`
(`let x = …`, or `x <- …` for a `let mut`), `x += 1`, a `for` or `while` statement, `class`, `def`,
`import math` and `from … import` (`extern import`), `lambda x: …` (`fun x -> …`), a `{x:.2f}` format
spec (the `Format` module), `xs[0]` (`List.get i xs`), `s.upper()` (`String.upper s`), a call written
`f(a, b)` (`f a b`), the conditional expression `a if c else b` (`if c then a else b`), and a member
called by another language's word (`Map.get` is answered with `Map.find`, `List.append` with
`List.concat`). Two rules keep this honest. A hint needs the whole Python shape, so a program that
merely names something `lambda` or `for` is untouched; and a type-directed hint fires only on a
program that was already wrong, so `xs[0]` is answered only when `xs` is a collection and the
application has failed, with the literal hard against it (`f [0]` is an ordinary argument).

**The string type is `str`.** Python's name, and the name the emitted annotations already use, so
`type User = { name: str }` reads the same on both sides of the boundary. The decoder follows the
type (`Decode.str`); the module of text operations stays `String`, a module name like `List`. The
`string` of releases before 0.11 stays reserved in type position and is an error naming the fix
(``Pyfun spells this type `str`, as Python does``). Reserving it is what keeps old code from
compiling to something else: a lowercase name the checker does not know is a type *variable* in an
`extern` signature (§6), so `extern size: string -> int` would otherwise accept every argument type.

**Booleans are `True` and `False`.** They are spelled as Python spells them, which is also how
Pyfun's own rule spells them: `bool` is a type with two constructors, and every constructor is
capitalized. Source, the emitted Python, a printed value and a witness in a diagnostic
(`` `Some False` is not matched ``) all read the same. They are keywords, so a `type` cannot declare a
constructor called `True`. The lowercase `true`/`false` of releases before 0.11 stay reserved and are
an error that names the fix (``Pyfun spells this `True`, as Python does``). Reserving them matters in
a pattern: were `true` an ordinary name, `case true:` would bind a variable and match everything.
The recovering lexer the editor uses reports the old spelling once and still yields the boolean, so
the rest of the file is checked as written.

**Doc comments.** A line starting with `##` at **column 0** is a *doc comment*: one
or more consecutive `##` lines attach to the **following top-level `let` / `type` / `extern`**
declaration (joined with newlines, the conventional single space after `##` stripped) and surface
in LSP hover (§9). Design rationale — the minimal option that can't silently re-purpose existing
comments:
- **`##` doubles the existing comment marker**, the same move as Rust's `///` over `//`; no new
  character class, and a plain `#` comment is never promoted to documentation by accident (the
  alternative — attaching any plain `#` comment that precedes a declaration — would turn every
  such comment into hover text).
- **Ordinary comments are untouched:** `##` indented, trailing after code, or inside brackets stays
  plain trivia — only the column-0, bracket-depth-0 form lexes as `Tok::Doc`. The one behavioural
  change is that a column-0 `##` line *between* the statements of a top-level multi-line binding
  now reads as a new top-level statement (as Rust's `///` would); plain `#` remains the
  place-anywhere comment.
- **Roundtrip-safe as attached metadata:** the parser stores the text on the declaration node
  (`doc: Option<String>` on `LetBinding`/`TypeDecl`/`ExternDecl` — AST metadata, not free-floating
  trivia, so navigation/analysis are unaffected), and the pretty-printer re-emits `## ` lines
  before the declaration — so docs survive parse→print→parse while every other comment is still
  dropped by the canonical printer.
- **A doc comment is the Python docstring** of what it documents, in every output: a function's `##`
  lines open its `def` in triple quotes, and a record's open its class, so `help(area)` and a Python
  editor's hover show what the Pyfun author wrote. One line stays on one line; several close on a line
  of their own. A documented value or sum type has no place in Python for a docstring and keeps none.
- **MVP scope:** top level only (no local/module-member docs); a doc with nothing documentable
  after it (EOF, or a `measure`/`module`/`import`/expression) is accepted and dropped like a
  comment; no markdown processing — hover shows the text verbatim.

**Statement separation & blocks (general offside rule).** Indentation is turned into
block structure by a layout rule, not semicolons or braces. At lexing time a layout stack of block
columns (outside any `()`/`{}` brackets, where line breaks are always continuations) emits three
synthetic tokens: `Indent` opens a block, `Dedent` closes one, `Sep` separates two statements.
- A block opens after any **tail-position keyword** at bracket depth 0: a `let … =` body, a `match`
  arm or lambda `->`, an `if`'s `:`/`then`/`else`, or a `type … =` body. (An inline body crosses no
  newline, so the priming lapses and no block opens.) The top level is the outermost (implicit) block.
  For a `type`, this is what lets a sum be laid out one constructor per line (the F#/ML habit) —
  `type T =` then an indented `| A` / `| B` / … — since `|` is a continuation token (below), the arms
  attach as continuations rather than separate statements; a leading `|` on each line is optional and a
  record body (`{ … }`) may likewise sit on the indented next line. The printer canonicalizes any of
  these back to the single-line `type T = A | B | C` form, so it is pure surface sugar (no AST change).
- A line on the current block's column starts a **new statement** (`Sep`) *unless* it leads with a
  continuation token (an infix operator, `|`, `then`/`else`/`with`/`and`/`or`/`in`) — none of which
  can begin a statement. A line indented *past* the block continues the current statement. So
  consecutive statements (`print a` then `print b`) are distinct, while multi-line `match`/`if` and
  CE blocks stay together.
- A **block** (any indented tail-position body) is a sequence of statements — local `let`/`let mut`,
  `<-` reassignments, expression statements — whose final expression is its value. A single-expression
  block is unwrapped, so existing one-expression bodies keep their plain form. This is what gives
  mutability (§3) the statement sequencing it needs. Blocks lower to flat Python statement sequences;
  in `match`-arm / `if`-branch / lambda positions the lowering recurses into the body, which already
  handled blocks, so they "just work". Because a block can't be parenthesized (the offside rule is off
  inside brackets), the canonical pretty-printer renders block-bearing `if`/`match`/`fun` with offside
  indentation rather than the inline parenthesized form.

The rule is orthogonal to the brace-delimited CEs and records; inside a CE's braces the parser
applies its own item-level offside rule (§8.1).

### 7.1 Numbers & arithmetic — Python-familiar
The design for floats puts **familiarity to Python programmers first** — Pyfun brings functional
discipline, but numeric behaviour should not surprise someone coming from Python. A Python user never
sees the type machinery; they feel a few surface behaviours, and those are what this design pins
down.

**Decisions:**

1. **`/` is true division; `//` floors; `%` is modulo; `**` exponentiates.** Pyfun `/`
   is Python `/` (result type `float`, `7 / 2 == 3.5`), `//` is Python floor division (`7 // 2 == 3`,
   result `int`), `%` is Python modulo (`10 % 3 == 1`, same `*`/`/` precedence tier), and `**` is Python
   exponentiation — **float-only and dimensionless** (`float -> float -> float`; a runtime exponent
   can't be dimensionally checked, and `int ** -1` isn't an int, so following F# it stays float;
   the one unit-carrying power op that *is* static is the prelude `sqrt : float<'u^2> -> float<'u>`, §8.2),
   **right-associative**, and **tighter than unary minus** (`-2.0 ** 2.0 == -4`, `2.0 ** 3.0 ** 2.0 ==
   512`). This
   matches Python 3's most well-known numeric fact (the old floor-meaning `/` was the single most
   un-Pythonic thing in the language). To free the `//` spelling, **line comments moved from `//` to
   `#`** (Python-style — another familiarity win). `%` is num-constrained and **unit-preserving like
   `+`/`-`** (`10<m> % 3<m> : int<m>`; mixing units is an error). Bonus: because each operator maps
   1:1 to a Python
   operator, lowering stays purely syntactic — no need to consult inferred types to choose `/` vs
   `//` (the type-directed-lowering problem this would otherwise create disappears).
2. **One built-in numeric constraint, `num`.** `+ - * //` (and the prelude numerics)
   are typed with a single compiler-known constraint: `let add a b = a + b : num a => a -> a ->
   a`. `int` and `float` (with any units) satisfy `num`; `bool`/`str` do not (→ "expected `int`,
   found `bool`"). Generic functions like `area`/`min`/`max` stay polymorphic over int *and* float
   *and* units — the property a hard-coded int-default would throw away. No type annotations are ever
   required (Pyfun has none anyway). Implemented as a `Ty::Num(var, unit)` variant resolved by a tiny
   `num` union-find, with `num` variables generalized/instantiated alongside type and unit variables.
   **Prefix negation `-e`** (`UnOp::Neg`) is `num`-constrained and **unit-preserving** (`-5<m> :
   int<m>`). It is a **parser-level prefix operator**, deliberately *not* a lexer negative-literal:
   a signed-literal token would make `x-1` lex as `x` applied to `-1` (the ML/F# whitespace trap), so
   instead `-` is subtraction when it has a left operand and negation when it doesn't. It binds tighter
   than `*`/`/` and looser than application (`-f x` = `-(f x)`, `2 * -3` = `2 * (-3)`), coexists with
   the `(-)` operator section, and enables **negative integer literal patterns** (`case -1:`, the sign
   folded into the pattern, as Python's `match` allows). Lowers to Python `-x`.
3. **Polymorphic numeric literals; default `int`.** An integer literal `1` has type
   `num a => a` and adapts to context, so mixed-literal arithmetic just works the Python way:
   `1 + 2.0 : float`. Float literals (`1.5`) are concretely `float`, and include **scientific notation**
   (`1e6`, `2.5e-3`, `1E3`, `6.674e-11<m^3 / kg s^2>`): the lexer consumes the exponent (including its
   sign, so `e-3` isn't handed to unary minus), a number carrying an exponent is `float` even with no
   `.`, and `e` is only consumed when a real exponent follows (so `2exp`/`1e` stay integer-then-identifier).
   An unresolved numeric defaults
   to `int` — operationally automatic rather than a separate pass: it *displays* as `int`, and since
   it lowers to an int literal that Python coerces in arithmetic, results stay correct. An integer
   literal that inference *monomorphically* resolves to `float` (the `2` in `[1.0, 2, 3.0]`, the `1` in
   `if b then 1 else 1.5`, a literal passed to a `float` parameter) **lowers to a Python float literal**
   (`2.0`/`1.0`), so its printed value matches its type — `compile` runs one `check_collecting` pass,
   `float_literal_spans` collects the `float`-typed spans, and lowering emits `PyExpr::Float` for a
   value-position integer literal whose span is in that set. A *generalized* `let x = 7` stays `7`: `x` is
   polymorphic `num`, not `float`, so no coercion is due.
4. **No implicit int→float coercion between *variables*.** Mixing two values of genuinely different
   concrete numeric type (an `int`-typed variable plus a `float`-typed one) is a (gentle) error
   rather than a silent widening. Full coercion would require subtyping (`int <: float`), which
   breaks HM principal types; literal polymorphism (decision 3) covers the cases Python users
   actually hit, so this stricter-than-Python corner is rare and is where the discipline pays off.
5. **`+ - *` stay numeric.** Python overloads `+` for string/list concatenation; Pyfun does not.
   String concatenation is a named function (or a distinct operator) later, with an error that
   steers users there. This is the one deliberate departure from Python familiarity — silent
   `+`-means-anything is exactly the dynamic mushiness Pyfun exists to replace.

6. **Comparison & equality.** `< > <= >=` carry a closed built-in **`comparison`**
   constraint, implemented like `num` (an `ord` constraint set on type variables, propagated through
   unification and generalized), so `let lt a b = a < b` infers `comparison a => a -> a -> bool`.
   The constraint is satisfied by `int`/`float`/`str` **and by
   user sum types, records, tuples, the built-in `Option`/`Result`, and `List`, compared structurally**
   (bools and functions are still rejected).
   A **sum type** orders by variant *declaration order* first, then field-by-field (`Red < Green < Blue`;
   any `Circle` < any `Rect`; `Circle a < Circle b` iff `a < b`); a **record** orders field-by-field in
   declaration order; a **tuple** and a **`List`** are lexicographic (`List a` orderable iff `a` is).
   Nested/recursive types compose (`type Tree = Leaf int
   | Node Tree Tree` orders structurally). `require_ord` recurses into a `Con`'s constructor/record field
   types (its type params substituted by the actual arguments) with a **visiting-set recursion guard**
   (keyed on the full `(name, args)`, so a recursive occurrence terminates while `List a` vs `List (List
   a)` stay distinct) — and the deferred-var mechanism still flows a late-resolved `comparison a` through
   this path via the `unify` hook. Codegen (§8.3): a **record** derives ordering from
   `@dataclass(order=True)`, which compares its field tuple. A sum **variant** needs a cross-variant key,
   so it defines `_pf_order_key` → `(variant_index, field0, …)` and one `__lt__`, and `@functools.total_ordering`
   derives `__le__`/`__gt__`/`__ge__`; the variant index (declaration order) is the key's first element, so a
   cross-variant comparison short-circuits before the differently-shaped field tails. Tuples and lists need
   *no* codegen (Python compares them lexicographically already). The type checker gates comparison to one
   orderable type, so no `isinstance`/`NotImplemented` guard is emitted. Ordering is emitted **on-demand**:
   `require_ord` (the single site that proves a type needs comparison) records the type's name, and lowering
   emits the methods only for those — so an ADT that is only matched, never sorted, carries none of this.
   A single-file compile sees the whole program, so this is exact and sound; a multi-file project falls back
   to always-on (a type may be compared in another, separately-compiled module).
   **Not orderable:** `Set`/`Map` (no natural element-wise order) and `Async`/`Seq`/`Exception` —
   comparing them is a type error. `== !=` need **no** constraint — they're `a -> a -> bool` (same type, every type has
   equality), and generated ADT classes get a structural `__eq__` so `Some 1 == Some 1`. Both produce
   `bool` and are looser than arithmetic, tighter than `|>`. Surface wrinkle: `<` opens a unit
   annotation only when *adjacent* to a literal (`5<m>`); spaced (`5 < m`) it is less-than — the F#
   rule. **Chained comparisons** are Python-style: `a < b < c` means `a < b and b < c` with each
   operand evaluated once and short-circuiting — *not* the left-associative `(a < b) < c` (a bool
   compared to `c`). A dedicated `ExprKind::Compare` node (produced when two or more comparison links
   chain; a single comparison stays `Binary`) **lowers 1:1 to Python's own chained comparison**, so
   evaluate-once and short-circuit come for free rather than via a desugaring to `and`. Each adjacent
   pair is typed independently (operands unify; ordering links carry `comparison`, equality links
   don't), and links may mix operators (`0 <= i < n`, `a == b == c`).
   **Membership** is Python's `in` and `not in`, at the same precedence as the comparisons:
   `2 in xs`, `x not in {1, 2}`, `"Ada" in ages`, `"ell" in "hello"`. The right operand says what is
   searched, and so what the left one must be: an element of a `List`, `Set` or `Seq`, a key of a
   `Map`, a substring of a `str`. Those are the five Python's own `in` covers, and the set is closed
   in the manner of `num` and `comparison`. It **lowers 1:1** to the Python operator (`BinOp::In`
   and `NotIn` map to `in` and `not in`), which already means the right thing for each of the five,
   so lowering consults no type. Like the two constraints, it **generalizes**: `let has x xs = x in
   xs` is polymorphic in what it searches, and `has 2 [1, 2]`, `has 3 {1, 2}` and `has "ell" "hello"`
   all check. A membership test relates two types (what is sought, what is searched), so it is not a
   mark on one variable the way `num` and `comparison` are; it is a pending obligation
   `(elem, container)`. Where the container's type is known the obligation is settled on the spot.
   Where it is still a variable it waits for the rest of the enclosing binding, the same deferral a
   field access gets (§8.3). And where that variable is one the binding is generalized over, the
   obligation moves into the binding's scheme (`Scheme::members`) and every use poses it again
   against its own instance, to be settled when that use says what its container is; a misuse is
   reported at the use (`has 1 5`: `in` searches a `List`, `Set`, `Seq`, `Map` or `str`, not `int`).
   An exported scheme carries its obligations across modules. Only a container that is neither
   pinned down nor a variable of the binding's own type is an error, naming the module function to
   write instead (`List.contains x xs`). A displayed type carries the constraint
   (`has : a in b => a -> b -> bool`, §7). A membership test does not chain with another
   comparison (`a < b in c` is an error asking for `and`). `in` is a keyword; a computation
   expression's `for x in xs:` uses the same token.
7. **Logical operators.** `and` / `or` / `not` — all keywords, lowering to the same
   Python keywords. Spelled the Python way rather than F#'s `&&`/`||` to stay consistent with the
   Python-familiarity theme of this section (and to lower 1:1). `not` is `bool -> bool`, `and`/`or`
   are `bool -> bool -> bool`. Precedence mirrors Python — `or` < `and` < `not` < comparison — so
   `not a == b` is `not (a == b)` and emitted Python needs minimal parentheses.
8. **Operator sections.** A binary operator wrapped in parentheses, `(op)`, is that
   operator as a first-class curried function — `(*)`, `(+)`, `(-)`, `(/)`, `(//)`, `(==)`, `(!=)`,
   `(<)`, `(>)`, `(<=)`, `(>=)` — and `(*) 2` partially applies it (F#-style). It parses to
   `ExprKind::OpFunc(BinOp)` (the parser's `(`-atom disambiguates a lone operator-then-`)` from unit
   `()`, grouping `(e)`, and tuples `(a, b)`) and **desugars to the lambda `fun a b -> a op b`**
   (`desugar::op_func`) at both inference and lowering, so the operator's own constraint
   (`num`/`comparison`), currying, and partial application all fall out with no bespoke checker or
   emitter code — the same desugar-at-use tactic the computation expressions use. The pretty-printer
   keeps the faithful `(op)` spelling. `and`/`or` are **excluded**: they are keywords, and a strict
   function value would silently drop their short-circuit evaluation (F# excludes `&&`/`||` for the
   same reason). This makes point-free style with the `List`/`Seq` combinators natural: `List.fold (+)
   0 xs`, `List.map ((*) 2) xs`.
9. **Function composition `>>` / `<<`.** `f >> g` is left-to-right composition
   (`fun x -> g (f x)`, f then g); `f << g` is right-to-left / math ∘ (`fun x -> f (g x)`, g then f).
   Two-char lexer tokens (`GtGt`/`LtLt`), lexed before single `<`/`>` and `<=`/`>=`/`<-` — so `<<` is one
   token and never opens a `5<m>` unit annotation. A new precedence level `parse_compose` sits between
   `parse_pipe` and `parse_or`: composition binds **tighter than `|>`** (`x |> f >> g` = `x |> (f >> g)`,
   the useful reading) and looser than everything else, and is **left-associative**. `ExprKind::Compose`
   **desugars to a composition lambda** (`desugar::compose`) at inference and lowering, like the operator
   sections — so currying and the operands' constraints fall out with no bespoke checker/emitter code; the
   pretty-printer keeps `(f >> g)`/`(f << g)`. Unlike a section (whose body uses only its own params) the
   body embeds the operands, which may reference outer variables, so the lambda parameter is picked free of
   both operands' free variables (`_pf_x`, else `_pf_x0`, …) — no capture. Pairs naturally with the
   combinators: `List.map (double >> inc) xs`.
10. **Backward pipe `<|`.** `f <| x` == `f x` (F#'s `<|`, Haskell's `$`), added for
    symmetry so the pipe/compose quad `|>` `<|` `>>` `<<` is complete. It's modeled as a `backward` flag on
    the existing `ExprKind::Pipe` (forward `|>` applies `rhs` to `lhs`; backward `<|` applies `lhs` to
    `rhs`), lexed as `Tok::PipeLeft` (`<|`, before single `<`), at the lowest precedence with `|>` but
    **right-associative** (`f <| g <| x` = `f (g x)`). It lowers to plain application by flattening through
    `flatten_app` exactly like `|>`, so there is no new lowering path. Its whole use is dropping parens on a
    trailing argument (`print <| List.sum xs`); `|>` remains the idiomatic left-to-right pipeline.

**Why a *closed* set of built-in constraints, not user type classes.** `num` and `comparison` are
baked into the compiler; there is **no `class`/`instance` surface syntax**. The set stays closed,
which is itself the guardrail against sprawling into a Haskell-style class system (§11). Notably, **Pyfun
needs none of F#'s `inline`/SRTP machinery**: F# requires compile-time monomorphization for generic
arithmetic because `+` is a static per-type method on .NET, whereas **Python dispatches `+`/`<`/`==`
at runtime** (`__add__`/`__lt__`/`__eq__`). So a generic `add` lowers to one ordinary
`def add(a, b): return a + b` that works at runtime on whatever flows in — the constraint system
lives entirely in the type checker (for safety), and lowering stays trivial.

**What this loses** (vs a real, user-extensible type-class system): users can't declare their own
type `num`/`comparison` (e.g. a `Vector` supporting `+`), there are no custom classes or `deriving`,
and equality/ordering for user ADTs (when those land) is the compiler's call — it would auto-generate
`__eq__`/`__lt__` on emitted classes the way it already generates `__repr__`. What it keeps is the
thing that matters here: numeric and **unit** polymorphism, with Python-native surface behaviour.

### 7.2 Pattern matching — Python-framed
Pyfun's original `match e with | pat -> body` framing is F#. Python 3.10+ users now know a *native*
`match`/`case`, so the F# framing is the second false friend (§ discussion): the audience has muscle
memory for `match x:` / `case …:` and Pyfun spells it differently for no functional gain. This section
adopts Python's **framing** while keeping Pyfun's **pattern language** untouched — the distinction is the
whole point.

**Surface form.**
```
match <scrutinee>:
  case <pattern>: <block>
  case <pattern> if <guard>: <block>     # optional guard (see below)
  case <a> | <b>: <block>                # or-pattern (see below)
  ...
```
- `match e:` — the `:` opens an offside block of `case` arms (the scrutinee is any expression; the `:`
  at bracket depth 0 ends it).
- `case pat:` — the `:` opens the arm's **block** body (a sequence of statements whose final expression is
  the arm's value, per §7's block rule — so a `match` remains an **expression** yielding the taken arm's
  value). Inline (`case None: 0`) and indented multi-statement bodies both work, exactly as `then`/`else`
  bodies do today.
- Arms are `case`-delimited (new statements in the match block), **not** `|`-delimited. `with` and the
  leading `|` leave `match` entirely.

**What is deliberately *not* Python.** The **pattern language is unchanged**, because it is the part you
value and the part that carries Pyfun's FP surface:
- Constructor patterns stay **juxtaposition**: `case Some v:`, not Python's call-form `case Some(v):`.
  Juxtaposition is how application and construction are spelled everywhere else (§7); patterns mirror
  construction, and `Some(v)` would drag `( )`-application into patterns and fight currying.
- Tuple patterns `case (a, b):`, literal patterns, `_`, and **tagged record patterns** `case Point { x =
  0, y }:` (§8.3) are as before. Record patterns keep `{ … }` rather than becoming Python class/mapping
  patterns — consistent with tagged construction, and Pyfun has no `{ }` mapping-pattern to collide with.
- **Sequence patterns over `List`** `case [a, b, *rest]:` — `[]`, `[a]`, `[a, b, …]`, `[a, *rest]`,
  `[*rest]` (the star binds the tail, itself a `List`; `*_` discards it). *First cut:* the star must be
  **last** (or absent) and there is at most one — a non-last star (`[*init, last]`) is a parse error
  (`the `*` rest element must be last`); front/middle stars are a deferred follow-on. The rest binder is a
  variable or `_`. Modelled as `Pattern::List { prefix, rest }`; **exhaustiveness models `List` as the
  finite `Nil | Cons a (List a)` inside the usefulness algorithm only** (no real ADT, no lowering change) —
  so `[] | [x, *rest]` is exhaustive with no wildcard, `case []:` alone reports the witness `[_, *_]`, and
  a lone star `[*rest]` is a catch-all (it is equivalent to `rest`, so it delegates in
  `pattern_tag`/`row_head`/`default_matrix`). Elements bind at the list's element type, so nested patterns
  (`case [Some x, *rest]:`, `case [0, y]:`) work and type-check. Lowers to a Python **list** sequence
  pattern `case [a, b, *rest]:` (brackets, `PyPattern::ListSeq`, distinct from a tuple's paren `Sequence`).

**Two slots this framing frees (Python-identical):**
- **Or-patterns.** With arms delimited by `case`, `|` inside a pattern means alternation, as in Python:
  `case 1 | 2 | 3:`. Parsed at the constructor-application level (`Some a | None` is `(Some a) | None`),
  modelled as `Pattern::Or(Vec<Pattern>)`. Every alternative must bind the **same variables at the same
  types** (checked in `bind_pattern` by binding each alternative into a temp scope and unifying); the
  exhaustiveness checker expands an or-pattern into its alternatives (`expand_first_column` in `useful`),
  and it lowers to a Python or-pattern `case a | b | c:`.
- **As-patterns.** `case p as x:` matches `p` and also binds the whole matched value to `x` (Python's
  spelling). `as` is a keyword binding looser than `|`, so `a | b as x` is `(a | b) as x`; modelled as
  `Pattern::As`. It binds `x` plus the inner pattern's variables, and is **transparent for
  exhaustiveness** — the usefulness algorithm peels it (delegating in `pattern_tag`/`row_head`/`expand_or`),
  so `Circle r as w` covers exactly `Circle` and `_ as x` is a catch-all. Lowers 1:1 to Python `case p as x`.
- **Guards.** `case pat if cond:` is a refutable arm condition, the Python spelling. The guard is checked
  in the arm's pattern-bound scope and must be `bool`; a **guarded arm never counts toward exhaustiveness**
  (`check_exhaustive` filters `guard.is_none()`, and lowering's `has_catch_all` treats a guarded arm as
  refutable). It lowers to `case pat if cond:`; because Python allows no statements in a guard, a guard
  that would need hoisted statements is a lowering error (realistic guards are pure expressions).

**Implementation scope.** The *framing* change is lexer + parser + pretty-printer only (mirroring the
"blocks in tail position" change); **guards and or-patterns** additionally reach the checker, lowering,
and Python IR (both are genuine new pattern power, not just spelling):
- **Lexer.** `case` becomes a keyword. `:` at bracket **depth 0** primes a pending block (a new
  tail-position opener alongside `=`/`->`/`then`/`else`). This is unambiguous: Pyfun puts `:` nowhere else
  at depth 0 — record field types (`x: int`) live inside `{ }` (offside off), and there are no `let`/param
  type annotations — so a depth-0 `:` is always a `match`/`case` block opener. (This quietly forbids ever
  adding a depth-0 `:` elsewhere, e.g. optional `let x : int` annotations; §8.3 decision 2 leans on their
  absence, so the dependency is already implied.)
- **Parser.** `match e:` then an indented (or, inside brackets, un-indented) `Sep`-separated sequence of
  `case` arms; each arm parses a pattern (with a top-level `|` folded into `Pattern::Or`), an optional `if`
  guard, then a block body via the existing `parse_block_or_expr`. `case` starts a new arm (default `Sep`,
  not a continuation token), so no continuation-token table change is needed.
- **Pretty-printer.** Renders `match e:` / `case p:` with offside indentation (an or-pattern parenthesized,
  a guard as ` if <cond>`), replacing the `with | … ->` rendering. A `match` embedded mid-expression still
  has an inline parenthesized form `(match e: case p: b …)`. Round-trip guarantee preserved.
- **Checker.** `MatchArm` gained `guard: Option<Expr>` (typed `bool` in the arm scope; excluded from
  coverage) and `Pattern::Or` (same-variables-same-types check; expanded for exhaustiveness).
- **Lowering / IR.** `PyCase` gained an optional guard; `PyPattern::Or` emits `a | b`. Arms still lower to
  a Python `match`/`case` statement — now an even closer 1:1 — with the defensive `case _: raise` guard.
  Witnesses still print in Pyfun pattern syntax (`` `Some False` ``, `` `Point { x = _ } ` ``).

**Migration.** This **replaces** `match … with | … ->`; the two spellings are not both supported (avoid
two ways to write one thing in an MVP). `->` is retained for lambdas (`fun x -> …`) and function types
(`int -> int`), where it does not compete with a Python form. Examples and `examples/hello.pyfun` move to
the new spelling in the same change.

**`if` has two shapes, and each branch is spelled by its own.** A branch on the same line follows
`then` (or a bare `else`); a branch on its own indented lines follows `:`, exactly as Python writes it:

```
let abs x = if x < 0 then -x else x

let sign x =
  if x > 0:
    print "positive"
    1
  elif x < 0:
    -1
  else:
    0
```

The block form is Python's `if:`/`elif:`/`else:` statement, used as an expression: the block's last
expression is its value. The one-line form keeps `then` because Python offers nothing better there:
its conditional expression is the value-first ternary `a if cond else b`, which has no `elif` and
would make the two shapes read in different orders, and `if cond: a else: b` on one line is a syntax
error in Python too. `then` is not a false friend (Python has no `then`, so nobody is *misled*).

The opener is chosen per branch, so a chain may mix them (`if c:` + block, then `else 0`), and **there
is never a second way to write the same branch**: `then` or a bare `else` followed by an indented
branch is an error naming `if cond:` or `else:`, and a `:` followed by a same-line branch is an error
naming `then`. Inside brackets the offside rule is off (§7), so there is no block to open and a
multi-line `if` there keeps `then`, its lines being continuations. The layout rule needed no change:
`:` at bracket depth 0 was already a block opener for `match`/`case` (§7). The pretty-printer picks
the opener the same way, from whether the branch needs a block. Before 0.11 a block branch followed
`then`/`else`; that spelling is the first of the two errors above.

**`elif`** is pure sugar for `else if`, parsed by `parse_if_rest` into a nested `If` in the else
branch (no new AST node). The pretty-printer canonicalizes any else-if chain (however written) to an
`elif` chain, so it round-trips; `elif` is a keyword and a statement-continuation lead (like `else`),
so a chain spans lines cleanly.

**Example (with §8.3 tagged records — construction and pattern now mirror):**
```
let describe p =
  match p:
    case Point { x = 0, y = 0 }: "origin"
    case Point { x = 0 }:        "y-axis"
    case Point { x, y }:         "elsewhere"

let classify n =
  match n:
    case 0: "zero"
    case _:
      let positive = n > 0
      if positive then "positive" else "negative"

let grade n =
  if n >= 90 then "A"
  elif n >= 80 then "B"
  elif n >= 70 then "C"
  else "F"
```

### 7.2.1 Active patterns
F#'s signature pattern-matching extension: a **named recognizer** whose cases are used like
constructors in `case` patterns, so ad-hoc classification logic gets the same exhaustive,
declarative surface as an ADT.

**Definition — banana brackets at the `let` name position** (top level only; an `(` immediately
after `let` is unambiguous, since a binding name is an identifier or `_`):

```
let (|Even|Odd|) n = if n % 2 == 0 then Even else Odd            # total (closed case set)
let (|Small|Big|) n = if n < 10 then Small n else Big (n - 10)   # cases carry data
let (|Positive|_|) n = if n > 0 then Some n else None            # partial, Option — binds
let (|Blank|_|) s = String.strip s == ""                         # partial, bool — a predicate
let (|DivisibleBy|_|) d n = n % d == 0                           # parameterized partial (d before n)
```

**The mental model.** A **total** pattern `(|A|B|)` is a hidden ADT (its case set) plus a function
`input -> <hidden>`: the cases behave exactly like ADT constructors (fields, structural eq/repr),
except they exist only in patterns and in the recognizer's own body — never as values elsewhere. A
**partial** pattern `(|A|_|)` is a function returning `Option a` (the case binds the `Some`
payload) or — the F#-9-style improvement — plain `bool` (a pure predicate; the case binds nothing,
no `Some ()`/`None` ceremony). The flavor is **inferred from the body's type**, and `case Blank x:`
on a bool pattern is a clear "binds nothing" error. The last parameter is always the match input;
only a *partial* pattern may take leading extra parameters (F#'s rule), filled by literal/variable
*expressions* at the use site (`case DivisibleBy 3:`).

**Use — ordinary `case` patterns, no new pattern syntax.** `case Even:`, `case Small s:`,
`case Positive p:`, `case DivisibleBy 3:` all parse as constructor patterns; the checker resolves a
case name through the active-pattern registry (case names share the constructor namespace — a clash
with an ADT constructor, in either declaration order, is a compile error).

**Typing.** A total pattern's case arities come from a syntactic scan of its body (every
construction site must agree; a never-constructed case is an error); the field types are fresh
*monomorphic* vars pinned by the body and by use sites (one module-wide substitution keeps every
site consistent — the `let mut` model), and the body must produce the hidden type, whose display
name is the banana spelling (`int -> (|Even|Odd|)` on hover). The recognizer is monomorphic and
**module-local** (not exported — its hidden type and mono field vars can't cross a module boundary
soundly). **Effects:** the recognizer's body effect is latent on its innermost arrow, and a `match`
that uses the pattern *performs* it — so a `let pure` binding whose match uses an impure active
pattern is rejected.

**Exhaustiveness.** A **total** pattern's cases are a closed set: `case Even:`/`case Odd:` is
exhaustive with no `_`, and a missing case is reported with a witness (`` `Odd` is not matched ``).
Totality is *trusted* from the declaration (as F# does — a body that misses at runtime is a Python
error, not a checker unsoundness). The case-set signature applies only when **every** head in the
column is a case of the *same* total pattern (`Infer::ap_signature`, consulted before the column
type's own constructor signature since the scrutinee type is the recognizer's *input*); a partial
case is a refutable leaf, and any mixing (two patterns, or literals beside cases) conservatively
demands a wildcard.

**Shape rules** (checker errors, keeping the lowering correct): an active pattern may appear as the
**whole** pattern of an arm, or as an alternative in a top-level **or-pattern whose alternatives are
all binder-free cases** (`case Even | Odd:`, `case Small _ | Big _:`, `case DivisibleBy 3 |
DivisibleBy 5:` — a disjunction of tests; a *binding* alternative would make the or-arm's bindings
partial, so case fields must be `_`). Case arguments after the parameter expressions must be
**binders** (variables or `_`). **Structural non-AP arms mix freely** — `case Some x:` / `case [x]:`
/ record and tuple patterns sit beside AP arms; each lowers as a one-armed native `match` in the
fall-through sequence (below), and exhaustiveness for the mix stays conservative (the closed-set
signature applies only to a pure same-AP column; a mixed column falls back to the structural
signature, with concrete witnesses). **Guards are supported** on both kinds of arm — a guarded match
lowers to the fall-through `if`-sequence, so a failing guard falls through to the next arm. Still
rejected, as **non-goals** (ROADMAP): nesting an AP *under* structural patterns (`case Some
(Positive p):`), nested destructuring case arguments, and cross-module export — each needs recognizer
application at projection paths and/or usefulness recursion into the hidden monomorphic case types
(soundness-sensitive), and each has a direct workaround (guards; bind-then-destructure in the body; a
nested `match`).

**Lowering — a plain if/elif chain.** An active pattern is a *function call*, not a structural
test, so Python's `match` cannot express it. The declaration lowers to a plain def
(`_ap_Even_Odd(n)`) plus, for a total pattern, hidden case classes (`_Even`, `_Odd` — ordinary ADT
classes, underscore-mangled, no ordering); a partial-Option pattern reuses `Some`/`None_`, a bool
one needs no classes. A `match` containing any active-pattern arm evaluates the scrutinee once,
hoists each **distinct** recognizer application (function + arguments) to a temp — so side effects
run at most once per match — then emits the chain: total/Option cases test `isinstance` and bind
fields (`s = _pf_t0._0`), bool cases test truthiness (`if _pf_t0:`), an or-pattern of cases is the
`or` of its alternatives' tests (one shared recognizer, memoized), literal arms compare (`==`), a
trailing `_`/variable arm becomes the `else` (else a defensive raise). The emitter collapses
`else: if` into `elif`, so the output reads as hand-written Python. A **guarded** match — or one
with a **structural** arm — instead lowers to a forward `if`-sequence with early exit
(`lower_ap_match_seq`): each AP arm computes its recognizer **lazily** (only when reached, memoized
so a repeated application still runs at most once), a structural arm becomes a **one-armed native
`match`** (`match subj:` / `case Some(x): …` with no default — a non-match falls out and on to the
next arm, and a native `case … if guard:` carries the guard), and a full match returns — or, in
value position, sets a `_done` sentinel that gates the remaining arms — so a failing test or guard
falls through. A match with no active-pattern arms keeps the native `match`/`case` lowering
unchanged.

```python
def _ap_Even_Odd(n):
    if n % 2 == 0:
        return _Even()
    else:
        return _Odd()
def describe(n):
    _pf_t0 = _ap_Even_Odd(n)
    if isinstance(_pf_t0, _Even):
        return "even"
    elif isinstance(_pf_t0, _Odd):
        return "odd"
    else:
        raise RuntimeError("non-exhaustive match")
```

### 7.3 Opaque types — zero-cost newtypes

`opaque type UserId = str` (optionally parameterized, `opaque type Tag a = List a`) declares a
**newtype**: a nominal type distinct from its underlying type everywhere, with **zero runtime cost**.
The spelling is Scala 3's, and so is the pitch — domain distinctions (`UserId` vs `Email`, both
`str`) enforced by the checker, erased by the emitter. Pyfun adapts the semantics to its own
ground rules:

- **Wrap** with the constructor of the same name (`UserId : str -> UserId`, pure, curried like any
  constructor); **unwrap** with the single-case pattern (`case UserId s:`), which participates in
  exhaustiveness exactly like a one-constructor sum (a missing case reports the witness `UserId _`;
  the payload position takes any pattern, so `case Cents 0:` refines on the underlying value).
  Checker-side a newtype *is* a single-case sum — no new inference rules.
- **Erasure.** No Python class is emitted. A fully-applied wrap lowers to its argument
  (`uid = "u-1001"`), a first-class constructor reference (`List.map UserId xs`) lowers to the
  `_pf_id` identity helper, and the pattern lowers to the bare payload pattern. One consequence
  needed care: after erasure `case UserId s:` is an *irrefutable* Python pattern, and Python rejects
  any `case` after an irrefutable one as a SyntaxError — so lowering seals a match's case list by
  truncating at the first unconditional lowered case (judged on the **lowered** pattern, not the
  source arm) instead of appending the defensive `case _: raise`. In a **binding position** (a
  parameter, a `let`, a computation expression's `let`/`let!`/`for`; allowed because a newtype has
  one constructor, §7) the pattern is erased earlier still, by a rewrite of the module ahead of
  lowering: `let raw (UserId s) = s` reaches every later pass as `let raw s = s`, so it emits
  `def raw(s)`, a lambda taking `(UserId s)` stays a Python `lambda`, and `let (UserId a, Email b) =
  pair` is the plain `a, b = pair`.
- **The interop payoff.** Because the runtime value *is* the underlying value, an `extern` may be
  typed with the newtype directly — `extern save : UserId -> unit` hands Python the bare `str` it
  expects while the Pyfun side enforces the distinction. This is the difference from the single-case
  ADT idiom (`type UserId = UserId str`), which allocates a wrapper object that would leak across
  the boundary; the emitted-representation contract (§5) is why single-case sums are *not* silently
  erased — a newtype is the author's explicit promise that the type never has a runtime identity.
- **Distinct everywhere, not scope-transparent.** Scala's opaque types are transparent inside the
  defining scope and abstract outside; that scoping needs visibility control, which Pyfun
  deliberately does not have (§6.1 — everything is public). So the Pyfun version enforces
  distinctness everywhere and leaves the constructor public: the safety story is *accidental-mixing
  prevention*, not information hiding. If visibility control ever lands, scoped abstraction can be
  layered on without breaking this design.
- **Modules.** Newtypes cross file-module boundaries like sums: `Ids.UserId` constructs, patterns,
  and passes first-class in a consumer, erased on both sides (the export interface carries the
  newtype flag so the consumer's lowering erases too).
- **Positioning.** Units of measure (§8.2) remain the tool for *numeric* quantities (they compose
  algebraically: `m/s^2`); opaque types cover the non-numeric ground (ids, tokens, sanitized
  strings) and non-algebraic wrappers generally. `extern type` (§6) remains the third corner:
  maximally opaque handles with no constructor at all, for values only Python looks inside.

## 8. Showcase features (MVP): computation expressions & units of measure

These two F# flagships are deliberately in the MVP — they are the clearest demonstrations of "what
Python can't do," and both reinforce the gatekeeper thesis (units in particular are pure
compile-time machinery erased at runtime). They are an intentional, bounded exception to the
"defer ambitious features" guardrail (§11); everything *outside* this list still gets deferred.

### 8.1 Computation expressions

F# CEs desugar `builder { ... }` into calls on a *builder* with methods like `Bind`, `Return`,
`ReturnFrom`, `Zero`, `Combine`, `Delay`, `For`, `While`. Pyfun follows the same model:

- The four built-ins (`async`/`seq`/`result`/`option`) keep **bespoke native lowerings** (await /
  generators / railway) — a generic bind/return desugar can't produce those idiomatically.
- **The built-in set is closed by a rule, not by taste:** one per built-in *short-circuit type*
  (`Option`, `Result`) and one per Python *control-flow form* (`async`, `seq`). There is no fifth
  candidate, which is what stops "add a fourth" from becoming "add a fifth"; everything else is a
  user builder. Each is spelled in lowercase, mirroring its type where it has one
  (`option`/`Option`, `result`/`Result`). Builder keywords are contextual rather than reserved, and
  a bare `{ … }` record literal no longer exists (§8.3), so nothing else can parse in that position.
  Writing the *type's* capital is the one likely near-miss, so `Option { let! … }` is rejected by the
  parser with a message naming the lowercase form rather than resolving against the prelude module
  `Option`, whose members are ordered for `|>` (`Option.bind` takes its function first) and could
  never satisfy the value-first builder protocol below.
- **User-defined builders are now supported** (`src/desugar.rs`). A builder is an in-file `module`
  providing the protocol functions; `Builder { … }` (an uppercase module name before `{`) desugars to
  calls on them, after which ordinary HM inference and lowering take over — no per-builder type rules
  or codegen. The desugaring is type-directed *for free*: the builder's `bind`/`return_` signatures
  determine the types via normal inference on the desugared calls.

The protocol (F#'s, lowercased and keyword-safe); a builder need only define what its bodies use:

| item            | desugaring                                           |
|-----------------|------------------------------------------------------|
| `let! x = e` …  | `B.bind e (fun x -> …)`                              |
| `do! e` …       | `B.bind e (fun _ -> …)`   (trailing `do! e` → `e`)   |
| `let x = e` …   | `(fun x -> …) e`                                     |
| `e` …           | `let _ = e` (a bare expression item; `e : unit`)     |
| `return e`      | `B.return_ e`        (must be last)                  |
| `return! e`     | `B.return_from e`     (must be last)                  |
| `yield e` …     | `B.combine (B.yield_ e) (B.delay (fun _ -> …))`      |
| `yield! e` …    | `B.combine (B.yield_from e) (B.delay (fun _ -> …))`   |
| `for x in e: b` … | `B.combine (B.for_ e (fun x -> b)) (B.delay (fun _ -> …))` |
| (empty)         | `B.zero`                                             |

A bare expression on its own line is an item in every builder, built-in or user (F# admits a unit
expression as `let () = e`): the parser reads it as `let _ = e`, which is what the checker requires
of it (`unit`) and what the canonical pretty-print spells, so a `print`, a unit-typed `match` or
`if` needs no `let _ =` in front of it. A trailing `do! e` ends a `result`/`option`/`async` block
as its value (`M unit`), as the table's user-builder row already did, so a fire-and-forget
`async { … do! drain w }` needs no `return ()` after it; a trailing unit expression (`post inbox
Quit`) ends the block the same way, F#'s implicit `Zero`. The item boundary is the offside rule (an
item owns its line and any line indented past it), so an expression item never swallows the line
below it as arguments.

**`for` inside a block** is the Python spelling, `for target in source:` with an offside body,
consistent with `match e:`/`case`; `for` is a keyword only in item position inside CE braces (Pyfun
has no loop statement elsewhere, a `fun` recurses and folds iterate). The source is a `List a` or a
`Seq a` (Python's `for` takes either; an unresolved source is taken to be a `List`), the target is
any irrefutable pattern, and the body is a nested level of the same block: `yield`s in `seq`,
`let!`/`do!` in `async` and `result`/`option` (a failed step inside the loop short-circuits the whole
block), anything but `return` (the block's value comes after the loop). Native `seq`/`async`/
`result`/`option` lower it to Python's `for` statement, the idiomatic generator that `yield!
(List.map f xs)` was standing in for; a user builder gets F#'s `For` as `B.for_ e (fun x -> body)`,
combined with what follows like a `yield`. Braces carry no layout tokens, so the body is delimited
by columns like every other item, with one Python-shaped rule: a body that begins on the same line
as the `:` is exactly one item (`for x in xs: yield x`), and a longer body goes on indented lines,
which is also what the pretty-printer emits.

`Builder { let! … }` is told from `Some { x = 1 }` (a constructor applied to a record) by one-token
lookahead: a CE body starts with a CE keyword (`for` included), a record with `ident =`. A first
line that is neither (a would-be custom operation, `Html { class_ "board"`) is diagnosed as "not a
computation-expression item", naming what the position takes, rather than as a record literal
missing its `=`. `delay` receives a thunk `unit -> m a` (force it with the unit value:
`let delay f = f ()`).

The four built-ins and how they lower to Python:

| CE          | Semantics                          | Lowers to                                              |
|-------------|------------------------------------|--------------------------------------------------------|
| `async {}`  | asynchronous, `let!`/`do!` = await | Python `async def` + `await` (native coroutines); carries the `Async` effect (§4) |
| `seq {}`    | lazy sequence, `yield`/`yield!`    | Python generator functions (`yield` / `yield from`); pure, lazy |
| `result {}` | railway-oriented; short-circuit on `Error` | the `Result` ADT + early-return / nested-bind chain; pure but short-circuiting |
| `option {}` | short-circuit on `None`            | the `Option` ADT, the same early-return chain; pure but short-circuiting |

`result` and `option` are one lowering (`lowering::ShortCircuit`) differing only in how they spell
their success case, so neither can drift from the other. Each `let!` is the `isinstance` ladder of
§5.5: `if isinstance(r, Ok): x = r._0 … else: return r`, the failure forwarded as the value in hand
(an `Error e` rebuilt as `Error(e)` would be structurally equal, and Pyfun exposes no identity to
tell them apart). Both emit a flat sequence of tests with early returns, which is the whole reason
they are built in: the user-builder path is
an *expression* transform, so the same program written against a hand-rolled `Opt` module compiles
to a chain of nested `bind` lambdas on one line. A CE earns its keep at **two or more** binds; a
single bind reads better as `Option.map`/`Option.bind`.

Notes:
- `result {}` depends on a `Result`/`Option` ADT in the prelude — its Python representation is a
  §5 representation contract.
- `async {}` is where CEs and the effect system meet; keep the `Async` effect and the CE
  desugaring as separate concerns that compose.
- Lowered output must stay readable (§5): `seq {}` should produce idiomatic generators, `async {}`
  idiomatic `async`/`await`.

**Why braces, not indentation (a deliberate choice).** F# is offside-sensitive yet still delimits
CEs with `{ }` — because braces and the offside rule solve *different* problems. The offside rule
delimits *declarations* (where a `let`/`match` body ends); the braces delimit a **builder applied
to a block** (in F#, `async`/`seq` are ordinary values, not keywords, and the braces tie the value
to the block; indentation still structures the items *inside* the braces). The deciding factor is
that a CE is an **expression in arbitrary position** — a function argument, a `let` RHS, nested in
another CE — and the offside rule is awkward at delimiting an expression embedded mid-expression.
Braces are a context-free delimiter that works identically everywhere. Python is the cautionary
case: being indentation-sensitive, it *forbids* blocks in expression position (hence the
single-expression `lambda`); an expression-oriented language that went indentation-only for CEs
would inherit exactly that limitation. So Pyfun keeps the braces deliberately, not by inheritance:

- Pyfun's offside rule (`lexer/mod.rs`) delimits *statements and blocks*, not an expression embedded
  mid-expression, so the `{ }` is what delimits a CE block.
- The contextual-keyword scheme (`async`/`seq`/`result`/`option` are keywords *only* immediately before `{`)
  depends on the explicit brace as its disambiguator.
- The offside rule for `let`/`match`/function bodies proved **orthogonal** and composed with this
  (exactly as in F#): adding it required no change to CE or record braces. Records (§8.3) reuse
  `{ }` as well, so the brace family stays consistent.
- Indentation still structures the items *inside* the braces (again as in F#): an item's
  expression owns the rest of its line plus any following lines indented past the item, and the
  first line that is not sits in item position, where it must start with `let!`/`let`/`do!`/
  `return`/`yield`. A bare statement line inside a CE block is therefore reported as a misplaced
  item wherever it appears, instead of being swallowed as application arguments of the binding
  above it. A continuation line that does supply an application argument must in turn start at or
  right of the function expression it attaches to (F#'s FS0058 offside warning, applied to
  application and made an error): a line indented past the item but left of the expression is
  almost always a statement the writer meant as a new item, so the parser rejects it at the
  offending token, naming the item being continued (its keyword and line) and both fixes: bind
  the line as its own item (`let _ = …`, since an atom start can never begin an item), or indent
  it past the start of the expression to continue it.
  Both rules are applied by the parser (the braces suppress the lexer's layout tokens)
  and judged only directly inside the CE's braces, so brackets nested within an item keep
  treating line breaks as continuations.

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
- **Derived-measure aliases:** `measure N = kg m / s^2` names a compound of base
  measures; aliases may build on earlier aliases (`measure Pa = N / m^2`). An alias **expands** to its
  base-measure unit at declaration time (stored in `Decls::measure_aliases`) and is substituted
  wherever it appears — so `<N>` and `<kg m / s^2>` are the *same* type and unify. Consequence: the
  inferred-type display shows the **expanded** form (`int<kg m/s^2>`, not `int<N>`) — there is no
  abbreviation/conversion tracking (F#'s richer model is out of scope). The alias body reuses the unit
  grammar (now also accepting `1/s` for a dimensionless numerator); aliases, like `let`s, must be
  declared before use.
- **Unit-aware roots `sqrt : float<'u^2> -> float<'u>` and `cbrt : float<'u^3> -> float<'u>`
 :** √area = length and ∛volume = length — `sqrt 16.0<m^2> :
  float<m>`, `sqrt x<m^4/s^2> : float<m^2/s>`, `cbrt 27.0<m^3> : float<m>`, and a **non-square** (for
  `sqrt`) or **non-cube** (for `cbrt`) unit is a
  compile-time dimensional error (`type mismatch: expected float<'u^2>, found float<m^3>`). These are
  the *two* tractable unit-carrying power operations — F# special-cases the `sqrt` signature —
  because each exponent is a static rational constant (½, ⅓); general `x<'u> ** y` is impossible (a runtime
  exponent makes the result unit depend on a value → dependent types), which is why `**` stays
  dimensionless (§7.1). **Exponent-representation decision:** unit exponents stay
  **integers** — no rational exponents (the more general option) and no bespoke "halve-the-unit"
  constraint either. Neither is needed: `sqrt`'s scheme is expressed with the existing
  representation (its argument unit is `'u^2`, `Unit::pow(2)` in `seed_prelude`), and the existing
  abelian-group unifier's variable-elimination step *already* halves even exponents when solving
  `'u^2 ~ m^4 s^-2` (`'u := m^2 s^-1`) and fails on odd ones — the constraint "is a perfect square"
  falls out of unification for free, and inference propagates it (`let norm x = sqrt (x * x)` is
  unit-polymorphic `float<'u> -> float<'u>`). Nothing else in the language can *produce* a
  fractional dimension, so rational exponents would generalize the whole `Unit` type, its display,
  and every unification path for a capability with exactly one client. The one real change this
  surfaced: `solve_unit`'s reduce step could previously recurse forever on an unsolvable
  odd-vs-even equation (`'u^2 ~ m^3` overflowed the stack — a latent, reachable bug via
  `let sq x = x * x`); it now detects the no-progress case (a bare `v ↦ w` renaming, i.e. one
  variable left and every base exponent a non-multiple of the pivot's) and reports a dimension
  mismatch. `sqrt` is a **prelude builtin** (not an extern — the `extern` surface has no unit
  syntax), pure, lowering to `math.sqrt` with `import math` via the same routing as
  `floor`/`ceil`/`truncate`; units erase as always. Declaring `extern sqrt` now hits the ordinary
  "already defined" clash error; a user `let sqrt` still shadows the builtin.
  **`cbrt`** is the exact sibling with the exponent bumped to 3 (`Unit::pow(3)`),
  so unification thirds a perfect-cube unit (`m^3 → m`, `m^6/s^3 → m^2/s`) and rejects a non-cube
  (`m`, `m^2`, `m^4`). It earns its keep *only* through units: dimensionless `cbrt` would just be
  `x ** (1.0/3.0)`, but `**` is dimensionless, so a unit-aware cube root is the only version that
  keeps the dimension — and `math.cbrt` additionally cube-roots negatives correctly, which
  `x ** (1.0/3.0)` does not. **Where the family stops — `{sqrt, cbrt}` and no
  further.** Each fixed-`n` root is a separate monomorphic builtin (`float<'u^n> -> float<'u>`) — a
  general `root n x` is impossible because `n` is a runtime value (the same dependent-type wall as
  `**`), so `sqrt`/`cbrt` can't be unified into one function and higher roots would each need their
  own. Two is the principled cutoff: √ and ∛ are exactly the roots that map to physical *spatial
  dimensions* (2D area, 3D volume), the quantities people actually take roots of; a 4th root of
  `m^4` is not a measured quantity, and if one is ever genuinely needed a dimensionless `extern`
  covers it. (This is also why adding `cbrt` shrank the set of `math.*` names usable as `extern`
  examples — `sqrt`/`cbrt` are now reserved builtins; tests use `math.tan`/`math.fabs`.)
  interact with Python interop (units can't cross the boundary — they're erased, so the boundary
  sees plain numbers).

### 8.3 Records — constructor-tagged literals

Named-field **product** types, complementing ADTs' sum types: `type Point = { x: int, y: int }`,
**construction `Point { x = 1, y = 2 }`**, access `p.x`, functional update `{ p with x = 3 }`.
Parameterized records (`type Box a = { item: a }`) are polymorphic.

**Motivation.** A bare literal spelling `{ x = 1, y = 2 }` would be a *false friend*
to Python readers: it reads as a `dict`, but a Python dict is `{ "x": 1 }` (colons, string keys) and a
Pyfun record is nominal with `=` and bare field names. The bare brace instead holds what Python gives
it (**collection literals**, below), so a record written there would collide with a real form as well
as with a reader's Python knowledge, which is exactly the "basic stuff should feel familiar" surface
we care about (§7.1). Tagging the literal with
its type constructor kills the false friend, is **explicit about nominality** (§ decision 1), matches
Haskell's record syntax (functional pedigree) *and* Python's dataclass call `Point(x=1, y=2)` (familiar),
and makes construction **mirror** its pattern (below).

**Collection literals: `{"a": 1}` is a `Map`, `{1, 2}` a `Set`, `{}` the empty `Map`.** The bare
brace reads as it does in Python, a dict display or a set display, and the empty one is the empty
dict there and the empty `Map` here (the empty set stays `Set.empty`, as Python's is `set()`). With
the record literal tagged, the forms a bare `{` can open are told apart by what follows the first
expression: `with` is a record update (`{ p with x = 3 }`), `:` a map entry, and `,` or `}` a set
element. A trailing comma is allowed, a mix of entries and bare elements is an error naming both
forms, and the old bare record `{ x = 1 }` keeps its migration error because `=` is none of the three.
The literal is **sugar with no rules of its own**: the parser builds an `ExprKind::Collection` that
records the form and holds the expression it means (`Map.of_list [("a", 1)]`, `Set.of_list [1, 2]`,
`Map.empty`), inference and every analysis read that expansion, and only the pretty-printer and
lowering look at the form, so the literal prints back as written and lowers to the Python display it
is spelled as (`{"a": 1}`, `{1, 2}`, `{}`), 1:1. A key or element may be any expression, and its type
is whatever `Map`/`Set` accept. `:` inside brackets opens no block (§7), so the entry separator needed
no layout change.

Decisions:

1. **Nominal, not structural / row-polymorphic.** (Unchanged.) A record literal/access resolves to a
   *declared* record type. Records reuse the existing `Ty::Con` machinery (a record is a type constructor
   with a field registry), so no new `Ty` variant, and they unify and generalize exactly like ADTs.
2. **Field names are not globally unique — the base's type decides, with a use-site fallback.**
   A `p.x` access resolves in two steps. **First, from `p`'s own type**: the base is inferred before the
   field is resolved, so whenever that type is already a known record, *that* record owns the `x` — two
   records sharing a field name never interfere. This is F#'s rule, and it covers nearly every access in
   real code, since a base is usually a constructed value, a field of one, a match binding, or a value
   returned by a typed function. **Second, when the base's type is still a unification variable** (the
   accessor lambda `fun p -> p.x` being the canonical case), resolution falls back to the field name via a
   **multimap** (`field_owner : field → [records]`): it resolves iff **exactly one** *visible* record
   declares `x`; **0** is an unknown field, **2+** is an ambiguity error **at that access site**
   (`` field `x` is ambiguous here: the value's type is not known at this point … ``). A known record that
   simply lacks the field is reported against that record (`` record `B` has no field `x` ``), which is
   the precise error rather than a downstream mismatch between two record types.

   Resolution is **deferred, not immediate**, when the base is still a variable and the name is shared:
   the obligation is recorded and settled once the enclosing top-level binding is fully inferred, by
   which point a later statement may have said what the base is. `let l = c.letter` followed by
   `let flag = c.blank` type-checks, because `blank` pins `c` to `Cell` and HM knows that by the end of
   the binding even though it did not at the access. A variable a pending obligation depends on stays
   **monomorphic** until it is settled: generalizing it would give each use its own copy, and the later
   resolution would reach none of them. Deferring never weakens the check — the field must still exist
   on the record that wins, and its type must still fit every use.

   Ambiguity therefore fires only at a *use* whose base type is unknown *and stays unknown*, never at declaration or import —
   two records (in one module or across modules) freely share `x`/`name`/`id`. The fix when it does fire is
   to pattern-match or tag the construction/update (both name their record type), or to give the base a
   type some other way. This is OCaml's record-label model, with the type-directed tiebreak restored and a
   name-based fallback underneath it where OCaml would take the last-declared label.

   The accessor lambda `fun p -> p.x` is **unaffected**: it still types by field name whenever `x` has a
   single visible owner (including an *imported* record's field), and degrades to an error only when two
   visible records share `x` — a case the old global-uniqueness rule made *impossible to even write* (the
   second declaration was rejected outright). So the model is strictly monotone: every program that checked
   before still checks, with identical types.

   **Why not the three alternatives.** Lifting global field-uniqueness had three "obvious" routes; the
   two-step rule above takes the useful half of the first and rejects the other two:
   - **Type-directed access *alone*** (resolve `p.x` only from `p`'s inferred type) — regresses
     `fun p -> p.x`: when `p` is a bare parameter its type is a unification variable at the access point,
     so which record `x` belongs to is unknowable there without row polymorphism. Layering it *over* the
     multimap has no such cost, which is what the rule above does: the type decides when it is known, and
     the name resolves when it is not, so neither case is worse than it was.
   - **Project-wide uniqueness** (export field registries, error on cross-module clashes) — defeats module
     isolation: two unrelated modules couldn't both have a `name`/`id`/`x` field, collisions inevitable at
     scale. The multimap is *not* this: nothing clashes at declaration or import; only an ambiguous *use*
     in the module that can see both records errors.
   - **Row polymorphism** — the clean general mechanism, but a whole new type-system axis (row variables,
     open records, row unification, presence/absence constraints, noisier errors) for *structural*
     records Pyfun deliberately doesn't have. Its records are **nominal** (mirroring Python's
     `dataclass`/named classes, the readable-Python target). A **non-goal** — and the multimap made it
     unnecessary for the problems it was held in reserve for.

   *(Row polymorphism, for the record: `fun p -> p.x` would type as `{ x : a | 'r } -> a`, `'r` a row
   variable standing for "whatever other fields are present," so the function works on any record with an
   `x`. PureScript/Elm build extensible records on it. It stays out of scope.)*

   **Cross-module records** ride the **same rails as sum-type ADTs** — construct/pattern/update/access an
   imported record via a qualified tag (`Geometry.Point { x = 1 }`, `case Geometry.Point { x, y }:`), lowered
   to a single emitted `geometry.Point` class that both sides share (`isinstance`/`match`-compatible); see
   §6.1 for that mechanism. The only *record-specific* wrinkle is the field registry: an imported record is
   merged under its bare identity name (with a `Geometry.Point → Point` surface alias) and its fields append
   to the **use-site multimap** (decision 2), so bare `p.x` field-access on an imported value resolves exactly
   as for a local record — by the value's type where that is known, and by field name otherwise.
3. **Construction is constructor-tagged: `T { f = v, … }`.** A record literal in **expression position**
   always names its type: `Point { x = 1, y = 2 }`, or — for an imported record — the **qualified** tag
   `Geometry.Point { x = 1, y = 2 }` (a bare tag resolves only to a *local* record, exactly as a bare
   constructor does; decision 2). There is **no bare `{ f = v }` literal** — that form is removed, which is
   what eliminates the dict false friend outright. The type-declaration body keeps bare braces (`type Point
   = { x: int, y: int }` — a *type* body, not an expression), and access `p.x` is unchanged.
   **A brace after an uppercase name is a record only when it starts with a field name.** Anything else
   there is a collection literal the name is applied to, so a constructor takes a `Map` or a `Set`
   without parentheses: `JObject {"id": JInt 1}`, `Some {1, 2}`, `JObject {}`. A record's name with
   empty braces (`Point {}`) is therefore the name alone, and the checker says how a record is built
   (`` `Point` is a record, built with its fields: `Point { x = …, y = … }` ``). The formatter spaces
   the two the way their kinds are spaced: `Point { x = 1 }` and `JObject {"id": JInt 1}`.
4. **Update stays bare: `{ e with f = v }`, and a field may be a dotted path.** The base expression `e`
   already fixes the record type, and the `with` keyword makes the form unambiguously an update (Python has
   no `{ … with … }`), so it is not a false friend and needs no tag. Lowering binds the base to a temp
   (evaluated **once**) then reconstructs positionally — `{ p with x = 3 }` → `_t = p; Point(3, _t.y)`.
   **Nested-update sugar:** a field may be a **dotted path**, `{ p with a.b = v }`, meaning
   `{ p with a = { p.a with b = v } }` — the standard remedy for the deep-immutable-update pain (today you'd
   hand-write the nested reconstruction). Arbitrary depth (`a.b.c = v`), and paths mix with plain fields
   (`{ p with a.b = 1, x = 2 }`) and share prefixes (`{ p with a.b = 1, a.c = 2 }` rebuilds `a` once). It is
   *not* a false friend and needs no new machinery beyond the field multimap: the type checker walks the
   path, descending into each intermediate record field (which must itself be a record — else a clear
   error) and unifying the value with the leaf field's type; lowering reconstructs each level from the
   single base temp (`{ o with inner.a = 99 }` → `_t = o; Outer(Inner(99, _t.inner.b), _t.tag)`), so the
   base is still evaluated once and sibling fields are copied, at every depth. A field updated both
   wholesale and through a sub-path (`{ p with a = v, a.b = w }`) is rejected (one would silently override
   the other). This is *lightweight optics* — the readability win of a lens for nested update without the
   HKT/type-class machinery full optics need (which is a non-goal). `RecordUpdate` carries `Vec<FieldUpdate
   { path, value }>`; the pretty-printer re-emits `a.b`, so it round-trips.
5. **Lowering reuses the ADT class machinery** (§5). A record type — like every ADT variant — becomes a
   **frozen `@dataclass`**: the decorator generates `__init__`, structural `__eq__`/`__hash__`, and
   `__match_args__` from the field annotations (each field is annotated with its Python type where that
   maps to a concrete builtin — `int`/`float`/`str`/`list`/… — else `object`, e.g. for a type variable, a
   user ADT, `Option`, or a function; a user type *name* is never emitted, to avoid a forward reference),
   and `frozen=True` makes the value immutable, matching Pyfun's semantics (the hand-rolled
   classes it replaced were mutable). A **record** keeps the dataclass-generated `repr` (`Point(x=1, y=2)`,
   naming its fields); a sum **variant** / builtin gets a positional `__repr__` instead (`Circle(2.0)`,
   matching the constructor form `Circle 2.0`), suppressing the generated one with `repr=False`. Ordering
   (§7.1) differs the same way: a **record** compares its
   field tuple, which `@dataclass(order=True)` already provides; a sum **variant** needs a cross-variant
   key `(index, fields…)`, so it defines `_pf_order_key` + `__lt__` and lets `@functools.total_ordering`
   derive the other three. Both are **on-demand** (`python_emitter::emit_class` + the `OrderPolicy` a
   single-file compile collects from `require_ord`): emitted only for a type the program actually compares,
   so an ADT that is only matched sheds them. A multi-file project falls back to always-on (sound across
   separate compilation, where a type may be compared in another module). `Point { x = 1, y = 2 }` lowers
   to the positional call `Point(1, 2)` in
   declared field order — the *tag erases into the class name it already denotes*.
6. **Syntax disambiguation.** `{` participates in three constructs; the rule is now cleaner because bare
   expression-position literals are gone:
   - A `{` immediately after `=` in a `type` declaration is a **record-type body**.
   - `Name { … }` in expression position: peek the brace body. A computation-expression item
     (`let!`/`return`/`yield`/`do!` — the existing `starts_ce_item` lookahead) ⇒ a **CE block** and
     `Name` is a builder (§8.1). Otherwise `field = expr, …` ⇒ a **record construction** node
     `Record { ty: Name, fields }`; the checker verifies `Name` is a declared record type (error
     otherwise). This resolves the `Maybe { let! … }` vs `Point { x = 1 }` ambiguity by brace content, as
     today — only the record arm changes from "apply `Name` to a bare literal" to "construct `Name`".
   - `Module.Name { … }` (an uppercase name, `.`, an uppercase name, then `{`) in expression position is a
     **qualified record literal** for an imported record — distinguished from a qualified constructor
     application (`Geometry.Circle 2.0`) and a qualified member (`Geometry.area x`) by the immediately
     following brace (parser `peek4`). The tag `Geometry.Point` resolves via the imported-record alias.
   - A bare `{` in expression position must be a **functional update** (`{ e with … }`); anything else is
     a parse error (the old bare-literal path is removed). A data constructor applied to a record is now
     written with the tag explicit, `Some (Point { x = 1 })`, not `Some { x = 1 }`.
   - `.field` is a postfix binding tighter than application (`f p.x` is `f (p.x)`).
**Record patterns** in `match` are correspondingly **tagged**: `case Point { x = 0, y }:` (see §7.2). The
form is `T { name = pat, … }`, with `{ x }` shorthand binding field `x` to a same-named variable. Tagging
makes the pattern **mirror construction** and matches the scrutinee's record type explicitly. A pattern
may name a **subset** of fields (omitted fields go unmatched). It lowers to a Python keyword class pattern
(`case Point(x=0, y=y):`). A record pattern whose named sub-patterns are all irrefutable acts as a
catch-all for exhaustiveness.

**Exhaustiveness is deep** and is **entirely unaffected** by this revision — the check operates on the
`Pattern` AST (Maranget usefulness, matrix specialization), which is unchanged; only the surface spelling
of construction and record patterns moves. `Point { item = Some n } | Point { item = None }` is still
recognized as complete without a `_`, and witnesses print in the tagged form (`` `Point { x = _, y = True
} ` ``). Infinite types (`int`, `str`) and types without matchable constructors are exhaustive only via
a wildcard arm.

**Alternative considered — distinctive delimiters `{| … |}`** (OCaml/F#-anonymous flavour, e.g.
`{| x = 1, y = 2 |}`). It also removes the false friend with a simpler grammar (no type-name
classification), but is noisier per use, *looks* structural/anonymous (dishonest about Pyfun records being
nominal), and gains no symmetry with construction-vs-pattern. Rejected in favour of the tag, which buys
familiarity (dataclass/Haskell) and honesty. Neither option lifts field-uniqueness (decision 2).

Derived ordering on records (§7.1) and shared field names across records (the use-site rule above) have both since shipped.

**Record-bodied constructors: `Rect { width: float, height: float }`.** A constructor of a sum type
declares its fields by position (`Circle float`) or by name, with a record declaration's body:

```
type Shape =
    | Circle { name: str, radius: float }
    | Rect { name: str, width: float, height: float }
    | Dot { name: str }
```

A named constructor reuses the two record forms, tagged with the constructor: it is **built** with
the literal (`Rect { name = "r", width = 2.0, height = 3.0 }`, the fields in any order, each exactly
once) and **matched** with the record pattern (`case Rect { width, height = h }:`, naming the fields
the arm needs). `case Dot {}:` names none and matches any `Dot`. The subset a tagged pattern names
may be empty for a record too (`case Point {}:` is any `Point`), so the two tags follow one rule.
Python is the model: a dataclass is built `Rect(width=2.0, height=3.0)` and matched
`case Rect(width=w):`, and `case Dot():` is its match-any.

- **One spelling per constructor.** A named constructor is not a function. `Rect "r" 2.0 3.0`,
  `List.map Dot names` and `case Dot n:` are errors that show the form to write, and a positional
  constructor written with braces is the same error the other way round. A reader who sees a
  constructor's declaration knows how every use of it is written. The two kinds mix freely within
  one type.
- **Exhaustiveness** treats the tagged pattern as its constructor's, with omitted fields as
  wildcards, and prints a witness in the same form (`` `Rect { name = _, width = _, height = _ }` ``).
- **In a parameter or a `let`** the pattern binds when its type has no other constructor
  (`let unwrap (Box { value }) = value`), under the rule every constructor pattern follows there
  (§7): a positional constructor's does the same (`let unwrap (Wrap v) = v`).
- **A field every constructor shares belongs to the whole type.** When every constructor of a sum
  type declares a field at the same type (`name` above), a value of the type has it: `s.name` reads
  it and `{ s with name = "big" }` copies the value with it replaced, keeping whichever constructor
  built it, both without a `match`. The sum type joins the field's owners in the use-site rule above,
  so a base of unknown type resolves by name when one type declares the field and is ambiguous when
  two do. A field that some constructor lacks, or declares at another type, is reachable only
  through a `match`, and the error names the constructor that stands in the way.
- **A constructor and a record type cannot share a name**, since each is a Python class of that
  name. (A sum type may share its own constructor's: `type User = User { name: str }`.)
- **Lowering.** The class is a dataclass with the declared field names and `kw_only=True`, which
  states the one-spelling rule in Python's terms and is what distinguishes it from a record's class.
  It keeps the generated repr (`Rect(name='r', width=2.0, height=3.0)`) and a variant's ordering
  key. The literal is a keyword call in the order the source wrote the fields, the pattern a keyword
  class pattern (`case Rect(width=width, height=h):`, `case Dot():`), a shared field an attribute,
  and its update `dataclasses.replace(s, name=n)`. A dotted path through a shared field rebuilds the
  record or value found there first, exactly as it does through a record's.
- **The derived codec** (§6) keeps the case tag and writes `fields` as an object keyed by field
  name: `{"type": "Rect", "fields": {"name": "r", "width": 2.0, "height": 3.0}}`.

## 9. Tooling

The compiler ships a language server, a REPL, typed holes, and editor support. `INTERNALS.md` documents
the Rust crate layout and how each of these is built; the language-relevant surface is:

**Language server (`pyfun lsp`).** A dependency-free stdio server that reuses the front end: live
diagnostics; hover showing the inferred type and its effects (the only way to *see* an inferred type
without provoking an error, since types are never written); inlay hints; signature help; quick fixes;
go-to-definition, find-references, rename, completion, and document symbols; cross-file across a
project's modules (§6.1); over resilient parsing, so a half-typed file still navigates. A thin VS Code
client launches it.

**Inlay hints.** Types are never written in a `let`, so the editor writes them in for the reader:
`total: int` after a value's name, `shape: Shape` after each parameter, and `-> float` ahead of the `=`
of a function, with the effect the call performs on that last arrow (`->{io} unit`). The notation is
the one Python annotates with, because that is where a Python reader looks for a type. The pieces of a
function's hints are cut from its one displayed type, so a type variable reads the same in each (`x:
a`, `y: b`, `-> (a, b)`); the constraints on those variables (`num a`) stay with hover, which shows the
whole type. A name bound to a literal gets no hint, and neither does a name that opens with `_`. A hint
is the editor's text and none of it is Pyfun syntax: typed into a `let`, it is answered with "a `let`
is written without a type".

**Signature help.** While the arguments of a call are being written, the editor shows the function's
type as hover does (`shout : int -> str -> str`), with the parameter the cursor is on picked out, and
the function's `##` documentation under it. A call is juxtaposition, so it is a space that opens an
argument: after `shout 3 ` the second parameter is the active one. The type is the one at this use, so
it sharpens as arguments pin its variables: `List.map` reads `(a -> b) -> List a -> List b` before its
function is written and `(str -> str) -> List str -> List str` after. The call is the innermost one the
cursor is in; a function passed as an argument is not being called, so after `List.map double ` the
help is still `List.map`'s. Once every parameter has its argument there is nothing left to show.

**Quick fixes.** Where the compiler has said what is wrong and the remedy is mechanical, the editor
offers to apply it. A non-exhaustive `match` gets "Add the missing cases": an arm for every case the
arms leave out, written after the last arm at its indentation, each with a typed hole for a body (`case
Amber: ?`), so the match is complete at once and each hole reports the type its arm has to produce. The
cases are the exhaustiveness checker's own witnesses, found by adding each to the arms and searching
again, so they reach inside a constructor or a tuple (`(False, _)`) and stop when the type is covered.
An unused local `let` gets "Rename to `_name`", and an unused `import` or `extern import` gets "Remove
the unused import".

**Formatting (`pyfun fmt`).** One way to space and indent a program, applied in place: `pyfun fmt`
formats every `.pyfun` file under the current folder, `pyfun fmt path…` the files and folders named,
`pyfun fmt --check` only says which files would change (and fails when any would, for CI), and `pyfun
fmt -` reads a program from standard input and writes it formatted, for an editor. The language server
answers `textDocument/formatting` with the same result, so "format document" and format-on-save work in
every editor that has the server.

**It keeps every line break.** The formatter never joins or splits a line, so it has no line width to
hold to, and a layout chosen for the reader stays as written: a pipeline with a step per line, a list
with an element per line, a short `match` arm on one line and a long one on two. This is `gofmt`'s
bargain. What it settles is what is left to taste on a line:

- **Indentation**: two spaces a level, as the lessons and examples are written. A line that continues a
  statement (inside brackets, or led by an operator) is one level in from where it hung.
- **Spaces between tokens**: one around `=`, `==`, `|>`, `->`, `+`, `*`, `/` and their kind; one after
  a comma and none before; none inside `( )` and `[ ]`; a `:` hugs what is before it. Braces are spaced
  by what they hold: `Rect { width = 2.0 }`, `{ p with x = 1 }`, `async { return 1 }` and a record
  type's `{ x: int }` are spaced, a map `{"a": 1}`, a set `{1, 2}` and an arrow's effects `->{io}` are
  tight. What reads two ways is left as written, a space kept as one space and none kept as none: `-`
  (a negative number or a subtraction), `<` and `>` (a unit `5.0<m/s>` or a comparison), `|` and `.`.
- **Comments** are kept, each at the level of the code it goes with. A comment after code on a line has
  two spaces before it at least, and keeps more when the author lined a column of them up.
- **Blank lines and the end**: one blank line at most between lines, none at the head of the file, no
  space at the end of a line, one newline at the end of the file. A string, an f-string and a number
  are never touched, and a file keeps the line endings it has.

**It cannot change what a program means.** Indentation carries meaning, so the result is checked before
anything is written: the formatted text must parse to the program the source parses to (the AST
compares without positions). When it does not, the formatter tries again with the indentation left
alone, and failing that reports the file and leaves it as it is. A file that does not parse is reported
with its error and left too. Every program in the repository (the examples, the lessons' code, the
regression programs) is held to this in the test suite: it formats, formatting it again changes
nothing, and no comment or line is lost. The lessons and examples are themselves kept formatted by a
test.

**Not done:** breaking a line that is too long, and joining ones that would fit. That needs a line
width and a rule for every construct, and it would take the layout out of the author's hands; it is a
separate decision from this one.

**Tests (`pyfun test`).** Tests are run by pytest, found by pytest's own two conventions: a test file
is `test_*.pyfun`, and a test is a top-level function in it whose name opens with `test_`. `pyfun test`
type-checks each test file with the modules it imports, compiles them, and runs pytest over the result;
`pyfun test path…` names files or directories, and what follows `--` goes to pytest (`pyfun test -- -k
area`).

```pyfun
import Shapes

## A rectangle is its sides multiplied.
let test_rect _ =
  Test.equal 6.0 (Shapes.area (Shapes.Rect { width = 2.0, height = 3.0 }))
```

A test says what it expects with the `Test` module: `Test.equal expected actual`, `Test.is_true
condition`, and `Test.fail message` for a branch that must not be reached (it fits a branch of any
type). Each lowers to a Python `assert` where it stands, the tested value on the left, so a failure is
explained by pytest in its own way, with the expression and the values in it:

```text
    def test_wrong():
>       assert _pf_map(lambda x: x + 1, [0, 1, 3]) == [1, 2, 3]
E       assert [1, 2, 4] == [1, 2, 3]
E         At index 2 diff: 4 != 3
```

A test is the function of one ignored parameter (`let test_rect _ = …`), since every Pyfun function
takes one; it is emitted as `def test_rect():`, which is what pytest calls. Its `##` comment is its
docstring.

An assertion is the one way Pyfun code fails on purpose (everything else signals failure with `Error`,
§6), and its type says so: every `Test` member performs **`exn`** (`Test.equal : a -> a ->{exn} unit`,
`Test.is_true : bool ->{exn} unit`, `Test.fail : str ->{exn} a`). A `Test` member may therefore be
used anywhere, and a helper shared by several tests may assert (`let expect_sorted xs = Test.is_true
(is_sorted xs)` is `List a ->{exn} unit` by inference, and lowers to a `def` holding the `assert`, so
pytest explains a failure inside it the same way). The effect system keeps the property the first
version got by a rule: `let pure` rejects a function that asserts, a signature shows which functions
can fail this way, and `try` turns a failed expectation into an `Error` like any other raise. That
first version allowed a `Test` member only inside a `test_*` function; it was replaced once `exn`
existed (§6), since a rule about where a name may be written said less than the type does.

**Typed holes.** A hole `?` (anonymous) or `?name` in expression position is a typed blank: the checker
infers its type from context and reports it as a note (`` hole `?body` has type `int` ``), which the LSP
surfaces on hover. It is the type-driven-development tool from Haskell/Idris/Agda, and fits here because
complete HM inference always knows the expected type at a hole. A hole never causes a spurious error,
but it **blocks `compile`/`run`** (there is no value to lower) and makes `check` exit non-zero, so a
leftover hole is caught. `check` also lists in-scope bindings that would fill the hole, including
*refinement* fits reported applied to further holes (`String.concat ? ?`). This reaches beyond F#, which
has no typed holes.

**Tracebacks in `.pyfun` lines.** A well-typed program can still fail when it runs: a library
raises, a file is missing, a division is by zero. Under `pyfun run`, in the REPL and in a notebook
the traceback names the source. Each frame of the program gives its `.pyfun` file, the line of the statement that was running
and that statement's text:

```text
Traceback (most recent call last):
  File "shapes.pyfun", line 14, in <module>
    let scores =
        [1, 2, 3]
        |> List.map (fun x -> x + describe (Circle 1.0))
        |> List.filter (fun x -> x > 0)
  File "shapes.pyfun", line 14, in <lambda>
    let scores =
  File "shapes.pyfun", line 11, in describe
    case Circle(r): ratio 1 2
  File "shapes.pyfun", line 7, in ratio
    a // (total - total)
ZeroDivisionError: division by zero
```

The unit is the **statement**: a top-level item, a line of a block, a `match` arm, a branch of an `if`,
an item of a computation expression. A statement written over several lines is shown whole (its first
lines, when it is long), since the emitted Python holds it on one line and cannot say which of them was
running; a lambda inside it gets a frame of its own on the same line. A frame the compiler wrote is
left out: a prelude helper such as the function behind `List.map`, the shared runtime `_pyfun_rt`, the
launcher. A function the compiler had to name (the body of a `result { }` block) reads `<fun>`. A
library's frames stay exactly as Python prints them, so the traceback still ends where the error was
raised. In a project each module's frames name that module's file.

The program that runs is the program `pyfun compile` writes, byte for byte. Nothing is added to it: the
line map travels beside it, and a hook reads the map only when an error reaches the top. The map is
built from a second lowering whose statements carry source positions, and it is used only when that
lowering emits exactly the Python that runs; a program for which it does not keeps Python's own
traceback. `PYFUN_TRACEBACK=python` asks for Python's own traceback over the emitted code, which is the
view to attach to a compiler bug report. Python emitted by `pyfun compile` and run with `python`
reports its own lines, as any Python file does.

In the REPL and in a notebook the source is what was entered, so a frame names the entry and the line
within it: `File "<input 3>", line 1` for the third entry of a REPL session, `File "In[7]", line 2` for
the second line of the notebook cell Jupyter numbered 7. A function defined in one entry and failing
when a later one calls it is shown where it was written, with its statement, however many entries ago
that was. The frames of the REPL's worker and of the kernel are left out.

```text
pyfun> describe 3
Traceback (most recent call last):
  File "<input 3>", line 1, in <module>
    describe 3
  File "<input 2>", line 3, in describe
    ratio total (n - n)
  File "<input 1>", line 1, in ratio
    let ratio a b = a // b
ZeroDivisionError: division by zero
```

**Syntax highlighting.** A TextMate grammar flags the escape-hatch tokens (`mut`, `<-`, `extern`) in a
caution colour, marking the opt-outs from the immutable-by-default / effect-checked defaults; `pure`
stays neutral, since it is an encouraged assertion rather than an escape hatch.

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
- **Phase 4+ — tooling:** formatter (`pyfun fmt`); LSP / editor support
  (`pyfun lsp`: diagnostics + hover-for-type/effect + go-to-definition + completion over stdio, plus a
  thin VS Code client; see §9); then user-defined CE builders and unit polymorphism if not already in.

Because effects, CEs, and units are all MVP, their checking lands in Phase 3 alongside HM type
inference — not deferred. Units-of-measure unification (§8.2) is the highest-risk item in Phase 3;
spike it early.

## 11. Non-goals / guardrails

**Scope creep is the #1 project risk.** A neat transpiler becomes a multi-year language by
accreting features. The MVP showcase set (§8) is a *deliberate, fixed* exception — everything
outside it is deferred. Hold the line:

- **Do not fork CPython** — Pyfun is a front end targeting Python, full stop.
- Beyond the MVP (effects + the CEs + units), defer **unit polymorphism** (if not trivially free),
  macros, and a package manager until the core is solid. **User-defined CE builders** were on this
  list and came off it once the core settled: they desugar through machinery that already existed
  (§8.1), so they cost no new checking rules.
- The built-in computation expressions are **closed by a rule, not by a count** (§8.1): one per
  built-in short-circuit type (`Option`, `Result`) and one per Python control-flow form (`async`,
  `seq`). That is four, and there is no fifth candidate — which is what stops "add one more" from
  becoming a habit. The built-in unit set stays **small** for the same reason.
- Syntax is cheap; resist inventing more. Parser quality, error quality, and predictable lowering
  are what make the language usable — spend effort there.
- Keep the effect lattice small until real programs justify expanding it.

## 12. Naming (decided)

- Prose: **Pyfun** (capitalized like "Python"); never "PyFun" (reads as two words).
- Machine-facing: lowercase `pyfun` — CLI command, Rust crate, PyPI package, repo.
- File extension: **`.pyfun`**. CLI: `pyfun compile foo.pyfun`, `pyfun check foo.pyfun`,
  `pyfun fmt`, `pyfun lsp`.
- `pyfun-lang` is the PyPI distribution name (`pip install pyfun-lang`), because the bare `pyfun`
  was already taken by an abandoned 2015 package. Settled, not provisional: write it plainly with
  no hedging about a future rename. The command it installs, the crate, and the prose name stay
  `pyfun`/Pyfun.
