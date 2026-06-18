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
//! Lowering runs after type-checking but doesn't yet consume inferred types, so
//! arity is taken from a syntactic module-level table of top-level functions and
//! data constructors (plus `fun` literals applied in place). When the callee's
//! arity is unknown (a parameter, or an imported Python name) the call is emitted
//! n-ary as-is — correct for full application and for Python interop, but it can't
//! synthesize a partial application for an unknown callee. Feeding the type
//! checker's results in here would make arity fully precise.

use std::collections::HashSet;

use crate::parser::ast::{
    BinOp, CeBuilder, CeItem, Expr, ExprKind, Item, LetBinding, Module, Pattern,
};
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
    /// Field count of each data constructor, used both to drive constructor
    /// application and to know which bare references are nullary (and so must be
    /// emitted as `Ctor()`).
    ctor_arity: std::collections::HashMap<String, usize>,
    tmp_counter: usize,
    fn_counter: usize,
    needs_functools: bool,
    /// Whether the built-in `Ok`/`Error` classes must be emitted (the `Result`
    /// prelude), set when a `result {}` block or an `Ok`/`Error` reference is lowered.
    needs_result: bool,
}

type Lowered = Result<(Vec<PyStmt>, PyExpr), LowerError>;

impl Lowerer {
    fn new(module: &Module) -> Self {
        let mut arities = std::collections::HashMap::new();
        let mut ctor_arity = std::collections::HashMap::new();
        for item in &module.items {
            match item {
                Item::Let(binding) => {
                    // A binding's callable arity is the number of parameters of the
                    // Python def/lambda it lowers to: its own `let` parameters, or —
                    // if it's a bare `let name = fun ... -> ...` — the lambda's. Extra
                    // arguments are handled as over-application at the call site.
                    let arity = if !binding.params.is_empty() {
                        Some(binding.params.len())
                    } else if let ExprKind::Fn { params, .. } = &binding.value.kind {
                        Some(params.len())
                    } else {
                        None
                    };
                    if let Some(k) = arity {
                        arities.insert(binding.name.clone(), k);
                    }
                }
                Item::Type(decl) => {
                    for variant in &decl.variants {
                        ctor_arity.insert(variant.name.clone(), variant.fields.len());
                    }
                }
                Item::Expr(_) => {}
            }
        }
        // Built-in Result constructors (see the `result {}` computation expression).
        ctor_arity.insert("Ok".to_string(), 1);
        ctor_arity.insert("Error".to_string(), 1);
        Lowerer {
            arities,
            ctor_arity,
            tmp_counter: 0,
            fn_counter: 0,
            needs_functools: false,
            needs_result: false,
        }
    }

