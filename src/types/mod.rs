//! Hindley–Milner type inference with algebraic data types and units of measure
//! (`DESIGN.md` §3, §8.2, §10).
//!
//! Algorithm W with a substitution map and let-generalization, so top-level
//! bindings are polymorphic. Functions are curried.
//!
//! `type` declarations introduce ADTs (constructor schemes + exhaustiveness) or
//! **records** (nominal named-field product types). Field names are globally
//! unique, so a bare `e.x` / `{ x = … }` resolves its record type from the field
//! name alone — Pyfun has no type annotations to fall back on. Record types reuse
//! [`Ty::Con`]; their fields live in a registry on [`Decls`].
//!
//! **Units of measure.** Numeric types carry a [`Unit`] — an element of the free
//! abelian group over base measures and *unit variables*. Unit equality is solved
//! by abelian-group unification (Knuth/Kennedy: pick the smallest-exponent
//! variable, eliminate, recurse), and unit variables are generalized just like
//! type variables, giving unit-polymorphic functions (`let area w h = w * h` infers
//! `int<'u> -> int<'v> -> int<'u 'v>`). Units are erased at lowering.
//!
//! **Numbers (`DESIGN.md` §7.1).** A single built-in `num` constraint makes
//! arithmetic polymorphic over `int`/`float`: integer literals are polymorphic
//! ([`Ty::Num`]) and adapt to context (`1 + 2.0 : float`), so `let add a b = a +
//! b` infers `num 'a => 'a -> 'a -> 'a` and works at both bases. Division mirrors
//! Python: `/` is true division (`float`), `//` floors. No user-extensible type
//! classes (the set is closed); an unresolved `num` defaults to `int`. Ordering
//! (`< > <= >=`) carries a closed `comparison` constraint (int/float/string);
//! equality (`== !=`) and logical `and`/`or`/`not` are unconstrained over bool.
//!
//! Still deferred until its syntax exists: effect inference.

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::lexer::Span;
use crate::parser::ast::{
    BinOp, BlockStmt, CeBuilder, CeItem, Expr, ExprKind, InterpPart, Item, LetBinding, MatchArm,
    Module, Pattern, TypeDeclKind, TypeExpr, UnOp, UnitExpr,
};

/// A factor in a unit term: a base measure or a unit variable.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Atom {
    Base(String),
    Var(u32),
}

/// A unit of measure: a free abelian group element, i.e. a product of atoms
/// raised to integer powers. The empty product is dimensionless. The map never
/// stores zero exponents, so equality is structural.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Unit {
    factors: BTreeMap<Atom, i32>,
}

impl Unit {
    fn dimensionless() -> Unit {
        Unit::default()
    }

    fn base(name: &str) -> Unit {
        let mut u = Unit::default();
        u.insert(Atom::Base(name.to_string()), 1);
        u
    }

    fn var(id: u32) -> Unit {
        let mut u = Unit::default();
        u.insert(Atom::Var(id), 1);
        u
    }

    fn is_dimensionless(&self) -> bool {
        self.factors.is_empty()
    }

    /// Multiply in `atom^exp`, dropping the atom if its exponent reaches zero.
    fn insert(&mut self, atom: Atom, exp: i32) {
        if exp == 0 {
            return;
        }
        let new = self.factors.get(&atom).copied().unwrap_or(0) + exp;
        if new == 0 {
            self.factors.remove(&atom);
        } else {
            self.factors.insert(atom, new);
        }
    }

    fn mul(&self, other: &Unit) -> Unit {
        let mut r = self.clone();
        for (a, e) in &other.factors {
            r.insert(a.clone(), *e);
        }
        r
    }

    fn inv(&self) -> Unit {
        Unit {
            factors: self.factors.iter().map(|(a, e)| (a.clone(), -e)).collect(),
        }
    }

    fn div(&self, other: &Unit) -> Unit {
        self.mul(&other.inv())
    }

    fn pow(&self, k: i32) -> Unit {
        if k == 0 {
            return Unit::dimensionless();
        }
        Unit {
            factors: self
                .factors
                .iter()
                .map(|(a, e)| (a.clone(), e * k))
                .collect(),
        }
    }

    /// The unit variables occurring in this term.
    fn var_ids(&self) -> Vec<u32> {
        self.factors
            .keys()
            .filter_map(|a| if let Atom::Var(v) = a { Some(*v) } else { None })
            .collect()
    }

    /// Substitute unit variables according to `map` (used by instantiation).
    fn subst(&self, map: &HashMap<u32, Unit>) -> Unit {
        let mut r = Unit::dimensionless();
        for (a, e) in &self.factors {
            match a {
                Atom::Var(v) if map.contains_key(v) => r = r.mul(&map[v].pow(*e)),
                _ => r.insert(a.clone(), *e),
            }
        }
        r
    }
}

/// A concrete effect label (`DESIGN.md` §4). The declaration order here is the
/// canonical display/sort order (`io` before `async`), so a multi-label arrow
/// always renders deterministically (`->{io, async}`). Extending the label set is
/// a matter of adding a variant plus its `name`/`from_name` entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum EffLabel {
    /// Observable side effects: printing, `<-` mutation, the (non-`pure`) Python
    /// FFI boundary.
    Io,
    /// Asynchronous execution. Inference-level only for now: representable,
    /// annotatable (`->{async}`), and displayable; the `async {}` CE still types
    /// via its `Async a` value form (whether it *produces* this label is an open
    /// design point — `DESIGN.md` §4).
    Async,
}

impl EffLabel {
    fn name(self) -> &'static str {
        match self {
            EffLabel::Io => "io",
            EffLabel::Async => "async",
        }
    }

    /// Look a surface label name up (used for `->{...}` annotations in declared
    /// types). `None` for an unknown label.
    fn from_name(name: &str) -> Option<EffLabel> {
        match name {
            "io" => Some(EffLabel::Io),
            "async" => Some(EffLabel::Async),
            _ => None,
        }
    }
}

/// An effect: what a function performs when called. A set of concrete *labels*
/// (`io` — printing, mutation via `<-` —, `async`, …) plus a set of effect
/// *variables* that make a function effect-polymorphic (its effect depends on its
/// arguments). The empty effect is pure. Effects ride on function arrows and are
/// **fully erased at lowering** (`DESIGN.md` §4).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Effect {
    labels: std::collections::BTreeSet<EffLabel>,
    vars: std::collections::BTreeSet<u32>,
}

impl Effect {
    fn pure() -> Effect {
        Effect::default()
    }

    fn label(l: EffLabel) -> Effect {
        let mut labels = std::collections::BTreeSet::new();
        labels.insert(l);
        Effect {
            labels,
            vars: std::collections::BTreeSet::new(),
        }
    }

    fn io() -> Effect {
        Effect::label(EffLabel::Io)
    }

    fn var(id: u32) -> Effect {
        let mut vars = std::collections::BTreeSet::new();
        vars.insert(id);
        Effect {
            labels: std::collections::BTreeSet::new(),
            vars,
        }
    }

    /// Union of two effects (used to accumulate the effect of an expression).
    fn union(&self, other: &Effect) -> Effect {
        Effect {
            labels: self.labels.union(&other.labels).cloned().collect(),
            vars: self.vars.union(&other.vars).copied().collect(),
        }
    }

    /// `Some(v)` iff this is exactly one bare variable (no concrete labels) — the
    /// case that unifies most-generally by binding `v`.
    fn as_single_var(&self) -> Option<u32> {
        if self.labels.is_empty() && self.vars.len() == 1 {
            self.vars.iter().next().copied()
        } else {
            None
        }
    }

    /// Render the concrete labels in canonical order, e.g. `io, async` — the
    /// deterministic body of a displayed `->{io, async}` arrow and of `let pure`
    /// violation messages.
    fn show_labels(&self) -> String {
        self.labels
            .iter()
            .map(|l| l.name())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// Substitute effect variables in `eff` according to `emap` (used by instantiation).
fn subst_eff(eff: &Effect, emap: &HashMap<u32, Effect>) -> Effect {
    let mut out = Effect {
        labels: eff.labels.clone(),
        vars: std::collections::BTreeSet::new(),
    };
    for v in &eff.vars {
        match emap.get(v) {
            Some(rep) => out = out.union(rep),
            None => {
                out.vars.insert(*v);
            }
        }
    }
    out
}

/// A monomorphic type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ty {
    /// `int<unit>` (dimensionless when the unit is empty).
    Int(Unit),
    /// `float<unit>`.
    Float(Unit),
    Bool,
    Str,
    /// The unit type `unit` (one value; lowers to Python `None`). The result of
    /// effectful prelude functions like `print`.
    Unit,
    /// A numeric type whose base (`int`/`float`) is not yet known — a variable
    /// constrained to `num`, carrying a unit. Polymorphic numeric (integer)
    /// literals start here and resolve to `Int`/`Float`; an unresolved one
    /// behaves and displays as `int` (the default). See `DESIGN.md` §7.1.
    Num(u32, Unit),
    /// A unification variable.
    Var(u32),
    /// A function `arg ->{effect} result` (curried). The [`Effect`] is the latent
    /// effect performed when this arrow is applied.
    Fun(Box<Ty>, Box<Ty>, Effect),
    /// An applied type constructor, e.g. `Option int`.
    Con(String, Vec<Ty>),
    /// A tuple type `(a, b)` — a structural product of two or more types.
    Tuple(Vec<Ty>),
}

/// The resolved base of a numeric type: a concrete `int`/`float`, or a `num`
/// variable (possibly still unbound).
#[derive(Debug, Clone, Copy)]
enum NumRef {
    Int,
    Float,
    Var(u32),
}

/// A type scheme, generalized over type variables, unit variables, and `num`
/// (numeric base) variables.
#[derive(Debug, Clone)]
struct Scheme {
    vars: Vec<u32>,
    uvars: Vec<u32>,
    num_vars: Vec<u32>,
    /// Generalized type variables carrying the `comparison` constraint.
    ord_vars: Vec<u32>,
    /// Generalized effect variables (for effect-polymorphic functions).
    eff_vars: Vec<u32>,
    /// Whether this binding was declared `let mut` (so `<-` may reassign it).
    /// Mutable bindings are monomorphic (never generalized).
    mutable: bool,
    ty: Ty,
}

impl Scheme {
    fn mono(ty: Ty) -> Scheme {
        Scheme {
            vars: vec![],
            uvars: vec![],
            num_vars: vec![],
            ord_vars: vec![],
            eff_vars: vec![],
            mutable: false,
            ty,
        }
    }
}

type Env = HashMap<String, Scheme>;

const BUILTIN_TYPES: [&str; 5] = ["int", "float", "bool", "string", "unit"];

/// Built-in container/reserved `Con` types that do **not** derive structural
/// ordering (`DESIGN.md` §7.1): `Option`/`Result` would need their bespoke prelude
/// classes extended, `Set`/`Map` have no natural order, and `Exception` is reserved.
/// Only *user* sum types and records (plus tuples) are orderable — these are excluded
/// even though some appear in `type_ctors`/`records`. A deferred follow-on.
// Built-in `Con` types that do NOT derive ordering. `Option`/`Result` DO (their
// prelude classes carry ordering methods, `None < Some`, `Ok < Error`); `List`
// lowers to a Python list (a follow-on could order it lexicographically), `Set`/
// `Map` have no natural order, and `Async`/`Seq`/`Exception` aren't comparable.
const RESERVED_UNORDERED: [&str; 6] = ["List", "Set", "Map", "Async", "Seq", "Exception"];

/// A depth cap on the structural-ordering check, so a pathological *non-regular*
/// recursive type (whose expansion never repeats a `(name, args)` key) can't loop
/// the checker forever. Far beyond any realistic type's structural depth; exceeding
/// it is reported rather than hung on (a sound, conservative rejection).
const MAX_ORD_DEPTH: usize = 100;

/// Prelude functions backed directly by Python builtins (`DESIGN.md` §6). This is
/// the single source of truth shared by the type checker (whose schemes live in
/// [`seed_prelude`], kept in sync with the arities here) and by lowering, which
/// reads the arities so a *partial* application of a builtin (e.g. `max 0`) still
/// lowers correctly. The Python name equals the Pyfun name for every entry, so no
/// call-site renaming is needed — the simplest honest interop surface.
pub const PRELUDE: &[(&str, usize)] = &[
    ("print", 1),
    ("abs", 1),
    ("min", 2),
    ("max", 2),
    ("round", 1),
    ("floor", 1),
    ("ceil", 1),
    ("truncate", 1),
    // Unit-aware square root: `sqrt : float<'u^2> -> float<'u>` (√area = length).
    // The one tractable unit-carrying power op (F# special-cases exactly this
    // signature); general `x ** y` stays dimensionless (§7.1).
    ("sqrt", 1),
    // Standard combinators (`DESIGN.md` §6). Fully type-polymorphic, pure —
    // except `flip`, which is effect-polymorphic (it *calls* its argument).
    ("id", 1),
    ("const", 2),
    ("ignore", 1),
    ("flip", 3),
];

/// The list prelude (`DESIGN.md` §6): functions over the eager `List a` type
/// (which lowers to a Python list — a dynamic array, so index/`len` are O(1),
/// prepend is O(n)). Like [`PRELUDE`], the `(name, arity)` pairs are the single
/// source of truth shared with [`seed_list_prelude`] (schemes) and lowering
/// (arities, for partial application). `len`/`sum` map directly onto the Python
/// builtins of the same name; the rest lower to small emitted helpers (which keep
/// the eager-list semantics Python's lazy `map`/`filter` would not).
pub const LIST_PRELUDE: &[(&str, usize)] = &[
    ("map", 2),
    ("filter", 2),
    ("fold", 3),
    ("len", 1),
    ("sum", 1),
    ("rev", 1),
    ("range", 2),
    ("zip", 2),
    ("get", 2),
    ("isEmpty", 1),
    ("contains", 2),
    ("concat", 2),
    ("sort", 1),
    ("find", 2),
];

/// The `Set` module (`DESIGN.md` §6): members of the built-in `Set a` (which lowers
/// to a Python `set`), accessed qualified (`Set.add`). Like [`LIST_PRELUDE`], the
/// `(member, arity)` pairs are the single source of truth shared with
/// [`seed_set_prelude`] (schemes) and lowering (arities + emitted helpers). All are
/// pure. Module qualification ([`MODULES`]) is what lets `len`/`contains`/… reuse
/// one name across collections without overloading. Elements must be hashable at
/// runtime (Pyfun primitives are); element types are otherwise unconstrained.
/// `Set.empty` is a nullary value (arity 0, lowers to `set()`).
pub const SET_PRELUDE: &[(&str, usize)] = &[
    ("empty", 0),
    ("add", 2),
    ("remove", 2),
    ("contains", 2),
    ("len", 1),
    ("union", 2),
    ("intersect", 2),
    ("difference", 2),
    ("ofList", 1),
    ("toList", 1),
];

/// The `Map` module (`DESIGN.md` §6): members of the built-in `Map k v` (which
/// lowers to a Python `dict`), accessed qualified (`Map.add`). Single source of
/// truth shared with [`seed_map_prelude`] + lowering, all pure. Keys must be
/// hashable at runtime. `Map.findOr key default m` is a total lookup with a
/// fallback (`dict.get`); `Map.tryFind key m : Option v` is the optional form.
/// `Map.empty` is a nullary value (arity 0, lowers to `dict()`). `Map.ofList`/
/// `Map.toList` convert to/from a `List (k, v)` of key/value tuples (now that
/// tuples exist), mirroring `Set.ofList`/`toList`.
pub const MAP_PRELUDE: &[(&str, usize)] = &[
    ("empty", 0),
    ("add", 3),
    ("remove", 2),
    ("contains", 2),
    ("findOr", 3),
    ("tryFind", 2),
    ("len", 1),
    ("keys", 1),
    ("values", 1),
    ("ofList", 1),
    ("toList", 1),
];

/// The `String` module (`DESIGN.md` §6): text operations over the built-in `string`
/// type (which lowers to a Python `str`). Like the collection preludes, the
/// `(member, arity)` pairs are the single source of truth shared with
/// [`seed_string_prelude`] (schemes) and lowering (arities + emitted helpers). All
/// pure and **monomorphic** (over `string`/`int`/`float`/`bool`) — no type vars.
/// Pyfun has no `char` type, so a "character" is a length-1 string: `String.toList`
/// yields single-char strings. `String.toInt : string -> Option int` is **total**
/// (a `ValueError`-guarded parse), so it lowers to a `try`/`except` helper.
pub const STRING_PRELUDE: &[(&str, usize)] = &[
    ("len", 1),
    ("concat", 2),
    ("join", 2),
    ("split", 2),
    ("upper", 1),
    ("lower", 1),
    ("strip", 1),
    ("contains", 2),
    ("startsWith", 2),
    ("endsWith", 2),
    ("replace", 3),
    ("fromInt", 1),
    ("fromFloat", 1),
    ("toInt", 1),
    ("toFloat", 1),
    ("toList", 1),
    ("slice", 3),
    ("tryIndexOf", 2),
];

/// The built-in module namespaces. A `Module.member` reference is parsed as the
/// ordinary field-access node `Field { base: Var("Module"), name: "member" }` (so
/// no parser change was needed); the checker and lowering recognize a base that is
/// one of these names and resolve the dotted member against the prelude instead of
/// as a record-field access. Casing disambiguates: `Upper.x` is a module member,
/// `lower.x` is record-field access.
pub const MODULES: &[&str] = &["List", "Set", "Map", "Option", "Result", "Seq", "String"];

/// Pairs each module with its members (`(member, arity)`), the single source of
/// truth for seeding qualified schemes, registering arities, and editor completion.
pub const MODULE_PRELUDES: &[(&str, &[(&str, usize)])] = &[
    ("List", LIST_PRELUDE),
    ("Set", SET_PRELUDE),
    ("Map", MAP_PRELUDE),
    ("Option", OPTION_PRELUDE),
    ("Result", RESULT_PRELUDE),
    ("Seq", SEQ_PRELUDE),
    ("String", STRING_PRELUDE),
];

/// The `Option` module (`DESIGN.md` §6): helpers over the built-in `Option a` type
/// (constructors `Some`/`None`, seeded like `Result`'s `Ok`/`Error`). The
/// constructors themselves are global; these are the qualified combinators.
pub const OPTION_PRELUDE: &[(&str, usize)] = &[
    ("map", 2),
    ("bind", 2),
    ("filter", 2),
    ("withDefault", 2),
    ("isSome", 1),
    ("isNone", 1),
    ("toResult", 2),
];

/// The `Result` module (`DESIGN.md` §6): combinators over the built-in `Result a e`
/// type (constructors `Ok`/`Error`, used by the `result {}` CE). `Result.map`/
/// `Result.mapError`/`Result.bind` are effect-polymorphic; `Result.toOption` bridges
/// to `Option` (`Ok v → Some v`, `Error _ → None`).
pub const RESULT_PRELUDE: &[(&str, usize)] = &[
    ("map", 2),
    ("mapError", 2),
    ("bind", 2),
    ("withDefault", 2),
    ("isOk", 1),
    ("isError", 1),
    ("toOption", 1),
];

/// The `Seq` module (`DESIGN.md` §6): the **lazy** counterpart to `List`, over the
/// built-in `Seq a` produced by the `seq {}` CE (a Python iterator/generator).
/// `Seq.map`/`filter`/`take`/`range` are lazy (Python's lazy `map`/`filter`/
/// `islice`/`range`); `Seq.fold`/`toList` force the sequence. `Seq.map`/`filter`/
/// `fold` are effect-polymorphic. NB Python iterators are **single-pass**.
pub const SEQ_PRELUDE: &[(&str, usize)] = &[
    ("map", 2),
    ("filter", 2),
    ("take", 2),
    ("fold", 3),
    ("toList", 1),
    ("ofList", 1),
    ("range", 2),
];

/// If `expr` is a module member access `Module.member` (built-in *or* user module),
/// return its dotted name (e.g. `"List.map"`). The base being an **uppercase**
/// identifier is the signal: value identifiers are lowercase, so `Upper.x` is only
/// ever module access (a record-field base is a lowercase value). Shared by the
/// checker and lowering so both resolve qualified references the same way.
pub fn qualified_name(expr: &Expr) -> Option<String> {
    if let ExprKind::Field { base, name } = &expr.kind
        && let ExprKind::Var(m) = &base.kind
        && m.chars().next().is_some_and(|c| c.is_uppercase())
    {
        Some(format!("{m}.{name}"))
    } else {
        None
    }
}

/// Classic Levenshtein edit distance (insert/delete/substitute), over `char`s so a
/// case slip counts as one substitution. Powers the "did you mean" member hint.
fn levenshtein(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    // A single rolling row of the DP matrix: `row[j]` is the distance from the
    // processed prefix of `a` to the first `j` chars of `b`.
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut prev = row[0]; // diagonal (row[i][j-1] before overwrite)
        row[0] = i + 1;
        for j in 0..b.len() {
            let cost = usize::from(ca != b[j]);
            let ins_del = row[j + 1].min(row[j]) + 1;
            let sub = prev + cost;
            prev = row[j + 1];
            row[j + 1] = ins_del.min(sub);
        }
    }
    row[b.len()]
}

/// The member of `module` closest to `name` by edit distance, when one is near
/// enough to be a plausible typo or casing slip (`String.startswith` → `startsWith`).
/// Scans the env's qualified keys, so it works for built-in *and* user modules alike.
fn closest_member(module: &str, name: &str, env: &Env) -> Option<String> {
    let prefix = format!("{module}.");
    let members = || env.keys().filter_map(|k| k.strip_prefix(&prefix));
    // Direct members only (qualified keys are single-dot).
    let direct = || members().filter(|m| !m.contains('.'));
    // A pure casing difference (`UPPER` → `upper`) is an unambiguous fix — suggest it
    // outright, before falling back to fuzzy distance.
    if let Some(m) = direct().find(|m| m.eq_ignore_ascii_case(name)) {
        return Some(m.to_string());
    }
    let (dist, best) = direct()
        .map(|m| (levenshtein(name, m), m))
        .min_by_key(|&(d, _)| d)?;
    let (nlen, mlen) = (name.chars().count(), best.chars().count());
    // Threshold ~1/3 the longer name (rustc-style), but always at least 2 so a
    // short name's casing slip (`Len` → `len`) is still caught.
    let within_threshold = dist <= (nlen.max(mlen) / 3).max(2);
    // Abbreviation confusion (`length` → `len`, `string` → `str`): one name is a
    // prefix of the other. Guarded on length ≥ 3 so tiny fragments don't over-match.
    let prefix_related = nlen.min(mlen) >= 3 && (name.starts_with(best) || best.starts_with(name));
    (within_threshold || prefix_related).then(|| best.to_string())
}

/// The unit variable id used by the unit-polymorphic prelude numerics
/// (`abs`/`min`/`max`). Reserved (below [`RESERVED_VARS`]) so a freshly allocated
/// variable can never alias it and corrupt the seeded schemes.
const PRELUDE_UVAR: u32 = 2;

