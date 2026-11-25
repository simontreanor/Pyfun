//! Which binding a name means on each line of a file, and the rewrite that
//! gives every top-level binding a name of its own before lowering
//! (`DESIGN.md` §5, §7.1).
//!
//! A file may bind one name at the top level more than once, and a `let` may
//! take the name of a built-in or of an `extern`. The checker reads each use
//! in source order, so a function above the second binding means the first.
//! Python has one global per name and looks it up when the function runs, so
//! the emitted program would run the second. [`settle`] closes the gap: every
//! binding but one is renamed, with the uses that mean it, so no two bindings
//! share a Python global.
//!
//! [`TopScope`] is the one statement of the rule. The checker builds its
//! dependency graph and its scopes from it, and [`settle`] renames from it, so
//! the two cannot read a name differently.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

use crate::parser::ast::{
    BlockStmt, CeItem, Expr, ExprKind, InterpPart, Item, LetBinding, Module, Pattern,
};

/// The prefix of the name a use of a built-in is given when the file also
/// binds that name itself (`_pf_builtin_max`), so the lowering reads it as the
/// built-in wherever it stands.
pub(crate) const BUILTIN_MARK: &str = "_pf_builtin_";

/// Which of a name's bindings keeps the name as written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Naming {
    /// A file: the last binding keeps the name, since that is the one the rest
    /// of the program, another module and a Python caller mean by it.
    #[default]
    File,
    /// A REPL or notebook session, which grows an entry at a time: the first
    /// binding keeps the name and each later one takes a name no source can
    /// spell, so what was already run never has to be run again.
    Session,
}

/// A binding is a function (safe to pre-bind in a cycle, and not run where it
/// is written) if it takes parameters or its value is a lambda, the same test
/// the lowerer uses for arity.
pub(crate) fn binds_function(b: &LetBinding) -> bool {
    !b.params.is_empty() || matches!(b.value.kind, ExprKind::Fn { .. })
}

/// How the top-level names of a file resolve, line by line.
#[derive(Default)]
pub(crate) struct TopScope {
    /// Each name to the items that bind it with a `let` (or, for a module's
    /// name, the in-file `module`), in source order.
    lets: HashMap<String, Vec<usize>>,
    /// Each `extern`'s name to its item. An `extern` is in scope from the top
    /// of the file.
    externs: HashMap<String, usize>,
    /// The `let` items that take parameters: such a function is in scope in
    /// its own body.
    with_params: HashSet<usize>,
    /// The `let` items that bind a function.
    functions: HashSet<usize>,
    /// Each member of a cycle of functions to the cycle's first member.
    group: HashMap<usize, usize>,
    /// The cycles of mutually recursive functions, each in source order.
    pub(crate) groups: Vec<Vec<usize>>,
    /// The names each item reads from outside itself.
    reads: Vec<HashSet<String>>,
    /// Each binding item to the binding items it reads.
    pub(crate) succ: HashMap<usize, Vec<usize>>,
}

/// What a name means at one place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Target {
    /// The `let` (or in-file module) at this item.
    Let(usize),
    /// The `extern` at this item.
    Extern(usize),
    /// Nothing the file binds: a built-in, or nothing at all.
    Outside,
}

