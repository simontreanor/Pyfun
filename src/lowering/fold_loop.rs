//! In-place linear accumulation for `Seq.fold` / `List.fold` (`DESIGN.md` §5) —
//! Tier 1 plus the Tier B coverage extensions.
//!
//! A qualifying fully-applied fold whose accumulator is threaded linearly
//! through the folder is rewritten from `functools.reduce(f, xs, acc)` — which
//! rebuilds a fresh container every step (O(n²) build-by-concat) — into a
//! `for`-loop over a **mutable** accumulator, turning copy-returning ops into
//! in-place mutations (`Map.add`→`m[k]=v`, `Map.remove`→`.pop(k, None)`,
//! `List.concat`→`.append`/`.extend`, `Set.add`→`.add`, `Set.remove`→
//! `.discard`). The folder may be a lambda, a top-level named `let`, or a
//! **block-local** named `let` (`local_fn_defs`, kept shadow-coherent in
//! `mod.rs`); a tail leaf may **chain** updates in one slot and **reset** a slot
//! to a fresh init; an init slot may be a named binding, bound via a defensive
//! shallow copy (mutated slots) or an alias (untouched slots).
//!
//! Correctness is absolute: the analysis is a set of conservative *syntactic*
//! proof obligations (P1–P11 in the memo) evaluated with **no** side effects on
//! the lowerer, so anything that does not provably qualify falls through to the
//! existing `_pf_fold` lowering **byte-identically**. When in doubt we reject.
//!
//! The pass runs in two phases so a rejected fold leaves zero residue (no helper
//! registration, no temp-counter churn):
//!   1. [`Lowerer::plan_fold`] — pure analysis over the AST, returns a [`FoldPlan`]
//!      or `None`.
//!   2. [`Lowerer::emit_fold_loop`] — the actual lowering, only reached on success.

use std::collections::HashSet;

use crate::parser::ast::{BlockStmt, CeItem, Expr, ExprKind, InterpPart, Param, Pattern};
use crate::python_emitter::{PyCase, PyExpr, PyStmt};

use super::{LowerError, Lowered, Lowerer};

