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
    BinOp, CeBuilder, CeItem, Expr, ExprKind, Item, LetBinding, MatchArm, Module, Pattern,
    TypeDeclKind, TypeExpr, UnOp, UnitExpr,
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
    /// A function `arg -> result` (curried).
    Fun(Box<Ty>, Box<Ty>),
    /// An applied type constructor, e.g. `Option int`.
    Con(String, Vec<Ty>),
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
    ty: Ty,
}

impl Scheme {
    fn mono(ty: Ty) -> Scheme {
        Scheme {
            vars: vec![],
            uvars: vec![],
            num_vars: vec![],
            ord_vars: vec![],
            ty,
        }
    }
}

type Env = HashMap<String, Scheme>;

const BUILTIN_TYPES: [&str; 5] = ["int", "float", "bool", "string", "unit"];

/// Prelude functions backed directly by Python builtins (`DESIGN.md` §6). This is
/// the single source of truth shared by the type checker (whose schemes live in
/// [`seed_prelude`], kept in sync with the arities here) and by lowering, which
/// reads the arities so a *partial* application of a builtin (e.g. `max 0`) still
/// lowers correctly. The Python name equals the Pyfun name for every entry, so no
/// call-site renaming is needed — the simplest honest interop surface.
pub const PRELUDE: &[(&str, usize)] = &[("print", 1), ("abs", 1), ("min", 2), ("max", 2)];

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
    measures: HashSet<String>,
    /// Record types by name.
    records: HashMap<String, RecordInfo>,
    /// Field name → owning record type. Field names are globally unique (the
    /// nominal-record MVP), so a bare `e.x`/`{ x = … }` resolves its record type
    /// from the field name alone, without type annotations (which Pyfun lacks).
    field_owner: HashMap<String, String>,
}

