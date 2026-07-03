//! Computation-expression desugaring (`DESIGN.md` §8.1) for **user-defined**
//! builders. A `Builder { … }` where `Builder` is an in-file `module` desugars
//! into ordinary calls on that module's protocol functions, after which the
//! normal HM inference and lowering handle it — no per-builder type rules or
//! codegen. (The three built-ins `async`/`seq`/`result` keep their bespoke native
//! lowering instead; they are not desugared here.)
//!
//! The protocol mirrors F#'s, lowercased and keyword-safe:
//!
//! | item            | desugaring                                           |
//! |-----------------|------------------------------------------------------|
//! | `let! x = e` …  | `B.bind e (fun x -> …)`                              |
//! | `do! e` …       | `B.bind e (fun _ -> …)`   (trailing `do! e` → `e`)   |
//! | `let x = e` …   | `(fun x -> …) e`                                     |
//! | `return e`      | `B.return_ e`        (must be last)                  |
//! | `return! e`     | `B.returnFrom e`     (must be last)                  |
//! | `yield e` …     | `B.combine (B.yield_ e) (B.delay (fun _ -> …))`      |
//! | `yield! e` …    | `B.combine (B.yieldFrom e) (B.delay (fun _ -> …))`   |
//! | (empty)         | `B.zero`                                             |
//!
//! A builder need only define the functions its bodies actually use; a missing
//! one surfaces as the ordinary "not a member of `B`" error. `delay` receives a
//! thunk `'t -> m a` (force it by applying to any value).

use crate::lexer::Span;
use crate::parser::ast::{BinOp, CeItem, Expr, ExprKind, NodeSpan, Param};

/// Desugar a user-builder CE body into a plain expression. Returns
/// `Err((message, span))` for a structurally invalid body (e.g. a non-final
/// `return`).
pub fn desugar_ce(builder: &str, items: &[CeItem], span: Span) -> Result<Expr, (String, Span)> {
    let Some((head, rest)) = items.split_first() else {
        // An empty `Builder { }` is the builder's zero value.
        return Ok(member(builder, "zero", span));
    };
    let is_last = rest.is_empty();
    match head {
        CeItem::LetBang {
            name,
            name_span,
            value,
        } => {
            let cont = require_rest(builder, rest, span, "`let!`", value)?;
            Ok(call2(
                builder,
                "bind",
                value.clone(),
                lam(name.clone(), *name_span, cont, span),
                span,
            ))
        }
        CeItem::DoBang(e) => {
            if is_last {
                Ok(e.clone())
            } else {
                let cont = desugar_ce(builder, rest, span)?;
                Ok(call2(
                    builder,
                    "bind",
                    e.clone(),
                    wild_lam(cont, span),
                    span,
                ))
            }
        }
        CeItem::Let {
            name,
            name_span,
            value,
        } => {
            let cont = require_rest(builder, rest, span, "a `let`", value)?;
            // `let x = e` followed by the rest is an immediately-applied lambda.
            Ok(app(
                lam(name.clone(), *name_span, cont, span),
                value.clone(),
                span,
            ))
        }
        CeItem::Return(e) => {
            require_last(is_last, "`return`", e)?;
            Ok(call1(builder, "return_", e.clone(), span))
        }
        CeItem::ReturnBang(e) => {
            require_last(is_last, "`return!`", e)?;
            Ok(call1(builder, "returnFrom", e.clone(), span))
        }
        CeItem::Yield(e) => {
            let y = call1(builder, "yield_", e.clone(), span);
            combined_with_rest(builder, y, rest, span)
        }
        CeItem::YieldBang(e) => {
            let y = call1(builder, "yieldFrom", e.clone(), span);
            combined_with_rest(builder, y, rest, span)
        }
    }
}

/// Desugar the rest, requiring it to be non-empty (the item can't be last).
fn require_rest(
    builder: &str,
    rest: &[CeItem],
    span: Span,
    what: &str,
    value: &Expr,
) -> Result<Expr, (String, Span)> {
    if rest.is_empty() {
        return Err((
            format!("{what} must be followed by another item"),
            value.span(),
        ));
    }
    desugar_ce(builder, rest, span)
}

fn require_last(is_last: bool, what: &str, e: &Expr) -> Result<(), (String, Span)> {
    if is_last {
        Ok(())
    } else {
        Err((format!("{what} must be the final item"), e.span()))
    }
}

/// `yield`/`yield!`: alone it is the value; followed by more, it is combined with
/// a delayed continuation (`B.combine y (B.delay (fun _ -> rest))`).
fn combined_with_rest(
    builder: &str,
    head: Expr,
    rest: &[CeItem],
    span: Span,
) -> Result<Expr, (String, Span)> {
    if rest.is_empty() {
        return Ok(head);
    }
    let cont = desugar_ce(builder, rest, span)?;
    let delayed = call1(builder, "delay", wild_lam(cont, span), span);
    Ok(call2(builder, "combine", head, delayed, span))
}