/// One recognized in-place update at a fold tail leaf, with the (still-unlowered)
/// argument expressions the mutation needs.
enum SlotOp<'a> {
    /// `Map.add k v M` → `M[k] = v`.
    MapAdd { key: &'a Expr, val: &'a Expr },
    /// `Map.remove k M` → `M.pop(k, None)` (mirrors `_pf_map_remove`).
    MapRemove { key: &'a Expr },
    /// `List.concat M [e]` → `M.append(e)`.
    ListAppend { elem: &'a Expr },
    /// `List.concat M ys` → `M.extend(ys)` (ys mentions no sensitive name).
    ListExtend { ys: &'a Expr },
    /// `Set.add x M` → `M.add(x)`.
    SetAdd { elem: &'a Expr },
    /// `Set.remove x M` → `M.discard(x)` (mirrors `_pf_set_remove`).
    SetRemove { elem: &'a Expr },
}

/// One slot's plan at a tail leaf: an optional **fresh reset** as the chain base
/// (`(Map.empty, …)` — the slot is rebound to a fresh container; safe because the
/// loop's containers are never aliased, P11), then a **chain** of updates,
/// innermost-first (`Map.add k2 v2 (Map.add k1 v1 m)` → `m[k1]=v1; m[k2]=v2`).
/// `reset: None, ops: []` is a pass-through (the slot carries unchanged).
struct SlotPlan<'a> {
    reset: Option<&'a Expr>,
    ops: Vec<SlotOp<'a>>,
}

/// How a slot's accumulator is initialized before the loop.
#[derive(Clone, Copy, PartialEq)]
enum InitKind {
    /// A syntactically fresh literal (`Map.empty` / `Set.empty` / `[…]`).
    Fresh,
    /// A named binding the loop mutates in place — a **defensive shallow copy**
    /// (`dict(v)` / `list(v)` / `set(v)`; the constructor is inferred from the
    /// slot's update ops) keeps the original intact, exactly as the copy-returning
    /// reduce form would. The copied *values* stay shared, as they do under
    /// reduce (immutable Pyfun data).
    Copy(&'static str),
    /// A named binding the loop never mutates in place (pass-through / reset-only
    /// slots) — bound directly; the reduce form returns the same object.
    Alias,
}

/// The result of a successful analysis: everything [`Lowerer::emit_fold_loop`]
/// needs to emit the loop. Borrows the folder body (owned by the caller) and the
/// init/collection argument expressions.
pub(super) struct FoldPlan<'a> {
    /// A single-slot accumulator (vs a flat tuple of slots).
    single: bool,
    /// The folder's element parameter — becomes the loop variable.
    elem_param: String,
    /// The accumulator slot local names (length 1 for a single accumulator).
    slots: Vec<String>,
    /// The folder's accumulator parameter (the destructure scrutinee / single slot).
    acc_param: String,
    /// The init expression for each slot (parallel to `slots`).
    inits: Vec<&'a Expr>,
    /// How each slot's init binds (fresh literal / defensive copy / alias),
    /// parallel to `inits`.
    init_kinds: Vec<InitKind>,
    /// The collection folded over.
    xs: &'a Expr,
    /// The folder body to inline as the loop body (the destructure arm body for a
    /// tuple accumulator, the whole folder body for a single accumulator).
    step_body: &'a Expr,
    /// The call-site locals — used to lower the init and collection arguments.
    site_locals: HashSet<String>,
    /// The locals the inlined body is lowered under (the folder's own scope).
    base_locals: HashSet<String>,
}

impl Lowerer {
    /// Try to rewrite a fully-applied `Seq.fold`/`List.fold` (`args = [folder,
    /// init, xs]`) into an in-place loop. Returns `Ok(None)` — fall back to the
    /// `_pf_fold` lowering — on any failed precondition, with no side effects.
    pub(super) fn try_lower_fold_loop(
        &mut self,
        args_ast: &[&Expr],
        locals: &HashSet<String>,
    ) -> Result<Option<(Vec<PyStmt>, PyExpr)>, LowerError> {
        // P8: an in-file `module`'s name mangling would apply inconsistently to an
        // inlined body — reject inside a module.
        if self.cur_module.is_some() {
            return Ok(None);
        }
        let [folder, init, xs] = args_ast else {
            return Ok(None);
        };
        // P2: the folder must be an in-place 2-ary lambda literal, a bare
        // reference to a top-level 2-ary `let` (`top_fn_defs`), or a bare
        // reference to a **block-local** 2-ary `let` (`local_fn_defs` — kept
        // shadow-coherent by the block/param/arm hooks in `mod.rs`, so a hit is
        // always the innermost binding). Anything else (a `mut`, an extern, an
        // imported member, a parameter) has no inlinable body — reject.
        //
        // `same_frame` marks a folder whose free variables already resolve in the
        // call site's own Python frame — a lambda (by construction) or a local
        // named folder (its `def` and the fold share one frame, and Pyfun `let`
        // shadowing lowers to reassignment of the same Python name, so inlining
        // preserves resolution). A *top-level* named folder's frees resolve at
        // module scope instead, so it keeps the P8 free-variable disjointness
        // check below.
        let (params, body, same_frame): (Vec<Param>, Expr, bool) = match &folder.kind {
            ExprKind::Fn { params, body } if params.len() == 2 => {
                (params.clone(), (**body).clone(), true)
            }
            ExprKind::Var(f) if locals.contains(f) => match self.local_fn_defs.get(f) {
                Some((ps, b)) if ps.len() == 2 => (ps.clone(), b.clone(), true),
                _ => return Ok(None),
            },
            ExprKind::Var(f) => match self.top_fn_defs.get(f) {
                Some((ps, b)) if ps.len() == 2 => (ps.clone(), b.clone(), false),
                _ => return Ok(None),
            },
            _ => return Ok(None),
        };
        // P8: names that would rebind under inlining — the call-site locals plus
        // every enclosing function frame (Python locals are function-wide, so a
        // name assigned *later* in an enclosing function still shadows).
        let mut enclosing = locals.clone();
        for frame in &self.fn_local_stack {
            enclosing.extend(frame.iter().cloned());
        }
        let Some(plan) = self.plan_fold(&params, &body, init, xs, same_frame, locals, &enclosing)
        else {
            return Ok(None);
        };
        let lowered = self.emit_fold_loop(&plan)?;
        Ok(Some(lowered))
    }

    /// The pure analysis (P3–P8). Borrows only the AST — no `self` mutation — so a
    /// `None` result leaves the lowerer untouched.
    #[allow(clippy::too_many_arguments)]
    fn plan_fold<'a>(
        &self,
        params: &[Param],
        body: &'a Expr,
        init: &'a Expr,
        xs: &'a Expr,
        same_frame: bool,
        locals: &HashSet<String>,
        enclosing: &HashSet<String>,
    ) -> Option<FoldPlan<'a>> {
        let acc_param = params[0].name.clone();
        let elem_param = params[1].name.clone();

        // P3 + P4: classify the accumulator shape. Each slot's init must be a
        // fresh literal (`Map.empty`/`Set.empty`/a list literal) or a bare `Var`
        // (bound by a defensive copy or an alias — classified after the tail
        // walk below, which reveals whether the slot is mutated in place).
        let (single, slots, step_body, inits): (bool, Vec<String>, &'a Expr, Vec<&'a Expr>) =
            if let Some((names, arm_body)) = tuple_destructure(body, &acc_param) {
                let ExprKind::Tuple { elems } = &init.kind else {
                    return None;
                };
                if elems.len() != names.len() || !elems.iter().all(is_allowed_init) {
                    return None;
                }
                (false, names, arm_body, elems.iter().collect())
            } else {
                if !is_allowed_init(init) {
                    return None;
                }
                (true, vec![acc_param.clone()], body, vec![init])
            };

        // Sensitive names: the accumulator parameter and every slot local.
        let mut sensitive: HashSet<String> = slots.iter().cloned().collect();
        sensitive.insert(acc_param.clone());

        // Frame-level binders the body introduces (not descending into nested
        // functions/CEs, which are their own Python scope).
        let mut binders = HashSet::new();
        collect_frame_binders(step_body, &mut binders);
        // A body binder that shadows a sensitive name would break the name-based
        // slot classification — reject (freshening is punted).
        if !binders.is_disjoint(&sensitive) {
            return None;
        }
        // P8: the loop var, slots, and body binders must not collide with an
        // enclosing local (inlining would clobber it).
        let mut introduced = binders;
        introduced.extend(slots.iter().cloned());
        introduced.insert(elem_param.clone());
        if !introduced.is_disjoint(enclosing) {
            return None;
        }

        // P8 (free-var half): a TOP-LEVEL named folder's free variable that is
        // also an enclosing local would resolve to the local at runtime after
        // inlining (A9). A lambda's — or a block-local folder's — free vars are
        // the call site's own frame by construction, so the check is skipped.
        if !same_frame {
            let mut bound: HashSet<String> = sensitive.clone();
            bound.insert(elem_param.clone());
            let mut free = HashSet::new();
            collect_free(body, &bound, &mut free);
            if !free.is_disjoint(enclosing) {
                return None;
            }
        }

        // P5/P6/P7: occurrence discipline + tail-leaf grammar.
        if !self.validate_fold_tail(step_body, &slots, &acc_param, single, &sensitive) {
            return None;
        }

        // P4 (second half): classify each slot's init. A `Var` init needs the
        // tail walk's verdict on whether the slot is mutated in place — if so, a
        // defensive shallow copy whose constructor comes from the op family; if
        // not (pass-through / reset-only), a plain alias. A `Var` init whose name
        // is one of the loop-introduced names would read the slot before its
        // first assignment (Python's function-wide locals) — reject.
        let ctors = slot_ctors(step_body, &slots, &acc_param, single)?;
        let mut init_kinds = Vec::with_capacity(inits.len());
        for (i, e) in inits.iter().enumerate() {
            let kind = match &e.kind {
                ExprKind::Var(v) => {
                    if introduced.contains(v) {
                        return None;
                    }
                    match ctors[i] {
                        Some(ctor) => InitKind::Copy(ctor),
                        None => InitKind::Alias,
                    }
                }
                _ => InitKind::Fresh,
            };
            init_kinds.push(kind);
        }

        // Locals for lowering the inlined body: the folder's own scope. A
        // top-level named folder sees only its params + slots (matching its own
        // `def`, so `lower_var` rerouting is identical); a lambda or local folder
        // additionally sees the call site's locals (where its free vars resolve).
        let mut base_locals: HashSet<String> = sensitive.clone();
        base_locals.insert(elem_param.clone());
        if same_frame {
            base_locals.extend(locals.iter().cloned());
        }

        Some(FoldPlan {
            single,
            elem_param,
            slots,
            acc_param,
            inits,
            init_kinds,
            xs,
            step_body,
            site_locals: locals.clone(),
            base_locals,
        })
    }

    /// P5/P6: walk the folder body in tail position, validating that every tail
    /// leaf is a whitelisted update/pass-through and every value-position
    /// occurrence of a sensitive name is a safe read. Pure (`&self` for the
    /// active-pattern registry only).
    fn validate_fold_tail(
        &self,
        body: &Expr,
        slots: &[String],
        acc_param: &str,
        single: bool,
        sensitive: &HashSet<String>,
    ) -> bool {
        match &body.kind {
            ExprKind::If { cond, then, else_ } => {
                value_ok(cond, sensitive)
                    && self.validate_fold_tail(then, slots, acc_param, single, sensitive)
                    && self.validate_fold_tail(else_, slots, acc_param, single, sensitive)
            }
            ExprKind::Match { scrutinee, arms } => {
                // An active-pattern match lowers to an if/elif chain, not handled by
                // the step lowering — reject (matches `lower_fold_step`).
                if self.match_uses_ap(arms) {
                    return false;
                }
                value_ok(scrutinee, sensitive)
                    && arms.iter().all(|a| {
                        a.guard.as_ref().is_none_or(|g| value_ok(g, sensitive))
                            && self.validate_fold_tail(&a.body, slots, acc_param, single, sensitive)
                    })
            }
            ExprKind::Block { stmts } => {
                // The final statement is the block's value (tail); the grammar
                // guarantees it is an expression.
                if !matches!(stmts.last(), Some(BlockStmt::Expr(_))) {
                    return false;
                }
                let last = stmts.len() - 1;
                stmts.iter().enumerate().all(|(i, s)| match s {
                    // A *parameterized* local `let` is a nested function: its body
                    // defers like a lambda, so a captured sensitive name would
                    // observe a future mutation — hold it to the closure rule
                    // (no sensitive mention at all), not the immediate-read rule.
                    BlockStmt::Let(b) if !b.params.is_empty() => {
                        !mentions_sensitive(&b.value, sensitive)
                    }
                    BlockStmt::Let(b) => value_ok(&b.value, sensitive),
                    BlockStmt::Expr(e) if i == last => {
                        self.validate_fold_tail(e, slots, acc_param, single, sensitive)
                    }
                    BlockStmt::Expr(e) => value_ok(e, sensitive),
                })
            }
            _ => validate_leaf(body, slots, acc_param, single, sensitive),
        }
    }

    /// The transform (only reached on a successful [`Lowerer::plan_fold`]).
    fn emit_fold_loop(&mut self, plan: &FoldPlan) -> Lowered {
        let mut stmts = Vec::new();
        // The accumulator per slot (P4), lowered under the site locals: a fresh
        // literal binds directly, a mutated `Var` binds a defensive shallow copy
        // (`dict(v)` — the original stays intact, exactly as under the
        // copy-returning reduce form), an unmutated `Var` binds as an alias.
        for ((slot, init), kind) in plan.slots.iter().zip(&plan.inits).zip(&plan.init_kinds) {
            let (s, v) = self.lower_value(init, &plan.site_locals)?;
            stmts.extend(s);
            let value = match kind {
                InitKind::Copy(ctor) => PyExpr::Call {
                    func: Box::new(PyExpr::Name((*ctor).to_string())),
                    args: vec![v],
                },
                InitKind::Fresh | InitKind::Alias => v,
            };
            stmts.push(PyStmt::Assign {
                target: slot.clone(),
                value,
            });
        }
        // The collection, evaluated once after the inits (P10), under site locals.
        let (xs_stmts, iter) = self.lower_value(plan.xs, &plan.site_locals)?;
        stmts.extend(xs_stmts);
        // The inlined folder body as the loop body.
        let body = self.lower_fold_step(
            plan.step_body,
            &plan.base_locals,
            &plan.slots,
            &plan.acc_param,
            plan.single,
        )?;
        stmts.push(PyStmt::For {
            target: plan.elem_param.clone(),
            iter,
            body,
        });
        // The result: the mutated accumulator(s) — a fresh container graph (P11).
        let value = if plan.single {
            PyExpr::Name(plan.slots[0].clone())
        } else {
            PyExpr::Tuple(plan.slots.iter().map(|s| PyExpr::Name(s.clone())).collect())
        };
        Ok((stmts, value))
    }

    /// Lower the inlined folder body as a sequence of loop-body statements,
    /// mirroring `lower_return` but emitting in-place mutations at the tail leaves.
    fn lower_fold_step(
        &mut self,
        body: &Expr,
        locals: &HashSet<String>,
        slots: &[String],
        acc_param: &str,
        single: bool,
    ) -> Result<Vec<PyStmt>, LowerError> {
        match &body.kind {
            ExprKind::If { cond, then, else_ } => {
                let (mut stmts, test) = self.lower_value(cond, locals)?;
                let body = self.lower_fold_step(then, locals, slots, acc_param, single)?;
                let orelse = self.lower_fold_step(else_, locals, slots, acc_param, single)?;
                stmts.push(PyStmt::If { test, body, orelse });
                Ok(stmts)
            }
            ExprKind::Match { scrutinee, arms } => {
                let (mut stmts, subject) = self.lower_value(scrutinee, locals)?;
                let mut cases = Vec::new();
                for arm in arms {
                    let pattern = self.lower_pattern(&arm.pattern);
                    let bindings = super::pattern_bindings(&arm.pattern);
                    let arm_locals = super::extend(locals, &bindings);
                    // Pattern binders shadow same-named local folders.
                    let shadowed = self.shadow_local_fns(&bindings);
                    let guard = self.lower_guard(&arm.guard, &arm_locals)?;
                    let body =
                        self.lower_fold_step(&arm.body, &arm_locals, slots, acc_param, single)?;
                    self.unshadow_local_fns(shadowed);
                    cases.push(PyCase {
                        pattern,
                        guard,
                        body,
                    });
                }
                if !super::has_catch_all(arms) {
                    cases.push(super::non_exhaustive_guard());
                }
                stmts.push(PyStmt::Match { subject, cases });
                Ok(stmts)
            }
            ExprKind::Block { stmts } => {
                let mut out = Vec::new();
                let mut locals = locals.clone();
                let last = stmts.len().saturating_sub(1);
                // Block scope for the local-folder registry (see `lower_block_return`).
                let saved_local_fns = self.local_fn_defs.clone();
                for (i, st) in stmts.iter().enumerate() {
                    match st {
                        BlockStmt::Let(b) => {
                            self.lower_let(b, &locals, &mut out)?;
                            self.note_block_let(b);
                            locals.insert(b.name.clone());
                        }
                        BlockStmt::Expr(e) if i == last => {
                            out.extend(self.lower_fold_step(e, &locals, slots, acc_param, single)?)
                        }
                        BlockStmt::Expr(e) => {
                            let (mut s, v) = self.lower_value(e, &locals)?;
                            out.append(&mut s);
                            if !matches!(v, PyExpr::NoneLit) {
                                out.push(PyStmt::Expr(v));
                            }
                        }
                    }
                }
                self.local_fn_defs = saved_local_fns;
                Ok(out)
            }
            _ => self.lower_fold_leaf(body, locals, slots, acc_param, single),
        }
    }

    /// Lower one tail leaf to its in-place mutation statements (P7). Every op
    /// argument is lowered (and hoisted to a temp when non-atomic) *first*, then
    /// the mutations are emitted in slot order — resets before their slot's ops,
    /// chain ops innermost-first. Sound because the reduce form never mutates
    /// anything: every read of a slot name there sees the iteration-start value,
    /// which is exactly what hoisting all reads before all mutations preserves
    /// (and the folder is pure, so hoist order among reads is unobservable).
    fn lower_fold_leaf(
        &mut self,
        leaf: &Expr,
        locals: &HashSet<String>,
        slots: &[String],
        acc_param: &str,
        single: bool,
    ) -> Result<Vec<PyStmt>, LowerError> {
        let plans = classify_leaf(leaf, slots, acc_param, single).ok_or_else(|| LowerError {
            message: "internal error: fold tail leaf unclassifiable after validation".to_string(),
        })?;

        enum LoweredMut {
            Reset(String, PyExpr),
            MapAdd(String, PyExpr, PyExpr),
            MapRemove(String, PyExpr),
            Append(String, PyExpr),
            Extend(String, PyExpr),
            SetAdd(String, PyExpr),
            SetRemove(String, PyExpr),
        }

        let mut pre = Vec::new();
        let mut lowered = Vec::new();
        for (i, plan) in plans.iter().enumerate() {
            let slot = &slots[i];
            if let Some(init) = plan.reset {
                let v = self.lower_hoist(init, &mut pre, locals)?;
                lowered.push(LoweredMut::Reset(slot.clone(), v));
            }
            for op in &plan.ops {
                match op {
                    SlotOp::MapAdd { key, val } => {
                        let k = self.lower_hoist(key, &mut pre, locals)?;
                        let v = self.lower_hoist_arg(val, &mut pre, locals, slots)?;
                        lowered.push(LoweredMut::MapAdd(slot.clone(), k, v));
                    }
                    SlotOp::MapRemove { key } => {
                        let k = self.lower_hoist(key, &mut pre, locals)?;
                        lowered.push(LoweredMut::MapRemove(slot.clone(), k));
                    }
                    SlotOp::ListAppend { elem } => {
                        let e = self.lower_hoist_arg(elem, &mut pre, locals, slots)?;
                        lowered.push(LoweredMut::Append(slot.clone(), e));
                    }
                    SlotOp::ListExtend { ys } => {
                        let y = self.lower_hoist(ys, &mut pre, locals)?;
                        lowered.push(LoweredMut::Extend(slot.clone(), y));
                    }
                    SlotOp::SetAdd { elem } => {
                        let x = self.lower_hoist_arg(elem, &mut pre, locals, slots)?;
                        lowered.push(LoweredMut::SetAdd(slot.clone(), x));
                    }
                    SlotOp::SetRemove { elem } => {
                        let x = self.lower_hoist(elem, &mut pre, locals)?;
                        lowered.push(LoweredMut::SetRemove(slot.clone(), x));
                    }
                }
            }
        }

        let mut out = pre;
        for m in lowered {
            match m {
                LoweredMut::Reset(slot, v) => out.push(PyStmt::Assign {
                    target: slot,
                    value: v,
                }),
                LoweredMut::MapAdd(slot, k, v) => out.push(PyStmt::SubscriptAssign {
                    obj: PyExpr::Name(slot),
                    index: k,
                    value: v,
                }),
                LoweredMut::MapRemove(slot, k) => {
                    out.push(method_call2(slot, "pop", k, Some(PyExpr::NoneLit)));
                }
                LoweredMut::Append(slot, e) => out.push(method_call2(slot, "append", e, None)),
                LoweredMut::Extend(slot, y) => out.push(method_call2(slot, "extend", y, None)),
                LoweredMut::SetAdd(slot, x) => out.push(method_call2(slot, "add", x, None)),
                LoweredMut::SetRemove(slot, x) => {
                    out.push(method_call2(slot, "discard", x, None));
                }
            }
        }
        Ok(out)
    }

    /// Lower an op argument, hoisting it to a fresh temp when it is not atomic (so
    /// whitelisted reads — which lower to non-atomic `_pf_*` calls — are always
    /// computed before the mutations; P7).
    fn lower_hoist(
        &mut self,
        e: &Expr,
        pre: &mut Vec<PyStmt>,
        locals: &HashSet<String>,
    ) -> Result<PyExpr, LowerError> {
        let (s, v) = self.lower_value(e, locals)?;
        pre.extend(s);
        if is_atomic(&v) {
            Ok(v)
        } else {
            let tmp = self.fresh_tmp();
            pre.push(PyStmt::Assign {
                target: tmp.clone(),
                value: v,
            });
            Ok(PyExpr::Name(tmp))
        }
    }

    /// [`Lowerer::lower_hoist`] for a *stored-value* position, which may legally
    /// be the whole object of a slot reset in the same leaf (`validate_leaf`).
    /// Such a bare slot name is **force-hoisted** to a temp even though a name is
    /// atomic: the mutation phase may rebind the slot (the reset) before this op
    /// runs, and the store must capture the pre-reset reference.
    fn lower_hoist_arg(
        &mut self,
        e: &Expr,
        pre: &mut Vec<PyStmt>,
        locals: &HashSet<String>,
        slots: &[String],
    ) -> Result<PyExpr, LowerError> {
        if let ExprKind::Var(v) = &e.kind
            && slots.contains(v)
        {
            let tmp = self.fresh_tmp();
            pre.push(PyStmt::Assign {
                target: tmp.clone(),
                value: PyExpr::Name(v.clone()),
            });
            return Ok(PyExpr::Name(tmp));
        }
        self.lower_hoist(e, pre, locals)
    }
}

/// `recv.method(arg[, arg2])` as a bare-expression statement.
fn method_call2(recv: String, method: &str, arg: PyExpr, arg2: Option<PyExpr>) -> PyStmt {
    let mut args = vec![arg];
    args.extend(arg2);
    PyStmt::Expr(PyExpr::Call {
        func: Box::new(PyExpr::Attribute {
            value: Box::new(PyExpr::Name(recv)),
            attr: method.to_string(),
        }),
        args,
    })
}

/// An atomic Python expression (no side effects, stable across mutations): a name,
/// a literal, or an attribute chain rooted at a name (a frozen-dataclass field
/// read). Anything else — notably a call (a whitelisted read) — is hoisted.
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

/// P3 (tuple case): recognize `match acc: case (v0, v1, …): body` — a single,
/// unguarded arm whose pattern is a tuple of distinct variable binders — and
/// return the slot names + the arm body. `None` for any other shape.
fn tuple_destructure<'a>(body: &'a Expr, acc_param: &str) -> Option<(Vec<String>, &'a Expr)> {
    let ExprKind::Match { scrutinee, arms } = &body.kind else {
        return None;
    };
    let ExprKind::Var(s) = &scrutinee.kind else {
        return None;
    };
    if s != acc_param || arms.len() != 1 {
        return None;
    }
    let arm = &arms[0];
    if arm.guard.is_some() {
        return None;
    }
    let Pattern::Tuple { elems } = &arm.pattern else {
        return None;
    };
    let mut names = Vec::with_capacity(elems.len());
    for el in elems {
        let Pattern::Var { name, .. } = el else {
            return None;
        };
        if names.contains(name) {
            return None; // slots must be distinct
        }
        names.push(name.clone());
    }
    Some((names, &arm.body))
}

