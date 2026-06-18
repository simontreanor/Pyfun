//! Lowering: Pyfun AST → Python-AST IR (`DESIGN.md` §5).
//!
//! Two things make this more than a 1:1 translation:
//!
//! 1. **Expression → statement bridging.** Pyfun is expression-oriented; Python
//!    is statement-oriented. Function bodies are lowered in *return position*
//!    (so `if`/`match` become clean Python statements), while sub-expressions are
//!    lowered in *value position*, hoisting statements before the value when a
//!    construct (a `match`, or an `if` whose arms need statements) can't be a
//!    single Python expression.
//!
//! 2. **Curried-in-types, n-ary-in-output.** Application spines are flattened and
//!    emitted as direct n-ary calls when the callee's arity is known; genuine
//!    partial application becomes `functools.partial`; over-application applies
//!    the remainder one argument at a time.
//!
//! Phase 2 has no type checker, so arity is taken from a module-level table of
//! top-level function definitions (and from `fun` literals applied in place).
//! When the callee's arity is unknown (a parameter, or an imported Python name)
//! the call is emitted n-ary as-is — correct for full application and for Python
//! interop, but it cannot synthesize a partial application for an unknown callee.
//! The type checker (Phase 3) will make arity precise.

use std::collections::HashSet;

use crate::parser::ast::{BinOp, Expr, Item, LetBinding, Module, Pattern};
use crate::python_emitter::{PyBinOp, PyCase, PyExpr, PyModule, PyPattern, PyStmt};

/// An error raised while lowering (e.g. a construct not yet supported).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LowerError {
    pub message: String,
}

impl std::fmt::Display for LowerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

/// Lower a whole module to a Python module.
pub fn lower(module: &Module) -> Result<PyModule, LowerError> {
    let mut lowerer = Lowerer::new(module);
    lowerer.lower_module(module)
}

struct Lowerer {
    /// Arity of each top-level function (params > 0), used to decide full vs
    /// partial application.
    arities: std::collections::HashMap<String, usize>,
    tmp_counter: usize,
    fn_counter: usize,
    needs_functools: bool,
}

type Lowered = Result<(Vec<PyStmt>, PyExpr), LowerError>;

impl Lowerer {
    fn new(module: &Module) -> Self {
        let mut arities = std::collections::HashMap::new();
        for item in &module.items {
            if let Item::Let(binding) = item {
                // A binding's callable arity is the number of parameters of the
                // Python def/lambda it lowers to: its own `let` parameters, or —
                // if it's a bare `let name = fun ... -> ...` — the lambda's. Extra
                // arguments are handled as over-application at the call site.
                let arity = if !binding.params.is_empty() {
                    Some(binding.params.len())
                } else if let Expr::Fn { params, .. } = &binding.value {
                    Some(params.len())
                } else {
                    None
                };
                if let Some(k) = arity {
                    arities.insert(binding.name.clone(), k);
                }
            }
        }
        Lowerer {
            arities,
            tmp_counter: 0,
            fn_counter: 0,
            needs_functools: false,
        }
    }

    fn lower_module(&mut self, module: &Module) -> Result<PyModule, LowerError> {
        let mut body = Vec::new();
        for item in &module.items {
            match item {
                Item::Let(binding) => self.lower_let(binding, &mut body)?,
                Item::Expr(expr) => {
                    let (mut stmts, value) = self.lower_value(expr, &HashSet::new())?;
                    body.append(&mut stmts);
                    body.push(PyStmt::Expr(value));
                }
            }
        }
        if self.needs_functools {
            body.insert(0, PyStmt::Import("functools".to_string()));
        }
        Ok(PyModule { body })
    }

    fn lower_let(&mut self, binding: &LetBinding, out: &mut Vec<PyStmt>) -> Result<(), LowerError> {
        if binding.params.is_empty() {
            let (mut stmts, value) = self.lower_value(&binding.value, &HashSet::new())?;
            out.append(&mut stmts);
            out.push(PyStmt::Assign {
                target: binding.name.clone(),
                value,
            });
        } else {
            let locals: HashSet<String> = binding.params.iter().cloned().collect();
            let body = self.lower_return(&binding.value, &locals)?;
            out.push(PyStmt::FuncDef {
                name: binding.name.clone(),
                params: binding.params.clone(),
                body,
            });
        }
        Ok(())
    }

