# 06 - Units of measure

Units of measure are Pyfun's clearest demonstration of the gatekeeper model: the compiler
does full dimensional analysis, then throws all of it away before emitting Python. The rule
is in [DESIGN.md §8.2](https://github.com/simontreanor/Pyfun/blob/main/DESIGN.md); the code
lives alongside HM inference in
[src/types/mod.rs](https://github.com/simontreanor/Pyfun/blob/main/src/types/mod.rs), in the
same file as the type checker rather than a separate sub-module, because unit unification is
part of unification itself.

## Units as a free abelian group

A unit is a product of base measures and unit variables raised to integer powers, with a
dimensionless identity. That is exactly a free abelian group, and the representation follows
directly:

```rust
// src/types/mod.rs
pub struct Unit {
    factors: BTreeMap<Atom, i32>,
}
```

`Atom` is either `Base(String)` (a declared measure like `m`) or `Var(u32)` (a unit variable
the checker will solve for). The map never stores a zero exponent, so `m^0` is not present and
two equal units have identical maps. The group operations are the obvious ones over that map:
`mul` adds exponents key by key, `inv` negates them, and `pow(k)` multiplies each by `k`. If
you are new to Rust, note that `BTreeMap` keeps the factors in a stable sorted order, which is
what lets equality and display stay deterministic without any extra sorting step.

## Unit unification is not syntactic

Ordinary Hindley-Milner unification matches type constructors structurally. Units cannot work
that way: `m/s` and `s^-1 m` are the same element, and solving `'u^2 ~ m^2` has to discover
`'u = m`. So unit equality is solved as a group equation. `unify_unit` reduces `a ~ b` to
`a / b ~ 1` and hands it to `solve_unit`, which runs Knuth/Kennedy variable elimination:

```rust
// src/types/mod.rs
fn unify_unit(&mut self, a: &Unit, b: &Unit) -> bool {
    let eq = self.apply_unit(a).div(&self.apply_unit(b));
    self.solve_unit(eq)
}
```

`solve_unit` picks the unit variable with the smallest absolute exponent as the pivot. If that
exponent divides every other exponent, the variable is solved outright (`v = product of the
rest`). Otherwise it substitutes a fresh variable to shrink the exponents and recurses. When
only base measures remain with non-zero exponents there is no variable to pivot on, and the
equation is a genuine dimension mismatch, which is exactly the `<m>` + `<s>` case below.

## Derived-measure aliases expand to base units

`measure N = kg m / s^2` is stored expanded. `resolve_unit_against` looks each name up in
`decls.measure_aliases` and substitutes its base-unit body, so `<N>` and `<kg m / s^2>` become
the same `Unit` value and unify freely. The tradeoff (DESIGN §8.2) is that inferred types
display the expanded form, since there is no back-mapping from base units to an alias name.

## Unit-aware sqrt halves exponents for free

`sqrt` has the prelude scheme `float<'u^2> -> float<'u>`, seeded with `Unit::var(...).pow(2)`.
Nothing special-cases roots in the unifier: applying `sqrt` to `float<m^2>` asks it to solve
`'u^2 ~ m^2`, and the elimination step halves the even exponent to give `'u = m`. An odd
exponent has no integer solution, so `sqrt` of a non-square unit is rejected with a dimension
mismatch. In the running example:

```pyfun
let side = sqrt 16.0<m^2>
```

infers `float<m>`, and `let norm x = sqrt (x * x)` stays unit-polymorphic `float<'u> ->
float<'u>`. `cbrt` is the identical mechanism with `pow(3)`.

## Erasure: a mismatch is caught, a clean program keeps no unit

The mismatch is a real compile error. `pyfun check` on `3.0<m> + 4.0<s>`:

```text
error: type mismatch: expected float<m>, found float<s>
 --> 4:11
  |
4 | let bad = 3.0<m> + 4.0<s>
  |           ^^^^^^^^^^^^^^^

1 error
```

Once a program checks, units vanish. `pyfun compile` on the running example lowers the
`sqrt 16.0<m^2>` line to plain arithmetic, with no unit anywhere in the output:

```python
side = math.sqrt(16.0)
```

The `<m^2>` annotation, the `<m>` result type, and every other dimension are gone. Nothing in
the emitted module can tell you the program ever had units, which is the whole point: maximum
static safety, zero runtime residue.

## Where you would add a built-in measure or root

To seed a new prelude unit operation (say a fourth root), you add its scheme in `seed_prelude`
in [src/types/mod.rs](https://github.com/simontreanor/Pyfun/blob/main/src/types/mod.rs) using
`Unit::var(PRELUDE_UVAR).pow(n)`, and its name/arity in `types::PRELUDE` plus a lowering route
in `lower_var` (`src/lowering/mod.rs`). The unifier needs no change: exponent division already
handles the new power. DESIGN §8.2 explains why the built-in root family deliberately stops at
`sqrt` and `cbrt`.
