//! Name resolution for editor navigation (`DESIGN.md` §9): a dependency-free walk
//! over the parsed AST that powers go-to-definition and completion. It is
//! independent of the type checker — it only needs spans and names, so it works on
//! any program that *parses*, even one that fails to type-check.
//!
//! Scope is **module-level symbols** (top-level `let`s, constructors, type/record
//! declarations, `extern`s, measures). Locals — function parameters, block-local
//! `let`s, and pattern bindings — have no source spans yet, so we cannot point at
//! them; instead [`references`] tracks lexical scopes and *skips* any identifier
//! that a local binder shadows. The honest result: a reference to a module symbol
//! resolves, a reference to a local resolves to nothing (rather than wrongly
//! jumping to a same-named module symbol).

use std::collections::HashSet;

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

/// Collect every identifier *reference* (a `Var` occurrence) that is **not** bound
/// by an enclosing local binder, paired with its span. References to locals/params
/// are deliberately omitted (we cannot resolve them yet — see the module docs).
pub fn references(module: &Module) -> Vec<(Span, String)> {
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
    /// A stack of locally-bound name sets (innermost last).
    scopes: Vec<HashSet<String>>,
    refs: Vec<(Span, String)>,
}

impl Resolver {
    fn bound(&self, name: &str) -> bool {
        self.scopes.iter().any(|s| s.contains(name))
    }

    /// Walk a binding's value with its parameters in scope. The binding's own name
    /// is resolved as a module symbol (so a recursive call jumps to the
    /// definition), hence it is not added to the local scope here.
    fn walk_binding(&mut self, binding: &LetBinding) {
        self.scopes.push(binding.params.iter().cloned().collect());
        self.walk_expr(&binding.value);
        self.scopes.pop();
    }

    fn walk_expr(&mut self, expr: &Expr) {
        match &expr.kind {
            ExprKind::Int(_) | ExprKind::Float(_) | ExprKind::Str(_) | ExprKind::Bool(_) => {}
            ExprKind::Var(name) => {
                if !self.bound(name) {
                    self.refs.push((expr.span(), name.clone()));
                }
            }
            ExprKind::Fn { params, body } => {
                self.scopes.push(params.iter().cloned().collect());
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
                    let mut bound = HashSet::new();
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
                // single growing scope frame across the block.
                self.scopes.push(HashSet::new());
                for item in items {
                    match item {
                        CeItem::LetBang { name, value } | CeItem::Let { name, value } => {
                            self.walk_expr(value);
                            self.scopes.last_mut().unwrap().insert(name.clone());
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
                self.scopes.push(HashSet::new());
                for stmt in stmts {
                    match stmt {
                        BlockStmt::Let(binding) => {
                            // The value sees the current scope (bindings are not
                            // recursive); the name then binds for later statements.
                            self.scopes.push(binding.params.iter().cloned().collect());
                            self.walk_expr(&binding.value);
                            self.scopes.pop();
                            self.scopes.last_mut().unwrap().insert(binding.name.clone());
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

/// Collect the variable names a pattern binds (constructor arguments recurse).
fn pattern_vars(pattern: &Pattern, out: &mut HashSet<String>) {
    match pattern {
        Pattern::Var(name) => {
            out.insert(name.clone());
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
    fn references_skip_locally_bound_names() {
        // `x` the parameter shadows nothing module-level here; the reference to
        // `x` inside must NOT be reported as a module reference.
        let m = module("let id x = x");
        let refs = references(&m);
        assert!(
            !refs.iter().any(|(_, n)| n == "x"),
            "param ref leaked: {refs:?}"
        );
    }

    #[test]
    fn references_report_module_level_uses() {
        let m = module("let one = 1\nlet two = one + one");
        let refs = references(&m);
        assert_eq!(refs.iter().filter(|(_, n)| n == "one").count(), 2);
    }

    #[test]
    fn recursive_self_reference_is_a_module_reference() {
        // `go` is not a parameter, so a call to it resolves to the definition.
        let m = module("let go n = go n");
        let refs = references(&m);
        assert!(refs.iter().any(|(_, n)| n == "go"));
        assert!(!refs.iter().any(|(_, n)| n == "n"));
    }
}
