# Why Pyfun

Pyfun is an F#-inspired, functional-first language that compiles to readable Python. You get algebraic
data types, exhaustive pattern matching, currying, inferred effects, and units of measure, and the Rust
compiler checks all of them before it emits any Python.

It exists to teach functional programming where students already are. CS courses run on Python, and
learning FP usually means leaving it for Haskell, OCaml, or F# and adopting an ecosystem students rarely
return to. Pyfun keeps them in Python, with no new runtime and no new package manager, and compiles to
Python they can read, so every concept stays visible in the code they already understand.

`DESIGN.md` is the full specification. This document explains the decisions behind it.

## Emit readable Python, and own nothing at runtime

The founding decision is that Pyfun is a front end. It compiles to plain Python source and ships no
runtime library of its own. A `List` is a Python `list`, a record is a frozen dataclass, and a `match`
is Python's `match`.

That decision pays off three ways. The output is a first-class artifact you can read, diff, and ship,
not a build product you look through. A student running `pyfun compile` watches an algebraic data type
become a class and currying become a closure, so the functional idea and its Python shape sit side by
side. And there is nothing to version or install alongside your program, because the compiler's work is
finished once the Python is written.

It also disciplines the language. A feature that cannot lower to clean Python does not ship. That
constraint keeps the surface small and the output legible, and it is why the rest of these decisions
bend toward Python's grain.

## Everything is settled before Python exists

Python's runtime gives no compile-time guarantees, so every guarantee Pyfun offers has to come from a
tool that runs first. The Rust compiler is that tool.

```
parse → type-infer → exhaustiveness → immutability → effect check → lower → emit Python
```

Each stage gates the next. When a check fails, compilation stops with a rustc-style diagnostic and no
Python is written. A forgotten case is the everyday example. Leave a variant unhandled and the compiler
names it:

```console
$ pyfun check shapes.pyfun
error: non-exhaustive match: `Rect _ _` is not matched
```

All the checking happens here, so the emitted Python carries none of it.

## Erasure is the recurring move

The same pattern runs through the whole design: a feature adds meaning at compile time and leaves no
trace at runtime. Effects are inferred and checked, then erased. Units are checked dimensionally, then
erased to plain numbers. The `pure` assertion is verified and compiles to nothing. Currying lives in the
type system and collapses to direct calls in the output. Typed holes are reported by the checker and
never reach the code. Safety is spent up front, and what ships is ordinary Python. The two features
below are the clearest cases.

### Effects are inferred, never written

A function's type records what its body does. It carries `io` when the function prints or mutates, and
`async` when it does async work. You write ordinary code and the labels appear on their own:

```fsharp
let add a b = a + b          # int -> int -> int
let greet n = print n        # string ->{io} unit
```

Effects propagate as you compose. Call an effectful function and yours becomes effectful, and a body
that prints and fetches carries both labels. Higher-order functions stay polymorphic in their effects,
so `apply print` is effectful at the call site while `apply` on its own is pure. When you want a
guarantee, `let pure f …` asserts it, and the compiler rejects the definition if the body performs any
effect.

The model follows Koka and Flix: effects are inferred and reported by tooling, and the source stays
clean. Code reads the same whether or not a function has effects, because the labels ride in the type
rather than in the body. They and the `pure` assertion erase completely, so the tracking leaves no trace
in the Python you ship.

### Units of measure that vanish

`1.0<m> / 2.0<s>` has type `float<m/s>` and compiles to a plain Python `float`. The compiler does the
dimensional analysis and then erases it, so a length can never be added to an area, and the runtime
still sees only numbers.

Units form a free abelian group under multiplication, division, and integer powers, so checking them is
equation solving. Unifying `'u^2` with `m^4/s^2` gives `'u = m^2/s`, by variable elimination over the
rationals. That solver is its own module in the type checker, written in Rust, and the Python you ship
never sees it.

## Types are inferred everywhere; annotations live at the boundary

Hindley-Milner inference is complete, so the compiler needs no type annotations, and Pyfun has none on
`let` or on parameters. The types are all there, surfaced by LSP hover, `pyfun check`, and the REPL,
without cluttering the source. Annotation-free code is a feature.

`extern` is the one place you write a type, and that placement is deliberate. An `extern` names a Python
function and signs a claim about it, so the type belongs at the signature. The boundary is effectful by
default, because a Python call can do anything, and `extern pure` is the opt-out for the calls where you
know it is safe.

## Currying that reads as plain calls

Functions curry by default. A fully applied call compiles straight to a direct call, so `f a b c`
becomes `f(a, b, c)`. Closures appear only when you partially apply, where they compile to
`functools.partial`. The `|>` pipe is sugar that resolves at compile time and costs nothing at runtime.

The Python side stays n-ary in both directions. You call an imported Python function with normal syntax,
and a Pyfun function you expose to Python has a plain `def` signature. Python callers work with ordinary
functions.

## Boundary and engine

Pyfun sits on top of CPython and uses what CPython already has. Programs tend to fall into two parts.
The boundary is small and effectful: it streams files, calls libraries, and talks to the outside. The
engine is the part you want to get right, and it is typed, total, and immutable. `extern` is the seam
between them:

```fsharp
extern get : string ->{async} bytes = httpx.get
```

Past that call you are in Python, and the call is effectful by default. You reach the whole ecosystem at
full speed, with a typed engine wrapped around it. A Python file object is an iterator, so an `extern`
can hand one back as a lazy `Seq string`, and a fold over it streams a file of any size in constant
memory. The engine that consumes those lines is typed end to end, and a malformed line becomes a value
you handle rather than a crash. The `examples/interop/` cookbook shows the pattern against `json`,
`sqlite3`, and a 3 GB rail-timetable feed.

## A small surface, on purpose

Pyfun keeps a deliberately small surface. There are three computation expressions, `async`, `seq`, and
`result`, and a fixed set of built-in units. The reason is that parser quality, error quality, and
predictable lowering are where the effort pays off, so the language spends its budget there rather than
on breadth. User-defined CE builders arrived after the core settled, because they desugar cleanly
through machinery that already existed.

New syntax mirrors a form a Python reader already knows. Pattern matching is `match e:` with `case`
arms, string interpolation is `f"..."`, comparisons chain the Python way, and the numeric operators
behave as Python's do. When a construct has a familiar Python spelling, Pyfun uses it, so the surface
reads as Python with functional structure. The full list of deferrals, with reasons, is in `DESIGN.md`
§10 and §11.

## Lineage

Pyfun borrows deliberately and openly. F# gives the syntax, the computation expressions, the units of
measure, and the pipe. Koka gives the inferred effect system, where effects are a property the compiler
tracks rather than a keyword you write. Elm gives the decoder-combinator style for turning untyped JSON
into typed data, and the habit of diagnostics that name the exact problem. Haskell and Idris give typed
holes, the type-driven-development tool. Hy is the precedent for lowering source to a Python AST rather
than splicing strings. `README.md` sets Pyfun beside Fable, Erg, and Coconut feature by feature.

## Try it

- **Browser playground:** <https://simontreanor.github.io/Pyfun/playground/>. The compiler runs as WebAssembly and
  Python runs in the browser, so you can open a worked example, tweak it, and watch it compile with no
  install.
- **Install:** `pip install pyfun-lang` (Python 3.12+).
- **Specification:** `DESIGN.md`.
- **Design in practice:** the `examples/interop/` cookbook, and the performance write-up (link once the
  article is published).
