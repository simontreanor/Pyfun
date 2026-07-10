//! Pretty-printer for the Pyfun AST.
//!
//! The output is *canonical*, not a faithful echo of the source: compound
//! expressions are fully parenthesized so the printed text always reparses to a
//! structurally identical AST. That property is what the parse→print→parse
//! roundtrip tests rely on. A formatting-quality printer (`pyfun fmt`) is a
//! later phase (`DESIGN.md` §10).

use crate::parser::ast::{
    ActivePatternDecl, BlockStmt, CeItem, Expr, ExprKind, ExternArg, ExternDecl, FieldDecl,
    FieldInit, InterpPart, Item, LetBinding, MatchArm, Module, Pattern, Receiver, TypeDecl,
    TypeDeclKind, TypeExpr, UnitExpr, VariantDecl,
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

/// Render the doc-comment lines attached to a declaration (`## …` per line,
/// column 0), ready to prefix the declaration itself. Re-emitting the doc is what
/// makes it survive the parse→print→parse roundtrip: the reprinted lines re-lex
/// as `Tok::Doc` and re-attach to the same item.
fn print_doc(doc: &Option<String>) -> String {
    let Some(doc) = doc else {
        return String::new();
    };
    doc.split('\n')
        .map(|line| {
            if line.is_empty() {
                "##\n".to_string()
            } else {
                format!("## {line}\n")
            }
        })
        .collect()
}

/// Render a single top-level item.
pub fn print_item(item: &Item) -> String {
    match item {
        Item::Measure {
            name, definition, ..
        } => match definition {
            Some(body) => format!("measure {name} = {}", print_unit(body)),
            None => format!("measure {name}"),
        },
        Item::Type(decl) => format!("{}{}", print_doc(&decl.doc), print_type_decl(decl)),
        Item::Extern(decl) => format!("{}{}", print_doc(&decl.doc), print_extern(decl)),
        Item::Let(binding) => format!("{}{}", print_doc(&binding.doc), print_let(binding, 0)),
        Item::Module { name, items, .. } => {
            let mut s = format!("module {name} =\n");
            let lines: Vec<String> = items
                .iter()
                .map(|b| format!("    {}", print_let(b, 1)))
                .collect();
            s.push_str(&lines.join("\n"));
            s
        }
        Item::Import { name, .. } => format!("import {name}"),
        Item::ActivePattern(decl) => {
            format!("{}{}", print_doc(&decl.doc), print_active_pattern(decl))
        }
        Item::Expr(expr) => print_layout(expr, 0),
    }
}

/// Print an active-pattern definition (`DESIGN.md` §7.2): the banana brackets
/// (`(|A|B|)` / `(|A|_|)`), the parameters, and the body like a `let` binding.
fn print_active_pattern(decl: &ActivePatternDecl) -> String {
    let mut s = String::from("let (|");
    for case in &decl.cases {
        s.push_str(&case.name);
        s.push('|');
    }
    if decl.partial {
        s.push_str("_|");
    }
    s.push(')');
    for param in &decl.params {
        s.push(' ');
        s.push_str(&param.name);
    }
    s.push_str(" =");
    s.push_str(&print_body(&decl.value, 0));
    s
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
    // An opaque handle type has no body: `extern type Conn [a b …]`.
    let mut s = if matches!(decl.kind, TypeDeclKind::Opaque) {
        String::from("extern type ")
    } else {
        String::from("type ")
    };
    s.push_str(&decl.name);
    for param in &decl.params {
        s.push(' ');
        s.push_str(param);
    }
    match &decl.kind {
        TypeDeclKind::Sum(variants) => {
            s.push_str(" = ");
            let variants: Vec<String> = variants.iter().map(print_variant).collect();
            s.push_str(&variants.join(" | "));
        }
        TypeDeclKind::Record(fields) => {
            s.push_str(" = ");
            let fields: Vec<String> = fields.iter().map(print_field_decl).collect();
            s.push_str(&format!("{{ {} }}", fields.join(", ")));
        }
        TypeDeclKind::Opaque => {}
    }
    s
}

/// Print an `extern` declaration. The `= target` clause is shown only when the
/// Python target differs from the Pyfun name (so the name-equals-name common case
/// reparses identically).
fn print_extern(decl: &ExternDecl) -> String {
    let mut s = String::from("extern ");
    if decl.pure {
        s.push_str("pure ");
    }
    s.push_str(&decl.name);
    s.push_str(": ");
    s.push_str(&print_type(&decl.ty));
    match decl.receiver {
        Some(Receiver::Method) => {
            // The parens are the method marker; they also carry any pinned kwargs.
            s.push_str(" = .");
            s.push_str(&decl.target.join("."));
            s.push_str(&print_extern_kwargs(&decl.kwargs));
        }
        Some(Receiver::Property) => {
            // A property read takes no call, hence no kwargs.
            s.push_str(" = .");
            s.push_str(&decl.target.join("."));
        }
        // An ordinary target prints its `= …` clause when the Python name differs
        // from the Pyfun name, or when kwargs pin something that must round-trip.
        None if decl.target != [decl.name.clone()] || !decl.kwargs.is_empty() => {
            s.push_str(" = ");
            s.push_str(&decl.target.join("."));
            if !decl.kwargs.is_empty() {
                s.push_str(&print_extern_kwargs(&decl.kwargs));
            }
        }
        None => {}
    }
    s
}

/// Print the pinned keyword arguments of an `extern` target as `(kw=lit, …)`.
/// Empty kwargs still print `()` so a receiver-method marker round-trips; the
/// caller decides whether to emit it (a property never does).
fn print_extern_kwargs(kwargs: &[(String, ExternArg)]) -> String {
    let parts: Vec<String> = kwargs
        .iter()
        .map(|(k, v)| format!("{k}={}", print_extern_arg(v)))
        .collect();
    format!("({})", parts.join(", "))
}

fn print_extern_arg(arg: &ExternArg) -> String {
    match arg {
        ExternArg::Str(s) => print_string(s),
        ExternArg::Int(n) => n.to_string(),
        ExternArg::Float(f) => format!("{f:?}"),
        ExternArg::Bool(b) => b.to_string(),
    }
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
        TypeExpr::Fun(a, b, effects) if effects.is_empty() => {
            format!("{} -> {}", print_type_atom(a), print_type(b))
        }
        // A declared-effect arrow prints its labels back as written
        // (`->{io, async}`), so the annotation round-trips faithfully.
        TypeExpr::Fun(a, b, effects) => format!(
            "{} ->{{{}}} {}",
            print_type_atom(a),
            effects.join(", "),
            print_type(b)
        ),
        TypeExpr::Con(name, _, args) if args.is_empty() => name.clone(),
        TypeExpr::Con(name, _, args) => {
            let args: Vec<String> = args.iter().map(print_type_atom).collect();
            format!("{name} {}", args.join(" "))
        }
        TypeExpr::Tuple(elems) => {
            let elems: Vec<String> = elems.iter().map(print_type).collect();
            format!("({})", elems.join(", "))
        }
    }
}

