# Pyfun

**Pyfun is to Python what F# is to C#.** An FP-first language — immutable by default,
expression-oriented, fully type-inferred — whose compiler is written in Rust and emits
**readable Python**. You write typed, exhaustively-checked functional code; you get plain
`.py` files that call straight into NumPy, pandas, httpx, or anything else on PyPI.

```fsharp
type Summary = { n: int, mean: float, stdev: float }

extern pure mean:  List float -> float = statistics.mean
extern pure stdev: List float -> float = statistics.stdev

let summarize xs =
  Summary { n = List.len xs, mean = mean xs, stdev = stdev xs }

print (summarize [1.0, 2.0, 3.0, 4.0])
```

> No syntax errors that Python would have found only at runtime. No `TypeError` an hour into a
> job. The Rust compiler checks types, effects, units, and match exhaustiveness **before a single
> line of Python is emitted** — then hands you code you can read, diff, and ship.

---

## Why Pyfun over plain Python?

Python is the best ecosystem in the world and one of the least safe languages to write large
programs in. Pyfun keeps the ecosystem and removes the footguns.

| | Plain Python | Pyfun |
|---|---|---|
| **Type errors** | found at runtime, in production | found at compile time, by the Rust checker |
| **`None` handling** | `AttributeError: 'NoneType'…` | `Option a` with exhaustive `match` — the compiler makes you handle `None` |
| **Missing `case`** | silently falls through | **exhaustiveness error** with a concrete missing-case witness |
| **Mutation** | everything is mutable, everywhere | immutable by default; `let mut` + `<-` is opt-in and tracked |
| **Side effects** | invisible | **inferred and tracked** — `let pure` is a compile-checked promise |
| **Units / dimensions** | a comment and a prayer | `10<N> / 2<m^2> : float<Pa>`, checked and then erased |
| **Refactoring** | grep and hope | a real language server: types on hover, rename, go-to-def, find-refs |
| **Runtime** | CPython | **CPython** — Pyfun *is* Python once compiled |

You don't give up the ecosystem to get any of this. That's the whole point.

---

## The killer feature: type-checked Python interop

`extern` imports **any** Python callable or value at a Pyfun type. The dotted target is imported
for you; the boundary is *effectful by default* (a Python call can do anything), and `pure` opts
out where you know better. Once imported, the function is a first-class curried Pyfun value —
type-checked, effect-tracked, partially applicable.

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
import statistics

class Summary:
    __match_args__ = ('n', 'mean', 'stdev')
    def __init__(self, n, mean, stdev):
        self.n = n
        self.mean = mean
        self.stdev = stdev
    def __repr__(self):
        return f"Summary({self.n!r}, {self.mean!r}, {self.stdev!r})"
    # ...structural __eq__/__hash__/ordering elided...

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

Notice what the compiler did and didn't do:

- **No wrapper layer.** `statistics.mean(xs)` is called directly. `List` *is* a Python `list`;
  a Pyfun record *is* a plain class. There is no runtime, no VM, no marshalling.
- **Effects tracked across the boundary.** A bare `extern` is `io` at full application, so it
  can't be called from a `let pure`. Mark it `pure` (like `statistics.mean`) and it composes into
  pure code. You can even annotate other effect labels: `extern fetch: string ->{async} string = httpx.get`.
- **Exceptions become values.** `try (parseInt s) : Result int Exception` catches whatever the
  Python side raises and hands you a `Result` to `match` on — the imperative FFI edge turns into
  the FP error type, with `errorKind` / `errorMessage` fields.

```fsharp
extern parseInt: string -> int = int          # Python's built-in int()

let safe s = Result.withDefault 0 (try (parseInt s))
print (safe "42")     # 42
print (safe "oops")   # 0   (the ValueError was caught into an Error)
```

---

## A whistle-stop tour

Everything below type-checks, compiles, and runs today. See
[`examples/hello.pyfun`](examples/hello.pyfun) for the exhaustive version.

**Algebraic data types, records, and exhaustive matching** — `None` cannot bite you:

```fsharp
type Shape = Circle float | Rect float float

let area s =
  match s:
    case Circle r: 3.14159 * r * r
    case Rect w h: w * h
# forget a case and the compiler reports the missing witness, e.g. `Rect _ _ is not matched`
```

**Pipelines, currying, composition** — F#'s `|>`, `<|`, `>>`, `<<`, operator sections `(+)`:

```fsharp
let describe =
  List.filter (fun x -> x > 0)
  >> List.map ((*) 2)
  >> List.fold (+) 0

let total = [1, -2, 3] |> describe    # (1 + 3) * 2 = 8
```

**Inferred effects** — purity is a checked promise, never boilerplate:

```fsharp
let pure add a b = a + b        # OK: no effects
# let pure shout n = print n    # compile error: `print` performs `io`
```

**Units of measure** — dimensional analysis at compile time, erased at runtime:

