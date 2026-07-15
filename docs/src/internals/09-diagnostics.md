# 09 - Diagnostics

Pyfun's pitch is that the compiler is the gatekeeper, so the compiler's voice matters.
[DESIGN.md §3](https://github.com/simontreanor/Pyfun/blob/main/DESIGN.md) puts it plainly: syntax
is cheap, error quality is not. A language that rejects a program owes the reader a clear reason,
a precise location, and where possible a way forward. That is why diagnostics get their own
attention even though the rendering code is small.

## The renderer is deliberately small

[src/diagnostics/mod.rs](https://github.com/simontreanor/Pyfun/blob/main/src/diagnostics/mod.rs)
is under a hundred lines. It renders one diagnostic in a rustc-like style: a level, a message,
and a single underlined span.

```rust
// src/diagnostics/mod.rs
pub fn render(source: &str, level: Level, message: &str, span: Span) -> String {
    let (line_no, col_no, line_start) = locate(source, span.start);
    // ... builds the gutter, the source line, and a `^^^` underline
}
```

`Level` is `Error`, `Warning`, or `Note`. `locate` maps a byte offset to a one-based line and
column, and the underline length is clamped to the rest of the line. That is the whole rendering
surface. The intelligence lives upstream: the type checker
([src/types/mod.rs](https://github.com/simontreanor/Pyfun/blob/main/src/types/mod.rs)) composes
the message string, including the "did you mean" member hints and the typed-hole reports, and
hands `render` a level, that message, and a span. Keeping presentation this thin means every
diagnostic across the compiler shares one format for free.

## Three real diagnostics

A type mismatch. HM inference reports the expected and found types at the offending call:

```text
error: type mismatch: expected int<'a>, found string
 --> 2:13
  |
2 | let wrong = add "hello" 5
  |             ^^^^^^^^^^^
```

A non-exhaustive match. The Maranget usefulness check produces a concrete witness for the case
the program forgot, so the message names it rather than saying only "not exhaustive":

```text
error: non-exhaustive match: `Rect _ _` is not matched
 --> 4:3
  |
4 |   match s:
  |   ^^^^^^^^
```

A typed hole report. This one is a `note`, not an error in the ordinary sense: it tells you the
type the blank must have and offers ways to fill it.

```text
note: hole `?body` has type `float` — try: h, w — or: area ?, cbrt ?, sqrt ?, abs ?
 --> 6:24
  |
6 |     case Rect w h: w * ?body
  |                        ^^^^^
1 unfilled hole
```

## Typed holes and hole fits

A `?` or `?name` is a typed blank. It infers as a fresh type variable that unifies freely, so it
never causes a spurious error and takes whatever type the context demands; here the context of
`w * ?body` forces `float`. The report is assembled by `Hole::message`:

```rust
// src/types/mod.rs
pub fn message(&self) -> String {
    let mut parts = vec![/* hole `?name` has type `T` */];
    if !self.fits.is_empty() { parts.push(format!("try: {}", self.fits.join(", "))); }
    if !self.refinements.is_empty() { parts.push(format!("or: {}", self.refinements.join(", "))); }
    parts.join(" — ")
}
```

The `try:` list is the valid hole fits: every in-scope binding whose type unifies with the
hole's. The test is a real trial unification that snapshots the substitution maps, instantiates
each candidate scheme, unifies it against the resolved hole type, and rolls back
(`Infer::hole_fits`, described in
[INTERNALS.md](https://github.com/simontreanor/Pyfun/blob/main/INTERNALS.md)'s typed-holes
section). Fits are ranked most-specific first and capped. The `or:` list is refinement fits: a
function whose result, after applying one or two further holes, unifies with the target, so
`area ?` and `sqrt ?` appear because each returns a `float`. A hole blocks `compile` and `run`
and makes `check` exit non-zero, so it is informative during development without ever slipping
into emitted Python.

## Why this framing

The mismatch names both types, the non-exhaustive report hands you the exact missing shape, and
the hole tells you what would type-check right there. Each one answers the reader's next question
instead of leaving them to guess. That is the concrete meaning of "error quality is not cheap":
the checker does extra work (witnesses, trial unification) specifically so the message can be
specific.

## Where you would add a new diagnostic note

A new hint or note is composed where the checker detects the condition, in
[src/types/mod.rs](https://github.com/simontreanor/Pyfun/blob/main/src/types/mod.rs) (the
`closest_member` "did you mean" path is a good model), and passed through `render` with the right
`Level`. If a diagnostic needs a shape the renderer cannot express yet, such as a second span or
a diagnostic code, that extension goes into
[src/diagnostics/mod.rs](https://github.com/simontreanor/Pyfun/blob/main/src/diagnostics/mod.rs);
the module's own header flags multi-span notes and codes as the natural next step.