/// Print a type expression as an atom, parenthesizing anything compound so it can
/// sit as a single constructor field and reparse identically.
fn print_type_atom(ty: &TypeExpr) -> String {
    match ty {
        TypeExpr::Con(name, _, args) if args.is_empty() => name.clone(),
        // A tuple is already self-delimiting (its own parens).
        TypeExpr::Tuple(_) => print_type(ty),
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
        s.push_str(&param.name);
    }
    s.push_str(" =");
    s.push_str(&print_body(&binding.value, indent));
    s
}

/// Does an expression contain an indented block in a tail position (so it can't
/// be rendered inline / parenthesized)? Blocks only ever open in tail positions
/// of `let`/`fun`/`if`/`match`, so this spine is exhaustive.
fn needs_block(expr: &Expr) -> bool {
    match &expr.kind {
        ExprKind::Block { .. } => true,
        ExprKind::If { then, else_, .. } => needs_block(then) || needs_block(else_),
        // A `match` always renders offside (`match e:` / `case p:`), the canonical
        // Python-framed form (`DESIGN.md` §7.2). An inline parenthesized form still
        // exists in `print_expr` for a `match` embedded mid-expression.
        ExprKind::Match { .. } => true,
        ExprKind::Fn { body, .. } => needs_block(body),
        _ => false,
    }
}

/// Print the body following a `=` / `->` / `then` / `else`. A body that needs a
/// block goes on deeper lines via [`print_layout`]; an ordinary body stays inline
/// (preceded by a space). The return value includes its leading separator.
fn print_body(expr: &Expr, indent: usize) -> String {
    if needs_block(expr) {
        format!("\n{}", print_layout(expr, indent + 1))
    } else {
        format!(" {}", print_expr(expr))
    }
}