/// The `num` (numeric base) variable id used by the prelude numerics, so they are
/// polymorphic over `int`/`float` as well as units. Also reserved.
const PRELUDE_NUMVAR: u32 = 3;

/// A type error, with the source span it should be reported against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeError {
    pub message: String,
    pub span: Span,
}

/// The inferred type of one expression node, rendered for display. Collected by
/// [`check_collecting`] so an editor (the LSP, `DESIGN.md` §9) can show a type on
/// hover — Pyfun types are inferred, never written, so this is the only way to see
/// one without provoking an error. The rendered string includes latent effects
/// (e.g. `string ->{io} unit`), since `show` prints them on arrows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeSpan {
    pub span: Span,
    pub ty: String,
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

/// Information about one (nominal) record type: how many type parameters it has,
/// and its fields in declared order with their types (parameter vars are `Ty::Var`
/// of the parameter's index, instantiated afresh at each use).
#[derive(Debug, Clone)]
struct RecordInfo {
    params_count: usize,
    fields: Vec<(String, Ty)>,
}

/// The result of the declaration pre-pass.
#[derive(Default)]
struct Decls {
    type_arity: HashMap<String, usize>,
    ctors: HashMap<String, CtorInfo>,
    type_ctors: HashMap<String, Vec<String>>,
    /// Known measure names — both base measures and derived aliases.
    measures: HashSet<String>,
    /// Derived measures (`measure N = kg m / s^2`) → their expansion over base
    /// measures. A name in `measures` but not here is a base measure.
    measure_aliases: HashMap<String, Unit>,
    /// Record types by **bare** type name (the type identity, like sum types);
    /// includes both this module's records and imported ones (merged under their
    /// bare names, so a cross-module record value unifies by identity).
    records: HashMap<String, RecordInfo>,
    /// Field name → the **records that declare it** (bare names), in a stable order
    /// (this module's records first, then imported ones by module name). Field names
    /// are no longer globally unique: a bare `e.x`/`{ e with x = … }` resolves *iff*
    /// exactly one visible record declares `x`; two or more is an ambiguity error at
    /// the *access* site, not at declaration/import (module isolation is preserved —
    /// see `DESIGN.md` §8.3).
    field_owner: HashMap<String, Vec<String>>,
    /// Records declared *in this module* (plus the reserved `Exception`). A bare
    /// construction/pattern tag (`Point { … }`) resolves only to a local record; an
    /// imported record must be tagged qualified (`Geometry.Point { … }`), exactly as
    /// an imported sum-type constructor must be (`Geometry.Circle`).
    local_records: HashSet<String>,
    /// Qualified surface tag (`Geometry.Point`) → the imported record's bare identity
    /// name (`Point`), so a qualified construction/pattern resolves to the same
    /// `RecordInfo` (and `Ty::Con`) the exporting module uses.
    record_aliases: HashMap<String, String>,
    /// User-declared in-file module names (`module Foo = …`), for the "X is a
    /// module" diagnostic on a bare reference.
    modules: HashSet<String>,
}

/// Type-check a whole module, returning every independent error found.
pub fn check(module: &Module) -> Result<(), Vec<TypeError>> {
    let (errors, _types, _schemes, _exports, _records, _measures) =
        run(module, false, &HashMap::new());
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// One sum type a module exports (`DESIGN.md` §6.1): its name, type-parameter
/// arity, and constructors (bare name + signature/arity). Carried in
/// [`ModuleExports`] so an importing module can construct and pattern-match the
/// type's values. Records / measures / externs are not yet cross-module exported.
#[derive(Clone)]
struct ExportedType {
    name: String,
    arity: usize,
    ctors: Vec<(String, CtorInfo)>,
}

/// A module's exported interface (`DESIGN.md` §6.1): its public top-level `let`
/// values' schemes (keyed by **bare** name) and its public sum types. Opaque —
/// the scheme/ctor representation is internal — produced by [`check_module`] and
/// fed back in as a dependent module's imports.
#[derive(Clone, Default)]
pub struct ModuleExports {
    schemes: Env,
    types: Vec<ExportedType>,
    /// Public **record** types (`DESIGN.md` §8.3): each bare name + its
    /// [`RecordInfo`] (params + fields), so an importing module can construct,
    /// pattern-match, update, and field-access the record via qualified tags.
    records: Vec<(String, RecordInfo)>,
    /// Public **base measure** names (`measure m`). Merged **unqualified** into a
    /// consumer's decls — there is no qualified unit syntax (`<m>` is bare), so
    /// measures cross by name and erase at lowering (`DESIGN.md` §6.1).
    measures: HashSet<String>,
    /// Public **derived-measure aliases** (`measure N = kg m / s^2`) → their
    /// expansion over base measures, so `<N>` unifies the same way in the consumer.
    measure_aliases: HashMap<String, Unit>,
}

/// Type-check `module` as one node of a multi-file project (`DESIGN.md` §6.1).
///
/// `imports` maps each imported module's **name** to its [`ModuleExports`]; its
/// values are seeded into the env under qualified keys (`Geometry.area`) and its
/// sum types into the decls/env under qualified constructor keys
/// (`Geometry.Circle`), which is exactly how the `Field` access node (and the new
/// qualified constructor pattern) resolve a `Module.member` reference — so using
/// an unexported / unimported name yields the ordinary "`x` is not a member of
/// `Geometry`" error. Returns the type errors plus this module's own exports for
/// any dependent module to import.
pub fn check_module(
    module: &Module,
    imports: &HashMap<String, ModuleExports>,
) -> (Vec<TypeError>, ModuleExports) {
    let (errors, _types, schemes, types, records, (measures, measure_aliases)) =
        run(module, false, imports);
    (
        errors,
        ModuleExports {
            schemes,
            types,
            records,
            measures,
            measure_aliases,
        },
    )
}

/// Like [`check_module`] but also returns the span→type table (as
/// [`check_collecting`] does), in a single inference pass. Used by the project
/// lowering path to mark integer literals that resolved to `float` (so they emit
/// as `7.0`) while still threading each module's exports to its dependents.
pub fn check_module_collecting(
    module: &Module,
    imports: &HashMap<String, ModuleExports>,
) -> (Vec<TypeError>, Vec<TypeSpan>, ModuleExports) {
    let (errors, types, schemes, tys, records, (measures, measure_aliases)) =
        run(module, true, imports);
    (
        errors,
        types,
        ModuleExports {
            schemes,
            types: tys,
            records,
            measures,
            measure_aliases,
        },
    )
}

/// Like [`check_collecting`] but with imported modules' exports seeded
/// (`DESIGN.md` §6.1), so the editor analysis of a multi-module file resolves
/// `Geometry.area` / `Geometry.Circle` instead of flagging them. Used by the LSP.
pub fn check_collecting_with_imports(
    module: &Module,
    imports: &HashMap<String, ModuleExports>,
) -> (Vec<TypeError>, Vec<TypeSpan>) {
    let (errors, types, _schemes, _exports, _records, _measures) = run(module, true, imports);
    (errors, types)
}

/// Type-check `module` and, in the same pass, collect the inferred type of every
/// expression node for editor hover (`DESIGN.md` §9). Returns the (possibly empty)
/// error list alongside a span→type table resolved against the final substitution.
/// Unlike [`check`], this never short-circuits to `Err` — the hover table is useful
/// even for a module that has type errors elsewhere.
pub fn check_collecting(module: &Module) -> (Vec<TypeError>, Vec<TypeSpan>) {
    let (errors, types, _schemes, _exports, _records, _measures) =
        run(module, true, &HashMap::new());
    (errors, types)
}

/// Shared core of [`check`] / [`check_collecting`] / [`check_module`]. When
/// `record` is set, the inferencer accumulates a `(span, ty)` entry per expression
/// node, which we resolve and render once inference is complete. `imports` pre-binds
/// imported modules' interfaces (members under qualified keys, sum types under
/// qualified constructor keys, for the multi-file driver); it is empty for a
/// single-file check. Returns the errors, the hover table, the module's exported
/// value schemes (top-level `let` bindings under their bare names), and its
/// exported sum types, and its exported records.
type RunResult = (
    Vec<TypeError>,
    Vec<TypeSpan>,
    Env,
    Vec<ExportedType>,
    Vec<(String, RecordInfo)>,
    ExportedMeasures,
);

/// This module's own measures, for its export interface: base names and derived
/// aliases (`DESIGN.md` §6.1).
type ExportedMeasures = (HashSet<String>, HashMap<String, Unit>);

fn run(module: &Module, record: bool, imports: &HashMap<String, ModuleExports>) -> RunResult {
    let mut errors = Vec::new();
    let (mut decls, ctor_env) = build_decls(module, &mut errors);
    // This module's own public sum types + records, captured before imports are
    // merged in (so we export only what this module itself declares).
    let exported_types = collect_exported_types(module, &decls);
    let exported_records = collect_exported_records(module, &decls);
    // Captured before imports are merged, so we export only this module's own
    // measures. `decls.measures` holds base + alias names; `decls.measure_aliases`
    // holds the aliases' expansions.
    let exported_measures: ExportedMeasures =
        (decls.measures.clone(), decls.measure_aliases.clone());
    // Merge imported modules' sum types into the decls (qualified constructor keys),
    // so `Geometry.Circle` construction, qualified ctor patterns, and exhaustiveness
    // all resolve against the imported type.
    merge_imported_types(&mut decls, imports, &mut errors);

    // Start fresh ids past the ids reserved for the seeded built-in schemes, so
    // a freshly allocated (and later bound) variable can't alias a builtin's
    // bound variable and corrupt it via the substitution.
    let mut inf = Infer {
        decls,
        next: RESERVED_VARS,
        record_types: record,
        ..Infer::default()
    };
    let mut env = ctor_env;
    // Seed imported modules' exported values (under qualified keys like
    // `Geometry.area`) and constructors (`Geometry.Circle`) so the `Field` access
    // path resolves cross-module references.
    for (module_name, exports) in imports {
        for (member, scheme) in &exports.schemes {
            env.insert(format!("{module_name}.{member}"), scheme.clone());
        }
        for ty in &exports.types {
            for (ctor, info) in &ty.ctors {
                env.insert(format!("{module_name}.{ctor}"), info.scheme.clone());
            }
        }
    }
    // Names of this module's top-level `let` values, for its export interface.
    let mut exported: Vec<String> = Vec::new();

    // Mutual-recursion grouping: find cycles among top-level `let` bindings so
    // mutually-recursive functions (and forward references between them) type-check.
    // Only all-function cycles are grouped (a value cycle stays declare-before-use,
    // erroring as before); each group is processed at its first member's position.
    enum Role {
        /// Process this whole group here (member item indices, in source order).
        Leader(Vec<usize>),
        /// A non-first member of a group already processed at its leader.
        Skip,
    }
    let roles: HashMap<usize, Role> = {
        let lets: Vec<(usize, &LetBinding)> = module
            .items
            .iter()
            .enumerate()
            .filter_map(|(i, it)| match it {
                Item::Let(b) => Some((i, b)),
                _ => None,
            })
            .collect();
        let name_to_local: HashMap<&str, usize> = lets
            .iter()
            .enumerate()
            .map(|(local, (_, b))| (b.name.as_str(), local))
            .collect();
        let succ: Vec<Vec<usize>> = lets
            .iter()
            .map(|(_, b)| {
                let mut bound = HashSet::new();
                for p in &b.params {
                    bound.insert(p.name.clone());
                }
                let mut refs = HashSet::new();
                collect_free(&b.value, &bound, &mut refs);
                let mut out: Vec<usize> = refs
                    .iter()
                    .filter_map(|n| name_to_local.get(n.as_str()).copied())
                    .collect();
                out.sort_unstable();
                out.dedup();
                out
            })
            .collect();
        // A binding is a "function" (safe to pre-bind in a cycle) if it takes
        // parameters or its value is a lambda — the same test the lowerer uses for
        // arity. A value cycle (`let a = b\nlet b = a`) is not grouped, so it stays
        // declare-before-use and errors as before.
        let is_fn =
            |b: &LetBinding| !b.params.is_empty() || matches!(b.value.kind, ExprKind::Fn { .. });
        let mut roles = HashMap::new();
        for comp in strongly_connected(&succ) {
            // Only group genuine multi-member cycles whose members are all functions.
            if comp.len() < 2 || !comp.iter().all(|&l| is_fn(lets[l].1)) {
                continue;
            }
            let mut member_items: Vec<usize> = comp.iter().map(|&l| lets[l].0).collect();
            member_items.sort_unstable();
            for (k, &item_idx) in member_items.iter().enumerate() {
                if k == 0 {
                    roles.insert(item_idx, Role::Leader(member_items.clone()));
                } else {
                    roles.insert(item_idx, Role::Skip);
                }
            }
        }
        roles
    };

    for (idx, item) in module.items.iter().enumerate() {
        match item {
            // Measures and types are handled by the pre-pass and bind no value.
            Item::Measure { .. } | Item::Type(_) => {}
            // An `extern` is resolved by the pre-pass (its scheme is already in
            // `env`); export its name so a dependent module can reference it
            // qualified (`Mathx.sqrt`), exactly like a `let` value (`DESIGN.md` §6.1).
            Item::Extern(decl) => exported.push(decl.name.clone()),
            // `import` is resolved by the multi-file driver, which feeds the
            // imported schemes in via `seed`; the item itself binds nothing here.
            Item::Import { .. } => {}
            Item::Let(_) if matches!(roles.get(&idx), Some(Role::Skip)) => {
                // Already inferred with its group's leader.
            }
            Item::Let(_) if matches!(roles.get(&idx), Some(Role::Leader(_))) => {
                let Some(Role::Leader(members)) = roles.get(&idx) else {
                    unreachable!()
                };
                let group: Vec<&LetBinding> = members
                    .iter()
                    .map(|&mi| match &module.items[mi] {
                        Item::Let(b) => b,
                        _ => unreachable!("group members are `let` items"),
                    })
                    .collect();
                for (name, res) in inf.infer_mutual_group(&group, &env) {
                    exported.push(name.clone());
                    match res {
                        Ok(scheme) => {
                            env.insert(name, scheme);
                        }
                        Err(e) => {
                            errors.push(e);
                            let ty = inf.fresh();
                            env.insert(name, Scheme::mono(ty));
                        }
                    }
                }
            }
            Item::Let(binding) => {
                exported.push(binding.name.clone());
                match inf.infer_binding(binding, &env) {
                    Ok((scheme, _eff)) => {
                        env.insert(binding.name.clone(), scheme);
                    }
                    Err(e) => {
                        errors.push(e);
                        let ty = inf.fresh();
                        env.insert(binding.name.clone(), Scheme::mono(ty));
                    }
                }
            }
            Item::Expr(expr) => {
                if let Err(e) = inf.infer_expr(expr, &env) {
                    errors.push(e);
                }
            }
            // A module is typed in its own scope: members see prior siblings
            // unqualified (and qualified); only `Module.member` escapes to the outer
            // env (so the bare names are not visible after the module).
            Item::Module { name, items, .. } => {
                let mut module_env = env.clone();
                for member in items {
                    let scheme = match inf.infer_binding(member, &module_env) {
                        Ok((scheme, _eff)) => scheme,
                        Err(e) => {
                            errors.push(e);
                            Scheme::mono(inf.fresh())
                        }
                    };
                    let qualified = format!("{name}.{}", member.name);
                    module_env.insert(member.name.clone(), scheme.clone());
                    module_env.insert(qualified.clone(), scheme.clone());
                    env.insert(qualified, scheme);
                }
            }
        }
    }

    // Resolve every recorded type against the final substitution and render it
    // (effects included). Done here, after inference, because a node's type may
    // still hold unbound vars at the moment it was recorded.
    let types = if record {
        inf.recorded
            .iter()
            .map(|(span, ty)| TypeSpan {
                span: *span,
                ty: show(&inf.apply(ty)),
            })
            .collect()
    } else {
        Vec::new()
    };

    // The module's export interface: each top-level `let` value's final scheme.
    // A top-level binding generalizes against an env of closed schemes, so its
    // scheme is itself closed (no free vars escape); applying the substitution to
    // its type resolves any var a later statement pinned, leaving it safe to
    // transplant into a dependent module's env (where `instantiate` refreshes the
    // quantified vars).
    let exports: Env = exported
        .into_iter()
        .filter_map(|name| {
            env.get(&name).map(|scheme| {
                let mut scheme = scheme.clone();
                scheme.ty = inf.apply(&scheme.ty);
                (name, scheme)
            })
        })
        .collect();

    (
        errors,
        types,
        exports,
        exported_types,
        exported_records,
        exported_measures,
    )
}

/// Capture a module's own public **sum** types from the freshly-built decls, for
/// its export interface (`DESIGN.md` §6.1). Records / measures / externs are not
/// yet cross-module exported.
fn collect_exported_types(module: &Module, decls: &Decls) -> Vec<ExportedType> {
    let mut out = Vec::new();
    for item in &module.items {
        let Item::Type(decl) = item else { continue };
        if !matches!(decl.kind, TypeDeclKind::Sum(_)) {
            continue;
        }
        let Some(&arity) = decls.type_arity.get(&decl.name) else {
            continue;
        };
        let ctors = decls
            .type_ctors
            .get(&decl.name)
            .into_iter()
            .flatten()
            .filter_map(|c| decls.ctors.get(c).map(|info| (c.clone(), info.clone())))
            .collect();
        out.push(ExportedType {
            name: decl.name.clone(),
            arity,
            ctors,
        });
    }
    out
}

/// Capture a module's own public **record** types from the freshly-built decls
/// (`DESIGN.md` §8.3), for its export interface: each bare name + its [`RecordInfo`]
/// (params + fields). The reserved `Exception` record is seeded, not user-declared,
/// so it is never exported.
fn collect_exported_records(module: &Module, decls: &Decls) -> Vec<(String, RecordInfo)> {
    let mut out = Vec::new();
    for item in &module.items {
        let Item::Type(decl) = item else { continue };
        if !matches!(decl.kind, TypeDeclKind::Record(_)) {
            continue;
        }
        if let Some(info) = decls.records.get(&decl.name) {
            out.push((decl.name.clone(), info.clone()));
        }
    }
    out
}

/// Merge imported modules' sum types **and records** into `decls` (`DESIGN.md`
/// §6.1 + §8.3). Sum types register under qualified constructor keys
/// (`Geometry.Circle`): the type's arity and constructor set (for exhaustiveness),
/// plus each constructor's [`CtorInfo`] (for construction and pattern binding).
/// Records register under their **bare identity name** (`Point`, like a sum type),
/// with a qualified surface alias (`Geometry.Point` → `Point`) for tagged
/// construction/patterns and their fields appended to the (multimap) field registry.
/// A type/record name clashing with one already present is reported — the same
/// bare-name uniqueness sum types rely on.
fn merge_imported_types(
    decls: &mut Decls,
    imports: &HashMap<String, ModuleExports>,
    errors: &mut Vec<TypeError>,
) {
    // Deterministic order so a clash / field-order is the same every run.
    let mut module_names: Vec<&String> = imports.keys().collect();
    module_names.sort();
    for module_name in &module_names {
        for ty in &imports[*module_name].types {
            if decls.type_arity.contains_key(&ty.name) {
                errors.push(TypeError {
                    message: format!(
                        "imported type `{}` (from `{module_name}`) clashes with an existing type",
                        ty.name
                    ),
                    span: Span::new(0, 0),
                });
                continue;
            }
            decls.type_arity.insert(ty.name.clone(), ty.arity);
            let mut ctor_names = Vec::with_capacity(ty.ctors.len());
            for (ctor, info) in &ty.ctors {
                let qualified = format!("{module_name}.{ctor}");
                decls.ctors.insert(qualified.clone(), info.clone());
                ctor_names.push(qualified);
            }
            decls.type_ctors.insert(ty.name.clone(), ctor_names);
        }
    }
    // Records after types (both share the `type_arity` namespace and clash check).
    for module_name in &module_names {
        for (name, info) in &imports[*module_name].records {
            if decls.type_arity.contains_key(name) {
                errors.push(TypeError {
                    message: format!(
                        "imported record `{name}` (from `{module_name}`) clashes with an existing type"
                    ),
                    span: Span::new(0, 0),
                });
                continue;
            }
            decls.type_arity.insert(name.clone(), info.params_count);
            decls.records.insert(name.clone(), info.clone());
            decls
                .record_aliases
                .insert(format!("{module_name}.{name}"), name.clone());
            // Fields join the multimap under the record's bare identity name. Local
            // records were registered first (in `build_decls`), so they lead.
            for (field, _) in &info.fields {
                decls
                    .field_owner
                    .entry(field.clone())
                    .or_default()
                    .push(name.clone());
            }
        }
    }
    // Measures merge **unqualified** (there is no qualified unit syntax, `DESIGN.md`
    // §6.1). A base measure re-imported under the same name is fine (measures are
    // nominal-by-name and erase — the shared-`Units`-module pattern), so base names
    // insert idempotently. An alias also inserts, but re-importing the *same* alias
    // name mapped to a *different* expansion is a genuine conflict and errors.
    for module_name in &module_names {
        let exports = &imports[*module_name];
        for name in &exports.measures {
            if let Some(unit) = exports.measure_aliases.get(name) {
                match decls.measure_aliases.get(name) {
                    Some(existing) if existing != unit => {
                        errors.push(TypeError {
                            message: format!(
                                "imported measure alias `{name}` (from `{module_name}`) conflicts \
                                 with a different definition already in scope"
                            ),
                            span: Span::new(0, 0),
                        });
                        continue;
                    }
                    _ => {
                        decls.measure_aliases.insert(name.clone(), unit.clone());
                    }
                }
            }
            // Base names (and alias names) join the known measure set idempotently.
            decls.measures.insert(name.clone());
        }
    }
}

/// Resolve a surface unit expression to a [`Unit`] over base measures, expanding
/// any derived-measure aliases. Shared by alias declaration (`measure N = …`) and
/// `<…>` annotations, so an alias resolves the same way wherever it appears.
fn resolve_unit_against(unit: &UnitExpr, decls: &Decls, span: Span) -> Result<Unit, TypeError> {
    let mut u = Unit::dimensionless();
    for (name, exp) in &unit.factors {
        if let Some(alias) = decls.measure_aliases.get(name) {
            u = u.mul(&alias.pow(*exp));
        } else if decls.measures.contains(name) {
            u = u.mul(&Unit::base(name).pow(*exp));
        } else {
            return Err(TypeError {
                message: format!("unknown measure `{name}`"),
                span,
            });
        }
    }
    Ok(u)
}

/// Register measures and `type` declarations; build the constructor environment.
fn build_decls(module: &Module, errors: &mut Vec<TypeError>) -> (Decls, Env) {
    let mut decls = Decls::default();
    let mut env = Env::new();
    seed_builtin_types(&mut decls, &mut env);
    seed_prelude(&mut env);
    seed_list_prelude(&mut env);
    seed_set_prelude(&mut env);
    seed_map_prelude(&mut env);
    seed_string_prelude(&mut env);
    seed_option_prelude(&mut env);
    seed_result_prelude(&mut env);
    seed_seq_prelude(&mut env);

    for item in &module.items {
        if let Item::Measure {
            name,
            definition,
            span,
        } = item
        {
            if decls.measures.contains(name) {
                errors.push(TypeError {
                    message: format!("measure `{name}` is already defined"),
                    span: span.span(),
                });
                continue;
            }
            // A derived alias must resolve (against measures declared before it)
            // before it joins the known set; a base measure just joins.
            if let Some(body) = definition {
                match resolve_unit_against(body, &decls, span.span()) {
                    Ok(unit) => {
                        decls.measure_aliases.insert(name.clone(), unit);
                    }
                    Err(e) => {
                        errors.push(e);
                        continue;
                    }
                }
            }
            decls.measures.insert(name.clone());
        }
    }

    // Collect user module names (reserving against built-in modules + duplicates),
    // so a bare reference to one reports "X is a module" rather than "unbound".
    for item in &module.items {
        if let Item::Module {
            name, name_span, ..
        } = item
        {
            if MODULES.contains(&name.as_str()) {
                errors.push(TypeError {
                    message: format!("cannot redefine built-in module `{name}`"),
                    span: name_span.span(),
                });
            } else if !decls.modules.insert(name.clone()) {
                errors.push(TypeError {
                    message: format!("module `{name}` is already defined"),
                    span: name_span.span(),
                });
            }
        }
    }

    // Pass 1: type names and arities.
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
                // Only sum types have a constructor set (used by exhaustiveness);
                // records resolve through their field registry instead.
                if let TypeDeclKind::Sum(_) = decl.kind {
                    decls.type_ctors.insert(decl.name.clone(), Vec::new());
                }
            }
        }
    }

    // Pass 2: constructor schemes (sum types) and field registries (records).
    for item in &module.items {
        let Item::Type(decl) = item else { continue };
        let span = decl.span.span();
        let param_map: HashMap<String, u32> = decl
            .params
            .iter()
            .enumerate()
            .map(|(i, p)| (p.clone(), i as u32))
            .collect();

        match &decl.kind {
            TypeDeclKind::Sum(variants) => {
                let result_ty = Ty::Con(
                    decl.name.clone(),
                    (0..decl.params.len() as u32).map(Ty::Var).collect(),
                );
                for variant in variants {
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
                            // Constructing a value is pure.
                            Ty::Fun(Box::new(f), Box::new(acc), Effect::pure())
                        });
                    let scheme = Scheme {
                        vars: (0..decl.params.len() as u32).collect(),
                        uvars: vec![],
                        num_vars: vec![],
                        ord_vars: vec![],
                        eff_vars: vec![],
                        mutable: false,
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
            TypeDeclKind::Record(fields) => {
                let mut resolved = Vec::with_capacity(fields.len());
                let mut ok = true;
                let mut local = HashSet::new();
                for field in fields {
                    if !local.insert(field.name.clone()) {
                        errors.push(TypeError {
                            message: format!(
                                "field `{}` is declared twice in record `{}`",
                                field.name, decl.name
                            ),
                            span,
                        });
                        ok = false;
                        continue;
                    }
                    // Field names are no longer globally unique across records
                    // (`DESIGN.md` §8.3): two records may share a field; the clash is
                    // only reported at an *ambiguous access* site, never here. (A field
                    // repeated within *one* record is still an error — the `local` set.)
                    match resolve(&field.ty, &param_map, &decls.type_arity, span) {
                        Ok(t) => resolved.push((field.name.clone(), t)),
                        Err(e) => {
                            errors.push(e);
                            ok = false;
                        }
                    }
                }
                // Only register a fully-valid record, so the field registry never
                // points at a record without a `RecordInfo`.
                if ok {
                    for (name, _) in &resolved {
                        decls
                            .field_owner
                            .entry(name.clone())
                            .or_default()
                            .push(decl.name.clone());
                    }
                    decls.local_records.insert(decl.name.clone());
                    decls.records.insert(
                        decl.name.clone(),
                        RecordInfo {
                            params_count: decl.params.len(),
                            fields: resolved,
                        },
                    );
                }
            }
        }
    }

    // Pass 3: `extern` declarations (typed Python imports — `DESIGN.md` §6).
    // Resolved last so an extern may reference any user-declared type. Type
    // variables are collected from the declared type (bare lowercase names, as in
    // `type` decls) and generalized; the boundary is effectful-by-default, so the
    // innermost arrow gets `io` unless the binding asserts `pure`.
    for item in &module.items {
        let Item::Extern(decl) = item else { continue };
        let span = decl.span.span();
        if env.contains_key(&decl.name) {
            errors.push(TypeError {
                message: format!("`{}` is already defined", decl.name),
                span,
            });
            continue;
        }
        let mut var_map = HashMap::new();
        collect_type_vars(&decl.ty, &mut var_map);
        match resolve(&decl.ty, &var_map, &decls.type_arity, span) {
            Ok(mut ty) => {
                // Effectful-by-default boundary: the innermost arrow gets `io`
                // unless the binding asserts `pure` — or the innermost arrow
                // carries an explicit `->{...}` annotation, which is trusted as
                // written (e.g. `extern fetch : string ->{async} string`). An
                // annotation elsewhere (say on a higher-order *argument* arrow)
                // does not suppress the default: the extern still calls Python.
                if !decl.pure && !innermost_arrow_annotated(&decl.ty) {
                    set_innermost_io(&mut ty);
                }
                env.insert(
                    decl.name.clone(),
                    Scheme {
                        vars: (0..var_map.len() as u32).collect(),
                        uvars: vec![],
                        num_vars: vec![],
                        ord_vars: vec![],
                        eff_vars: vec![],
                        mutable: false,
                        ty,
                    },
                );
            }
            Err(e) => errors.push(e),
        }
    }

    (decls, env)
}

