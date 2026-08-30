//! Arm-scoped captures and nested-block `let`s (`DESIGN.md` §5).
//!
//! A Pyfun match arm's capture (`case Some pair: …`) is scoped to the arm, and a
//! `let` in a nested block is scoped to that block. The Python either lowers to
//! (`case Some(pair):`, `pair = o._0`, `x = 10`) is an assignment to a local of
//! the enclosing *function*, and Python locals are function-wide: one slot per
//! name per frame. So such a binder that reuses a name the frame also uses for
//! something else (a block-local `def`, a parameter, a global read later on)
//! rebinds that slot for the rest of the function, and a later read sees the
//! inner value where Pyfun meant the original binding (issues #92 and #96).
//!
//! The fix is a rename, and the rule is liveness (issue #97): binder `B` of
//! name `n` is freshened to `_n` when some reference to `n` outside `B`
//! resolves, under Pyfun scoping, to a binding of `n` that is live across `B`:
//! a parameter, a root-level `let`, a global or builtin, a binding of an
//! enclosing frame, or an arm/nested-block binder whose extent encloses `B`. A
//! reference bound by a *disjoint* arm capture or nested-block `let` (a sibling
//! arm, an arm of a different match, another block's `let`) reads its own
//! binding and does not count, so the common `case Error why:` in two
//! sequential matches keeps its plain name. The one exception is a reference
//! inside a nested closure, which always counts, because a closure can outlive
//! its arm. A binder's own references never count for it.
//!
//! The census walks the frame's whole body once, resolving every `Var`
//! reference and `<-` target to the binder it reads under Pyfun scoping: the
//! frame root (parameters, root-level `let`s, anything outside the frame), a
//! registered arm or nested-block binder in this frame, or a binder that
//! belongs to a nested Python scope (a lambda's own parameter or `let`), which
//! produces no occurrence at all since it is that scope's own slot. Binders are
//! identified by the address of their AST node, which is what the lowering
//! hands back when it decides them, so no spans are involved.

use std::collections::{HashMap, HashSet};

use crate::parser::ast::{BlockStmt, CeItem, Expr, ExprKind, InterpPart, Item, LetBinding};

/// The identity of an arm or nested-block binder: the address of its AST node.
pub(super) fn binder_key<T>(node: &T) -> usize {
    node as *const T as usize
}

/// One occurrence of a name: the binder it resolves to (`None` for the frame
/// root or anything outside the frame) and whether it sits inside a nested
/// closure of the frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Occurrence {
    binder: Option<usize>,
    in_closure: bool,
}

/// One Python frame's census plus the fresh names already handed out in it (so
/// two binders in one frame that both freshen `x` get distinct names).
#[derive(Debug, Default, Clone)]
pub(super) struct Frame {
    /// Every occurrence of each name, resolved.
    occurrences: HashMap<String, Vec<Occurrence>>,
    /// For each registered arm/nested-block binder, the binders enclosing it
    /// (any name), so a decision can tell an enclosing binder from a disjoint one.
    ancestors: HashMap<usize, Vec<usize>>,
    pub fresh: HashSet<String>,
    /// How many blocks deep the lowering currently is inside this frame.
    pub depth: usize,
    /// Whether the first block entered at depth 1 is the frame's own body (a
    /// function whose body is a block). Its `let`s are root-level and never
    /// renamed; any other block is nested.
    pub root_is_body: bool,
}

impl Frame {
    /// The frame for a function whose body is `body` (parameters are root
    /// bindings of the frame).
    pub fn of_body(body: &Expr) -> Frame {
        let root_is_body = matches!(body.kind, ExprKind::Block { .. });
        let mut census = Census::new(root_is_body);
        census.expr(body);
        census.finish(root_is_body)
    }

    /// The frame for a computation expression body (its own Python function).
    pub fn of_ce(items: &[CeItem]) -> Frame {
        let mut census = Census::new(false);
        census.ce_items(items);
        census.finish(false)
    }

    /// The module frame: every top-level value evaluated at module scope. A
    /// parameterised binding (or an active-pattern recognizer) is its own def;
    /// an in-file `module` flattens into the same scope.
    pub fn of_items(items: &[Item]) -> Frame {
        // A block-valued top-level binding evaluates at module scope; its `let`s
        // are module globals, and a collision with a module-level binding is
        // already isolated by `lower_module`'s frame wrap, so such a block counts
        // as root here.
        let mut census = Census::new(true);
        census.items(items);
        census.finish(true)
    }

    /// This frame's census with `body`'s occurrences added on top, `body` being
    /// code that is inlined into the frame (a fold loop's folder body, a user
    /// builder's CE items). Its top-level block is nested, like the inlined
    /// block the lowering enters. An over-count is always safe: it can only
    /// freshen more.
    pub fn merged_with(&self, body: &Expr) -> Frame {
        let mut census = Census::new(false);
        census.out = self.occurrences.clone();
        census.ancestors = self.ancestors.clone();
        census.expr(body);
        let mut frame = census.finish(false);
        frame.fresh = self.fresh.clone();
        frame
    }

