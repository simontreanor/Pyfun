//! Name resolution for editor navigation (`DESIGN.md` §9): a dependency-free walk
//! over the parsed AST that powers go-to-definition and completion. It is
//! independent of the type checker — it only needs spans and names, so it works on
//! any program that *parses*, even one that fails to type-check.
//!
//! Every identifier *reference* resolves to a [`Target`]: either a **local** binder
//! (a function parameter, block-local `let`, pattern variable, or computation-
//! expression `let`/`let!` — resolved to the binder's own span) or a **module-level**
//! symbol (resolved by name against [`definitions`]). The walk tracks lexical scopes
//! so an inner binding correctly shadows an outer one.

use std::collections::HashMap;

use crate::lexer::Span;
use crate::syntax::{
    BlockStmt, CeItem, Expr, ExprKind, Item, LetBinding, MatchArm, Module, Pattern, TypeDeclKind,
};

/// What kind of thing a module-level symbol is (drives the editor's icon and, for
/// completion, the `CompletionItemKind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolKind {
    Value,
    Constructor,
    Type,
    Record,
    Extern,
    Measure,
}

/// A module-level definition: its name, the span to jump to, and its kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol {
    pub name: String,
    pub span: Span,
    pub kind: SymbolKind,
}

/// Where a reference resolves to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// A local binder (parameter / block `let` / pattern var), with its span.
    Local(Span),
    /// A module-level symbol, by name (look up in [`definitions`]).
    Module(String),
}

/// One identifier reference: the span of the occurrence and what it resolves to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reference {
    pub span: Span,
    pub target: Target,
}

/// Collect every module-level definition (in declaration order). Constructor and
/// `extern` definitions point at their whole declaration (no separate name span
/// exists yet); top-level `let`s point at their precise name span.
pub fn definitions(module: &Module) -> Vec<Symbol> {
    let mut out = Vec::new();
    for item in &module.items {
        match item {
            Item::Let(binding) => out.push(Symbol {
                name: binding.name.clone(),
                span: binding.name_span.span(),
                kind: SymbolKind::Value,
            }),
            Item::Extern(decl) => out.push(Symbol {
                name: decl.name.clone(),
                span: decl.span.span(),
                kind: SymbolKind::Extern,
            }),
            Item::Type(decl) => {
                let kind = match decl.kind {
                    TypeDeclKind::Sum(_) => SymbolKind::Type,
                    TypeDeclKind::Record(_) => SymbolKind::Record,
                };
                out.push(Symbol {
                    name: decl.name.clone(),
                    span: decl.span.span(),
                    kind,
                });
                if let TypeDeclKind::Sum(variants) = &decl.kind {
                    for variant in variants {
                        out.push(Symbol {
                            name: variant.name.clone(),
                            span: decl.span.span(),
                            kind: SymbolKind::Constructor,
                        });
                    }
                }
            }
            Item::Measure { name, span } => out.push(Symbol {
                name: name.clone(),
                span: span.span(),
                kind: SymbolKind::Measure,
            }),
            // A module's members appear qualified (`Geometry.area`) in the outline
            // and completion, each at its own name span.
            Item::Module { name, items, .. } => {
                for member in items {
                    out.push(Symbol {
                        name: format!("{name}.{}", member.name),
                        span: member.name_span.span(),
                        kind: SymbolKind::Value,
                    });
                }
            }
            Item::Expr(_) => {}
        }
    }
    out
}

/// Collect every resolvable identifier reference in the module (see [`Reference`]).
pub fn references(module: &Module) -> Vec<Reference> {
    walk(module).refs
}

/// Walk the whole module, collecting references and local binder spans.
fn walk(module: &Module) -> Resolver {
    let mut r = Resolver::default();
    for item in &module.items {
        match item {
            Item::Let(binding) => r.walk_binding(binding),
            Item::Expr(expr) => r.walk_expr(expr),
            // Walk each module member's body (params in scope) so locals inside
            // resolve for hover/local navigation.
            Item::Module { items, .. } => {
                for member in items {
                    r.walk_binding(member);
                }
            }
            _ => {}
        }
    }
    r
}

