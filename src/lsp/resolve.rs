//! Name resolution for editor navigation (`DESIGN.md` §9): a dependency-free walk
//! over the parsed AST that powers go-to-definition and completion. It is
//! independent of the type checker — it only needs spans and names, so it works on
//! any program that *parses*, even one that fails to type-check.
//!
//! Every identifier *reference* resolves to a [`Target`]: either a **local** binder
//! (a function parameter, block-local `let`, or pattern variable — resolved to the
//! binder's own span) or a **module-level** symbol (resolved by name against
//! [`definitions`]). The walk tracks lexical scopes so an inner binding correctly
//! shadows an outer one. Computation-expression `let`/`let!` names are the one
//! local kind without a span yet, so a reference to one is dropped (no jump)
//! rather than mis-resolved to a same-named module symbol.

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
            Item::Expr(_) => {}
        }
    }
    out
}

/// Collect every resolvable identifier reference in the module (see [`Reference`]).
pub fn references(module: &Module) -> Vec<Reference> {
    let mut r = Resolver::default();
    for item in &module.items {
        match item {
            Item::Let(binding) => r.walk_binding(binding),
            Item::Expr(expr) => r.walk_expr(expr),
            _ => {}
        }
    }
    r.refs
}

#[derive(Default)]
struct Resolver {
    /// A stack of local scopes (innermost last); each maps a bound name to its
    /// binder span, or `None` for a binder that has no span yet (CE `let`).
    scopes: Vec<HashMap<String, Option<Span>>>,
    refs: Vec<Reference>,
}

impl Resolver {
    /// The binder span for `name`, if it is locally bound (`Some(None)` = bound but
    /// span-less); `None` if it is free (a module-level reference).
    fn lookup(&self, name: &str) -> Option<Option<Span>> {
        self.scopes.iter().rev().find_map(|s| s.get(name).copied())
    }

    /// Walk a binding's value with its parameters in scope. The binding's own name
    /// is resolved as a module symbol (so a recursive call jumps to the
    /// definition), hence it is not added to the local scope here.
    fn walk_binding(&mut self, binding: &LetBinding) {
        self.scopes.push(param_scope(binding));
        self.walk_expr(&binding.value);
        self.scopes.pop();
    }

    fn walk_expr(&mut self, expr: &Expr) {
        match &expr.kind {
            ExprKind::Int(_) | ExprKind::Float(_) | ExprKind::Str(_) | ExprKind::Bool(_) => {}
            ExprKind::Var(name) => match self.lookup(name) {
                Some(Some(span)) => self.refs.push(Reference {
                    span: expr.span(),
                    target: Target::Local(span),
                }),
                Some(None) => {} // span-less local (CE binding) — no jump target
                None => self.refs.push(Reference {
                    span: expr.span(),
                    target: Target::Module(name.clone()),
                }),
            },
            ExprKind::Fn { params, body } => {
                self.scopes.push(
                    params
                        .iter()
                        .map(|p| (p.name.clone(), Some(p.span.span())))
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
                    self.scopes.push(bound);
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
                // single growing scope frame across the block. CE binders have no
                // span yet, so they shadow with `None` (a reference to one is
                // dropped rather than mis-resolved).
                self.scopes.push(HashMap::new());
                for item in items {
                    match item {
                        CeItem::LetBang { name, value } | CeItem::Let { name, value } => {
                            self.walk_expr(value);
                            self.scopes.last_mut().unwrap().insert(name.clone(), None);
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
            ExprKind::Field { base, .. } => self.walk_expr(base),
            ExprKind::Block { stmts } => {
                self.scopes.push(HashMap::new());
                for stmt in stmts {
                    match stmt {
                        BlockStmt::Let(binding) => {
                            // The value sees the current scope (bindings are not
                            // recursive); the name then binds for later statements,
                            // pointing at its own precise name span.
                            self.scopes.push(param_scope(binding));
                            self.walk_expr(&binding.value);
                            self.scopes.pop();
                            self.scopes
                                .last_mut()
                                .unwrap()
                                .insert(binding.name.clone(), Some(binding.name_span.span()));
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
fn param_scope(binding: &LetBinding) -> HashMap<String, Option<Span>> {
    binding
        .params
        .iter()
        .map(|p| (p.name.clone(), Some(p.span.span())))
        .collect()
}

/// Collect the variables a pattern binds, mapped to their spans (constructor
/// arguments recurse).
fn pattern_vars(pattern: &Pattern, out: &mut HashMap<String, Option<Span>>) {
    match pattern {
        Pattern::Var { name, span } => {
            out.insert(name.clone(), Some(span.span()));
        }
        Pattern::Ctor { args, .. } => {
            for arg in args {
                pattern_vars(arg, out);
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
}