/// P4: a syntactically fresh accumulator init — `Map.empty`, `Set.empty`, or a
/// list literal.
fn is_fresh_init(e: &Expr) -> bool {
    if matches!(&e.kind, ExprKind::List { .. }) {
        return true;
    }
    matches!(
        crate::types::qualified_name(e).as_deref(),
        Some("Map.empty") | Some("Set.empty")
    )
}

/// P4: an acceptable slot init — a fresh literal, or a bare `Var` (bound via a
/// defensive copy or an alias; classified after the tail walk in `plan_fold`).
/// Mutating a named binding *in place* would corrupt a value read after the
/// fold, so a `Var` init is never mutated directly — only its shallow copy is.
fn is_allowed_init(e: &Expr) -> bool {
    is_fresh_init(e) || matches!(&e.kind, ExprKind::Var(_))
}

/// P6: classify a tail leaf into per-slot update plans, or `None` if it is not a
/// valid tail form. `Var(acc)` (or an all-pass-through tuple) is a no-op.
fn classify_leaf<'a>(
    leaf: &'a Expr,
    slots: &[String],
    acc_param: &str,
    single: bool,
) -> Option<Vec<SlotPlan<'a>>> {
    // `Var(acc)` — the accumulator returned unchanged (tuple: all slots).
    if let ExprKind::Var(v) = &leaf.kind
        && v == acc_param
    {
        return Some(
            (0..slots.len())
                .map(|_| SlotPlan {
                    reset: None,
                    ops: vec![],
                })
                .collect(),
        );
    }
    if single {
        Some(vec![classify_slot(leaf, &slots[0])?])
    } else {
        let ExprKind::Tuple { elems } = &leaf.kind else {
            return None;
        };
        if elems.len() != slots.len() {
            return None;
        }
        let mut plans = Vec::with_capacity(elems.len());
        for (i, el) in elems.iter().enumerate() {
            // A slot returned bare must be *its own* slot (position-preserving) —
            // a swap `(b, a)` or duplication `(m, m)` rejects in `classify_slot`
            // (a foreign `Var` matches neither the slot nor any update form).
            plans.push(classify_slot(el, &slots[i])?);
        }
        Some(plans)
    }
}

