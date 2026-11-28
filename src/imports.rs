//! What a file's `import` lines let it write (`DESIGN.md` §6.1), put back into
//! the one form the rest of the compiler knows: a module's whole name, a dot,
//! and the member.
//!
//! Two things are shorter in the source than that. `from Module import a, B`
//! lets the listed names be written bare, and `import Shapes.Geometry` lets the
//! module be written by the last part of its name, `Geometry.area`. So this is
//! a rewrite ahead of everything else: each such use becomes the reference it
//! stands for (`Shapes.Geometry.area`), at the same span, and the checker, the
//! lowering and the language server see a program that was written out in
//! full. A local binder of the same name shadows a `from` import, as it does
//! in Python.
//!
//! `from` follows Python strictly. It binds the names it lists and nothing
//! else, so `Module.other` still needs `import Module`; and a name it lists
//! cannot also be defined at the top of the file. A file imports one module
//! by each last name, since that name is how the code says it.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

use crate::lexer::Span;
use crate::parser::ast::{
    BlockStmt, CeBuilder, CeItem, Expr, ExprKind, InterpPart, Item, LetBinding, Module, NodeSpan,
    Pattern, TypeDeclKind, TypeExpr,
};
use crate::project::module_short_name;

/// Something wrong, or unused, in a file's imports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub message: String,
    pub span: Span,
}

/// What [`report`] finds.
#[derive(Debug, Default)]
pub struct Report {
    /// A name imported twice or also defined in the file, a `Module.x`
    /// written for a module that was only imported `from`, and two modules
    /// imported under one last name.
    pub errors: Vec<Problem>,
    /// Each imported name nothing in the file uses.
    pub unused: Vec<Problem>,
}

/// `module` with every bare use of a `from`-imported name written out as
/// `Module.name`, and every module in a package written by its whole name.
/// Borrowed, untouched, when the file has no such import.
pub fn expand(module: &Module) -> Cow<'_, Module> {
    if !needs_rewriting(module) {
        return Cow::Borrowed(module);
    }
    let mut expanded = module.clone();
    Rewriter::new(module).run(&mut expanded);
    Cow::Owned(expanded)
}

/// The errors of `module`'s imports, and the names its `from` imports list
/// that nothing uses.
pub fn report(module: &Module) -> Report {
    if !needs_rewriting(module) {
        return Report::default();
    }
    let mut rewriter = Rewriter::new(module);
    let mut scratch = module.clone();
    rewriter.run(&mut scratch);
    // A type is one flat namespace across a project, so a listed type name is
    // used by being mentioned in a type, where nothing is rewritten.
    let type_mentions: HashSet<String> = crate::lsp::resolve::type_references(module)
        .into_iter()
        .map(|t| t.name)
        .collect();
    let mut report = Report {
        errors: std::mem::take(&mut rewriter.errors),
        unused: Vec::new(),
    };
    for (name, (from, span)) in &rewriter.aliases {
        if !rewriter.used.contains(name) && !type_mentions.contains(name) {
            report.unused.push(Problem {
                message: format!("`{name}` (from `{from}`) is never used"),
                span: *span,
            });
        }
    }
    report.unused.sort_by_key(|p| p.span.start);
    report.errors.sort_by_key(|p| p.span.start);
    report
}

/// Whether any import of `module` lets something be written short: a `from`
/// import's names, or a module in a package.
fn needs_rewriting(module: &Module) -> bool {
    module.items.iter().any(
        |item| matches!(item, Item::Import { name, names, .. } if !names.is_empty() || name.contains('.')),
    )
}

struct Rewriter {
    /// Each imported name: the module it comes from and where it was listed.
    aliases: HashMap<String, (String, Span)>,
    /// Each module a plain `import` binds, by the name the code uses for it
    /// (the last part), to its whole name.
    plain: HashMap<String, String>,
    /// Modules imported only with `from`, the same way: `Module.x` is not
    /// theirs to write.
    from_only: HashMap<String, String>,
    /// The names in scope that are not the import: the file's own top-level
    /// names (innermost first is not needed, only membership), then each
    /// enclosing binder's.
    scopes: Vec<HashSet<String>>,
    /// The file's own type names, which a bare type name always means.
    local_types: HashSet<String>,
    used: HashSet<String>,
    errors: Vec<Problem>,
}

