# 03 - Desugaring

Desugaring rewrites a few surface conveniences into the core forms the type checker and lowerer
already understand, so those later stages need no special cases for the sugar. In Pyfun the pass
lives in [`src/desugar.rs`](https://github.com/simontreanor/Pyfun/blob/main/src/desugar.rs) and
covers three things: user-defined computation-expression builders, operator sections, and function
composition. The design rule is stated in
[`DESIGN.md`](https://github.com/simontreanor/Pyfun/blob/main/DESIGN.md) §8.1: rewrite to ordinary
calls and lambdas, then let normal inference and lowering take over.

## Operator sections and composition

The smallest cases are the clearest. An operator wrapped in parentheses, like `(+)`, is a
first-class curried function. Rather than teaching the checker and emitter about operator values,
`op_func` rewrites it to the lambda you would have written by hand:

```rust
// src/desugar.rs
pub fn op_func(op: BinOp, span: Span) -> Expr {
    let body = mk(
        ExprKind::Binary { op, lhs: Box::new(var("a", span)), rhs: Box::new(var("b", span)) },
        span,
    );
    mk(ExprKind::Fn { params: vec![/* a, b */], body: Box::new(body) }, span)
}
```

So `(+)` becomes `fun a b -> a + b`, and its `num` constraint, currying, and partial application
all fall out of the ordinary rules with no bespoke code. Composition is the same tactic: `f >> g`
desugars to `fun x -> g (f x)` and `f << g` to `fun x -> f (g x)`. Composition has one extra care
that a section does not, because its body embeds the operands, which may reference outer variables,
so `compose` picks a parameter name (`_pf_x`, else `_pf_x0`, ...) that is free of both operands'
free variables, ruling out capture. The pretty-printer keeps the faithful `(op)` / `(f >> g)`
spelling regardless, so the roundtrip tests still see the source form.

The running example uses an explicit lambda in its fold, `shapes |> List.fold (fun acc s -> acc +
area s) 0.0`, so it does not exercise a section. But the point-free cousin `List.fold (+) 0.0` would
desugar to exactly the lambda the example spells out, which is why both styles type-check and lower
identically.

## Computation-expression builders

Computation expressions are the F# flagship for monadic sugar, written `builder { ... }`. Pyfun
splits them in two. The three built-ins keep **bespoke native lowerings** and are not desugared
here, because a generic bind/return rewrite cannot produce idiomatic output for them. User-defined
builders, on the other hand, are handled entirely by this pass. A builder is an in-file `module`
providing protocol functions, and `Builder { ... }` desugars to calls on them. The protocol, from
the module documentation:

```rust
// src/desugar.rs
//! | item            | desugaring                                           |
//! |-----------------|------------------------------------------------------|
//! | `let! x = e` …  | `B.bind e (fun x -> …)`                              |
//! | `do! e` …       | `B.bind e (fun _ -> …)`   (trailing `do! e` → `e`)   |
//! | `let x = e` …   | `(fun x -> …) e`                                     |
//! | `return e`      | `B.return_ e`        (must be last)                  |
//! | `return! e`     | `B.returnFrom e`     (must be last)                  |
//! | `yield e` …     | `B.combine (B.yield_ e) (B.delay (fun _ -> …))`      |
```

The driver, `desugar_ce`, is a recursion over the CE items. Each item is rewritten into a call on
the builder module, threading the rest of the block through as a continuation lambda:

```rust
// src/desugar.rs
CeItem::LetBang { name, name_span, value } => {
    let cont = require_rest(builder, rest, span, "`let!`", value)?;
    Ok(call2(builder, "bind", value.clone(),
             lam(name.clone(), *name_span, cont, span), span))
}
```

Because the result is ordinary calls, the desugaring is **type-directed for free**: the builder's
own `bind` and `return_` signatures determine the types through normal inference on the rewritten
calls. There are no per-builder type rules and no per-builder codegen. Note too that the
`let!`-bound name keeps its original span on the generated lambda parameter, so hover and rename
still work on a name that only exists after desugaring.

## The built-in `result {}`, natively

The three built-ins lower directly rather than through this pass, so it is worth seeing what
"native" buys. Here is a small `result {}` block compiled with `pyfun compile`:

```python
def safeDiv(a, b):
    def _pf_fn0():
        match Error("div by zero") if b == 0 else Ok(a):
            case Ok(x):
                return Ok(x / b)
            case Error(_pf_t0):
                return Error(_pf_t0)
    return _pf_fn0()
```

That is railway-oriented short-circuiting written as a real `match`: on `Ok` it continues, on
`Error` it returns the error unchanged. A generic bind/return desugaring would produce a chain of
closure calls instead, which is why the built-ins keep their own lowering while user builders take
the desugaring path. The two mechanisms sit side by side: the same protocol shape, but the built-ins
earn idiomatic Python by not being desugared.

## Where you would add desugared sugar

A new value-level sugar that can be expressed with existing core forms belongs here. Add a
constructor function to
[`src/desugar.rs`](https://github.com/simontreanor/Pyfun/blob/main/src/desugar.rs) that builds the
equivalent `Fn`/`App`/`Binary` tree (following `op_func` and `compose`), call it from the parser
where the surface form is recognized, and keep the pretty-printer rendering the surface spelling so
the roundtrip holds. If the sugar needs its own lowering to stay idiomatic, as the built-in CEs do,
it instead belongs downstream in [lowering](README.md), not in this pass.
