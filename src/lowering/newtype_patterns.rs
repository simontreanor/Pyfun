//! Newtype patterns in binding positions erase before lowering (`DESIGN.md`
//! §7.3).
//!
//! A newtype's constructor does not exist at run time, so `let raw (UserId s) =
//! s` takes its argument *as* `s`: the pattern is its payload's, on the value
//! itself. Rewriting the pattern to its payload ahead of lowering makes that
//! literal. Every later pass sees `let raw s = s`, and the parameter is named
//! `s` in the emitted `def`, where unpacking it there would bind a reserved
//! argument name and assign `s` from it on the first line.
//!
//! Only binding positions are rewritten: a parameter, a `let` target, and a
//! computation expression's `let`, `let!` and `for` targets. A `case` pattern is
//! left to [`super::Lowerer::lower_pattern`], which erases it in place and
//! seals the case list after an arm that erasure made catch-all.

use std::collections::HashSet;

use crate::parser::ast::{
    BlockStmt, CeItem, Expr, ExprKind, InterpPart, Item, LetBinding, Module, Param, Pattern,
};

/// `module` with every newtype-constructor pattern in a binding position
/// replaced by its payload. `newtypes` holds the constructors as the source
/// writes them: bare for a local newtype, `Module.Name` for an imported one.
pub(super) fn erase(module: &Module, newtypes: &HashSet<String>) -> Module {
    let mut module = module.clone();
    if newtypes.is_empty() {
        return module;
    }
    for item in &mut module.items {
        match item {
            Item::Let(binding) => binding_mut(binding, newtypes),
            Item::Module { items, .. } => {
                for member in items {
                    binding_mut(member, newtypes);
                }
            }
            Item::ActivePattern(decl) => {
                params_mut(&mut decl.params, newtypes);
                expr_mut(&mut decl.value, newtypes);
            }
            Item::Expr(expr) => expr_mut(expr, newtypes),
            Item::Measure { .. }
            | Item::Type(_)
            | Item::Extern(_)
            | Item::Import { .. }
            | Item::ExternImport { .. } => {}
        }
    }
    module
}

fn binding_mut(binding: &mut LetBinding, newtypes: &HashSet<String>) {
    pattern_mut(&mut binding.target, newtypes);
    params_mut(&mut binding.params, newtypes);
    expr_mut(&mut binding.value, newtypes);
}

fn params_mut(params: &mut [Param], newtypes: &HashSet<String>) {
    for param in params {
        pattern_mut(&mut param.pattern, newtypes);
    }
}

/// Replace each newtype-constructor pattern in `pattern` by its payload, at any
/// depth: `(UserId s, Email e)` becomes `(s, e)`.
fn pattern_mut(pattern: &mut Pattern, newtypes: &HashSet<String>) {
    // Peel newtype constructors off the top: one may wrap another.
    while let Pattern::Ctor { name, args, .. } = pattern {
        if !(newtypes.contains(name.as_str()) && args.len() == 1) {
            break;
        }
        let payload = args.pop().expect("one argument");
        *pattern = payload;
    }
    match pattern {
        Pattern::Ctor { args, .. } => {
            for arg in args {
                pattern_mut(arg, newtypes);
            }
        }
        Pattern::Record { fields, .. } => {
            for field in fields {
                pattern_mut(&mut field.pattern, newtypes);
            }
        }
        Pattern::Tuple { elems } => {
            for elem in elems {
                pattern_mut(elem, newtypes);
            }
        }
        // The rest cannot appear in a binding position, or hold no pattern.
        Pattern::List { .. }
        | Pattern::Or(_)
        | Pattern::As { .. }
        | Pattern::Wildcard
        | Pattern::Var { .. }
        | Pattern::Int(_)
        | Pattern::Str(_)
        | Pattern::Bool(_) => {}
    }
}

fn ce_items_mut(items: &mut [CeItem], newtypes: &HashSet<String>) {
    for item in items {
        match item {
            CeItem::LetBang { target, value, .. } | CeItem::Let { target, value, .. } => {
                pattern_mut(target, newtypes);
                expr_mut(value, newtypes);
            }
            CeItem::DoBang(e)
            | CeItem::Return(e)
            | CeItem::ReturnBang(e)
            | CeItem::Yield(e)
            | CeItem::YieldBang(e) => expr_mut(e, newtypes),
            CeItem::For {
                target,
                source,
                body,
                ..
            } => {
                pattern_mut(target, newtypes);
                expr_mut(source, newtypes);
                ce_items_mut(body, newtypes);
            }
        }
    }
}

fn expr_mut(expr: &mut Expr, newtypes: &HashSet<String>) {
    match &mut expr.kind {
        ExprKind::Fn { params, body } => {
            params_mut(params, newtypes);
            expr_mut(body, newtypes);
        }
        ExprKind::Block { stmts } => {
            for stmt in stmts {
                match stmt {
                    BlockStmt::Let(binding) => binding_mut(binding, newtypes),
                    BlockStmt::Expr(e) => expr_mut(e, newtypes),
                }
            }
        }
        ExprKind::Ce { items, .. } => ce_items_mut(items, newtypes),
        // A `case` pattern stays as written; only the arm's expressions are walked.
        ExprKind::Match { scrutinee, arms } => {
            expr_mut(scrutinee, newtypes);
            for arm in arms {
                if let Some(guard) = &mut arm.guard {
                    expr_mut(guard, newtypes);
                }
                expr_mut(&mut arm.body, newtypes);
            }
        }
        ExprKind::Interp { parts } => {
            for part in parts {
                if let InterpPart::Expr(e) = part {
                    expr_mut(e, newtypes);
                }
            }
        }
        ExprKind::App { func, arg } => {
            expr_mut(func, newtypes);
            expr_mut(arg, newtypes);
        }
        ExprKind::If { cond, then, else_ } => {
            expr_mut(cond, newtypes);
            expr_mut(then, newtypes);
            expr_mut(else_, newtypes);
        }
        ExprKind::Binary { lhs, rhs, .. }
        | ExprKind::Pipe { lhs, rhs, .. }
        | ExprKind::Compose { lhs, rhs, .. } => {
            expr_mut(lhs, newtypes);
            expr_mut(rhs, newtypes);
        }
        ExprKind::Compare { first, rest } => {
            expr_mut(first, newtypes);
            for (_, e) in rest {
                expr_mut(e, newtypes);
            }
        }
        ExprKind::List { elems } | ExprKind::Tuple { elems } => {
            for e in elems {
                expr_mut(e, newtypes);
            }
        }
        ExprKind::Record { fields, .. } => {
            for field in fields {
                expr_mut(&mut field.value, newtypes);
            }
        }
        ExprKind::RecordUpdate { base, fields } => {
            expr_mut(base, newtypes);
            for field in fields {
                expr_mut(&mut field.value, newtypes);
            }
        }
        ExprKind::Try { body: inner }
        | ExprKind::Unary { expr: inner, .. }
        | ExprKind::Annot { value: inner, .. }
        | ExprKind::Collection {
            expansion: inner, ..
        }
        | ExprKind::Field { base: inner, .. }
        | ExprKind::Assign { value: inner, .. } => expr_mut(inner, newtypes),
        ExprKind::Int(_)
        | ExprKind::Float(_)
        | ExprKind::Str(_)
        | ExprKind::Hole { .. }
        | ExprKind::Bool(_)
        | ExprKind::Unit
        | ExprKind::Var(_)
        | ExprKind::OpFunc(_) => {}
    }
}
