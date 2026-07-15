# 07 - Lowering

Once a program has passed type, effect, exhaustiveness, and unit checking, lowering turns the
Pyfun AST into a Python-AST IR. The strategy is in
[DESIGN.md §5](https://github.com/simontreanor/Pyfun/blob/main/DESIGN.md); the code is
[src/lowering/mod.rs](https://github.com/simontreanor/Pyfun/blob/main/src/lowering/mod.rs). The
firm rule is that lowering never splices strings. It builds structured
[`PyStmt`/`PyExpr`](https://github.com/simontreanor/Pyfun/blob/main/src/python_emitter/mod.rs)
nodes, and a separate emitter (covered in [emission](08-emission.md)) renders them. Two things
make this more than a one-to-one copy, and both show up in the running example.

## Expression language to statement language

Pyfun is expression-oriented; Python is statement-oriented. Function bodies lower in return
position (`lower_return`), so an `if` or a `match` becomes a clean Python statement rather than
a nested ternary. Sub-expressions lower in value position (`lower_value`), hoisting any
statements they need before the value. The `area` function shows the return-position path: its
`match` becomes a Python `match` statement whose arms `return` directly.

## Curried in types, n-ary in output

Functions curry by default, but emitting `add(1)(2)` everywhere would be slow and unreadable.
Because arities are known statically, `build_call` collapses a fully-applied call to a direct
n-ary Python call and only synthesizes a closure for a genuine partial application:

```rust
// src/lowering/mod.rs
fn build_call(&mut self, head: PyExpr, arity: Option<usize>, args: Vec<PyExpr>) -> PyExpr {
    let n = args.len();
    match arity {
        Some(k) if n < k => {
            // Partial application.
            self.needs_functools = true;
            // ... functools.partial(head, args...)
        }
        Some(k) if n > k => { /* full call, then apply the remainder one at a time */ }
        // Exact arity, or unknown arity (treated as n-ary).
        _ => PyExpr::Call { func: Box::new(head), args },
    }
}
```

In the running example, `area s` inside the fold is a full application, so it collapses to the
direct call `area(s)`, and the fold's folder is a two-argument lambda applied fully. Arities
come from a syntactic table of top-level functions, constructors, and prelude members, built in
the lowerer's constructor. A genuine partial application (`add 1`) instead sets `needs_functools`
and emits `functools.partial(add, 1)`. This mirrors what F# does at the IL level: closures only
where partial application is real.

## The pipe is lowering-time sugar

`x |> f` has no runtime cost. `flatten_app` treats a pipe exactly like an application spine when
it flattens the head and arguments:

```rust
// src/lowering/mod.rs
// `x |> f` is treated as `f x`, so pipes flatten alongside ordinary calls.
```

So `x |> f |> g` lowers to `g(f(x))` with no intermediate helper, and a piped call reuses the
same currying collapse as a written-out call.

## Scope, name binding, and captured muts

Python makes an assigned name local to its function unless declared otherwise. A closure that
reassigns a `mut` captured from an enclosing scope would therefore miscompile into a fresh local.
`lower_fn_body` scans each body for names it reassigns but does not itself bind, then classifies
each: a name found in an enclosing function frame (`fn_local_stack`) becomes `nonlocal`, and a
module-level name becomes `global`.

```rust
// src/lowering/mod.rs
for name in &assigned {
    if bound.contains(name) { continue; }
    if self.fn_local_stack.iter().any(|f| f.contains(name)) {
        nonlocals.push(name.clone());
    } else {
        globals.push(name.clone());
    }
}
```

This is the same idea as F# 4.0 auto-boxing a captured mutable into a reference cell; Python's
`nonlocal`/`global` mechanism does the job directly.

## Unit erasure happens here

Units are checked in `types/` and then dropped. By the time lowering runs, a numeric value is
just a number: there are no `Unit` nodes in the Python IR to carry, so `sqrt 16.0<m^2>` lowers
with the annotation already gone (see [units](06-units.md)).

## The running example, lowered

`pyfun compile` shows both transformations at once:

```python
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
```

The lambda's `area s` collapsed to `area(s)`, `List.fold` routed to the emitted `_pf_fold`
helper (`functools.reduce`) with its three arguments direct and n-ary, and the unit is gone.

## Where you would add a stdlib function's lowering

A new collection or string helper is routed in `lower_var` (for its emitted `_pf_*` helper) or,
for a pure one-liner that should inline when fully applied, in `try_inline_stdlib` inside
`lower_application`, both in
[src/lowering/mod.rs](https://github.com/simontreanor/Pyfun/blob/main/src/lowering/mod.rs).
Register the member's arity in the lowerer's arity table so partial application still lowers to
`functools.partial`, and add any new IR shape to the Python emitter first.
