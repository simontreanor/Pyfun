# Contributing to Pyfun

Thank you for taking a look. Pyfun is an F#-inspired, functional-first language whose compiler is
written in Rust and emits readable Python. Contributions of every size are welcome: a clearer error
message, a missing test, a lesson that explains something better, or a compiler change.

## Getting set up

You need two things.

- **Rust.** The repository pins its toolchain in `rust-toolchain.toml`, and `rustup` selects it
  automatically inside the repo. The crate has no dependencies, so the first build is quick.
- **Python 3.12 or newer** on your `PATH` as `python` or `python3`. The end-to-end tests run the
  emitted Python, and emitted code uses `match` statements and PEP 701 f-strings.

Then:

```bash
cargo build
cargo test                                 # the whole suite
cargo test parse_print                     # one test, by substring
cargo run -- run examples/hello.pyfun      # compile and run a program
cargo run -- compile examples/hello.pyfun  # see the Python it emits
```

On Windows, an editor running the Pyfun language server keeps `target/debug/pyfun.exe` open, so a
build can fail with "Access is denied". Stop the `pyfun` process, or build into a second directory
with `CARGO_TARGET_DIR=target-test`.

## Finding your way around

Start with the [compiler tour](https://simontreanor.github.io/Pyfun/internals/), which walks the
pipeline from lexing to emission one chapter at a time. Every chapter ends with a **"Where you would
add..."** note naming the first file and function you would touch for a typical change at that stage:

| You want to... | Read |
| --- | --- |
| add a token or keyword | [Lexing](docs/src/internals/01-lexing.md) |
| add an expression form | [Parsing](docs/src/internals/02-parsing.md) |
| add syntax sugar | [Desugaring](docs/src/internals/03-desugaring.md) |
| give a builtin its type | [Inference](docs/src/internals/04-inference.md) |
| add a pattern form | [Exhaustiveness](docs/src/internals/05-exhaustiveness.md) |
| add a measure or root | [Units](docs/src/internals/06-units.md) |
| lower a stdlib function | [Lowering](docs/src/internals/07-lowering.md) |
| emit a new Python construct | [Emission](docs/src/internals/08-emission.md) |
| improve an error message | [Diagnostics](docs/src/internals/09-diagnostics.md) |
| add an editor feature | [Tooling](docs/src/internals/10-tooling.md) |

Three documents at the root carry the detail. [`DESIGN.md`](DESIGN.md) is the source of truth for
what the language means and why; read the relevant section before changing a language rule.
[`INTERNALS.md`](INTERNALS.md) maps each rule to the code that implements it. [`ROADMAP.md`](ROADMAP.md)
holds the backlog, including the non-goals, each with the reason it was decided against.

The issues labelled [good first issue](https://github.com/simontreanor/Pyfun/labels/good%20first%20issue)
are small and self-contained, and each one names the file to start from.

## Making a change

1. Open an issue first for anything that changes the language itself, so the design can be agreed
   before you spend time on code. Bug fixes, tests, diagnostics and docs can go straight to a PR.
2. Add a test next to the ones like it. `tests/roundtrip.rs` covers parsing, `tests/typecheck.rs` the
   checker and its messages, `tests/compile.rs` the emitted Python and what it prints, and
   `src/lsp/mod.rs` the language server.
3. Run the same checks CI runs:

   ```bash
   cargo fmt
   cargo clippy --all-targets
   cargo test
   ```

4. Update `DESIGN.md` or `INTERNALS.md` in the same PR when your change touches what they describe,
   and strike through any `ROADMAP.md` item it closes.

A few principles shape what gets merged:

- **The emitted Python should read as if a person wrote it.** Lowering builds a Python syntax tree
  in Rust and prints it, and a fully applied curried call becomes a plain `f(a, b)`.
- **New syntax mirrors a form Python programmers already know.** Ask how Python spells the idea
  before inventing a spelling.
- **Error messages say what to do next.** A message that names the fix is worth more than a new
  feature.
- **The scope is deliberately small.** Proposals for type classes, macros and similar are listed as
  non-goals in `ROADMAP.md`, with the reasoning.

## Writing lessons and docs

The learning site lives in `docs/src` and builds with mdBook. Lessons talk to the reader as "you",
show an example before stating a rule, and keep to plain prose. Every code block in a lesson is
checked against the real compiler, so after editing one run:

```bash
cargo build
python docs/verify_lessons.py
```

It checks that each playground link decodes to the starter shown beside it and that each solution
prints what the lesson says it prints.

## Licence

Code contributions are made under the [Apache License 2.0](LICENSE), the licence of the project.
Teaching prose on the learning site is licensed CC BY 4.0.
