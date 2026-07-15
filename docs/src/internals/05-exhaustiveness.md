# 05 - Exhaustiveness

A Pyfun `match` must handle every case. This is one of the safety guarantees the compiler enforces
before any Python exists (see
[`DESIGN.md`](https://github.com/simontreanor/Pyfun/blob/main/DESIGN.md) §3): a `match` that misses
a variant is a compile error, not a runtime surprise. The check lives in
[`src/types/`](https://github.com/simontreanor/Pyfun/blob/main/src/types/mod.rs) and runs as part
of type checking, because it needs the inferred type of the scrutinee to know what the complete set
of cases is.

## Usefulness, not case-counting

A naive exhaustiveness check might count constructors, but that breaks down as soon as patterns
nest: `Some 0 | Some n | None` should be exhaustive, and `Some (Some x)` should not be. Pyfun uses
the standard, principled approach, Maranget's usefulness algorithm. The `check_exhaustive` method
documents the idea:

```rust
// src/types/mod.rs
/// Deep exhaustiveness via Maranget's usefulness algorithm ("Warnings for
/// pattern matching", JFP 2007). A `match` is exhaustive iff a wildcard row is
/// *not* useful against the matrix of arm patterns — i.e. there is no value the
/// wildcard would catch that no arm already does.
```

The reframing is what makes it work. Instead of asking "are all cases covered," the algorithm asks
"is a bare wildcard *useful* against the existing arms," meaning: is there some value the wildcard
would match that no arm already matches. If the wildcard is useless, every value is already covered
and the match is exhaustive. If it is useful, the value it would catch is precisely a case that is
missing, and the algorithm hands that value back as a **witness**.

The arms become a matrix of patterns, one row per unguarded arm (guarded arms may fail at runtime,
so they never count toward coverage), and `useful` recurses column by column. It specializes the
matrix against each constructor of the column's type and recurses into the constructor's fields,
which is what gives *deep* exhaustiveness: nested patterns are checked all the way down, not just
at the top level. A short internal enum, `Wit`, represents the witness it builds on the way back
up, a constructor applied to sub-witnesses or `_` for "any value."

## A witness from the running example

Delete the `Rect` arm from `area`, leaving only `Circle`:

```pyfun
let area s =
  match s:
    case Circle r: 3.14159 * r * r
```

Running `pyfun check` reports:

```
error: non-exhaustive match: `Rect _ _` is not matched
 --> 4:3
  |
4 |   match s:
  |   ^^^^^^^^

1 error
```

That message is the algorithm's output, rendered. The scrutinee's type is `Shape`, whose
constructors are `Circle` and `Rect`. With only the `Circle` arm present, a wildcard is still
useful, because a `Rect` value would slip past it, and the witness the algorithm produces is `Rect`
applied to two wildcards. `render_witness` prints that witness back in Pyfun's own pattern syntax,
so the error does not just say "a case is missing," it names the exact missing shape, `Rect _ _`,
which is the pattern you would write to fix it. When no constructor is missing but a leaf over an
infinite type is (a string or an integer literal match), the witness is `_` and the message instead
suggests adding a wildcard arm.

Two things are worth noticing about the boundary with the [inference chapter](04-inference.md).
First, exhaustiveness runs *inside* type checking: `check_exhaustive` takes the already-inferred
scrutinee type, so the two passes are not separable stages but one traversal. Second, the same
usefulness machinery drives more than plain ADTs. Tuples recurse into their element columns,
records into their fields, and even `List` sequence patterns are checked by modeling a list as the
finite `Nil | Cons` type inside the algorithm only, with no real ADT and no change to lowering.

## Why this earns its place

Exhaustiveness is the check that most directly repays the ML-family surface. Because the compiler
proves every variant is handled, the emitted Python can lower a `match` to a real `match`/`case`
statement and still add a defensive `case _: raise` arm that, by construction, is unreachable. The
program is total on its ADTs before it ever runs, and adding a variant to a type turns every
now-incomplete match into a compile error that points at the exact gap. That is the gatekeeper
model working for the programmer rather than against them.

## Where you would add a new pattern form

A new kind of pattern touches three places in
[`src/types/mod.rs`](https://github.com/simontreanor/Pyfun/blob/main/src/types/mod.rs): binding and
typing it (`bind_pattern`), and teaching the usefulness algorithm how it specializes (the column
helpers `default_matrix` and `expand_first_column`, plus the witness rendering in
`render_witness`). The `List`/`Nil`-`Cons` modeling and the tuple-column recursion are the two
worked examples to follow. A pattern that adds no coverage power, such as an as-pattern, can instead
be made transparent by peeling it before these steps, which is how Pyfun handles `case p as x`.
