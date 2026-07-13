//! Specialization of statically-known `Decode` decoders (`DESIGN.md` §5.3).
//!
//! `Decode.decodeString dec s` normally builds a runtime decoder *value* — a
//! tree of raising closures — and interprets it over `json.loads` output. When
//! `dec` is a syntactically-known composition of the simple combinators
//! (`string`/`int`/`float`/`bool`/`field`/`list`/`nullable`/`map`–`map4`/
//! `succeed`/`fail`/`oneOf`, possibly through top-level `let` decoder names),
//! this pass deforests the interpreter: it emits the equivalent **direct
//! dict/list access with inline error handling** — one `try` whose body reads
//! like a hand-written validating parser — producing a `Result` **byte-identical**
//! to the interpreter's (same `Ok` payload, same `Error(_Exception(kind, msg))`
//! on every failure, including `KeyError`'s quoted repr for a missing field).
//!
//! Correctness mirrors the fold pass (`fold_loop.rs`): a pure syntactic
//! analysis ([`Lowerer::classify_decoder`], no side effects) that rejects — and
//! falls back to the interpreter byte-identically — on anything dynamic
//! (`andThen`, a decoder passed as a value, a local/foreign decoder name, a
//! non-literal `oneOf` list, a shadowed free variable of a named decoder, a
//! cyclic reference). `PYFUN_NO_DECODE_OPT` is the kill switch for differential
//! testing.
//!
//! Faithfulness notes, matching the interpreter exactly:
//! - *Configuration expressions* (a `map` fan-in function, a `succeed` value, a
//!   `fail` message, a `field` name) are evaluated when the interpreter
//!   **builds** the decoder — outside `decodeString`'s try — so they are hoisted
//!   before the emitted `try`, in construction (pre-)order.
//! - Every node decodes from a **bound temp**, so a `v[name]` subscript
//!   evaluates exactly once (its `KeyError` is the missing-field error).
//! - `oneOf` catches `Exception` per alternative and exhausts to
//!   `ValueError("Decode.oneOf: no decoder matched")`, exactly like the
//!   recursive `_try` helper.

use std::collections::HashSet;

use crate::parser::ast::{Expr, ExprKind};
use crate::python_emitter::{PyBinOp, PyExpr, PyStmt};

use super::{LowerError, Lowerer};

/// A statically-classified decoder. Configuration expressions stay unlowered
/// until emission; they are owned clones, since a named decoder's body is
/// resolved out of the lowerer's own registry (`top_val_defs`).
enum DecPlan {
    Str,
    Int,
    Float,
    Bool,
    Field {
        name: Expr,
        inner: Box<DecPlan>,
    },
    List(Box<DecPlan>),
    Nullable(Box<DecPlan>),
    /// `map`/`map2`/`map3`/`map4` — sub-decode each argument (in order, from
    /// the same input), then apply the fan-in function.
    Map {
        f: Expr,
        args: Vec<DecPlan>,
    },
    Succeed(Expr),
    Fail(Expr),
    OneOf(Vec<DecPlan>),
}

impl Lowerer {
    /// Try to specialize a fully-applied `Decode.decodeString dec s`
    /// (`args = [dec, s]`). Returns `Ok(None)` — fall back to the interpreter —
    /// on any failed precondition, with no side effects on the lowerer.
    pub(super) fn try_lower_decode_spec(
        &mut self,
        args_ast: &[&Expr],
        locals: &HashSet<String>,
    ) -> Result<Option<(Vec<PyStmt>, PyExpr)>, LowerError> {
        // An in-file `module`'s name mangling would apply inconsistently to
        // inlined configuration expressions — reject (as the fold pass does).
        if self.cur_module.is_some() {
            return Ok(None);
        }
        let [dec, s] = args_ast else {
            return Ok(None);
        };
        // Names bound anywhere in the enclosing frames: a named decoder's free
        // variables must not collide (they resolve at top level in the
        // interpreter, but would resolve to the local after inlining).
        let mut enclosing = locals.clone();
        for frame in &self.fn_local_stack {
            enclosing.extend(frame.iter().cloned());
        }
        let mut visiting = HashSet::new();
        let Some(plan) = self.classify_decoder(dec, locals, &enclosing, &mut visiting) else {
            return Ok(None);
        };
        let lowered = self.emit_decode(&plan, s, locals)?;
        Ok(Some(lowered))
    }