/// Identify the symbol at byte `offset` — whether the cursor sits on a reference,
/// a local binder, or a module-level definition name. Returns the **occurrence
/// span** under the cursor (the identifier the editor highlights) together with the
/// symbol's [`Target`] identity, choosing the narrowest enclosing span so a precise
/// name beats an enclosing declaration.
pub fn symbol_at(module: &Module, offset: usize) -> Option<(Span, Target)> {
    let r = walk(module);
    // Candidate (span, identity) pairs from references, local binders, and
    // module-level definitions; the narrowest span containing the offset wins.
    let mut candidates: Vec<(Span, Target)> = Vec::new();
    for reference in &r.refs {
        candidates.push((reference.span, reference.target.clone()));
    }
    for &span in &r.binders {
        candidates.push((span, Target::Local(span)));
    }
    for sym in definitions(module) {
        candidates.push((sym.span, Target::Module(sym.name)));
    }
    candidates
        .into_iter()
        .filter(|(span, _)| span.start <= offset && offset < span.end)
        .min_by_key(|(span, _)| span.end - span.start)
}

/// Every occurrence of `symbol` in the module: all references to it, plus its
/// declaration(s) when `include_declaration` is set. Spans are de-duplicated and
/// returned in source order.
pub fn find_references(module: &Module, symbol: &Target, include_declaration: bool) -> Vec<Span> {
    let mut spans: Vec<Span> = references(module)
        .into_iter()
        .filter(|r| &r.target == symbol)
        .map(|r| r.span)
        .collect();

    if include_declaration {
        match symbol {
            // A local's declaration is its binder span (the `Target` itself).
            Target::Local(span) => spans.push(*span),
            // A module symbol may have one declaration (a `let`) — or, for a
            // constructor sharing a type's decl span, more than one named match.
            Target::Module(name) => {
                for sym in definitions(module) {
                    if &sym.name == name {
                        spans.push(sym.span);
                    }
                }
            }
        }
    }

    spans.sort_by_key(|s| (s.start, s.end));
    spans.dedup();
    spans
}

#[derive(Default)]
struct Resolver {
    /// A stack of local scopes (innermost last); each maps a bound name to its
    /// binder span.
    scopes: Vec<HashMap<String, Span>>,
    refs: Vec<Reference>,
    /// Every local binder's span (params, block `let`s, pattern vars, CE `let`s),
    /// so find-references can recognize the cursor sitting on a binder itself.
    binders: Vec<Span>,
}

impl Resolver {
    /// The binder span for `name`, if it is locally bound; `None` if it is free
    /// (a module-level reference).
    fn lookup(&self, name: &str) -> Option<Span> {
        self.scopes.iter().rev().find_map(|s| s.get(name).copied())
    }

    /// Push a scope frame, recording its binder spans.
    fn push_scope(&mut self, frame: HashMap<String, Span>) {
        self.binders.extend(frame.values().copied());
        self.scopes.push(frame);
    }

    /// Add one binding to the innermost scope (for the sequential binders of a
    /// block or computation expression), recording its span.
    fn bind_in_scope(&mut self, name: String, span: Span) {
        self.binders.push(span);
        self.scopes.last_mut().unwrap().insert(name, span);
    }

    /// Walk a binding's value with its parameters in scope. The binding's own name
    /// is resolved as a module symbol (so a recursive call jumps to the
    /// definition), hence it is not added to the local scope here.
    fn walk_binding(&mut self, binding: &LetBinding) {
        self.push_scope(param_scope(binding));
        self.walk_expr(&binding.value);
        self.scopes.pop();
    }