impl Rewriter {
    fn new(module: &Module) -> Rewriter {
        let mut rewriter = Rewriter {
            aliases: HashMap::new(),
            plain: HashMap::new(),
            from_only: HashMap::new(),
            scopes: Vec::new(),
            local_types: module
                .items
                .iter()
                .filter_map(|item| match item {
                    Item::Type(decl) => Some(decl.name.clone()),
                    _ => None,
                })
                .collect(),
            used: HashSet::new(),
            errors: Vec::new(),
        };
        for item in &module.items {
            let Item::Import { name, names, span } = item else {
                continue;
            };
            let short = module_short_name(name).to_string();
            if !names.is_empty() {
                rewriter.from_only.insert(short, name.clone());
                continue;
            }
            match rewriter.plain.get(&short) {
                Some(other) if other != name => rewriter.errors.push(Problem {
                    message: format!(
                        "`{short}` already means `{other}` in this file, so `{name}` cannot be \
                         imported beside it: a module is used by the last part of its name. \
                         Ask this one for what you need, with `from {name} import …`"
                    ),
                    span: span.span(),
                }),
                Some(_) => {}
                None => {
                    rewriter.plain.insert(short, name.clone());
                }
            }
        }
        let plain = &rewriter.plain;
        rewriter
            .from_only
            .retain(|short, _| !plain.contains_key(short));
        let own = own_names(module);
        for item in &module.items {
            let Item::Import { name, names, .. } = item else {
                continue;
            };
            for imported in names {
                let span = imported.span.span();
                if own.contains(&imported.name) {
                    rewriter.errors.push(Problem {
                        message: format!(
                            "`{n}` is imported from `{name}` and also defined in this file: give \
                             one of them another name, or `import {name}` and write `{name}.{n}`",
                            n = imported.name
                        ),
                        span,
                    });
                } else if let Some((other, _)) = rewriter.aliases.get(&imported.name) {
                    let message = if other == name {
                        format!("`{}` is imported from `{name}` twice", imported.name)
                    } else {
                        format!(
                            "`{n}` is imported from both `{other}` and `{name}`: import one of \
                             the modules whole and write `{name}.{n}`",
                            n = imported.name
                        )
                    };
                    rewriter.errors.push(Problem { message, span });
                } else {
                    rewriter
                        .aliases
                        .insert(imported.name.clone(), (name.clone(), span));
                }
            }
        }
        // The file's own names never mean the import (a clash is reported
        // above, once, and not again at each use).
        rewriter.scopes.push(own);
        rewriter
    }

    fn run(&mut self, module: &mut Module) {
        for item in &mut module.items {
            match item {
                Item::Let(binding) => self.binding(binding),
                Item::Module { items, .. } => {
                    // A member's siblings are in scope by their bare names.
                    self.scopes.push(
                        items
                            .iter()
                            .flat_map(|member| member.bound_names())
                            .collect(),
                    );
                    for member in items {
                        self.binding(member);
                    }
                    self.scopes.pop();
                }
                Item::Expr(expr) => self.expr(expr),
                Item::ActivePattern(decl) => {
                    let mut bound = HashSet::new();
                    for param in &mut decl.params {
                        self.pattern(&mut param.pattern);
                        bound.extend(param.pattern.bound_names());
                    }
                    self.scopes.push(bound);
                    self.expr(&mut decl.value);
                    self.scopes.pop();
                }
                Item::Type(decl) => match &mut decl.kind {
                    TypeDeclKind::Sum(variants) => {
                        for variant in variants {
                            for field in &mut variant.fields {
                                self.type_expr(field);
                            }
                        }
                    }
                    TypeDeclKind::Record(fields) => {
                        for field in fields {
                            self.type_expr(&mut field.ty);
                        }
                    }
                    TypeDeclKind::Newtype(underlying) => self.type_expr(underlying),
                    TypeDeclKind::Opaque => {}
                },
                Item::Extern(decl) => self.type_expr(&mut decl.ty),
                Item::Measure { .. } | Item::Import { .. } | Item::ExternImport { .. } => {}
            }
        }
    }