impl TopScope {
    pub(crate) fn of(module: &Module) -> TopScope {
        let mut scope = TopScope::default();
        for (idx, item) in module.items.iter().enumerate() {
            match item {
                Item::Let(b) => {
                    for name in b.bound_names() {
                        let items = scope.lets.entry(name).or_default();
                        if items.last() != Some(&idx) {
                            items.push(idx);
                        }
                    }
                    if !b.params.is_empty() {
                        scope.with_params.insert(idx);
                    }
                    if binds_function(b) {
                        scope.functions.insert(idx);
                    }
                }
                Item::Module { name, .. } => {
                    scope.lets.entry(name.clone()).or_default().push(idx);
                }
                Item::Extern(decl) => {
                    scope.externs.insert(decl.name.clone(), idx);
                }
                _ => {}
            }
        }
        let ap_extra = ap_extras(module);
        let mut copy = module.clone();
        scope.reads = copy
            .items
            .iter_mut()
            .map(|item| item_reads(item, &ap_extra))
            .collect();

        // The dependency graph over the binding items. An edge follows what a
        // name would mean if every forward reference were allowed; the cycles
        // of functions found in it are the only place one is.
        let nodes: Vec<usize> = (0..module.items.len())
            .filter(|&i| matches!(module.items[i], Item::Let(_) | Item::Module { .. }))
            .collect();
        let local: HashMap<usize, usize> = nodes.iter().enumerate().map(|(l, &i)| (i, l)).collect();
        let succ: Vec<Vec<usize>> = nodes
            .iter()
            .map(|&idx| {
                let mut out: Vec<usize> = scope.reads[idx]
                    .iter()
                    .filter_map(|name| match scope.target(idx, name, false) {
                        Target::Let(j) => local.get(&j).copied(),
                        _ => None,
                    })
                    .collect();
                out.sort_unstable();
                out.dedup();
                out
            })
            .collect();
        for comp in crate::types::strongly_connected(&succ) {
            // Only a genuine cycle whose members are all functions is a group.
            // A value cycle (`let a = b\nlet b = a`) stays declare-before-use.
            let mut members: Vec<usize> = comp.iter().map(|&l| nodes[l]).collect();
            if members.len() < 2 || !members.iter().all(|i| scope.functions.contains(i)) {
                continue;
            }
            members.sort_unstable();
            for &m in &members {
                scope.group.insert(m, members[0]);
            }
            scope.groups.push(members);
        }
        scope.groups.sort();
        // With the cycles known, the graph the checker plans from: a forward
        // reference is an edge only inside a cycle.
        for &idx in &nodes {
            let mut out: Vec<usize> = scope.reads[idx]
                .iter()
                .filter_map(|name| match scope.resolve(idx, name) {
                    Target::Let(j) => Some(j),
                    _ => None,
                })
                .collect();
            out.sort_unstable();
            out.dedup();
            scope.succ.insert(idx, out);
        }
        scope
    }

    /// What `name` means when it is read by the item at `at` (anywhere in it,
    /// where nothing nearer binds the name).
    pub(crate) fn resolve(&self, at: usize, name: &str) -> Target {
        self.target(at, name, true)
    }

    /// [`TopScope::resolve`], or with `grouped` off, what the name would mean
    /// if a forward reference needed no cycle (the graph the cycles are found
    /// in).
    fn target(&self, at: usize, name: &str, grouped: bool) -> Target {
        let lets = self.lets.get(name).map_or(&[][..], Vec::as_slice);
        // The nearest binding above. A function with parameters is in scope
        // in its own body; a value is not (`let x = x + 1` reads the `x`
        // before it).
        let above = lets
            .iter()
            .rev()
            .find(|&&j| j < at || (j == at && self.with_params.contains(&j)));
        if let Some(&j) = above {
            return Target::Let(j);
        }
        if let Some(&e) = self.externs.get(name) {
            return Target::Extern(e);
        }
        let in_group = |i: usize| !grouped || self.group.contains_key(&i);
        // A function bound to a lambda reads itself only as part of a cycle.
        if lets.contains(&at) && self.functions.contains(&at) && in_group(at) {
            return Target::Let(at);
        }
        // The one forward reference: to a function of the same cycle.
        match lets.iter().find(|&&j| j > at) {
            Some(&below) if !grouped => Target::Let(below),
            Some(&below)
                if self.group.contains_key(&at)
                    && self.group.get(&at) == self.group.get(&below) =>
            {
                Target::Let(below)
            }
            _ => Target::Outside,
        }
    }

    /// The names the item at `idx` reads from outside itself.
    pub(crate) fn reads(&self, idx: usize) -> &HashSet<String> {
        &self.reads[idx]
    }

    /// The first item below `at` that binds `name` with a `let`, if any: what a
    /// name that is not in scope yet is waiting for.
    pub(crate) fn defined_below(&self, at: usize, name: &str) -> bool {
        self.lets
            .get(name)
            .is_some_and(|items| items.iter().any(|&j| j > at))
    }
}

/// Each active-pattern case that takes arguments ahead of its input, with how
/// many: in `case DivisibleBy n:` the `n` is read, not bound.
fn ap_extras(module: &Module) -> HashMap<String, usize> {
    let mut out = HashMap::new();
    for item in &module.items {
        if let Item::ActivePattern(decl) = item
            && decl.params.len() > 1
        {
            for case in &decl.cases {
                out.insert(case.name.clone(), decl.params.len() - 1);
            }
        }
    }
    out
}