/// Classify one slot's tail expression as a **chain** of whitelisted update ops
/// over a base that is either the slot itself (`Var(slot)`) or a fresh re-init
/// (`Map.add k v (Map.remove k0 m)`, `(Map.empty, runs)`, …). Each op's
/// collection argument must sit in exactly the position the copy helper mutates
/// (P6) — `List.concat [e] M` (prepend) is **not** an update. Ops come out
/// innermost-first, matching the value the copy chain would build.
fn classify_slot<'a>(e: &'a Expr, slot: &str) -> Option<SlotPlan<'a>> {
    if is_var(e, slot) {
        return Some(SlotPlan {
            reset: None,
            ops: vec![],
        });
    }
    if is_fresh_init(e) {
        return Some(SlotPlan {
            reset: Some(e),
            ops: vec![],
        });
    }
    let mut args = Vec::new();
    let head = super::flatten_app(e, &mut args);
    let q = crate::types::qualified_name(head)?;
    let (op, inner) = match q.as_str() {
        "Map.add" if args.len() == 3 => (
            SlotOp::MapAdd {
                key: args[0],
                val: args[1],
            },
            args[2],
        ),
        "Map.remove" if args.len() == 2 => (SlotOp::MapRemove { key: args[0] }, args[1]),
        "Set.add" if args.len() == 2 => (SlotOp::SetAdd { elem: args[0] }, args[1]),
        "Set.remove" if args.len() == 2 => (SlotOp::SetRemove { elem: args[0] }, args[1]),
        "List.concat" if args.len() == 2 => {
            let op = if let ExprKind::List { elems } = &args[1].kind
                && elems.len() == 1
            {
                SlotOp::ListAppend { elem: &elems[0] }
            } else {
                SlotOp::ListExtend { ys: args[1] }
            };
            (op, args[0])
        }
        _ => return None,
    };
    let mut plan = classify_slot(inner, slot)?;
    plan.ops.push(op);
    Some(plan)
}

