//! Self tail calls as loops — implements `DESIGN.md` §5.4.
//!
//! CPython has no tail-call elimination and caps the stack at ~1000 frames, so a
//! function that drives an unbounded loop by calling itself in tail position walks
//! a stack it has no reason to build. This pass rewrites that one shape: a direct,
//! saturated call to the enclosing function in tail position becomes *rebind the
//! parameters and go round again*, inside a `while True:`.
//!
//! ```text
//! def turn(state, deck):          def turn(state, deck):
//!     if over(state):                 while True:
//!         return state                    if over(state):
//!     return turn(step(state), rest)          return state
//!                                         state, deck = step(state), rest
//!                                         continue
//! ```
//!
//! It runs on the **lowered** body rather than the Pyfun AST, which is what makes
//! it small: `lower_return` emits a `PyStmt::Return` only in tail position, so
//! "tail call" is just "a `Return` whose value is a call to us", and the tail-position
//! walk is a walk over statements that already exist. General and mutual TCO stay
//! out of scope (`ROADMAP.md`): they cost the readable output lowering exists to
//! protect.
//!
//! The pass is conservative by construction — anything it cannot prove safe is
//! left as the ordinary recursive def, which is always correct, just stack-bound.

use std::collections::HashSet;

use crate::python_emitter::{PyCase, PyExpr, PyFStrPart, PyStmt};

/// What [`rewrite`] did: the body to emit, and — when the function *does* call
/// itself in tail position but a precondition refused the rewrite — why it kept
/// its recursive form. A rejection is otherwise invisible, and a program that
/// silently keeps recursing is exactly the one whose author needs to know.
pub(super) struct Outcome {
    pub body: Vec<PyStmt>,
    pub note: Option<String>,
}

impl Outcome {
    fn kept(body: Vec<PyStmt>) -> Self {
        Outcome { body, note: None }
    }
}

/// Rewrite `body` so that self tail calls loop instead of recursing, or return it
/// unchanged when any precondition fails. `name` is the function's *emitted* name
/// and `params` its emitted parameter names.
pub(super) fn rewrite(name: &str, params: &[String], body: Vec<PyStmt>) -> Outcome {
    if params.is_empty() {
        return Outcome::kept(body);
    }
    // `global`/`nonlocal` must stay at the top of the def, so they sit outside the
    // loop; everything after them is what goes round.
    let split = body
        .iter()
        .position(|s| !matches!(s, PyStmt::Global(_) | PyStmt::Nonlocal(_)))
        .unwrap_or(body.len());
    let (decls, rest) = body.split_at(split);
    let (decls, mut rest) = (decls.to_vec(), rest.to_vec());

    // Nothing to say (and nothing to do) about a function that never calls itself
    // in tail position, which is almost all of them.
    if !has_self_tail_call(&rest, name, params) {
        return Outcome::kept(join(decls, rest));
    }
    let rejected = |reason: String, decls: Vec<PyStmt>, rest: Vec<PyStmt>| Outcome {
        body: join(decls, rest),
        note: Some(format!(
            "`{name}` calls itself in tail position but keeps its recursive form: {reason}"
        )),
    };

    // P1: the name must still mean *this* function everywhere we would rewrite. A
    // body that rebinds it (a shadowing `let`, a nested def of the same name) would
    // have us loop where it meant to call something else.
    if binds(&rest, name) {
        return rejected(format!("the body rebinds `{name}`"), decls, rest);
    }
    // P2: a generator or coroutine body is not a plain call/return discipline —
    // `return` in a generator raises StopIteration with a value, and an async tail
    // call is an `await`. Neither is what this rewrite assumes.
    if has_yield(&rest) {
        return rejected("it is a generator".to_string(), decls, rest);
    }
    // P3: the loop reuses **one** cell per parameter and per local, where recursion
    // gave each frame its own. That is unobservable unless a closure outlives the
    // iteration that made it, so reject when a nested function has one of this
    // frame's names *free* in it. (`fun k -> x` captured into the accumulator is the
    // shape that would otherwise silently share the final `x` across every closure.)
    let frame = frame_names(&rest, params);
    let mut captured = HashSet::new();
    for stmt in &rest {
        nested_refs_stmt(stmt, &mut captured);
    }
    if let Some(shared) = frame.iter().find(|n| captured.contains(*n)) {
        return rejected(format!("a closure in it captures `{shared}`"), decls, rest);
    }

    let mut found = false;
    rewrite_stmts(&mut rest, name, params, &mut found);
    Outcome::kept(join(decls, vec![PyStmt::WhileTrue { body: rest }]))
}

