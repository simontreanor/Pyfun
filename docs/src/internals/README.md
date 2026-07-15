# Inside the compiler

A guided tour of the Pyfun compiler, read from the front. Pyfun is an F#-inspired,
functional-first language that type-checks a small ML-family surface and emits readable
Python, and its compiler is a dependency-free Rust crate. This tour walks the pipeline
one stage at a time, from raw source text to the diagnostics and tooling on the other end.

## Two audiences, one path

The tour is written for two readers at once.

- **People learning how a language is built.** Each stage of a real compiler is here in
  order: a hand-written lexer, a recursive-descent parser, a desugaring pass, Hindley-Milner
  type inference, exhaustiveness checking, lowering, and code emission. The chapters explain
  what each stage is responsible for and why the boundaries fall where they do.
- **People learning Rust by reading a real codebase.** The crate takes no dependencies, so
  every data structure and algorithm is in the tree and readable end to end. Chapters point at
  small, real excerpts and call out Rust idioms (enums and exhaustive `match`, ownership,
  trait derivation) as they appear in context.

It also doubles as **contributor onboarding**. Every numbered chapter closes with a short
"Where you would add..." note naming the first file and function a contributor touches for a
plausible change at that stage, so the tour is also a map from an intended change to the code
that implements it.

The tour summarizes and points *into* two reference documents rather than duplicating them:
[`DESIGN.md`](https://github.com/simontreanor/Pyfun/blob/main/DESIGN.md) is the source of
truth for what the language does and why, and
[`INTERNALS.md`](https://github.com/simontreanor/Pyfun/blob/main/INTERNALS.md) is the map from
a semantic rule to the module that implements it. When you want the full rule, follow the link.

## The running example

Every chapter follows the same short program. The second half of the tour uses it too, so you
can carry one mental model the whole way through.

```pyfun
type Shape = Circle float | Rect float float

let area s =
  match s:
    case Circle r: 3.14159 * r * r
    case Rect w h: w * h

measure m

let shapes = [Circle 2.0, Rect 3.0 4.0]
let total = List.fold (fun acc s -> acc + area s) 0.0 shapes
let side = sqrt 16.0<m^2>
print (f"total {total}, side {side}")
```

It is small, but it exercises a lot of the surface: an algebraic data type, a curried function
with an exhaustive `match`, a unit-of-measure declaration and annotation, a list literal, a
higher-order stdlib call with a lambda, and an interpolated `print`. Compiled and run, it
produces:

```
total 24.56636, side 4.0
```

## The pipeline at a glance

Pyfun's central design commitment is that **the compiler is the gatekeeper**: every
type, effect, exhaustiveness, immutability, and unit check runs before any Python is emitted, so
a program that compiles cannot fail those checks at runtime. The stages below run in order, and
each one gates the next.

```
source text
  -> lexing              (01)  tokens + offside layout
  -> parsing             (02)  a span-carrying AST
  -> desugaring          (03)  builders/sections collapse to core forms
  -> type inference      (04)  Hindley-Milner + effects
  -> exhaustiveness      (05)  every ADT variant handled
  -> units of measure    (06)  dimensional analysis, erased at runtime
  -> lowering            (07)  Pyfun AST -> Python-AST IR
  -> emission            (08)  readable Python source
  -> diagnostics         (09)  rustc-style errors throughout
  -> the compiler as a library (10)  the same front end, in the CLI and editor
```

The chapters:

- **[00 - Orientation](00-orientation.md)**: the gatekeeper thesis, the dependency-free ethos,
  the crate layout and build order, and how to build and test.
- **[01 - Lexing](01-lexing.md)**: tokens, the offside rule, unit annotations, holes.
- **[02 - Parsing](02-parsing.md)**: recursive descent, precedence climbing, the span-carrying
  AST, and error recovery.
- **[03 - Desugaring](03-desugaring.md)**: computation-expression builders, operator sections,
  and composition collapsing to core forms.
- **[04 - Type inference](04-inference.md)**: Hindley-Milner with let-generalization, inferred
  effects, and the check-not-annotate philosophy.
- **[05 - Exhaustiveness](05-exhaustiveness.md)**: Maranget-style usefulness and concrete
  witnesses.

The second half of the tour continues with units of measure (06), lowering (07), emission (08),
diagnostics (09), and the compiler as a library, in the CLI and the editor (10).
