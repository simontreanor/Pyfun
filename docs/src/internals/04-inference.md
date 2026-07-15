# 04 - Type inference

This is where the gatekeeper does most of its work. The checker in
[`src/types/`](https://github.com/simontreanor/Pyfun/blob/main/src/types/mod.rs) runs
Hindley-Milner type inference, and alongside the ordinary types it infers units of measure and
effects in the same pass. The module's own header states the shape:

```rust
// src/types/mod.rs
//! Algorithm W with a substitution map and let-generalization, so top-level
//! bindings are polymorphic. Functions are curried.
```

## Inference with let-generalization

Algorithm W walks the AST, inventing fresh type variables for unknowns and unifying them as it
learns constraints, accumulating the results in a substitution map. At a `let` binding it
**generalizes**: any type variable in the inferred type that is not pinned by the surrounding
environment is quantified, turning a concrete type into a reusable scheme. That is what makes
`let id x = x` usable at many types, and it is the reason a top-level definition needs no
annotation to be polymorphic.

Consider `area` from the running example. Its body is a `match` on `s` with two arms, `Circle r`
and `Rect w h`. Matching `s` against the `Circle`/`Rect` constructors forces `s` to have type
`Shape`. Both arms compute with floats (`3.14159 * r * r` and `w * h`), so the result unifies to
`float`. Nothing was annotated; the type `Shape -> float` fell out of the constructor patterns and
the arithmetic. We can see the checker's reasoning by planting a typed hole where `area`'s argument
goes and running `pyfun check`:

```
note: hole `?s` has type `Shape` — or: Circle ?, Rect ? ?
 --> 8:18
  |
8 | let probe = area ?s
  |                  ^^
```

The checker inferred that whatever fills `?s` must be a `Shape`, and it even suggests the two
constructors that build one. That is the inferred parameter type of `area`, surfaced without a
single annotation in the program.

## Check, do not annotate

The absence of type annotations on `let` and lambda is a deliberate design choice, not a missing
feature. HM inference is complete enough that the compiler never needs them, and annotation-free
code is part of the point:
[`DESIGN.md`](https://github.com/simontreanor/Pyfun/blob/main/DESIGN.md) §3 makes optional
`let x : T` annotations an explicit non-goal. The one place types are written on purpose is at the
`extern` boundary, where Pyfun is told the type of a Python function it cannot infer. Everywhere
else the compiler infers and the tooling reports: hover, `pyfun check`, and the REPL's `:type` all
surface the inferred type, so the type is always available even though it is never in the source.

The type machinery is a little richer than textbook HM because Pyfun's numbers and units ride the
same inference. A single built-in `num` constraint keeps arithmetic polymorphic over `int` and
`float`, so `let add a b = a + b` infers `num 'a => 'a -> 'a -> 'a` and works at both. Units are an
abelian group carried on numeric types and generalized like type variables, so `let area w h = w *
h` infers a unit-polymorphic `int<'u> -> int<'v> -> int<'u 'v>`. Both are still Algorithm W; they
just extend what a variable can be constrained by, and both are solved during the same walk.

## Effects, inferred in the same pass

Effects are tracked in the type system rather than bolted on, and they are inferred, never written
in ordinary code. Each function arrow carries a latent effect, a set of concrete labels plus effect
variables for polymorphism:

```rust
// src/types/mod.rs
pub enum EffLabel {
    /// Observable side effects: printing, `<-` mutation, the (non-`pure`) Python
    /// FFI boundary.
    Io,
    /// Asynchronous execution. Produced ... by an `async {}` CE block ...
    Async,
}
```

The rules are propagation rules. `print` is `'a ->{io} unit`, impurity flows outward (calling an
impure function makes the caller impure), and labels from different calls union. A pure function
reads exactly as it always did, with a plain `->`. The one opt-in is the definition-level
assertion `let pure f ... = ...`, which is a compile error if the binding introduces any concrete
label. That assertion is how the effect inference becomes visible on the command line. Wrapping the
example's `print` in a `pure` binding is rejected, and the error names the exact label that was
inferred:

```
error: `greet` is declared `pure` but performs `io`
 --> 2:3
  |
2 |   print (f"hello {name}")
  |   ^^^^^^^^^^^^^^^^^^^^^^^
```

The checker inferred `io` from the `print` call and propagated it up to the binding, then checked
it against the `pure` assertion and found the contradiction. Effects are fully erased at lowering,
so none of this leaves any residue in the emitted Python. Like units, effect tracking is maximum
information at compile time and zero cost at runtime.

## Where you would add a builtin's type

The prelude and the stdlib modules are seeded as type schemes in
[`src/types/mod.rs`](https://github.com/simontreanor/Pyfun/blob/main/src/types/mod.rs): the single
source of truth for each is a names-and-arities table (`PRELUDE`, `LIST_PRELUDE`, and the rest)
paired with a `seed_*` function that builds the scheme. To add a builtin you add its entry and its
scheme there, marking the effect on the relevant arrow if it is not pure. Lowering reads the same
arity table so partial application still works, which the [lowering chapter](README.md) picks up.