/// Whether any tail position holds a saturated call to this function.
fn has_self_tail_call(stmts: &[PyStmt], name: &str, params: &[String]) -> bool {
    stmts.iter().any(|s| match s {
        PyStmt::Return(value) => self_call_args(value, name, params).is_some(),
        PyStmt::If { body, orelse, .. } => {
            has_self_tail_call(body, name, params) || has_self_tail_call(orelse, name, params)
        }
        PyStmt::Match { cases, .. } => cases
            .iter()
            .any(|c| has_self_tail_call(&c.body, name, params)),
        _ => false,
    })
}

fn join(mut decls: Vec<PyStmt>, mut rest: Vec<PyStmt>) -> Vec<PyStmt> {
    decls.append(&mut rest);
    decls
}

/// Replace each self tail call with parameter rebinding plus `continue`.
///
/// Descends only where a `continue` would bind to the loop this pass adds and
/// where the surrounding semantics are unchanged: `if` branches and `match` case
/// bodies. It does **not** descend into a `for` (a `continue` there belongs to the
/// `for`), a nested `def`/`class` (a different frame), or a `try` (looping inside
/// it would put every later iteration under a handler that only covered one call).
fn rewrite_stmts(stmts: &mut Vec<PyStmt>, name: &str, params: &[String], found: &mut bool) {
    let mut out = Vec::with_capacity(stmts.len());
    for mut stmt in stmts.drain(..) {
        match &mut stmt {
            PyStmt::Return(value) => {
                let rebound = self_call_args(value, name, params).map(|args| rebind(params, args));
                if let Some(mut stmts) = rebound {
                    out.append(&mut stmts);
                    *found = true;
                    continue;
                }
            }
            PyStmt::If { body, orelse, .. } => {
                rewrite_stmts(body, name, params, found);
                rewrite_stmts(orelse, name, params, found);
            }
            PyStmt::Match { cases, .. } => {
                for PyCase { body, .. } in cases.iter_mut() {
                    rewrite_stmts(body, name, params, found);
                }
            }
            _ => {}
        }
        out.push(stmt);
    }
    *stmts = out;
}

/// The argument list of `value`, when it is a saturated call to `name`.
fn self_call_args<'a>(value: &'a PyExpr, name: &str, params: &[String]) -> Option<&'a [PyExpr]> {
    let PyExpr::Call { func, args } = value else {
        return None;
    };
    match func.as_ref() {
        PyExpr::Name(n) if n == name && args.len() == params.len() => Some(args),
        _ => None,
    }
}

/// `a, b = <arg0>, <arg1>` then `continue` — one simultaneous rebinding, so an
/// argument still reads the *previous* iteration's parameters, exactly as the call
/// it replaces evaluated its arguments before entering the function.
fn rebind(params: &[String], args: &[PyExpr]) -> Vec<PyStmt> {
    let assign = if params.len() == 1 {
        PyStmt::Assign {
            target: params[0].clone(),
            value: args[0].clone(),
        }
    } else {
        PyStmt::UnpackAssign {
            targets: params.to_vec(),
            value: PyExpr::Tuple(args.to_vec()),
        }
    };
    vec![assign, PyStmt::Continue]
}