/// P6 leaf validation: the leaf is classifiable *and* each op's non-collection
/// arguments are occurrence-safe (extend's `ys` must mention no sensitive name;
/// a reset's list-literal elements are ordinary value-position reads).
///
/// One deliberate exception — the **store-then-reset** pattern (`([], List.concat
/// done [cur])`, the batching idiom): a *stored value* position (`Map.add`'s
/// val, `List.concat`'s appended element, `Set.add`'s element) may be the whole
/// object of a slot **reset in the same leaf**. Sound because after the leaf the
/// slot aliases a fresh container, so the stored object is never mutated through
/// it again — exactly the object graph the copy-returning reduce form builds.
/// (Key positions stay conservative, and the emission force-hoists the old
/// reference before the reset rebinds the name.)
fn validate_leaf(
    leaf: &Expr,
    slots: &[String],
    acc_param: &str,
    single: bool,
    sensitive: &HashSet<String>,
) -> bool {
    let Some(plans) = classify_leaf(leaf, slots, acc_param, single) else {
        return false;
    };
    // Slots reset in this leaf — their whole objects may be stored (above).
    let reset_here: HashSet<&str> = plans
        .iter()
        .enumerate()
        .filter(|(_, p)| p.reset.is_some())
        .map(|(i, _)| slots[i].as_str())
        .collect();
    let storable = |e: &Expr| {
        matches!(&e.kind, ExprKind::Var(v) if reset_here.contains(v.as_str()))
            || value_ok(e, sensitive)
    };
    plans.iter().all(|plan| {
        plan.reset.is_none_or(|init| value_ok(init, sensitive))
            && plan.ops.iter().all(|op| match op {
                SlotOp::MapAdd { key, val } => value_ok(key, sensitive) && storable(val),
                SlotOp::MapRemove { key } => value_ok(key, sensitive),
                SlotOp::ListAppend { elem } | SlotOp::SetAdd { elem } => storable(elem),
                SlotOp::SetRemove { elem } => value_ok(elem, sensitive),
                SlotOp::ListExtend { ys } => !mentions_sensitive(ys, sensitive),
            })
    })
}