/// The names an item reads from outside itself.
fn item_reads(item: &mut Item, ap_extra: &HashMap<String, usize>) -> HashSet<String> {
    let mut out = HashSet::new();
    match item {
        Item::Module { items, .. } => {
            // A member reads its earlier siblings by their bare names.
            let mut own: HashSet<String> = HashSet::new();
            for member in items {
                let recursive = !member.params.is_empty();
                let names = member.bound_names();
                Refs::new(ap_extra, &mut |name| {
                    if !own.contains(name.as_str()) && !(recursive && names.contains(name)) {
                        out.insert(name.clone());
                    }
                })
                .binding(member);
                own.extend(names);
            }
        }
        other => for_each_ref(other, ap_extra, &mut |name| {
            out.insert(name.clone());
        }),
    }
    out
}

/// Visit each name a `let`, a top-level expression or an active pattern reads
/// from outside itself. (An in-file module is walked a member at a time by its
/// callers, since a member's siblings are in scope by position.)
fn for_each_ref(
    item: &mut Item,
    ap_extra: &HashMap<String, usize>,
    visit: &mut dyn FnMut(&mut String),
) {
    let mut refs = Refs::new(ap_extra, visit);
    match item {
        Item::Let(b) => refs.binding(b),
        Item::Expr(e) => refs.expr(e),
        Item::ActivePattern(decl) => {
            let bound = decl
                .params
                .iter()
                .flat_map(|p| refs.binders(&p.pattern))
                .collect();
            refs.scopes.push(bound);
            refs.expr(&mut decl.value);
            refs.scopes.pop();
        }
        _ => {}
    }
}

/// A walk over an expression that hands each *free* name to `visit`: a name
/// read (or assigned with `<-`) where no parameter, `let`, pattern or
/// computation-expression binder of the expression itself is in scope. The
/// name may be rewritten in place.
struct Refs<'a> {
    ap_extra: &'a HashMap<String, usize>,
    scopes: Vec<HashSet<String>>,
    visit: &'a mut dyn FnMut(&mut String),
}

impl<'a> Refs<'a> {
    fn new(ap_extra: &'a HashMap<String, usize>, visit: &'a mut dyn FnMut(&mut String)) -> Self {
        Refs {
            ap_extra,
            scopes: Vec::new(),
            visit,
        }
    }

    fn reference(&mut self, name: &mut String) {
        if !self
            .scopes
            .iter()
            .any(|scope| scope.contains(name.as_str()))
        {
            (self.visit)(name);
        }
    }

    /// The value of a top-level `let` or of an in-file module's member. Its
    /// own name is left free: what it means there is the caller's to say.
    fn binding(&mut self, b: &mut LetBinding) {
        let bound = b
            .params
            .iter()
            .flat_map(|p| self.binders(&p.pattern))
            .collect();
        self.scopes.push(bound);
        self.expr(&mut b.value);
        self.scopes.pop();
    }

    /// The names a pattern binds. The leading arguments of a parameterized
    /// active pattern are read, so they are not among them.
    fn binders(&self, pattern: &Pattern) -> Vec<String> {
        let mut out = Vec::new();
        self.collect_binders(pattern, &mut out);
        out
    }

    fn collect_binders(&self, pattern: &Pattern, out: &mut Vec<String>) {
        match pattern {
            Pattern::Wildcard | Pattern::Int(_) | Pattern::Str(_) | Pattern::Bool(_) => {}
            Pattern::Var { name, .. } => out.push(name.clone()),
            Pattern::Ctor { name, args, .. } => {
                let skip = self
                    .ap_extra
                    .get(name)
                    .copied()
                    .unwrap_or(0)
                    .min(args.len());
                for arg in &args[skip..] {
                    self.collect_binders(arg, out);
                }
            }
            Pattern::Record { fields, .. } => {
                for field in fields {
                    self.collect_binders(&field.pattern, out);
                }
            }
            Pattern::Tuple { elems } => {
                for elem in elems {
                    self.collect_binders(elem, out);
                }
            }
            Pattern::List {
                prefix,
                rest,
                suffix,
            } => {
                for elem in prefix {
                    self.collect_binders(elem, out);
                }
                if let Some(rest) = rest {
                    self.collect_binders(rest, out);
                }
                for elem in suffix {
                    self.collect_binders(elem, out);
                }
            }
            // Every alternative binds the same names.
            Pattern::Or(alts) => {
                if let Some(first) = alts.first() {
                    self.collect_binders(first, out);
                }
            }
            Pattern::As { pattern, name, .. } => {
                out.push(name.clone());
                self.collect_binders(pattern, out);
            }
        }
    }