// ----- node constructors -----

fn mk(kind: ExprKind, span: Span) -> Expr {
    Expr::new(kind, span)
}

/// `(op)` — a binary operator used as a value, desugared to the curried lambda
/// `fun a b -> a op b`. Reuses ordinary inference and lowering (which handle the
/// operator's constraints, currying, and partial application), like the CE
/// desugarings above; the pretty-printer keeps the faithful `(op)` spelling. The
/// two params are lambda-bound and the body references only them, so their fixed
/// names can't capture or clash with anything in the surrounding scope.
pub fn op_func(op: BinOp, span: Span) -> Expr {
    let body = mk(
        ExprKind::Binary {
            op,
            lhs: Box::new(var("a", span)),
            rhs: Box::new(var("b", span)),
        },
        span,
    );
    mk(
        ExprKind::Fn {
            params: vec![
                Param {
                    name: "a".to_string(),
                    span: NodeSpan::new(span),
                },
                Param {
                    name: "b".to_string(),
                    span: NodeSpan::new(span),
                },
            ],
            body: Box::new(body),
        },
        span,
    )
}

/// Function composition `lhs >> rhs` / `lhs << rhs`, desugared to a single-argument
/// lambda `fun x -> rhs (lhs x)` (left-to-right `>>`) resp. `fun x -> lhs (rhs x)`
/// (right-to-left `<<`). Ordinary inference and lowering then handle it (currying,
/// the operands' own constraints), like [`op_func`]; the pretty-printer keeps the
/// operator spelling.
///
/// **Capture:** unlike `op_func` (whose body uses only its own params), the body
/// here embeds the operands `lhs`/`rhs`, which may reference outer variables. So the
/// lambda parameter is chosen to be **free of both operands' free variables**
/// (`_pf_x`, else `_pf_x0`, `_pf_x1`, …) — no capture is possible.
pub fn compose(lhs: Expr, rhs: Expr, right_to_left: bool, span: Span) -> Expr {
    let mut free = std::collections::HashSet::new();
    let bound = std::collections::HashSet::new();
    crate::types::collect_free(&lhs, &bound, &mut free);
    crate::types::collect_free(&rhs, &bound, &mut free);
    let param = fresh_name("_pf_x", &free);

    // `>>` applies `lhs` first, then `rhs`; `<<` (math ∘) applies `rhs` first.
    let (first, second) = if right_to_left { (rhs, lhs) } else { (lhs, rhs) };
    let body = app(second, app(first, var(&param, span), span), span);
    mk(
        ExprKind::Fn {
            params: vec![Param {
                name: param,
                span: NodeSpan::new(span),
            }],
            body: Box::new(body),
        },
        span,
    )
}

/// A name based on `base` that is not in `taken`: `base`, else `base0`, `base1`, ….
fn fresh_name(base: &str, taken: &std::collections::HashSet<String>) -> String {
    if !taken.contains(base) {
        return base.to_string();
    }
    (0..).map(|i| format!("{base}{i}")).find(|n| !taken.contains(n)).unwrap()
}

fn var(name: &str, span: Span) -> Expr {
    mk(ExprKind::Var(name.to_string()), span)
}

/// `Module.method` — a field access the type checker / lowerer resolve as a module
/// member (the base is uppercase).
fn member(module: &str, method: &str, span: Span) -> Expr {
    mk(
        ExprKind::Field {
            base: Box::new(var(module, span)),
            name: method.to_string(),
        },
        span,
    )
}

fn app(func: Expr, arg: Expr, span: Span) -> Expr {
    mk(
        ExprKind::App {
            func: Box::new(func),
            arg: Box::new(arg),
        },
        span,
    )
}

fn call1(module: &str, method: &str, a: Expr, span: Span) -> Expr {
    app(member(module, method, span), a, span)
}

fn call2(module: &str, method: &str, a: Expr, b: Expr, span: Span) -> Expr {
    app(call1(module, method, a, span), b, span)
}

/// `fun name -> body`, the param carrying the binder's original span so hover and
/// rename still work on a `let!`-bound name.
fn lam(name: String, name_span: NodeSpan, body: Expr, span: Span) -> Expr {
    mk(
        ExprKind::Fn {
            params: vec![Param {
                name,
                span: name_span,
            }],
            body: Box::new(body),
        },
        span,
    )
}

/// `fun _ -> body` — an ignored-argument lambda (for `do!` and `delay` thunks).
fn wild_lam(body: Expr, span: Span) -> Expr {
    lam("_".to_string(), NodeSpan::new(span), body, span)
}