/// The in-place-mutation constructor each slot needs for a defensive copy
/// (`dict`/`list`/`set`), or `None` for a slot never mutated in place
/// (pass-through / reset-only). Walks the same tail spine as
/// `validate_fold_tail` (call only after it succeeds); a cross-family conflict
/// (impossible for well-typed code) rejects the whole plan.
fn slot_ctors(
    body: &Expr,
    slots: &[String],
    acc_param: &str,
    single: bool,
) -> Option<Vec<Option<&'static str>>> {
    fn walk(
        body: &Expr,
        slots: &[String],
        acc_param: &str,
        single: bool,
        out: &mut Vec<Option<&'static str>>,
    ) -> bool {
        match &body.kind {
            ExprKind::If { then, else_, .. } => {
                walk(then, slots, acc_param, single, out)
                    && walk(else_, slots, acc_param, single, out)
            }
            ExprKind::Match { arms, .. } => arms
                .iter()
                .all(|a| walk(&a.body, slots, acc_param, single, out)),
            ExprKind::Block { stmts } => match stmts.last() {
                Some(BlockStmt::Expr(e)) => walk(e, slots, acc_param, single, out),
                _ => false,
            },
            _ => {
                let Some(plans) = classify_leaf(body, slots, acc_param, single) else {
                    return false;
                };
                for (i, plan) in plans.iter().enumerate() {
                    for op in &plan.ops {
                        let ctor = match op {
                            SlotOp::MapAdd { .. } | SlotOp::MapRemove { .. } => "dict",
                            SlotOp::ListAppend { .. } | SlotOp::ListExtend { .. } => "list",
                            SlotOp::SetAdd { .. } | SlotOp::SetRemove { .. } => "set",
                        };
                        match out[i] {
                            None => out[i] = Some(ctor),
                            Some(prev) if prev == ctor => {}
                            Some(_) => return false,
                        }
                    }
                }
                true
            }
        }
    }
    let mut out = vec![None; slots.len()];
    walk(body, slots, acc_param, single, &mut out).then_some(out)
}

fn is_var(e: &Expr, name: &str) -> bool {
    matches!(&e.kind, ExprKind::Var(v) if v == name)
}

/// The whitelisted **read** ops (P5.5) and their full arity. Each returns a
/// scalar, an element, or a *fresh* copy — never the spine object — so the
/// collection can appear bare as the (last, curried) argument without retention.
fn read_op_arity(q: &str) -> Option<usize> {
    Some(match q {
        "Map.len" | "Map.keys" | "Map.values" | "Map.toList" => 1,
        "Map.tryFind" | "Map.contains" => 2,
        "Map.findOr" => 3,
        "Set.len" | "Set.toList" => 1,
        "Set.contains" => 2,
        "List.len" | "List.isEmpty" => 1,
        "List.contains" | "List.get" => 2,
        _ => return None,
    })
}

/// P5: is every occurrence of a sensitive name in this value-position expression
/// safe? A bare sensitive name is allowed **only** as the collection argument of a
/// whitelisted read; everything else rejects. A non-wildcard match so a future
/// `ExprKind` variant fails to compile rather than slipping through unchecked
/// (Risk #3).
fn value_ok(e: &Expr, sensitive: &HashSet<String>) -> bool {
    match &e.kind {
        ExprKind::Var(name) => !sensitive.contains(name),
        ExprKind::App { .. } | ExprKind::Pipe { .. } => value_ok_app(e, sensitive),
        // A closure / generator / composition / operator section defers
        // evaluation — a captured sensitive name would observe a future mutation.
        ExprKind::Fn { .. }
        | ExprKind::Ce { .. }
        | ExprKind::Compose { .. }
        | ExprKind::OpFunc(_) => !mentions_sensitive(e, sensitive),
        ExprKind::Int(_)
        | ExprKind::Float(_)
        | ExprKind::Str(_)
        | ExprKind::Bool(_)
        | ExprKind::Unit
        | ExprKind::Hole { .. } => true,
        ExprKind::Interp { parts } => parts.iter().all(|p| match p {
            InterpPart::Expr(e) => value_ok(e, sensitive),
            InterpPart::Lit(_) => true,
        }),
        ExprKind::If { cond, then, else_ } => {
            value_ok(cond, sensitive) && value_ok(then, sensitive) && value_ok(else_, sensitive)
        }
        ExprKind::Try { body } => value_ok(body, sensitive),
        ExprKind::Match { scrutinee, arms } => {
            value_ok(scrutinee, sensitive)
                && arms.iter().all(|a| {
                    a.guard.as_ref().is_none_or(|g| value_ok(g, sensitive))
                        && value_ok(&a.body, sensitive)
                })
        }
        ExprKind::Binary { lhs, rhs, .. } => value_ok(lhs, sensitive) && value_ok(rhs, sensitive),
        ExprKind::Unary { expr, .. } => value_ok(expr, sensitive),
        ExprKind::Compare { first, rest } => {
            value_ok(first, sensitive) && rest.iter().all(|(_, o)| value_ok(o, sensitive))
        }
        ExprKind::Block { stmts } => stmts.iter().all(|s| match s {
            BlockStmt::Let(b) => value_ok(&b.value, sensitive),
            BlockStmt::Expr(e) => value_ok(e, sensitive),
        }),
        ExprKind::List { elems } | ExprKind::Tuple { elems } => {
            elems.iter().all(|e| value_ok(e, sensitive))
        }
        ExprKind::Record { fields, .. } => fields.iter().all(|f| value_ok(&f.value, sensitive)),
        ExprKind::RecordUpdate { base, fields } => {
            value_ok(base, sensitive) && fields.iter().all(|f| value_ok(&f.value, sensitive))
        }
        ExprKind::Field { base, .. } => value_ok(base, sensitive),
        ExprKind::Annot { value, .. } => value_ok(value, sensitive),
        ExprKind::Assign { value, .. } => value_ok(value, sensitive),
    }
}