/// Collect the type variables of a declared type — bare lowercase names that are
/// neither builtins nor (uppercase) type constructors — in order of first
/// appearance, each mapped to a sequential id. Used to generalize `extern` types,
/// which (unlike `type` declarations) have no explicit parameter list.
fn collect_type_vars(ty: &TypeExpr, map: &mut HashMap<String, u32>) {
    match ty {
        TypeExpr::Fun(a, b, _) => {
            collect_type_vars(a, map);
            collect_type_vars(b, map);
        }
        TypeExpr::Tuple(elems) => {
            for e in elems {
                collect_type_vars(e, map);
            }
        }
        TypeExpr::Con(name, _, args) => {
            if args.is_empty()
                && !BUILTIN_TYPES.contains(&name.as_str())
                && name.chars().next().is_some_and(char::is_lowercase)
            {
                let next = map.len() as u32;
                map.entry(name.clone()).or_insert(next);
            }
            for a in args {
                collect_type_vars(a, map);
            }
        }
    }
}

/// Whether the innermost (rightmost) arrow of a declared type carries an explicit
/// `->{...}` effect annotation. Such an `extern` is trusted as written and skips
/// the `io`-by-default rule (`DESIGN.md` §4/§6).
fn innermost_arrow_annotated(ty: &TypeExpr) -> bool {
    match ty {
        TypeExpr::Fun(_, ret, effects) => {
            if matches!(**ret, TypeExpr::Fun(..)) {
                innermost_arrow_annotated(ret)
            } else {
                !effects.is_empty()
            }
        }
        _ => false,
    }
}

/// Set the innermost (rightmost) arrow's latent effect to `io`, so a *fully
/// applied* `extern` performs the Python call's effect (partial applications stay
/// pure — no call has happened yet). A non-function (value) extern is left pure.
fn set_innermost_io(ty: &mut Ty) {
    if let Ty::Fun(_, ret, eff) = ty {
        if matches!(**ret, Ty::Fun(..)) {
            set_innermost_io(ret);
        } else {
            *eff = Effect::io();
        }
    }
}

/// A head constructor in the exhaustiveness matrix: a boolean/integer literal, a
/// sum-type constructor, or a record type (its single implicit constructor).
#[derive(Clone, PartialEq)]
enum Tag {
    Bool(bool),
    Int(i64),
    Str(String),
    Sum(String),
    Record(String),
    /// A tuple's single implicit constructor, carrying its arity.
    Tuple(usize),
    /// The empty list `[]`. `List` is modeled as `Nil | Cons a (List a)` *inside the
    /// usefulness algorithm only* (no real ADT, no lowering change), so sequence
    /// patterns get proper exhaustiveness (`DESIGN.md` §7.2).
    Nil,
    /// A non-empty list — `Cons head tail`, arity 2 (element type, `List` tail type).
    Cons,
}

/// A witness value produced when a `match` is non-exhaustive — a constructor
/// applied to sub-witnesses, or `_` for "any value". Rendered into the diagnostic.
#[derive(Clone)]
enum Wit {
    Wild,
    Con(Tag, Vec<Wit>),
}

/// The default matrix: rows whose first column is a wildcard/variable, with that
/// column dropped. Used when the present constructors don't cover the type, so a
/// wildcard could match an absent constructor and only the catch-all rows remain.
fn default_matrix(matrix: &[Vec<Pattern>]) -> Vec<Vec<Pattern>> {
    matrix
        .iter()
        .filter_map(|row| {
            let (head, rest) = row.split_first().expect("non-empty row");
            is_wildcard_like(head).then(|| rest.to_vec())
        })
        .collect()
}

/// Whether a pattern acts as a **catch-all** column head (a wildcard, a variable,
/// or a lone-star list pattern `[*r]` whose binder is itself a catch-all). Such a
/// head matches an absent constructor, so its row is kept — with the head column
/// dropped — in the [`default_matrix`]. `[*r]` is equivalent to `r` (the star binds
/// the whole list), so it is a catch-all exactly when `r` is (`DESIGN.md` §7.2).
fn is_wildcard_like(pat: &Pattern) -> bool {
    match pat {
        Pattern::Wildcard | Pattern::Var { .. } => true,
        Pattern::List {
            prefix,
            rest,
            suffix,
        } if prefix.is_empty() && suffix.is_empty() => {
            rest.as_deref().is_some_and(is_wildcard_like)
        }
        _ => false,
    }
}

/// Whether a pattern contains (at any depth) a list pattern with elements **after**
/// the star (`[*r, s…]`). Only such patterns reproduce themselves under `Cons`
/// specialization, so only they need the cycle guard in [`Infer::useful`].
fn has_suffix_star(pat: &Pattern) -> bool {
    match pat {
        Pattern::List {
            prefix,
            rest,
            suffix,
        } => {
            !suffix.is_empty()
                || prefix.iter().any(has_suffix_star)
                || rest.as_deref().is_some_and(has_suffix_star)
        }
        Pattern::Ctor { args, .. } => args.iter().any(has_suffix_star),
        Pattern::Record { fields, .. } => fields.iter().any(|f| has_suffix_star(&f.pattern)),
        Pattern::Tuple { elems } => elems.iter().any(has_suffix_star),
        Pattern::Or(alts) => alts.iter().any(has_suffix_star),
        Pattern::As { pattern, .. } => has_suffix_star(pattern),
        Pattern::Wildcard
        | Pattern::Var { .. }
        | Pattern::Int(_)
        | Pattern::Str(_)
        | Pattern::Bool(_) => false,
    }
}