    /// The names a pattern reads: the leading arguments of a parameterized
    /// active pattern (`case DivisibleBy n:`).
    fn pattern_refs(&mut self, pattern: &mut Pattern) {
        match pattern {
            Pattern::Wildcard
            | Pattern::Var { .. }
            | Pattern::Int(_)
            | Pattern::Str(_)
            | Pattern::Bool(_) => {}
            Pattern::Ctor { name, args, .. } => {
                let skip = self
                    .ap_extra
                    .get(name)
                    .copied()
                    .unwrap_or(0)
                    .min(args.len());
                for (i, arg) in args.iter_mut().enumerate() {
                    match arg {
                        Pattern::Var { name, .. } if i < skip => self.reference(name),
                        _ if i < skip => {}
                        other => self.pattern_refs(other),
                    }
                }
            }
            Pattern::Record { fields, .. } => {
                for field in fields {
                    self.pattern_refs(&mut field.pattern);
                }
            }
            Pattern::Tuple { elems } | Pattern::Or(elems) => {
                for elem in elems {
                    self.pattern_refs(elem);
                }
            }
            Pattern::List {
                prefix,
                rest,
                suffix,
            } => {
                for elem in prefix.iter_mut().chain(suffix) {
                    self.pattern_refs(elem);
                }
                if let Some(rest) = rest {
                    self.pattern_refs(rest);
                }
            }
            Pattern::As { pattern, .. } => self.pattern_refs(pattern),
        }
    }

    fn ce_items(&mut self, items: &mut [CeItem]) {
        self.scopes.push(HashSet::new());
        for item in items {
            match item {
                CeItem::LetBang { target, value, .. } | CeItem::Let { target, value, .. } => {
                    self.expr(value);
                    let names = self.binders(target);
                    self.scopes.last_mut().expect("pushed above").extend(names);
                }
                CeItem::DoBang(e)
                | CeItem::Return(e)
                | CeItem::ReturnBang(e)
                | CeItem::Yield(e)
                | CeItem::YieldBang(e) => self.expr(e),
                CeItem::For {
                    target,
                    source,
                    body,
                    ..
                } => {
                    self.expr(source);
                    let names = self.binders(target).into_iter().collect();
                    self.scopes.push(names);
                    self.ce_items(body);
                    self.scopes.pop();
                }
            }
        }
        self.scopes.pop();
    }

