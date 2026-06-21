//! Pretty-printer for the Pyfun AST.
//!
//! The output is *canonical*, not a faithful echo of the source: compound
//! expressions are fully parenthesized so the printed text always reparses to a
//! structurally identical AST. That property is what the parse→print→parse
//! roundtrip tests rely on. A formatting-quality printer (`pyfun fmt`) is a
//! later phase (`DESIGN.md` §10).

use crate::parser::ast::{
    BlockStmt, CeItem, Expr, ExprKind, FieldDecl, FieldInit, Item, LetBinding, MatchArm, Module,
    Pattern, TypeDecl, TypeDeclKind, TypeExpr, UnitExpr, VariantDecl,
};

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
        Item::Measure { name, .. } => format!("measure {name}"),
        Item::Type(decl) => print_type_decl(decl),
        Item::Let(binding) => print_let(binding, 0),
        Item::Expr(expr) => print_expr(expr),
    }
}

/// Render a unit expression as it appears inside `<...>`.
fn print_unit(unit: &UnitExpr) -> String {
    if unit.factors.is_empty() {
        return "1".to_string();
    }
    let factor = |name: &str, exp: i32| {
        if exp.abs() == 1 {
            name.to_string()
        } else {
            format!("{name}^{}", exp.abs())
        }
    };
    let numer: Vec<String> = unit
        .factors
        .iter()
        .filter(|(_, e)| *e > 0)
        .map(|(n, e)| factor(n, *e))
        .collect();
    let denom: Vec<String> = unit
        .factors
        .iter()
        .filter(|(_, e)| *e < 0)
        .map(|(n, e)| factor(n, *e))
        .collect();
    let numer = if numer.is_empty() {
        "1".to_string()
    } else {
        numer.join(" ")
    };
    if denom.is_empty() {
        numer
    } else {
        format!("{numer}/{}", denom.join(" "))
    }
}

fn print_type_decl(decl: &TypeDecl) -> String {
    let mut s = String::from("type ");
    s.push_str(&decl.name);
    for param in &decl.params {
        s.push(' ');
        s.push_str(param);
    }
    s.push_str(" = ");
    match &decl.kind {
        TypeDeclKind::Sum(variants) => {
            let variants: Vec<String> = variants.iter().map(print_variant).collect();
            s.push_str(&variants.join(" | "));
        }
        TypeDeclKind::Record(fields) => {
            let fields: Vec<String> = fields.iter().map(print_field_decl).collect();
            s.push_str(&format!("{{ {} }}", fields.join(", ")));
        }
    }
    s
}

fn print_field_decl(field: &FieldDecl) -> String {
    format!("{}: {}", field.name, print_type(&field.ty))
}

fn print_variant(variant: &VariantDecl) -> String {
    let mut s = variant.name.clone();
    for field in &variant.fields {
        s.push(' ');
        s.push_str(&print_type_atom(field));
    }
    s
}

/// Print a full type expression (may contain `->`).
fn print_type(ty: &TypeExpr) -> String {
    match ty {
        TypeExpr::Fun(a, b) => format!("{} -> {}", print_type_atom(a), print_type(b)),
        TypeExpr::Con(name, args) if args.is_empty() => name.clone(),
        TypeExpr::Con(name, args) => {
            let args: Vec<String> = args.iter().map(print_type_atom).collect();
            format!("{name} {}", args.join(" "))
        }
    }
}

/// Print a type expression as an atom, parenthesizing anything compound so it can
/// sit as a single constructor field and reparse identically.
fn print_type_atom(ty: &TypeExpr) -> String {
    match ty {
        TypeExpr::Con(name, args) if args.is_empty() => name.clone(),
        _ => format!("({})", print_type(ty)),
    }
}

/// Print a binding at indentation `indent` (in 4-space levels). A block-valued
/// body is rendered as `=` followed by its statements on deeper lines, so it
/// reparses through the offside rule; an inline body stays on one line.
fn print_let(binding: &LetBinding, indent: usize) -> String {
    let mut s = String::from("let ");
    if binding.mutable {
        s.push_str("mut ");
    }
    if binding.pure {
        s.push_str("pure ");
    }
    s.push_str(&binding.name);
    for param in &binding.params {
        s.push(' ');
        s.push_str(param);
    }
    if let ExprKind::Block { stmts } = &binding.value.kind {
        s.push_str(" =\n");
        s.push_str(&print_block(stmts, indent + 1));
    } else {
        s.push_str(" = ");
        s.push_str(&print_expr(&binding.value));
    }
    s
}

/// Print block statements, one per line, each indented `indent` levels.
fn print_block(stmts: &[BlockStmt], indent: usize) -> String {
    let pad = "    ".repeat(indent);
    let lines: Vec<String> = stmts
        .iter()
        .map(|stmt| match stmt {
            BlockStmt::Let(b) => format!("{pad}{}", print_let(b, indent)),
            BlockStmt::Expr(e) => format!("{pad}{}", print_expr(e)),
        })
        .collect();
    lines.join("\n")
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
        ExprKind::Unary { op, expr } => {
            format!("({} {})", op.symbol(), print_expr(expr))
        }
        ExprKind::Pipe { lhs, rhs } => {
            format!("({} |> {})", print_expr(lhs), print_expr(rhs))
        }
        ExprKind::Ce { builder, items } => {
            let items: Vec<String> = items.iter().map(print_ce_item).collect();
            format!("{} {{ {} }}", builder.name(), items.join(" "))
        }
        ExprKind::Annot { value, unit } => {
            format!("{}<{}>", print_expr(value), print_unit(unit))
        }
        ExprKind::Record { fields } => {
            let fields: Vec<String> = fields.iter().map(print_field_init).collect();
            format!("{{ {} }}", fields.join(", "))
        }
        ExprKind::RecordUpdate { base, fields } => {
            let fields: Vec<String> = fields.iter().map(print_field_init).collect();
            format!("{{ {} with {} }}", print_expr(base), fields.join(", "))
        }
        ExprKind::Field { base, name } => format!("{}.{name}", print_expr(base)),
        // A block only ever appears as a binding's body (printed by `print_let`),
        // so this arm is a defensive fallback.
        ExprKind::Block { stmts } => format!("\n{}", print_block(stmts, 1)),
        ExprKind::Assign { target, value } => {
            format!("({target} <- {})", print_expr(value))
        }
    }
}

fn print_field_init(field: &FieldInit) -> String {
    format!("{} = {}", field.name, print_expr(&field.value))
}

fn print_ce_item(item: &CeItem) -> String {
    match item {
        CeItem::LetBang { name, value } => format!("let! {name} = {}", print_expr(value)),
        CeItem::Let { name, value } => format!("let {name} = {}", print_expr(value)),
        CeItem::DoBang(e) => format!("do! {}", print_expr(e)),
        CeItem::Return(e) => format!("return {}", print_expr(e)),
        CeItem::ReturnBang(e) => format!("return! {}", print_expr(e)),
        CeItem::Yield(e) => format!("yield {}", print_expr(e)),
        CeItem::YieldBang(e) => format!("yield! {}", print_expr(e)),
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