/// The top-level alternatives a pattern denotes: an or-pattern flattens to its
/// (recursively flattened) alternatives, anything else is itself (`DESIGN.md`
/// §7.2). Used to expand or-patterns for exhaustiveness.
/// Collect the free variable names of `expr` — references not bound by an
/// enclosing param / lambda / block-`let` / `match` pattern / CE binding. Used to
/// build the top-level dependency graph for mutual-recursion grouping.
pub(crate) fn collect_free(expr: &Expr, bound: &HashSet<String>, out: &mut HashSet<String>) {
    match &expr.kind {
        ExprKind::Var(n) => {
            if !bound.contains(n) {
                out.insert(n.clone());
            }
        }
        ExprKind::Int(_)
        | ExprKind::Float(_)
        | ExprKind::Str(_)
        | ExprKind::Bool(_)
        | ExprKind::Unit
        | ExprKind::OpFunc(_) => {}
        ExprKind::Interp { parts } => {
            for part in parts {
                if let InterpPart::Expr(e) = part {
                    collect_free(e, bound, out);
                }
            }
        }
        ExprKind::Fn { params, body } => {
            let mut b = bound.clone();
            for p in params {
                b.insert(p.name.clone());
            }
            collect_free(body, &b, out);
        }
        ExprKind::App { func, arg } => {
            collect_free(func, bound, out);
            collect_free(arg, bound, out);
        }
        ExprKind::If { cond, then, else_ } => {
            collect_free(cond, bound, out);
            collect_free(then, bound, out);
            collect_free(else_, bound, out);
        }
        ExprKind::Try { body } => collect_free(body, bound, out),
        ExprKind::Match { scrutinee, arms } => {
            collect_free(scrutinee, bound, out);
            for arm in arms {
                let mut b = bound.clone();
                pattern_names(&arm.pattern, &mut b);
                if let Some(g) = &arm.guard {
                    collect_free(g, &b, out);
                }
                collect_free(&arm.body, &b, out);
            }
        }
        ExprKind::Binary { lhs, rhs, .. } => {
            collect_free(lhs, bound, out);
            collect_free(rhs, bound, out);
        }
        ExprKind::Unary { expr, .. } => collect_free(expr, bound, out),
        ExprKind::Compare { first, rest } => {
            collect_free(first, bound, out);
            for (_, e) in rest {
                collect_free(e, bound, out);
            }
        }
        ExprKind::Pipe { lhs, rhs, .. } | ExprKind::Compose { lhs, rhs, .. } => {
            collect_free(lhs, bound, out);
            collect_free(rhs, bound, out);
        }
        ExprKind::Ce { items, .. } => {
            let mut b = bound.clone();
            for item in items {
                match item {
                    CeItem::Let { name, value, .. } | CeItem::LetBang { name, value, .. } => {
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
        ExprKind::Annot { value, .. } => collect_free(value, bound, out),
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
        ExprKind::Block { stmts } => {
            let mut b = bound.clone();
            for stmt in stmts {
                match stmt {
                    BlockStmt::Let(binding) => {
                        let mut vb = b.clone();
                        for p in &binding.params {
                            vb.insert(p.name.clone());
                        }
                        collect_free(&binding.value, &vb, out);
                        b.insert(binding.name.clone());
                    }
                    BlockStmt::Expr(e) => collect_free(e, &b, out),
                }
            }
        }
        ExprKind::Assign { value, .. } => collect_free(value, bound, out),
    }
}

/// Collect the variables a pattern binds (for shadowing in [`collect_free`]).
fn pattern_names(pat: &Pattern, out: &mut HashSet<String>) {
    match pat {
        Pattern::Var { name, .. } => {
            out.insert(name.clone());
        }
        Pattern::Ctor { args, .. } => args.iter().for_each(|a| pattern_names(a, out)),
        Pattern::Record { fields, .. } => {
            fields.iter().for_each(|f| pattern_names(&f.pattern, out))
        }
        Pattern::Tuple { elems } => elems.iter().for_each(|e| pattern_names(e, out)),
        Pattern::List {
            prefix,
            rest,
            suffix,
        } => {
            prefix.iter().for_each(|p| pattern_names(p, out));
            if let Some(r) = rest {
                pattern_names(r, out);
            }
            suffix.iter().for_each(|p| pattern_names(p, out));
        }
        Pattern::Or(alts) => {
            if let Some(a) = alts.first() {
                pattern_names(a, out);
            }
        }
        Pattern::As { pattern, name, .. } => {
            out.insert(name.clone());
            pattern_names(pattern, out);
        }
        Pattern::Wildcard | Pattern::Int(_) | Pattern::Str(_) | Pattern::Bool(_) => {}
    }
}

/// Strongly-connected components of a directed graph given each node's successors,
/// via reachability (n is small — the top-level binding count). Two distinct nodes
/// share a component iff each reaches the other. Returned as node-index lists; the
/// order is by ascending first member.
fn strongly_connected(succ: &[Vec<usize>]) -> Vec<Vec<usize>> {
    let n = succ.len();
    let reach: Vec<HashSet<usize>> = (0..n)
        .map(|i| {
            let mut seen = HashSet::new();
            let mut stack = succ[i].clone();
            while let Some(j) = stack.pop() {
                if seen.insert(j) {
                    stack.extend(succ[j].iter().copied());
                }
            }
            seen
        })
        .collect();
    let mut comp_of = vec![None; n];
    let mut comps = Vec::new();
    for i in 0..n {
        if comp_of[i].is_some() {
            continue;
        }
        let cid = comps.len();
        comp_of[i] = Some(cid);
        let mut members = vec![i];
        for j in (i + 1)..n {
            if comp_of[j].is_none() && reach[i].contains(&j) && reach[j].contains(&i) {
                comp_of[j] = Some(cid);
                members.push(j);
            }
        }
        comps.push(members);
    }
    comps
}

fn expand_or(pat: &Pattern) -> Vec<Pattern> {
    match pat {
        Pattern::Or(alts) => alts.iter().flat_map(expand_or).collect(),
        // `p as x` covers exactly what `p` does; the binding is transparent here.
        Pattern::As { pattern, .. } => expand_or(pattern),
        p => vec![p.clone()],
    }
}

/// Replace each row `[head, ..rest]` with one row per top-level alternative of
/// `head`, so no row's first column is an or-pattern. Empty rows pass through.
fn expand_first_column(matrix: &[Vec<Pattern>]) -> Vec<Vec<Pattern>> {
    let mut out = Vec::new();
    for row in matrix {
        match row.split_first() {
            Some((head, rest)) => {
                for alt in expand_or(head) {
                    let mut r = vec![alt];
                    r.extend_from_slice(rest);
                    out.push(r);
                }
            }
            None => out.push(row.clone()),
        }
    }
    out
}

/// The number of variable ids reserved for built-in schemes: `Ok`/`Error` use
/// type vars 0 and 1, and the prelude numerics use unit var [`PRELUDE_UVAR`] (2)
/// and `num` var [`PRELUDE_NUMVAR`] (3). Inference must start allocating fresh ids
/// past this so a freshly allocated (and later bound) variable can't alias a
/// seeded bound var and corrupt it via a substitution.
const RESERVED_VARS: u32 = 4;

/// Seed the prelude functions ([`PRELUDE`]) into the global environment. These are
/// thin typed views over Python builtins (`DESIGN.md` §6): `print` is effectful
/// and returns `unit`; `abs`/`min`/`max` are polymorphic over the numeric base
/// (`num`) *and* the unit, i.e. `num 'a => 'a<'u> -> …`.
fn seed_prelude(env: &mut Env) {
    // print : 'a ->{io} unit  — the prelude's one effectful builtin.
    env.insert(
        "print".to_string(),
        Scheme {
            vars: vec![0],
            uvars: vec![],
            num_vars: vec![],
            ord_vars: vec![],
            eff_vars: vec![],
            mutable: false,
            ty: Ty::Fun(Box::new(Ty::Var(0)), Box::new(Ty::Unit), Effect::io()),
        },
    );
    let num_u = || Ty::Num(PRELUDE_NUMVAR, Unit::var(PRELUDE_UVAR));
    let scheme = |ty| Scheme {
        vars: vec![],
        uvars: vec![PRELUDE_UVAR],
        num_vars: vec![PRELUDE_NUMVAR],
        ord_vars: vec![],
        eff_vars: vec![],
        mutable: false,
        ty,
    };
    // abs : num 'a => 'a<'u> -> 'a<'u>  (pure)
    env.insert(
        "abs".to_string(),
        scheme(Ty::Fun(
            Box::new(num_u()),
            Box::new(num_u()),
            Effect::pure(),
        )),
    );
    // min / max : num 'a => 'a<'u> -> 'a<'u> -> 'a<'u>  (pure)
    let binary = || {
        Ty::Fun(
            Box::new(num_u()),
            Box::new(Ty::Fun(
                Box::new(num_u()),
                Box::new(num_u()),
                Effect::pure(),
            )),
            Effect::pure(),
        )
    };
    for name in ["min", "max"] {
        env.insert(name.to_string(), scheme(binary()));
    }
    // round / floor / ceil / truncate : float<'u> -> int<'u>  (pure, unit-preserving,
    // like abs/min/max). Concrete float→int (not num-polymorphic), one shared unit.
    let f_to_i = Scheme {
        vars: vec![],
        uvars: vec![PRELUDE_UVAR],
        num_vars: vec![],
        ord_vars: vec![],
        eff_vars: vec![],
        mutable: false,
        ty: Ty::Fun(
            Box::new(Ty::Float(Unit::var(PRELUDE_UVAR))),
            Box::new(Ty::Int(Unit::var(PRELUDE_UVAR))),
            Effect::pure(),
        ),
    };
    for name in ["round", "floor", "ceil", "truncate"] {
        env.insert(name.to_string(), f_to_i.clone());
    }
    // sqrt : float<'u^2> -> float<'u>  (pure, unit-halving — √area = length).
    // The argument unit is the *square* of the result's, expressed with the
    // existing integer-exponent representation (`Unit::pow(2)`); applying it to
    // `float<m^2>` makes the abelian-group unifier solve `'u^2 ~ m^2` → `'u = m`
    // (and `m^4/s^2` → `m^2/s`), while a non-square unit (`m`, `m^3`) fails
    // unification — a compile-time dimensional error. Dimensionless stays
    // dimensionless. See `DESIGN.md` §8.2; the extern surface can't express this
    // (it has no unit syntax), which is why it is a prelude builtin.
    env.insert(
        "sqrt".to_string(),
        Scheme {
            vars: vec![],
            uvars: vec![PRELUDE_UVAR],
            num_vars: vec![],
            ord_vars: vec![],
            eff_vars: vec![],
            mutable: false,
            ty: Ty::Fun(
                Box::new(Ty::Float(Unit::var(PRELUDE_UVAR).pow(2))),
                Box::new(Ty::Float(Unit::var(PRELUDE_UVAR))),
                Effect::pure(),
            ),
        },
    );

    // Standard combinators (`DESIGN.md` §6). Plain type-var polymorphism, no
    // num/unit/comparison constraints. `id`/`const`/`ignore` never call anything,
    // so they're pure; `flip` calls its function argument, so it's effect-poly.
    let poly = |vars: Vec<u32>, ty: Ty| Scheme {
        vars,
        uvars: vec![],
        num_vars: vec![],
        ord_vars: vec![],
        eff_vars: vec![],
        mutable: false,
        ty,
    };
    let pure_fn = |a: Ty, b: Ty| Ty::Fun(Box::new(a), Box::new(b), Effect::pure());
    // id : 'a -> 'a
    env.insert(
        "id".to_string(),
        poly(vec![0], pure_fn(Ty::Var(0), Ty::Var(0))),
    );
    // const : 'a -> 'b -> 'a   (return the first, ignore the second)
    env.insert(
        "const".to_string(),
        poly(
            vec![0, 1],
            pure_fn(Ty::Var(0), pure_fn(Ty::Var(1), Ty::Var(0))),
        ),
    );
    // ignore : 'a -> unit
    env.insert(
        "ignore".to_string(),
        poly(vec![0], pure_fn(Ty::Var(0), Ty::Unit)),
    );
    // flip : (a -> b ->{e} c) -> b -> a ->{e} c   (swap the first two arguments).
    // Effect-polymorphic: `flip f x y = f y x` performs f's latent effect, which
    // rides `e` on f's inner arrow and the innermost result arrow (other arrows
    // are pure — partial application performs nothing).
    let e = 0u32;
    let arrow_e = |a: Ty, b: Ty| Ty::Fun(Box::new(a), Box::new(b), Effect::var(e));
    env.insert(
        "flip".to_string(),
        Scheme {
            vars: vec![0, 1, 2],
            uvars: vec![],
            num_vars: vec![],
            ord_vars: vec![],
            eff_vars: vec![e],
            mutable: false,
            ty: pure_fn(
                // f : a -> b ->{e} c
                pure_fn(Ty::Var(0), arrow_e(Ty::Var(1), Ty::Var(2))),
                // result : b -> a ->{e} c
                pure_fn(Ty::Var(1), arrow_e(Ty::Var(0), Ty::Var(2))),
            ),
        },
    );
}

/// Seed the list prelude ([`LIST_PRELUDE`]) — functions over the eager `List a`.
/// The higher-order ones (`map`/`filter`/`fold`) are **effect-polymorphic**: the
/// effect of mapping is the effect of the supplied function (`map : (a ->{e} b) ->
/// List a ->{e} List b`), so you can map an impure function and the impurity flows
/// out (`DESIGN.md` §4). A single effect variable (id 0, bound in each scheme and
/// refreshed at instantiation, like the type vars) links the function arrow to the
/// traversal arrow.
fn seed_list_prelude(env: &mut Env) {
    let list = |t: Ty| Ty::Con("List".to_string(), vec![t]);
    let int = || Ty::Int(Unit::dimensionless());
    let pure_fn = |a: Ty, b: Ty| Ty::Fun(Box::new(a), Box::new(b), Effect::pure());
    let mono = |vars: Vec<u32>, ty: Ty| Scheme {
        vars,
        uvars: vec![],
        num_vars: vec![],
        ord_vars: vec![],
        eff_vars: vec![],
        mutable: false,
        ty,
    };
    // List.len : List a -> int   (pure)
    env.insert(
        "List.len".to_string(),
        mono(vec![0], pure_fn(list(Ty::Var(0)), int())),
    );
    // List.sum : List int -> int   (pure; MVP keeps it int-only)
    env.insert(
        "List.sum".to_string(),
        mono(vec![], pure_fn(list(int()), int())),
    );
    // List.rev : List a -> List a   (pure)
    env.insert(
        "List.rev".to_string(),
        mono(vec![0], pure_fn(list(Ty::Var(0)), list(Ty::Var(0)))),
    );
    // List.range : int -> int -> List int   (pure)
    env.insert(
        "List.range".to_string(),
        mono(vec![], pure_fn(int(), pure_fn(int(), list(int())))),
    );
    // List.zip : List a -> List b -> List (a, b)   (pure)
    env.insert(
        "List.zip".to_string(),
        mono(
            vec![0, 1],
            pure_fn(
                list(Ty::Var(0)),
                pure_fn(
                    list(Ty::Var(1)),
                    list(Ty::Tuple(vec![Ty::Var(0), Ty::Var(1)])),
                ),
            ),
        ),
    );
    let option = |t: Ty| Ty::Con("Option".to_string(), vec![t]);
    // List.get : int -> List a -> Option a   (pure, O(1) — bounds-checked, total)
    env.insert(
        "List.get".to_string(),
        mono(
            vec![0],
            pure_fn(int(), pure_fn(list(Ty::Var(0)), option(Ty::Var(0)))),
        ),
    );
    // List.isEmpty : List a -> bool   (pure, O(1))
    env.insert(
        "List.isEmpty".to_string(),
        mono(vec![0], pure_fn(list(Ty::Var(0)), Ty::Bool)),
    );
    // List.contains : a -> List a -> bool   (pure, O(n) linear scan; `Set` is O(1))
    env.insert(
        "List.contains".to_string(),
        mono(
            vec![0],
            pure_fn(Ty::Var(0), pure_fn(list(Ty::Var(0)), Ty::Bool)),
        ),
    );
    // List.concat : List a -> List a -> List a   (pure, O(n+m))
    env.insert(
        "List.concat".to_string(),
        mono(
            vec![0],
            pure_fn(
                list(Ty::Var(0)),
                pure_fn(list(Ty::Var(0)), list(Ty::Var(0))),
            ),
        ),
    );
    // List.sort : comparison a => List a -> List a   (pure, O(n log n))
    env.insert(
        "List.sort".to_string(),
        Scheme {
            vars: vec![0],
            uvars: vec![],
            num_vars: vec![],
            ord_vars: vec![0],
            eff_vars: vec![],
            mutable: false,
            ty: pure_fn(list(Ty::Var(0)), list(Ty::Var(0))),
        },
    );

    // Effect-polymorphic schemes share one bound effect variable `e` (id 0).
    let e = 0u32;
    let eff_scheme = |vars: Vec<u32>, ty: Ty| Scheme {
        vars,
        uvars: vec![],
        num_vars: vec![],
        ord_vars: vec![],
        eff_vars: vec![e],
        mutable: false,
        ty,
    };
    let arrow_e = |a: Ty, b: Ty| Ty::Fun(Box::new(a), Box::new(b), Effect::var(e));
    // List.map : (a ->{e} b) -> List a ->{e} List b
    env.insert(
        "List.map".to_string(),
        eff_scheme(
            vec![0, 1],
            pure_fn(
                arrow_e(Ty::Var(0), Ty::Var(1)),
                arrow_e(list(Ty::Var(0)), list(Ty::Var(1))),
            ),
        ),
    );
    // List.filter : (a ->{e} bool) -> List a ->{e} List a
    env.insert(
        "List.filter".to_string(),
        eff_scheme(
            vec![0],
            pure_fn(
                arrow_e(Ty::Var(0), Ty::Bool),
                arrow_e(list(Ty::Var(0)), list(Ty::Var(0))),
            ),
        ),
    );
    // List.fold : (b ->{e} a ->{e} b) -> b -> List a ->{e} b
    env.insert(
        "List.fold".to_string(),
        eff_scheme(
            vec![0, 1],
            pure_fn(
                arrow_e(Ty::Var(0), arrow_e(Ty::Var(1), Ty::Var(0))),
                pure_fn(Ty::Var(0), arrow_e(list(Ty::Var(1)), Ty::Var(0))),
            ),
        ),
    );
    // List.find : (a ->{e} bool) -> List a ->{e} Option a   (O(n), stops at the
    // first match; effect-polymorphic like filter)
    env.insert(
        "List.find".to_string(),
        eff_scheme(
            vec![0],
            pure_fn(
                arrow_e(Ty::Var(0), Ty::Bool),
                arrow_e(list(Ty::Var(0)), option(Ty::Var(0))),
            ),
        ),
    );
}

/// Seed the `Set` module ([`SET_PRELUDE`]) — pure functions over `Set a` (var 0),
/// under qualified keys (`Set.add`).
fn seed_set_prelude(env: &mut Env) {
    let set = |t: Ty| Ty::Con("Set".to_string(), vec![t]);
    let list = |t: Ty| Ty::Con("List".to_string(), vec![t]);
    let int = || Ty::Int(Unit::dimensionless());
    let pure_fn = |a: Ty, b: Ty| Ty::Fun(Box::new(a), Box::new(b), Effect::pure());
    let a = || Ty::Var(0);
    // Each scheme generalizes the one element variable (id 0).
    let scheme = |ty: Ty| Scheme {
        vars: vec![0],
        uvars: vec![],
        num_vars: vec![],
        ord_vars: vec![],
        eff_vars: vec![],
        mutable: false,
        ty,
    };
    let mut put = |member: &str, ty: Ty| {
        env.insert(format!("Set.{member}"), scheme(ty));
    };
    put("empty", set(a()));
    put("add", pure_fn(a(), pure_fn(set(a()), set(a()))));
    put("remove", pure_fn(a(), pure_fn(set(a()), set(a()))));
    put("contains", pure_fn(a(), pure_fn(set(a()), Ty::Bool)));
    put("len", pure_fn(set(a()), int()));
    put("union", pure_fn(set(a()), pure_fn(set(a()), set(a()))));
    put("intersect", pure_fn(set(a()), pure_fn(set(a()), set(a()))));
    put("difference", pure_fn(set(a()), pure_fn(set(a()), set(a()))));
    put("ofList", pure_fn(list(a()), set(a())));
    put("toList", pure_fn(set(a()), list(a())));
}

/// Seed the `String` module ([`STRING_PRELUDE`]) — pure, **monomorphic** text
/// operations over the built-in `string`, under qualified keys (`String.split`).
/// No type variables (all concrete over `string`/`int`/`float`), so the schemes are
/// closed. `String.toInt` returns `Option int` (a total parse).
fn seed_string_prelude(env: &mut Env) {
    let str_ = || Ty::Str;
    let int = || Ty::Int(Unit::dimensionless());
    let float = || Ty::Float(Unit::dimensionless());
    let list = |t: Ty| Ty::Con("List".to_string(), vec![t]);
    let opt = |t: Ty| Ty::Con("Option".to_string(), vec![t]);
    let pure_fn = |a: Ty, b: Ty| Ty::Fun(Box::new(a), Box::new(b), Effect::pure());
    // Closed schemes — no quantified type/unit/effect variables.
    let scheme = |ty: Ty| Scheme {
        vars: vec![],
        uvars: vec![],
        num_vars: vec![],
        ord_vars: vec![],
        eff_vars: vec![],
        mutable: false,
        ty,
    };
    let mut put = |member: &str, ty: Ty| {
        env.insert(format!("String.{member}"), scheme(ty));
    };
    put("len", pure_fn(str_(), int()));
    put("concat", pure_fn(str_(), pure_fn(str_(), str_())));
    // String.join sep xs — separator first (curry-friendly, F#-style).
    put("join", pure_fn(str_(), pure_fn(list(str_()), str_())));
    put("split", pure_fn(str_(), pure_fn(str_(), list(str_()))));
    put("upper", pure_fn(str_(), str_()));
    put("lower", pure_fn(str_(), str_()));
    put("strip", pure_fn(str_(), str_()));
    put("contains", pure_fn(str_(), pure_fn(str_(), Ty::Bool)));
    put("startsWith", pure_fn(str_(), pure_fn(str_(), Ty::Bool)));
    put("endsWith", pure_fn(str_(), pure_fn(str_(), Ty::Bool)));
    // String.replace old new s
    put(
        "replace",
        pure_fn(str_(), pure_fn(str_(), pure_fn(str_(), str_()))),
    );
    put("fromInt", pure_fn(int(), str_()));
    put("fromFloat", pure_fn(float(), str_()));
    put("toInt", pure_fn(str_(), opt(int())));
    // String.toFloat : string -> Option float  (a total parse, mirrors toInt).
    put("toFloat", pure_fn(str_(), opt(float())));
    // Each element of the result is a single-character string (no `char` type).
    put("toList", pure_fn(str_(), list(str_())));
    // String.slice start end s -> s[start:end]  (Python slice semantics: total,
    // end-exclusive, clamps out-of-range).
    put(
        "slice",
        pure_fn(int(), pure_fn(int(), pure_fn(str_(), str_()))),
    );
    // String.tryIndexOf sub s -> Some i (first occurrence) or None if absent.
    put("tryIndexOf", pure_fn(str_(), pure_fn(str_(), opt(int()))));
}

/// Seed the `Map` module ([`MAP_PRELUDE`]) — pure functions over `Map k v`
/// (key var 0, value var 1), under qualified keys (`Map.add`).
fn seed_map_prelude(env: &mut Env) {
    let map = |k: Ty, v: Ty| Ty::Con("Map".to_string(), vec![k, v]);
    let list = |t: Ty| Ty::Con("List".to_string(), vec![t]);
    let opt = |t: Ty| Ty::Con("Option".to_string(), vec![t]);
    let int = || Ty::Int(Unit::dimensionless());
    let pure_fn = |a: Ty, b: Ty| Ty::Fun(Box::new(a), Box::new(b), Effect::pure());
    let k = || Ty::Var(0);
    let v = || Ty::Var(1);
    // Each scheme generalizes both the key and value variables (ids 0 and 1).
    let scheme = |ty: Ty| Scheme {
        vars: vec![0, 1],
        uvars: vec![],
        num_vars: vec![],
        ord_vars: vec![],
        eff_vars: vec![],
        mutable: false,
        ty,
    };
    let mv = || map(k(), v());
    let mut put = |member: &str, ty: Ty| {
        env.insert(format!("Map.{member}"), scheme(ty));
    };
    put("empty", mv());
    put("add", pure_fn(k(), pure_fn(v(), pure_fn(mv(), mv()))));
    put("remove", pure_fn(k(), pure_fn(mv(), mv())));
    put("contains", pure_fn(k(), pure_fn(mv(), Ty::Bool)));
    put("findOr", pure_fn(k(), pure_fn(v(), pure_fn(mv(), v()))));
    put("tryFind", pure_fn(k(), pure_fn(mv(), opt(v()))));
    put("len", pure_fn(mv(), int()));
    put("keys", pure_fn(mv(), list(k())));
    put("values", pure_fn(mv(), list(v())));
    // Map.ofList : List (k, v) -> Map k v ; Map.toList : Map k v -> List (k, v)
    let pair = || list(Ty::Tuple(vec![k(), v()]));
    put("ofList", pure_fn(pair(), mv()));
    put("toList", pure_fn(mv(), pair()));
}

/// Seed the `Option` module ([`OPTION_PRELUDE`]) — combinators over the built-in
/// `Option a` (var 0). `Option.map` is effect-polymorphic like `List.map` (a single
/// bound effect variable, id 0, links the function arrow to the call), so mapping an
/// impure function over an option is `io`.
fn seed_option_prelude(env: &mut Env) {
    let opt = |t: Ty| Ty::Con("Option".to_string(), vec![t]);
    let pure_fn = |a: Ty, b: Ty| Ty::Fun(Box::new(a), Box::new(b), Effect::pure());
    let a = || Ty::Var(0);
    let b = || Ty::Var(1);
    let scheme = |vars: Vec<u32>, ty: Ty| Scheme {
        vars,
        uvars: vec![],
        num_vars: vec![],
        ord_vars: vec![],
        eff_vars: vec![],
        mutable: false,
        ty,
    };
    let mut put = |member: &str, sch: Scheme| {
        env.insert(format!("Option.{member}"), sch);
    };
    // Option.map : (a ->{e} b) -> Option a ->{e} Option b
    let e = 0u32;
    let arrow_e = |x: Ty, y: Ty| Ty::Fun(Box::new(x), Box::new(y), Effect::var(e));
    put(
        "map",
        Scheme {
            vars: vec![0, 1],
            uvars: vec![],
            num_vars: vec![],
            ord_vars: vec![],
            eff_vars: vec![e],
            mutable: false,
            ty: pure_fn(arrow_e(a(), b()), arrow_e(opt(a()), opt(b()))),
        },
    );
    // Option.bind : (a ->{e} Option b) -> Option a ->{e} Option b   (effect-poly)
    put(
        "bind",
        Scheme {
            vars: vec![0, 1],
            uvars: vec![],
            num_vars: vec![],
            ord_vars: vec![],
            eff_vars: vec![e],
            mutable: false,
            ty: pure_fn(arrow_e(a(), opt(b())), arrow_e(opt(a()), opt(b()))),
        },
    );
    // Option.filter : (a ->{e} bool) -> Option a ->{e} Option a   (effect-poly)
    put(
        "filter",
        Scheme {
            vars: vec![0],
            uvars: vec![],
            num_vars: vec![],
            ord_vars: vec![],
            eff_vars: vec![e],
            mutable: false,
            ty: pure_fn(arrow_e(a(), Ty::Bool), arrow_e(opt(a()), opt(a()))),
        },
    );
    put(
        "withDefault",
        scheme(vec![0], pure_fn(a(), pure_fn(opt(a()), a()))),
    );
    put("isSome", scheme(vec![0], pure_fn(opt(a()), Ty::Bool)));
    put("isNone", scheme(vec![0], pure_fn(opt(a()), Ty::Bool)));
    // Option.toResult : e -> Option a -> Result a e   (Some x -> Ok x, None ->
    // Error e; the inverse of Result.toOption). `b` (var 1) is the error type.
    let res = |a: Ty, e: Ty| Ty::Con("Result".to_string(), vec![a, e]);
    put(
        "toResult",
        scheme(vec![0, 1], pure_fn(b(), pure_fn(opt(a()), res(a(), b())))),
    );
}

/// Seed the `Result` module ([`RESULT_PRELUDE`]) — combinators over the built-in
/// `Result a e` (ok var 0, error var 1, mapped var 2). `Result.map`/`mapError`/
/// `bind` are effect-polymorphic (one bound effect variable, id 0), like
/// `List.map`/`Option.map`.
fn seed_result_prelude(env: &mut Env) {
    let res = |a: Ty, e: Ty| Ty::Con("Result".to_string(), vec![a, e]);
    let opt = |t: Ty| Ty::Con("Option".to_string(), vec![t]);
    let pure_fn = |a: Ty, b: Ty| Ty::Fun(Box::new(a), Box::new(b), Effect::pure());
    let a = || Ty::Var(0);
    let e = || Ty::Var(1);
    let m = || Ty::Var(2); // the mapped value/error variable
    let scheme = |vars: Vec<u32>, ty: Ty| Scheme {
        vars,
        uvars: vec![],
        num_vars: vec![],
        ord_vars: vec![],
        eff_vars: vec![],
        mutable: false,
        ty,
    };
    let mut put = |member: &str, sch: Scheme| {
        env.insert(format!("Result.{member}"), sch);
    };
    let ev = 0u32;
    let arrow_e = |x: Ty, y: Ty| Ty::Fun(Box::new(x), Box::new(y), Effect::var(ev));
    let eff_scheme = |vars: Vec<u32>, ty: Ty| Scheme {
        vars,
        uvars: vec![],
        num_vars: vec![],
        ord_vars: vec![],
        eff_vars: vec![ev],
        mutable: false,
        ty,
    };
    // Result.map : (a ->{e} m) -> Result a e -> Result m e   (maps the Ok value)
    put(
        "map",
        eff_scheme(
            vec![0, 1, 2],
            pure_fn(arrow_e(a(), m()), arrow_e(res(a(), e()), res(m(), e()))),
        ),
    );
    // Result.mapError : (e ->{x} m) -> Result a e -> Result a m
    put(
        "mapError",
        eff_scheme(
            vec![0, 1, 2],
            pure_fn(arrow_e(e(), m()), arrow_e(res(a(), e()), res(a(), m()))),
        ),
    );
    // Result.bind : (a ->{x} Result m e) -> Result a e -> Result m e
    put(
        "bind",
        eff_scheme(
            vec![0, 1, 2],
            pure_fn(
                arrow_e(a(), res(m(), e())),
                arrow_e(res(a(), e()), res(m(), e())),
            ),
        ),
    );
    // Result.withDefault : a -> Result a e -> a
    put(
        "withDefault",
        scheme(vec![0, 1], pure_fn(a(), pure_fn(res(a(), e()), a()))),
    );
    put("isOk", scheme(vec![0, 1], pure_fn(res(a(), e()), Ty::Bool)));
    put(
        "isError",
        scheme(vec![0, 1], pure_fn(res(a(), e()), Ty::Bool)),
    );
    // Result.toOption : Result a e -> Option a
    put(
        "toOption",
        scheme(vec![0, 1], pure_fn(res(a(), e()), opt(a()))),
    );
}

/// Seed the `Seq` module ([`SEQ_PRELUDE`]) — the lazy counterpart to `List` over
/// `Seq a` (var 0, mapped var 1). `Seq.map`/`filter`/`fold` are effect-polymorphic
/// (one bound effect variable, id 0), like `List.map`.
fn seed_seq_prelude(env: &mut Env) {
    let seq = |t: Ty| Ty::Con("Seq".to_string(), vec![t]);
    let list = |t: Ty| Ty::Con("List".to_string(), vec![t]);
    let int = || Ty::Int(Unit::dimensionless());
    let pure_fn = |a: Ty, b: Ty| Ty::Fun(Box::new(a), Box::new(b), Effect::pure());
    let a = || Ty::Var(0);
    let b = || Ty::Var(1);
    let mono = |vars: Vec<u32>, ty: Ty| Scheme {
        vars,
        uvars: vec![],
        num_vars: vec![],
        ord_vars: vec![],
        eff_vars: vec![],
        mutable: false,
        ty,
    };
    let ev = 0u32;
    let arrow_e = |x: Ty, y: Ty| Ty::Fun(Box::new(x), Box::new(y), Effect::var(ev));
    let eff_scheme = |vars: Vec<u32>, ty: Ty| Scheme {
        vars,
        uvars: vec![],
        num_vars: vec![],
        ord_vars: vec![],
        eff_vars: vec![ev],
        mutable: false,
        ty,
    };
    let mut put = |member: &str, sch: Scheme| {
        env.insert(format!("Seq.{member}"), sch);
    };
    // Seq.map : (a ->{e} b) -> Seq a ->{e} Seq b   (lazy, but the effect still flows)
    put(
        "map",
        eff_scheme(
            vec![0, 1],
            pure_fn(arrow_e(a(), b()), arrow_e(seq(a()), seq(b()))),
        ),
    );
    // Seq.filter : (a ->{e} bool) -> Seq a ->{e} Seq a
    put(
        "filter",
        eff_scheme(
            vec![0],
            pure_fn(arrow_e(a(), Ty::Bool), arrow_e(seq(a()), seq(a()))),
        ),
    );
    // Seq.take : int -> Seq a -> Seq a
    put(
        "take",
        mono(vec![0], pure_fn(int(), pure_fn(seq(a()), seq(a())))),
    );
    // Seq.fold : (b ->{e} a ->{e} b) -> b -> Seq a ->{e} b   (forces the sequence)
    put(
        "fold",
        eff_scheme(
            vec![0, 1],
            pure_fn(
                arrow_e(a(), arrow_e(b(), a())),
                pure_fn(a(), arrow_e(seq(b()), a())),
            ),
        ),
    );
    // Seq.toList : Seq a -> List a   (forces)
    put("toList", mono(vec![0], pure_fn(seq(a()), list(a()))));
    // Seq.ofList : List a -> Seq a
    put("ofList", mono(vec![0], pure_fn(list(a()), seq(a()))));
    // Seq.range : int -> int -> Seq int
    put(
        "range",
        mono(vec![], pure_fn(int(), pure_fn(int(), seq(int())))),
    );
}

/// Seed the built-in computation-expression types `Async a`, `Seq a`, and
/// `Result a e` (with constructors `Ok`/`Error`) — see `DESIGN.md` §8.1.
fn seed_builtin_types(decls: &mut Decls, env: &mut Env) {
    decls.type_arity.insert("Async".to_string(), 1);
    decls.type_arity.insert("Seq".to_string(), 1);
    decls.type_arity.insert("Result".to_string(), 2);
    // `List a` — the eager collection (lowers to a Python list). It has no
    // constructors (list patterns in `match` are deferred), so no `type_ctors`.
    decls.type_arity.insert("List".to_string(), 1);
    // `Set a` / `Map k v` — the hashed collections (lower to Python `set`/`dict`).
    // Built purely from the `Set.*` / `Map.*` modules (no constructors, no literal
    // syntax — `{…}` is records/CEs).
    decls.type_arity.insert("Set".to_string(), 1);
    decls.type_arity.insert("Map".to_string(), 2);
    // `Option a` — the built-in optional type (constructors `Some`/`None`), seeded
    // like `Result`. Reserved, so a user `type Option` is an error.
    decls.type_arity.insert("Option".to_string(), 1);
    decls.type_ctors.insert("Async".to_string(), Vec::new());
    decls.type_ctors.insert("Seq".to_string(), Vec::new());
    decls.type_ctors.insert(
        "Result".to_string(),
        vec!["Ok".to_string(), "Error".to_string()],
    );
    decls.type_ctors.insert(
        "Option".to_string(),
        vec!["Some".to_string(), "None".to_string()],
    );

    let result_ty = Ty::Con("Result".to_string(), vec![Ty::Var(0), Ty::Var(1)]);
    let ok = Scheme {
        vars: vec![0, 1],
        uvars: vec![],
        num_vars: vec![],
        ord_vars: vec![],
        eff_vars: vec![],
        mutable: false,
        ty: Ty::Fun(
            Box::new(Ty::Var(0)),
            Box::new(result_ty.clone()),
            Effect::pure(),
        ),
    };
    let err = Scheme {
        vars: vec![0, 1],
        uvars: vec![],
        num_vars: vec![],
        ord_vars: vec![],
        eff_vars: vec![],
        mutable: false,
        ty: Ty::Fun(Box::new(Ty::Var(1)), Box::new(result_ty), Effect::pure()),
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

    // `Option a` with constructors `Some : a -> Option a` and `None : Option a`.
    let option_ty = Ty::Con("Option".to_string(), vec![Ty::Var(0)]);
    let some = Scheme {
        vars: vec![0],
        uvars: vec![],
        num_vars: vec![],
        ord_vars: vec![],
        eff_vars: vec![],
        mutable: false,
        ty: Ty::Fun(
            Box::new(Ty::Var(0)),
            Box::new(option_ty.clone()),
            Effect::pure(),
        ),
    };
    let none = Scheme {
        vars: vec![0],
        uvars: vec![],
        num_vars: vec![],
        ord_vars: vec![],
        eff_vars: vec![],
        mutable: false,
        ty: option_ty,
    };
    env.insert("Some".to_string(), some.clone());
    env.insert("None".to_string(), none.clone());
    decls.ctors.insert(
        "Some".to_string(),
        CtorInfo {
            scheme: some,
            arity: 1,
        },
    );
    decls.ctors.insert(
        "None".to_string(),
        CtorInfo {
            scheme: none,
            arity: 0,
        },
    );

    // `Exception` — the reserved record carrying a caught Python exception's
    // `errorKind` (the class name, `type(e).__name__`) and `errorMessage` (`str(e)`).
    // Produced only by `try e : Result a Exception` (`ExprKind::Try`). Reserved (a
    // user `type Exception` is rejected via the `type_arity` clash), and it reuses
    // the ordinary record machinery, so its fields join the global field registry.
    decls.type_arity.insert("Exception".to_string(), 0);
    decls.records.insert(
        "Exception".to_string(),
        RecordInfo {
            params_count: 0,
            fields: vec![
                ("errorKind".to_string(), Ty::Str),
                ("errorMessage".to_string(), Ty::Str),
            ],
        },
    );
    decls
        .field_owner
        .entry("errorKind".to_string())
        .or_default()
        .push("Exception".to_string());
    decls
        .field_owner
        .entry("errorMessage".to_string())
        .or_default()
        .push("Exception".to_string());
    decls.local_records.insert("Exception".to_string());
}

/// Resolve a surface type expression into a [`Ty`].
fn resolve(
    ty: &TypeExpr,
    params: &HashMap<String, u32>,
    type_arity: &HashMap<String, usize>,
    span: Span,
) -> Result<Ty, TypeError> {
    match ty {
        // A function type written in a declaration (an ADT/record field or an
        // `extern` signature) carries the effect labels of its `->{...}`
        // annotation; a bare `->` stays pure (`DESIGN.md` §4).
        TypeExpr::Fun(a, b, effects) => {
            let mut eff = Effect::pure();
            for name in effects {
                match EffLabel::from_name(name) {
                    Some(label) => {
                        eff.labels.insert(label);
                    }
                    None => {
                        return Err(TypeError {
                            message: format!(
                                "unknown effect label `{name}` (known labels: `io`, `async`)"
                            ),
                            span,
                        });
                    }
                }
            }
            Ok(Ty::Fun(
                Box::new(resolve(a, params, type_arity, span)?),
                Box::new(resolve(b, params, type_arity, span)?),
                eff,
            ))
        }
        TypeExpr::Tuple(elems) => {
            let resolved: Result<Vec<Ty>, TypeError> = elems
                .iter()
                .map(|e| resolve(e, params, type_arity, span))
                .collect();
            Ok(Ty::Tuple(resolved?))
        }
        TypeExpr::Con(name, _, args) => {
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
                "int" => return no_args(Ty::Int(Unit::dimensionless())),
                "float" => return no_args(Ty::Float(Unit::dimensionless())),
                "bool" => return no_args(Ty::Bool),
                "string" => return no_args(Ty::Str),
                "unit" => return no_args(Ty::Unit),
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
    unit_subst: HashMap<u32, Unit>,
    /// Resolution of `num` (numeric base) variables — a small union-find: a var
    /// maps to a concrete `Int`/`Float` or to another `num` var.
    num_subst: HashMap<u32, NumRef>,
    /// Type variables carrying the `comparison` constraint (from `< > <= >=`).
    ord: HashSet<u32>,
    /// Resolution of effect variables.
    eff_subst: HashMap<u32, Effect>,
    /// The effect accumulated for the expression currently being inferred. Saved
    /// and reset around function bodies so a body's effect becomes the arrow's
    /// latent effect rather than leaking into the enclosing expression.
    cur_eff: Effect,
    next: u32,
    decls: Decls,
    /// When set, [`infer_expr`](Infer::infer_expr) records the inferred type of
    /// every expression node into [`recorded`](Infer::recorded) for editor hover.
    record_types: bool,
    /// Collected `(span, ty)` pairs (unresolved — resolved in [`run`] once the
    /// substitution is final). Empty unless `record_types` is set.
    recorded: Vec<(Span, Ty)>,
    /// Set per `match` by [`check_exhaustive`](Infer::check_exhaustive) when an arm
    /// contains a suffix-star list pattern (`[*r, s…]`) — the only patterns that can
    /// reproduce themselves under specialization — enabling the cycle guard in
    /// [`useful`](Infer::useful). Off for every other program, so the guard is
    /// zero-cost in the common case.
    seq_guard: bool,
    /// The usefulness sub-problems currently on the recursion stack (keyed on the
    /// order-normalized matrix + column types), for the suffix-star cycle guard.
    seq_visiting: HashSet<String>,
}

impl Infer {
    /// Allocate a fresh variable id (shared id space for type/unit/num vars).
    fn fresh_id(&mut self) -> u32 {
        let id = self.next;
        self.next += 1;
        id
    }

    /// A fresh polymorphic numeric (a `num` variable), dimensionless. Integer
    /// literals get one of these so they adapt to int or float by context.
    fn fresh_num(&mut self) -> Ty {
        Ty::Num(self.fresh_id(), Unit::dimensionless())
    }

    fn fresh(&mut self) -> Ty {
        let id = self.next;
        self.next += 1;
        Ty::Var(id)
    }

    fn fresh_unit(&mut self) -> Unit {
        let id = self.next;
        self.next += 1;
        Unit::var(id)
    }

    /// Resolve a type through the type and unit substitutions.
    fn apply(&self, ty: &Ty) -> Ty {
        match ty {
            Ty::Var(n) => match self.subst.get(n) {
                Some(t) => self.apply(&t.clone()),
                None => Ty::Var(*n),
            },
            Ty::Int(u) => Ty::Int(self.apply_unit(u)),
            Ty::Float(u) => Ty::Float(self.apply_unit(u)),
            Ty::Num(v, u) => self.num_ty(NumRef::Var(*v), self.apply_unit(u)),
            Ty::Fun(a, b, eff) => Ty::Fun(
                Box::new(self.apply(a)),
                Box::new(self.apply(b)),
                self.apply_eff(eff),
            ),
            Ty::Con(name, args) => {
                Ty::Con(name.clone(), args.iter().map(|a| self.apply(a)).collect())
            }
            Ty::Tuple(elems) => Ty::Tuple(elems.iter().map(|a| self.apply(a)).collect()),
            other => other.clone(),
        }
    }

    /// Resolve an effect through the effect substitution, unioning bound vars.
    fn apply_eff(&self, eff: &Effect) -> Effect {
        let mut out = Effect {
            labels: eff.labels.clone(),
            vars: std::collections::BTreeSet::new(),
        };
        for v in &eff.vars {
            match self.eff_subst.get(v) {
                Some(bound) => out = out.union(&self.apply_eff(&bound.clone())),
                None => {
                    out.vars.insert(*v);
                }
            }
        }
        out
    }

    /// A fresh, open (polymorphic) effect variable.
    fn fresh_eff(&mut self) -> Effect {
        Effect::var(self.fresh_id())
    }

    /// Accumulate `eff` into the effect of the expression currently being inferred.
    fn perform(&mut self, eff: &Effect) {
        self.cur_eff = self.cur_eff.union(eff);
    }

    /// Unify two effects (latent effects of two arrows being unified). Binds a
    /// bare effect variable most-generally; widens to the joined label set
    /// otherwise; fails only when two closed effects disagree on their labels.
    fn unify_eff(&mut self, a: &Effect, b: &Effect) -> bool {
        let a = self.apply_eff(a);
        let b = self.apply_eff(b);
        if a == b {
            return true;
        }
        if let Some(v) = a.as_single_var()
            && !b.vars.contains(&v)
        {
            self.eff_subst.insert(v, b);
            return true;
        }
        if let Some(v) = b.as_single_var()
            && !a.vars.contains(&v)
        {
            self.eff_subst.insert(v, a);
            return true;
        }
        if a.vars.is_empty() && b.vars.is_empty() {
            return false; // two closed effects, their label sets differ
        }
        // Conservatively widen: close every involved variable to the joined labels.
        let joined = Effect {
            labels: a.labels.union(&b.labels).cloned().collect(),
            vars: std::collections::BTreeSet::new(),
        };
        for v in a.vars.iter().chain(b.vars.iter()) {
            self.eff_subst.insert(*v, joined.clone());
        }
        true
    }

    fn apply_unit(&self, u: &Unit) -> Unit {
        let mut r = Unit::dimensionless();
        for (a, e) in &u.factors {
            match a {
                Atom::Var(v) => match self.unit_subst.get(v) {
                    Some(bound) => {
                        let resolved = self.apply_unit(&bound.clone());
                        r = r.mul(&resolved.pow(*e));
                    }
                    None => r.insert(a.clone(), *e),
                },
                Atom::Base(_) => r.insert(a.clone(), *e),
            }
        }
        r
    }

    /// Follow the `num` union-find to a representative (a concrete base, or an
    /// unbound `num` var).
    fn resolve_num(&self, v: u32) -> NumRef {
        match self.num_subst.get(&v) {
            Some(NumRef::Var(w)) => self.resolve_num(*w),
            Some(other) => *other,
            None => NumRef::Var(v),
        }
    }

    /// Build the concrete numeric type for a (possibly variable) base + unit. An
    /// unresolved base stays `Num`; a resolved one becomes `Int`/`Float`.
    fn num_ty(&self, base: NumRef, unit: Unit) -> Ty {
        let base = match base {
            NumRef::Var(v) => self.resolve_num(v),
            concrete => concrete,
        };
        match base {
            NumRef::Int => Ty::Int(unit),
            NumRef::Float => Ty::Float(unit),
            NumRef::Var(v) => Ty::Num(v, unit),
        }
    }

    /// Unify two numeric bases. Returns false only on a genuine `int` vs `float`
    /// clash (Pyfun does not implicitly coerce between numeric bases — §7.1).
    fn unify_num(&mut self, a: NumRef, b: NumRef) -> bool {
        let a = match a {
            NumRef::Var(v) => self.resolve_num(v),
            c => c,
        };
        let b = match b {
            NumRef::Var(v) => self.resolve_num(v),
            c => c,
        };
        match (a, b) {
            (NumRef::Int, NumRef::Int) | (NumRef::Float, NumRef::Float) => true,
            (NumRef::Int, NumRef::Float) | (NumRef::Float, NumRef::Int) => false,
            (NumRef::Var(x), NumRef::Var(y)) => {
                if x != y {
                    self.num_subst.insert(x, NumRef::Var(y));
                }
                true
            }
            (NumRef::Var(x), c) | (c, NumRef::Var(x)) => {
                self.num_subst.insert(x, c);
                true
            }
        }
    }

    /// Require `ty` to be numeric, returning its base and unit (binding a bare
    /// type variable to a fresh `num` if needed). Diagnostics read "int" because
    /// `int` is the default numeric (§7.1).
    fn expect_num(&mut self, ty: &Ty, span: Span) -> Result<(NumRef, Unit), TypeError> {
        match self.apply(ty) {
            Ty::Int(u) => Ok((NumRef::Int, u)),
            Ty::Float(u) => Ok((NumRef::Float, u)),
            Ty::Num(v, u) => Ok((NumRef::Var(v), u)),
            Ty::Var(n) => {
                let v = self.fresh_id();
                let u = self.fresh_unit();
                self.subst.insert(n, Ty::Num(v, u.clone()));
                Ok((NumRef::Var(v), u))
            }
            other => Err(TypeError {
                message: format!("expected int, found {}", show(&other)),
                span,
            }),
        }
    }

    /// Arithmetic (`+ - * / //`): both operands the same numeric base (no int/float
    /// mixing beyond literal adaptation — §7.1). `+ - * //` keep that base; `/` is
    /// true division and always yields `float`.
    fn infer_arithmetic(
        &mut self,
        op: BinOp,
        lhs: &Expr,
        rhs: &Expr,
        span: Span,
        env: &Env,
    ) -> Result<Ty, TypeError> {
        let lt = self.infer_expr(lhs, env)?;
        let (lb, lu) = self
            .expect_num(&lt, lhs.span())
            .map_err(|e| self.string_concat_hint(op, &lt, e))?;
        let rt = self.infer_expr(rhs, env)?;
        let (rb, ru) = self
            .expect_num(&rt, rhs.span())
            .map_err(|e| self.string_concat_hint(op, &rt, e))?;
        let base_clash = |this: &Self| {
            mismatch(
                &this.num_ty(lb, this.apply_unit(&lu)),
                &this.num_ty(rb, this.apply_unit(&ru)),
                span,
            )
        };
        if !self.unify_num(lb, rb) {
            return Err(base_clash(self));
        }
        match op {
            // `%` preserves the (shared) unit like `+`/`-` — `10<m> % 3<m> : int<m>`,
            // and mixing units is an error — and the numeric base (int%int=int,
            // float%float=float).
            BinOp::Add | BinOp::Sub | BinOp::Mod => {
                if !self.unify_unit(&lu, &ru) {
                    return Err(base_clash(self));
                }
                Ok(self.num_ty(lb, self.apply_unit(&lu)))
            }
            BinOp::Mul => Ok(self.num_ty(lb, self.apply_unit(&lu).mul(&self.apply_unit(&ru)))),
            BinOp::Div => Ok(Ty::Float(self.apply_unit(&lu).div(&self.apply_unit(&ru)))),
            BinOp::FloorDiv => Ok(self.num_ty(lb, self.apply_unit(&lu).div(&self.apply_unit(&ru)))),
            // infer_arithmetic is only called for the five arithmetic operators.
            _ => unreachable!("non-arithmetic operator in infer_arithmetic"),
        }
    }

    /// Turn the generic "expected int, found string" from an `expect_num` failure
    /// into a guiding hint when the user wrote `+` between strings — `+` is numeric
    /// (§7.1), and `String.concat` is the concatenation path. Only augments the
    /// `Add`-on-`string` case; every other numeric mismatch keeps its message.
    fn string_concat_hint(&self, op: BinOp, operand: &Ty, mut err: TypeError) -> TypeError {
        if op == BinOp::Add && matches!(self.apply(operand), Ty::Str) {
            err.message =
                "`+` is numeric and does not concatenate strings — use `String.concat a b`"
                    .to_string();
        }
        err
    }

    /// Require `ty` to support ordering comparison (`< > <= >=`): the `comparison`
    /// constraint, satisfied by `int`/`float`/`string` (numbers and strings), plus
    /// **tuples** and **user sum types / records** compared structurally
    /// (`DESIGN.md` §7.1). A bare type variable gains the constraint and is checked
    /// once it resolves (the [`unify`](Self::unify) hook re-runs this).
    fn require_ord(&mut self, ty: &Ty, span: Span) -> Result<(), TypeError> {
        let mut visiting = HashSet::new();
        self.require_ord_rec(ty, span, &mut visiting)
    }

    /// The recursive core of [`require_ord`]. `visiting` holds the structural keys
    /// (`show`) of the `Con` types currently being expanded, so a recursive type
    /// (`type Tree = Leaf int | Node Tree Tree`) terminates: re-visiting the same
    /// `(name, args)` is treated as satisfied, sound by structural induction (a
    /// recursive type is orderable iff its non-recursive parts are). Keying on the
    /// full applied type distinguishes a recursive occurrence `List a` from a genuine
    /// nesting `List (List a)`.
    fn require_ord_rec(
        &mut self,
        ty: &Ty,
        span: Span,
        visiting: &mut HashSet<String>,
    ) -> Result<(), TypeError> {
        let applied = self.apply(ty);
        let unsupported = |t: &Ty| TypeError {
            message: format!("type {} does not support comparison (`<`)", show(t)),
            span,
        };
        match &applied {
            Ty::Int(_) | Ty::Float(_) | Ty::Num(_, _) | Ty::Str => Ok(()),
            Ty::Var(n) => {
                self.ord.insert(*n);
                Ok(())
            }
            // A tuple is orderable iff every element is (lexicographic, matching the
            // Python tuple it lowers to — no codegen needed).
            Ty::Tuple(elems) => {
                let elems = elems.clone();
                for e in &elems {
                    self.require_ord_rec(e, span, visiting)?;
                }
                Ok(())
            }
            Ty::Con(name, args) => {
                let name = name.clone();
                let args = args.clone();
                // Only *user* sum types / records derive ordering; built-in containers
                // and reserved types (`Option`/`Result`/`Set`/`Map`/…) do not.
                let is_user = !RESERVED_UNORDERED.contains(&name.as_str())
                    && (self.decls.type_ctors.contains_key(&name)
                        || self.decls.records.contains_key(&name));
                if !is_user {
                    return Err(unsupported(&applied));
                }
                // Recursion guard (and memo): a re-visited type is satisfied.
                if !visiting.insert(show(&applied)) {
                    return Ok(());
                }
                if visiting.len() > MAX_ORD_DEPTH {
                    return Err(TypeError {
                        message: format!(
                            "type {} is too deeply nested to check for comparison",
                            show(&applied)
                        ),
                        span,
                    });
                }
                for ft in self.ord_field_types(&name, &args) {
                    self.require_ord_rec(&ft, span, visiting)?;
                }
                Ok(())
            }
            other => Err(unsupported(other)),
        }
    }

    /// The field types a user sum type / record contributes to its ordering key, with
    /// the type parameters substituted by `args` (a ctor/record field type uses bound
    /// vars `0..params_count`). For a sum type: every constructor's fields, in
    /// declaration order; for a record: its fields.
    fn ord_field_types(&self, name: &str, args: &[Ty]) -> Vec<Ty> {
        let tmap: HashMap<u32, Ty> = args
            .iter()
            .cloned()
            .enumerate()
            .map(|(i, t)| (i as u32, t))
            .collect();
        let (eu, en, ee) = (HashMap::new(), HashMap::new(), HashMap::new());
        let subst = |t: &Ty| subst_all(t, &tmap, &eu, &en, &ee);
        let mut out = Vec::new();
        if let Some(ctors) = self.decls.type_ctors.get(name) {
            for cn in ctors {
                let info = &self.decls.ctors[cn];
                let (fields, _) = split_fun(&info.scheme.ty, info.arity);
                out.extend(fields.iter().map(&subst));
            }
        } else if let Some(rec) = self.decls.records.get(name) {
            out.extend(rec.fields.iter().map(|(_, t)| subst(t)));
        }
        out
    }

    /// Infer a binding's scheme and its effect. The body's effect lands on the
    /// innermost arrow (a function definition is itself pure) or, for a value
    /// binding, is the binding's effect (and leaks to the enclosing scope, since
    /// evaluating the binding performs it).
    fn infer_binding(
        &mut self,
        binding: &LetBinding,
        env: &Env,
    ) -> Result<(Scheme, Effect), TypeError> {
        let outer = std::mem::replace(&mut self.cur_eff, Effect::pure());
        let ty_res = if binding.params.is_empty() {
            self.infer_expr(&binding.value, env)
        } else {
            let mut body_env = env.clone();
            // Self-recursion: a function binding is in scope inside its own body
            // (like Python's `def`), so a recursive call resolves. Monomorphic
            // recursion — the name gets one fresh type that is unified with the
            // binding's eventual function type once the body is inferred. A plain
            // value binding is *not* made self-visible (only the `params` branch is
            // reached here, so `let x = x` still errors as unbound — matching Python,
            // where `x = x` is a module-level NameError). Mutual recursion across
            // bindings is not supported (declare-before-use; `DESIGN.md` §6.1).
            let self_ty = self.fresh();
            body_env.insert(binding.name.clone(), Scheme::mono(self_ty.clone()));
            let mut param_tys = Vec::with_capacity(binding.params.len());
            for param in &binding.params {
                let pty = self.fresh();
                param_tys.push(pty.clone());
                if self.record_types {
                    self.recorded.push((param.span.span(), pty.clone()));
                }
                body_env.insert(param.name.clone(), Scheme::mono(pty));
            }
            self.infer_expr(&binding.value, &body_env)
                .and_then(|body_ty| {
                    // The innermost arrow (applied last) carries the body's effect;
                    // the outer, currying arrows are pure (they just build closures).
                    let mut ty = body_ty;
                    let mut eff = self.cur_eff.clone();
                    for p in param_tys.into_iter().rev() {
                        ty = Ty::Fun(Box::new(p), Box::new(ty), eff);
                        eff = Effect::pure();
                    }
                    // Tie the recursive knot: the name as used in the body *is* this
                    // function. Monomorphic, so no polymorphic recursion.
                    self.unify(&self_ty, &ty, binding.value.span())?;
                    Ok(ty)
                })
        };
        let body_eff = self.cur_eff.clone();
        // Restore the enclosing effect: a function definition leaks nothing; a
        // value binding's evaluation performs its effect.
        self.cur_eff = if binding.params.is_empty() {
            outer.union(&body_eff)
        } else {
            outer
        };
        let ty = ty_res?;

        // `let pure` asserts the binding introduces no concrete effect of its own
        // (effect variables — "pure up to its arguments" — are fine).
        let resolved_body_eff = self.apply_eff(&body_eff);
        if binding.pure && !resolved_body_eff.labels.is_empty() {
            return Err(TypeError {
                message: format!(
                    "`{}` is declared `pure` but performs `{}`",
                    binding.name,
                    resolved_body_eff.show_labels()
                ),
                span: binding.value.span(),
            });
        }

        // Record the binding's inferred type against its name span, so hovering a
        // definition (e.g. a function name) shows its signature — the headline
        // hover case, since Pyfun signatures are inferred, never written.
        if self.record_types {
            self.recorded.push((binding.name_span.span(), ty.clone()));
        }

        // A `mut` binding is monomorphic (so a later `<-` can't change its type)
        // and cannot be a function — only a reassignable value.
        let scheme = if binding.mutable {
            if !binding.params.is_empty() {
                return Err(TypeError {
                    message: format!(
                        "a mutable binding cannot take parameters (`{}` is `let mut`)",
                        binding.name
                    ),
                    span: binding.value.span(),
                });
            }
            let mut scheme = Scheme::mono(self.apply(&ty));
            scheme.mutable = true;
            scheme
        } else {
            self.generalize(env, &ty)
        };
        Ok((scheme, self.apply_eff(&body_eff)))
    }

    /// Infer a group of mutually-recursive **function** bindings together
    /// (`DESIGN.md` §7.1): pre-bind every member at a fresh monomorphic type so
    /// each body sees the others, infer all bodies, tie each recursive knot, then
    /// generalize each against the *outer* env (not the group's own mono bindings) —
    /// so the group is monomorphic within itself but each member stays polymorphic
    /// to the rest of the module. Extends `infer_binding`'s single-binding
    /// self-recursion across the group. Returns one `(name, scheme-or-error)` per
    /// member, in the given order. Callers pass only all-function groups.
    fn infer_mutual_group(
        &mut self,
        group: &[&LetBinding],
        outer_env: &Env,
    ) -> Vec<(String, Result<Scheme, TypeError>)> {
        // Pre-bind each member's name at a fresh monomorphic type.
        let mut body_env = outer_env.clone();
        let mut self_tys = Vec::with_capacity(group.len());
        for b in group {
            let t = self.fresh();
            body_env.insert(b.name.clone(), Scheme::mono(t.clone()));
            self_tys.push(t);
        }
        // Infer each body against the group env plus its own params; tie the knot.
        let mut inferred: Vec<Result<(Ty, Effect), TypeError>> = Vec::with_capacity(group.len());
        for (b, self_ty) in group.iter().zip(&self_tys) {
            let outer_eff = std::mem::replace(&mut self.cur_eff, Effect::pure());
            let mut fn_env = body_env.clone();
            let mut param_tys = Vec::with_capacity(b.params.len());
            for p in &b.params {
                let pty = self.fresh();
                param_tys.push(pty.clone());
                if self.record_types {
                    self.recorded.push((p.span.span(), pty.clone()));
                }
                fn_env.insert(p.name.clone(), Scheme::mono(pty));
            }
            let res = self.infer_expr(&b.value, &fn_env).and_then(|body_ty| {
                let mut ty = body_ty;
                let mut eff = self.cur_eff.clone();
                for p in param_tys.into_iter().rev() {
                    ty = Ty::Fun(Box::new(p), Box::new(ty), eff);
                    eff = Effect::pure();
                }
                self.unify(self_ty, &ty, b.value.span())?;
                Ok(ty)
            });
            let body_eff = self.cur_eff.clone();
            self.cur_eff = outer_eff; // a function definition leaks no effect
            inferred.push(res.map(|ty| (ty, body_eff)));
        }
        // Generalize each against the outer env (the mono pre-bindings live only in
        // `body_env`, so they do not block generalization of the members' vars).
        group
            .iter()
            .zip(inferred)
            .map(|(b, res)| {
                let scheme = res.and_then(|(ty, body_eff)| {
                    let body_eff = self.apply_eff(&body_eff);
                    if b.pure && !body_eff.labels.is_empty() {
                        return Err(TypeError {
                            message: format!(
                                "`{}` is declared `pure` but performs `{}`",
                                b.name,
                                body_eff.show_labels()
                            ),
                            span: b.value.span(),
                        });
                    }
                    if self.record_types {
                        self.recorded.push((b.name_span.span(), ty.clone()));
                    }
                    Ok(self.generalize(outer_env, &ty))
                });
                (b.name.clone(), scheme)
            })
            .collect()
    }

    /// Infer the type of `expr`, recording it for hover when `record_types` is
    /// set. The recording happens here (around the real inference in
    /// [`infer_expr_inner`](Infer::infer_expr_inner)) so every subexpression is
    /// captured with a single hook, regardless of which arm produced it.
    fn infer_expr(&mut self, expr: &Expr, env: &Env) -> Result<Ty, TypeError> {
        let ty = self.infer_expr_inner(expr, env)?;
        if self.record_types {
            self.recorded.push((expr.span(), ty.clone()));
        }
        Ok(ty)
    }

    fn infer_expr_inner(&mut self, expr: &Expr, env: &Env) -> Result<Ty, TypeError> {
        let span = expr.span();
        match &expr.kind {
            // Integer literals are polymorphic numerics (`num 'a => 'a`) so they
            // adapt to int or float by context; float literals are concretely
            // float (§7.1).
            ExprKind::Int(_) => Ok(self.fresh_num()),
            ExprKind::Float(_) => Ok(Ty::Float(Unit::dimensionless())),
            ExprKind::Str(_) => Ok(Ty::Str),
            // An interpolated string is a `string`; each hole may be any type (the
            // emitted Python f-string stringifies it). We still infer every hole so
            // its effect propagates (`f"{impure x}"` is `io`).
            ExprKind::Interp { parts } => {
                for part in parts {
                    if let InterpPart::Expr(e) = part {
                        self.infer_expr(e, env)?;
                    }
                }
                Ok(Ty::Str)
            }
            ExprKind::Bool(_) => Ok(Ty::Bool),
            ExprKind::Unit => Ok(Ty::Unit),

            ExprKind::Var(name) => match env.get(name) {
                Some(scheme) => Ok(self.instantiate(scheme)),
                None if MODULES.contains(&name.as_str()) || self.decls.modules.contains(name) => {
                    Err(TypeError {
                        message: format!("`{name}` is a module; use `{name}.member`"),
                        span,
                    })
                }
                None => Err(TypeError {
                    message: format!("unbound name `{name}`"),
                    span,
                }),
            },

            ExprKind::Annot { value, unit } => {
                let vt = self.infer_expr(value, env)?;
                let u = self.resolve_unit_expr(unit, span)?;
                match self.apply(&vt) {
                    Ty::Int(_) => Ok(Ty::Int(u)),
                    Ty::Float(_) => Ok(Ty::Float(u)),
                    // A polymorphic numeric literal keeps its base var, gaining
                    // the annotated unit (`5<m>` : `num 'a => 'a<m>`).
                    Ty::Num(v, _) => Ok(Ty::Num(v, u)),
                    other => Err(TypeError {
                        message: format!(
                            "unit annotations apply only to numeric values, not {}",
                            show(&other)
                        ),
                        span,
                    }),
                }
            }

            ExprKind::Binary { op, lhs, rhs } => match op {
                BinOp::Add
                | BinOp::Sub
                | BinOp::Mul
                | BinOp::Div
                | BinOp::FloorDiv
                | BinOp::Mod => self.infer_arithmetic(*op, lhs, rhs, span, env),
                // `**` is float-only and dimensionless (units + a runtime exponent
                // can't be dimensionally checked). Num literals coerce to float.
                BinOp::Pow => {
                    let float = || Ty::Float(Unit::dimensionless());
                    let lt = self.infer_expr(lhs, env)?;
                    self.unify(&float(), &lt, lhs.span())?;
                    let rt = self.infer_expr(rhs, env)?;
                    self.unify(&float(), &rt, rhs.span())?;
                    Ok(float())
                }
                // Equality: operands of the same type, result `bool` (§7.1). No
                // constraint — every type has equality (ADTs structurally).
                BinOp::Eq | BinOp::Ne => {
                    let lt = self.infer_expr(lhs, env)?;
                    let rt = self.infer_expr(rhs, env)?;
                    self.unify(&lt, &rt, span)?;
                    Ok(Ty::Bool)
                }
                // Ordering: same type, result `bool`, operand must be `comparison`.
                BinOp::Lt | BinOp::Gt | BinOp::Le | BinOp::Ge => {
                    let lt = self.infer_expr(lhs, env)?;
                    let rt = self.infer_expr(rhs, env)?;
                    self.unify(&lt, &rt, span)?;
                    self.require_ord(&lt, span)?;
                    Ok(Ty::Bool)
                }
                // Logical: both operands `bool`, result `bool`.
                BinOp::And | BinOp::Or => {
                    let lt = self.infer_expr(lhs, env)?;
                    self.unify(&Ty::Bool, &lt, lhs.span())?;
                    let rt = self.infer_expr(rhs, env)?;
                    self.unify(&Ty::Bool, &rt, rhs.span())?;
                    Ok(Ty::Bool)
                }
            },

            ExprKind::Unary { op, expr } => match op {
                UnOp::Not => {
                    let t = self.infer_expr(expr, env)?;
                    self.unify(&Ty::Bool, &t, expr.span())?;
                    Ok(Ty::Bool)
                }
                // Arithmetic negation: numeric, preserving the operand's unit
                // (`-5<m> : int<m>`).
                UnOp::Neg => {
                    let t = self.infer_expr(expr, env)?;
                    let (base, unit) = self.expect_num(&t, expr.span())?;
                    Ok(self.num_ty(base, self.apply_unit(&unit)))
                }
            },

            // A chained comparison `a < b < c`: each adjacent pair is typed like a
            // single comparison — operands unify, ordering links (`< > <= >=`) carry
            // the `comparison` constraint, equality links (`== !=`) don't — and the
            // whole chain is `bool`.
            ExprKind::Compare { first, rest } => {
                let mut prev = self.infer_expr(first, env)?;
                for (op, operand) in rest {
                    let t = self.infer_expr(operand, env)?;
                    self.unify(&prev, &t, operand.span())?;
                    if matches!(op, BinOp::Lt | BinOp::Gt | BinOp::Le | BinOp::Ge) {
                        self.require_ord(&t, operand.span())?;
                    }
                    prev = t;
                }
                Ok(Ty::Bool)
            }

            // `(op)` is the curried lambda `fun a b -> a op b`: desugar and infer,
            // so the operator's constraints (num/comparison) fall out for free.
            ExprKind::OpFunc(op) => {
                let lam = crate::desugar::op_func(*op, span);
                self.infer_expr(&lam, env)
            }

            // `f >> g` / `f << g` — desugar to a composition lambda, then infer.
            ExprKind::Compose {
                lhs,
                rhs,
                right_to_left,
            } => {
                let lam =
                    crate::desugar::compose((**lhs).clone(), (**rhs).clone(), *right_to_left, span);
                self.infer_expr(&lam, env)
            }

            ExprKind::If { cond, then, else_ } => {
                let ct = self.infer_expr(cond, env)?;
                self.unify(&Ty::Bool, &ct, cond.span())?;
                let tt = self.infer_expr(then, env)?;
                let et = self.infer_expr(else_, env)?;
                self.unify(&tt, &et, else_.span())?;
                Ok(self.apply(&tt))
            }

            // `try body : Result <body> Exception`. The body's own effect still
            // propagates (the call is performed — `try` catches a *thrown*
            // exception, it does not suppress the `io`), so nothing is discharged.
            ExprKind::Try { body } => {
                let inner = self.infer_expr(body, env)?;
                Ok(Ty::Con(
                    "Result".to_string(),
                    vec![self.apply(&inner), Ty::Con("Exception".to_string(), vec![])],
                ))
            }

            ExprKind::Fn { params, body } => {
                let mut body_env = env.clone();
                let mut param_tys = Vec::with_capacity(params.len());
                for param in params {
                    let pty = self.fresh();
                    param_tys.push(pty.clone());
                    if self.record_types {
                        self.recorded.push((param.span.span(), pty.clone()));
                    }
                    body_env.insert(param.name.clone(), Scheme::mono(pty));
                }
                // Defining a lambda is pure: capture the body's effect as the
                // innermost arrow's latent effect rather than performing it here.
                let outer = std::mem::replace(&mut self.cur_eff, Effect::pure());
                let body_ty = self.infer_expr(body, &body_env)?;
                let mut ty = body_ty;
                let mut eff = self.cur_eff.clone();
                for p in param_tys.into_iter().rev() {
                    ty = Ty::Fun(Box::new(p), Box::new(ty), eff);
                    eff = Effect::pure();
                }
                self.cur_eff = outer;
                Ok(ty)
            }

            ExprKind::App { func, arg } => self.infer_apply(func, arg, span, env),
            // `lhs |> rhs` applies `rhs` to `lhs`; `lhs <| rhs` applies `lhs` to `rhs`.
            ExprKind::Pipe { lhs, rhs, backward } => {
                if *backward {
                    self.infer_apply(lhs, rhs, span, env)
                } else {
                    self.infer_apply(rhs, lhs, span, env)
                }
            }

            ExprKind::Match { scrutinee, arms } => {
                let scrut_ty = self.infer_expr(scrutinee, env)?;
                let result = self.fresh();
                for arm in arms {
                    let mut arm_env = env.clone();
                    self.bind_pattern(&arm.pattern, &scrut_ty, scrutinee.span(), &mut arm_env)?;
                    // A guard is checked in the arm's pattern-bound scope and must be
                    // `bool` (`DESIGN.md` §7.2).
                    if let Some(guard) = &arm.guard {
                        let g_ty = self.infer_expr(guard, &arm_env)?;
                        self.unify(&Ty::Bool, &g_ty, guard.span())?;
                    }
                    let body_ty = self.infer_expr(&arm.body, &arm_env)?;
                    self.unify(&result, &body_ty, arm.body.span())?;
                }
                self.check_exhaustive(&scrut_ty, arms, span)?;
                Ok(self.apply(&result))
            }

            ExprKind::Ce { builder, items } => self.infer_ce(builder, items, span, env),

            ExprKind::List { elems } => {
                // All elements share one type; an empty list is polymorphic.
                let elem = self.fresh();
                for e in elems {
                    let t = self.infer_expr(e, env)?;
                    self.unify(&elem, &t, e.span())?;
                }
                Ok(Ty::Con("List".to_string(), vec![self.apply(&elem)]))
            }

            ExprKind::Tuple { elems } => {
                let tys: Result<Vec<Ty>, TypeError> =
                    elems.iter().map(|e| self.infer_expr(e, env)).collect();
                Ok(Ty::Tuple(tys?))
            }

            ExprKind::Record {
                ty,
                ty_span,
                fields,
            } => self.infer_record(ty, ty_span.span(), fields, span, env),
            ExprKind::RecordUpdate { base, fields } => {
                self.infer_record_update(base, fields, span, env)
            }
            ExprKind::Field { base, name } => {
                // `Module.member` (built-in or user module) resolves against the
                // qualified env; otherwise it is ordinary record-field access.
                if let Some(q) = qualified_name(expr) {
                    return match env.get(&q) {
                        Some(scheme) => Ok(self.instantiate(scheme)),
                        None => {
                            let module = q.split('.').next().unwrap_or("");
                            let hint = match closest_member(module, name, env) {
                                Some(m) => format!(" (did you mean `{module}.{m}`?)"),
                                None => String::new(),
                            };
                            Err(TypeError {
                                message: format!("`{name}` is not a member of `{module}`{hint}"),
                                span,
                            })
                        }
                    };
                }
                self.infer_field(base, name, span, env)
            }

            ExprKind::Block { stmts } => self.infer_block(stmts, env),
            ExprKind::Assign { target, value } => self.infer_assign(target, value, span, env),
        }
    }

    /// A block introduces a new scope: local bindings (immutable ones generalized,
    /// `mut` ones monomorphic) are visible to later statements; every statement but
    /// the last must be `unit` (so a value is never silently dropped); the last
    /// statement's type is the block's type.
    fn infer_block(&mut self, stmts: &[BlockStmt], env: &Env) -> Result<Ty, TypeError> {
        let mut scope = env.clone();
        let mut result = Ty::Unit;
        let last = stmts.len().saturating_sub(1);
        for (i, stmt) in stmts.iter().enumerate() {
            match stmt {
                BlockStmt::Let(binding) => {
                    // The binding's effect is accumulated into `cur_eff` by
                    // `infer_binding` (for value bindings), so the block's effect
                    // includes it.
                    let (scheme, _eff) = self.infer_binding(binding, &scope)?;
                    scope.insert(binding.name.clone(), scheme);
                }
                BlockStmt::Expr(e) => {
                    let t = self.infer_expr(e, &scope)?;
                    if i == last {
                        result = self.apply(&t);
                    } else {
                        let applied = self.apply(&t);
                        if self.unify(&Ty::Unit, &applied, e.span()).is_err() {
                            return Err(TypeError {
                                message: format!(
                                    "this statement has type {} but must be `unit`; bind it with `let` or make it the block's final expression",
                                    show(&applied)
                                ),
                                span: e.span(),
                            });
                        }
                    }
                }
            }
        }
        Ok(result)
    }

    /// `target <- value` — reassignment, valid only for a `let mut` binding.
    fn infer_assign(
        &mut self,
        target: &str,
        value: &Expr,
        span: Span,
        env: &Env,
    ) -> Result<Ty, TypeError> {
        let Some(scheme) = env.get(target).cloned() else {
            return Err(TypeError {
                message: format!("unbound name `{target}`"),
                span,
            });
        };
        if !scheme.mutable {
            return Err(TypeError {
                message: format!(
                    "cannot assign to `{target}`: it is immutable (declare it with `let mut`)"
                ),
                span,
            });
        }
        let target_ty = self.instantiate(&scheme);
        let vt = self.infer_expr(value, env)?;
        self.unify(&target_ty, &vt, value.span())?;
        // Reassignment is an `io` effect (mutation of observable state).
        self.perform(&Effect::io());
        Ok(Ty::Unit)
    }

    /// The bare identity name of the record type owning `field` (`DESIGN.md` §8.3).
    /// Resolution is by field name, but no longer global: **0** owners is an unknown
    /// field, **1** owner resolves, **2+** is an ambiguity error *at this access site*
    /// (the field is declared by two visible records — pattern-match, or tag the
    /// construction/update, to disambiguate). Ambiguity is never an error at
    /// declaration or import; module isolation is preserved.
    fn record_of_field(&self, field: &str, span: Span) -> Result<String, TypeError> {
        match self.decls.field_owner.get(field).map(Vec::as_slice) {
            Some([only]) => Ok(only.clone()),
            Some(owners) if owners.len() >= 2 => {
                let names = owners
                    .iter()
                    .map(|r| format!("`{r}`"))
                    .collect::<Vec<_>>()
                    .join(" and ");
                Err(TypeError {
                    message: format!(
                        "field `{field}` is ambiguous here: it is declared by records {names}; \
                         pattern-match the value (`case {} {{ {field} }}:`) to disambiguate",
                        owners[0]
                    ),
                    span,
                })
            }
            _ => {
                // Empty `decls.records` means records aren't in use at all.
                let hint = if self.decls.records.is_empty() {
                    " (no record types are declared)"
                } else {
                    ""
                };
                Err(TypeError {
                    message: format!("unknown record field `{field}`{hint}"),
                    span,
                })
            }
        }
    }

    /// Resolve a surface record tag (`Point` or `Geometry.Point`) at a construction
    /// or pattern site to its **bare identity name** (`DESIGN.md` §8.3). A qualified
    /// tag resolves via the imported-record alias table; a bare tag resolves only to
    /// a **local** record (an imported record must be tagged qualified, exactly as an
    /// imported sum-type constructor must be). Anything else is "not a record type"
    /// (or "not a member of `M`" for an unknown qualified tag).
    fn resolve_record_tag(&self, tag: &str, span: Span) -> Result<String, TypeError> {
        if let Some((module, rec)) = tag.split_once('.') {
            if let Some(bare) = self.decls.record_aliases.get(tag) {
                return Ok(bare.clone());
            }
            return Err(TypeError {
                message: format!("`{rec}` is not a member of `{module}`"),
                span,
            });
        }
        if self.decls.local_records.contains(tag) {
            Ok(tag.to_string())
        } else {
            Err(TypeError {
                message: format!("`{tag}` is not a record type"),
                span,
            })
        }
    }

    /// The bare identity name for a record tag as stored on a pattern/literal AST
    /// node — mapping a qualified `Geometry.Point` to `Point` (and a bare name to
    /// itself), so exhaustiveness `Tag::Record`s and `decls.records` keys agree.
    fn canonical_record(&self, tag: &str) -> String {
        self.decls
            .record_aliases
            .get(tag)
            .cloned()
            .unwrap_or_else(|| tag.to_string())
    }

    /// Instantiate a record type's parameters with fresh variables, returning the
    /// record type itself and its field types (under the same instantiation).
    fn instantiate_record(&mut self, name: &str) -> (Ty, Vec<(String, Ty)>) {
        let info = self
            .decls
            .records
            .get(name)
            .cloned()
            .expect("record type registered");
        let fresh: Vec<Ty> = (0..info.params_count).map(|_| self.fresh()).collect();
        let tmap: HashMap<u32, Ty> = (0..info.params_count as u32)
            .zip(fresh.iter().cloned())
            .collect();
        let empty_u = HashMap::new();
        let empty_n = HashMap::new();
        let empty_e = HashMap::new();
        let record_ty = Ty::Con(name.to_string(), fresh);
        let fields = info
            .fields
            .iter()
            .map(|(f, t)| (f.clone(), subst_all(t, &tmap, &empty_u, &empty_n, &empty_e)))
            .collect();
        (record_ty, fields)
    }

    /// `Point { x = e, … }` — a constructor-tagged record literal (`DESIGN.md`
    /// §8.3). The type is the tag; the literal must mention exactly the record's
    /// fields, once each.
    fn infer_record(
        &mut self,
        ty: &str,
        ty_span: Span,
        fields: &[crate::parser::ast::FieldInit],
        span: Span,
        env: &Env,
    ) -> Result<Ty, TypeError> {
        let owner = self.resolve_record_tag(ty, ty_span)?;
        let (record_ty, field_tys) = self.instantiate_record(&owner);

        let mut seen: HashSet<String> = HashSet::new();
        for init in fields {
            if !seen.insert(init.name.clone()) {
                return Err(TypeError {
                    message: format!("field `{}` is set twice", init.name),
                    span: init.value.span(),
                });
            }
            if !field_tys.iter().any(|(n, _)| n == &init.name) {
                return Err(TypeError {
                    message: format!("record `{owner}` has no field `{}`", init.name),
                    span: init.value.span(),
                });
            }
        }
        let missing: Vec<&str> = field_tys
            .iter()
            .map(|(n, _)| n.as_str())
            .filter(|n| !seen.contains(*n))
            .collect();
        if !missing.is_empty() {
            let names = missing
                .iter()
                .map(|m| format!("`{m}`"))
                .collect::<Vec<_>>()
                .join(", ");
            return Err(TypeError {
                message: format!("record `{owner}` is missing field(s) {names}"),
                span,
            });
        }
        for (fname, fty) in &field_tys {
            let init = fields.iter().find(|i| &i.name == fname).unwrap();
            let vt = self.infer_expr(&init.value, env)?;
            self.unify(fty, &vt, init.value.span())?;
        }
        Ok(self.apply(&record_ty))
    }

    /// `{ base with x = e, … }` — copy `base`, replacing the listed fields.
    fn infer_record_update(
        &mut self,
        base: &Expr,
        fields: &[crate::parser::ast::FieldInit],
        span: Span,
        env: &Env,
    ) -> Result<Ty, TypeError> {
        let owner = self.record_of_field(&fields[0].name, span)?;
        let (record_ty, field_tys) = self.instantiate_record(&owner);
        let bt = self.infer_expr(base, env)?;
        self.unify(&record_ty, &bt, base.span())?;

        let mut seen: HashSet<String> = HashSet::new();
        for init in fields {
            if !seen.insert(init.name.clone()) {
                return Err(TypeError {
                    message: format!("field `{}` is set twice", init.name),
                    span: init.value.span(),
                });
            }
            let Some((_, fty)) = field_tys.iter().find(|(n, _)| n == &init.name) else {
                return Err(TypeError {
                    message: format!("record `{owner}` has no field `{}`", init.name),
                    span: init.value.span(),
                });
            };
            let fty = fty.clone();
            let vt = self.infer_expr(&init.value, env)?;
            self.unify(&fty, &vt, init.value.span())?;
        }
        Ok(self.apply(&record_ty))
    }

    /// `base.name` — record field access.
    fn infer_field(
        &mut self,
        base: &Expr,
        name: &str,
        span: Span,
        env: &Env,
    ) -> Result<Ty, TypeError> {
        let owner = self.record_of_field(name, span)?;
        let (record_ty, field_tys) = self.instantiate_record(&owner);
        let bt = self.infer_expr(base, env)?;
        self.unify(&record_ty, &bt, base.span())?;
        let fty = field_tys
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, t)| t.clone())
            .unwrap();
        Ok(self.apply(&fty))
    }

    fn resolve_unit_expr(&self, unit: &UnitExpr, span: Span) -> Result<Unit, TypeError> {
        resolve_unit_against(unit, &self.decls, span)
    }

    fn infer_ce(
        &mut self,
        builder: &CeBuilder,
        items: &[CeItem],
        span: Span,
        env: &Env,
    ) -> Result<Ty, TypeError> {
        match builder {
            CeBuilder::Seq => self.infer_seq(items, span, env),
            CeBuilder::Result => self.infer_monad(items, span, env, "Result", true),
            CeBuilder::Async => self.infer_monad(items, span, env, "Async", false),
            // A user builder desugars to plain calls on its module's protocol
            // functions; ordinary inference takes it from there.
            CeBuilder::User(name) => {
                let expr = crate::desugar::desugar_ce(name, items, span)
                    .map_err(|(message, span)| TypeError { message, span })?;
                self.infer_expr(&expr, env)
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
                CeItem::Let {
                    name,
                    name_span,
                    value,
                } => {
                    let t = self.infer_expr(value, &env)?;
                    let applied = self.apply(&t);
                    if self.record_types {
                        self.recorded.push((name_span.span(), applied.clone()));
                    }
                    env.insert(name.clone(), Scheme::mono(applied));
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

    /// Shared inference for the `result` and `async` monads.
    fn infer_monad(
        &mut self,
        items: &[CeItem],
        span: Span,
        env: &Env,
        con: &str,
        binary: bool,
    ) -> Result<Ty, TypeError> {
        let err = self.fresh();
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
                CeItem::LetBang {
                    name,
                    name_span,
                    value,
                } => {
                    let t = self.infer_expr(value, &env)?;
                    let inner = self.fresh();
                    let expected = monad(inner.clone(), self);
                    self.unify(&expected, &t, value.span())?;
                    let bound = self.apply(&inner);
                    if self.record_types {
                        self.recorded.push((name_span.span(), bound.clone()));
                    }
                    env.insert(name.clone(), Scheme::mono(bound));
                }
                CeItem::Let {
                    name,
                    name_span,
                    value,
                } => {
                    let t = self.infer_expr(value, &env)?;
                    let applied = self.apply(&t);
                    if self.record_types {
                        self.recorded.push((name_span.span(), applied.clone()));
                    }
                    env.insert(name.clone(), Scheme::mono(applied));
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
        // The effects of evaluating `func` and `arg` are accumulated into
        // `cur_eff` by their own inference; applying the arrow then performs its
        // latent effect.
        let func_ty = self.infer_expr(func, env)?;
        let arg_ty = self.infer_expr(arg, env)?;
        let result = self.fresh();
        let latent = self.fresh_eff();
        let expected = Ty::Fun(Box::new(arg_ty), Box::new(result.clone()), latent.clone());
        self.unify(&func_ty, &expected, span)?;
        let latent = self.apply_eff(&latent);
        self.perform(&latent);
        Ok(self.apply(&result))
    }

    fn bind_pattern(
        &mut self,
        pattern: &Pattern,
        scrut_ty: &Ty,
        span: Span,
        env: &mut Env,
    ) -> Result<(), TypeError> {
        match pattern {
            Pattern::Wildcard => Ok(()),
            Pattern::Var {
                name,
                span: var_span,
            } => {
                if self.record_types {
                    self.recorded.push((var_span.span(), scrut_ty.clone()));
                }
                env.insert(name.clone(), Scheme::mono(scrut_ty.clone()));
                Ok(())
            }
            // `p as x`: bind `x` to the whole matched value (like a `Var`), then
            // bind the inner pattern against the same scrutinee type.
            Pattern::As {
                pattern,
                name,
                name_span,
            } => {
                if self.record_types {
                    self.recorded.push((name_span.span(), scrut_ty.clone()));
                }
                env.insert(name.clone(), Scheme::mono(scrut_ty.clone()));
                self.bind_pattern(pattern, scrut_ty, span, env)
            }
            Pattern::Int(_) => self.unify(scrut_ty, &Ty::Int(Unit::dimensionless()), span),
            Pattern::Str(_) => self.unify(scrut_ty, &Ty::Str, span),
            Pattern::Bool(_) => self.unify(scrut_ty, &Ty::Bool, span),
            Pattern::Ctor { name, args, .. } => {
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
            Pattern::Record {
                ty,
                ty_span,
                fields,
            } => {
                let owner = self.resolve_record_tag(ty, ty_span.span())?;
                let (record_ty, field_tys) = self.instantiate_record(&owner);
                self.unify(&record_ty, scrut_ty, span)?;
                let mut seen: HashSet<String> = HashSet::new();
                for fp in fields {
                    if !seen.insert(fp.name.clone()) {
                        return Err(TypeError {
                            message: format!("field `{}` is matched twice", fp.name),
                            span,
                        });
                    }
                    let Some((_, fty)) = field_tys.iter().find(|(n, _)| n == &fp.name) else {
                        return Err(TypeError {
                            message: format!("record `{owner}` has no field `{}`", fp.name),
                            span,
                        });
                    };
                    let fty = fty.clone();
                    if self.record_types {
                        self.recorded.push((fp.name_span.span(), fty.clone()));
                    }
                    self.bind_pattern(&fp.pattern, &fty, span, env)?;
                }
                Ok(())
            }
            Pattern::Tuple { elems } => {
                let elem_tys: Vec<Ty> = elems.iter().map(|_| self.fresh()).collect();
                self.unify(&Ty::Tuple(elem_tys.clone()), scrut_ty, span)?;
                for (sub, ety) in elems.iter().zip(&elem_tys) {
                    self.bind_pattern(sub, ety, span, env)?;
                }
                Ok(())
            }
            // `[a, b, *mid, z]` — a sequence pattern over `List a`: each prefix and
            // suffix element binds against `a`, and the star's binder against the
            // unmatched middle slice, itself a `List a` (`DESIGN.md` §7.2).
            Pattern::List {
                prefix,
                rest,
                suffix,
            } => {
                let elem = self.fresh();
                let list_ty = Ty::Con("List".to_string(), vec![elem.clone()]);
                self.unify(&list_ty, scrut_ty, span)?;
                for sub in prefix {
                    self.bind_pattern(sub, &elem, span, env)?;
                }
                if let Some(r) = rest {
                    self.bind_pattern(r, &list_ty, span, env)?;
                }
                for sub in suffix {
                    self.bind_pattern(sub, &elem, span, env)?;
                }
                Ok(())
            }
            // Every alternative matches the same scrutinee type and must bind the
            // same variables at the same types (`DESIGN.md` §7.2). Bind the first
            // into a temp scope, then unify each other alternative's bindings
            // against it before committing the (agreed) bindings to `env`.
            Pattern::Or(alts) => {
                let base = env.clone();
                let mut first_env = base.clone();
                self.bind_pattern(&alts[0], scrut_ty, span, &mut first_env)?;
                let new_names: Vec<String> = first_env
                    .keys()
                    .filter(|k| !base.contains_key(*k))
                    .cloned()
                    .collect();
                for alt in &alts[1..] {
                    let mut alt_env = base.clone();
                    self.bind_pattern(alt, scrut_ty, span, &mut alt_env)?;
                    let bound_here = |n: &str| alt_env.contains_key(n) && !base.contains_key(n);
                    for n in &new_names {
                        if !bound_here(n) {
                            return Err(TypeError {
                                message: format!(
                                    "variable `{n}` is bound in only some alternatives of the or-pattern"
                                ),
                                span,
                            });
                        }
                        let t1 = first_env.get(n).unwrap().ty.clone();
                        let t2 = alt_env.get(n).unwrap().ty.clone();
                        self.unify(&t1, &t2, span)?;
                    }
                    for n in alt_env.keys() {
                        if !base.contains_key(n) && !new_names.contains(n) {
                            return Err(TypeError {
                                message: format!(
                                    "variable `{n}` is bound in only some alternatives of the or-pattern"
                                ),
                                span,
                            });
                        }
                    }
                }
                for n in &new_names {
                    env.insert(n.clone(), first_env.get(n).unwrap().clone());
                }
                Ok(())
            }
        }
    }

    /// Deep exhaustiveness via Maranget's usefulness algorithm ("Warnings for
    /// pattern matching", JFP 2007). A `match` is exhaustive iff a wildcard row is
    /// *not* useful against the matrix of arm patterns — i.e. there is no value the
    /// wildcard would catch that no arm already does. The check recurses into
    /// nested sub-patterns, so e.g. `{ item = Some n } | { item = None }` is
    /// recognized as complete. When it isn't, the algorithm yields a concrete
    /// witness value (`Some _`, `None`, `{ x = 0, y = _ }`, …) for the diagnostic.
    fn check_exhaustive(
        &mut self,
        scrut_ty: &Ty,
        arms: &[MatchArm],
        span: Span,
    ) -> Result<(), TypeError> {
        // A guarded arm may fail at runtime, so it never contributes to coverage
        // (`DESIGN.md` §7.2): only unguarded arms form the exhaustiveness matrix.
        let matrix: Vec<Vec<Pattern>> = arms
            .iter()
            .filter(|a| a.guard.is_none())
            .map(|a| vec![a.pattern.clone()])
            .collect();
        // A suffix-star list pattern (`[*r, s…]`) reproduces itself under `Cons`
        // specialization (see `row_heads`), so `useful` needs its cycle guard; every
        // other pattern shrinks strictly, so the guard stays off (zero cost).
        self.seq_guard = matrix.iter().flatten().any(has_suffix_star);
        self.seq_visiting.clear();
        let types = vec![self.apply(scrut_ty)];
        let Some(witness) = self.useful(&matrix, &types) else {
            return Ok(());
        };
        let pat = self.render_witness(&witness[0], false);
        let message = if pat == "_" {
            "non-exhaustive match: add a wildcard `_` arm".to_string()
        } else {
            format!("non-exhaustive match: `{pat}` is not matched")
        };
        Err(TypeError { message, span })
    }

    /// Is a wildcard row useful against `matrix` over the column `types`? `Some`
    /// returns a witness row (one [`Wit`] per column) exposing an uncovered value;
    /// `None` means every value is already matched. Each matrix row has the same
    /// width as `types`.
    fn useful(&mut self, matrix: &[Vec<Pattern>], types: &[Ty]) -> Option<Vec<Wit>> {
        if types.is_empty() {
            // Width 0: a wildcard row is useful only when no arm row remains.
            return matrix.is_empty().then(Vec::new);
        }
        // Flatten any or-pattern in the first column into separate rows, so the
        // constructor machinery below never sees a `Pattern::Or` head (`DESIGN.md`
        // §7.2). Deeper or-patterns surface as first-column heads in the recursive
        // calls and are expanded there.
        let matrix = expand_first_column(matrix);
        // Cycle guard, active only when a suffix-star list pattern is in play: that
        // pattern reproduces itself under `Cons` specialization (`row_heads`), so
        // the recursion can re-reach an already-open sub-problem. Re-reaching one
        // means "useful here iff useful here with a strictly smaller witness" —
        // every cycle passes through a `Cons` specialization, which peels one list
        // element off any witness — an infinite descent, so no finite witness
        // exists via that path: answer "not useful" (the least fixed point). The
        // key order-normalizes rows (sorted + deduped — the matrix is a union of
        // rows) so a cycle is caught regardless of row order.
        let guard_key = self.seq_guard.then(|| {
            let mut rows: Vec<String> = matrix.iter().map(|r| format!("{r:?}")).collect();
            rows.sort();
            rows.dedup();
            let tys: Vec<Ty> = types.iter().map(|t| self.apply(t)).collect();
            format!("{rows:?} :: {tys:?}")
        });
        if let Some(key) = &guard_key
            && !self.seq_visiting.insert(key.clone())
        {
            return None;
        }
        let result = self.useful_at(&matrix, types);
        if let Some(key) = &guard_key {
            self.seq_visiting.remove(key);
        }
        result
    }

    /// The body of [`useful`](Infer::useful), after or-expansion and the cycle
    /// guard: the constructor-signature case analysis over the first column.
    fn useful_at(&mut self, matrix: &[Vec<Pattern>], types: &[Ty]) -> Option<Vec<Wit>> {
        let col_ty = self.apply(&types[0]);
        let rest = &types[1..];
        let present = self.present_tags(matrix);
        let full_sig = self.ctor_signature(&col_ty);
        let complete = full_sig
            .as_ref()
            .is_some_and(|sig| sig.iter().all(|t| present.contains(t)));

        if complete {
            // The column's constructors are all present: a wildcard is useful only
            // if some constructor specializes to a useful sub-problem.
            for tag in full_sig.unwrap() {
                let arg_tys = self.tag_field_types(&col_ty, &tag);
                let arity = arg_tys.len();
                let spec = self.specialize(matrix, &tag, arity);
                let mut sub_types = arg_tys;
                sub_types.extend_from_slice(rest);
                if let Some(mut w) = self.useful(&spec, &sub_types) {
                    let tail = w.split_off(arity);
                    let mut row = vec![Wit::Con(tag, w)];
                    row.extend(tail);
                    return Some(row);
                }
            }
            None
        } else {
            // The signature is incomplete (or the type is infinite): a wildcard can
            // pick an absent constructor, so only the default rows matter.
            let def = default_matrix(matrix);
            let w = self.useful(&def, rest)?;
            let head = match &full_sig {
                Some(sig) => match sig.iter().find(|t| !present.contains(t)) {
                    Some(tag) => Wit::Con(tag.clone(), vec![Wit::Wild; self.tag_arity(tag)]),
                    None => Wit::Wild,
                },
                None => Wit::Wild,
            };
            let mut row = vec![head];
            row.extend(w);
            Some(row)
        }
    }

    /// The distinct head constructors appearing in a matrix's first column.
    fn present_tags(&self, matrix: &[Vec<Pattern>]) -> Vec<Tag> {
        let mut out: Vec<Tag> = Vec::new();
        for row in matrix {
            if let Some(tag) = row.first().and_then(|p| self.pattern_tag(p))
                && !out.contains(&tag)
            {
                out.push(tag);
            }
        }
        out
    }

    /// The head constructor of a pattern, or `None` for a wildcard/variable.
    fn pattern_tag(&self, pat: &Pattern) -> Option<Tag> {
        match pat {
            Pattern::Wildcard | Pattern::Var { .. } => None,
            Pattern::Bool(b) => Some(Tag::Bool(*b)),
            Pattern::Int(n) => Some(Tag::Int(*n)),
            Pattern::Str(s) => Some(Tag::Str(s.clone())),
            Pattern::Ctor { name, .. } => Some(Tag::Sum(name.clone())),
            Pattern::Record { ty, .. } => Some(Tag::Record(self.canonical_record(ty))),
            Pattern::Tuple { elems } => Some(Tag::Tuple(elems.len())),
            // A list pattern's head constructor: a non-empty prefix *or suffix*
            // requires at least one element, so it is Cons; `[]` is Nil; a lone star
            // `[*r]` is *equivalent to `r`* (the star binds the whole list), so it
            // delegates — `[*rest]`/`[*_]` → `None` (a catch-all) (`DESIGN.md` §7.2).
            Pattern::List {
                prefix,
                rest,
                suffix,
            } => {
                if !prefix.is_empty() || !suffix.is_empty() {
                    Some(Tag::Cons)
                } else {
                    match rest {
                        None => Some(Tag::Nil),
                        Some(r) => self.pattern_tag(r),
                    }
                }
            }
            // An as-pattern is transparent — its tag is the inner pattern's.
            Pattern::As { pattern, .. } => self.pattern_tag(pattern),
            // Or-patterns are expanded away in `useful` before this is reached.
            Pattern::Or(_) => None,
        }
    }

    /// The complete set of constructors for a column type, or `None` when the type
    /// is infinite (`int`/`string`/…) or has no matchable constructors — in which
    /// case only a wildcard can be exhaustive.
    fn ctor_signature(&self, ty: &Ty) -> Option<Vec<Tag>> {
        match ty {
            Ty::Bool => Some(vec![Tag::Bool(true), Tag::Bool(false)]),
            // A tuple has one implicit constructor, so the column is "complete"
            // once a tuple pattern is present; exhaustiveness recurses into the
            // element columns (like records).
            Ty::Tuple(elems) => Some(vec![Tag::Tuple(elems.len())]),
            // `List a` is modeled as the finite 2-constructor `Nil | Cons a (List a)`,
            // so `[] | [x, *rest]` is exhaustive without a wildcard (`DESIGN.md` §7.2).
            Ty::Con(name, _) if name == "List" => Some(vec![Tag::Nil, Tag::Cons]),
            Ty::Con(name, _) => {
                if let Some(ctors) = self.decls.type_ctors.get(name) {
                    (!ctors.is_empty()).then(|| ctors.iter().map(|c| Tag::Sum(c.clone())).collect())
                } else {
                    self.decls
                        .records
                        .contains_key(name)
                        .then(|| vec![Tag::Record(name.clone())])
                }
            }
            _ => None,
        }
    }

    /// The number of sub-patterns a constructor binds (its arity).
    fn tag_arity(&self, tag: &Tag) -> usize {
        match tag {
            Tag::Bool(_) | Tag::Int(_) | Tag::Str(_) | Tag::Nil => 0,
            Tag::Sum(name) => self.decls.ctors[name].arity,
            Tag::Record(name) => self.decls.records[name].fields.len(),
            Tag::Tuple(n) => *n,
            // `Cons head tail` — head + list tail.
            Tag::Cons => 2,
        }
    }

    /// The types of a constructor's arguments at the given column type (its type
    /// parameters pinned by unifying the constructor's result with the column).
    fn tag_field_types(&mut self, ty: &Ty, tag: &Tag) -> Vec<Ty> {
        match tag {
            Tag::Bool(_) | Tag::Int(_) | Tag::Str(_) | Tag::Nil => Vec::new(),
            // `Cons head tail` at `List a`: head is `a`, tail is `List a`. The element
            // type is the column's type argument (a fresh var if not yet a `List`).
            Tag::Cons => {
                let elem = match self.apply(ty) {
                    Ty::Con(name, args) if name == "List" && args.len() == 1 => args[0].clone(),
                    _ => self.fresh(),
                };
                vec![elem.clone(), Ty::Con("List".to_string(), vec![elem])]
            }
            Tag::Sum(name) => {
                let info = self.decls.ctors[name].clone();
                let cty = self.instantiate(&info.scheme);
                let (fields, result) = split_fun(&cty, info.arity);
                let _ = self.unify(&result, ty, Span::new(0, 0));
                fields.iter().map(|f| self.apply(f)).collect()
            }
            Tag::Record(name) => {
                let (record_ty, fields) = self.instantiate_record(name);
                let _ = self.unify(&record_ty, ty, Span::new(0, 0));
                fields.iter().map(|(_, t)| self.apply(t)).collect()
            }
            Tag::Tuple(_) => match self.apply(ty) {
                Ty::Tuple(elems) => elems,
                _ => Vec::new(),
            },
        }
    }

    /// Specialize a matrix by a constructor `tag` of the given `arity`: rows headed
    /// by `tag` keep their sub-patterns (records expand to all fields positionally,
    /// absent ones as wildcards); wildcard rows expand to `arity` wildcards; rows
    /// headed by another constructor are dropped. The first column is replaced by
    /// the `arity` new columns. A row may expand to **several** rows: a suffix-star
    /// list pattern `[*r, s…]` splits by length under `Cons` (see [`row_heads`]);
    /// splitting a row into rows covering the same value set is sound — the matrix
    /// is a union of rows.
    fn specialize(&self, matrix: &[Vec<Pattern>], tag: &Tag, arity: usize) -> Vec<Vec<Pattern>> {
        let mut out = Vec::new();
        for row in matrix {
            let (head, rest) = row.split_first().expect("non-empty row");
            for mut expanded in self.row_heads(head, tag, arity) {
                expanded.extend_from_slice(rest);
                out.push(expanded);
            }
        }
        out
    }

    /// The rows a row's head contributes when specializing by `tag` (each a vector
    /// of `arity` sub-patterns), or empty if the head is a different constructor
    /// (the row drops out). All heads yield zero or one row except a suffix-star
    /// list pattern under `Cons`, which yields two (a length case split).
    fn row_heads(&self, pat: &Pattern, tag: &Tag, arity: usize) -> Vec<Vec<Pattern>> {
        let single = |row: Option<Vec<Pattern>>| row.into_iter().collect::<Vec<_>>();
        match pat {
            Pattern::Wildcard | Pattern::Var { .. } => vec![vec![Pattern::Wildcard; arity]],
            Pattern::Bool(b) => single((*tag == Tag::Bool(*b)).then(Vec::new)),
            Pattern::Int(n) => single((*tag == Tag::Int(*n)).then(Vec::new)),
            Pattern::Str(s) => single((*tag == Tag::Str(s.clone())).then(Vec::new)),
            Pattern::Ctor { name, args, .. } => {
                single((*tag == Tag::Sum(name.clone())).then(|| args.clone()))
            }
            Pattern::Record { fields, .. } => {
                let Tag::Record(rname) = tag else {
                    return Vec::new();
                };
                let order = &self.decls.records[rname].fields;
                let mut slots = vec![Pattern::Wildcard; order.len()];
                for fp in fields {
                    if let Some(idx) = order.iter().position(|(n, _)| n == &fp.name) {
                        slots[idx] = fp.pattern.clone();
                    }
                }
                vec![slots]
            }
            Pattern::Tuple { elems } => {
                single((*tag == Tag::Tuple(elems.len())).then(|| elems.clone()))
            }
            // A list pattern against the `Nil | Cons` model (`DESIGN.md` §7.2).
            Pattern::List {
                prefix,
                rest,
                suffix,
            } => {
                if let Some((first, remaining)) = prefix.split_first() {
                    // A non-empty prefix is `Cons first (tail-list-pattern)`: peel
                    // the first element, the tail keeps the remaining prefix + the
                    // same star + suffix (`[a,b,*r,z]` → `[a, [b,*r,z]]`). It never
                    // matches Nil.
                    if *tag == Tag::Cons {
                        let tail = Pattern::List {
                            prefix: remaining.to_vec(),
                            rest: rest.clone(),
                            suffix: suffix.clone(),
                        };
                        vec![vec![first.clone(), tail]]
                    } else {
                        Vec::new()
                    }
                } else if rest.is_none() {
                    // `[]` matches only Nil (0 sub-patterns; suffix is empty — a
                    // suffix implies a star).
                    single((*tag == Tag::Nil).then(Vec::new))
                } else if suffix.is_empty() {
                    // A lone star `[*r]` ≡ `r` (binds the whole list): delegate.
                    self.row_heads(rest.as_deref().expect("star present"), tag, arity)
                } else if *tag == Tag::Cons {
                    // `[*r, s1, …, sk]` (star first, k ≥ 1 suffix elements) matches
                    // lists of length ≥ k whose *last* k elements match `s1…sk`.
                    // Under `Cons head tail` that splits by length into two rows
                    // covering exactly the same values:
                    //   • length == k: head is `s1`, tail is exactly `[s2, …, sk]`;
                    //   • length  > k: head is anything (the star absorbs it), tail
                    //     is the same pattern `[*r, s1, …, sk]` again.
                    // The self-reproducing second row is what the cycle guard in
                    // [`useful`](Infer::useful) terminates.
                    let (first, remaining) = suffix.split_first().expect("non-empty suffix");
                    let exact_tail = Pattern::List {
                        prefix: remaining.to_vec(),
                        rest: None,
                        suffix: Vec::new(),
                    };
                    vec![
                        vec![first.clone(), exact_tail],
                        vec![Pattern::Wildcard, pat.clone()],
                    ]
                } else {
                    // A suffix requires at least one element: never Nil.
                    Vec::new()
                }
            }
            // An as-pattern specializes exactly like its inner pattern.
            Pattern::As { pattern, .. } => self.row_heads(pattern, tag, arity),
            // Or-patterns are expanded away in `useful` before specialization.
            Pattern::Or(_) => Vec::new(),
        }
    }

    /// Render a witness value as a Pyfun pattern for a diagnostic. `atom` requests
    /// parenthesization where a constructor application would bind too loosely.
    fn render_witness(&self, w: &Wit, atom: bool) -> String {
        match w {
            Wit::Wild => "_".to_string(),
            Wit::Con(Tag::Bool(b), _) => b.to_string(),
            Wit::Con(Tag::Int(n), _) => n.to_string(),
            Wit::Con(Tag::Str(s), _) => format!("{s:?}"),
            Wit::Con(Tag::Sum(name), args) => {
                if args.is_empty() {
                    name.clone()
                } else {
                    let inner = args
                        .iter()
                        .map(|a| self.render_witness(a, true))
                        .collect::<Vec<_>>()
                        .join(" ");
                    let s = format!("{name} {inner}");
                    if atom { format!("({s})") } else { s }
                }
            }
            Wit::Con(Tag::Record(name), args) => {
                let order = &self.decls.records[name].fields;
                let parts = order
                    .iter()
                    .zip(args)
                    .map(|((f, _), a)| format!("{f} = {}", self.render_witness(a, false)))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{name} {{ {parts} }}")
            }
            Wit::Con(Tag::Tuple(_), args) => {
                let parts = args
                    .iter()
                    .map(|a| self.render_witness(a, false))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("({parts})")
            }
            // The empty list.
            Wit::Con(Tag::Nil, _) => "[]".to_string(),
            // A non-empty list — walk the `Cons` spine into a readable list form:
            // `[a, b]` when the tail bottoms out at Nil, `[a, *_]` when it is an open
            // wildcard (an uncovered non-empty list of unknown length).
            Wit::Con(Tag::Cons, _) => {
                let mut elems = Vec::new();
                let mut cur = w;
                loop {
                    match cur {
                        Wit::Con(Tag::Cons, args) => {
                            elems.push(self.render_witness(&args[0], false));
                            cur = &args[1];
                        }
                        Wit::Con(Tag::Nil, _) => break,
                        _ => {
                            // An open tail (wildcard): the list continues arbitrarily.
                            elems.push("*_".to_string());
                            break;
                        }
                    }
                }
                format!("[{}]", elems.join(", "))
            }
        }
    }

    fn unify(&mut self, a: &Ty, b: &Ty, span: Span) -> Result<(), TypeError> {
        let a = self.apply(a);
        let b = self.apply(b);
        match (&a, &b) {
            (Ty::Bool, Ty::Bool) | (Ty::Str, Ty::Str) | (Ty::Unit, Ty::Unit) => Ok(()),
            (Ty::Int(u1), Ty::Int(u2)) | (Ty::Float(u1), Ty::Float(u2)) => {
                if self.unify_unit(u1, u2) {
                    Ok(())
                } else {
                    Err(mismatch(&a, &b, span))
                }
            }
            // A `num` variable meets another numeric: resolve its base, unify units.
            (Ty::Num(x, ux), Ty::Num(y, uy)) => {
                if self.unify_num(NumRef::Var(*x), NumRef::Var(*y)) && self.unify_unit(ux, uy) {
                    Ok(())
                } else {
                    Err(mismatch(&a, &b, span))
                }
            }
            (Ty::Num(v, un), Ty::Int(ui)) | (Ty::Int(ui), Ty::Num(v, un)) => {
                if self.unify_num(NumRef::Var(*v), NumRef::Int) && self.unify_unit(un, ui) {
                    Ok(())
                } else {
                    Err(mismatch(&a, &b, span))
                }
            }
            (Ty::Num(v, un), Ty::Float(uf)) | (Ty::Float(uf), Ty::Num(v, un)) => {
                if self.unify_num(NumRef::Var(*v), NumRef::Float) && self.unify_unit(un, uf) {
                    Ok(())
                } else {
                    Err(mismatch(&a, &b, span))
                }
            }
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
                // Carry a `comparison` constraint onto whatever `x` resolves to.
                if self.ord.remove(x) {
                    self.require_ord(t, span)?;
                }
                Ok(())
            }
            (Ty::Fun(a1, a2, e1), Ty::Fun(b1, b2, e2)) => {
                self.unify(a1, b1, span)?;
                self.unify(a2, b2, span)?;
                if self.unify_eff(e1, e2) {
                    Ok(())
                } else {
                    Err(TypeError {
                        message: format!(
                            "effect mismatch: cannot unify {} with {}",
                            show(&a),
                            show(&b)
                        ),
                        span,
                    })
                }
            }
            (Ty::Con(n1, a1), Ty::Con(n2, a2)) if n1 == n2 && a1.len() == a2.len() => {
                for (x, y) in a1.iter().zip(a2) {
                    self.unify(x, y, span)?;
                }
                Ok(())
            }
            (Ty::Tuple(a1), Ty::Tuple(a2)) if a1.len() == a2.len() => {
                for (x, y) in a1.iter().zip(a2) {
                    self.unify(x, y, span)?;
                }
                Ok(())
            }
            _ => Err(mismatch(&a, &b, span)),
        }
    }

    /// Unify two units in the free abelian group. Returns false on a genuine
    /// dimension mismatch (a ground term that can't be made the identity).
    fn unify_unit(&mut self, a: &Unit, b: &Unit) -> bool {
        let eq = self.apply_unit(a).div(&self.apply_unit(b));
        self.solve_unit(eq)
    }

    /// Solve `u == 1` for its unit variables (Knuth/Kennedy elimination).
    fn solve_unit(&mut self, u: Unit) -> bool {
        if u.is_dimensionless() {
            return true;
        }
        // Pick the unit variable with the smallest absolute exponent.
        let pivot = u
            .factors
            .iter()
            .filter_map(|(a, e)| {
                if let Atom::Var(v) = a {
                    Some((*v, *e))
                } else {
                    None
                }
            })
            .min_by_key(|(_, e)| e.abs());
        let Some((v, e)) = pivot else {
            // Only base measures remain with nonzero exponents → mismatch.
            return false;
        };

        let divides_all = u
            .factors
            .iter()
            .all(|(a, exp)| matches!(a, Atom::Var(x) if *x == v) || exp % e == 0);
        if divides_all {
            // v = ∏ (other atoms)^(-exp/e)
            let mut s = Unit::dimensionless();
            for (a, exp) in &u.factors {
                if matches!(a, Atom::Var(x) if *x == v) {
                    continue;
                }
                s.insert(a.clone(), -(exp / e));
            }
            self.unit_subst.insert(v, s);
            true
        } else {
            // Introduce a fresh variable and reduce exponents toward zero. If
            // every non-pivot quotient is zero the substitution would be a bare
            // renaming (`v ↦ w`) and the equation would come back unchanged —
            // that happens exactly when `v` is the only variable left and every
            // base exponent `r` satisfies `0 < r < |e|`, so `v^e · ∏ b^r = 1`
            // has no integer solution (`e ∤ r`): a genuine dimension mismatch
            // (e.g. `sqrt` demanding `'u^2 ~ m^3`). Report it rather than
            // recursing forever (this non-termination predated `sqrt`).
            if u.factors.iter().all(|(a, exp)| {
                matches!(a, Atom::Var(x) if *x == v) || exp.div_euclid(e) == 0
            }) {
                return false;
            }
            let w = self.next;
            self.next += 1;
            let mut s = Unit::var(w);
            for (a, exp) in &u.factors {
                if matches!(a, Atom::Var(x) if *x == v) {
                    continue;
                }
                s.insert(a.clone(), -exp.div_euclid(e));
            }
            self.unit_subst.insert(v, s);
            let reduced = self.apply_unit(&u);
            self.solve_unit(reduced)
        }
    }

    fn instantiate(&mut self, scheme: &Scheme) -> Ty {
        let tmap: HashMap<u32, Ty> = scheme.vars.iter().map(|v| (*v, self.fresh())).collect();
        let umap: HashMap<u32, Unit> = scheme
            .uvars
            .iter()
            .map(|v| (*v, self.fresh_unit()))
            .collect();
        let nmap: HashMap<u32, u32> = scheme
            .num_vars
            .iter()
            .map(|v| (*v, self.fresh_id()))
            .collect();
        let emap: HashMap<u32, Effect> = scheme
            .eff_vars
            .iter()
            .map(|v| (*v, self.fresh_eff()))
            .collect();
        // Carry the `comparison` constraint onto each fresh type variable.
        for v in &scheme.ord_vars {
            if let Some(Ty::Var(fresh)) = tmap.get(v) {
                self.ord.insert(*fresh);
            }
        }
        subst_all(&scheme.ty, &tmap, &umap, &nmap, &emap)
    }

    fn generalize(&self, env: &Env, ty: &Ty) -> Scheme {
        let ty = self.apply(ty);
        let (env_t, env_u, env_n, env_e) = self.env_free_vars(env);
        let mut vars = Vec::new();
        free_type_vars(&ty, &mut |v| {
            if !env_t.contains(&v) && !vars.contains(&v) {
                vars.push(v);
            }
        });
        let mut uvars = Vec::new();
        free_unit_vars(&ty, &mut |v| {
            if !env_u.contains(&v) && !uvars.contains(&v) {
                uvars.push(v);
            }
        });
        let mut num_vars = Vec::new();
        free_num_vars(&ty, &mut |v| {
            if !env_n.contains(&v) && !num_vars.contains(&v) {
                num_vars.push(v);
            }
        });
        let mut eff_vars = Vec::new();
        free_eff_vars(&ty, &mut |v| {
            if !env_e.contains(&v) && !eff_vars.contains(&v) {
                eff_vars.push(v);
            }
        });
        // A generalized type variable that carries `comparison` records it.
        let ord_vars: Vec<u32> = vars
            .iter()
            .copied()
            .filter(|v| self.ord.contains(v))
            .collect();
        Scheme {
            vars,
            uvars,
            num_vars,
            ord_vars,
            eff_vars,
            mutable: false,
            ty,
        }
    }

    fn env_free_vars(&self, env: &Env) -> (HashSet<u32>, HashSet<u32>, HashSet<u32>, HashSet<u32>) {
        let mut tys = HashSet::new();
        let mut units = HashSet::new();
        let mut nums = HashSet::new();
        let mut effs = HashSet::new();
        for scheme in env.values() {
            let bound_t: HashSet<u32> = scheme.vars.iter().copied().collect();
            let bound_u: HashSet<u32> = scheme.uvars.iter().copied().collect();
            let bound_n: HashSet<u32> = scheme.num_vars.iter().copied().collect();
            let bound_e: HashSet<u32> = scheme.eff_vars.iter().copied().collect();
            let applied = self.apply(&scheme.ty);
            free_type_vars(&applied, &mut |v| {
                if !bound_t.contains(&v) {
                    tys.insert(v);
                }
            });
            free_unit_vars(&applied, &mut |v| {
                if !bound_u.contains(&v) {
                    units.insert(v);
                }
            });
            free_num_vars(&applied, &mut |v| {
                if !bound_n.contains(&v) {
                    nums.insert(v);
                }
            });
            free_eff_vars(&applied, &mut |v| {
                if !bound_e.contains(&v) {
                    effs.insert(v);
                }
            });
        }
        (tys, units, nums, effs)
    }
}

fn mismatch(a: &Ty, b: &Ty, span: Span) -> TypeError {
    TypeError {
        message: format!("type mismatch: expected {}, found {}", show(a), show(b)),
        span,
    }
}

/// Split a constructor type into its `n` field types and its result type.
fn split_fun(ty: &Ty, n: usize) -> (Vec<Ty>, Ty) {
    let mut fields = Vec::with_capacity(n);
    let mut cur = ty.clone();
    for _ in 0..n {
        match cur {
            Ty::Fun(a, b, _) => {
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
        Ty::Fun(a, b, _) => occurs(var, a) || occurs(var, b),
        Ty::Con(_, args) | Ty::Tuple(args) => args.iter().any(|a| occurs(var, a)),
        _ => false,
    }
}

fn free_type_vars(ty: &Ty, f: &mut impl FnMut(u32)) {
    match ty {
        Ty::Var(n) => f(*n),
        Ty::Fun(a, b, _) => {
            free_type_vars(a, f);
            free_type_vars(b, f);
        }
        Ty::Con(_, args) | Ty::Tuple(args) => args.iter().for_each(|a| free_type_vars(a, f)),
        _ => {}
    }
}

fn free_unit_vars(ty: &Ty, f: &mut impl FnMut(u32)) {
    match ty {
        Ty::Int(u) | Ty::Float(u) | Ty::Num(_, u) => u.var_ids().into_iter().for_each(f),
        Ty::Fun(a, b, _) => {
            free_unit_vars(a, f);
            free_unit_vars(b, f);
        }
        Ty::Con(_, args) | Ty::Tuple(args) => args.iter().for_each(|a| free_unit_vars(a, f)),
        _ => {}
    }
}

fn free_num_vars(ty: &Ty, f: &mut impl FnMut(u32)) {
    match ty {
        Ty::Num(v, _) => f(*v),
        Ty::Fun(a, b, _) => {
            free_num_vars(a, f);
            free_num_vars(b, f);
        }
        Ty::Con(_, args) | Ty::Tuple(args) => args.iter().for_each(|a| free_num_vars(a, f)),
        _ => {}
    }
}

/// Collect the effect variables occurring in a type's arrows.
fn free_eff_vars(ty: &Ty, f: &mut impl FnMut(u32)) {
    if let Ty::Fun(a, b, eff) = ty {
        eff.vars.iter().for_each(|v| f(*v));
        free_eff_vars(a, f);
        free_eff_vars(b, f);
    } else if let Ty::Con(_, args) | Ty::Tuple(args) = ty {
        args.iter().for_each(|a| free_eff_vars(a, f));
    }
}

fn subst_all(
    ty: &Ty,
    tmap: &HashMap<u32, Ty>,
    umap: &HashMap<u32, Unit>,
    nmap: &HashMap<u32, u32>,
    emap: &HashMap<u32, Effect>,
) -> Ty {
    match ty {
        Ty::Var(n) => tmap.get(n).cloned().unwrap_or(Ty::Var(*n)),
        Ty::Int(u) => Ty::Int(u.subst(umap)),
        Ty::Float(u) => Ty::Float(u.subst(umap)),
        Ty::Num(v, u) => Ty::Num(*nmap.get(v).unwrap_or(v), u.subst(umap)),
        Ty::Fun(a, b, eff) => Ty::Fun(
            Box::new(subst_all(a, tmap, umap, nmap, emap)),
            Box::new(subst_all(b, tmap, umap, nmap, emap)),
            subst_eff(eff, emap),
        ),
        Ty::Con(name, args) => Ty::Con(
            name.clone(),
            args.iter()
                .map(|a| subst_all(a, tmap, umap, nmap, emap))
                .collect(),
        ),
        Ty::Tuple(elems) => Ty::Tuple(
            elems
                .iter()
                .map(|a| subst_all(a, tmap, umap, nmap, emap))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// Names type and unit variables as `'a`, `'b`, … for diagnostics.
#[derive(Default)]
struct Namer {
    tys: HashMap<u32, String>,
    units: HashMap<u32, String>,
}

impl Namer {
    fn ty(&mut self, id: u32) -> String {
        let n = self.tys.len();
        self.tys.entry(id).or_insert_with(|| var_name(n)).clone()
    }

    fn unit(&mut self, id: u32) -> String {
        let n = self.units.len();
        self.units.entry(id).or_insert_with(|| var_name(n)).clone()
    }
}

/// Render a type for diagnostics.
fn show(ty: &Ty) -> String {
    let mut namer = Namer::default();
    let mut buf = String::new();
    show_into(ty, &mut namer, &mut buf, false);
    buf
}

fn show_into(ty: &Ty, namer: &mut Namer, out: &mut String, atom: bool) {
    match ty {
        // An unresolved `num` literal displays as its default, `int`.
        Ty::Int(u) | Ty::Num(_, u) => show_numeric("int", u, namer, out),
        Ty::Float(u) => show_numeric("float", u, namer, out),
        Ty::Bool => out.push_str("bool"),
        Ty::Str => out.push_str("string"),
        Ty::Unit => out.push_str("unit"),
        Ty::Var(n) => out.push_str(&namer.ty(*n)),
        Ty::Con(name, args) if args.is_empty() => out.push_str(name),
        Ty::Con(name, args) => {
            if atom {
                out.push('(');
            }
            out.push_str(name);
            for arg in args {
                out.push(' ');
                show_into(arg, namer, out, true);
            }
            if atom {
                out.push(')');
            }
        }
        Ty::Fun(a, b, eff) => {
            if atom {
                out.push('(');
            }
            show_into(a, namer, out, true);
            // Render the latent effect only when it carries concrete labels
            // (`->{io}`, `->{async}`, `->{io, async}` — canonical order, so the
            // display is deterministic); a pure or purely-polymorphic arrow stays
            // the familiar `->`.
            if eff.labels.is_empty() {
                out.push_str(" -> ");
            } else {
                out.push_str(" ->{");
                out.push_str(&eff.show_labels());
                out.push_str("} ");
            }
            show_into(b, namer, out, false);
            if atom {
                out.push(')');
            }
        }
        // A tuple type is self-delimiting (its own parens), matching the literal
        // and annotation surface syntax `(a, b)`.
        Ty::Tuple(elems) => {
            out.push('(');
            for (i, e) in elems.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                show_into(e, namer, out, false);
            }
            out.push(')');
        }
    }
}

fn show_numeric(base: &str, unit: &Unit, namer: &mut Namer, out: &mut String) {
    out.push_str(base);
    if !unit.is_dimensionless() {
        out.push('<');
        out.push_str(&show_unit(unit, namer));
        out.push('>');
    }
}

fn show_unit(unit: &Unit, namer: &mut Namer) -> String {
    let render = |atom: &Atom, exp: i32, namer: &mut Namer| {
        let name = match atom {
            Atom::Base(n) => n.clone(),
            Atom::Var(v) => namer.unit(*v),
        };
        if exp.abs() == 1 {
            name
        } else {
            format!("{name}^{}", exp.abs())
        }
    };
    let mut numer = Vec::new();
    let mut denom = Vec::new();
    for (atom, exp) in &unit.factors {
        if *exp > 0 {
            numer.push(render(atom, *exp, namer));
        } else {
            denom.push(render(atom, *exp, namer));
        }
    }
    let numer = if numer.is_empty() {
        "1".to_string()
    } else {
        numer.join(" ")
    };
    if denom.is_empty() {
        numer
    } else {
        format!("{numer}/{}", denom.join(" "))
    }
}

fn var_name(index: usize) -> String {
    let letter = (b'a' + (index % 26) as u8) as char;
    if index < 26 {
        format!("'{letter}")
    } else {
        format!("'{letter}{}", index / 26)
    }
}