    /// Lower `expr` in tail position, producing statements that end by returning
    /// the value. `if`/`match` become native Python statements here.
    fn lower_return(
        &mut self,
        expr: &Expr,
        locals: &HashSet<String>,
    ) -> Result<Vec<PyStmt>, LowerError> {
        match expr {
            Expr::If { cond, then, else_ } => {
                let (mut stmts, test) = self.lower_value(cond, locals)?;
                let body = self.lower_return(then, locals)?;
                let orelse = self.lower_return(else_, locals)?;
                stmts.push(PyStmt::If { test, body, orelse });
                Ok(stmts)
            }
            Expr::Match { scrutinee, arms } => {
                let (mut stmts, subject) = self.lower_value(scrutinee, locals)?;
                let mut cases = Vec::new();
                for arm in arms {
                    let pattern = lower_pattern(&arm.pattern)?;
                    let arm_locals = extend(locals, &pattern_bindings(&arm.pattern));
                    let body = self.lower_return(&arm.body, &arm_locals)?;
                    cases.push(PyCase { pattern, body });
                }
                if !has_catch_all(arms) {
                    cases.push(non_exhaustive_guard());
                }
                stmts.push(PyStmt::Match { subject, cases });
                Ok(stmts)
            }
            _ => {
                let (mut stmts, value) = self.lower_value(expr, locals)?;
                stmts.push(PyStmt::Return(value));
                Ok(stmts)
            }
        }
    }

    /// Lower `expr` in value position: a list of statements to run first, plus a
    /// Python expression denoting the value.
    fn lower_value(&mut self, expr: &Expr, locals: &HashSet<String>) -> Lowered {
        match expr {
            Expr::Int(n) => Ok((vec![], PyExpr::Int(*n))),
            Expr::Float(f) => Ok((vec![], PyExpr::Float(*f))),
            Expr::Str(s) => Ok((vec![], PyExpr::Str(s.clone()))),
            Expr::Bool(b) => Ok((vec![], PyExpr::Bool(*b))),
            Expr::Var(name) => Ok((vec![], PyExpr::Name(name.clone()))),

            Expr::Binary { op, lhs, rhs } => {
                let (mut stmts, left) = self.lower_value(lhs, locals)?;
                let (right_stmts, right) = self.lower_value(rhs, locals)?;
                stmts.extend(right_stmts);
                Ok((
                    stmts,
                    PyExpr::BinOp {
                        op: lower_binop(*op),
                        left: Box::new(left),
                        right: Box::new(right),
                    },
                ))
            }

            Expr::If { cond, then, else_ } => {
                let (mut stmts, test) = self.lower_value(cond, locals)?;
                let (then_stmts, then_val) = self.lower_value(then, locals)?;
                let (else_stmts, else_val) = self.lower_value(else_, locals)?;
                if then_stmts.is_empty() && else_stmts.is_empty() {
                    // Both arms are pure expressions: a Python conditional works.
                    Ok((
                        stmts,
                        PyExpr::IfExp {
                            body: Box::new(then_val),
                            test: Box::new(test),
                            orelse: Box::new(else_val),
                        },
                    ))
                } else {
                    // An arm needs statements: hoist into an `if` assigning a temp.
                    let tmp = self.fresh_tmp();
                    let body = with_assign(then_stmts, &tmp, then_val);
                    let orelse = with_assign(else_stmts, &tmp, else_val);
                    stmts.push(PyStmt::If { test, body, orelse });
                    Ok((stmts, PyExpr::Name(tmp)))
                }
            }

            Expr::Match { scrutinee, arms } => {
                // Python `match` is a statement, so always hoist into a temp.
                let (mut stmts, subject) = self.lower_value(scrutinee, locals)?;
                let tmp = self.fresh_tmp();
                let mut cases = Vec::new();
                for arm in arms {
                    let pattern = lower_pattern(&arm.pattern)?;
                    let arm_locals = extend(locals, &pattern_bindings(&arm.pattern));
                    let (arm_stmts, arm_val) = self.lower_value(&arm.body, &arm_locals)?;
                    cases.push(PyCase {
                        pattern,
                        body: with_assign(arm_stmts, &tmp, arm_val),
                    });
                }
                if !has_catch_all(arms) {
                    cases.push(non_exhaustive_guard());
                }
                stmts.push(PyStmt::Match { subject, cases });
                Ok((stmts, PyExpr::Name(tmp)))
            }

            Expr::Fn { params, body } => {
                let inner = extend(locals, params);
                let (body_stmts, body_val) = self.lower_value(body, &inner)?;
                if body_stmts.is_empty() {
                    Ok((
                        vec![],
                        PyExpr::Lambda {
                            params: params.clone(),
                            body: Box::new(body_val),
                        },
                    ))
                } else {
                    // Body needs statements: emit a named nested def and use it.
                    let name = self.fresh_fn();
                    let def_body = self.lower_return(body, &inner)?;
                    let def = PyStmt::FuncDef {
                        name: name.clone(),
                        params: params.clone(),
                        body: def_body,
                    };
                    Ok((vec![def], PyExpr::Name(name)))
                }
            }

            Expr::App { .. } | Expr::Pipe { .. } => self.lower_application(expr, locals),
        }
    }

    fn lower_application(&mut self, expr: &Expr, locals: &HashSet<String>) -> Lowered {
        let mut args_ast = Vec::new();
        let head = flatten_app(expr, &mut args_ast);

        let arity = match head {
            Expr::Var(name) if !locals.contains(name) => self.arities.get(name).copied(),
            Expr::Fn { params, .. } => Some(params.len()),
            _ => None,
        };

        let (mut stmts, head_val) = self.lower_value(head, locals)?;
        let mut arg_vals = Vec::with_capacity(args_ast.len());
        for arg in &args_ast {
            let (arg_stmts, arg_val) = self.lower_value(arg, locals)?;
            stmts.extend(arg_stmts);
            arg_vals.push(arg_val);
        }

        Ok((stmts, self.build_call(head_val, arity, arg_vals)))
    }

