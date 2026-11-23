//! `from Module import a, B` (`DESIGN.md` §6.1): the names a file asks a module
//! for, written bare from then on.
//!
//! The rest of the compiler knows one way to refer to another module's member,
//! the qualified `Module.member`. So this is a rewrite ahead of it: each bare
//! use of an imported name becomes the qualified reference it stands for, at
//! the same span, and the checker, the lowering and the language server see a
//! program that was written out in full. A local binder of the same name
//! shadows the import, as it does in Python.
//!
//! The statement follows Python strictly. It binds the names it lists and
//! nothing else, so `Module.other` still needs `import Module`; and a name it
//! lists cannot also be defined at the top of the file.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

use crate::lexer::Span;
use crate::parser::ast::{
    BlockStmt, CeBuilder, CeItem, Expr, ExprKind, InterpPart, Item, LetBinding, Module, NodeSpan,
    Pattern, TypeDeclKind,
};

/// Something wrong, or unused, in a file's `from` imports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub message: String,
    pub span: Span,
}

/// What [`report`] finds.
#[derive(Debug, Default)]
pub struct Report {
    /// A name imported twice or also defined in the file, and a `Module.x`
    /// written for a module that was only imported `from`.
    pub errors: Vec<Problem>,
    /// Each imported name nothing in the file uses.
    pub unused: Vec<Problem>,
}

/// `module` with every bare use of a `from`-imported name written out as
/// `Module.name`. Borrowed, untouched, when the file has no such import.
pub fn expand(module: &Module) -> Cow<'_, Module> {
    if !has_from_imports(module) {
        return Cow::Borrowed(module);
    }
    let mut expanded = module.clone();
    Rewriter::new(module).run(&mut expanded);
    Cow::Owned(expanded)
}

/// The errors and the unused names of `module`'s `from` imports.
pub fn report(module: &Module) -> Report {
    if !has_from_imports(module) {
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

/// The modules `module` imports with `from` and never with a plain `import`.
pub fn from_only_modules(module: &Module) -> HashSet<String> {
    let mut plain = HashSet::new();
    let mut from = HashSet::new();
    for item in &module.items {
        if let Item::Import { name, names, .. } = item {
            if names.is_empty() {
                plain.insert(name.clone());
            } else {
                from.insert(name.clone());
            }
        }
    }
    from.retain(|name| !plain.contains(name));
    from
}

fn has_from_imports(module: &Module) -> bool {
    module
        .items
        .iter()
        .any(|item| matches!(item, Item::Import { names, .. } if !names.is_empty()))
}

struct Rewriter {
    /// Each imported name: the module it comes from and where it was listed.
    aliases: HashMap<String, (String, Span)>,
    /// Modules imported only with `from`: `Module.x` is not theirs to write.
    from_only: HashSet<String>,
    /// The names in scope that are not the import: the file's own top-level
    /// names (innermost first is not needed, only membership), then each
    /// enclosing binder's.
    scopes: Vec<HashSet<String>>,
    used: HashSet<String>,
    errors: Vec<Problem>,
}

impl Rewriter {
    fn new(module: &Module) -> Rewriter {
        let mut rewriter = Rewriter {
            aliases: HashMap::new(),
            from_only: from_only_modules(module),
            scopes: Vec::new(),
            used: HashSet::new(),
            errors: Vec::new(),
        };
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
                Item::Measure { .. }
                | Item::Type(_)
                | Item::Extern(_)
                | Item::Import { .. }
                | Item::ExternImport { .. } => {}
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

    /// `Module.x` written out by hand, for a module imported only with `from`.
    fn qualified_by_hand(&mut self, module: &str, span: Span) {
        if self.from_only.contains(module) {
            self.errors.push(Problem {
                message: format!(
                    "`{module}` is not imported as a module: `from {module} import …` brings in \
                     the names it lists. Add `import {module}` to write `{module}.…`"
                ),
                span,
            });
        }
    }

    /// A constructor or record tag, bare or qualified by hand.
    fn tag(&mut self, name: &mut String, span: Span) {
        match name.split_once('.') {
            Some((module, _)) => {
                let module = module.to_string();
                self.qualified_by_hand(&module, span);
            }
            None => {
                if let Some(module) = self.imported(name) {
                    *name = format!("{module}.{name}");
                }
            }
        }
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
            ExprKind::Field { base, .. } => {
                if let ExprKind::Var(module) = &base.kind
                    && module.chars().next().is_some_and(char::is_uppercase)
                {
                    let module = module.clone();
                    self.qualified_by_hand(&module, span);
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
                    let module = module.clone();
                    self.qualified_by_hand(&module, span);
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
