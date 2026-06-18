//! Hindley–Milner type inference with algebraic data types (`DESIGN.md` §3, §10).
//!
//! Algorithm W with a substitution map and let-generalization, so top-level
//! bindings are polymorphic (`let id x = x` has type `'a -> 'a` and each use is
//! instantiated). Functions are curried, matching the rest of the language.
//!
//! `type` declarations introduce ADTs: a pre-pass registers each type's arity and
//! gives every constructor a polymorphic scheme (`Some : 'a -> Option 'a`), which
//! is added to the initial environment. Constructor patterns are checked against
//! those schemes, and `match` is checked for **exhaustiveness** (shallow: only the
//! head constructor set is considered; nested patterns rely on the runtime guard).
//!
//! Still deferred until their syntax exists: effect and unit inference. Arithmetic
//! is integer-only (no numeric type classes); `/` is integer division.

use std::collections::{HashMap, HashSet};

use crate::lexer::Span;
use crate::parser::ast::{
    CeBuilder, CeItem, Expr, ExprKind, Item, LetBinding, MatchArm, Module, Pattern, TypeExpr,
};

/// A monomorphic type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ty {
    Int,
    Float,
    Bool,
    Str,
    /// A unification variable.
    Var(u32),
    /// A function `arg -> result` (curried).
    Fun(Box<Ty>, Box<Ty>),
    /// An applied type constructor, e.g. `Option int` = `Con("Option", [Int])`.
    Con(String, Vec<Ty>),
}

/// A type scheme: a type generalized over zero or more variables.
#[derive(Debug, Clone)]
struct Scheme {
    vars: Vec<u32>,
    ty: Ty,
}

type Env = HashMap<String, Scheme>;

const BUILTIN_TYPES: [&str; 4] = ["int", "float", "bool", "string"];

/// A type error, with the source span it should be reported against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeError {
    pub message: String,
    pub span: Span,
}

impl std::fmt::Display for TypeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

/// Information about one data constructor.
#[derive(Debug, Clone)]
struct CtorInfo {
    scheme: Scheme,
    arity: usize,
}

/// The result of the type-declaration pre-pass.
#[derive(Default)]
struct Decls {
    /// Type constructor → number of type parameters (`Option` → 1).
    type_arity: HashMap<String, usize>,
    /// Data constructor → its scheme and field count (`Some` → `'a -> Option 'a`).
    ctors: HashMap<String, CtorInfo>,
    /// Type name → its constructor names, in declaration order.
    type_ctors: HashMap<String, Vec<String>>,
}