    /// Whether a block entered at the current depth is nested (its `let`s may
    /// need renaming) rather than the frame's root block.
    pub fn block_is_nested(&self) -> bool {
        self.depth > 1 || !self.root_is_body
    }

    /// Whether `name` (a Python-side spelling) is already in use in this frame.
    pub fn uses(&self, name: &str) -> bool {
        self.occurrences.contains_key(name) || self.fresh.contains(name)
    }

    /// The liveness rule: whether binder `key` of `name` must be renamed. An
    /// occurrence counts when it is not the binder's own, and either sits in a
    /// closure, resolves to the frame root, or resolves to a binder enclosing
    /// `key`. A binder the census never saw (code lowered from a tree the census
    /// did not walk) gets the conservative answer: every foreign occurrence counts.
    pub fn must_rename(&self, name: &str, key: usize) -> bool {
        let Some(occurrences) = self.occurrences.get(name) else {
            return false;
        };
        let ancestors = self.ancestors.get(&key);
        occurrences.iter().any(|o| match o.binder {
            Some(b) if b == key => false,
            None => true,
            Some(b) => o.in_closure || ancestors.is_none_or(|a| a.contains(&b)),
        })
    }
}

/// What a name resolves to at a point of the walk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Resolution {
    /// A registered arm/nested-block binder of this frame.
    Binder(usize),
    /// A binder belonging to a nested Python scope: its own slot, never an
    /// occurrence for this frame.
    Inner,
}

/// The one-pass walk. `env` maps a name to what it currently resolves to (absent
/// means the frame root or something outside the frame); `stack` is the chain of
/// registered binders enclosing the current point.
struct Census {
    out: HashMap<String, Vec<Occurrence>>,
    ancestors: HashMap<usize, Vec<usize>>,
    env: HashMap<String, Resolution>,
    stack: Vec<usize>,
    in_closure: bool,
    depth: usize,
    root_is_body: bool,
}

impl Census {
    fn new(root_is_body: bool) -> Census {
        Census {
            out: HashMap::new(),
            ancestors: HashMap::new(),
            env: HashMap::new(),
            stack: Vec::new(),
            in_closure: false,
            depth: 0,
            root_is_body,
        }
    }

    fn finish(self, root_is_body: bool) -> Frame {
        Frame {
            occurrences: self.out,
            ancestors: self.ancestors,
            fresh: HashSet::new(),
            depth: 0,
            root_is_body,
        }
    }

    fn occurrence(&mut self, name: &str) {
        let binder = match self.env.get(name) {
            Some(Resolution::Inner) => return,
            Some(Resolution::Binder(b)) => Some(*b),
            None => None,
        };
        self.out
            .entry(name.to_string())
            .or_default()
            .push(Occurrence {
                binder,
                in_closure: self.in_closure,
            });
    }

    /// Bind `names` for the extent of `f`: to a registered frame binder in the
    /// frame proper, to a nested scope's own slot inside a closure.
    fn scoped<F: FnOnce(&mut Census)>(&mut self, key: Option<usize>, names: &[String], f: F) {
        let saved = self.env.clone();
        self.bind(key, names);
        let pushed = key.is_some() && !self.in_closure;
        if let Some(k) = key
            && pushed
        {
            self.ancestors.insert(k, self.stack.clone());
            self.stack.push(k);
        }
        f(self);
        if pushed {
            self.stack.pop();
        }
        self.env = saved;
    }

    /// Bind `names` from here on (no restore): a block `let` for the rest of its
    /// block. `key` is `None` for a root-level `let` (a root binding).
    fn bind(&mut self, key: Option<usize>, names: &[String]) {
        for n in names {
            match (self.in_closure, key) {
                (true, _) => {
                    self.env.insert(n.clone(), Resolution::Inner);
                }
                (false, Some(k)) => {
                    if !self.ancestors.contains_key(&k) {
                        self.ancestors.insert(k, self.stack.clone());
                    }
                    self.env.insert(n.clone(), Resolution::Binder(k));
                }
                (false, None) => {
                    self.env.remove(n);
                }
            }
        }
    }

    /// Walk a nested Python scope (a lambda, a parameterised `let`'s body, a
    /// computation expression): its parameters are its own, and everything it
    /// binds inside is its own too, while its free references still resolve
    /// through this frame's environment and count as closure occurrences.
    fn closure<F: FnOnce(&mut Census)>(&mut self, params: &[String], f: F) {
        let saved_env = self.env.clone();
        let saved_closure = self.in_closure;
        let saved_depth = self.depth;
        self.in_closure = true;
        for p in params {
            self.env.insert(p.clone(), Resolution::Inner);
        }
        f(self);
        self.depth = saved_depth;
        self.in_closure = saved_closure;
        self.env = saved_env;
    }