    fn expr(&mut self, expr: &mut Expr) {
        match &mut expr.kind {
            ExprKind::Int(_)
            | ExprKind::Float(_)
            | ExprKind::Str(_)
            | ExprKind::Bool(_)
            | ExprKind::Unit
            | ExprKind::Hole { .. }
            | ExprKind::OpFunc(_) => {}
            ExprKind::Var(name) => self.reference(name),
            ExprKind::Assign { target, value } => {
                self.reference(target);
                self.expr(value);
            }
            ExprKind::Interp { parts } => {
                for part in parts {
                    if let InterpPart::Expr(e) = part {
                        self.expr(e);
                    }
                }
            }
            ExprKind::Fn { params, body } => {
                let bound = params
                    .iter()
                    .flat_map(|p| self.binders(&p.pattern))
                    .collect();
                self.scopes.push(bound);
                self.expr(body);
                self.scopes.pop();
            }
            ExprKind::App { func, arg } => {
                self.expr(func);
                self.expr(arg);
            }
            ExprKind::If { cond, then, else_ } => {
                self.expr(cond);
                self.expr(then);
                self.expr(else_);
            }
            ExprKind::Try { body } => self.expr(body),
            ExprKind::Collection { expansion, .. } => self.expr(expansion),
            ExprKind::Match { scrutinee, arms } => {
                self.expr(scrutinee);
                for arm in arms {
                    self.pattern_refs(&mut arm.pattern);
                    let bound = self.binders(&arm.pattern).into_iter().collect();
                    self.scopes.push(bound);
                    if let Some(guard) = &mut arm.guard {
                        self.expr(guard);
                    }
                    self.expr(&mut arm.body);
                    self.scopes.pop();
                }
            }
            ExprKind::Binary { lhs, rhs, .. }
            | ExprKind::Pipe { lhs, rhs, .. }
            | ExprKind::Compose { lhs, rhs, .. } => {
                self.expr(lhs);
                self.expr(rhs);
            }
            ExprKind::Unary { expr, .. } => self.expr(expr),
            ExprKind::Compare { first, rest } => {
                self.expr(first);
                for (_, e) in rest {
                    self.expr(e);
                }
            }
            ExprKind::Ce { items, .. } => self.ce_items(items),
            ExprKind::Annot { value, .. } => self.expr(value),
            ExprKind::List { elems } | ExprKind::Tuple { elems } => {
                for e in elems {
                    self.expr(e);
                }
            }
            ExprKind::Record { fields, .. } => {
                for field in fields {
                    self.expr(&mut field.value);
                }
            }
            ExprKind::RecordUpdate { base, fields } => {
                self.expr(base);
                for field in fields {
                    self.expr(&mut field.value);
                }
            }
            ExprKind::Field { base, .. } => self.expr(base),
            ExprKind::Block { stmts } => {
                self.scopes.push(HashSet::new());
                for stmt in stmts {
                    match stmt {
                        BlockStmt::Let(b) => {
                            // A local function is in scope in its own body; a
                            // local value reads what was there before it.
                            let mut inner: HashSet<String> = b
                                .params
                                .iter()
                                .flat_map(|p| self.binders(&p.pattern))
                                .collect();
                            if !b.params.is_empty() {
                                inner.extend(b.bound_names());
                            }
                            self.scopes.push(inner);
                            self.expr(&mut b.value);
                            self.scopes.pop();
                            let names = self.binders(&b.target);
                            self.scopes.last_mut().expect("pushed above").extend(names);
                        }
                        BlockStmt::Expr(e) => self.expr(e),
                    }
                }
                self.scopes.pop();
            }
        }
    }
}

/// Rename the names a pattern binds, as `spell` says.
fn rename_binders(pattern: &mut Pattern, spell: &dyn Fn(&str) -> Option<String>) {
    match pattern {
        Pattern::Wildcard | Pattern::Int(_) | Pattern::Str(_) | Pattern::Bool(_) => {}
        Pattern::Var { name, .. } => {
            if let Some(new) = spell(name) {
                *name = new;
            }
        }
        Pattern::Ctor { args, .. } => args.iter_mut().for_each(|a| rename_binders(a, spell)),
        Pattern::Record { fields, .. } => fields
            .iter_mut()
            .for_each(|f| rename_binders(&mut f.pattern, spell)),
        Pattern::Tuple { elems } | Pattern::Or(elems) => {
            elems.iter_mut().for_each(|e| rename_binders(e, spell))
        }
        Pattern::List {
            prefix,
            rest,
            suffix,
        } => {
            prefix
                .iter_mut()
                .chain(suffix)
                .for_each(|e| rename_binders(e, spell));
            if let Some(rest) = rest {
                rename_binders(rest, spell);
            }
        }
        Pattern::As { pattern, name, .. } => {
            if let Some(new) = spell(name) {
                *name = new;
            }
            rename_binders(pattern, spell);
        }
    }
}

/// Hands out names nothing in the file uses.
struct Fresh {
    taken: HashSet<String>,
}

impl Fresh {
    fn of(module: &Module, ap_extra: &HashMap<String, usize>) -> Fresh {
        let mut taken = crate::lowering::module_binder_names(module);
        let mut copy = module.clone();
        for item in &mut copy.items {
            match item {
                Item::Module { name, items, .. } => {
                    taken.insert(name.clone());
                    for member in items {
                        Refs::new(ap_extra, &mut |name| {
                            taken.insert(name.clone());
                        })
                        .binding(member);
                    }
                }
                Item::Extern(decl) => {
                    taken.insert(decl.name.clone());
                }
                // A `from` import's names are written bare until they are
                // expanded, which a caller may or may not have done.
                Item::Import { names, .. } => {
                    taken.extend(names.iter().map(|n| n.name.clone()));
                }
                other => for_each_ref(other, ap_extra, &mut |name| {
                    taken.insert(name.clone());
                }),
            }
        }
        Fresh { taken }
    }