/// Whether these statements rebind `name` in this frame.
fn binds(stmts: &[PyStmt], name: &str) -> bool {
    stmts.iter().any(|s| match s {
        PyStmt::Assign { target, .. } => target == name,
        PyStmt::UnpackAssign { targets, .. } => targets.iter().any(|t| t == name),
        PyStmt::For { target, body, .. } => target == name || binds(body, name),
        PyStmt::FuncDef { name: n, .. } => n == name,
        PyStmt::ClassDef { name: n, .. } => n == name,
        PyStmt::If { body, orelse, .. } => binds(body, name) || binds(orelse, name),
        PyStmt::Match { cases, .. } => cases
            .iter()
            .any(|c| pattern_binds(&c.pattern, name) || binds(&c.body, name)),
        PyStmt::Try { body, handler, .. } => binds(body, name) || binds(handler, name),
        PyStmt::WhileTrue { body } => binds(body, name),
        _ => false,
    })
}

fn pattern_binds(pattern: &crate::python_emitter::PyPattern, name: &str) -> bool {
    use crate::python_emitter::PyPattern as P;
    match pattern {
        P::Capture(n) => n == name,
        P::As { pattern, name: n } => n == name || pattern_binds(pattern, name),
        P::Class { args, .. } => args.iter().any(|a| pattern_binds(a, name)),
        P::ClassKw { fields, .. } => fields.iter().any(|(_, p)| pattern_binds(p, name)),
        P::Sequence(ps) | P::Or(ps) => ps.iter().any(|p| pattern_binds(p, name)),
        P::ListSeq {
            elems,
            star,
            suffix,
        } => {
            star.as_deref() == Some(name)
                || elems.iter().chain(suffix).any(|p| pattern_binds(p, name))
        }
        P::Wildcard | P::Literal(_) => false,
    }
}

/// Whether these statements yield — i.e. the enclosing def is a generator.
fn has_yield(stmts: &[PyStmt]) -> bool {
    stmts.iter().any(|s| match s {
        PyStmt::Yield(_) | PyStmt::YieldFrom(_) => true,
        PyStmt::If { body, orelse, .. } => has_yield(body) || has_yield(orelse),
        PyStmt::Match { cases, .. } => cases.iter().any(|c| has_yield(&c.body)),
        PyStmt::For { body, .. } | PyStmt::WhileTrue { body } => has_yield(body),
        PyStmt::Try { body, handler, .. } => has_yield(body) || has_yield(handler),
        _ => false,
    })
}

/// Every name this frame binds: the parameters plus anything assigned here.
/// Nested `def`s are a different frame and are not descended into.
fn frame_names(stmts: &[PyStmt], params: &[String]) -> HashSet<String> {
    let mut out: HashSet<String> = params.iter().cloned().collect();
    collect_bound(stmts, &mut out);
    out
}

fn collect_bound(stmts: &[PyStmt], out: &mut HashSet<String>) {
    for stmt in stmts {
        match stmt {
            PyStmt::Assign { target, .. } => {
                out.insert(target.clone());
            }
            PyStmt::UnpackAssign { targets, .. } => out.extend(targets.iter().cloned()),
            PyStmt::For { target, body, .. } => {
                out.insert(target.clone());
                collect_bound(body, out);
            }
            PyStmt::If { body, orelse, .. } => {
                collect_bound(body, out);
                collect_bound(orelse, out);
            }
            PyStmt::Match { cases, .. } => {
                for c in cases {
                    collect_bound(&c.body, out);
                }
            }
            PyStmt::Try { body, handler, .. } => {
                collect_bound(body, out);
                collect_bound(handler, out);
            }
            PyStmt::WhileTrue { body } => collect_bound(body, out),
            _ => {}
        }
    }
}