```fsharp
measure m
measure s
measure kg
measure N = kg m / s^2          # derived aliases expand to base units

let speed = 100<m> / 10<s>      # float<m/s>
let force = 10<N>
# let bad = 100<m> + 10<s>      # compile error: m vs s
let side = sqrt 16.0<m^2>       # float<m> — unit-aware roots
```

**Computation expressions** (F#'s showcase feature) — `result`, `seq`, `async`, plus your own:

```fsharp
let checked ok v =
  result {                      # railway-oriented; short-circuits on Error
    let! x = if ok then Ok v else Error "bad"
    return x + 1
  }
```

**Rich literals and strings** — f-strings, raw strings, triple-quotes, scientific notation,
digit separators, hex/octal/binary:

```fsharp
let planck = 6.626e-34
let million = 1_000_000
let mask = 0xFF
let who = "Ada"
let line = f"{who} scored {million} ({String.upper who})"
let path = r"C:\Users\pyfun"    # raw string — backslashes literal
```

And a standard library that reads like F#'s: module-qualified `List` / `Set` / `Map` / `Option` /
`Result` / `Seq` / `String` (`List.map`, `Map.tryFind`, `Result.bind`, lazy `Seq.take`,
`String.split`), tuples, active patterns, typed holes for type-driven development, and
multi-file projects with `import`.

---

## Getting started

You need [Rust](https://rustup.rs/) (the toolchain is pinned to 1.96) to build the compiler, and
**Python 3.12+** on your `PATH` to run the emitted code.

```bash
git clone https://github.com/simontreanor/Pyfun
cd Pyfun
cargo build --release

# Type-check, compile, and run:
cargo run -- check   examples/hello.pyfun     # rustc-style diagnostics
cargo run -- compile examples/hello.pyfun     # emit Python to stdout
cargo run -- compile examples/hello.pyfun -o out.py
cargo run -- run     examples/hello.pyfun     # compile + execute via python
cargo run -- parse   examples/hello.pyfun     # canonical pretty-print
cargo run -- repl                             # interactive REPL
```

Multi-file projects just work — `import Geometry` pulls in a sibling `geometry.pyfun`, and the CLI
drives the whole graph:

```bash
cargo run -- run examples/modules/main.pyfun
```

---

## Editor support

Pyfun ships a dependency-free language server (`pyfun lsp`) and a VS Code client in
[`editors/vscode/`](editors/vscode/). You get, over resilient analysis that survives a half-typed
file:

- **Diagnostics** as you type
- **Hover** showing the inferred type *and* effect of any expression, binding, or parameter
- **Go-to-definition** and **find-references** — across files
- **Rename** — project-wide, for values, constructors, and types
- **Completion**, **document symbols**, and **workspace symbols**

---

## How it works

Pyfun is a dependency-free Rust crate that runs a classic pipeline — and the compiler is the
gatekeeper: **nothing is emitted until every check passes.**

```
.pyfun ──► lexer ──► parser ──► Hindley–Milner type inference ──► Python-AST IR ──► readable .py
              │         │        (+ effects, units, exhaustiveness)      │
          offside    recursive                                     lowered, not
           rule       descent                                    string-spliced
```

- **Type inference** is full HM with let-generalization, so you rarely write a type; it also does
  unit-of-measure inference (abelian-group unification), effect-row inference, and Maranget-style
  exhaustiveness with concrete witnesses.
- **Lowering** targets a Python-AST IR and emits real, formatted Python — curried functions
  collapse to n-ary `def`s and direct calls (closures only for genuine partial application), CEs
  desugar to their natural Python (`async`/`await`, generators, railway `Result`), and units erase.
- **No CPython fork.** Pyfun is a front end for the Python ecosystem, not a competing runtime.

The full language design and rationale live in [`DESIGN.md`](DESIGN.md).

---

## Status

MVP showcase complete and runnable: ADTs, records, tuples, computation expressions (including
user-defined builders), units of measure, mutability, inferred multi-label effects, general Python
FFI via `extern`, a module-qualified standard library, string interpolation, active patterns,
typed holes, file-based modules, and a full LSP. See [`ROADMAP.md`](ROADMAP.md) for what's next and
[`GUIDE.md`](GUIDE.md) for the working notes.

This is a solo research project under active development. Expect sharp edges; the language surface
is stabilizing but not frozen.

---

## License and attribution

Pyfun is free and open source under the **[Apache License 2.0](LICENSE)**.

You may use, modify, and redistribute it, including commercially. In return, the license requires
that attribution to the original author travels with the code: if you redistribute Pyfun or build a
derivative work, you must retain the [`LICENSE`](LICENSE) and reproduce the [`NOTICE`](NOTICE) file,
which names **Simon Treanor** as the creator and original author of Pyfun.

The **Pyfun** language, its Rust compiler, and this design are the original work of Simon Treanor.
Please keep the credit.

Copyright © 2026 Simon Treanor.
