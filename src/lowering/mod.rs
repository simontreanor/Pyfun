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

use std::collections::{BTreeSet, HashSet};

use crate::parser::ast::{
    BinOp, BlockStmt, CeBuilder, CeItem, Expr, ExprKind, FieldInit, Item, LetBinding, Module,
    Param, Pattern, TypeDeclKind, TypeExpr,
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
    /// Declared field order of each record type, so literals and updates emit a
    /// positional constructor call in the class's `__init__` order.
    record_fields: std::collections::HashMap<String, Vec<String>>,
    /// Field name → owning record type (mirrors the type checker's registry), to
    /// resolve which class a `{ … }` literal or update constructs.
    field_to_record: std::collections::HashMap<String, String>,
    /// `extern` name → its dotted Python target path (e.g. `["math", "sqrt"]`).
    /// A reference to one lowers to the target rather than the Pyfun name.
    extern_targets: std::collections::HashMap<String, Vec<String>>,
    /// Python modules an *used* extern needs imported (the first segment of a
    /// dotted target, e.g. `math` for `math.sqrt`). Bare builtins import nothing.
    needed_imports: BTreeSet<String>,
    /// Names of top-level `let` bindings, so a user definition shadows a seeded
    /// name (prelude/extern/list helper) at lowering instead of being rerouted.
    user_defs: HashSet<String>,
    /// List-prelude helpers actually referenced, emitted on demand (like the
    /// `Result` prelude). Stored as the Python helper names (e.g. `_pf_map`).
    needed_list_helpers: BTreeSet<&'static str>,
    /// Set/Map-prelude helpers actually referenced (e.g. `_pf_set_add`), emitted on
    /// demand by [`collection_prelude`].
    needed_collection_helpers: BTreeSet<&'static str>,
    /// While lowering an in-file `module`, its name + member names, so a bare
    /// sibling reference rewrites to the mangled top-level name (`Geometry_area`).
    cur_module: Option<(String, HashSet<String>)>,
    tmp_counter: usize,
    fn_counter: usize,
    needs_functools: bool,
    /// Whether the built-in `Ok`/`Error` classes must be emitted (the `Result`
    /// prelude), set when a `result {}` block or an `Ok`/`Error` reference is lowered.
    needs_result: bool,
    /// Whether the built-in `Some`/`None` classes must be emitted (the `Option`
    /// prelude), set when `Some`/`None` or an `Option.*` / `Map.tryFind` member that
    /// constructs them is lowered.
    needs_option: bool,
}

type Lowered = Result<(Vec<PyStmt>, PyExpr), LowerError>;

/// Hoisted statements plus each record field's lowered value, keyed by name.
type LoweredFields = Result<(Vec<PyStmt>, Vec<(String, PyExpr)>), LowerError>;