    fn walk_expr(&mut self, expr: &Expr) {
        match &expr.kind {
            ExprKind::Int(_)
            | ExprKind::Float(_)
            | ExprKind::Str(_)
            | ExprKind::Bool(_)
            | ExprKind::Unit => {}
            ExprKind::Var(name) => match self.lookup(name) {
                Some(span) => self.refs.push(Reference {
                    span: expr.span(),
                    target: Target::Local(span),
                }),
                None => self.refs.push(Reference {
                    span: expr.span(),
                    target: Target::Module(name.clone()),
                }),
            },
            ExprKind::Fn { params, body } => {
                self.push_scope(
                    params
                        .iter()
                        .map(|p| (p.name.clone(), p.span.span()))
                        .collect(),
                );
                self.walk_expr(body);
                self.scopes.pop();
            }
            ExprKind::App { func, arg } => {
                self.walk_expr(func);
                self.walk_expr(arg);
            }
            ExprKind::If { cond, then, else_ } => {
                self.walk_expr(cond);
                self.walk_expr(then);
                self.walk_expr(else_);
            }
            ExprKind::Match { scrutinee, arms } => {
                self.walk_expr(scrutinee);
                for MatchArm { pattern, body } in arms {
                    let mut bound = HashMap::new();
                    pattern_vars(pattern, &mut bound);
                    self.push_scope(bound);
                    self.walk_expr(body);
                    self.scopes.pop();
                }
            }
            ExprKind::Binary { lhs, rhs, .. } => {
                self.walk_expr(lhs);
                self.walk_expr(rhs);
            }
            ExprKind::Unary { expr, .. } => self.walk_expr(expr),
            ExprKind::Pipe { lhs, rhs } => {
                self.walk_expr(lhs);
                self.walk_expr(rhs);
            }
            ExprKind::Ce { items, .. } => {
                // Each `let!`/`let` binds for the *following* items, so track a
                // single growing scope frame across the block; a reference resolves
                // to the binding's name span.
                self.scopes.push(HashMap::new());
                for item in items {
                    match item {
                        CeItem::LetBang {
                            name,
                            name_span,
                            value,
                        }
                        | CeItem::Let {
                            name,
                            name_span,
                            value,
                        } => {
                            self.walk_expr(value);
                            self.bind_in_scope(name.clone(), name_span.span());
                        }
                        CeItem::DoBang(e)
                        | CeItem::Return(e)
                        | CeItem::ReturnBang(e)
                        | CeItem::Yield(e)
                        | CeItem::YieldBang(e) => self.walk_expr(e),
                    }
                }
                self.scopes.pop();
            }
            ExprKind::Annot { value, .. } => self.walk_expr(value),
            ExprKind::List { elems } => {
                for e in elems {
                    self.walk_expr(e);
                }
            }
            ExprKind::Record { fields } => {
                for f in fields {
                    self.walk_expr(&f.value);
                }
            }
            ExprKind::RecordUpdate { base, fields } => {
                self.walk_expr(base);
                for f in fields {
                    self.walk_expr(&f.value);
                }
            }
            ExprKind::Field { base, .. } => {
                // `Module.member` (built-in modules) is a qualified name, not a
                // reference to a local/definition — don't resolve the module base.
                if crate::types::qualified_name(expr).is_none() {
                    self.walk_expr(base);
                }
            }
            ExprKind::Block { stmts } => {
                self.scopes.push(HashMap::new());
                for stmt in stmts {
                    match stmt {
                        BlockStmt::Let(binding) => {
                            // The value sees the current scope (bindings are not
                            // recursive); the name then binds for later statements,
                            // pointing at its own precise name span.
                            self.push_scope(param_scope(binding));
                            self.walk_expr(&binding.value);
                            self.scopes.pop();
                            self.bind_in_scope(binding.name.clone(), binding.name_span.span());
                        }
                        BlockStmt::Expr(e) => self.walk_expr(e),
                    }
                }
                self.scopes.pop();
            }
            // `target <- value`: the target has no precise span (the span covers
            // the whole assignment), so only the value is walked for references.
            ExprKind::Assign { value, .. } => self.walk_expr(value),
        }
    }
}

/// A scope frame for a binding's parameters (name → its span).
fn param_scope(binding: &LetBinding) -> HashMap<String, Span> {
    binding
        .params
        .iter()
        .map(|p| (p.name.clone(), p.span.span()))
        .collect()
}

