//! Arm-scoped captures (`DESIGN.md` §5, "Arm-scoped captures").
//!
//! A Pyfun match arm's capture (`case Some pair: …`) is scoped to the arm. The
//! Python it lowers to (`case Some(pair):`) is an assignment to a local of the
//! enclosing *function*, and Python locals are function-wide: one slot per name
//! per frame. So a capture that reuses a name the frame also uses for something
//! else (a block-local `def`, a parameter, a global read later on) rebinds that
//! slot for the rest of the function, and a later read sees the captured value
//! where Pyfun meant the original binding (issue #92).
//!
//! The fix is a rename, decided per arm from a count of the name's occurrences
//! in the enclosing Python frame: a capture is freshened to `_name` when the
//! name occurs anywhere in the frame outside its own arm, not counting sibling
//! arms of the same match that capture the same name (disjoint alternatives that
//! read their own capture). Occurrence means a `Var` reference or an `<-`
//! assignment target; binders are not occurrences. The walk covers the frame's
//! whole body, nested closures included (a closure's free reference resolves to
//! the frame's slot), but a name that a nested Python scope binds for itself is
//! hidden inside that scope: a lambda's own `x` is its own slot.

use std::collections::{HashMap, HashSet};

use crate::parser::ast::{BlockStmt, CeItem, Expr, ExprKind, InterpPart, Item, LetBinding};

/// One Python frame's occurrence census plus the fresh names already handed out
/// in it (so two matches in one frame that both freshen `x` get distinct names).
#[derive(Debug, Default, Clone)]
pub(super) struct Frame {
    pub occurrences: HashMap<String, usize>,
    pub fresh: HashSet<String>,
}

impl Frame {
    /// The frame for a function whose body is `body` (parameters are same-frame
    /// binders, so they hide nothing).
    pub fn of_body(body: &Expr) -> Frame {
        let mut occurrences = HashMap::new();
        collect_occurrences(body, &HashSet::new(), &mut occurrences);
        Frame {
            occurrences,
            fresh: HashSet::new(),
        }
    }

    /// The frame for a computation expression body (its own Python function).
    pub fn of_ce(items: &[CeItem]) -> Frame {
        let mut occurrences = HashMap::new();
        collect_ce_occurrences(items, &HashSet::new(), &mut occurrences);
        Frame {
            occurrences,
            fresh: HashSet::new(),
        }
    }

    /// The module frame: every top-level value evaluated at module scope. A
    /// parameterised binding (or an active-pattern recognizer) is its own def and
    /// hides what it binds; an in-file `module` flattens into the same scope.
    pub fn of_items(items: &[Item]) -> Frame {
        let mut occurrences = HashMap::new();
        collect_item_occurrences(items, &mut occurrences);
        Frame {
            occurrences,
            fresh: HashSet::new(),
        }
    }

    /// Add `other`'s occurrences on top of this frame's (an over-count is always
    /// safe: it can only freshen more).
    pub fn merged_with(&self, body: &Expr) -> Frame {
        let mut occurrences = self.occurrences.clone();
        collect_occurrences(body, &HashSet::new(), &mut occurrences);
        Frame {
            occurrences,
            fresh: self.fresh.clone(),
        }
    }

    /// Whether `name` (a Python-side spelling) is already in use in this frame.
    pub fn uses(&self, name: &str) -> bool {
        self.occurrences.contains_key(name) || self.fresh.contains(name)
    }
}

fn bump(out: &mut HashMap<String, usize>, name: &str) {
    *out.entry(name.to_string()).or_insert(0) += 1;
}

/// Everything a nested Python scope binds for itself, so it is hidden inside
/// that scope: its parameters and every frame-level binder of its body.
fn scope_bound(params: &[String], body: &Expr, hidden: &HashSet<String>) -> HashSet<String> {
    let mut inner = hidden.clone();
    inner.extend(params.iter().cloned());
    super::fold_loop::collect_frame_binders(body, &mut inner);
    inner
}