    /// A type written `Geometry.Shape` names its module by the last part too,
    /// and a bare type a `from` import lists is that module's (`Reader` after
    /// `from Net import Reader` is `Net.Reader`, whatever else declares one).
    fn type_expr(&mut self, ty: &mut TypeExpr) {
        match ty {
            TypeExpr::Con(name, span, args) => {
                if let Some((module, member)) = name.rsplit_once('.') {
                    let (module, member) = (module.to_string(), member.to_string());
                    if let Some(whole) = self.whole_name(&module, span.span()) {
                        *name = format!("{whole}.{member}");
                    }
                } else if !self.local_types.contains(name.as_str())
                    && let Some((module, _)) = self.aliases.get(name.as_str())
                {
                    let module = module.clone();
                    self.used.insert(name.clone());
                    *name = format!("{module}.{name}");
                }
                for arg in args {
                    self.type_expr(arg);
                }
            }
            TypeExpr::Fun(from, to, _) => {
                self.type_expr(from);
                self.type_expr(to);
            }
            TypeExpr::Tuple(elems) => {
                for elem in elems {
                    self.type_expr(elem);
                }
            }
        }
    }

    fn is_bound(&self, name: &str) -> bool {
        self.scopes.iter().any(|scope| scope.contains(name))
    }

    /// The module `name` is imported from, if it means the import here.
    fn imported(&mut self, name: &str) -> Option<String> {
        if self.is_bound(name) {
            return None;
        }
        let module = self.aliases.get(name)?.0.clone();
        self.used.insert(name.to_string());
        Some(module)
    }

    /// The whole name of the module written `module` in a `module.x`, when
    /// it is longer than what was written: a module in a package, imported
    /// plainly. Reports a module that was only imported `from`, which has no
    /// name of its own to write here.
    fn whole_name(&mut self, module: &str, span: Span) -> Option<String> {
        if let Some(whole) = self.plain.get(module) {
            return (whole != module).then(|| whole.clone());
        }
        if let Some(whole) = self.from_only.get(module) {
            self.errors.push(Problem {
                message: format!(
                    "`{module}` is not imported as a module: `from {whole} import …` brings in \
                     the names it lists. Add `import {whole}` to write `{module}.…`"
                ),
                span,
            });
            // Read on as the module it names, so the mistake is reported once.
            return (whole != module).then(|| whole.clone());
        }
        None
    }

    /// A constructor or record tag, bare or qualified by hand.
    fn tag(&mut self, name: &mut String, span: Span) {
        match name.rsplit_once('.') {
            Some((module, member)) => {
                let (module, member) = (module.to_string(), member.to_string());
                if let Some(whole) = self.whole_name(&module, span) {
                    *name = format!("{whole}.{member}");
                }
            }
            None => {
                if let Some(module) = self.imported(name) {
                    *name = format!("{module}.{name}");
                }
            }
        }
    }

    /// The module a chain of capitalized names spells out in full
    /// (`Shapes.Geometry`), when the file imports one by that whole name.
    fn spelled_out(&self, expr: &Expr) -> Option<String> {
        fn path(expr: &Expr) -> Option<String> {
            match &expr.kind {
                ExprKind::Var(name) if name.starts_with(char::is_uppercase) => Some(name.clone()),
                ExprKind::Field { base, name } if name.starts_with(char::is_uppercase) => {
                    Some(format!("{}.{name}", path(base)?))
                }
                _ => None,
            }
        }
        let whole = path(expr)?;
        (whole.contains('.')
            && self
                .plain
                .values()
                .chain(self.from_only.values())
                .any(|name| *name == whole))
        .then_some(whole)
    }