/// Names a nested function has **free** — the ones a closure could still be
/// reading after this iteration ends. A name the nested function binds itself (its
/// parameters, its own locals) is not a capture, however it is spelled: a
/// `fun n -> n + 1` inside a function whose parameter is also `n` shares nothing
/// with it.
fn nested_refs_stmt(stmt: &PyStmt, out: &mut HashSet<String>) {
    match stmt {
        PyStmt::FuncDef { params, body, .. } => free_in_function(params, body, out),
        PyStmt::Assign { value, .. }
        | PyStmt::UnpackAssign { value, .. }
        | PyStmt::Return(value)
        | PyStmt::Expr(value)
        | PyStmt::Yield(value)
        | PyStmt::YieldFrom(value)
        | PyStmt::Raise(value) => nested_refs_expr(value, out),
        PyStmt::SubscriptAssign { obj, index, value } => {
            nested_refs_expr(obj, out);
            nested_refs_expr(index, out);
            nested_refs_expr(value, out);
        }
        PyStmt::For { iter, body, .. } => {
            nested_refs_expr(iter, out);
            for s in body {
                nested_refs_stmt(s, out);
            }
        }
        PyStmt::If { test, body, orelse } => {
            nested_refs_expr(test, out);
            for s in body.iter().chain(orelse) {
                nested_refs_stmt(s, out);
            }
        }
        PyStmt::Match { subject, cases } => {
            nested_refs_expr(subject, out);
            for c in cases {
                if let Some(g) = &c.guard {
                    nested_refs_expr(g, out);
                }
                for s in &c.body {
                    nested_refs_stmt(s, out);
                }
            }
        }
        PyStmt::Try { body, handler, .. } => {
            for s in body.iter().chain(handler) {
                nested_refs_stmt(s, out);
            }
        }
        PyStmt::WhileTrue { body } => {
            for s in body {
                nested_refs_stmt(s, out);
            }
        }
        _ => {}
    }
}

fn nested_refs_expr(expr: &PyExpr, out: &mut HashSet<String>) {
    match expr {
        PyExpr::Lambda { params, body } => {
            let bound: HashSet<String> = params.iter().cloned().collect();
            free_expr(body, &bound, out);
        }
        _ => walk_children(expr, &mut |e| nested_refs_expr(e, out)),
    }
}

/// The free names of a nested `def`: everything it mentions, less what it binds
/// (parameters and its own assignments), plus anything it declares `nonlocal` or
/// `global` — those name the *enclosing* cell by definition, so they are captures
/// even though the body also assigns them.
fn free_in_function(params: &[String], body: &[PyStmt], out: &mut HashSet<String>) {
    let mut bound: HashSet<String> = params.iter().cloned().collect();
    collect_bound(body, &mut bound);
    let mut declared = HashSet::new();
    for stmt in body {
        if let PyStmt::Nonlocal(names) | PyStmt::Global(names) = stmt {
            declared.extend(names.iter().cloned());
        }
    }
    for name in &declared {
        bound.remove(name);
    }
    free_stmts(body, &bound, out);
    out.extend(declared);
}

fn free_stmts(stmts: &[PyStmt], bound: &HashSet<String>, out: &mut HashSet<String>) {
    for stmt in stmts {
        match stmt {
            PyStmt::FuncDef { params, body, .. } => {
                let mut inner = bound.clone();
                inner.extend(params.iter().cloned());
                collect_bound(body, &mut inner);
                free_stmts(body, &inner, out);
            }
            PyStmt::Assign { value, .. }
            | PyStmt::UnpackAssign { value, .. }
            | PyStmt::Return(value)
            | PyStmt::Expr(value)
            | PyStmt::Yield(value)
            | PyStmt::YieldFrom(value)
            | PyStmt::Raise(value) => free_expr(value, bound, out),
            PyStmt::SubscriptAssign { obj, index, value } => {
                free_expr(obj, bound, out);
                free_expr(index, bound, out);
                free_expr(value, bound, out);
            }
            PyStmt::For { iter, body, .. } => {
                free_expr(iter, bound, out);
                free_stmts(body, bound, out);
            }
            PyStmt::If { test, body, orelse } => {
                free_expr(test, bound, out);
                free_stmts(body, bound, out);
                free_stmts(orelse, bound, out);
            }
            PyStmt::Match { subject, cases } => {
                free_expr(subject, bound, out);
                for c in cases {
                    // A case pattern binds its captures for that arm's body.
                    let mut inner = bound.clone();
                    collect_pattern_names(&c.pattern, &mut inner);
                    if let Some(g) = &c.guard {
                        free_expr(g, &inner, out);
                    }
                    free_stmts(&c.body, &inner, out);
                }
            }
            PyStmt::Try {
                body,
                binding,
                handler,
                ..
            } => {
                free_stmts(body, bound, out);
                let mut inner = bound.clone();
                if let Some(b) = binding {
                    inner.insert(b.clone());
                }
                free_stmts(handler, &inner, out);
            }
            PyStmt::WhileTrue { body } => free_stmts(body, bound, out),
            _ => {}
        }
    }
}