    fn lower_module(&mut self, module: &Module) -> Result<PyModule, LowerError> {
        // User constructor classes.
        let mut classes = Vec::new();
        for item in &module.items {
            if let Item::Type(decl) = item {
                for variant in &decl.variants {
                    let fields = (0..variant.fields.len()).map(|i| format!("_{i}")).collect();
                    classes.push(PyStmt::ClassDef {
                        name: py_ctor_name(&variant.name),
                        fields,
                    });
                }
            }
        }

        // Lower the code; this is what sets needs_functools / needs_result.
        let mut code = Vec::new();
        for item in &module.items {
            match item {
                Item::Type(_) => {} // classes handled above
                Item::Let(binding) => self.lower_let(binding, &mut code)?,
                Item::Expr(expr) => {
                    let (mut stmts, value) = self.lower_value(expr, &HashSet::new())?;
                    code.append(&mut stmts);
                    code.push(PyStmt::Expr(value));
                }
            }
        }

        // Assemble: imports, then the Result prelude, then classes, then code —
        // so every definition precedes its use.
        let mut body = Vec::new();
        if self.needs_functools {
            body.push(PyStmt::Import("functools".to_string()));
        }
        if self.needs_result {
            body.extend(result_prelude());
        }
        body.extend(classes);
        body.extend(code);
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
                is_async: false,
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
        match &expr.kind {
            ExprKind::If { cond, then, else_ } => {
                let (mut stmts, test) = self.lower_value(cond, locals)?;
                let body = self.lower_return(then, locals)?;
                let orelse = self.lower_return(else_, locals)?;
                stmts.push(PyStmt::If { test, body, orelse });
                Ok(stmts)
            }
            ExprKind::Match { scrutinee, arms } => {
                let (mut stmts, subject) = self.lower_value(scrutinee, locals)?;
                let mut cases = Vec::new();
                for arm in arms {
                    let pattern = self.lower_pattern(&arm.pattern);
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
        match &expr.kind {
            ExprKind::Int(n) => Ok((vec![], PyExpr::Int(*n))),
            ExprKind::Float(f) => Ok((vec![], PyExpr::Float(*f))),
            ExprKind::Str(s) => Ok((vec![], PyExpr::Str(s.clone()))),
            ExprKind::Bool(b) => Ok((vec![], PyExpr::Bool(*b))),
            ExprKind::Var(name) => Ok((vec![], self.lower_var(name))),

            ExprKind::Binary { op, lhs, rhs } => {
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

            ExprKind::If { cond, then, else_ } => {
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

            ExprKind::Match { scrutinee, arms } => {
                // Python `match` is a statement, so always hoist into a temp.
                let (mut stmts, subject) = self.lower_value(scrutinee, locals)?;
                let tmp = self.fresh_tmp();
                let mut cases = Vec::new();
                for arm in arms {
                    let pattern = self.lower_pattern(&arm.pattern);
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

            ExprKind::Fn { params, body } => {
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
                        is_async: false,
                    };
                    Ok((vec![def], PyExpr::Name(name)))
                }
            }

            ExprKind::App { .. } | ExprKind::Pipe { .. } => self.lower_application(expr, locals),

            ExprKind::Ce { builder, items } => self.lower_ce(*builder, items, locals),
        }
    }

    fn lower_application(&mut self, expr: &Expr, locals: &HashSet<String>) -> Lowered {
        let mut args_ast = Vec::new();
        let head = flatten_app(expr, &mut args_ast);

        let arity = match &head.kind {
            ExprKind::Var(name) if !locals.contains(name) => self
                .arities
                .get(name)
                .or_else(|| self.ctor_arity.get(name))
                .copied(),
            ExprKind::Fn { params, .. } => Some(params.len()),
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

    /// Lower a variable reference, special-casing data constructors: a nullary
    /// constructor used as a value becomes an instance (`Ctor()`), and any
    /// constructor name is mangled to dodge Python keywords (`None` → `None_`).
    fn lower_var(&mut self, name: &str) -> PyExpr {
        if name == "Ok" || name == "Error" {
            self.needs_result = true;
        }
        match self.ctor_arity.get(name) {
            Some(0) => PyExpr::Call {
                func: Box::new(PyExpr::Name(py_ctor_name(name))),
                args: vec![],
            },
            Some(_) => PyExpr::Name(py_ctor_name(name)),
            None => PyExpr::Name(name.to_string()),
        }
    }

    fn lower_pattern(&mut self, pattern: &Pattern) -> PyPattern {
        match pattern {
            Pattern::Wildcard => PyPattern::Wildcard,
            Pattern::Var(name) => PyPattern::Capture(name.clone()),
            Pattern::Int(n) => PyPattern::Literal(PyExpr::Int(*n)),
            Pattern::Bool(b) => PyPattern::Literal(PyExpr::Bool(*b)),
            Pattern::Ctor { name, args } => {
                if name == "Ok" || name == "Error" {
                    self.needs_result = true;
                }
                let mut lowered = Vec::with_capacity(args.len());
                for arg in args {
                    lowered.push(self.lower_pattern(arg));
                }
                PyPattern::Class {
                    name: py_ctor_name(name),
                    args: lowered,
                }
            }
        }
    }

    // ----- computation expressions (`DESIGN.md` §8.1) -----

    fn lower_ce(
        &mut self,
        builder: CeBuilder,
        items: &[CeItem],
        locals: &HashSet<String>,
    ) -> Lowered {
        match builder {
            CeBuilder::Seq => self.lower_seq(items, locals),
            CeBuilder::Result => {
                self.needs_result = true;
                self.lower_result_ce(items, locals)
            }
            CeBuilder::Async => self.lower_async(items, locals),
        }
    }

    /// `seq { ... }` → a generator function returning its result.
    fn lower_seq(&mut self, items: &[CeItem], locals: &HashSet<String>) -> Lowered {
        let mut body = Vec::new();
        let mut locals = locals.clone();
        let mut has_yield = false;
        for item in items {
            match item {
                CeItem::Yield(e) => {
                    let (mut s, v) = self.lower_value(e, &locals)?;
                    body.append(&mut s);
                    body.push(PyStmt::Yield(v));
                    has_yield = true;
                }
                CeItem::YieldBang(e) => {
                    let (mut s, v) = self.lower_value(e, &locals)?;
                    body.append(&mut s);
                    body.push(PyStmt::YieldFrom(v));
                    has_yield = true;
                }
                CeItem::Let { name, value } => {
                    let (mut s, v) = self.lower_value(value, &locals)?;
                    body.append(&mut s);
                    body.push(PyStmt::Assign {
                        target: name.clone(),
                        value: v,
                    });
                    locals.insert(name.clone());
                }
                _ => return Err(ce_item_error("seq")),
            }
        }
        // A function with no `yield` isn't a generator, so an element-free `seq`
        // returns an empty iterator instead.
        if !has_yield {
            body.push(PyStmt::Return(PyExpr::Call {
                func: Box::new(PyExpr::Name("iter".to_string())),
                args: vec![PyExpr::Call {
                    func: Box::new(PyExpr::Name("tuple".to_string())),
                    args: vec![],
                }],
            }));
        }
        let name = self.fresh_fn();
        let def = PyStmt::FuncDef {
            name: name.clone(),
            params: vec![],
            body,
            is_async: false,
        };
        Ok((vec![def], call0(&name)))
    }

    /// `result { ... }` → a function that short-circuits on `Error`.
    fn lower_result_ce(&mut self, items: &[CeItem], locals: &HashSet<String>) -> Lowered {
        let body = self.lower_result_items(items, locals)?;
        let name = self.fresh_fn();
        let def = PyStmt::FuncDef {
            name: name.clone(),
            params: vec![],
            body,
            is_async: false,
        };
        Ok((vec![def], call0(&name)))
    }

    fn lower_result_items(
        &mut self,
        items: &[CeItem],
        locals: &HashSet<String>,
    ) -> Result<Vec<PyStmt>, LowerError> {
        let Some((first, rest)) = items.split_first() else {
            return Ok(vec![]);
        };
        match first {
            CeItem::Return(e) => {
                let (mut s, v) = self.lower_value(e, locals)?;
                s.push(PyStmt::Return(call1("Ok", v)));
                Ok(s)
            }
            CeItem::ReturnBang(e) => {
                let (mut s, v) = self.lower_value(e, locals)?;
                s.push(PyStmt::Return(v));
                Ok(s)
            }
            CeItem::Let { name, value } => {
                let (mut s, v) = self.lower_value(value, locals)?;
                s.push(PyStmt::Assign {
                    target: name.clone(),
                    value: v,
                });
                let mut locals = locals.clone();
                locals.insert(name.clone());
                s.extend(self.lower_result_items(rest, &locals)?);
                Ok(s)
            }
            CeItem::LetBang { name, value } => {
                let (mut s, v) = self.lower_value(value, locals)?;
                let mut inner_locals = locals.clone();
                inner_locals.insert(name.clone());
                let rest_stmts = self.lower_result_items(rest, &inner_locals)?;
                s.push(self.result_bind_match(v, PyPattern::Capture(name.clone()), rest_stmts));
                Ok(s)
            }
            CeItem::DoBang(e) => {
                let (mut s, v) = self.lower_value(e, locals)?;
                let rest_stmts = self.lower_result_items(rest, locals)?;
                s.push(self.result_bind_match(v, PyPattern::Wildcard, rest_stmts));
                Ok(s)
            }
            _ => Err(ce_item_error("result")),
        }
    }

    /// `match <subject>: case Ok(<ok_pat>): <rest>  case Error(e): return Error(e)`
    fn result_bind_match(
        &mut self,
        subject: PyExpr,
        ok_pat: PyPattern,
        rest: Vec<PyStmt>,
    ) -> PyStmt {
        let e_tmp = self.fresh_tmp();
        PyStmt::Match {
            subject,
            cases: vec![
                PyCase {
                    pattern: PyPattern::Class {
                        name: "Ok".to_string(),
                        args: vec![ok_pat],
                    },
                    body: rest,
                },
                PyCase {
                    pattern: PyPattern::Class {
                        name: "Error".to_string(),
                        args: vec![PyPattern::Capture(e_tmp.clone())],
                    },
                    body: vec![PyStmt::Return(call1("Error", PyExpr::Name(e_tmp)))],
                },
            ],
        }
    }

    /// `async { ... }` → an `async def` returning a coroutine.
    fn lower_async(&mut self, items: &[CeItem], locals: &HashSet<String>) -> Lowered {
        let mut body = Vec::new();
        let mut locals = locals.clone();
        for item in items {
            match item {
                CeItem::LetBang { name, value } => {
                    let (mut s, v) = self.lower_value(value, &locals)?;
                    body.append(&mut s);
                    body.push(PyStmt::Assign {
                        target: name.clone(),
                        value: PyExpr::Await(Box::new(v)),
                    });
                    locals.insert(name.clone());
                }
                CeItem::Let { name, value } => {
                    let (mut s, v) = self.lower_value(value, &locals)?;
                    body.append(&mut s);
                    body.push(PyStmt::Assign {
                        target: name.clone(),
                        value: v,
                    });
                    locals.insert(name.clone());
                }
                CeItem::DoBang(e) => {
                    let (mut s, v) = self.lower_value(e, &locals)?;
                    body.append(&mut s);
                    body.push(PyStmt::Expr(PyExpr::Await(Box::new(v))));
                }
                CeItem::Return(e) => {
                    let (mut s, v) = self.lower_value(e, &locals)?;
                    body.append(&mut s);
                    body.push(PyStmt::Return(v));
                }
                CeItem::ReturnBang(e) => {
                    let (mut s, v) = self.lower_value(e, &locals)?;
                    body.append(&mut s);
                    body.push(PyStmt::Return(PyExpr::Await(Box::new(v))));
                }
                _ => return Err(ce_item_error("async")),
            }
        }
        let name = self.fresh_fn();
        let def = PyStmt::FuncDef {
            name: name.clone(),
            params: vec![],
            body,
            is_async: true,
        };
        Ok((vec![def], call0(&name)))
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
    match &expr.kind {
        ExprKind::App { func, arg } => {
            let head = flatten_app(func, args);
            args.push(arg);
            head
        }
        ExprKind::Pipe { lhs, rhs } => {
            // `lhs |> rhs` == `rhs lhs`: flatten the callee spine, then the value.
            let head = flatten_app(rhs, args);
            args.push(lhs);
            head
        }
        _ => expr,
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

/// Mangle a constructor name to a valid, non-keyword Python identifier.
fn py_ctor_name(name: &str) -> String {
    if matches!(name, "None" | "True" | "False") {
        format!("{name}_")
    } else {
        name.to_string()
    }
}

/// `name()` — a zero-argument call (used to invoke generated CE helper functions).
fn call0(name: &str) -> PyExpr {
    PyExpr::Call {
        func: Box::new(PyExpr::Name(name.to_string())),
        args: vec![],
    }
}

/// `name(arg)` — a one-argument call (used for `Ok`/`Error` construction).
fn call1(name: &str, arg: PyExpr) -> PyExpr {
    PyExpr::Call {
        func: Box::new(PyExpr::Name(name.to_string())),
        args: vec![arg],
    }
}

/// The `Ok`/`Error` classes backing the `result` computation expression.
fn result_prelude() -> Vec<PyStmt> {
    vec![
        PyStmt::ClassDef {
            name: "Ok".to_string(),
            fields: vec!["_0".to_string()],
        },
        PyStmt::ClassDef {
            name: "Error".to_string(),
            fields: vec!["_0".to_string()],
        },
    ]
}

/// A defensive error for a CE item the type checker should already have rejected.
fn ce_item_error(builder: &str) -> LowerError {
    LowerError {
        message: format!("unexpected item in a `{builder}` computation expression"),
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