/// Render an expression at column `indent` (every line padded, first included),
/// using the offside layout for blocks and the `if`/`match`/`fun` that contain
/// them. Anything block-free falls back to the inline [`print_expr`] form, which
/// parenthesizes for unambiguous reparsing.
fn print_layout(expr: &Expr, indent: usize) -> String {
    let pad = "    ".repeat(indent);
    if !needs_block(expr) {
        return format!("{pad}{}", print_expr(expr));
    }
    match &expr.kind {
        ExprKind::Block { stmts } => print_block(stmts, indent),
        ExprKind::If { cond, then, else_ } => {
            // A right-nested `if` in the else branch prints as an `elif` chain (the
            // canonical form for `else if` too; `DESIGN.md` §7.2).
            let mut s = format!(
                "{pad}if {} then{}",
                print_expr(cond),
                print_body(then, indent)
            );
            let mut cur: &Expr = else_;
            while let ExprKind::If { cond, then, else_ } = &cur.kind {
                s.push_str(&format!(
                    "\n{pad}elif {} then{}",
                    print_expr(cond),
                    print_body(then, indent)
                ));
                cur = else_;
            }
            s.push_str(&format!("\n{pad}else{}", print_body(cur, indent)));
            s
        }
        ExprKind::Match { scrutinee, arms } => {
            let mut s = format!("{pad}match {}:", print_expr(scrutinee));
            for arm in arms {
                s.push_str(&format!(
                    "\n{pad}    case {}{}:{}",
                    print_pattern(&arm.pattern),
                    print_guard(&arm.guard),
                    print_body(&arm.body, indent + 1),
                ));
            }
            s
        }
        ExprKind::Fn { params, body } => {
            let names: Vec<&str> = params.iter().map(|p| p.name.as_str()).collect();
            format!(
                "{pad}fun {} ->{}",
                names.join(" "),
                print_body(body, indent)
            )
        }
        // Unreachable: `needs_block` is true only for the arms above.
        _ => format!("{pad}{}", print_expr(expr)),
    }
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
        ExprKind::Interp { parts } => print_interp(parts),
        ExprKind::Hole { name } => match name {
            Some(n) => format!("?{n}"),
            None => "?".to_string(),
        },
        ExprKind::Bool(b) => b.to_string(),
        ExprKind::Unit => "()".to_string(),
        ExprKind::Var(name) => name.clone(),

        ExprKind::Fn { params, body } => {
            let names: Vec<&str> = params.iter().map(|p| p.name.as_str()).collect();
            format!("(fun {} -> {})", names.join(" "), print_expr(body))
        }
        ExprKind::App { func, arg } => {
            format!("({} {})", print_expr(func), print_expr(arg))
        }
        // Parenthesized so it reparses correctly wherever it sits (e.g. as an
        // application argument); the body is itself self-parenthesizing.
        ExprKind::Try { body } => format!("(try {})", print_expr(body)),
        ExprKind::If { cond, then, else_ } => {
            // A right-nested `if` in the else branch flattens into an `elif` chain
            // (reparses to the same nested `If`).
            let mut s = format!("(if {} then {}", print_expr(cond), print_expr(then));
            let mut cur: &Expr = else_;
            while let ExprKind::If { cond, then, else_ } = &cur.kind {
                s.push_str(&format!(
                    " elif {} then {}",
                    print_expr(cond),
                    print_expr(then)
                ));
                cur = else_;
            }
            s.push_str(&format!(" else {})", print_expr(cur)));
            s
        }
        ExprKind::Match { scrutinee, arms } => {
            let arms: Vec<String> = arms.iter().map(print_arm).collect();
            format!("(match {}: {})", print_expr(scrutinee), arms.join(" "))
        }
        ExprKind::Binary { op, lhs, rhs } => {
            format!("({} {} {})", print_expr(lhs), op.symbol(), print_expr(rhs))
        }
        ExprKind::Unary { op, expr } => {
            format!("({} {})", op.symbol(), print_expr(expr))
        }
        ExprKind::Compare { first, rest } => {
            let mut s = print_expr(first);
            for (op, operand) in rest {
                s.push_str(&format!(" {} {}", op.symbol(), print_expr(operand)));
            }
            format!("({s})")
        }
        ExprKind::OpFunc(op) => format!("({})", op.symbol()),
        ExprKind::Pipe { lhs, rhs, backward } => {
            let op = if *backward { "<|" } else { "|>" };
            format!("({} {op} {})", print_expr(lhs), print_expr(rhs))
        }
        ExprKind::Compose {
            lhs,
            rhs,
            right_to_left,
        } => {
            let op = if *right_to_left { "<<" } else { ">>" };
            format!("({} {op} {})", print_expr(lhs), print_expr(rhs))
        }
        ExprKind::Ce { builder, items } => {
            let items: Vec<String> = items.iter().map(print_ce_item).collect();
            format!("{} {{ {} }}", builder.name(), items.join(" "))
        }
        ExprKind::Annot { value, unit } => {
            format!("{}<{}>", print_expr(value), print_unit(unit))
        }
        ExprKind::List { elems } => {
            let elems: Vec<String> = elems.iter().map(print_expr).collect();
            format!("[{}]", elems.join(", "))
        }
        ExprKind::Tuple { elems } => {
            let elems: Vec<String> = elems.iter().map(print_expr).collect();
            format!("({})", elems.join(", "))
        }
        ExprKind::Record { ty, fields, .. } => {
            let fields: Vec<String> = fields.iter().map(print_field_init).collect();
            format!("{ty} {{ {} }}", fields.join(", "))
        }
        ExprKind::RecordUpdate { base, fields } => {
            let fields: Vec<String> = fields.iter().map(print_field_update).collect();
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

/// Print a record-update assignment, joining a dotted field path (`a.b = v`).
fn print_field_update(field: &crate::syntax::FieldUpdate) -> String {
    format!("{} = {}", field.path.join("."), print_expr(&field.value))
}

fn print_ce_item(item: &CeItem) -> String {
    match item {
        CeItem::LetBang { name, value, .. } => format!("let! {name} = {}", print_expr(value)),
        CeItem::Let { name, value, .. } => format!("let {name} = {}", print_expr(value)),
        CeItem::DoBang(e) => format!("do! {}", print_expr(e)),
        CeItem::Return(e) => format!("return {}", print_expr(e)),
        CeItem::ReturnBang(e) => format!("return! {}", print_expr(e)),
        CeItem::Yield(e) => format!("yield {}", print_expr(e)),
        CeItem::YieldBang(e) => format!("yield! {}", print_expr(e)),
    }
}

/// Render the ` if guard` suffix of a guarded arm (empty when unguarded).
fn print_guard(guard: &Option<Expr>) -> String {
    match guard {
        Some(g) => format!(" if {}", print_expr(g)),
        None => String::new(),
    }
}

fn print_arm(arm: &MatchArm) -> String {
    format!(
        "case {}{}: {}",
        print_pattern(&arm.pattern),
        print_guard(&arm.guard),
        print_expr(&arm.body)
    )
}

/// Render a pattern. Constructors with arguments are parenthesized so they nest
/// and sit in arm position unambiguously.
pub fn print_pattern(pattern: &Pattern) -> String {
    match pattern {
        Pattern::Wildcard => "_".to_string(),
        Pattern::Var { name, .. } => name.clone(),
        Pattern::Int(n) => n.to_string(),
        Pattern::Str(s) => print_string(s),
        Pattern::Bool(b) => b.to_string(),
        Pattern::Ctor { name, args, .. } if args.is_empty() => name.clone(),
        Pattern::Ctor { name, args, .. } => {
            let args: Vec<String> = args.iter().map(print_pattern).collect();
            format!("({} {})", name, args.join(" "))
        }
        Pattern::Record { ty, fields, .. } => {
            let parts: Vec<String> = fields
                .iter()
                .map(|f| match &f.pattern {
                    // Print the `{ x }` shorthand back as shorthand.
                    Pattern::Var { name, .. } if *name == f.name => f.name.clone(),
                    p => format!("{} = {}", f.name, print_pattern(p)),
                })
                .collect();
            format!("{ty} {{ {} }}", parts.join(", "))
        }
        Pattern::Tuple { elems } => {
            let elems: Vec<String> = elems.iter().map(print_pattern).collect();
            format!("({})", elems.join(", "))
        }
        // `[a, b, *mid, z]` / `[]` — a list sequence pattern (brackets); the star
        // may sit anywhere, with `suffix` elements after it.
        Pattern::List {
            prefix,
            rest,
            suffix,
        } => {
            let mut parts: Vec<String> = prefix.iter().map(print_pattern).collect();
            if let Some(r) = rest {
                parts.push(format!("*{}", print_pattern(r)));
            }
            parts.extend(suffix.iter().map(print_pattern));
            format!("[{}]", parts.join(", "))
        }
        // Always parenthesized so a nested or-pattern (a constructor argument, a
        // tuple element) reparses to the same alternation rather than binding to
        // the enclosing constructor.
        Pattern::Or(alts) => {
            let alts: Vec<String> = alts.iter().map(print_pattern).collect();
            format!("({})", alts.join(" | "))
        }
        // Parenthesized so it reparses as its own unit (e.g. a constructor arg or
        // tuple element `(p as x)`).
        Pattern::As { pattern, name, .. } => {
            format!("({} as {name})", print_pattern(pattern))
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

/// Print an interpolated string `f"..."`: literal chunks re-escape their specials
/// (and re-double `{`/`}`), holes print their expression inside `{…}`. Reparsing the
/// result yields the same `ExprKind::Interp`.
fn print_interp(parts: &[InterpPart]) -> String {
    let mut out = String::from("f\"");
    for part in parts {
        match part {
            InterpPart::Lit(s) => {
                for c in s.chars() {
                    match c {
                        '"' => out.push_str("\\\""),
                        '\\' => out.push_str("\\\\"),
                        '\n' => out.push_str("\\n"),
                        '\t' => out.push_str("\\t"),
                        '{' => out.push_str("{{"),
                        '}' => out.push_str("}}"),
                        _ => out.push(c),
                    }
                }
            }
            InterpPart::Expr(e) => {
                out.push('{');
                out.push_str(&print_expr(e));
                out.push('}');
            }
        }
    }
    out.push('"');
    out
}
