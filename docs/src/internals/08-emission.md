# 08 - Emission

Lowering produces a Python-AST IR; emission turns it into text. The IR and the emitter both
live in
[src/python_emitter/mod.rs](https://github.com/simontreanor/Pyfun/blob/main/src/python_emitter/mod.rs),
and the design goal (per [DESIGN.md §5](https://github.com/simontreanor/Pyfun/blob/main/DESIGN.md))
is blunt: the output is the interface. Pyfun compiles to Python you are meant to read, keep in
your repo, and hand to Python tooling, so the emitted source has to look like something a person
would write.

## A small Python IR

`PyModule` is a flat list of `PyStmt`, and the statement enum covers only what lowering produces:
`Import`, `Assign`, `Return`, `Expr`, `FuncDef`, `If`, `Match`, `Try`, `ClassDef`, and a handful
more. If you are new to Rust, this is a textbook use of an `enum`: each statement shape is one
variant carrying exactly its own fields, and the emitter is one `match` over them. The IR grows
only when the language does, so there is no dead abstraction to wade through.

```rust
// src/python_emitter/mod.rs
pub enum PyStmt {
    Import(String),
    Assign { target: String, value: PyExpr },
    Return(PyExpr),
    Match { subject: PyExpr, cases: Vec<PyCase> },
    // ...
}
```

## The emitter is line-based and deterministic

`emit` walks the module, emits the imports that dataclasses need once at the top, then recurses
through `emit_block`/`emit_stmt` with a depth counter. Indentation is four spaces per level via a
single `line` helper. There is no formatter pass and no randomness: the same IR always produces
byte-identical text. That determinism is load-bearing beyond tidiness. The REPL and Jupyter
kernel (see [tooling](10-tooling.md)) split the emitted program into top-level chunks and send
only the chunks the worker has not run yet, which is only sound because re-emitting an unchanged
definition yields the exact same bytes.

## Match lowers one-for-one

A Pyfun `match` becomes a Python `match`, arm for arm. `emit_stmt` renders the subject, then each
case with its optional guard:

```rust
// src/python_emitter/mod.rs
PyStmt::Match { subject, cases } => {
    line(out, depth, &format!("match {}:", expr(subject)));
    for case in cases {
        let guard = match &case.guard {
            Some(g) => format!(" if {}", expr(g)),
            None => String::new(),
        };
        line(out, depth + 1, &format!("case {}{guard}:", pattern(&case.pattern)));
        emit_block(&case.body, depth + 2, out);
    }
}
```

Constructor patterns emit as Python class patterns (`case Circle(r):`), which is why Pyfun's ADTs
lower to `@dataclass` classes with generated `__match_args__`: it lets the emitted match read
like idiomatic structural Python. The checker has already proven the match exhaustive, so the
trailing `case _: raise RuntimeError(...)` is a belt-and-braces guard, not something the program
should ever hit.

## The running example, in full

Here is the complete emitted Python for the running example, straight from `pyfun compile`:

```python
from dataclasses import dataclass
import functools
import math
def _pf_fold(f, acc, xs):
    return functools.reduce(f, xs, acc)
@dataclass(frozen=True, repr=False)
class Circle:
    _0: float
    def __repr__(self):
        return f"Circle({self._0!r})"
@dataclass(frozen=True, repr=False)
class Rect:
    _0: float
    _1: float
    def __repr__(self):
        return f"Rect({self._0!r}, {self._1!r})"
def area(s):
    match s:
        case Circle(r):
            return 3.14159 * r * r
        case Rect(w, h):
            return w * h
        case _:
            raise RuntimeError("non-exhaustive match")
shapes = [Circle(2.0), Rect(3.0, 4.0)]
total = _pf_fold(lambda acc, s: acc + area(s), 0.0, shapes)
side = math.sqrt(16.0)
print(f"total {total}, side {side}")
```

Walking through it: the header imports are emitted once because a class is present and helpers
are needed. `_pf_fold` is the emitted-on-demand `List.fold` helper, a thin wrapper over
`functools.reduce` so the curried Pyfun function is a single callable. `Circle` and `Rect` are
frozen dataclasses: `frozen=True` matches Pyfun's immutability, and the hand-written positional
`__repr__` (with `repr=False` suppressing the generated one) makes a value print the way it was
constructed, `Circle(2.0)` rather than `Circle(_0=2.0)`. `area` is the return-position match.
The three top-level `let`s become plain assignments, the fold folder is an inline `lambda`,
`sqrt` erased its unit down to `math.sqrt(16.0)`, and the final `print` uses a real Python
f-string. Running it prints `total 24.56636, side 4.0`. Nothing about this file signals that it
came from a compiler.

## Where you would add a new emitted construct

A new Python shape (say a `while` loop for a future optimization) starts as a variant on `PyStmt`
or `PyExpr` in
[src/python_emitter/mod.rs](https://github.com/simontreanor/Pyfun/blob/main/src/python_emitter/mod.rs),
with its rendering arm in `emit_stmt`/`emit_expr`, and only then does lowering get to produce it.
Adding the IR node first keeps the "no string splicing" contract intact and gives you one place
to get indentation and precedence right.