/// Type-check a whole module. Returns every independent error found, so a single
/// bad binding doesn't hide the rest.
pub fn check(module: &Module) -> Result<(), Vec<TypeError>> {
    let mut errors = Vec::new();
    let (decls, ctor_env) = build_decls(module, &mut errors);

    let mut inf = Infer {
        decls,
        ..Infer::default()
    };
    let mut env = ctor_env;

    for item in &module.items {
        match item {
            Item::Type(_) => {} // handled by the pre-pass
            Item::Let(binding) => match inf.infer_binding(binding, &env) {
                Ok(scheme) => {
                    env.insert(binding.name.clone(), scheme);
                }
                Err(e) => {
                    errors.push(e);
                    let ty = inf.fresh();
                    env.insert(binding.name.clone(), Scheme { vars: vec![], ty });
                }
            },
            Item::Expr(expr) => {
                if let Err(e) = inf.infer_expr(expr, &env) {
                    errors.push(e);
                }
            }
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// Register every `type` declaration and build the constructor environment.
fn build_decls(module: &Module, errors: &mut Vec<TypeError>) -> (Decls, Env) {
    let mut decls = Decls::default();
    let mut env = Env::new();
    seed_builtin_types(&mut decls, &mut env);

    // Pass 1: type names and arities (so fields can reference any type, including
    // self/forward references).
    for item in &module.items {
        if let Item::Type(decl) = item {
            let span = decl.span.span();
            if BUILTIN_TYPES.contains(&decl.name.as_str()) {
                errors.push(TypeError {
                    message: format!("cannot redefine builtin type `{}`", decl.name),
                    span,
                });
            } else if decls.type_arity.contains_key(&decl.name) {
                errors.push(TypeError {
                    message: format!("type `{}` is already defined", decl.name),
                    span,
                });
            } else {
                decls
                    .type_arity
                    .insert(decl.name.clone(), decl.params.len());
                decls.type_ctors.insert(decl.name.clone(), Vec::new());
            }
        }
    }

    // Pass 2: constructor schemes.
    for item in &module.items {
        let Item::Type(decl) = item else { continue };
        let span = decl.span.span();
        let param_map: HashMap<String, u32> = decl
            .params
            .iter()
            .enumerate()
            .map(|(i, p)| (p.clone(), i as u32))
            .collect();
        let result_ty = Ty::Con(
            decl.name.clone(),
            (0..decl.params.len() as u32).map(Ty::Var).collect(),
        );

        for variant in &decl.variants {
            if decls.ctors.contains_key(&variant.name) {
                errors.push(TypeError {
                    message: format!("constructor `{}` is already defined", variant.name),
                    span,
                });
                continue;
            }
            let mut field_tys = Vec::with_capacity(variant.fields.len());
            let mut ok = true;
            for field in &variant.fields {
                match resolve(field, &param_map, &decls.type_arity, span) {
                    Ok(t) => field_tys.push(t),
                    Err(e) => {
                        errors.push(e);
                        ok = false;
                    }
                }
            }
            if !ok {
                continue;
            }
            let arity = field_tys.len();
            let ctor_ty = field_tys
                .into_iter()
                .rev()
                .fold(result_ty.clone(), |acc, f| {
                    Ty::Fun(Box::new(f), Box::new(acc))
                });
            let scheme = Scheme {
                vars: (0..decl.params.len() as u32).collect(),
                ty: ctor_ty,
            };
            env.insert(variant.name.clone(), scheme.clone());
            decls
                .ctors
                .insert(variant.name.clone(), CtorInfo { scheme, arity });
            if let Some(list) = decls.type_ctors.get_mut(&decl.name) {
                list.push(variant.name.clone());
            }
        }
    }

    (decls, env)
}

/// Seed the registry with the built-in computation-expression types: `Async a`,
/// `Seq a`, and `Result a e` with constructors `Ok`/`Error` (`DESIGN.md` §8.1).
fn seed_builtin_types(decls: &mut Decls, env: &mut Env) {
    decls.type_arity.insert("Async".to_string(), 1);
    decls.type_arity.insert("Seq".to_string(), 1);
    decls.type_arity.insert("Result".to_string(), 2);
    decls.type_ctors.insert("Async".to_string(), Vec::new());
    decls.type_ctors.insert("Seq".to_string(), Vec::new());
    decls.type_ctors.insert(
        "Result".to_string(),
        vec!["Ok".to_string(), "Error".to_string()],
    );

    // Ok : 'a -> Result 'a 'e   and   Error : 'e -> Result 'a 'e
    let result_ty = Ty::Con("Result".to_string(), vec![Ty::Var(0), Ty::Var(1)]);
    let ok = Scheme {
        vars: vec![0, 1],
        ty: Ty::Fun(Box::new(Ty::Var(0)), Box::new(result_ty.clone())),
    };
    let err = Scheme {
        vars: vec![0, 1],
        ty: Ty::Fun(Box::new(Ty::Var(1)), Box::new(result_ty)),
    };
    env.insert("Ok".to_string(), ok.clone());
    env.insert("Error".to_string(), err.clone());
    decls.ctors.insert(
        "Ok".to_string(),
        CtorInfo {
            scheme: ok,
            arity: 1,
        },
    );
    decls.ctors.insert(
        "Error".to_string(),
        CtorInfo {
            scheme: err,
            arity: 1,
        },
    );
}

/// Resolve a surface type expression into a [`Ty`], given the declaration's type
/// parameters and the set of known type constructors.
fn resolve(
    ty: &TypeExpr,
    params: &HashMap<String, u32>,
    type_arity: &HashMap<String, usize>,
    span: Span,
) -> Result<Ty, TypeError> {
    match ty {
        TypeExpr::Fun(a, b) => Ok(Ty::Fun(
            Box::new(resolve(a, params, type_arity, span)?),
            Box::new(resolve(b, params, type_arity, span)?),
        )),
        TypeExpr::Con(name, args) => {
            let no_args = |t: Ty| -> Result<Ty, TypeError> {
                if args.is_empty() {
                    Ok(t)
                } else {
                    Err(TypeError {
                        message: format!("type `{name}` does not take arguments"),
                        span,
                    })
                }
            };
            if let Some(&v) = params.get(name) {
                return no_args(Ty::Var(v));
            }
            match name.as_str() {
                "int" => return no_args(Ty::Int),
                "float" => return no_args(Ty::Float),
                "bool" => return no_args(Ty::Bool),
                "string" => return no_args(Ty::Str),
                _ => {}
            }
            match type_arity.get(name) {
                Some(&arity) if arity == args.len() => {
                    let resolved: Result<Vec<Ty>, TypeError> = args
                        .iter()
                        .map(|a| resolve(a, params, type_arity, span))
                        .collect();
                    Ok(Ty::Con(name.clone(), resolved?))
                }
                Some(&arity) => Err(TypeError {
                    message: format!(
                        "type `{name}` expects {arity} argument(s), found {}",
                        args.len()
                    ),
                    span,
                }),
                None => Err(TypeError {
                    message: format!("unknown type `{name}`"),
                    span,
                }),
            }
        }
    }
}

#[derive(Default)]
struct Infer {
    subst: HashMap<u32, Ty>,
    next: u32,
    decls: Decls,
}

impl Infer {
    fn fresh(&mut self) -> Ty {
        let id = self.next;
        self.next += 1;
        Ty::Var(id)
    }

    /// Resolve a type through the current substitution.
    fn apply(&self, ty: &Ty) -> Ty {
        match ty {
            Ty::Var(n) => match self.subst.get(n) {
                Some(t) => self.apply(&t.clone()),
                None => Ty::Var(*n),
            },
            Ty::Fun(a, b) => Ty::Fun(Box::new(self.apply(a)), Box::new(self.apply(b))),
            Ty::Con(name, args) => {
                Ty::Con(name.clone(), args.iter().map(|a| self.apply(a)).collect())
            }
            other => other.clone(),
        }
    }

    fn infer_binding(&mut self, binding: &LetBinding, env: &Env) -> Result<Scheme, TypeError> {
        let ty = if binding.params.is_empty() {
            self.infer_expr(&binding.value, env)?
        } else {
            let mut body_env = env.clone();
            let mut param_tys = Vec::with_capacity(binding.params.len());
            for param in &binding.params {
                let pty = self.fresh();
                param_tys.push(pty.clone());
                body_env.insert(
                    param.clone(),
                    Scheme {
                        vars: vec![],
                        ty: pty,
                    },
                );
            }
            let body_ty = self.infer_expr(&binding.value, &body_env)?;
            param_tys
                .into_iter()
                .rev()
                .fold(body_ty, |acc, p| Ty::Fun(Box::new(p), Box::new(acc)))
        };
        Ok(self.generalize(env, &ty))
    }

    fn infer_expr(&mut self, expr: &Expr, env: &Env) -> Result<Ty, TypeError> {
        let span = expr.span();
        match &expr.kind {
            ExprKind::Int(_) => Ok(Ty::Int),
            ExprKind::Float(_) => Ok(Ty::Float),
            ExprKind::Str(_) => Ok(Ty::Str),
            ExprKind::Bool(_) => Ok(Ty::Bool),

            ExprKind::Var(name) => match env.get(name) {
                Some(scheme) => Ok(self.instantiate(scheme)),
                None => Err(TypeError {
                    message: format!("unbound name `{name}`"),
                    span,
                }),
            },

            ExprKind::Binary { lhs, rhs, .. } => {
                // Integer arithmetic only. Expected type first so diagnostics read
                // "expected int, found <actual>".
                let lt = self.infer_expr(lhs, env)?;
                self.unify(&Ty::Int, &lt, lhs.span())?;
                let rt = self.infer_expr(rhs, env)?;
                self.unify(&Ty::Int, &rt, rhs.span())?;
                Ok(Ty::Int)
            }

            ExprKind::If { cond, then, else_ } => {
                let ct = self.infer_expr(cond, env)?;
                self.unify(&Ty::Bool, &ct, cond.span())?;
                let tt = self.infer_expr(then, env)?;
                let et = self.infer_expr(else_, env)?;
                self.unify(&tt, &et, else_.span())?;
                Ok(self.apply(&tt))
            }

            ExprKind::Fn { params, body } => {
                let mut body_env = env.clone();
                let mut param_tys = Vec::with_capacity(params.len());
                for param in params {
                    let pty = self.fresh();
                    param_tys.push(pty.clone());
                    body_env.insert(
                        param.clone(),
                        Scheme {
                            vars: vec![],
                            ty: pty,
                        },
                    );
                }
                let body_ty = self.infer_expr(body, &body_env)?;
                Ok(param_tys
                    .into_iter()
                    .rev()
                    .fold(body_ty, |acc, p| Ty::Fun(Box::new(p), Box::new(acc))))
            }

            ExprKind::App { func, arg } => self.infer_apply(func, arg, span, env),

            // `lhs |> rhs` == `rhs lhs`.
            ExprKind::Pipe { lhs, rhs } => self.infer_apply(rhs, lhs, span, env),

            ExprKind::Match { scrutinee, arms } => {
                let scrut_ty = self.infer_expr(scrutinee, env)?;
                let result = self.fresh();
                for arm in arms {
                    let mut arm_env = env.clone();
                    self.bind_pattern(&arm.pattern, &scrut_ty, scrutinee.span(), &mut arm_env)?;
                    let body_ty = self.infer_expr(&arm.body, &arm_env)?;
                    self.unify(&result, &body_ty, arm.body.span())?;
                }
                self.check_exhaustive(&scrut_ty, arms, span)?;
                Ok(self.apply(&result))
            }

            ExprKind::Ce { builder, items } => self.infer_ce(*builder, items, span, env),
        }
    }

    fn infer_ce(
        &mut self,
        builder: CeBuilder,
        items: &[CeItem],
        span: Span,
        env: &Env,
    ) -> Result<Ty, TypeError> {
        match builder {
            CeBuilder::Seq => self.infer_seq(items, span, env),
            CeBuilder::Result => {
                self.infer_monad(items, span, env, "Result", /* binary */ true)
            }
            CeBuilder::Async => {
                self.infer_monad(items, span, env, "Async", /* binary */ false)
            }
        }
    }

    fn infer_seq(&mut self, items: &[CeItem], span: Span, env: &Env) -> Result<Ty, TypeError> {
        let elem = self.fresh();
        let mut env = env.clone();
        for item in items {
            match item {
                CeItem::Yield(e) => {
                    let t = self.infer_expr(e, &env)?;
                    self.unify(&elem, &t, e.span())?;
                }
                CeItem::YieldBang(e) => {
                    let t = self.infer_expr(e, &env)?;
                    self.unify(
                        &Ty::Con("Seq".to_string(), vec![elem.clone()]),
                        &t,
                        e.span(),
                    )?;
                }
                CeItem::Let { name, value } => {
                    let t = self.infer_expr(value, &env)?;
                    env.insert(
                        name.clone(),
                        Scheme {
                            vars: vec![],
                            ty: self.apply(&t),
                        },
                    );
                }
                _ => {
                    return Err(TypeError {
                        message: "only `yield`, `yield!`, and `let` are allowed in a `seq` block"
                            .to_string(),
                        span,
                    });
                }
            }
        }
        Ok(Ty::Con("Seq".to_string(), vec![self.apply(&elem)]))
    }

    /// Shared inference for the `result` and `async` monads. `binary` selects the
    /// two-parameter `Result a e` (with a shared error type) versus `Async a`.
    fn infer_monad(
        &mut self,
        items: &[CeItem],
        span: Span,
        env: &Env,
        con: &str,
        binary: bool,
    ) -> Result<Ty, TypeError> {
        let err = self.fresh(); // the shared error type (unused for Async)
        let monad = |inner: Ty, this: &Self| {
            if binary {
                Ty::Con(con.to_string(), vec![inner, this.apply(&err)])
            } else {
                Ty::Con(con.to_string(), vec![inner])
            }
        };
        let mut env = env.clone();
        let mut result_val: Option<Ty> = None;

        for (i, item) in items.iter().enumerate() {
            let is_last = i + 1 == items.len();
            match item {
                CeItem::LetBang { name, value } => {
                    let t = self.infer_expr(value, &env)?;
                    let inner = self.fresh();
                    let expected = monad(inner.clone(), self);
                    self.unify(&expected, &t, value.span())?;
                    env.insert(
                        name.clone(),
                        Scheme {
                            vars: vec![],
                            ty: self.apply(&inner),
                        },
                    );
                }
                CeItem::Let { name, value } => {
                    let t = self.infer_expr(value, &env)?;
                    env.insert(
                        name.clone(),
                        Scheme {
                            vars: vec![],
                            ty: self.apply(&t),
                        },
                    );
                }
                CeItem::DoBang(e) => {
                    let t = self.infer_expr(e, &env)?;
                    let inner = self.fresh();
                    let expected = monad(inner, self);
                    self.unify(&expected, &t, e.span())?;
                }
                CeItem::Return(e) => {
                    if !is_last {
                        return Err(TypeError {
                            message: "`return` must be the final item".to_string(),
                            span,
                        });
                    }
                    result_val = Some(self.infer_expr(e, &env)?);
                }
                CeItem::ReturnBang(e) => {
                    if !is_last {
                        return Err(TypeError {
                            message: "`return!` must be the final item".to_string(),
                            span,
                        });
                    }
                    let t = self.infer_expr(e, &env)?;
                    let inner = self.fresh();
                    let expected = monad(inner.clone(), self);
                    self.unify(&expected, &t, e.span())?;
                    result_val = Some(self.apply(&inner));
                }
                CeItem::Yield(_) | CeItem::YieldBang(_) => {
                    return Err(TypeError {
                        message: format!(
                            "`yield` is not allowed in a `{}` block",
                            con.to_lowercase()
                        ),
                        span,
                    });
                }
            }
        }

        match result_val {
            Some(inner) => Ok(monad(self.apply(&inner), self)),
            None => Err(TypeError {
                message: format!("a `{}` block must end with `return`", con.to_lowercase()),
                span,
            }),
        }
    }

    fn infer_apply(
        &mut self,
        func: &Expr,
        arg: &Expr,
        span: Span,
        env: &Env,
    ) -> Result<Ty, TypeError> {
        let func_ty = self.infer_expr(func, env)?;
        let arg_ty = self.infer_expr(arg, env)?;
        let result = self.fresh();
        let expected = Ty::Fun(Box::new(arg_ty), Box::new(result.clone()));
        self.unify(&func_ty, &expected, span)?;
        Ok(self.apply(&result))
    }

    /// Check a pattern against the scrutinee type, recording bindings into `env`.
    fn bind_pattern(
        &mut self,
        pattern: &Pattern,
        scrut_ty: &Ty,
        span: Span,
        env: &mut Env,
    ) -> Result<(), TypeError> {
        match pattern {
            Pattern::Wildcard => Ok(()),
            Pattern::Var(name) => {
                env.insert(
                    name.clone(),
                    Scheme {
                        vars: vec![],
                        ty: scrut_ty.clone(),
                    },
                );
                Ok(())
            }
            Pattern::Int(_) => self.unify(scrut_ty, &Ty::Int, span),
            Pattern::Bool(_) => self.unify(scrut_ty, &Ty::Bool, span),
            Pattern::Ctor { name, args } => {
                let Some(info) = self.decls.ctors.get(name).cloned() else {
                    return Err(TypeError {
                        message: format!("unknown constructor `{name}`"),
                        span,
                    });
                };
                if args.len() != info.arity {
                    return Err(TypeError {
                        message: format!(
                            "constructor `{name}` expects {} argument(s), found {}",
                            info.arity,
                            args.len()
                        ),
                        span,
                    });
                }
                let cty = self.instantiate(&info.scheme);
                let (field_tys, result_ty) = split_fun(&cty, info.arity);
                self.unify(&result_ty, scrut_ty, span)?;
                for (sub, field_ty) in args.iter().zip(field_tys) {
                    self.bind_pattern(sub, &field_ty, span, env)?;
                }
                Ok(())
            }
        }
    }

    /// Shallow exhaustiveness: an irrefutable arm covers everything; otherwise the
    /// head constructor set must be complete for the scrutinee's type.
    fn check_exhaustive(
        &self,
        scrut_ty: &Ty,
        arms: &[MatchArm],
        span: Span,
    ) -> Result<(), TypeError> {
        if arms
            .iter()
            .any(|a| matches!(a.pattern, Pattern::Wildcard | Pattern::Var(_)))
        {
            return Ok(());
        }
        let missing = match self.apply(scrut_ty) {
            Ty::Bool => {
                let mut has_true = false;
                let mut has_false = false;
                for arm in arms {
                    if let Pattern::Bool(b) = arm.pattern {
                        if b {
                            has_true = true;
                        } else {
                            has_false = true;
                        }
                    }
                }
                let mut miss = Vec::new();
                if !has_true {
                    miss.push("true".to_string());
                }
                if !has_false {
                    miss.push("false".to_string());
                }
                miss
            }
            Ty::Con(name, _) if self.decls.type_ctors.contains_key(&name) => {
                let covered: HashSet<&str> = arms
                    .iter()
                    .filter_map(|a| match &a.pattern {
                        Pattern::Ctor { name, .. } => Some(name.as_str()),
                        _ => None,
                    })
                    .collect();
                self.decls.type_ctors[&name]
                    .iter()
                    .filter(|c| !covered.contains(c.as_str()))
                    .cloned()
                    .collect()
            }
            // int/float/string/function/type-variable scrutinees can't be
            // enumerated — they need a wildcard.
            _ => {
                return Err(TypeError {
                    message: "non-exhaustive match: add a wildcard `_` arm".to_string(),
                    span,
                });
            }
        };
        if missing.is_empty() {
            Ok(())
        } else {
            let names = missing
                .iter()
                .map(|m| format!("`{m}`"))
                .collect::<Vec<_>>()
                .join(", ");
            Err(TypeError {
                message: format!("non-exhaustive match: missing {names}"),
                span,
            })
        }
    }

    fn unify(&mut self, a: &Ty, b: &Ty, span: Span) -> Result<(), TypeError> {
        let a = self.apply(a);
        let b = self.apply(b);
        match (&a, &b) {
            (Ty::Int, Ty::Int)
            | (Ty::Float, Ty::Float)
            | (Ty::Bool, Ty::Bool)
            | (Ty::Str, Ty::Str) => Ok(()),
            (Ty::Var(x), Ty::Var(y)) if x == y => Ok(()),
            (Ty::Var(x), t) | (t, Ty::Var(x)) => {
                if occurs(*x, t) {
                    return Err(TypeError {
                        message: format!(
                            "cannot construct the infinite type {} = {}",
                            show(&a),
                            show(&b)
                        ),
                        span,
                    });
                }
                self.subst.insert(*x, t.clone());
                Ok(())
            }
            (Ty::Fun(a1, a2), Ty::Fun(b1, b2)) => {
                self.unify(a1, b1, span)?;
                self.unify(a2, b2, span)
            }
            (Ty::Con(n1, a1), Ty::Con(n2, a2)) if n1 == n2 && a1.len() == a2.len() => {
                for (x, y) in a1.iter().zip(a2) {
                    self.unify(x, y, span)?;
                }
                Ok(())
            }
            _ => Err(TypeError {
                message: format!("type mismatch: expected {}, found {}", show(&a), show(&b)),
                span,
            }),
        }
    }

    fn instantiate(&mut self, scheme: &Scheme) -> Ty {
        let mapping: HashMap<u32, Ty> = scheme.vars.iter().map(|v| (*v, self.fresh())).collect();
        subst_vars(&scheme.ty, &mapping)
    }

    fn generalize(&self, env: &Env, ty: &Ty) -> Scheme {
        let ty = self.apply(ty);
        let env_free = self.env_free_vars(env);
        let mut vars: Vec<u32> = Vec::new();
        free_vars(&ty, &mut |v| {
            if !env_free.contains(&v) && !vars.contains(&v) {
                vars.push(v);
            }
        });
        Scheme { vars, ty }
    }

    fn env_free_vars(&self, env: &Env) -> HashSet<u32> {
        let mut free = HashSet::new();
        for scheme in env.values() {
            let bound: HashSet<u32> = scheme.vars.iter().copied().collect();
            let applied = self.apply(&scheme.ty);
            free_vars(&applied, &mut |v| {
                if !bound.contains(&v) {
                    free.insert(v);
                }
            });
        }
        free
    }
}

/// Split a constructor type into its `n` field types and its result type.
fn split_fun(ty: &Ty, n: usize) -> (Vec<Ty>, Ty) {
    let mut fields = Vec::with_capacity(n);
    let mut cur = ty.clone();
    for _ in 0..n {
        match cur {
            Ty::Fun(a, b) => {
                fields.push(*a);
                cur = *b;
            }
            other => {
                cur = other;
                break;
            }
        }
    }
    (fields, cur)
}

fn occurs(var: u32, ty: &Ty) -> bool {
    match ty {
        Ty::Var(n) => *n == var,
        Ty::Fun(a, b) => occurs(var, a) || occurs(var, b),
        Ty::Con(_, args) => args.iter().any(|a| occurs(var, a)),
        _ => false,
    }
}

fn free_vars(ty: &Ty, f: &mut impl FnMut(u32)) {
    match ty {
        Ty::Var(n) => f(*n),
        Ty::Fun(a, b) => {
            free_vars(a, f);
            free_vars(b, f);
        }
        Ty::Con(_, args) => args.iter().for_each(|a| free_vars(a, f)),
        _ => {}
    }
}

fn subst_vars(ty: &Ty, mapping: &HashMap<u32, Ty>) -> Ty {
    match ty {
        Ty::Var(n) => mapping.get(n).cloned().unwrap_or(Ty::Var(*n)),
        Ty::Fun(a, b) => Ty::Fun(
            Box::new(subst_vars(a, mapping)),
            Box::new(subst_vars(b, mapping)),
        ),
        Ty::Con(name, args) => Ty::Con(
            name.clone(),
            args.iter().map(|a| subst_vars(a, mapping)).collect(),
        ),
        other => other.clone(),
    }
}

/// Render a type for diagnostics, naming variables `'a`, `'b`, … in order.
fn show(ty: &Ty) -> String {
    let mut names = HashMap::new();
    let mut buf = String::new();
    show_into(ty, &mut names, &mut buf, false);
    buf
}

/// `atom` requests a form that can sit as a single argument — compound types
/// (functions, applied constructors) get parenthesized.
fn show_into(ty: &Ty, names: &mut HashMap<u32, String>, out: &mut String, atom: bool) {
    match ty {
        Ty::Int => out.push_str("int"),
        Ty::Float => out.push_str("float"),
        Ty::Bool => out.push_str("bool"),
        Ty::Str => out.push_str("string"),
        Ty::Var(n) => {
            let next = names.len();
            let name = names.entry(*n).or_insert_with(|| var_name(next));
            out.push_str(name);
        }
        Ty::Con(name, args) if args.is_empty() => out.push_str(name),
        Ty::Con(name, args) => {
            if atom {
                out.push('(');
            }
            out.push_str(name);
            for arg in args {
                out.push(' ');
                show_into(arg, names, out, true);
            }
            if atom {
                out.push(')');
            }
        }
        Ty::Fun(a, b) => {
            if atom {
                out.push('(');
            }
            show_into(a, names, out, true);
            out.push_str(" -> ");
            show_into(b, names, out, false);
            if atom {
                out.push(')');
            }
        }
    }
}

fn var_name(index: usize) -> String {
    // 0 -> 'a, 1 -> 'b, … 26 -> 'a1, etc.
    let letter = (b'a' + (index % 26) as u8) as char;
    if index < 26 {
        format!("'{letter}")
    } else {
        format!("'{letter}{}", index / 26)
    }
}
