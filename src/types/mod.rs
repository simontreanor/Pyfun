//! Hindley–Milner type inference (`DESIGN.md` §3, §10).
//!
//! Algorithm W with a substitution map and let-generalization, so top-level
//! bindings are polymorphic (`let id x = x` has type `'a -> 'a` and each use is
//! instantiated). Functions are curried, matching the rest of the language: a
//! definition `let f a b = ...` has type `ta -> tb -> tbody`.
//!
//! Phase 3 scope: types only. Effect, unit, and exhaustiveness checking arrive
//! with the language features they need (effects/units have no surface syntax
//! yet; exhaustiveness is bound up with ADTs). Arithmetic is integer-only for now
//! — there is no numeric type-class machinery, so `+ - * /` are `int -> int ->
//! int` and `/` is integer division (it lowers to Python `//`).

use std::collections::{HashMap, HashSet};

use crate::lexer::Span;
use crate::parser::ast::{Expr, ExprKind, Item, LetBinding, Module, Pattern};

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
}

/// A type scheme: a type generalized over zero or more variables.
#[derive(Debug, Clone)]
struct Scheme {
    vars: Vec<u32>,
    ty: Ty,
}

type Env = HashMap<String, Scheme>;

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

/// Type-check a whole module. Returns every independent error found, so a single
/// bad binding doesn't hide the rest.
pub fn check(module: &Module) -> Result<(), Vec<TypeError>> {
    let mut inf = Infer::default();
    let mut env: Env = Env::new();
    let mut errors = Vec::new();

    for item in &module.items {
        match item {
            Item::Let(binding) => match inf.infer_binding(binding, &env) {
                Ok(scheme) => {
                    env.insert(binding.name.clone(), scheme);
                }
                Err(e) => {
                    errors.push(e);
                    // Bind the name to a fresh type so later references don't
                    // cascade into spurious "unbound" errors.
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

#[derive(Default)]
struct Infer {
    subst: HashMap<u32, Ty>,
    next: u32,
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
            other => other.clone(),
        }
    }

    fn infer_binding(&mut self, binding: &LetBinding, env: &Env) -> Result<Scheme, TypeError> {
        let ty = if binding.params.is_empty() {
            self.infer_expr(&binding.value, env)?
        } else {
            // Parameters are monomorphic inside the body; the function type is
            // the curried arrow over them.
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
                // Integer arithmetic only (see module docs). Expected type first
                // so diagnostics read "expected int, found <actual>".
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
                // The `then` branch sets the expectation for `else`.
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
                Ok(self.apply(&result))
            }
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

    /// Check a pattern against the scrutinee type and record any bindings it
    /// introduces (monomorphic) into `env`.
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
            Pattern::Ctor { .. } => Err(TypeError {
                message:
                    "constructor patterns require ADT declarations, which are not implemented \
                          yet (planned for a later phase)"
                        .to_string(),
                span,
            }),
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

fn occurs(var: u32, ty: &Ty) -> bool {
    match ty {
        Ty::Var(n) => *n == var,
        Ty::Fun(a, b) => occurs(var, a) || occurs(var, b),
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

fn show_into(ty: &Ty, names: &mut HashMap<u32, String>, out: &mut String, paren_fun: bool) {
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
        Ty::Fun(a, b) => {
            if paren_fun {
                out.push('(');
            }
            show_into(a, names, out, true);
            out.push_str(" -> ");
            show_into(b, names, out, false);
            if paren_fun {
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