    /// Apply currying policy (`DESIGN.md` §5) given the callee's known arity.
    fn build_call(&mut self, head: PyExpr, arity: Option<usize>, args: Vec<PyExpr>) -> PyExpr {
        let n = args.len();
        match arity {
            Some(k) if n < k => {
                // Partial application.
                self.needs_functools = true;
                let mut partial_args = Vec::with_capacity(n + 1);
                partial_args.push(head);
                partial_args.extend(args);
                PyExpr::Call {
                    func: Box::new(PyExpr::Attribute {
                        value: Box::new(PyExpr::Name("functools".to_string())),
                        attr: "partial".to_string(),
                    }),
                    args: partial_args,
                }
            }
            Some(k) if n > k => {
                // Over-application: full call, then apply the remainder one at a time.
                let mut rest = args;
                let first = rest.drain(..k).collect();
                let mut call = PyExpr::Call {
                    func: Box::new(head),
                    args: first,
                };
                for extra in rest {
                    call = PyExpr::Call {
                        func: Box::new(call),
                        args: vec![extra],
                    };
                }
                call
            }
            // Exact arity, or unknown arity (treated as n-ary).
            _ => PyExpr::Call {
                func: Box::new(head),
                args,
            },
        }
    }

    fn fresh_tmp(&mut self) -> String {
        let name = format!("_pf_t{}", self.tmp_counter);
        self.tmp_counter += 1;
        name
    }

    fn fresh_fn(&mut self) -> String {
        let name = format!("_pf_fn{}", self.fn_counter);
        self.fn_counter += 1;
        name
    }
}

/// Flatten an application/pipe spine into `(head, args)` in left-to-right order.
/// `x |> f` is treated as `f x`, so pipes flatten alongside ordinary calls.
fn flatten_app<'a>(expr: &'a Expr, args: &mut Vec<&'a Expr>) -> &'a Expr {
    match expr {
        Expr::App { func, arg } => {
            let head = flatten_app(func, args);
            args.push(arg);
            head
        }
        Expr::Pipe { lhs, rhs } => {
            // `lhs |> rhs` == `rhs lhs`: flatten the callee spine, then the value.
            let head = flatten_app(rhs, args);
            args.push(lhs);
            head
        }
        other => other,
    }
}

fn lower_binop(op: BinOp) -> PyBinOp {
    match op {
        BinOp::Add => PyBinOp::Add,
        BinOp::Sub => PyBinOp::Sub,
        BinOp::Mul => PyBinOp::Mul,
        BinOp::Div => PyBinOp::Div,
    }
}

fn lower_pattern(pattern: &Pattern) -> Result<PyPattern, LowerError> {
    match pattern {
        Pattern::Wildcard => Ok(PyPattern::Wildcard),
        Pattern::Var(name) => Ok(PyPattern::Capture(name.clone())),
        Pattern::Int(n) => Ok(PyPattern::Literal(PyExpr::Int(*n))),
        Pattern::Bool(b) => Ok(PyPattern::Literal(PyExpr::Bool(*b))),
        Pattern::Ctor { .. } => Err(LowerError {
            message:
                "constructor patterns require ADT declarations, which are not implemented yet \
                      (planned for a later phase)"
                    .to_string(),
        }),
    }
}

/// Names a pattern binds, so they can be treated as locals when lowering the arm.
fn pattern_bindings(pattern: &Pattern) -> Vec<String> {
    match pattern {
        Pattern::Var(name) => vec![name.clone()],
        Pattern::Ctor { args, .. } => args.iter().flat_map(pattern_bindings).collect(),
        _ => vec![],
    }
}

/// A `match` is exhaustive at lowering time only if some arm is irrefutable.
fn has_catch_all(arms: &[crate::parser::ast::MatchArm]) -> bool {
    arms.iter()
        .any(|arm| matches!(arm.pattern, Pattern::Wildcard | Pattern::Var(_)))
}

fn non_exhaustive_guard() -> PyCase {
    PyCase {
        pattern: PyPattern::Wildcard,
        body: vec![PyStmt::RaiseRuntimeError(
            "non-exhaustive match".to_string(),
        )],
    }
}

fn extend(base: &HashSet<String>, names: &[String]) -> HashSet<String> {
    let mut out = base.clone();
    out.extend(names.iter().cloned());
    out
}

/// Append `target = value` to a (possibly empty) statement list.
fn with_assign(mut stmts: Vec<PyStmt>, target: &str, value: PyExpr) -> Vec<PyStmt> {
    stmts.push(PyStmt::Assign {
        target: target.to_string(),
        value,
    });
    stmts
}