/// Count the occurrences of every name in `e` that belongs to the enclosing
/// frame, skipping names in `hidden` (bound by a nested scope for itself).
pub(super) fn collect_occurrences(
    e: &Expr,
    hidden: &HashSet<String>,
    out: &mut HashMap<String, usize>,
) {
    match &e.kind {
        ExprKind::Var(n) => {
            if !hidden.contains(n) {
                bump(out, n);
            }
        }
        ExprKind::Assign { target, value } => {
            if !hidden.contains(target) {
                bump(out, target);
            }
            collect_occurrences(value, hidden, out);
        }
        // A lambda is its own Python scope: what it binds is hidden inside it,
        // but its free references still resolve to this frame's slots.
        ExprKind::Fn { params, body } => {
            let names: Vec<String> = params
                .iter()
                .flat_map(|p| super::pattern_bindings(&p.pattern))
                .collect();
            let inner = scope_bound(&names, body, hidden);
            collect_occurrences(body, &inner, out);
        }
        ExprKind::Block { stmts } => {
            for s in stmts {
                match s {
                    BlockStmt::Let(b) => collect_let_occurrences(b, hidden, out),
                    BlockStmt::Expr(e) => collect_occurrences(e, hidden, out),
                }
            }
        }
        // A computation expression body is its own Python function.
        ExprKind::Ce { items, .. } => {
            let mut inner = hidden.clone();
            for it in items {
                match it {
                    CeItem::LetBang { target, value, .. } | CeItem::Let { target, value, .. } => {
                        inner.extend(target.bound_names());
                        super::fold_loop::collect_frame_binders(value, &mut inner);
                    }
                    CeItem::DoBang(e)
                    | CeItem::Return(e)
                    | CeItem::ReturnBang(e)
                    | CeItem::Yield(e)
                    | CeItem::YieldBang(e) => {
                        super::fold_loop::collect_frame_binders(e, &mut inner);
                    }
                }
            }
            collect_ce_occurrences(items, &inner, out);
        }
        ExprKind::Match { scrutinee, arms } => {
            collect_occurrences(scrutinee, hidden, out);
            for a in arms {
                if let Some(g) = &a.guard {
                    collect_occurrences(g, hidden, out);
                }
                collect_occurrences(&a.body, hidden, out);
            }
        }
        ExprKind::App { func, arg } => {
            collect_occurrences(func, hidden, out);
            collect_occurrences(arg, hidden, out);
        }
        ExprKind::Pipe { lhs, rhs, .. }
        | ExprKind::Compose { lhs, rhs, .. }
        | ExprKind::Binary { lhs, rhs, .. } => {
            collect_occurrences(lhs, hidden, out);
            collect_occurrences(rhs, hidden, out);
        }
        ExprKind::If { cond, then, else_ } => {
            collect_occurrences(cond, hidden, out);
            collect_occurrences(then, hidden, out);
            collect_occurrences(else_, hidden, out);
        }
        ExprKind::Unary { expr, .. }
        | ExprKind::Annot { value: expr, .. }
        | ExprKind::Try { body: expr } => collect_occurrences(expr, hidden, out),
        ExprKind::Compare { first, rest } => {
            collect_occurrences(first, hidden, out);
            for (_, o) in rest {
                collect_occurrences(o, hidden, out);
            }
        }
        ExprKind::List { elems } | ExprKind::Tuple { elems } => {
            for e in elems {
                collect_occurrences(e, hidden, out);
            }
        }
        ExprKind::Record { fields, .. } => {
            for f in fields {
                collect_occurrences(&f.value, hidden, out);
            }
        }
        ExprKind::RecordUpdate { base, fields } => {
            collect_occurrences(base, hidden, out);
            for f in fields {
                collect_occurrences(&f.value, hidden, out);
            }
        }
        ExprKind::Field { base, .. } => collect_occurrences(base, hidden, out),
        ExprKind::Interp { parts } => {
            for p in parts {
                if let InterpPart::Expr(e) = p {
                    collect_occurrences(e, hidden, out);
                }
            }
        }
        ExprKind::Int(_)
        | ExprKind::Float(_)
        | ExprKind::Str(_)
        | ExprKind::Bool(_)
        | ExprKind::Unit
        | ExprKind::Hole { .. }
        | ExprKind::OpFunc(_) => {}
    }
}

/// A `let` in the frame: a parameterised binding is a nested def (its own
/// scope); a value binding evaluates in this frame.
fn collect_let_occurrences(
    b: &LetBinding,
    hidden: &HashSet<String>,
    out: &mut HashMap<String, usize>,
) {
    if b.params.is_empty() {
        collect_occurrences(&b.value, hidden, out);
    } else {
        let names: Vec<String> = b
            .params
            .iter()
            .flat_map(|p| super::pattern_bindings(&p.pattern))
            .collect();
        let inner = scope_bound(&names, &b.value, hidden);
        collect_occurrences(&b.value, &inner, out);
    }
}

fn collect_ce_occurrences(
    items: &[CeItem],
    hidden: &HashSet<String>,
    out: &mut HashMap<String, usize>,
) {
    for it in items {
        match it {
            CeItem::LetBang { value, .. } | CeItem::Let { value, .. } => {
                collect_occurrences(value, hidden, out)
            }
            CeItem::DoBang(e)
            | CeItem::Return(e)
            | CeItem::ReturnBang(e)
            | CeItem::Yield(e)
            | CeItem::YieldBang(e) => collect_occurrences(e, hidden, out),
        }
    }
}

fn collect_item_occurrences(items: &[Item], out: &mut HashMap<String, usize>) {
    let none = HashSet::new();
    for item in items {
        match item {
            Item::Let(b) => collect_let_occurrences(b, &none, out),
            // A top-level expression statement (`print m`) evaluates at module scope.
            Item::Expr(e) => collect_occurrences(e, &none, out),
            Item::ActivePattern(decl) => {
                let names: Vec<String> = decl
                    .params
                    .iter()
                    .flat_map(|p| super::pattern_bindings(&p.pattern))
                    .collect();
                let inner = scope_bound(&names, &decl.value, &none);
                collect_occurrences(&decl.value, &inner, out);
            }
            // An in-file `module` flattens into module scope.
            Item::Module { items, .. } => {
                for b in items {
                    collect_let_occurrences(b, &none, out);
                }
            }
            _ => {}
        }
    }
}

/// Occurrences of `name` within one arm (its guard and body), counted the way
/// the frame census counted them, so the two can be subtracted.
pub(super) fn arm_occurrences(arm: &crate::parser::ast::MatchArm, name: &str) -> usize {
    let mut out = HashMap::new();
    let none = HashSet::new();
    if let Some(g) = &arm.guard {
        collect_occurrences(g, &none, &mut out);
    }
    collect_occurrences(&arm.body, &none, &mut out);
    out.get(name).copied().unwrap_or(0)
}