/// Type-check a whole module, returning every independent error found.
pub fn check(module: &Module) -> Result<(), Vec<TypeError>> {
    let mut errors = Vec::new();
    let (decls, ctor_env) = build_decls(module, &mut errors);

    // Start fresh ids past the ids reserved for the seeded built-in schemes, so
    // a freshly allocated (and later bound) variable can't alias a builtin's
    // bound variable and corrupt it via the substitution.
    let mut inf = Infer {
        decls,
        next: RESERVED_VARS,
        ..Infer::default()
    };
    let mut env = ctor_env;

    for item in &module.items {
        match item {
            Item::Measure { .. } | Item::Type(_) => {} // handled by the pre-pass
            Item::Let(binding) => match inf.infer_binding(binding, &env) {
                Ok(scheme) => {
                    env.insert(binding.name.clone(), scheme);
                }
                Err(e) => {
                    errors.push(e);
                    let ty = inf.fresh();
                    env.insert(binding.name.clone(), Scheme::mono(ty));
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

/// Register measures and `type` declarations; build the constructor environment.
fn build_decls(module: &Module, errors: &mut Vec<TypeError>) -> (Decls, Env) {
    let mut decls = Decls::default();
    let mut env = Env::new();
    seed_builtin_types(&mut decls, &mut env);
    seed_prelude(&mut env);

    for item in &module.items {
        if let Item::Measure { name, span } = item
            && !decls.measures.insert(name.clone())
        {
            errors.push(TypeError {
                message: format!("measure `{name}` is already defined"),
                span: span.span(),
            });
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
                            Ty::Fun(Box::new(f), Box::new(acc))
                        });
                    let scheme = Scheme {
                        vars: (0..decl.params.len() as u32).collect(),
                        uvars: vec![],
                        num_vars: vec![],
                        ord_vars: vec![],
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
                    if let Some(other) = decls.field_owner.get(&field.name) {
                        errors.push(TypeError {
                            message: format!(
                                "field `{}` is already defined in record `{other}`",
                                field.name
                            ),
                            span,
                        });
                        ok = false;
                        continue;
                    }
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
                        decls.field_owner.insert(name.clone(), decl.name.clone());
                    }
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

    (decls, env)
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
    // print : 'a -> unit
    env.insert(
        "print".to_string(),
        Scheme {
            vars: vec![0],
            uvars: vec![],
            num_vars: vec![],
            ord_vars: vec![],
            ty: Ty::Fun(Box::new(Ty::Var(0)), Box::new(Ty::Unit)),
        },
    );
    let num_u = || Ty::Num(PRELUDE_NUMVAR, Unit::var(PRELUDE_UVAR));
    let scheme = |ty| Scheme {
        vars: vec![],
        uvars: vec![PRELUDE_UVAR],
        num_vars: vec![PRELUDE_NUMVAR],
        ord_vars: vec![],
        ty,
    };
    // abs : num 'a => 'a<'u> -> 'a<'u>
    env.insert(
        "abs".to_string(),
        scheme(Ty::Fun(Box::new(num_u()), Box::new(num_u()))),
    );
    // min / max : num 'a => 'a<'u> -> 'a<'u> -> 'a<'u>
    let binary = || {
        Ty::Fun(
            Box::new(num_u()),
            Box::new(Ty::Fun(Box::new(num_u()), Box::new(num_u()))),
        )
    };
    for name in ["min", "max"] {
        env.insert(name.to_string(), scheme(binary()));
    }
}

/// Seed the built-in computation-expression types `Async a`, `Seq a`, and
/// `Result a e` (with constructors `Ok`/`Error`) — see `DESIGN.md` §8.1.
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

    let result_ty = Ty::Con("Result".to_string(), vec![Ty::Var(0), Ty::Var(1)]);
    let ok = Scheme {
        vars: vec![0, 1],
        uvars: vec![],
        num_vars: vec![],
        ord_vars: vec![],
        ty: Ty::Fun(Box::new(Ty::Var(0)), Box::new(result_ty.clone())),
    };
    let err = Scheme {
        vars: vec![0, 1],
        uvars: vec![],
        num_vars: vec![],
        ord_vars: vec![],
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

/// Resolve a surface type expression into a [`Ty`].
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
    next: u32,
    decls: Decls,
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
            Ty::Fun(a, b) => Ty::Fun(Box::new(self.apply(a)), Box::new(self.apply(b))),
            Ty::Con(name, args) => {
                Ty::Con(name.clone(), args.iter().map(|a| self.apply(a)).collect())
            }
            other => other.clone(),
        }
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
        let (lb, lu) = self.expect_num(&lt, lhs.span())?;
        let rt = self.infer_expr(rhs, env)?;
        let (rb, ru) = self.expect_num(&rt, rhs.span())?;
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
            BinOp::Add | BinOp::Sub => {
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

    /// Require `ty` to support ordering comparison (`< > <= >=`): the `comparison`
    /// constraint, satisfied by `int`/`float`/`string` (numbers and strings). A
    /// bare type variable gains the constraint and is checked once it resolves.
    fn require_ord(&mut self, ty: &Ty, span: Span) -> Result<(), TypeError> {
        match self.apply(ty) {
            Ty::Int(_) | Ty::Float(_) | Ty::Num(_, _) | Ty::Str => Ok(()),
            Ty::Var(n) => {
                self.ord.insert(n);
                Ok(())
            }
            other => Err(TypeError {
                message: format!("type {} does not support comparison (`<`)", show(&other)),
                span,
            }),
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
                body_env.insert(param.clone(), Scheme::mono(pty));
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
            // Integer literals are polymorphic numerics (`num 'a => 'a`) so they
            // adapt to int or float by context; float literals are concretely
            // float (§7.1).
            ExprKind::Int(_) => Ok(self.fresh_num()),
            ExprKind::Float(_) => Ok(Ty::Float(Unit::dimensionless())),
            ExprKind::Str(_) => Ok(Ty::Str),
            ExprKind::Bool(_) => Ok(Ty::Bool),

            ExprKind::Var(name) => match env.get(name) {
                Some(scheme) => Ok(self.instantiate(scheme)),
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
                BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::FloorDiv => {
                    self.infer_arithmetic(*op, lhs, rhs, span, env)
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
            },

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
                    body_env.insert(param.clone(), Scheme::mono(pty));
                }
                let body_ty = self.infer_expr(body, &body_env)?;
                Ok(param_tys
                    .into_iter()
                    .rev()
                    .fold(body_ty, |acc, p| Ty::Fun(Box::new(p), Box::new(acc))))
            }

            ExprKind::App { func, arg } => self.infer_apply(func, arg, span, env),
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

            ExprKind::Record { fields } => self.infer_record(fields, span, env),
            ExprKind::RecordUpdate { base, fields } => {
                self.infer_record_update(base, fields, span, env)
            }
            ExprKind::Field { base, name } => self.infer_field(base, name, span, env),
        }
    }

    /// The record type owning `field`, or an error if no record declares it.
    fn record_of_field(&self, field: &str, span: Span) -> Result<String, TypeError> {
        self.decls.field_owner.get(field).cloned().ok_or_else(|| {
            // `decls.records` empty means records aren't in use at all.
            let hint = if self.decls.records.is_empty() {
                " (no record types are declared)"
            } else {
                ""
            };
            TypeError {
                message: format!("unknown record field `{field}`{hint}"),
                span,
            }
        })
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
        let record_ty = Ty::Con(name.to_string(), fresh);
        let fields = info
            .fields
            .iter()
            .map(|(f, t)| (f.clone(), subst_all(t, &tmap, &empty_u, &empty_n)))
            .collect();
        (record_ty, fields)
    }

    /// `{ x = e, … }` — a record literal. The type is resolved from the field
    /// names; the literal must mention exactly the record's fields, once each.
    fn infer_record(
        &mut self,
        fields: &[crate::parser::ast::FieldInit],
        span: Span,
        env: &Env,
    ) -> Result<Ty, TypeError> {
        let owner = self.record_of_field(&fields[0].name, span)?;
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
        let mut u = Unit::dimensionless();
        for (name, exp) in &unit.factors {
            if !self.decls.measures.contains(name) {
                return Err(TypeError {
                    message: format!("unknown measure `{name}`"),
                    span,
                });
            }
            u = u.mul(&Unit::base(name).pow(*exp));
        }
        Ok(u)
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
            CeBuilder::Result => self.infer_monad(items, span, env, "Result", true),
            CeBuilder::Async => self.infer_monad(items, span, env, "Async", false),
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
                    let applied = self.apply(&t);
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
                CeItem::LetBang { name, value } => {
                    let t = self.infer_expr(value, &env)?;
                    let inner = self.fresh();
                    let expected = monad(inner.clone(), self);
                    self.unify(&expected, &t, value.span())?;
                    let bound = self.apply(&inner);
                    env.insert(name.clone(), Scheme::mono(bound));
                }
                CeItem::Let { name, value } => {
                    let t = self.infer_expr(value, &env)?;
                    let applied = self.apply(&t);
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
        let func_ty = self.infer_expr(func, env)?;
        let arg_ty = self.infer_expr(arg, env)?;
        let result = self.fresh();
        let expected = Ty::Fun(Box::new(arg_ty), Box::new(result.clone()));
        self.unify(&func_ty, &expected, span)?;
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
            Pattern::Var(name) => {
                env.insert(name.clone(), Scheme::mono(scrut_ty.clone()));
                Ok(())
            }
            Pattern::Int(_) => self.unify(scrut_ty, &Ty::Int(Unit::dimensionless()), span),
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
            // Introduce a fresh variable and reduce exponents toward zero.
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
        // Carry the `comparison` constraint onto each fresh type variable.
        for v in &scheme.ord_vars {
            if let Some(Ty::Var(fresh)) = tmap.get(v) {
                self.ord.insert(*fresh);
            }
        }
        subst_all(&scheme.ty, &tmap, &umap, &nmap)
    }

    fn generalize(&self, env: &Env, ty: &Ty) -> Scheme {
        let ty = self.apply(ty);
        let (env_t, env_u, env_n) = self.env_free_vars(env);
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
            ty,
        }
    }

    fn env_free_vars(&self, env: &Env) -> (HashSet<u32>, HashSet<u32>, HashSet<u32>) {
        let mut tys = HashSet::new();
        let mut units = HashSet::new();
        let mut nums = HashSet::new();
        for scheme in env.values() {
            let bound_t: HashSet<u32> = scheme.vars.iter().copied().collect();
            let bound_u: HashSet<u32> = scheme.uvars.iter().copied().collect();
            let bound_n: HashSet<u32> = scheme.num_vars.iter().copied().collect();
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
        }
        (tys, units, nums)
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

fn free_type_vars(ty: &Ty, f: &mut impl FnMut(u32)) {
    match ty {
        Ty::Var(n) => f(*n),
        Ty::Fun(a, b) => {
            free_type_vars(a, f);
            free_type_vars(b, f);
        }
        Ty::Con(_, args) => args.iter().for_each(|a| free_type_vars(a, f)),
        _ => {}
    }
}

fn free_unit_vars(ty: &Ty, f: &mut impl FnMut(u32)) {
    match ty {
        Ty::Int(u) | Ty::Float(u) | Ty::Num(_, u) => u.var_ids().into_iter().for_each(f),
        Ty::Fun(a, b) => {
            free_unit_vars(a, f);
            free_unit_vars(b, f);
        }
        Ty::Con(_, args) => args.iter().for_each(|a| free_unit_vars(a, f)),
        _ => {}
    }
}

fn free_num_vars(ty: &Ty, f: &mut impl FnMut(u32)) {
    match ty {
        Ty::Num(v, _) => f(*v),
        Ty::Fun(a, b) => {
            free_num_vars(a, f);
            free_num_vars(b, f);
        }
        Ty::Con(_, args) => args.iter().for_each(|a| free_num_vars(a, f)),
        _ => {}
    }
}

fn subst_all(
    ty: &Ty,
    tmap: &HashMap<u32, Ty>,
    umap: &HashMap<u32, Unit>,
    nmap: &HashMap<u32, u32>,
) -> Ty {
    match ty {
        Ty::Var(n) => tmap.get(n).cloned().unwrap_or(Ty::Var(*n)),
        Ty::Int(u) => Ty::Int(u.subst(umap)),
        Ty::Float(u) => Ty::Float(u.subst(umap)),
        Ty::Num(v, u) => Ty::Num(*nmap.get(v).unwrap_or(v), u.subst(umap)),
        Ty::Fun(a, b) => Ty::Fun(
            Box::new(subst_all(a, tmap, umap, nmap)),
            Box::new(subst_all(b, tmap, umap, nmap)),
        ),
        Ty::Con(name, args) => Ty::Con(
            name.clone(),
            args.iter()
                .map(|a| subst_all(a, tmap, umap, nmap))
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
        Ty::Fun(a, b) => {
            if atom {
                out.push('(');
            }
            show_into(a, namer, out, true);
            out.push_str(" -> ");
            show_into(b, namer, out, false);
            if atom {
                out.push(')');
            }
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