    fn binding(&mut self, binding: &mut LetBinding) {
        let mut bound: HashSet<String> = HashSet::new();
        for param in &mut binding.params {
            self.pattern(&mut param.pattern);
            bound.extend(param.pattern.bound_names());
        }
        // A function is in scope in its own body.
        if !binding.params.is_empty() {
            bound.extend(binding.bound_names());
        }
        self.pattern(&mut binding.target);
        self.scopes.push(bound);
        self.expr(&mut binding.value);
        self.scopes.pop();
    }

    fn pattern(&mut self, pattern: &mut Pattern) {
        match pattern {
            Pattern::Wildcard
            | Pattern::Var { .. }
            | Pattern::Int(_)
            | Pattern::Str(_)
            | Pattern::Bool(_) => {}
            Pattern::Ctor {
                name,
                name_span,
                args,
            } => {
                self.tag(name, name_span.span());
                for arg in args {
                    self.pattern(arg);
                }
            }
            Pattern::Record {
                ty,
                ty_span,
                fields,
            } => {
                self.tag(ty, ty_span.span());
                for field in fields {
                    self.pattern(&mut field.pattern);
                }
            }
            Pattern::Tuple { elems } | Pattern::Or(elems) => {
                for elem in elems {
                    self.pattern(elem);
                }
            }
            Pattern::List {
                prefix,
                rest,
                suffix,
            } => {
                for elem in prefix.iter_mut().chain(suffix) {
                    self.pattern(elem);
                }
                if let Some(rest) = rest {
                    self.pattern(rest);
                }
            }
            Pattern::As { pattern, .. } => self.pattern(pattern),
        }
    }

    fn items(&mut self, items: &mut [CeItem]) {
        self.scopes.push(HashSet::new());
        for item in items {
            match item {
                CeItem::LetBang { target, value, .. } | CeItem::Let { target, value, .. } => {
                    self.expr(value);
                    self.pattern(target);
                    let names = target.bound_names();
                    self.scopes.last_mut().expect("pushed above").extend(names);
                }
                CeItem::DoBang(e)
                | CeItem::Return(e)
                | CeItem::ReturnBang(e)
                | CeItem::Yield(e)
                | CeItem::YieldBang(e) => self.expr(e),
                CeItem::For {
                    target,
                    source,
                    body,
                    ..
                } => {
                    self.expr(source);
                    self.pattern(target);
                    self.scopes.push(target.bound_names().into_iter().collect());
                    self.items(body);
                    self.scopes.pop();
                }
            }
        }
        self.scopes.pop();
    }