/// Collect the variables a pattern binds, mapped to their spans (constructor
/// arguments recurse).
fn pattern_vars(pattern: &Pattern, out: &mut HashMap<String, Span>) {
    match pattern {
        Pattern::Var { name, span } => {
            out.insert(name.clone(), span.span());
        }
        Pattern::Ctor { args, .. } => {
            for arg in args {
                pattern_vars(arg, out);
            }
        }
        Pattern::Record { fields } => {
            for f in fields {
                pattern_vars(&f.pattern, out);
            }
        }
        Pattern::Wildcard | Pattern::Int(_) | Pattern::Bool(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn module(src: &str) -> Module {
        crate::parse(src).expect("parse")
    }

    #[test]
    fn collects_top_level_definitions() {
        let m = module("type Color = Red | Green\nlet x = 1\nextern sqrt : float -> float");
        let defs = definitions(&m);
        let names: Vec<_> = defs.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"Color"));
        assert!(names.contains(&"Red"));
        assert!(names.contains(&"Green"));
        assert!(names.contains(&"x"));
        assert!(names.contains(&"sqrt"));
    }

    #[test]
    fn parameter_reference_resolves_to_the_parameter() {
        // `x` is referenced at the end; it must resolve to the parameter binder,
        // not be treated as a module reference.
        let m = module("let id x = x");
        let refs = references(&m);
        let r = refs.iter().find(|r| matches!(r.target, Target::Local(_)));
        assert!(r.is_some(), "param ref not local: {refs:?}");
    }

    #[test]
    fn module_level_uses_resolve_to_module() {
        let m = module("let one = 1\nlet two = one + one");
        let refs = references(&m);
        let module_refs = refs
            .iter()
            .filter(|r| r.target == Target::Module("one".to_string()))
            .count();
        assert_eq!(module_refs, 2);
    }

    #[test]
    fn inner_binding_shadows_outer_definition() {
        // The module-level `x` is shadowed by the parameter `x`; the reference
        // resolves locally, never to the module symbol.
        let m = module("let x = 1\nlet f x = x");
        let refs = references(&m);
        assert!(refs.iter().all(|r| match &r.target {
            Target::Module(n) => n != "x",
            Target::Local(_) => true,
        }));
        assert!(refs.iter().any(|r| matches!(r.target, Target::Local(_))));
    }

    #[test]
    fn pattern_binding_is_a_local_target() {
        let m = module("let f o = match o with | Some y -> y | None -> 0");
        let refs = references(&m);
        // `y` resolves to its pattern binder.
        assert!(refs.iter().any(|r| matches!(r.target, Target::Local(_))));
    }

    #[test]
    fn find_references_from_the_definition_name() {
        // Clicking the top-level definition `one` finds its two uses + the
        // declaration — the headline find-references case.
        let src = "let one = 1\nlet two = one + one";
        let m = module(src);
        let def_off = src.find("one").unwrap() + 1; // inside the name span
        let (_, sym) = symbol_at(&m, def_off).unwrap();
        assert_eq!(sym, Target::Module("one".to_string()));
        assert_eq!(find_references(&m, &sym, true).len(), 3);
        assert_eq!(find_references(&m, &sym, false).len(), 2);
    }

    #[test]
    fn find_references_on_a_parameter() {
        // From a use of the parameter `x`, find both uses + its declaration.
        let src = "let add x = x + x";
        let m = module(src);
        let (_, sym) = symbol_at(&m, src.rfind('x').unwrap()).unwrap();
        assert!(matches!(sym, Target::Local(_)));
        assert_eq!(find_references(&m, &sym, true).len(), 3);
        assert_eq!(find_references(&m, &sym, false).len(), 2);
    }

    #[test]
    fn symbol_at_recognizes_a_binder_under_the_cursor() {
        // Cursor on the parameter binder itself (not a use) still identifies it.
        let src = "let id x = x";
        let m = module(src);
        let binder_off = src.find(" x ").unwrap() + 1;
        let (span, target) = symbol_at(&m, binder_off).unwrap();
        assert!(matches!(target, Target::Local(_)));
        // The occurrence span is the binder identifier itself.
        assert_eq!((span.start, span.end), (binder_off, binder_off + 1));
    }

    #[test]
    fn computation_expression_binding_is_a_local_target() {
        // `x` bound by `let!` in a `result {}` block; the later reference resolves
        // to its binder, not to a module symbol.
        let m = module("let go = result {\n  let! x = Ok 1\n  return x\n}");
        let refs = references(&m);
        assert!(
            refs.iter().any(|r| matches!(r.target, Target::Local(_))),
            "CE binding ref not local: {refs:?}"
        );
        assert!(refs.iter().all(|r| match &r.target {
            Target::Module(n) => n != "x",
            Target::Local(_) => true,
        }));
    }
}