    /// The pure analysis: classify `e` as a known combinator composition, or
    /// `None`. A bare `Var` resolves through the top-level value registry
    /// (`top_val_defs`) with a cycle guard; anything dynamic rejects.
    fn classify_decoder(
        &self,
        e: &Expr,
        locals: &HashSet<String>,
        enclosing: &HashSet<String>,
        visiting: &mut HashSet<String>,
    ) -> Option<DecPlan> {
        // A named top-level decoder (`let user = Decode.map2 …`): inline its
        // body. Its free variables resolve at top level, so any enclosing local
        // of the same name would capture them after inlining — reject.
        if let ExprKind::Var(name) = &e.kind {
            if locals.contains(name) || visiting.contains(name) {
                return None;
            }
            let body = self.top_val_defs.get(name)?.clone();
            let mut free = HashSet::new();
            super::fold_loop::collect_free(&body, &HashSet::new(), &mut free);
            if !free.is_disjoint(enclosing) {
                return None;
            }
            visiting.insert(name.clone());
            let plan = self.classify_decoder(&body, locals, enclosing, visiting);
            visiting.remove(name);
            return plan;
        }
        let mut args = Vec::new();
        let head = super::flatten_app(e, &mut args);
        let q = crate::types::qualified_name(head)?;
        match (q.as_str(), args.len()) {
            ("Decode.string", 0) => Some(DecPlan::Str),
            ("Decode.int", 0) => Some(DecPlan::Int),
            ("Decode.float", 0) => Some(DecPlan::Float),
            ("Decode.bool", 0) => Some(DecPlan::Bool),
            ("Decode.field", 2) => Some(DecPlan::Field {
                name: args[0].clone(),
                inner: Box::new(self.classify_decoder(args[1], locals, enclosing, visiting)?),
            }),
            ("Decode.list", 1) => Some(DecPlan::List(Box::new(
                self.classify_decoder(args[0], locals, enclosing, visiting)?,
            ))),
            ("Decode.nullable", 1) => Some(DecPlan::Nullable(Box::new(
                self.classify_decoder(args[0], locals, enclosing, visiting)?,
            ))),
            ("Decode.succeed", 1) => Some(DecPlan::Succeed(args[0].clone())),
            ("Decode.fail", 1) => Some(DecPlan::Fail(args[0].clone())),
            ("Decode.map", 2) | ("Decode.map2", 3) | ("Decode.map3", 4) | ("Decode.map4", 5) => {
                let mut sub_plans = Vec::with_capacity(args.len() - 1);
                for a in &args[1..] {
                    sub_plans.push(self.classify_decoder(a, locals, enclosing, visiting)?);
                }
                Some(DecPlan::Map {
                    f: args[0].clone(),
                    args: sub_plans,
                })
            }
            ("Decode.oneOf", 1) => {
                let ExprKind::List { elems } = &args[0].kind else {
                    return None;
                };
                let mut alts = Vec::with_capacity(elems.len());
                for a in elems {
                    alts.push(self.classify_decoder(a, locals, enclosing, visiting)?);
                }
                Some(DecPlan::OneOf(alts))
            }
            // `Decode.andThen`, a partial application, a decoder-typed
            // parameter, anything else: dynamic — interpreter.
            _ => None,
        }
    }

    /// Emit the specialized decode. Configuration expressions are lowered and
    /// hoisted first (construction order, outside the `try`), then the subject
    /// string, then the `try` whose body decodes `json.loads(s)` and whose
    /// handler folds any raise into `Error(_Exception(...))` — exactly
    /// `_pf_dec_decode_string`'s boundary.
    fn emit_decode(
        &mut self,
        plan: &DecPlan,
        s_expr: &Expr,
        locals: &HashSet<String>,
    ) -> Result<(Vec<PyStmt>, PyExpr), LowerError> {
        self.needs_result = true;
        self.needs_exception = true;
        self.needed_imports.insert("json".to_string());

        let mut stmts = Vec::new();
        let mut configs = Vec::new();
        self.lower_configs(plan, locals, &mut stmts, &mut configs)?;
        let mut configs = configs.into_iter();

        let (s_stmts, s_val) = self.lower_value(s_expr, locals)?;
        stmts.extend(s_stmts);

        // try: v = json.loads(s); …decode…; r = Ok(result)
        let parsed = self.fresh_tmp();
        let mut body = vec![PyStmt::Assign {
            target: parsed.clone(),
            value: PyExpr::Call {
                func: Box::new(PyExpr::Attribute {
                    value: Box::new(PyExpr::Name("json".to_string())),
                    attr: "loads".to_string(),
                }),
                args: vec![s_val],
            },
        }];
        let decoded = self.emit_plan(plan, &parsed, &mut body, &mut configs);
        let result = self.fresh_tmp();
        body.push(PyStmt::Assign {
            target: result.clone(),
            value: call("Ok", vec![decoded]),
        });
        // except Exception as e: r = Error(_Exception(type(e).__name__, str(e)))
        let handler = vec![PyStmt::Assign {
            target: result.clone(),
            value: call(
                "Error",
                vec![call(
                    "_Exception",
                    vec![
                        PyExpr::Attribute {
                            value: Box::new(call("type", vec![PyExpr::Name("e".to_string())])),
                            attr: "__name__".to_string(),
                        },
                        call("str", vec![PyExpr::Name("e".to_string())]),
                    ],
                )],
            ),
        }];
        stmts.push(PyStmt::Try {
            body,
            exc_type: Some("Exception".to_string()),
            binding: Some("e".to_string()),
            handler,
        });
        Ok((stmts, PyExpr::Name(result)))
    }