    fn expr(&mut self, expr: &mut Expr) {
        let span = expr.span();
        match &mut expr.kind {
            ExprKind::Int(_)
            | ExprKind::Float(_)
            | ExprKind::Str(_)
            | ExprKind::Bool(_)
            | ExprKind::Hole { .. }
            | ExprKind::Unit
            | ExprKind::OpFunc(_) => {}
            ExprKind::Var(name) => {
                if let Some(module) = self.imported(name) {
                    let member = std::mem::take(name);
                    expr.kind = ExprKind::Field {
                        base: Box::new(Expr {
                            kind: ExprKind::Var(module),
                            span: NodeSpan::new(span),
                        }),
                        name: member,
                    };
                }
            }
            ExprKind::Field { base, name } => {
                if let ExprKind::Var(module) = &base.kind
                    && module.chars().next().is_some_and(char::is_uppercase)
                {
                    let module = module.clone();
                    if let Some(whole) = self.whole_name(&module, span) {
                        base.kind = ExprKind::Var(whole);
                    }
                } else if let Some(whole) = self.spelled_out(base) {
                    // `Shapes.Geometry.area`, the path written again at the use.
                    let short = module_short_name(&whole).to_string();
                    self.errors.push(Problem {
                        message: format!(
                            "write `{short}.{name}`: the `import` line gives the module's whole \
                             name once, and the code then uses the last part of it"
                        ),
                        span,
                    });
                    base.kind = ExprKind::Var(whole);
                } else {
                    self.expr(base);
                }
            }
            ExprKind::Fn { params, body } => {
                let mut bound = HashSet::new();
                for param in params {
                    self.pattern(&mut param.pattern);
                    bound.extend(param.pattern.bound_names());
                }
                self.scopes.push(bound);
                self.expr(body);
                self.scopes.pop();
            }
            ExprKind::App { func, arg } => {
                self.expr(func);
                self.expr(arg);
            }
            ExprKind::If { cond, then, else_ } => {
                self.expr(cond);
                self.expr(then);
                self.expr(else_);
            }
            ExprKind::Try { body } => self.expr(body),
            ExprKind::Collection { expansion, .. } => self.expr(expansion),
            ExprKind::Match { scrutinee, arms } => {
                self.expr(scrutinee);
                for arm in arms {
                    self.pattern(&mut arm.pattern);
                    self.scopes
                        .push(arm.pattern.bound_names().into_iter().collect());
                    if let Some(guard) = &mut arm.guard {
                        self.expr(guard);
                    }
                    self.expr(&mut arm.body);
                    self.scopes.pop();
                }
            }
            ExprKind::Binary { lhs, rhs, .. }
            | ExprKind::Pipe { lhs, rhs, .. }
            | ExprKind::Compose { lhs, rhs, .. } => {
                self.expr(lhs);
                self.expr(rhs);
            }
            ExprKind::Unary { expr, .. } => self.expr(expr),
            ExprKind::Compare { first, rest } => {
                self.expr(first);
                for (_, operand) in rest {
                    self.expr(operand);
                }
            }
            ExprKind::Ce { builder, items } => {
                if let CeBuilder::User(module) = builder {
                    let written = module.clone();
                    if let Some(whole) = self.whole_name(&written, span) {
                        *module = whole;
                    }
                }
                self.items(items);
            }
            ExprKind::Annot { value, .. } => self.expr(value),
            ExprKind::List { elems } | ExprKind::Tuple { elems } => {
                for elem in elems {
                    self.expr(elem);
                }
            }
            ExprKind::Interp { parts } => {
                for part in parts {
                    if let InterpPart::Expr(e) = part {
                        self.expr(e);
                    }
                }
            }
            ExprKind::Record {
                ty,
                ty_span,
                fields,
            } => {
                self.tag(ty, ty_span.span());
                for field in fields {
                    self.expr(&mut field.value);
                }
            }
            ExprKind::RecordUpdate { base, fields } => {
                self.expr(base);
                for field in fields {
                    self.expr(&mut field.value);
                }
            }
            ExprKind::Block { stmts } => {
                self.scopes.push(HashSet::new());
                for stmt in stmts {
                    match stmt {
                        BlockStmt::Let(binding) => {
                            self.binding(binding);
                            let names = binding.bound_names();
                            self.scopes.last_mut().expect("pushed above").extend(names);
                        }
                        BlockStmt::Expr(e) => self.expr(e),
                    }
                }
                self.scopes.pop();
            }
            // The target of `<-` is a `let mut` of this file.
            ExprKind::Assign { value, .. } => self.expr(value),
        }
    }
}

/// Every name the file itself defines at top level: values, externs,
/// constructors, record types and active-pattern cases.
fn own_names(module: &Module) -> HashSet<String> {
    let mut names = HashSet::new();
    for item in &module.items {
        match item {
            Item::Let(binding) => names.extend(binding.bound_names()),
            Item::Extern(decl) => {
                names.insert(decl.name.clone());
            }
            Item::Type(decl) => match &decl.kind {
                TypeDeclKind::Sum(variants) => {
                    names.extend(variants.iter().map(|v| v.name.clone()));
                }
                TypeDeclKind::Record(_) | TypeDeclKind::Newtype(_) => {
                    names.insert(decl.name.clone());
                }
                TypeDeclKind::Opaque => {}
            },
            Item::ActivePattern(decl) => {
                names.extend(decl.cases.iter().map(|case| case.name.clone()));
            }
            Item::Measure { .. }
            | Item::Module { .. }
            | Item::Import { .. }
            | Item::ExternImport { .. }
            | Item::Expr(_) => {}
        }
    }
    names
}