    /// `_name`, or `_name2`, `_name3`, … while that is in use. Never a name in
    /// the emitter's own `_pf_` space.
    fn name(&mut self, name: &str) -> String {
        let base = format!("_{name}");
        let mut candidate = base.clone();
        let mut k = 2;
        while self.taken.contains(&candidate) || candidate.starts_with("_pf_") {
            candidate = format!("{base}{k}");
            if candidate.starts_with("_pf_") {
                candidate = format!("_{candidate}");
            }
            k += 1;
        }
        self.taken.insert(candidate.clone());
        candidate
    }
}

/// A session's names. The source only ever grows, so a binding's name must not
/// depend on what is entered after it: the first binding of a name keeps it
/// (a built-in counts as the first), and each later one is `name·2`,
/// `name·3`, …, which no source can write. One exception keeps a notebook
/// cell cheap to run again: a `let` that is the binding before it, word for
/// word and meaning the same things, shares that binding's name, so its
/// Python is what already ran.
fn session_spellings(
    module: &Module,
    scope: &TopScope,
    prelude: &HashSet<&str>,
    spelling: &mut HashMap<(usize, String), String>,
) {
    // Each `let` of a name, in source order: (item, name, which binding of
    // the name it is, the binding before it).
    let mut bindings: Vec<(usize, &String, usize, Option<usize>)> = Vec::new();
    for (name, lets) in &scope.lets {
        if !matches!(module.items[lets[0]], Item::Let(_)) {
            continue;
        }
        let external = scope.externs.get(name).copied();
        let builtin = external.is_none() && prelude.contains(name.as_str());
        let mut before = external;
        let mut ordinal = usize::from(external.is_some() || builtin);
        for &item in lets {
            ordinal += 1;
            bindings.push((item, name, ordinal, before));
            before = Some(item);
        }
    }
    bindings.sort();
    for (item, name, ordinal, before) in bindings {
        if ordinal == 1 {
            continue;
        }
        let spelled = |target: Target, name: &str| match target {
            Target::Let(j) | Target::Extern(j) => spelling
                .get(&(j, name.to_string()))
                .cloned()
                .unwrap_or_else(|| name.to_string()),
            Target::Outside => name.to_string(),
        };
        let repeats = before.filter(|&prev| {
            module.items[prev] == module.items[item]
                && scope.reads(item).iter().all(|read| {
                    let (was, is) = (scope.resolve(prev, read), scope.resolve(item, read));
                    (was == Target::Let(prev) && is == Target::Let(item))
                        || spelled(was, read) == spelled(is, read)
                })
        });
        let new = match repeats {
            Some(prev) => spelled(Target::Let(prev), name),
            None => format!("{name}\u{b7}{ordinal}"),
        };
        if new != *name {
            spelling.insert((item, name.clone()), new);
        }
    }
}