    /// Lower every configuration expression in construction (pre-)order — the
    /// order the interpreter's factory calls would evaluate them — hoisting
    /// each non-atomic one to a temp so it runs outside the `try`, once.
    fn lower_configs(
        &mut self,
        plan: &DecPlan,
        locals: &HashSet<String>,
        out: &mut Vec<PyStmt>,
        configs: &mut Vec<PyExpr>,
    ) -> Result<(), LowerError> {
        let cfg = |this: &mut Self, e: &Expr, out: &mut Vec<PyStmt>, configs: &mut Vec<PyExpr>| {
            let (s, v) = this.lower_value(e, locals)?;
            out.extend(s);
            let v = if is_atomic(&v) {
                v
            } else {
                let tmp = this.fresh_tmp();
                out.push(PyStmt::Assign {
                    target: tmp.clone(),
                    value: v,
                });
                PyExpr::Name(tmp)
            };
            configs.push(v);
            Ok::<(), LowerError>(())
        };
        match plan {
            DecPlan::Str | DecPlan::Int | DecPlan::Float | DecPlan::Bool => {}
            DecPlan::Field { name, inner } => {
                cfg(self, name, out, configs)?;
                self.lower_configs(inner, locals, out, configs)?;
            }
            DecPlan::List(inner) | DecPlan::Nullable(inner) => {
                self.lower_configs(inner, locals, out, configs)?;
            }
            DecPlan::Map { f, args } => {
                cfg(self, f, out, configs)?;
                for a in args {
                    self.lower_configs(a, locals, out, configs)?;
                }
            }
            DecPlan::Succeed(x) | DecPlan::Fail(x) => cfg(self, x, out, configs)?,
            DecPlan::OneOf(alts) => {
                for a in alts {
                    self.lower_configs(a, locals, out, configs)?;
                }
            }
        }
        Ok(())
    }

