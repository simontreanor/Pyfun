//! Pretty-printer for the Pyfun AST.
//!
//! The output is *canonical*, not a faithful echo of the source: compound
//! expressions are fully parenthesized so the printed text always reparses to a
//! structurally identical AST. That property is what the parse→print→parse
//! roundtrip tests rely on. A formatting-quality printer (`pyfun fmt`) is a
//! later phase (`DESIGN.md` §10).

use crate::parser::ast::{Expr, ExprKind, Item, LetBinding, MatchArm, Module, Pattern};

/// Render a whole module, one item per line.
pub fn print_module(module: &Module) -> String {
    let mut out = String::new();
    for item in &module.items {
        out.push_str(&print_item(item));
        out.push('\n');
    }
    out
}

/// Render a single top-level item.
pub fn print_item(item: &Item) -> String {
    match item {
        Item::Let(binding) => print_let(binding),
        Item::Expr(expr) => print_expr(expr),
    }
}

fn print_let(binding: &LetBinding) -> String {
    let mut s = String::from("let ");
    if binding.mutable {
        s.push_str("mut ");
    }
    s.push_str(&binding.name);
    for param in &binding.params {
        s.push(' ');
        s.push_str(param);
    }
    s.push_str(" = ");
    s.push_str(&print_expr(&binding.value));
    s
}

/// Render an expression. Atoms print bare; everything compound is wrapped in
/// parentheses so it can sit in any position and still reparse identically.
pub fn print_expr(expr: &Expr) -> String {
    match &expr.kind {
        ExprKind::Int(n) => n.to_string(),
        // `{:?}` guarantees a decimal point (e.g. `1.0`), so floats never
        // reparse as integers.
        ExprKind::Float(f) => format!("{f:?}"),
        ExprKind::Str(s) => print_string(s),
        ExprKind::Bool(b) => b.to_string(),
        ExprKind::Var(name) => name.clone(),

        ExprKind::Fn { params, body } => {
            format!("(fun {} -> {})", params.join(" "), print_expr(body))
        }
        ExprKind::App { func, arg } => {
            format!("({} {})", print_expr(func), print_expr(arg))
        }
        ExprKind::If { cond, then, else_ } => {
            format!(
                "(if {} then {} else {})",
                print_expr(cond),
                print_expr(then),
                print_expr(else_)
            )
        }
        ExprKind::Match { scrutinee, arms } => {
            let arms: Vec<String> = arms.iter().map(print_arm).collect();
            format!("(match {} with {})", print_expr(scrutinee), arms.join(" "))
        }
        ExprKind::Binary { op, lhs, rhs } => {
            format!("({} {} {})", print_expr(lhs), op.symbol(), print_expr(rhs))
        }
        ExprKind::Pipe { lhs, rhs } => {
            format!("({} |> {})", print_expr(lhs), print_expr(rhs))
        }
    }
}

fn print_arm(arm: &MatchArm) -> String {
    format!(
        "| {} -> {}",
        print_pattern(&arm.pattern),
        print_expr(&arm.body)
    )
}

/// Render a pattern. Constructors with arguments are parenthesized so they nest
/// and sit in arm position unambiguously.
pub fn print_pattern(pattern: &Pattern) -> String {
    match pattern {
        Pattern::Wildcard => "_".to_string(),
        Pattern::Var(name) => name.clone(),
        Pattern::Int(n) => n.to_string(),
        Pattern::Bool(b) => b.to_string(),
        Pattern::Ctor { name, args } if args.is_empty() => name.clone(),
        Pattern::Ctor { name, args } => {
            let args: Vec<String> = args.iter().map(print_pattern).collect();
            format!("({} {})", name, args.join(" "))
        }
    }
}

fn print_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}