/// `module` with a name of its own for every top-level binding: where a name
/// is bound more than once (by two `let`s, or by an `extern` and a `let`),
/// all but one of the bindings are renamed together with the uses that mean
/// them, and a use of a built-in whose name the file also binds is marked as
/// the built-in ([`BUILTIN_MARK`]). The same is done among the members of an
/// in-file module. Borrowed, untouched, when no name is bound twice, which is
/// nearly every file.
pub(crate) fn settle(module: &Module, naming: Naming) -> Cow<'_, Module> {
    let scope = TopScope::of(module);
    let prelude: HashSet<&str> = crate::types::PRELUDE.iter().map(|(n, _)| *n).collect();
    // The names bound more than once, each with its bindings in order: the
    // `extern` (in scope from the top) and then the `let`s.
    let mut rebound: Vec<(&String, Vec<usize>)> = scope
        .lets
        .iter()
        .filter(|(_, items)| matches!(module.items[items[0]], Item::Let(_)))
        .map(|(name, items)| {
            let mut all: Vec<usize> = scope.externs.get(name).copied().into_iter().collect();
            all.extend(items);
            (name, all)
        })
        .filter(|(_, all)| all.len() > 1)
        .collect();
    rebound.sort();
    let shadows_builtin = scope
        .lets
        .keys()
        .any(|name| prelude.contains(name.as_str()) && !scope.externs.contains_key(name));
    let member_rebound = module.items.iter().any(|item| match item {
        Item::Module { items, .. } => {
            let mut seen = HashSet::new();
            items
                .iter()
                .flat_map(|m| m.bound_names())
                .any(|name| !seen.insert(name))
        }
        _ => false,
    });
    if rebound.is_empty() && !shadows_builtin && !member_rebound {
        return Cow::Borrowed(module);
    }

    let ap_extra = ap_extras(module);
    let mut fresh = Fresh::of(module, &ap_extra);
    // The name each renamed binding goes by: (item, name as written) → new name.
    let mut spelling: HashMap<(usize, String), String> = HashMap::new();
    match naming {
        Naming::File => {
            for (name, items) in rebound {
                for &item in &items[..items.len() - 1] {
                    spelling.insert((item, name.clone()), fresh.name(name));
                }
            }
        }
        Naming::Session => session_spellings(module, &scope, &prelude, &mut spelling),
    }
    // How a free name in the item at `at` is spelled, when not as written.
    let spell_ref = |at: usize, name: &str| -> Option<String> {
        match scope.resolve(at, name) {
            Target::Let(j) | Target::Extern(j) => spelling.get(&(j, name.to_string())).cloned(),
            // A built-in, in a file that binds the name too: say so, since
            // the lowering would otherwise read it as the file's own. (In a
            // session it is the file's own that is renamed.)
            Target::Outside => {
                (naming == Naming::File && prelude.contains(name) && scope.lets.contains_key(name))
                    .then(|| format!("{BUILTIN_MARK}{name}"))
            }
        }
    };

    let mut settled = module.clone();
    for (idx, item) in settled.items.iter_mut().enumerate() {
        match item {
            Item::Let(_) | Item::Expr(_) | Item::ActivePattern(_) => {
                for_each_ref(item, &ap_extra, &mut |name| {
                    if let Some(new) = spell_ref(idx, name) {
                        *name = new;
                    }
                });
                if let Item::Let(b) = item {
                    rename_binders(&mut b.target, &|name| {
                        spelling.get(&(idx, name.to_string())).cloned()
                    });
                }
            }
            Item::Extern(decl) => {
                if let Some(new) = spelling.get(&(idx, decl.name.clone())) {
                    decl.name = new.clone();
                }
            }
            Item::Module { items, .. } => {
                // Among a module's members the same rule holds, a member at a
                // time: the last binding of a name keeps it (it is the one
                // `Module.name` means outside).
                let mut count: HashMap<String, usize> = HashMap::new();
                for member in items.iter() {
                    for name in member.bound_names() {
                        *count.entry(name).or_default() += 1;
                    }
                }
                let siblings: HashSet<String> = count.keys().cloned().collect();
                // What each sibling name means at the member being walked.
                let mut current: HashMap<String, String> = HashMap::new();
                for member in items.iter_mut() {
                    let names = member.bound_names();
                    let mut own: HashMap<String, String> = HashMap::new();
                    for name in &names {
                        let left = count.get_mut(name).expect("counted above");
                        *left -= 1;
                        let new = if *left > 0 {
                            let mut new = fresh.name(name);
                            while siblings.contains(&new) {
                                new = fresh.name(name);
                            }
                            new
                        } else {
                            name.clone()
                        };
                        own.insert(name.clone(), new);
                    }
                    let recursive = !member.params.is_empty();
                    Refs::new(&ap_extra, &mut |name| {
                        let sibling = if recursive && own.contains_key(name.as_str()) {
                            own.get(name.as_str())
                        } else {
                            current.get(name.as_str())
                        };
                        if let Some(new) = sibling {
                            *name = new.clone();
                        } else if let Some(new) = spell_ref(idx, name) {
                            *name = new;
                        }
                    })
                    .binding(member);
                    rename_binders(&mut member.target, &|name| {
                        own.get(name).filter(|new| *new != name).cloned()
                    });
                    current.extend(own);
                }
            }
            _ => {}
        }
    }
    Cow::Owned(settled)
}
