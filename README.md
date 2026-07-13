# Pyfun

[![PyPI](https://img.shields.io/pypi/v/pyfun-lang.svg)](https://pypi.org/project/pyfun-lang/) [![VS Code Marketplace](https://img.shields.io/badge/VS_Code-Marketplace-007ACC?logo=visualstudiocode&logoColor=white)](https://marketplace.visualstudio.com/items?itemName=pyfun.pyfun) [![License: Apache 2.0](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](https://github.com/simontreanor/Pyfun/blob/main/LICENSE)

**Functional programming for the language classrooms already teach.**

Pyfun is an F#-inspired, functional-first language that compiles to readable Python. It brings
algebraic data types, exhaustive matching, currying, inferred effects, and units of measure to the
Python ecosystem, and its Rust compiler checks every one of them before a single line of Python is
emitted.

It exists to make functional programming teachable where students already are. CS courses run on
Python; learning FP usually means leaving it for Haskell, OCaml, or F# and adopting a whole new
ecosystem students rarely touch again. Pyfun keeps them in Python, with no new runtime and no new
package manager, and compiles to Python they can read, so every concept stays visible in the code
they already understand.

**▶ [Try it in your browser](https://simontreanor.github.io/Pyfun/)** — no install: write Pyfun,
watch it compile to readable Python live, and hit Run to execute it (the compiler runs as
WebAssembly, the Python runs in CPython-via-WASM).

Or install it with **Python 3.12+**:

```bash
pip install pyfun-lang
```

```fsharp
type Shape = Circle float | Rect float float

# `area` handles Circle but forgets Rect, so Pyfun refuses to compile it:
let area s =
  match s:
    case Circle r: 3.14159 * r * r
```

```console
$ pyfun check shapes.pyfun
error: non-exhaustive match: `Rect _ _` is not matched
 --> 5:3
  |
5 |   match s:
  |   ^^^^^^^^
```

> Plain Python compiles and runs this, then silently returns `None` the day a `Rect` reaches it,
> and you debug the `TypeError` an hour downstream. Pyfun's Rust compiler checks types, effects,
> units, and match exhaustiveness **before a single line of Python is emitted**, then hands you
> code you can read, diff, and ship.

And when it compiles, the output *is* the point. There's no runtime library to ship and nothing to
read around — your `match` becomes Python's `match`/`case`, one for one:

```fsharp
let grade score =
  match score:
    case s if s >= 90: "A"
    case s if s >= 80: "B"
    case _: "C"
```

```python
# exactly what `pyfun compile` emits — no wrappers, no runtime:
def grade(score):
    match score:
        case s if s >= 90:
            return "A"
        case s if s >= 80:
            return "B"
        case _:
            return "C"
```

A Pyfun `List` is a Python `list`, a record is a plain class, and `extern` calls a real library
directly ([fuller example below](#type-checked-python-interop)).

---

## Made for the classroom

Teaching FP normally forces a detour: a new language, a new toolchain, and a new ecosystem the
students abandon the moment the course ends. Pyfun removes the detour.

- **They already have the runtime.** Pyfun compiles to plain Python, so anything a student writes
  runs on the interpreter already installed on every lab machine. No VM, no new package manager.
- **The concepts stay visible.** `pyfun compile` shows the Python your functional code becomes, so a
  student watches an ADT turn into a class, a `match` into `match`/`case`, and currying into a
  closure. They learn the idea and how it maps to the imperative code they know.
- **Good habits are enforced, not suggested.** The compiler refuses to skip a case, ignore a `None`,
  or mutate what should stay immutable, so students learn to handle every path because the tool insists.
- **A small, learnable core.** Pyfun is deliberately compact, so the language stays out of the way of
  the ideas you are teaching.

Pyfun is a real, general-purpose language, not a toy. But teaching is why it exists.

---

## Why Pyfun over plain Python?

Python is the best ecosystem in the world, and even with `mypy`/`pyright` bolted on, large Python
programs still fail in ways a compiler could have caught. Pyfun keeps the ecosystem and makes the
checks mandatory.

| | Plain Python | Pyfun |
|---|---|---|
| **Type errors** | `mypy`/`pyright` are optional and unsound; they warn, they don't gate | found at compile time; no Python is emitted until they pass |
| **`None` handling** | `AttributeError: 'NoneType'…` | `Option a` with exhaustive `match`; the compiler makes you handle `None` |
| **Missing `case`** | silently falls through, returns `None` | **exhaustiveness error** with a concrete missing-case witness |
| **Mutation** | everything is mutable, everywhere | immutable by default; `let mut` + `<-` is opt-in and tracked |
| **Side effects** | invisible | **inferred and tracked**; `let pure` is a compile-checked promise |
| **Units / dimensions** | a comment and a prayer | `100<m> / 10<s> : float<m/s>`, checked and then erased |
| **Runtime** | CPython | **CPython**: Pyfun *is* Python once compiled |

**Why not just `mypy`/`pyright`?** They're a gradual, optional overlay: unsound by design, never
required, and one `# type: ignore` from silence. They report; they don't gate. Pyfun makes the same
class of check *mandatory*: it blocks compilation, infers the signatures pyright often needs spelled
out, and there is no untyped Pyfun to fall back to. And you keep the entire Python ecosystem while
you do it.

---

## Type-checked Python interop

`extern` imports **any** Python callable or value at a Pyfun type. The dotted target is imported
for you; the boundary is *effectful by default* (a Python call can do anything), and `pure` opts
out where you know better. Once imported, the function is a first-class curried Pyfun value:
type-checked, effect-tracked, and partially applicable.

```fsharp
extern pure mean:  List float -> float = statistics.mean
extern pure stdev: List float -> float = statistics.stdev

type Summary = { n: int, mean: float, stdev: float }

let summarize xs =
  Summary { n = List.len xs, mean = mean xs, stdev = stdev xs }

let report xs =
  let s = summarize xs
  f"n={s.n} mean={s.mean} sd={s.stdev}"

print (report [1.0, 2.0, 3.0, 4.0])
```

`pyfun compile` turns that into Python you'd be happy to have written by hand:

```python
from dataclasses import dataclass
import statistics

@dataclass(frozen=True)
class Summary:
    n: int
    mean: float
    stdev: float

def summarize(xs):
    return Summary(len(xs), statistics.mean(xs), statistics.stdev(xs))

def report(xs):
    s = summarize(xs)
    return f"n={s.n} mean={s.mean} sd={s.stdev}"

print(report([1.0, 2.0, 3.0, 4.0]))
```

```console
$ pyfun run stats.pyfun
n=4 mean=2.5 sd=1.2909944487358056
```

Notice what the compiler does:

- **No wrapper layer.** `statistics.mean(xs)` is called directly. `List` *is* a Python `list`,
  and a record or ADT variant *is* a **frozen `@dataclass`** — so `frozen=True` even enforces in the
  Python the immutability Pyfun promises. There is no runtime, no VM, no marshalling.
- **Effects tracked across the boundary.** A bare `extern` is `io` at full application, so it
  can't be called from a `let pure`. Mark it `pure` (like `statistics.mean`) and it composes into
  pure code. You can even annotate other effect labels: `extern fetch: string ->{async} string = httpx.get`.
- **Exceptions become values.** `try (parseInt s) : Result int Exception` catches whatever the
  Python side raises and hands you a `Result` to `match` on. The imperative FFI edge becomes the
  FP error type, with `errorKind` and `errorMessage` fields.

```fsharp
extern parseInt: string -> int = int          # Python's built-in int()

let safe s = Result.withDefault 0 (try (parseInt s))
print (safe "42")     # 42
print (safe "oops")   # 0   (the ValueError was caught into an Error)
```

---

## A whistle-stop tour

Everything below type-checks, compiles, and runs today. See
[`examples/hello.pyfun`](https://github.com/simontreanor/Pyfun/blob/main/examples/hello.pyfun) for the exhaustive version.

**Algebraic data types, records, and exhaustive matching.** `None` cannot bite you:

```fsharp
type Shape = Circle float | Rect float float

let area s =
  match s:
    case Circle r: 3.14159 * r * r
    case Rect w h: w * h
# forget a case and the compiler reports the missing witness, e.g. `Rect _ _ is not matched`
```

**Decode untrusted JSON into typed data, totally.** `json.loads` hands back an untyped dict that
explodes three layers downstream. The built-in Elm-style `Decode` module turns JSON into your own
records — a missing field or wrong type is a value you handle, never an `AttributeError` an hour later:

```fsharp
type User = { name: string, age: int }

let user =
  Decode.map2 (fun name age -> User { name = name, age = age })
    (Decode.field "name" Decode.string)
    (Decode.field "age" Decode.int)

# Decode.decodeString user : string -> Result User Exception
#   good input   -> Ok (a typed User)
#   missing/bad  -> Error (a value describing exactly what was wrong)
```

The [`examples/interop/`](https://github.com/simontreanor/Pyfun/blob/main/examples/interop) cookbook
calls `json`, `sqlite3`, `pathlib`, and `urllib` this way — typed and effect-tracked at the boundary.

**Pipelines, currying, composition.** F#'s `|>`, `<|`, `>>`, `<<`, and operator sections `(+)`:

```fsharp
let describe =
  List.filter (fun x -> x > 0)
  >> List.map ((*) 2)
  >> List.fold (+) 0

let total = [1, -2, 3] |> describe    # (1 + 3) * 2 = 8
```

**Inferred effects.** Purity is a checked promise, never boilerplate:

```fsharp
let pure add a b = a + b        # OK: no effects
# let pure shout n = print n    # compile error: `print` performs `io`
```

**Units of measure.** Dimensional analysis at compile time, erased at runtime:

```fsharp
measure m
measure s
measure kg
measure N = kg m / s^2          # derived aliases expand to base units

let speed = 100<m> / 10<s>      # float<m/s>
let force = 10<N>
# let bad = 100<m> + 10<s>      # compile error: m vs s
let side = sqrt 16.0<m^2>       # float<m>, unit-aware roots
```

**Computation expressions** (F#'s showcase feature): `result`, `seq`, `async`, plus your own:

```fsharp
let checked ok v =
  result {                      # railway-oriented; short-circuits on Error
    let! x = if ok then Ok v else Error "bad"
    return x + 1
  }
```

**Rich literals and strings.** F-strings, raw strings, triple-quotes, scientific notation,
digit separators, hex/octal/binary:

```fsharp
let planck = 6.626e-34
let million = 1_000_000
let mask = 0xFF
let who = "Ada"
let line = f"{who} scored {million} ({String.upper who})"
let path = r"C:\Users\pyfun"    # raw string, backslashes literal
```

And a standard library that reads like F#'s: module-qualified `List` / `Set` / `Map` / `Option` /
`Result` / `Seq` / `String` (`List.map`, `Map.tryFind`, `Result.bind`, lazy `Seq.take`,
`String.split`), tuples, active patterns, typed holes for type-driven development, and
multi-file projects with `import`.

---

## How Pyfun compares

A few projects bring functional or statically-typed code to Python. Here is the field, and the
bet Pyfun makes within it:

- **[Fable](https://fable.io)** compiles real F# to Python, the most capable option by far, because
  it *is* F#, with the whole language and a mature ecosystem. The trade-offs: it needs the **.NET
  toolchain**, and its output depends on a **runtime library** (`fable_library`).
- **[Erg](https://erg-lang.org)** is a statically-typed, Python-compatible language with a rich type
  system and marker-based effect control. It is the closest to Pyfun in ambition, though "rusty"/OO
  rather than ML-family, with *explicit* effect annotations.
- **[Coconut](https://coconut-lang.org)** is a functional *superset* of Python; static typing is an
  optional MyPy add-on, so nothing is enforced.
- **Dynamic dialects** (**Hy**, **Mochi**, **Dogelang**) are dynamically-typed FP/Lisp languages
  that run on Python; they share the last column, since they trade static guarantees for Python's
  dynamism.

Legend: ✅ yes · ⚠️ partial · ➖ different approach · ❌ no

| | **Pyfun** | **Fable** | **Erg** | **Coconut** | **Dynamic dialects** |
|---|:--:|:--:|:--:|:--:|:--:|
| FP-first language (not a Python superset) | ✅ | ✅ | ✅ | ➖ | ⚠️ |
| ML / F#-family syntax | ✅ | ✅ | ➖ | ➖ | ❌ |
| **Mandatory** static typing | ✅ | ✅ | ✅ | ❌ | ❌ |
| Type inference | ✅ | ✅ | ✅ | ➖ | ❌ |
| **Zero** annotations required | ✅ | ⚠️ | ⚠️ | ❌ | ➖ |
| ADTs + **enforced** exhaustiveness | ✅ | ✅ | ⚠️ | ⚠️ | ❌ |
| **Inferred** effects (never annotated) | ✅ | ❌ | ➖ | ❌ | ❌ |
| Units of measure | ✅ | ✅ | ❌ | ❌ | ❌ |
| Computation expressions | ✅ | ✅ | ❌ | ❌ | ❌ |
| Nested record-update (`{ p with a.b = v }`) | ✅ | ✅ | ❌ | ❌ | ➖ |
| Typed holes (type-driven dev) | ✅ | ❌ | ❌ | ❌ | ❌ |
| Chained comparisons (`a < b < c`) | ✅ | ❌ | ⚠️ | ✅ | ✅ |
| Compiler-as-gatekeeper | ✅ | ✅ | ✅ | ❌ | ❌ |
| **Self-contained output** (no runtime library) | ✅ | ❌ | ❌ | ❌ | ➖ |
| **No .NET / host-runtime toolchain** | ✅ | ❌ | ✅ | ➖ | ➖ |
| Python-library interop | ✅ | ✅ | ✅ | ✅ | ✅ |
| Maturity / production use | ❌ pre-1.0 | ⚠️ Py beta | ⚠️ | ✅ | ⚠️ |
| Language surface (built-in constructs) | ⚠️ small core | ✅ full F# | ⚠️ | ✅ Python superset | ✅ |
| Community, docs, support | ❌ solo | ✅ | ⚠️ | ✅ | ⚠️ |

Pyfun's strengths are the bold rows: **self-contained, runtime-free Python output** (a `List` is a
`list`, a record is a plain class), a **single dependency-free compiler** with no .NET, **inferred
effects**, and a language **designed for Python interop first**. On several rows it reaches past F#
itself, borrowing inferred effects from Koka, typed holes from Haskell and Idris, and Python-style
chained comparisons. Every tool here reaches the full Python ecosystem
(the interop row), so Pyfun's small core costs nothing in libraries; it just buys simplicity.

Reach for Fable when you want all of F# and are happy to bring the .NET toolchain and a runtime
library along. Reach for Pyfun when you want the emitted Python to be a first-class, readable
artifact you own outright, or when you are teaching functional programming to people who live in
Python.

---

## Getting started

Pyfun runs on the Python you already have. With **Python 3.12+** and pip, install the compiler:

```bash
pip install pyfun-lang
```

That puts the `pyfun` command on your `PATH`, with no Rust toolchain required. (The PyPI package is
`pyfun-lang`; the command it installs is `pyfun`.)

**Write your first program.** Save this as `hello.pyfun`:

```fsharp
type Shape = Circle float | Rect float float

let area s =
  match s:
    case Circle r: 3.14159 * r * r
    case Rect w h: w * h

print (area (Circle 2.0))
```

Then run it, type-check it, or see the Python it becomes:

```bash
pyfun run     hello.pyfun            # 12.56636
pyfun check   hello.pyfun            # type-check, rustc-style diagnostics
pyfun compile hello.pyfun            # emit readable Python to stdout
pyfun repl                           # interactive REPL
```

Multi-file projects just work: `import Geometry` pulls in a sibling `geometry.pyfun`, and any command
drives the whole graph. Clone the repo for a runnable tour in [`examples/`](https://github.com/simontreanor/Pyfun/tree/main/examples), including a
multi-module project (`pyfun run examples/modules/main.pyfun`).

**Building from source** (or hacking on the compiler) needs [Rust](https://rustup.rs/), which
auto-selects the pinned 1.96 toolchain:

```bash
cargo install --git https://github.com/simontreanor/Pyfun pyfun
# or, from a clone:  cargo install --path .
```

---

## Editor support

Pyfun ships a dependency-free language server (`pyfun lsp`) and a [VS Code
extension](https://marketplace.visualstudio.com/items?itemName=pyfun.pyfun). Over resilient analysis that survives a half-typed file, you get:

- **Diagnostics** as you type
- **Hover** showing the inferred type *and* effect of any expression, binding, or parameter
- **Go-to-definition** and **find-references**, across files
- **Rename**, project-wide, for values, constructors, and types
- **Completion**, **document symbols**, and **workspace symbols**

Install **Pyfun** from the VS Code Marketplace (or search "Pyfun" in the Extensions panel); once
`pyfun` is on your `PATH` (from `pip install pyfun-lang`), it launches `pyfun lsp` automatically.
Building the extension from source is covered in
[`editors/vscode/DEVELOPMENT.md`](https://github.com/simontreanor/Pyfun/tree/main/editors/vscode/DEVELOPMENT.md).

Not a VS Code user? Because the server is plain LSP over stdio, any editor with an LSP client
works — copy-paste configs for **Neovim**, **Helix**, and **Emacs** are in
[`editors/README.md`](https://github.com/simontreanor/Pyfun/tree/main/editors/README.md).

---

## How it works

Pyfun is a dependency-free Rust crate that runs a classic pipeline, and the compiler is the
gatekeeper: **nothing is emitted until every check passes.**

```
.pyfun ──► lexer ──► parser ──► Hindley–Milner type inference ──► Python-AST IR ──► readable .py
              │         │        (+ effects, units, exhaustiveness)      │
          offside    recursive                                     lowered, not
           rule       descent                                    string-spliced
```

- **Type inference** is full HM with let-generalization: you never annotate a value. The only
  types you write are in `type`/`extern` declarations, and every signature is inferred. It also does
  unit-of-measure inference (abelian-group unification), effect-row inference, and Maranget-style
  exhaustiveness with concrete witnesses.
- **Lowering** targets a Python-AST IR and emits real, formatted Python: curried functions
  collapse to n-ary `def`s and direct calls (closures only for genuine partial application), CEs
  desugar to their natural Python (`async`/`await`, generators, railway `Result`), and units erase.
- **No CPython fork.** Pyfun is a front end for the Python ecosystem, not a competing runtime.

The full language design and rationale live in [`DESIGN.md`](https://github.com/simontreanor/Pyfun/blob/main/DESIGN.md).

---

## Status

MVP showcase complete and runnable: ADTs, records, tuples, computation expressions (including
user-defined builders), units of measure, mutability, inferred multi-label effects, general Python
FFI via `extern`, a module-qualified standard library, string interpolation, active patterns,
typed holes, file-based modules, and a full LSP. See [`ROADMAP.md`](https://github.com/simontreanor/Pyfun/blob/main/ROADMAP.md) for what's next.

This is a solo, actively-developed project: the MVP is feature-complete and runnable, but it's
pre-1.0. Expect sharp edges; the language surface is stabilizing but not frozen.

---

## License

Pyfun is free and open source under the **[Apache License 2.0](https://github.com/simontreanor/Pyfun/blob/main/LICENSE)**: use, modify, and
redistribute it, including commercially. The accompanying [`NOTICE`](https://github.com/simontreanor/Pyfun/blob/main/NOTICE) names **Simon Treanor**
as the original author; keep it with any redistribution or derivative work.

Copyright © 2026 Simon Treanor.