/// The application/pipe case of [`value_ok`]: a whitelisted read may consume the
/// spine object as its (bare) collection argument; any other application must be
/// entirely sensitive-free.
fn value_ok_app(e: &Expr, sensitive: &HashSet<String>) -> bool {
    let mut args = Vec::new();
    let head = super::flatten_app(e, &mut args);
    if let Some(q) = crate::types::qualified_name(head)
        && let Some(arity) = read_op_arity(&q)
        && args.len() == arity
    {
        // The collection is the last curried argument; it alone may be a bare
        // sensitive name. Every other argument must itself be occurrence-safe.
        let (coll, rest) = args.split_last().unwrap();
        if !rest.iter().all(|a| value_ok(a, sensitive)) {
            return false;
        }
        if let ExprKind::Var(n) = &coll.kind
            && sensitive.contains(n)
        {
            return true;
        }
        return value_ok(coll, sensitive);
    }
    // A generic application: no sensitive name may appear anywhere (passing the
    // spine object to an unknown callee could retain it).
    value_ok(head, sensitive) && args.iter().all(|a| value_ok(a, sensitive))
}

/// Does any sensitive name occur anywhere in this expression (including inside a
/// nested function/CE — the point of the check)? A non-wildcard match (Risk #3).
fn mentions_sensitive(e: &Expr, sensitive: &HashSet<String>) -> bool {
    match &e.kind {
        ExprKind::Var(name) => sensitive.contains(name),
        ExprKind::Int(_)
        | ExprKind::Float(_)
        | ExprKind::Str(_)
        | ExprKind::Bool(_)
        | ExprKind::Unit
        | ExprKind::Hole { .. }
        | ExprKind::OpFunc(_) => false,
        ExprKind::Interp { parts } => parts
            .iter()
            .any(|p| matches!(p, InterpPart::Expr(e) if mentions_sensitive(e, sensitive))),
        ExprKind::Fn { body, .. } => mentions_sensitive(body, sensitive),
        ExprKind::App { func, arg } => {
            mentions_sensitive(func, sensitive) || mentions_sensitive(arg, sensitive)
        }
        ExprKind::If { cond, then, else_ } => {
            mentions_sensitive(cond, sensitive)
                || mentions_sensitive(then, sensitive)
                || mentions_sensitive(else_, sensitive)
        }
        ExprKind::Try { body } => mentions_sensitive(body, sensitive),
        ExprKind::Match { scrutinee, arms } => {
            mentions_sensitive(scrutinee, sensitive)
                || arms.iter().any(|a| {
                    a.guard
                        .as_ref()
                        .is_some_and(|g| mentions_sensitive(g, sensitive))
                        || mentions_sensitive(&a.body, sensitive)
                })
        }
        ExprKind::Binary { lhs, rhs, .. }
        | ExprKind::Pipe { lhs, rhs, .. }
        | ExprKind::Compose { lhs, rhs, .. } => {
            mentions_sensitive(lhs, sensitive) || mentions_sensitive(rhs, sensitive)
        }
        ExprKind::Unary { expr, .. } | ExprKind::Annot { value: expr, .. } => {
            mentions_sensitive(expr, sensitive)
        }
        ExprKind::Compare { first, rest } => {
            mentions_sensitive(first, sensitive)
                || rest.iter().any(|(_, o)| mentions_sensitive(o, sensitive))
        }
        ExprKind::Ce { items, .. } => items
            .iter()
            .any(|it| mentions_sensitive(ce_item_value(it), sensitive)),
        ExprKind::List { elems } | ExprKind::Tuple { elems } => {
            elems.iter().any(|e| mentions_sensitive(e, sensitive))
        }
        ExprKind::Record { fields, .. } => fields
            .iter()
            .any(|f| mentions_sensitive(&f.value, sensitive)),
        ExprKind::RecordUpdate { base, fields } => {
            mentions_sensitive(base, sensitive)
                || fields
                    .iter()
                    .any(|f| mentions_sensitive(&f.value, sensitive))
        }
        ExprKind::Field { base, .. } => mentions_sensitive(base, sensitive),
        ExprKind::Block { stmts } => stmts.iter().any(|s| match s {
            BlockStmt::Let(b) => mentions_sensitive(&b.value, sensitive),
            BlockStmt::Expr(e) => mentions_sensitive(e, sensitive),
        }),
        ExprKind::Assign { value, .. } => mentions_sensitive(value, sensitive),
    }
}

/// The value expression carried by a computation-expression item.
fn ce_item_value(it: &CeItem) -> &Expr {
    match it {
        CeItem::LetBang { value, .. } | CeItem::Let { value, .. } => value,
        CeItem::DoBang(e)
        | CeItem::Return(e)
        | CeItem::ReturnBang(e)
        | CeItem::Yield(e)
        | CeItem::YieldBang(e) => e,
    }
}

