# 00 - Orientation

Before we open the lexer, it helps to know the one idea the whole compiler is arranged around,
the constraint the crate holds itself to, and where the pieces live.

## The gatekeeper thesis

Python compiles to untyped bytecode: the runtime offers no compile-time guarantees. Pyfun gets
F#-level safety the way TypeScript, Elm, and Haskell do, by putting a real compiler in front of
the runtime. [`DESIGN.md`](https://github.com/simontreanor/Pyfun/blob/main/DESIGN.md) §2 states
it plainly: **the Rust compiler enforces everything before any Python is emitted.** A failed
check stops compilation and produces a rustc-style diagnostic, and Python never runs.

That single commitment explains the shape of the pipeline. Type inference, effect checking,
exhaustiveness, immutability, and unit checking all happen on the Pyfun AST, upstream of
lowering. Only once a program has passed every gate does the compiler build a Python-AST IR and
emit source. So the emitted Python is not where safety lives; it is the *output* of a process
that already proved the program safe. This is why, in later chapters, you will see the checker
carry rich structure (schemes, effect sets, a usefulness matrix) while lowering and emission
stay comparatively mechanical. The hard thinking is done before a single line of Python exists.

## The dependency-free ethos

The crate takes no third-party dependencies. The lexer and parser are hand-written, the
diagnostics renderer is hand-written, and even the language server's JSON layer is hand-rolled
(`src/lsp/json.rs`) rather than pulling in `serde`. For a tour aimed partly at Rust learners this
is a gift: every algorithm the compiler runs is in this tree, readable start to finish, with
nothing hidden behind a crate boundary. It also keeps the shipped binary small and its behavior
fully owned. The tradeoff is that Pyfun re-implements a few things a dependency would give for
free, and the code is written with that in mind, favoring small single-purpose modules.

## Crate layout and build order

[`INTERNALS.md`](https://github.com/simontreanor/Pyfun/blob/main/INTERNALS.md) is the canonical
module map; the short version is that each stage of the pipeline is its own directory under
`src/`:

- `src/lexer/` tokenizer and token types
- `src/parser/` recursive descent, with `parser/ast.rs` holding the span-carrying AST
- `src/ast/` traversal and the canonical pretty-printer
- `src/desugar.rs` computation-expression desugaring, sections, composition
- `src/types/` Hindley-Milner inference, effects, units, exhaustiveness
- `src/lowering/` Pyfun AST to Python-AST IR
- `src/python_emitter/` the IR to readable Python source
- `src/diagnostics/` rustc-style error rendering
- `src/main.rs` the CLI driver, `src/project/` the module graph
- `src/lsp/` the language server

The dependency order among them is the pipeline order:

> **Build order:** `lexer` + `parser` + `ast` -> `desugar` -> `types` (incl. `units`) ->
> `lowering` + `python_emitter` -> `diagnostics` + `cli` -> `lsp`.

Reading the modules in that order is reading the compiler from front to back, which is exactly
what this tour does.

## The three reference documents

Keep three files in view as you read the code.
[`DESIGN.md`](https://github.com/simontreanor/Pyfun/blob/main/DESIGN.md) is the semantic source
of truth: what a construct means and why it was designed that way.
[`INTERNALS.md`](https://github.com/simontreanor/Pyfun/blob/main/INTERNALS.md) is the
implementation map: which module and function carries a given rule.
[`ROADMAP.md`](https://github.com/simontreanor/Pyfun/blob/main/ROADMAP.md) holds the backlog and
the non-goals. A change to the language touches DESIGN; a change to how the compiler realizes it
touches INTERNALS. This tour points into both rather than restating them.

## Building and testing

The toolchain is pinned (Rust 1.97.0) for reproducible formatting and lint. The everyday loop is
short:

```bash
cargo build                                 # build the compiler
cargo test                                  # lexer + roundtrip + typecheck + compile/e2e
cargo run -- check   examples/hello.pyfun   # type-check with rustc-style diagnostics
cargo run -- compile examples/hello.pyfun   # lower to Python on stdout
cargo run -- run     examples/hello.pyfun   # compile then execute via python
```

The test suite leans on **snapshot and golden tests**: the parser round-trips source through
its canonical pretty-printer, the checker compares diagnostics against expected text, and the
end-to-end tests run the emitted Python through a real interpreter (skipping, not failing, when
none is on `PATH`). This style keeps the tests close to observable behavior, which is the same
lens the rest of this tour takes. When a chapter shows CLI output, it is the real output of the
`pyfun` binary on the running example from the [tour landing](README.md).