    fn expr(&mut self, e: &Expr) {
        match &e.kind {
            ExprKind::Var(n) => self.occurrence(n),
            ExprKind::Assign { target, value } => {
                self.occurrence(target);
                self.expr(value);
            }
            ExprKind::Fn { params, body } => {
                let names: Vec<String> = params
                    .iter()
                    .flat_map(|p| super::pattern_bindings(&p.pattern))
                    .collect();
                self.closure(&names, |c| c.expr(body));
            }
            ExprKind::Block { stmts } => self.block(stmts),
            ExprKind::Ce { items, .. } => self.closure(&[], |c| c.ce_items(items)),
            ExprKind::Match { scrutinee, arms } => {
                self.expr(scrutinee);
                for arm in arms {
                    let names = super::pattern_bindings(&arm.pattern);
                    self.scoped(Some(binder_key(arm)), &names, |c| {
                        if let Some(g) = &arm.guard {
                            c.expr(g);
                        }
                        c.expr(&arm.body);
                    });
                }
            }
            ExprKind::App { func, arg } => {
                self.expr(func);
                self.expr(arg);
            }
            ExprKind::Pipe { lhs, rhs, .. }
            | ExprKind::Compose { lhs, rhs, .. }
            | ExprKind::Binary { lhs, rhs, .. } => {
                self.expr(lhs);
                self.expr(rhs);
            }
            ExprKind::If { cond, then, else_ } => {
                self.expr(cond);
                self.expr(then);
                self.expr(else_);
            }
            ExprKind::Unary { expr, .. }
            | ExprKind::Annot { value: expr, .. }
            | ExprKind::Try { body: expr } => self.expr(expr),
            ExprKind::Compare { first, rest } => {
                self.expr(first);
                for (_, o) in rest {
                    self.expr(o);
                }
            }
            ExprKind::List { elems } | ExprKind::Tuple { elems } => {
                for e in elems {
                    self.expr(e);
                }
            }
            ExprKind::Record { fields, .. } => {
                for f in fields {
                    self.expr(&f.value);
                }
            }
            ExprKind::RecordUpdate { base, fields } => {
                self.expr(base);
                for f in fields {
                    self.expr(&f.value);
                }
            }
            ExprKind::Field { base, .. } => self.expr(base),
            ExprKind::Interp { parts } => {
                for p in parts {
                    if let InterpPart::Expr(e) = p {
                        self.expr(e);
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

    /// A block: its `let`s bind for the rest of it. Mirrors the lowering's
    /// depth counter, so a block is root exactly when `Frame::block_is_nested`
    /// would say so at that point.
    fn block(&mut self, stmts: &[BlockStmt]) {
        let saved = self.env.clone();
        self.depth += 1;
        let nested = self.depth > 1 || !self.root_is_body;
        for s in stmts {
            match s {
                BlockStmt::Let(b) => self.block_let(b, nested),
                BlockStmt::Expr(e) => self.expr(e),
            }
        }
        self.depth -= 1;
        self.env = saved;
    }

    /// A `let` in a block. A value `let` reads its value under the outer binding
    /// and binds after; a parameterised `let` is a nested def whose own name is
    /// bound before its body, so a recursive reference resolves to it.
    fn block_let(&mut self, b: &LetBinding, nested: bool) {
        let key = nested.then(|| binder_key(b));
        let names = b.bound_names();
        if b.params.is_empty() {
            self.expr(&b.value);
            self.bind(key, &names);
        } else {
            self.bind(key, &names);
            self.closure_let(b);
        }
    }

    /// The body of a parameterised `let` (a nested def), whatever scope it is in.
    fn closure_let(&mut self, b: &LetBinding) {
        let params: Vec<String> = b
            .params
            .iter()
            .flat_map(|p| super::pattern_bindings(&p.pattern))
            .collect();
        self.closure(&params, |c| c.expr(&b.value));
    }

    /// Computation-expression items in the scope they are walked in: binders
    /// are the CE function's own (`Inner`) when walked as a closure of an outer
    /// frame, and root bindings when the CE is the frame itself.
    fn ce_items(&mut self, items: &[CeItem]) {
        for it in items {
            match it {
                CeItem::LetBang { target, value, .. } | CeItem::Let { target, value, .. } => {
                    self.expr(value);
                    let names = target.bound_names();
                    self.bind(None, &names);
                }
                CeItem::DoBang(e)
                | CeItem::Return(e)
                | CeItem::ReturnBang(e)
                | CeItem::Yield(e)
                | CeItem::YieldBang(e) => self.expr(e),
            }
        }
    }

    fn items(&mut self, items: &[Item]) {
        for item in items {
            match item {
                Item::Let(b) => self.top_let(b),
                // A top-level expression statement (`print m`) evaluates at module scope.
                Item::Expr(e) => self.expr(e),
                Item::ActivePattern(decl) => {
                    let params: Vec<String> = decl
                        .params
                        .iter()
                        .flat_map(|p| super::pattern_bindings(&p.pattern))
                        .collect();
                    self.closure(&params, |c| c.expr(&decl.value));
                }
                // An in-file `module` flattens into module scope.
                Item::Module { items, .. } => {
                    for b in items {
                        self.top_let(b);
                    }
                }
                _ => {}
            }
        }
    }

    /// A top-level binding: a value evaluates at module scope (its names are
    /// module globals, root bindings); a parameterised one is its own def.
    fn top_let(&mut self, b: &LetBinding) {
        if b.params.is_empty() {
            self.expr(&b.value);
        } else {
            self.closure_let(b);
        }
    }
}