    /// Emit the decode of `plan` from the temp `input`, appending statements and
    /// returning the decoded-value expression. `configs` yields the lowered
    /// configuration expressions in the same pre-order as [`Lowerer::lower_configs`].
    fn emit_plan(
        &mut self,
        plan: &DecPlan,
        input: &str,
        out: &mut Vec<PyStmt>,
        configs: &mut std::vec::IntoIter<PyExpr>,
    ) -> PyExpr {
        let in_name = || PyExpr::Name(input.to_string());
        // `if not <test>: raise ValueError(<msg expr>)`.
        let guard = |test: PyExpr, msg: PyExpr| PyStmt::If {
            test: PyExpr::Not(Box::new(test)),
            body: vec![PyStmt::Raise(call("ValueError", vec![msg]))],
            orelse: vec![],
        };
        // "expected <noun>, got " + type(v).__name__ — the strict primitives'
        // message, byte-for-byte.
        let got = |noun: &str| PyExpr::BinOp {
            op: PyBinOp::Add,
            left: Box::new(PyExpr::Str(format!("expected {noun}, got "))),
            right: Box::new(PyExpr::Attribute {
                value: Box::new(call("type", vec![in_name()])),
                attr: "__name__".to_string(),
            }),
        };
        let isinst = |ty: &str| call("isinstance", vec![in_name(), PyExpr::Name(ty.to_string())]);
        match plan {
            DecPlan::Str => {
                out.push(guard(isinst("str"), got("a string")));
                in_name()
            }
            DecPlan::Int => {
                out.push(guard(
                    PyExpr::BinOp {
                        op: PyBinOp::And,
                        left: Box::new(isinst("int")),
                        right: Box::new(PyExpr::Not(Box::new(isinst("bool")))),
                    },
                    got("an int"),
                ));
                in_name()
            }
            DecPlan::Float => {
                out.push(guard(
                    PyExpr::BinOp {
                        op: PyBinOp::And,
                        left: Box::new(call(
                            "isinstance",
                            vec![
                                in_name(),
                                PyExpr::Tuple(vec![
                                    PyExpr::Name("int".to_string()),
                                    PyExpr::Name("float".to_string()),
                                ]),
                            ],
                        )),
                        right: Box::new(PyExpr::Not(Box::new(isinst("bool")))),
                    },
                    got("a float"),
                ));
                call("float", vec![in_name()])
            }
            DecPlan::Bool => {
                out.push(guard(isinst("bool"), got("a bool")));
                in_name()
            }
            DecPlan::Field { inner, .. } => {
                let name_cfg = configs.next().expect("config walk is aligned");
                out.push(guard(
                    isinst("dict"),
                    PyExpr::Str("expected a JSON object".to_string()),
                ));
                // t = v[name] — bound once; a missing field's KeyError is the error.
                let t = self.fresh_tmp();
                out.push(PyStmt::Assign {
                    target: t.clone(),
                    value: PyExpr::Subscript {
                        value: Box::new(in_name()),
                        index: Box::new(name_cfg),
                    },
                });
                self.emit_plan(inner, &t, out, configs)
            }
            DecPlan::List(inner) => {
                out.push(guard(
                    isinst("list"),
                    PyExpr::Str("expected a JSON array".to_string()),
                ));
                let acc = self.fresh_tmp();
                out.push(PyStmt::Assign {
                    target: acc.clone(),
                    value: PyExpr::List(vec![]),
                });
                let item = self.fresh_tmp();
                let mut body = Vec::new();
                let decoded = self.emit_plan(inner, &item, &mut body, configs);
                body.push(PyStmt::Expr(PyExpr::Call {
                    func: Box::new(PyExpr::Attribute {
                        value: Box::new(PyExpr::Name(acc.clone())),
                        attr: "append".to_string(),
                    }),
                    args: vec![decoded],
                }));
                out.push(PyStmt::For {
                    target: item,
                    iter: in_name(),
                    body,
                });
                PyExpr::Name(acc)
            }
            DecPlan::Nullable(inner) => {
                self.needs_option = true;
                let r = self.fresh_tmp();
                let mut orelse = Vec::new();
                let decoded = self.emit_plan(inner, input, &mut orelse, configs);
                orelse.push(PyStmt::Assign {
                    target: r.clone(),
                    value: call("Some", vec![decoded]),
                });
                out.push(PyStmt::If {
                    test: PyExpr::BinOp {
                        op: PyBinOp::Is,
                        left: Box::new(in_name()),
                        right: Box::new(PyExpr::NoneLit),
                    },
                    body: vec![PyStmt::Assign {
                        target: r.clone(),
                        value: call("None_", vec![]),
                    }],
                    orelse,
                });
                PyExpr::Name(r)
            }
            DecPlan::Map { args, .. } => {
                let f_cfg = configs.next().expect("config walk is aligned");
                let mut decoded = Vec::with_capacity(args.len());
                for a in args {
                    decoded.push(self.emit_plan(a, input, out, configs));
                }
                PyExpr::Call {
                    func: Box::new(f_cfg),
                    args: decoded,
                }
            }
            DecPlan::Succeed(_) => configs.next().expect("config walk is aligned"),
            DecPlan::Fail(_) => {
                let msg = configs.next().expect("config walk is aligned");
                out.push(PyStmt::Raise(call("ValueError", vec![msg])));
                // Unreachable, but the node needs a value expression.
                PyExpr::NoneLit
            }
            DecPlan::OneOf(alts) => {
                let r = self.fresh_tmp();
                // Innermost handler: every alternative failed.
                let mut handler = vec![PyStmt::Raise(call(
                    "ValueError",
                    vec![PyExpr::Str("Decode.oneOf: no decoder matched".to_string())],
                ))];
                // Consume configs in alternative order, then nest the tries
                // back-to-front so alt 1 is outermost (tried first).
                let mut lowered_alts = Vec::with_capacity(alts.len());
                for alt in alts {
                    let mut body = Vec::new();
                    let decoded = self.emit_plan(alt, input, &mut body, configs);
                    body.push(PyStmt::Assign {
                        target: r.clone(),
                        value: decoded,
                    });
                    lowered_alts.push(body);
                }
                for body in lowered_alts.into_iter().rev() {
                    handler = vec![PyStmt::Try {
                        body,
                        exc_type: Some("Exception".to_string()),
                        binding: None,
                        handler,
                    }];
                }
                // The outermost layer is the first alternative's try (or, for an
                // empty `oneOf`, the bare raise — exactly the interpreter).
                out.extend(handler);
                PyExpr::Name(r)
            }
        }
    }
}

fn call(f: &str, args: Vec<PyExpr>) -> PyExpr {
    PyExpr::Call {
        func: Box::new(PyExpr::Name(f.to_string())),
        args,
    }
}

/// An atomic expression that may be used in place without hoisting (mirrors
/// `fold_loop::is_atomic`).
fn is_atomic(e: &PyExpr) -> bool {
    match e {
        PyExpr::Name(_)
        | PyExpr::Int(_)
        | PyExpr::Float(_)
        | PyExpr::Str(_)
        | PyExpr::Bool(_)
        | PyExpr::NoneLit => true,
        PyExpr::Attribute { value, .. } => is_atomic(value),
        _ => false,
    }
}