/// P8: collect the names bound in this expression's own (Python function) frame —
/// `let` bindings, `match` binders, `as`-patterns — **without** descending into
/// nested functions/CEs (their bindings are a separate scope). Also used by
/// `lower_module`'s top-level shadow check (`top_binding_needs_frame`).
pub(super) fn collect_frame_binders(e: &Expr, out: &mut HashSet<String>) {
    match &e.kind {
        ExprKind::Block { stmts } => {
            for s in stmts {
                match s {
                    BlockStmt::Let(b) => {
                        out.insert(b.name.clone());
                        // A value binding shares this frame; a parameterized `let`
                        // is a nested function (its own scope) — only its name leaks.
                        if b.params.is_empty() {
                            collect_frame_binders(&b.value, out);
                        }
                    }
                    BlockStmt::Expr(e) => collect_frame_binders(e, out),
                }
            }
        }
        ExprKind::If { cond, then, else_ } => {
            collect_frame_binders(cond, out);
            collect_frame_binders(then, out);
            collect_frame_binders(else_, out);
        }
        ExprKind::Match { scrutinee, arms } => {
            collect_frame_binders(scrutinee, out);
            for a in arms {
                for n in super::pattern_bindings(&a.pattern) {
                    out.insert(n);
                }
                if let Some(g) = &a.guard {
                    collect_frame_binders(g, out);
                }
                collect_frame_binders(&a.body, out);
            }
        }
        ExprKind::App { func, arg } => {
            collect_frame_binders(func, out);
            collect_frame_binders(arg, out);
        }
        ExprKind::Pipe { lhs, rhs, .. }
        | ExprKind::Compose { lhs, rhs, .. }
        | ExprKind::Binary { lhs, rhs, .. } => {
            collect_frame_binders(lhs, out);
            collect_frame_binders(rhs, out);
        }
        ExprKind::Unary { expr, .. }
        | ExprKind::Annot { value: expr, .. }
        | ExprKind::Try { body: expr }
        | ExprKind::Assign { value: expr, .. } => collect_frame_binders(expr, out),
        ExprKind::Compare { first, rest } => {
            collect_frame_binders(first, out);
            for (_, o) in rest {
                collect_frame_binders(o, out);
            }
        }
        ExprKind::List { elems } | ExprKind::Tuple { elems } => {
            for e in elems {
                collect_frame_binders(e, out);
            }
        }
        ExprKind::Record { fields, .. } => {
            for f in fields {
                collect_frame_binders(&f.value, out);
            }
        }
        ExprKind::RecordUpdate { base, fields } => {
            collect_frame_binders(base, out);
            for f in fields {
                collect_frame_binders(&f.value, out);
            }
        }
        ExprKind::Field { base, .. } => collect_frame_binders(base, out),
        ExprKind::Interp { parts } => {
            for p in parts {
                if let InterpPart::Expr(e) = p {
                    collect_frame_binders(e, out);
                }
            }
        }
        // Nested functions / CEs are their own Python scope.
        ExprKind::Fn { .. } | ExprKind::Ce { .. } => {}
        ExprKind::Int(_)
        | ExprKind::Float(_)
        | ExprKind::Str(_)
        | ExprKind::Bool(_)
        | ExprKind::Unit
        | ExprKind::Hole { .. }
        | ExprKind::OpFunc(_)
        | ExprKind::Var(_) => {}
    }
}

/// P8: collect the free variables of `e` (names referenced but not bound within),
/// given the already-bound set. Used for the named-folder capture check and the
/// decode pass's named-decoder check (`decode_spec.rs`).
pub(super) fn collect_free(e: &Expr, bound: &HashSet<String>, out: &mut HashSet<String>) {
    match &e.kind {
        ExprKind::Var(n) => {
            if !bound.contains(n) {
                out.insert(n.clone());
            }
        }
        ExprKind::Assign { target, value } => {
            if !bound.contains(target) {
                out.insert(target.clone());
            }
            collect_free(value, bound, out);
        }
        ExprKind::Fn { params, body } => {
            let mut b = bound.clone();
            for p in params {
                b.insert(p.name.clone());
            }
            collect_free(body, &b, out);
        }
        ExprKind::Match { scrutinee, arms } => {
            collect_free(scrutinee, bound, out);
            for a in arms {
                let mut b = bound.clone();
                for n in super::pattern_bindings(&a.pattern) {
                    b.insert(n);
                }
                if let Some(g) = &a.guard {
                    collect_free(g, &b, out);
                }
                collect_free(&a.body, &b, out);
            }
        }
        ExprKind::Block { stmts } => {
            let mut b = bound.clone();
            for s in stmts {
                match s {
                    BlockStmt::Let(bind) => {
                        let mut inner = b.clone();
                        for p in &bind.params {
                            inner.insert(p.name.clone());
                        }
                        collect_free(&bind.value, &inner, out);
                        b.insert(bind.name.clone());
                    }
                    BlockStmt::Expr(e) => collect_free(e, &b, out),
                }
            }
        }
        ExprKind::Ce { items, .. } => {
            let mut b = bound.clone();
            for it in items {
                match it {
                    CeItem::LetBang { name, value, .. } | CeItem::Let { name, value, .. } => {
                        collect_free(value, &b, out);
                        b.insert(name.clone());
                    }
                    CeItem::DoBang(e)
                    | CeItem::Return(e)
                    | CeItem::ReturnBang(e)
                    | CeItem::Yield(e)
                    | CeItem::YieldBang(e) => collect_free(e, &b, out),
                }
            }
        }
        ExprKind::App { func, arg } => {
            collect_free(func, bound, out);
            collect_free(arg, bound, out);
        }
        ExprKind::Pipe { lhs, rhs, .. }
        | ExprKind::Compose { lhs, rhs, .. }
        | ExprKind::Binary { lhs, rhs, .. } => {
            collect_free(lhs, bound, out);
            collect_free(rhs, bound, out);
        }
        ExprKind::If { cond, then, else_ } => {
            collect_free(cond, bound, out);
            collect_free(then, bound, out);
            collect_free(else_, bound, out);
        }
        ExprKind::Unary { expr, .. } | ExprKind::Annot { value: expr, .. } => {
            collect_free(expr, bound, out)
        }
        ExprKind::Try { body } => collect_free(body, bound, out),
        ExprKind::Compare { first, rest } => {
            collect_free(first, bound, out);
            for (_, o) in rest {
                collect_free(o, bound, out);
            }
        }
        ExprKind::List { elems } | ExprKind::Tuple { elems } => {
            for e in elems {
                collect_free(e, bound, out);
            }
        }
        ExprKind::Record { fields, .. } => {
            for f in fields {
                collect_free(&f.value, bound, out);
            }
        }
        ExprKind::RecordUpdate { base, fields } => {
            collect_free(base, bound, out);
            for f in fields {
                collect_free(&f.value, bound, out);
            }
        }
        ExprKind::Field { base, .. } => collect_free(base, bound, out),
        ExprKind::Interp { parts } => {
            for p in parts {
                if let InterpPart::Expr(e) = p {
                    collect_free(e, bound, out);
                }
            }
        }
        ExprKind::Int(_)
        | ExprKind::Float(_)
        | ExprKind::Str(_)
        | ExprKind::Bool(_)
        | ExprKind::Unit
        | ExprKind::Hole { .. }
        | ExprKind::OpFunc(_) => {}
    }
}