impl Lowerer {
    fn new(module: &Module) -> Self {
        let mut arities = std::collections::HashMap::new();
        let mut ctor_arity = std::collections::HashMap::new();
        let mut record_fields = std::collections::HashMap::new();
        let mut field_to_record = std::collections::HashMap::new();
        let mut extern_targets = std::collections::HashMap::new();
        let mut user_defs = HashSet::new();
        for item in &module.items {
            match item {
                Item::Extern(decl) => {
                    // Arity drives full-vs-partial application, exactly like the
                    // prelude: it is the number of leading arrows in the type.
                    arities.insert(decl.name.clone(), arrow_arity(&decl.ty));
                    extern_targets.insert(decl.name.clone(), decl.target.clone());
                }
                Item::Let(binding) => {
                    user_defs.insert(binding.name.clone());
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
                Item::Type(decl) => match &decl.kind {
                    TypeDeclKind::Sum(variants) => {
                        for variant in variants {
                            ctor_arity.insert(variant.name.clone(), variant.fields.len());
                        }
                    }
                    TypeDeclKind::Record(fields) => {
                        let names: Vec<String> = fields.iter().map(|f| f.name.clone()).collect();
                        for name in &names {
                            field_to_record.insert(name.clone(), decl.name.clone());
                        }
                        record_fields.insert(decl.name.clone(), names);
                    }
                },
                // A module's members register their arity under the qualified name
                // (`Geometry.area`), matching how `Field` heads are looked up.
                Item::Module { name, items, .. } => {
                    for member in items {
                        let arity = if !member.params.is_empty() {
                            Some(member.params.len())
                        } else if let ExprKind::Fn { params, .. } = &member.value.kind {
                            Some(params.len())
                        } else {
                            None
                        };
                        if let Some(k) = arity {
                            arities.insert(format!("{name}.{}", member.name), k);
                        }
                    }
                }
                Item::Measure { .. } | Item::Expr(_) => {}
            }
        }
        // Built-in Result constructors (see the `result {}` computation expression).
        ctor_arity.insert("Ok".to_string(), 1);
        ctor_arity.insert("Error".to_string(), 1);
        // Built-in Option constructors (`Some`/`None`).
        ctor_arity.insert("Some".to_string(), 1);
        ctor_arity.insert("None".to_string(), 0);
        // Prelude builtins (`print`/`abs`/`min`/`max`): register arities so a
        // *partial* application lowers to `functools.partial`. Pyfun names equal
        // their Python builtin names, so no call-site renaming is needed. User
        // definitions take precedence (`or_insert`), letting a program shadow one.
        for (name, arity) in crate::types::PRELUDE {
            arities.entry((*name).to_string()).or_insert(*arity);
        }
        // Module members (`List.map`, `Set.add`, `Map.findOr`, `Option.map`, …):
        // register their arity under the dotted name so partial application lowers
        // to `functools.partial`. Each routes to a bare Python builtin, an emitted
        // `_pf_*` helper, or a fresh empty container in `lower_module_member`.
        for (module, members) in crate::types::MODULE_PRELUDES {
            for (name, arity) in *members {
                arities.entry(format!("{module}.{name}")).or_insert(*arity);
            }
        }
        Lowerer {
            arities,
            ctor_arity,
            record_fields,
            field_to_record,
            extern_targets,
            needed_imports: BTreeSet::new(),
            user_defs,
            needed_list_helpers: BTreeSet::new(),
            needed_collection_helpers: BTreeSet::new(),
            cur_module: None,
            tmp_counter: 0,
            fn_counter: 0,
            needs_functools: false,
            needs_result: false,
            needs_option: false,
        }
    }

    fn lower_module(&mut self, module: &Module) -> Result<PyModule, LowerError> {
        // User constructor classes (sum variants) and record classes.
        let mut classes = Vec::new();
        for item in &module.items {
            if let Item::Type(decl) = item {
                match &decl.kind {
                    TypeDeclKind::Sum(variants) => {
                        for variant in variants {
                            let fields =
                                (0..variant.fields.len()).map(|i| format!("_{i}")).collect();
                            classes.push(PyStmt::ClassDef {
                                name: py_ctor_name(&variant.name),
                                fields,
                            });
                        }
                    }
                    // Records lower to a class with their real field names.
                    TypeDeclKind::Record(fields) => {
                        classes.push(PyStmt::ClassDef {
                            name: decl.name.clone(),
                            fields: fields.iter().map(|f| f.name.clone()).collect(),
                        });
                    }
                }
            }
        }

        // Lower the code; this is what sets needs_functools / needs_result.
        let mut code = Vec::new();
        for item in &module.items {
            match item {
                // Measures, type declarations, and externs have no runtime code
                // (an extern's effect is purely at its reference sites).
                Item::Measure { .. } | Item::Type(_) | Item::Extern(_) => {}
                Item::Let(binding) => self.lower_let(binding, &HashSet::new(), &mut code)?,
                // A module's members lower to flat top-level defs/assignments with
                // mangled names (`Geometry.area` → `Geometry_area`); bare sibling
                // references rewrite to the same names via `cur_module` (set in
                // `lower_var`/`lower_call`).
                Item::Module { name, items, .. } => {
                    let members = items.iter().map(|m| m.name.clone()).collect();
                    self.cur_module = Some((name.clone(), members));
                    for member in items {
                        let mangled = format!("{name}_{}", member.name);
                        self.lower_binding_as(&mangled, member, &HashSet::new(), &mut code)?;
                    }
                    self.cur_module = None;
                }
                Item::Expr(expr) => {
                    let (mut stmts, value) = self.lower_value(expr, &HashSet::new())?;
                    code.append(&mut stmts);
                    // A unit-valued statement (e.g. an assignment) has no useful
                    // expression to emit — drop the bare `None`.
                    if !matches!(value, PyExpr::NoneLit) {
                        code.push(PyStmt::Expr(value));
                    }
                }
            }
        }

        // Assemble: imports, then the Result prelude, then classes, then code —
        // so every definition precedes its use.
        let mut body = Vec::new();
        if self.needs_functools {
            body.push(PyStmt::Import("functools".to_string()));
        }
        // Modules needed by referenced `extern`s (sorted for deterministic output).
        for module in &self.needed_imports {
            body.push(PyStmt::Import(module.clone()));
        }
        if self.needs_result {
            body.extend(result_prelude());
        }
        if self.needs_option {
            body.extend(option_prelude());
        }
        // List-prelude helpers referenced by the program (deterministic order).
        body.extend(list_prelude(&self.needed_list_helpers));
        // Set/Map-prelude helpers referenced by the program.
        body.extend(collection_prelude(&self.needed_collection_helpers));
        body.extend(classes);
        body.extend(code);
        Ok(PyModule { body })
    }

    fn lower_let(
        &mut self,
        binding: &LetBinding,
        locals: &HashSet<String>,
        out: &mut Vec<PyStmt>,
    ) -> Result<(), LowerError> {
        self.lower_binding_as(&binding.name, binding, locals, out)
    }

    /// Lower a `let` binding, emitting it under `name` (so a module member can use
    /// its mangled `Module_member` name instead of `binding.name`).
    fn lower_binding_as(
        &mut self,
        name: &str,
        binding: &LetBinding,
        locals: &HashSet<String>,
        out: &mut Vec<PyStmt>,
    ) -> Result<(), LowerError> {
        if binding.params.is_empty() {
            let (mut stmts, value) = self.lower_value(&binding.value, locals)?;
            out.append(&mut stmts);
            out.push(PyStmt::Assign {
                target: name.to_string(),
                value,
            });
        } else {
            // A nested function captures the enclosing locals (Python closures),
            // so they count as locals when resolving names in its body.
            let names = param_names(&binding.params);
            let inner = extend(locals, &names);
            let body = self.lower_return(&binding.value, &inner)?;
            out.push(PyStmt::FuncDef {
                name: name.to_string(),
                params: names,
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
            ExprKind::Block { stmts } => self.lower_block_return(stmts, locals),
            _ => {
                let (mut stmts, value) = self.lower_value(expr, locals)?;
                stmts.push(PyStmt::Return(value));
                Ok(stmts)
            }
        }
    }

    /// Lower a block in tail position: each non-final statement becomes Python
    /// statements; the final expression is lowered in return position.
    fn lower_block_return(
        &mut self,
        stmts: &[BlockStmt],
        locals: &HashSet<String>,
    ) -> Result<Vec<PyStmt>, LowerError> {
        let mut out = Vec::new();
        let mut locals = locals.clone();
        let last = stmts.len().saturating_sub(1);
        for (i, stmt) in stmts.iter().enumerate() {
            match stmt {
                BlockStmt::Let(b) => {
                    self.lower_let(b, &locals, &mut out)?;
                    locals.insert(b.name.clone());
                }
                BlockStmt::Expr(e) if i == last => out.extend(self.lower_return(e, &locals)?),
                BlockStmt::Expr(e) => {
                    let (mut s, v) = self.lower_value(e, &locals)?;
                    out.append(&mut s);
                    if !matches!(v, PyExpr::NoneLit) {
                        out.push(PyStmt::Expr(v));
                    }
                }
            }
        }
        Ok(out)
    }

    /// Lower `expr` in value position: a list of statements to run first, plus a
    /// Python expression denoting the value.
    fn lower_value(&mut self, expr: &Expr, locals: &HashSet<String>) -> Lowered {
        match &expr.kind {
            ExprKind::Int(n) => Ok((vec![], PyExpr::Int(*n))),
            ExprKind::Float(f) => Ok((vec![], PyExpr::Float(*f))),
            ExprKind::Str(s) => Ok((vec![], PyExpr::Str(s.clone()))),
            ExprKind::Bool(b) => Ok((vec![], PyExpr::Bool(*b))),
            ExprKind::Var(name) => Ok((vec![], self.lower_var(name, locals))),

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
                let names = param_names(params);
                let inner = extend(locals, &names);
                let (body_stmts, body_val) = self.lower_value(body, &inner)?;
                if body_stmts.is_empty() {
                    Ok((
                        vec![],
                        PyExpr::Lambda {
                            params: names,
                            body: Box::new(body_val),
                        },
                    ))
                } else {
                    // Body needs statements: emit a named nested def and use it.
                    let name = self.fresh_fn();
                    let def_body = self.lower_return(body, &inner)?;
                    let def = PyStmt::FuncDef {
                        name: name.clone(),
                        params: names,
                        body: def_body,
                        is_async: false,
                    };
                    Ok((vec![def], PyExpr::Name(name)))
                }
            }

            ExprKind::Unary { op, expr } => {
                let (stmts, value) = self.lower_value(expr, locals)?;
                let lowered = match op {
                    crate::parser::ast::UnOp::Not => PyExpr::Not(Box::new(value)),
                };
                Ok((stmts, lowered))
            }

            ExprKind::App { .. } | ExprKind::Pipe { .. } => self.lower_application(expr, locals),

            ExprKind::Ce { builder, items } => self.lower_ce(builder, items, expr.span(), locals),

            // Units are compile-time only: erase the annotation, keep the value.
            ExprKind::Annot { value, .. } => self.lower_value(value, locals),

            ExprKind::List { elems } => {
                let mut stmts = Vec::new();
                let mut vals = Vec::with_capacity(elems.len());
                for e in elems {
                    let (s, v) = self.lower_value(e, locals)?;
                    stmts.extend(s);
                    vals.push(v);
                }
                Ok((stmts, PyExpr::List(vals)))
            }

            ExprKind::Record { fields } => self.lower_record(fields, locals),
            ExprKind::RecordUpdate { base, fields } => {
                self.lower_record_update(base, fields, locals)
            }
            ExprKind::Field { base, name } => {
                // `Module.member` resolves to its builtin/helper; otherwise it is an
                // ordinary record-field access.
                if let Some(q) = crate::types::qualified_name(expr) {
                    return Ok((vec![], self.lower_module_member(&q)));
                }
                let (stmts, value) = self.lower_value(base, locals)?;
                Ok((
                    stmts,
                    PyExpr::Attribute {
                        value: Box::new(value),
                        attr: name.clone(),
                    },
                ))
            }

            ExprKind::Block { stmts } => self.lower_block_value(stmts, locals),

            ExprKind::Assign { target, value } => {
                let (mut stmts, v) = self.lower_value(value, locals)?;
                stmts.push(PyStmt::Assign {
                    target: target.clone(),
                    value: v,
                });
                // An assignment is a Python statement; its value is unit.
                Ok((stmts, PyExpr::NoneLit))
            }
        }
    }

    /// Lower a block in value position: non-final statements are hoisted before
    /// the value, the final expression supplies the value.
    fn lower_block_value(&mut self, stmts: &[BlockStmt], locals: &HashSet<String>) -> Lowered {
        let mut out = Vec::new();
        let mut locals = locals.clone();
        let last = stmts.len().saturating_sub(1);
        let mut value = PyExpr::NoneLit;
        for (i, stmt) in stmts.iter().enumerate() {
            match stmt {
                BlockStmt::Let(b) => {
                    self.lower_let(b, &locals, &mut out)?;
                    locals.insert(b.name.clone());
                }
                BlockStmt::Expr(e) => {
                    let (mut s, v) = self.lower_value(e, &locals)?;
                    out.append(&mut s);
                    if i == last {
                        value = v;
                    } else if !matches!(v, PyExpr::NoneLit) {
                        out.push(PyStmt::Expr(v));
                    }
                }
            }
        }
        Ok((out, value))
    }

    /// `{ x = a, y = b }` → `Record(a, b)` — a positional constructor call in the
    /// class's declared field order (the type checker guarantees the literal names
    /// exactly the record's fields).
    fn lower_record(&mut self, fields: &[FieldInit], locals: &HashSet<String>) -> Lowered {
        let record = self.field_to_record[&fields[0].name].clone();
        let order = self.record_fields[&record].clone();
        let (stmts, mut lowered) = self.lower_field_inits(fields, locals)?;
        let mut args = Vec::with_capacity(order.len());
        for name in &order {
            let i = lowered
                .iter()
                .position(|(n, _)| n == name)
                .expect("type-checked record literal is complete");
            args.push(lowered.remove(i).1);
        }
        Ok((
            stmts,
            PyExpr::Call {
                func: Box::new(PyExpr::Name(record)),
                args,
            },
        ))
    }

    /// `{ base with x = a }` → `Record(_t.x_or_a, …)` — bind `base` to a temp so it
    /// is evaluated once, then construct a fresh record taking each field from the
    /// update or, failing that, from the temp.
    fn lower_record_update(
        &mut self,
        base: &Expr,
        fields: &[FieldInit],
        locals: &HashSet<String>,
    ) -> Lowered {
        let record = self.field_to_record[&fields[0].name].clone();
        let order = self.record_fields[&record].clone();
        let (mut stmts, base_val) = self.lower_value(base, locals)?;
        let tmp = self.fresh_tmp();
        stmts.push(PyStmt::Assign {
            target: tmp.clone(),
            value: base_val,
        });
        let (field_stmts, mut lowered) = self.lower_field_inits(fields, locals)?;
        stmts.extend(field_stmts);
        let mut args = Vec::with_capacity(order.len());
        for name in &order {
            match lowered.iter().position(|(n, _)| n == name) {
                Some(i) => args.push(lowered.remove(i).1),
                None => args.push(PyExpr::Attribute {
                    value: Box::new(PyExpr::Name(tmp.clone())),
                    attr: name.clone(),
                }),
            }
        }
        Ok((
            stmts,
            PyExpr::Call {
                func: Box::new(PyExpr::Name(record)),
                args,
            },
        ))
    }

    /// Lower a list of `name = value` field initializers in source order, hoisting
    /// any statements; returns the hoisted statements and the lowered values keyed
    /// by field name.
    fn lower_field_inits(
        &mut self,
        fields: &[FieldInit],
        locals: &HashSet<String>,
    ) -> LoweredFields {
        let mut stmts = Vec::new();
        let mut values = Vec::with_capacity(fields.len());
        for init in fields {
            let (s, v) = self.lower_value(&init.value, locals)?;
            stmts.extend(s);
            values.push((init.name.clone(), v));
        }
        Ok((stmts, values))
    }

    fn lower_application(&mut self, expr: &Expr, locals: &HashSet<String>) -> Lowered {
        let mut args_ast = Vec::new();
        let head = flatten_app(expr, &mut args_ast);

        let arity = match &head.kind {
            // A bare reference to a sibling member inside a module — its arity is
            // registered under the qualified name (`Geometry.area`).
            ExprKind::Var(name)
                if !locals.contains(name)
                    && self
                        .cur_module
                        .as_ref()
                        .is_some_and(|(_, members)| members.contains(name)) =>
            {
                let module = &self.cur_module.as_ref().unwrap().0;
                self.arities.get(&format!("{module}.{name}")).copied()
            }
            ExprKind::Var(name) if !locals.contains(name) => self
                .arities
                .get(name)
                .or_else(|| self.ctor_arity.get(name))
                .copied(),
            // A module-qualified head (`List.map`, `Set.add`) — arity from the dotted
            // name registered from `MODULE_PRELUDES`.
            ExprKind::Field { .. } => {
                crate::types::qualified_name(head).and_then(|q| self.arities.get(&q).copied())
            }
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
    fn lower_var(&mut self, name: &str, locals: &HashSet<String>) -> PyExpr {
        if name == "Ok" || name == "Error" {
            self.needs_result = true;
        }
        if name == "Some" || name == "None" {
            self.needs_option = true;
        }
        // A bare reference to a sibling member inside a module → its mangled
        // top-level name (`pi` → `Geometry_pi`), unless shadowed by a local.
        if !locals.contains(name)
            && let Some((m, members)) = &self.cur_module
            && members.contains(name)
        {
            return PyExpr::Name(format!("{m}_{name}"));
        }
        // A local parameter or a user top-level binding shadows a seeded name
        // (extern routing), so skip rerouting in that case. Module members
        // (`List.map`, …) are field-access nodes, routed in `lower_value`, not here.
        if !locals.contains(name) && !self.user_defs.contains(name) {
            // An `extern` reference lowers to its dotted Python target (e.g.
            // `math.sqrt`), recording any module that must be imported.
            if let Some(target) = self.extern_targets.get(name).cloned() {
                if target.len() > 1 {
                    self.needed_imports.insert(target[0].clone());
                }
                return dotted_path(&target);
            }
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

    /// Lower a built-in module member (`List.map`, `Set.empty`, `Map.tryFind`, …) to
    /// the Python it routes to: a bare builtin name (`len`/`set`/`list`), a fresh
    /// empty container (`set()`/`dict()`), or an emitted `_pf_*` helper (recorded so
    /// it is defined, and flagging `functools` / the `Option` prelude as needed).
    fn lower_module_member(&mut self, qualified: &str) -> PyExpr {
        let bare = |n: &str| PyExpr::Name(n.to_string());
        let empty = |n: &str| PyExpr::Call {
            func: Box::new(PyExpr::Name(n.to_string())),
            args: vec![],
        };
        // Route to an emitted list helper.
        let list = |s: &mut Self, helper: &'static str| {
            s.needed_list_helpers.insert(helper);
            PyExpr::Name(helper.to_string())
        };
        // Route to an emitted set/map/option helper.
        let coll = |s: &mut Self, helper: &'static str| {
            s.needed_collection_helpers.insert(helper);
            PyExpr::Name(helper.to_string())
        };
        match qualified {
            // List
            "List.len" => bare("len"),
            "List.sum" => bare("sum"),
            "List.map" => list(self, "_pf_map"),
            "List.filter" => list(self, "_pf_filter"),
            "List.fold" => {
                self.needs_functools = true;
                list(self, "_pf_fold")
            }
            "List.rev" => list(self, "_pf_rev"),
            "List.range" => list(self, "_pf_range"),
            // Set
            "Set.empty" => empty("set"),
            "Set.len" => bare("len"),
            "Set.ofList" => bare("set"),
            "Set.toList" => bare("list"),
            "Set.add" => coll(self, "_pf_set_add"),
            "Set.remove" => coll(self, "_pf_set_remove"),
            "Set.contains" => coll(self, "_pf_set_contains"),
            "Set.union" => coll(self, "_pf_set_union"),
            "Set.intersect" => coll(self, "_pf_set_intersect"),
            "Set.difference" => coll(self, "_pf_set_difference"),
            // Map
            "Map.empty" => empty("dict"),
            "Map.len" => bare("len"),
            "Map.add" => coll(self, "_pf_map_add"),
            "Map.remove" => coll(self, "_pf_map_remove"),
            "Map.contains" => coll(self, "_pf_map_contains"),
            "Map.findOr" => coll(self, "_pf_map_find_or"),
            "Map.tryFind" => {
                self.needs_option = true;
                coll(self, "_pf_map_try_find")
            }
            "Map.keys" => coll(self, "_pf_map_keys"),
            "Map.values" => coll(self, "_pf_map_values"),
            // Option (helpers construct `Some`/`None`, so flag the Option prelude)
            "Option.map" => {
                self.needs_option = true;
                coll(self, "_pf_option_map")
            }
            "Option.withDefault" => {
                self.needs_option = true;
                coll(self, "_pf_option_with_default")
            }
            "Option.isSome" => {
                self.needs_option = true;
                coll(self, "_pf_option_is_some")
            }
            "Option.isNone" => {
                self.needs_option = true;
                coll(self, "_pf_option_is_none")
            }
            // Result (helpers inspect/construct `Ok`/`Error`, so flag the Result
            // prelude; `toOption` also constructs `Some`/`None`).
            "Result.map" => {
                self.needs_result = true;
                coll(self, "_pf_result_map")
            }
            "Result.mapError" => {
                self.needs_result = true;
                coll(self, "_pf_result_map_error")
            }
            "Result.bind" => {
                self.needs_result = true;
                coll(self, "_pf_result_bind")
            }
            "Result.withDefault" => {
                self.needs_result = true;
                coll(self, "_pf_result_with_default")
            }
            "Result.isOk" => {
                self.needs_result = true;
                coll(self, "_pf_result_is_ok")
            }
            "Result.isError" => {
                self.needs_result = true;
                coll(self, "_pf_result_is_error")
            }
            "Result.toOption" => {
                self.needs_result = true;
                self.needs_option = true;
                coll(self, "_pf_result_to_option")
            }
            // Seq — the lazy module. Map/filter/range/iter/list are Python's own lazy
            // builtins (no wrappers needed, unlike the eager `List`); fold reuses the
            // list `_pf_fold` (reduce); take needs `itertools.islice`.
            "Seq.map" => bare("map"),
            "Seq.filter" => bare("filter"),
            "Seq.ofList" => bare("iter"),
            "Seq.toList" => bare("list"),
            "Seq.range" => bare("range"),
            "Seq.fold" => {
                self.needs_functools = true;
                list(self, "_pf_fold")
            }
            "Seq.take" => {
                self.needed_imports.insert("itertools".to_string());
                coll(self, "_pf_seq_take")
            }
            // A user module member (`Geometry.area`) → its mangled top-level name.
            other => PyExpr::Name(other.replace('.', "_")),
        }
    }

    fn lower_pattern(&mut self, pattern: &Pattern) -> PyPattern {
        match pattern {
            Pattern::Wildcard => PyPattern::Wildcard,
            Pattern::Var { name, .. } => PyPattern::Capture(name.clone()),
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
            Pattern::Record { fields } => {
                // Records lower to a class named after the record type; the field
                // names match its attributes, so emit a keyword class pattern.
                let record = self.field_to_record[&fields[0].name].clone();
                let lowered = fields
                    .iter()
                    .map(|f| (f.name.clone(), self.lower_pattern(&f.pattern)))
                    .collect();
                PyPattern::ClassKw {
                    name: record,
                    fields: lowered,
                }
            }
        }
    }

    // ----- computation expressions (`DESIGN.md` §8.1) -----

    fn lower_ce(
        &mut self,
        builder: &CeBuilder,
        items: &[CeItem],
        span: crate::lexer::Span,
        locals: &HashSet<String>,
    ) -> Lowered {
        match builder {
            CeBuilder::Seq => self.lower_seq(items, locals),
            CeBuilder::Result => {
                self.needs_result = true;
                self.lower_result_ce(items, locals)
            }
            CeBuilder::Async => self.lower_async(items, locals),
            // A user builder desugars to plain calls; lower the desugared form.
            // (Any structural error was already reported during type-checking.)
            CeBuilder::User(name) => {
                let expr = crate::desugar::desugar_ce(name, items, span)
                    .map_err(|(message, _)| LowerError { message })?;
                self.lower_value(&expr, locals)
            }
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
                CeItem::Let { name, value, .. } => {
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
            CeItem::Let { name, value, .. } => {
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
            CeItem::LetBang { name, value, .. } => {
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
                CeItem::LetBang { name, value, .. } => {
                    let (mut s, v) = self.lower_value(value, &locals)?;
                    body.append(&mut s);
                    body.push(PyStmt::Assign {
                        target: name.clone(),
                        value: PyExpr::Await(Box::new(v)),
                    });
                    locals.insert(name.clone());
                }
                CeItem::Let { name, value, .. } => {
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
        BinOp::FloorDiv => PyBinOp::FloorDiv,
        BinOp::Eq => PyBinOp::Eq,
        BinOp::Ne => PyBinOp::Ne,
        BinOp::Lt => PyBinOp::Lt,
        BinOp::Gt => PyBinOp::Gt,
        BinOp::Le => PyBinOp::Le,
        BinOp::Ge => PyBinOp::Ge,
        BinOp::And => PyBinOp::And,
        BinOp::Or => PyBinOp::Or,
    }
}

/// The number of leading arrows in a declared type — an `extern`'s callable arity,
/// used (as for the prelude) to decide full vs partial application.
fn arrow_arity(ty: &TypeExpr) -> usize {
    match ty {
        TypeExpr::Fun(_, ret) => 1 + arrow_arity(ret),
        TypeExpr::Con(..) => 0,
    }
}

/// Build a Python expression from a dotted path: `["math", "sqrt"]` → `math.sqrt`,
/// a single segment → a bare name.
fn dotted_path(segments: &[String]) -> PyExpr {
    let mut iter = segments.iter();
    let mut expr = PyExpr::Name(iter.next().expect("non-empty target").clone());
    for seg in iter {
        expr = PyExpr::Attribute {
            value: Box::new(expr),
            attr: seg.clone(),
        };
    }
    expr
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

/// The `Some`/`None_` classes backing the built-in `Option` type (`None` is mangled
/// to dodge the Python keyword). Structural `__eq__`/`__repr__`/`__match_args__` come
/// from `emit_class`, like any data constructor.
fn option_prelude() -> Vec<PyStmt> {
    vec![
        PyStmt::ClassDef {
            name: "Some".to_string(),
            fields: vec!["_0".to_string()],
        },
        PyStmt::ClassDef {
            name: "None_".to_string(),
            fields: vec![],
        },
    ]
}

/// The list-prelude helper definitions actually referenced (`DESIGN.md` §6). Each
/// keeps eager-list semantics that Python's lazy `map`/`filter` would not: they
/// force results into a `list`. `_pf_fold` reuses `functools.reduce` with an
/// initial accumulator (a *total* left fold). Built from the IR (no string
/// splicing); emitted in the helper-name's sorted order for deterministic output.
fn list_prelude(used: &BTreeSet<&'static str>) -> Vec<PyStmt> {
    // `func(args...)` where `func` is a bare name.
    let call = |func: &str, args: Vec<PyExpr>| PyExpr::Call {
        func: Box::new(PyExpr::Name(func.to_string())),
        args,
    };
    let name = |n: &str| PyExpr::Name(n.to_string());
    let def = |fn_name: &str, params: &[&str], ret: PyExpr| PyStmt::FuncDef {
        name: fn_name.to_string(),
        params: params.iter().map(|p| p.to_string()).collect(),
        body: vec![PyStmt::Return(ret)],
        is_async: false,
    };
    used.iter()
        .map(|&helper| match helper {
            // _pf_map(f, xs) -> list(map(f, xs))
            "_pf_map" => def(
                "_pf_map",
                &["f", "xs"],
                call("list", vec![call("map", vec![name("f"), name("xs")])]),
            ),
            // _pf_filter(f, xs) -> list(filter(f, xs))
            "_pf_filter" => def(
                "_pf_filter",
                &["f", "xs"],
                call("list", vec![call("filter", vec![name("f"), name("xs")])]),
            ),
            // _pf_fold(f, acc, xs) -> functools.reduce(f, xs, acc)
            "_pf_fold" => def(
                "_pf_fold",
                &["f", "acc", "xs"],
                PyExpr::Call {
                    func: Box::new(PyExpr::Attribute {
                        value: Box::new(name("functools")),
                        attr: "reduce".to_string(),
                    }),
                    args: vec![name("f"), name("xs"), name("acc")],
                },
            ),
            // _pf_rev(xs) -> list(reversed(xs))
            "_pf_rev" => def(
                "_pf_rev",
                &["xs"],
                call("list", vec![call("reversed", vec![name("xs")])]),
            ),
            // _pf_range(lo, hi) -> list(range(lo, hi))
            "_pf_range" => def(
                "_pf_range",
                &["lo", "hi"],
                call("list", vec![call("range", vec![name("lo"), name("hi")])]),
            ),
            other => unreachable!("unknown list helper {other}"),
        })
        .collect()
}

/// The `Set` / `Map` / `Option` module-helper definitions actually referenced
/// (`DESIGN.md` §6). Each is a small wrapper over Python's `set`/`dict` (or the
/// `Some`/`None_` classes) so the curried Pyfun function is a single callable
/// (partial application still works). The collections are immutable-style: every
/// operation returns a fresh container. Built from the IR (no string splicing);
/// emitted in sorted helper-name order for deterministic output.
fn collection_prelude(used: &BTreeSet<&'static str>) -> Vec<PyStmt> {
    let name = |n: &str| PyExpr::Name(n.to_string());
    let call = |func: &str, args: Vec<PyExpr>| PyExpr::Call {
        func: Box::new(PyExpr::Name(func.to_string())),
        args,
    };
    let attr = |recv: PyExpr, a: &str| PyExpr::Attribute {
        value: Box::new(recv),
        attr: a.to_string(),
    };
    // `recv.method(args...)`
    let method = |recv: PyExpr, m: &str, args: Vec<PyExpr>| PyExpr::Call {
        func: Box::new(PyExpr::Attribute {
            value: Box::new(recv),
            attr: m.to_string(),
        }),
        args,
    };
    let binop = |op: PyBinOp, left: PyExpr, right: PyExpr| PyExpr::BinOp {
        op,
        left: Box::new(left),
        right: Box::new(right),
    };
    // `isinstance(x, Class)` — the Option/Result discriminants.
    let is_some = |o: &str| call("isinstance", vec![name(o), name("Some")]);
    let is_ok = |r: &str| call("isinstance", vec![name(r), name("Ok")]);
    let is_err = |r: &str| call("isinstance", vec![name(r), name("Error")]);
    let def1 = |fn_name: &str, params: &[&str], ret: PyExpr| PyStmt::FuncDef {
        name: fn_name.to_string(),
        params: params.iter().map(|p| p.to_string()).collect(),
        body: vec![PyStmt::Return(ret)],
        is_async: false,
    };
    let def = |fn_name: &str, params: &[&str], body: Vec<PyStmt>| PyStmt::FuncDef {
        name: fn_name.to_string(),
        params: params.iter().map(|p| p.to_string()).collect(),
        body,
        is_async: false,
    };
    used.iter()
        .map(|&helper| match helper {
            // Set.add(x, s) -> s.union([x])  (a fresh set)
            "_pf_set_add" => def1(
                helper,
                &["x", "s"],
                method(name("s"), "union", vec![PyExpr::List(vec![name("x")])]),
            ),
            // Set.remove(x, s) -> s.difference([x])
            "_pf_set_remove" => def1(
                helper,
                &["x", "s"],
                method(name("s"), "difference", vec![PyExpr::List(vec![name("x")])]),
            ),
            // Set.contains(x, s) -> x in s
            "_pf_set_contains" => def1(
                helper,
                &["x", "s"],
                binop(PyBinOp::In, name("x"), name("s")),
            ),
            // Set.union(a, b) -> a.union(b)
            "_pf_set_union" => def1(
                helper,
                &["a", "b"],
                method(name("a"), "union", vec![name("b")]),
            ),
            // Set.intersect(a, b) -> a.intersection(b)
            "_pf_set_intersect" => def1(
                helper,
                &["a", "b"],
                method(name("a"), "intersection", vec![name("b")]),
            ),
            // Set.difference(a, b) -> a.difference(b)
            "_pf_set_difference" => def1(
                helper,
                &["a", "b"],
                method(name("a"), "difference", vec![name("b")]),
            ),
            // Map.add(k, v, m) -> dict(list(m.items()) + [[k, v]])  (last pair wins)
            "_pf_map_add" => def1(
                helper,
                &["k", "v", "m"],
                call(
                    "dict",
                    vec![binop(
                        PyBinOp::Add,
                        call("list", vec![method(name("m"), "items", vec![])]),
                        PyExpr::List(vec![PyExpr::List(vec![name("k"), name("v")])]),
                    )],
                ),
            ),
            // Map.remove(k, m): copy then pop (no comprehensions in the IR).
            "_pf_map_remove" => def(
                helper,
                &["k", "m"],
                vec![
                    PyStmt::Assign {
                        target: "r".to_string(),
                        value: call("dict", vec![name("m")]),
                    },
                    PyStmt::Expr(method(name("r"), "pop", vec![name("k"), PyExpr::NoneLit])),
                    PyStmt::Return(name("r")),
                ],
            ),
            // Map.contains(k, m) -> k in m
            "_pf_map_contains" => def1(
                helper,
                &["k", "m"],
                binop(PyBinOp::In, name("k"), name("m")),
            ),
            // Map.findOr(k, default, m) -> m.get(k, default)
            "_pf_map_find_or" => def1(
                helper,
                &["k", "default", "m"],
                method(name("m"), "get", vec![name("k"), name("default")]),
            ),
            // Map.tryFind(k, m) -> Some(m.get(k)) if k in m else None_()
            "_pf_map_try_find" => def(
                helper,
                &["k", "m"],
                vec![
                    PyStmt::If {
                        test: binop(PyBinOp::In, name("k"), name("m")),
                        body: vec![PyStmt::Return(call1(
                            "Some",
                            method(name("m"), "get", vec![name("k")]),
                        ))],
                        orelse: vec![],
                    },
                    PyStmt::Return(call0("None_")),
                ],
            ),
            // Map.keys(m) -> list(m.keys())
            "_pf_map_keys" => def1(
                helper,
                &["m"],
                call("list", vec![method(name("m"), "keys", vec![])]),
            ),
            // Map.values(m) -> list(m.values())
            "_pf_map_values" => def1(
                helper,
                &["m"],
                call("list", vec![method(name("m"), "values", vec![])]),
            ),
            // Option.map(f, o) -> Some(f(o._0)) if isinstance(o, Some) else None_()
            "_pf_option_map" => def(
                helper,
                &["f", "o"],
                vec![
                    PyStmt::If {
                        test: is_some("o"),
                        body: vec![PyStmt::Return(call1(
                            "Some",
                            PyExpr::Call {
                                func: Box::new(name("f")),
                                args: vec![attr(name("o"), "_0")],
                            },
                        ))],
                        orelse: vec![],
                    },
                    PyStmt::Return(call0("None_")),
                ],
            ),
            // Option.withDefault(d, o) -> o._0 if isinstance(o, Some) else d
            "_pf_option_with_default" => def(
                helper,
                &["d", "o"],
                vec![
                    PyStmt::If {
                        test: is_some("o"),
                        body: vec![PyStmt::Return(attr(name("o"), "_0"))],
                        orelse: vec![],
                    },
                    PyStmt::Return(name("d")),
                ],
            ),
            // Option.isSome(o) -> isinstance(o, Some)
            "_pf_option_is_some" => def1(helper, &["o"], is_some("o")),
            // Option.isNone(o) -> not isinstance(o, Some)
            "_pf_option_is_none" => def1(helper, &["o"], PyExpr::Not(Box::new(is_some("o")))),
            // Result.map(f, r) -> Ok(f(r._0)) if isinstance(r, Ok) else r  (Error passes through)
            "_pf_result_map" => def(
                helper,
                &["f", "r"],
                vec![
                    PyStmt::If {
                        test: is_ok("r"),
                        body: vec![PyStmt::Return(call1(
                            "Ok",
                            PyExpr::Call {
                                func: Box::new(name("f")),
                                args: vec![attr(name("r"), "_0")],
                            },
                        ))],
                        orelse: vec![],
                    },
                    PyStmt::Return(name("r")),
                ],
            ),
            // Result.mapError(f, r) -> Error(f(r._0)) if isinstance(r, Error) else r
            "_pf_result_map_error" => def(
                helper,
                &["f", "r"],
                vec![
                    PyStmt::If {
                        test: is_err("r"),
                        body: vec![PyStmt::Return(call1(
                            "Error",
                            PyExpr::Call {
                                func: Box::new(name("f")),
                                args: vec![attr(name("r"), "_0")],
                            },
                        ))],
                        orelse: vec![],
                    },
                    PyStmt::Return(name("r")),
                ],
            ),
            // Result.bind(f, r) -> f(r._0) if isinstance(r, Ok) else r
            "_pf_result_bind" => def(
                helper,
                &["f", "r"],
                vec![
                    PyStmt::If {
                        test: is_ok("r"),
                        body: vec![PyStmt::Return(PyExpr::Call {
                            func: Box::new(name("f")),
                            args: vec![attr(name("r"), "_0")],
                        })],
                        orelse: vec![],
                    },
                    PyStmt::Return(name("r")),
                ],
            ),
            // Result.withDefault(d, r) -> r._0 if isinstance(r, Ok) else d
            "_pf_result_with_default" => def(
                helper,
                &["d", "r"],
                vec![
                    PyStmt::If {
                        test: is_ok("r"),
                        body: vec![PyStmt::Return(attr(name("r"), "_0"))],
                        orelse: vec![],
                    },
                    PyStmt::Return(name("d")),
                ],
            ),
            // Result.isOk(r) -> isinstance(r, Ok)
            "_pf_result_is_ok" => def1(helper, &["r"], is_ok("r")),
            // Result.isError(r) -> isinstance(r, Error)
            "_pf_result_is_error" => def1(helper, &["r"], is_err("r")),
            // Result.toOption(r) -> Some(r._0) if isinstance(r, Ok) else None_()
            "_pf_result_to_option" => def(
                helper,
                &["r"],
                vec![
                    PyStmt::If {
                        test: is_ok("r"),
                        body: vec![PyStmt::Return(call1("Some", attr(name("r"), "_0")))],
                        orelse: vec![],
                    },
                    PyStmt::Return(call0("None_")),
                ],
            ),
            // Seq.take(n, xs) -> itertools.islice(xs, n)  (reorders args; stays lazy)
            "_pf_seq_take" => def1(
                helper,
                &["n", "xs"],
                PyExpr::Call {
                    func: Box::new(PyExpr::Attribute {
                        value: Box::new(name("itertools")),
                        attr: "islice".to_string(),
                    }),
                    args: vec![name("xs"), name("n")],
                },
            ),
            other => unreachable!("unknown collection helper {other}"),
        })
        .collect()
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
        Pattern::Var { name, .. } => vec![name.clone()],
        Pattern::Ctor { args, .. } => args.iter().flat_map(pattern_bindings).collect(),
        Pattern::Record { fields } => fields
            .iter()
            .flat_map(|f| pattern_bindings(&f.pattern))
            .collect(),
        _ => vec![],
    }
}

/// A `match` is exhaustive at lowering time only if some arm is irrefutable (a
/// wildcard, a variable, or a record pattern with all-irrefutable fields).
fn has_catch_all(arms: &[crate::parser::ast::MatchArm]) -> bool {
    arms.iter().any(|arm| is_irrefutable(&arm.pattern))
}

fn is_irrefutable(pattern: &Pattern) -> bool {
    match pattern {
        Pattern::Wildcard | Pattern::Var { .. } => true,
        Pattern::Record { fields } => fields.iter().all(|f| is_irrefutable(&f.pattern)),
        Pattern::Int(_) | Pattern::Bool(_) | Pattern::Ctor { .. } => false,
    }
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

/// The names of a parameter list — parameters lower to plain Python argument
/// names; their source spans (carried for the LSP) are erased here.
fn param_names(params: &[Param]) -> Vec<String> {
    params.iter().map(|p| p.name.clone()).collect()
}

/// Append `target = value` to a (possibly empty) statement list.
fn with_assign(mut stmts: Vec<PyStmt>, target: &str, value: PyExpr) -> Vec<PyStmt> {
    stmts.push(PyStmt::Assign {
        target: target.to_string(),
        value,
    });
    stmts
}