fn free_expr(expr: &PyExpr, bound: &HashSet<String>, out: &mut HashSet<String>) {
    match expr {
        PyExpr::Name(n) => {
            if !bound.contains(n) {
                out.insert(n.clone());
            }
        }
        PyExpr::Lambda { params, body } => {
            let mut inner = bound.clone();
            inner.extend(params.iter().cloned());
            free_expr(body, &inner, out);
        }
        _ => walk_children(expr, &mut |e| free_expr(e, bound, out)),
    }
}

/// The names a lowered case pattern captures.
fn collect_pattern_names(pattern: &crate::python_emitter::PyPattern, out: &mut HashSet<String>) {
    use crate::python_emitter::PyPattern as P;
    match pattern {
        P::Capture(n) => {
            out.insert(n.clone());
        }
        P::As { pattern, name } => {
            out.insert(name.clone());
            collect_pattern_names(pattern, out);
        }
        P::Class { args, .. } => {
            for a in args {
                collect_pattern_names(a, out);
            }
        }
        P::ClassKw { fields, .. } => {
            for (_, p) in fields {
                collect_pattern_names(p, out);
            }
        }
        P::Sequence(ps) | P::Or(ps) => {
            for p in ps {
                collect_pattern_names(p, out);
            }
        }
        P::ListSeq {
            elems,
            star,
            suffix,
        } => {
            if let Some(s) = star {
                out.insert(s.clone());
            }
            for p in elems.iter().chain(suffix) {
                collect_pattern_names(p, out);
            }
        }
        P::Wildcard | P::Literal(_) => {}
    }
}

/// Apply `f` to each immediate sub-expression.
fn walk_children(expr: &PyExpr, f: &mut impl FnMut(&PyExpr)) {
    match expr {
        PyExpr::BinOp { left, right, .. } => {
            f(left);
            f(right);
        }
        PyExpr::Compare {
            left, comparators, ..
        } => {
            f(left);
            for c in comparators {
                f(c);
            }
        }
        PyExpr::Call { func, args } => {
            f(func);
            for a in args {
                f(a);
            }
        }
        PyExpr::CallKw { func, args, kwargs } => {
            f(func);
            for a in args {
                f(a);
            }
            for (_, v) in kwargs {
                f(v);
            }
        }
        PyExpr::IfExp { body, test, orelse } => {
            f(body);
            f(test);
            f(orelse);
        }
        PyExpr::Lambda { body, .. } => f(body),
        PyExpr::Attribute { value, .. } => f(value),
        PyExpr::Subscript { value, index } => {
            f(value);
            f(index);
        }
        PyExpr::Slice {
            value,
            lower,
            upper,
        } => {
            f(value);
            f(lower);
            f(upper);
        }
        PyExpr::Await(e) | PyExpr::Not(e) | PyExpr::Neg(e) => f(e),
        PyExpr::List(es) | PyExpr::Tuple(es) => {
            for e in es {
                f(e);
            }
        }
        PyExpr::FStr(parts) => {
            for p in parts {
                if let PyFStrPart::Expr(e) = p {
                    f(e);
                }
            }
        }
        PyExpr::Int(_)
        | PyExpr::Float(_)
        | PyExpr::Str(_)
        | PyExpr::Bool(_)
        | PyExpr::Name(_)
        | PyExpr::NoneLit => {}
    }
}
