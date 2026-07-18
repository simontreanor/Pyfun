//! Python 3.11 compatibility pass (`--target 3.11`). Python only allowed an
//! f-string hole to reuse the outer quote or contain a backslash from 3.12
//! (PEP 701), and Pyfun's default target leans on that: a hole may carry a
//! nested string literal emitted with the same `"` quotes. This pass rewrites
//! every f-string a 3.11 parser would reject into an equivalent
//! `"template".format(args…)` call — the hole expressions move out of the
//! string, where no quoting restrictions apply. F-strings whose holes render
//! clean stay f-strings, so 3.11-target output only changes where it must.
//!
//! The safety check runs on the *rendered* hole text (through the emitter
//! itself), so it is exact by construction: a hole is 3.11-unsafe iff its
//! emitted expression contains a `"` or a `\`. Patterns are not walked: an
//! f-string cannot occur in a `case` pattern, and a `.format()` call would
//! not be a valid one.

use super::{PyExpr, PyFStrPart, PyModule, PyStmt};

/// Rewrite every f-string in `module` that a Python 3.11 parser would reject.
pub fn rewrite_module(module: &mut PyModule) {
    for stmt in &mut module.body {
        rewrite_stmt(stmt);
    }
}

fn rewrite_stmt(stmt: &mut PyStmt) {
    match stmt {
        PyStmt::Import(_)
        | PyStmt::ImportFrom { .. }
        | PyStmt::Nonlocal(_)
        | PyStmt::Global(_)
        | PyStmt::RaiseRuntimeError(_)
        | PyStmt::ClassDef { .. } => {}
        PyStmt::Assign { value, .. } => rewrite_expr(value),
        PyStmt::SubscriptAssign { obj, index, value } => {
            rewrite_expr(obj);
            rewrite_expr(index);
            rewrite_expr(value);
        }
        PyStmt::For { iter, body, .. } => {
            rewrite_expr(iter);
            for s in body {
                rewrite_stmt(s);
            }
        }
        PyStmt::Return(e) | PyStmt::Expr(e) | PyStmt::Yield(e) | PyStmt::YieldFrom(e) => {
            rewrite_expr(e)
        }
        PyStmt::FuncDef { body, .. } => {
            for s in body {
                rewrite_stmt(s);
            }
        }
        PyStmt::If { test, body, orelse } => {
            rewrite_expr(test);
            for s in body.iter_mut().chain(orelse) {
                rewrite_stmt(s);
            }
        }
        PyStmt::Match { subject, cases } => {
            rewrite_expr(subject);
            for case in cases {
                if let Some(guard) = &mut case.guard {
                    rewrite_expr(guard);
                }
                for s in &mut case.body {
                    rewrite_stmt(s);
                }
            }
        }
        PyStmt::Raise(e) => rewrite_expr(e),
        PyStmt::Try { body, handler, .. } => {
            for s in body.iter_mut().chain(handler) {
                rewrite_stmt(s);
            }
        }
    }
}

/// Rewrite children first (an f-string nested in a hole becomes a `.format()`
/// call before the outer hole's text is judged), then the expression itself.
fn rewrite_expr(e: &mut PyExpr) {
    match e {
        PyExpr::Int(_)
        | PyExpr::Float(_)
        | PyExpr::Str(_)
        | PyExpr::Bool(_)
        | PyExpr::Name(_)
        | PyExpr::NoneLit => {}
        PyExpr::FStr(parts) => {
            for part in parts.iter_mut() {
                if let PyFStrPart::Expr(hole) = part {
                    rewrite_expr(hole);
                }
            }
            if fstr_needs_rewrite(parts) {
                *e = format_call(parts);
            }
        }
        PyExpr::BinOp { left, right, .. } => {
            rewrite_expr(left);
            rewrite_expr(right);
        }
        PyExpr::Compare {
            left, comparators, ..
        } => {
            rewrite_expr(left);
            for c in comparators {
                rewrite_expr(c);
            }
        }
        PyExpr::Call { func, args } => {
            rewrite_expr(func);
            for a in args {
                rewrite_expr(a);
            }
        }
        PyExpr::CallKw { func, args, kwargs } => {
            rewrite_expr(func);
            for a in args {
                rewrite_expr(a);
            }
            for (_, v) in kwargs {
                rewrite_expr(v);
            }
        }
        PyExpr::IfExp { body, test, orelse } => {
            rewrite_expr(body);
            rewrite_expr(test);
            rewrite_expr(orelse);
        }
        PyExpr::Lambda { body, .. } => rewrite_expr(body),
        PyExpr::Attribute { value, .. } => rewrite_expr(value),
        PyExpr::Subscript { value, index } => {
            rewrite_expr(value);
            rewrite_expr(index);
        }
        PyExpr::Slice {
            value,
            lower,
            upper,
        } => {
            rewrite_expr(value);
            rewrite_expr(lower);
            rewrite_expr(upper);
        }
        PyExpr::Await(inner) | PyExpr::Not(inner) | PyExpr::Neg(inner) => rewrite_expr(inner),
        PyExpr::List(elems) | PyExpr::Tuple(elems) => {
            for el in elems {
                rewrite_expr(el);
            }
        }
    }
}

/// A hole is 3.11-unsafe iff its emitted text contains a `"` (outer-quote
/// reuse) or a `\` (any escape) — both PEP 701-only inside a hole.
fn fstr_needs_rewrite(parts: &[PyFStrPart]) -> bool {
    parts.iter().any(|part| match part {
        PyFStrPart::Lit(_) => false,
        PyFStrPart::Expr(hole) => {
            let rendered = super::emit_expr(hole, 0);
            rendered.contains('"') || rendered.contains('\\')
        }
    })
}

/// Build `"template".format(hole0, hole1, …)`: literal chunks with `{`/`}`
/// doubled so `str.format` reads them literally, one positional `{}` per hole.
fn format_call(parts: &[PyFStrPart]) -> PyExpr {
    let mut template = String::new();
    let mut args = Vec::new();
    for part in parts {
        match part {
            PyFStrPart::Lit(s) => {
                for c in s.chars() {
                    match c {
                        '{' => template.push_str("{{"),
                        '}' => template.push_str("}}"),
                        _ => template.push(c),
                    }
                }
            }
            PyFStrPart::Expr(hole) => {
                template.push_str("{}");
                args.push(hole.clone());
            }
        }
    }
    PyExpr::Call {
        func: Box::new(PyExpr::Attribute {
            value: Box::new(PyExpr::Str(template)),
            attr: "format".to_string(),
        }),
        args,
    }
}
