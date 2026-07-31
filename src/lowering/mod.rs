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

use std::collections::{BTreeSet, HashMap, HashSet};

mod decode_spec;
mod fold_loop;
mod self_tail_call;

use crate::lexer::Span;

use crate::parser::ast::{
    ActivePatternDecl, BinOp, BlockStmt, CeBuilder, CeItem, Expr, ExprKind, ExternArg, FieldInit,
    FieldUpdate, InterpPart, Item, LetBinding, Module, Param, Pattern, Receiver, TypeDeclKind,
    TypeExpr,
};
use crate::python_emitter::{PyBinOp, PyCase, PyExpr, PyFStrPart, PyModule, PyPattern, PyStmt};

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

/// Which user sum types / records get their comparison methods emitted (`DESIGN.md`
/// §7.1) — a type only needs `__lt__`/`_pf_order_key` (or, for a record,
/// `@dataclass(order=True)`) where the program actually compares it.
pub enum OrderPolicy {
    /// Order every user type — the sound default for a multi-file project, where a
    /// type declared in one module may be compared in another, separately-compiled one.
    All,
    /// Order only these types — a single-file compile sees the whole program, so it can
    /// emit ordering exactly where a type is compared and shed it everywhere else.
    OnDemand(HashSet<String>),
}

impl OrderPolicy {
    fn needs(&self, type_name: &str) -> bool {
        match self {
            OrderPolicy::All => true,
            OrderPolicy::OnDemand(set) => set.contains(type_name),
        }
    }
}

/// Lower a whole (single-file) module to a Python module — the `Option`/`Result`
/// classes are inlined, the original behavior.
pub fn lower(
    module: &Module,
    float_literals: &HashSet<Span>,
    order: OrderPolicy,
) -> Result<PyModule, LowerError> {
    lower_collecting(module, float_literals, order).map(|(py, _)| py)
}

/// [`lower`], also returning the lowering **notes**: things worth telling the
/// author that are neither errors nor visible in the output — today, a function
/// that calls itself in tail position but keeps its recursive form because a
/// precondition of the loop rewrite failed (`DESIGN.md` §5.4).
pub fn lower_collecting(
    module: &Module,
    float_literals: &HashSet<Span>,
    order: OrderPolicy,
) -> Result<(PyModule, Vec<String>), LowerError> {
    let mut lowerer = Lowerer::new(module);
    lowerer.float_literals = float_literals.clone();
    lowerer.order = order;
    let py = lowerer.lower_module(module)?;
    Ok((py, lowerer.notes))
}

/// Per-module context for multi-file lowering (`DESIGN.md` §6.1): the names of
/// the file modules this module imports (so `Geometry.area` lowers to Python
/// `geometry.area` with `import geometry` hoisted) and the arities of their
/// exported members (so a *partial* application of an imported curried function
/// still lowers to `functools.partial`).
#[derive(Default)]
pub struct ImportContext {
    /// Imported file-module names (`Geometry`).
    pub modules: HashSet<String>,
    /// Qualified member name (`Geometry.area`) → its arity. Includes constructors
    /// that take arguments (`Geometry.Circle`).
    pub member_arities: HashMap<String, usize>,
    /// Qualified names of imported **nullary** constructors (`Palette.Red`), which
    /// must lower to a call (`palette.Red()`) when referenced as a value.
    pub nullary_ctors: HashSet<String>,
    /// Qualified names of imported newtype constructors (`Ids.UserId`), erased at
    /// every use site exactly like the defining module's bare name.
    pub newtype_ctors: HashSet<String>,
    /// Imported records' declared field order, keyed by **qualified** surface tag
    /// (`Geometry.Point` → `["x", "y"]`), so a cross-module literal/update emits a
    /// positional constructor call in the exporting class's `__init__` order.
    pub record_fields: HashMap<String, Vec<String>>,
    /// Field name → the qualified tag of the imported record declaring it
    /// (`x` → `Geometry.Point`), so a cross-module update `{ p with x = 3 }`
    /// (which carries no tag) routes to the imported class.
    pub record_field_owners: HashMap<String, String>,
}

/// A module lowered as part of a multi-file project.
pub struct LoweredModule {
    pub py: PyModule,
    /// Whether this module emitted a `from _pyfun_rt import …` (so the driver
    /// knows the shared runtime file is needed).
    pub uses_runtime: bool,
    /// Lowering notes for this module — see [`lower_collecting`].
    pub notes: Vec<String>,
}

/// Lower a module as one node of a multi-file project (`DESIGN.md` §6.1).
///
/// Unlike [`lower`], the nominal `Option`/`Result` classes are **not** inlined:
/// a module that needs them imports them from the shared `_pyfun_rt.py`
/// ([`runtime_module`]) so that an `Option`/`Result` value crossing a module
/// boundary stays `isinstance`-compatible. Cross-module references route through
/// `ctx`.
pub fn lower_in_project(
    module: &Module,
    ctx: &ImportContext,
    float_literals: &HashSet<Span>,
) -> Result<LoweredModule, LowerError> {
    let mut lowerer = Lowerer::new(module);
    lowerer.float_literals = float_literals.clone();
    lowerer.imported_modules = ctx.modules.clone();
    lowerer.imported_nullary_ctors = ctx.nullary_ctors.clone();
    lowerer
        .newtype_ctors
        .extend(ctx.newtype_ctors.iter().cloned());
    lowerer.use_runtime = true;
    lowerer.project_mode = true;
    for (name, arity) in &ctx.member_arities {
        lowerer.arities.entry(name.clone()).or_insert(*arity);
    }
    // Imported records' field order + field→tag map, keyed by qualified surface tag,
    // so cross-module construction/update route to the exporting class.
    for (tag, fields) in &ctx.record_fields {
        lowerer
            .record_fields
            .entry(tag.clone())
            .or_insert_with(|| fields.clone());
    }
    for (field, tag) in &ctx.record_field_owners {
        lowerer
            .field_to_record
            .entry(field.clone())
            .or_insert_with(|| tag.clone());
    }
    let py = lowerer.lower_module(module)?;
    let uses_runtime = lowerer.needs_result || lowerer.needs_option || lowerer.needs_exception;
    Ok(LoweredModule {
        py,
        uses_runtime,
        notes: lowerer.notes,
    })
}

/// The shared runtime module (`_pyfun_rt.py`): the nominal `Ok`/`Error`/`Some`/
/// `None_`/`_Exception` classes every project module imports, so those values are
/// `isinstance`-compatible across files (`DESIGN.md` §6.1).
pub fn runtime_module() -> PyModule {
    // The shared runtime is a project artifact (multi-file), so it always carries the
    // comparison methods — a `Result`/`Option` may be compared in any importing module.
    let mut body = result_prelude(true);
    body.extend(option_prelude(true));
    body.extend(exception_prelude());
    PyModule { body }
}

/// Lowering-side registry entry for one active-pattern **case** (`DESIGN.md`
/// §7.2), keyed by case name. Everything here is syntactic: `total` comes from
/// the declaration's `|_|` marker, `extra` from its parameter count, and the
/// bool-vs-Option flavor of a partial case is revealed by the use site's binder
/// count (the checker enforces exactly one binder for Option, zero for bool).
#[derive(Clone)]
struct ApUse {
    /// The emitted recognizer function's name (`_ap_Even_Odd`).
    py_fn: String,
    /// Total (`(|A|B|)`) vs partial (`(|A|_|)`).
    total: bool,
    /// Leading parameter-argument count at a use site (`params.len() - 1`).
    extra: usize,
}

struct Lowerer {
    /// Arity of each top-level function (params > 0), used to decide full vs
    /// partial application.
    arities: std::collections::HashMap<String, usize>,
    /// Active-pattern cases (case name → recognizer + shape), from the module's
    /// `let (|…|)` declarations. Drives the if/elif match lowering.
    ap_uses: std::collections::HashMap<String, ApUse>,
    /// Field count of each data constructor, used both to drive constructor
    /// application and to know which bare references are nullary (and so must be
    /// emitted as `Ctor()`).
    ctor_arity: std::collections::HashMap<String, usize>,
    /// Newtype constructors (`opaque type UserId = string`) — erased at lowering:
    /// a fully-applied wrap is the bare argument, a first-class reference is
    /// `_pf_id`, and the single-case pattern recurses into its payload.
    newtype_ctors: HashSet<String>,
    /// Every binder name appearing ANYWHERE in the module — top-level bindings,
    /// parameters, block `let`s, lambda parameters, match-pattern captures, CE
    /// binders ([`collect_binders`]). Drives the module-alias shadow check
    /// ([`Lowerer::py_module_ref`]): a whole-module overapproximation, so the
    /// check never has to model Python's function-wide local hoisting and stays
    /// independent of the fold pass's and closure capture's scope machinery.
    binder_names: HashSet<String>,
    /// Declared field order of each record type, so literals and updates emit a
    /// positional constructor call in the class's `__init__` order.
    record_fields: std::collections::HashMap<String, Vec<String>>,
    /// Field name → owning record type (mirrors the type checker's registry), to
    /// resolve which class a `{ … }` literal or update constructs.
    field_to_record: std::collections::HashMap<String, String>,
    /// `extern` name → its dotted Python target path (e.g. `["math", "sqrt"]`).
    /// A reference to one lowers to the target rather than the Pyfun name.
    extern_targets: std::collections::HashMap<String, Vec<String>>,
    /// Declared `extern import` module paths with their optional `as` aliases
    /// (`DESIGN.md` §6). A used extern target rooted at a declared path — or at
    /// its alias — imports the module exactly as declared, overriding the
    /// lowercase-prefix heuristic (which cannot see that `datetime.datetime` is
    /// a class, not a submodule).
    extern_module_imports: Vec<(Vec<String>, Option<String>)>,
    /// Instance-access externs (`= .read()` / `= .text`) → whether the target is a
    /// method call or a bare property read on the first argument (the receiver).
    receiver_externs: std::collections::HashMap<String, Receiver>,
    /// Externs with a `unit` domain (`unit -> a`, e.g. `time.time`): a *nullary*
    /// Python callable, applied to `()` as a zero-argument call (`time.time()`).
    nullary_externs: HashSet<String>,
    /// `extern` name → its Python keyword arguments (literals already lowered to
    /// `PyExpr`, `...` slots left to be filled from the call), appended to every
    /// emitted call (`open(path, encoding="utf-8")`). Under-application routes
    /// literals through `functools.partial`, and anything with an unfilled slot
    /// through a lambda, so nothing is ever dropped (`DESIGN.md` §6).
    extern_kwargs: std::collections::HashMap<String, Vec<(String, KwSource)>>,
    /// Python modules an *used* extern needs imported (the first segment of a
    /// dotted target, e.g. `math` for `math.sqrt`). Bare builtins import nothing.
    needed_imports: BTreeSet<String>,
    /// Names of top-level `let` bindings, so a user definition shadows a seeded
    /// name (prelude/extern/list helper) at lowering instead of being rerouted.
    user_defs: HashSet<String>,
    /// Bodies of top-level, non-`mut`, 2-parameter `let` bindings (params + body),
    /// so a named folder passed to `Seq.fold`/`List.fold` can be inlined by the
    /// in-place linear-accumulation pass (`src/lowering/fold_loop.rs`, `DESIGN.md`
    /// §5). Only 2-ary defs are recorded (the folder arity).
    top_fn_defs: HashMap<String, (Vec<Param>, Expr)>,
    /// Bodies of top-level, non-`mut`, parameterless `let` bindings, so a
    /// *named decoder* (`let user = Decode.map2 …`) can be classified by the
    /// decode-specialization pass (`src/lowering/decode_spec.rs`, `DESIGN.md`
    /// §5.3). Consulted only for names not shadowed by a local.
    top_val_defs: HashMap<String, Expr>,
    /// The *block-local* counterpart of `top_fn_defs`: 2-ary `let` bindings of the
    /// blocks currently being lowered, so a local named folder (`dedupLegs`'s
    /// inner `step`) can be inlined too. Scope-correct by construction: each block
    /// saves/restores the map around its statements, a non-folder rebinding
    /// removes the name, and every shadowing binder introduction (function
    /// parameters, match-arm pattern bindings) temporarily displaces its entry
    /// (`shadow_local_fns`) — a stale entry must never be consulted, since the
    /// pass would inline the *wrong* body.
    local_fn_defs: HashMap<String, (Vec<Param>, Expr)>,
    /// Whether the in-place linear-fold optimization is enabled. Defaults to on;
    /// the `PYFUN_NO_FOLD_OPT` environment variable turns it off (a kill switch for
    /// differential testing — the rejected path is byte-identical to no-opt).
    fold_opt: bool,
    /// Whether decode specialization is enabled (`src/lowering/decode_spec.rs`).
    /// Defaults to on; `PYFUN_NO_DECODE_OPT` turns it off (the differential kill
    /// switch — the rejected path is the byte-identical interpreter).
    decode_opt: bool,
    /// List-prelude helpers actually referenced, emitted on demand (like the
    /// `Result` prelude). Stored as the Python helper names (e.g. `_pf_map`).
    needed_list_helpers: BTreeSet<&'static str>,
    /// Set/Map-prelude helpers actually referenced (e.g. `_pf_set_add`), emitted on
    /// demand by [`collection_prelude`].
    needed_collection_helpers: BTreeSet<&'static str>,
    /// Standard combinators (`id`/`const`/`ignore`/`flip`) actually referenced,
    /// emitted on demand by [`combinator_prelude`] as `_pf_*` helpers.
    needed_combinators: BTreeSet<&'static str>,
    /// `Decode`-module helpers actually referenced (e.g. `_pf_dec_field`), emitted on
    /// demand by [`decode_prelude`] as `_pf_dec_*` functions.
    needed_decode_helpers: BTreeSet<&'static str>,
    /// Spans of value-position integer *literals* that inference resolved to
    /// `float` (e.g. the `7` in `let x = 7` used later as `x + 1.5`). Such a
    /// literal is emitted as a Python float (`7.0`) so the runtime value matches
    /// its inferred type — otherwise a bare `print x` would show `7`, not `7.0`.
    /// Supplied by the caller (from the type checker); empty means "no coercions".
    float_literals: HashSet<Span>,
    /// Things worth telling the author that are neither errors nor visible in the
    /// emitted output — see [`lower_collecting`].
    notes: Vec<String>,
    /// While lowering an in-file `module`, its name + member names, so a bare
    /// sibling reference rewrites to the mangled top-level name (`Geometry_area`).
    cur_module: Option<(String, HashSet<String>)>,
    /// Names of imported *file* modules (`Geometry`), set for multi-file lowering.
    /// A `Geometry.member` reference routes to Python `geometry.member` (vs the
    /// `Geometry_member` mangling used for in-file `module` declarations).
    imported_modules: HashSet<String>,
    /// Qualified names of imported nullary constructors (`Palette.Red`), referenced
    /// as values, which must lower to a call (`palette.Red()`) not the bare class.
    imported_nullary_ctors: HashSet<String>,
    /// Whether to import the nominal `Option`/`Result` classes from the shared
    /// `_pyfun_rt.py` (multi-file projects) instead of inlining them (single file).
    use_runtime: bool,
    /// Whether this is a multi-file project module. In a project an `extern` is also
    /// emitted as a real top-level binding (`sqrt = math.sqrt`) so a *dependent*
    /// module can reference it as `mathx.sqrt` (`DESIGN.md` §6.1); single-file
    /// lowering keeps externs fully erased (references inline to their dotted target).
    project_mode: bool,
    /// Which user types get comparison methods emitted (`DESIGN.md` §7.1). `All` for a
    /// project (sound across separate compilation); `OnDemand` for a single file (only
    /// the types the program actually compares).
    order: OrderPolicy,
    /// Stack of enclosing *function* scopes (one frame of bound names per nested
    /// function; empty at module level). Used to classify a captured-and-reassigned
    /// `mut` as `nonlocal` (found in an enclosing function) vs `global`
    /// (module-level) when emitting a closure.
    fn_local_stack: Vec<HashSet<String>>,
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
    /// Whether the built-in `Exception` record class (`_Exception`) must be emitted,
    /// set when a `try` expression (which builds one on a caught exception) is lowered.
    needs_exception: bool,
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
        // The reserved `Exception` record (fields errorKind/errorMessage) — the
        // payload of a `try`'s `Error`. Seeded like a user record so its literals and
        // patterns lower through the same machinery (class name mangled by
        // `py_record_class` to dodge Python's builtin `Exception`).
        record_fields.insert(
            "Exception".to_string(),
            vec!["errorKind".to_string(), "errorMessage".to_string()],
        );
        field_to_record.insert("errorKind".to_string(), "Exception".to_string());
        field_to_record.insert("errorMessage".to_string(), "Exception".to_string());
        let mut extern_targets = std::collections::HashMap::new();
        let mut extern_module_imports: Vec<(Vec<String>, Option<String>)> = Vec::new();
        let mut receiver_externs = std::collections::HashMap::new();
        let mut nullary_externs = HashSet::new();
        let mut extern_kwargs: std::collections::HashMap<String, Vec<(String, KwSource)>> =
            std::collections::HashMap::new();
        let mut user_defs = HashSet::new();
        let mut top_fn_defs: HashMap<String, (Vec<Param>, Expr)> = HashMap::new();
        let mut top_val_defs: HashMap<String, Expr> = HashMap::new();
        let mut ap_uses = std::collections::HashMap::new();
        let mut newtype_ctors = HashSet::new();
        // Every binder name in the module, for the module-alias shadow check
        // (`py_module_ref`) — see `collect_binders`.
        let mut binder_names = HashSet::new();
        for item in &module.items {
            match item {
                Item::Let(binding) => {
                    binder_names.insert(binding.name.clone());
                    binder_names.extend(param_names(&binding.params));
                    collect_binders(&binding.value, &mut binder_names);
                }
                Item::Expr(e) => collect_binders(e, &mut binder_names),
                Item::Module { items, .. } => {
                    for member in items {
                        binder_names.insert(member.name.clone());
                        binder_names.extend(param_names(&member.params));
                        collect_binders(&member.value, &mut binder_names);
                    }
                }
                Item::ActivePattern(decl) => {
                    binder_names.extend(param_names(&decl.params));
                    collect_binders(&decl.value, &mut binder_names);
                }
                Item::Type(_)
                | Item::Measure { .. }
                | Item::Import { .. }
                | Item::ExternImport { .. }
                | Item::Extern(_) => {}
            }
        }
        for item in &module.items {
            match item {
                Item::Extern(decl) => {
                    // Arity drives full-vs-partial application, exactly like the
                    // prelude: it is the number of leading arrows in the type.
                    arities.insert(decl.name.clone(), arrow_arity(&decl.ty));
                    extern_targets.insert(decl.name.clone(), decl.target.clone());
                    if let Some(kind) = decl.receiver {
                        receiver_externs.insert(decl.name.clone(), kind);
                    }
                    if is_unit_domain(&decl.ty) {
                        nullary_externs.insert(decl.name.clone());
                    }
                    if !decl.kwargs.is_empty() {
                        let lowered = decl
                            .kwargs
                            .iter()
                            .map(|(k, v)| (k.clone(), lower_extern_arg(v)))
                            .collect();
                        extern_kwargs.insert(decl.name.clone(), lowered);
                    }
                }
                Item::ExternImport { path, alias, .. } => {
                    extern_module_imports.push((path.clone(), alias.clone()));
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
                    // Record a top-level, non-`mut`, parameterless value binding
                    // so the decode-specialization pass can classify a *named*
                    // decoder (`let user = Decode.map2 …`).
                    if !binding.mutable && binding.params.is_empty() {
                        top_val_defs.insert(binding.name.clone(), binding.value.clone());
                    }
                    // Record the body of a top-level, non-`mut`, 2-parameter binding
                    // so the fold-loop pass can inline it as a named folder. The
                    // 2-ary shape is either two `let` parameters or a bare
                    // `let f = fun a x -> …`.
                    if !binding.mutable {
                        let folder = if binding.params.len() == 2 {
                            Some((binding.params.clone(), binding.value.clone()))
                        } else if binding.params.is_empty()
                            && let ExprKind::Fn { params, body } = &binding.value.kind
                            && params.len() == 2
                        {
                            Some((params.clone(), (**body).clone()))
                        } else {
                            None
                        };
                        if let Some(fb) = folder {
                            top_fn_defs.insert(binding.name.clone(), fb);
                        }
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
                    // An opaque handle type erases — no constructor, no class.
                    TypeDeclKind::Opaque => {}
                    // A newtype's constructor is registered (arity 1, so
                    // application/partial-application resolve like any ctor) but
                    // flagged for erasure at every use site.
                    TypeDeclKind::Newtype(_) => {
                        ctor_arity.insert(decl.name.clone(), 1);
                        newtype_ctors.insert(decl.name.clone());
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
                // An active pattern (`DESIGN.md` §7.2): register each case so
                // match arms recognize it, its construction sites (total, inside
                // the body) route to the hidden `_Case` classes, and the
                // recognizer's deterministic Python name is known everywhere.
                Item::ActivePattern(decl) => {
                    let py_fn = ap_py_fn(decl);
                    let extra = decl.params.len() - 1;
                    for case in &decl.cases {
                        ap_uses.insert(
                            case.name.clone(),
                            ApUse {
                                py_fn: py_fn.clone(),
                                total: !decl.partial,
                                extra,
                            },
                        );
                    }
                    if !decl.partial {
                        // Case construction arities (validated by the checker;
                        // `compile` is gated on it, so the scan cannot fail here).
                        for (case, arity) in crate::types::ap_case_arities(decl).unwrap_or_default()
                        {
                            ctor_arity.insert(case, arity);
                        }
                    }
                }
                // `import` lowers to nothing on its own (slice 1); the multi-file
                // driver emits the Python `import` line and routes cross-module refs.
                Item::Measure { .. } | Item::Import { .. } | Item::Expr(_) => {}
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
            notes: Vec::new(),
            arities,
            ap_uses,
            ctor_arity,
            newtype_ctors,
            binder_names,
            record_fields,
            field_to_record,
            extern_targets,
            extern_module_imports,
            receiver_externs,
            nullary_externs,
            extern_kwargs,
            needed_imports: BTreeSet::new(),
            user_defs,
            top_fn_defs,
            top_val_defs,
            local_fn_defs: HashMap::new(),
            // Default the optimization on; `PYFUN_NO_FOLD_OPT` (any value) disables
            // it. Read once per module lowering — cheap, and covers every entry
            // point (`compile`/`run`/project/tests) for differential testing.
            fold_opt: std::env::var_os("PYFUN_NO_FOLD_OPT").is_none(),
            decode_opt: std::env::var_os("PYFUN_NO_DECODE_OPT").is_none(),
            needed_list_helpers: BTreeSet::new(),
            needed_collection_helpers: BTreeSet::new(),
            needed_combinators: BTreeSet::new(),
            needed_decode_helpers: BTreeSet::new(),
            float_literals: HashSet::new(),
            cur_module: None,
            imported_modules: HashSet::new(),
            imported_nullary_ctors: HashSet::new(),
            use_runtime: false,
            project_mode: false,
            // Default to the sound multi-file policy; single-file `lower` overrides it.
            order: OrderPolicy::All,
            fn_local_stack: Vec::new(),
            tmp_counter: 0,
            fn_counter: 0,
            needs_functools: false,
            needs_result: false,
            needs_option: false,
            needs_exception: false,
        }
    }

    fn lower_module(&mut self, module: &Module) -> Result<PyModule, LowerError> {
        // User constructor classes (sum variants) and record classes.
        let mut classes = Vec::new();
        for item in &module.items {
            // A total active pattern's hidden case classes (`_Even`, `_Odd`, …):
            // ordinary ADT classes (structural eq/hash/repr + `__match_args__`)
            // with no ordering (the hidden type never surfaces as a value the
            // checker lets programs compare).
            if let Item::ActivePattern(decl) = item
                && !decl.partial
            {
                for (case, arity) in crate::types::ap_case_arities(decl).unwrap_or_default() {
                    classes.push(PyStmt::ClassDef {
                        name: ap_case_class(&case),
                        fields: (0..arity).map(|i| format!("_{i}")).collect(),
                        // The captured values can be any type — no concrete annotation.
                        field_types: vec!["object".to_string(); arity],
                        order: None,
                        record: false,
                    });
                }
            }
            if let Item::Type(decl) = item {
                // Ordering methods are emitted only where the program compares this type
                // (`DESIGN.md` §7.1); `order` is `Some(rank)` then, `None` otherwise.
                let ordered = self.order.needs(&decl.name);
                match &decl.kind {
                    TypeDeclKind::Sum(variants) => {
                        // The variant's declaration index is its ordering rank, so a
                        // compared sum type derives structural `<`.
                        for (index, variant) in variants.iter().enumerate() {
                            let fields =
                                (0..variant.fields.len()).map(|i| format!("_{i}")).collect();
                            classes.push(PyStmt::ClassDef {
                                name: py_ctor_name(&variant.name),
                                fields,
                                field_types: variant.fields.iter().map(py_annotation).collect(),
                                order: ordered.then_some(index),
                                record: false,
                            });
                        }
                    }
                    // Records lower to a single class (ordering rank 0).
                    TypeDeclKind::Record(fields) => {
                        classes.push(PyStmt::ClassDef {
                            name: decl.name.clone(),
                            fields: fields.iter().map(|f| py_field_name(&f.name)).collect(),
                            field_types: fields.iter().map(|f| py_annotation(&f.ty)).collect(),
                            order: ordered.then_some(0),
                            record: true,
                        });
                    }
                    // An opaque handle type erases — it emits no Python class.
                    TypeDeclKind::Opaque => {}
                    // A newtype erases — its wrap/unwrap sites vanish too.
                    TypeDeclKind::Newtype(_) => {}
                }
            }
        }

        // Lower the code; this is what sets needs_functools / needs_result.
        let mut code = Vec::new();
        for item in &module.items {
            match item {
                // Measures, type declarations, `import`, and `extern import` emit
                // no runtime code (the latter's Python import is hoisted only when
                // a target rooted at it is used — `extern_import_spec`).
                Item::Measure { .. }
                | Item::Type(_)
                | Item::Import { .. }
                | Item::ExternImport { .. } => {}
                // An `extern` erases in single-file lowering (references inline to
                // their dotted target). In a *project* it is also bound at top level
                // (`sqrt = math.sqrt`, `import math` hoisted) so a dependent module can
                // reference it as `mathx.sqrt` (`DESIGN.md` §6.1).
                Item::Extern(decl) => {
                    if self.project_mode {
                        // Any pinned keyword arguments (already lowered in `new`).
                        let kwargs = self
                            .extern_kwargs
                            .get(&decl.name)
                            .cloned()
                            .unwrap_or_default();
                        let value = if let Some(kind) = decl.receiver {
                            // An instance-access extern binds to a receiver-taking
                            // lambda so dependent modules can reference it.
                            receiver_lambda(&decl.target, arrow_arity(&decl.ty), kind, kwargs)
                        } else if is_unit_domain(&decl.ty) {
                            // A nullary extern binds to a lambda that ignores its
                            // unit argument, so a cross-module `Mod.now ()` works.
                            nullary_lambda(&self.extern_path(&decl.target), kwargs)
                        } else {
                            let path = self.extern_path(&decl.target);
                            // A plain extern with pinned kwargs binds to a
                            // `functools.partial` that carries them (or, with a `...`
                            // slot, to a lambda that places it); otherwise to the
                            // bare dotted target.
                            if kwargs.is_empty() {
                                dotted_path(&path)
                            } else {
                                self.build_call_kw_bare(
                                    dotted_path(&path),
                                    Some(arrow_arity(&decl.ty)),
                                    kwargs,
                                )
                            }
                        };
                        code.push(PyStmt::Assign {
                            target: py_value_name(&decl.name),
                            value,
                        });
                    }
                }
                Item::Let(binding) => {
                    // A block-valued binding at module scope has no Python frame
                    // to hide its locals in — a block-local `let` colliding with a
                    // module-level name would emit a module-level assignment that
                    // REBINDS it, corrupting every closure and later use of the
                    // original. When (and only when) such a collision exists, wrap
                    // the evaluation in a fresh nullary function: Python function
                    // scope isolates the block locals, and `lower_fn_body`'s
                    // capture analysis keeps `mut` reassignment declarations right.
                    if binding.params.is_empty() && self.top_binding_needs_frame(binding) {
                        let fname = self.fresh_fn();
                        let body = self.lower_fn_body(&[], &binding.value, &HashSet::new())?;
                        code.push(PyStmt::FuncDef {
                            name: fname.clone(),
                            params: vec![],
                            body,
                            is_async: false,
                        });
                        code.push(PyStmt::Assign {
                            target: py_value_name(&binding.name),
                            value: PyExpr::Call {
                                func: Box::new(PyExpr::Name(fname)),
                                args: vec![],
                            },
                        });
                    } else {
                        self.lower_let(binding, &HashSet::new(), &mut code)?;
                    }
                }
                // The active-pattern recognizer lowers to a plain Python def
                // under its deterministic `_ap_…` name; inside its body the
                // (total) cases construct the hidden `_Case` classes via the
                // ordinary constructor path (registered in `new`).
                Item::ActivePattern(decl) => {
                    let names = param_names(&decl.params);
                    let inner = extend(&HashSet::new(), &param_bindings(&decl.params));
                    let body = self.lower_fn_body(&decl.params, &decl.value, &inner)?;
                    code.push(PyStmt::FuncDef {
                        name: ap_py_fn(decl),
                        params: py_param_names(&names),
                        body,
                        is_async: false,
                    });
                }
                // A module's members lower to flat top-level defs/assignments with
                // mangled names (`Geometry.area` → `Geometry_area`); bare sibling
                // references rewrite to the same names via `cur_module` (set in
                // `lower_var`/`lower_call`).
                Item::Module { name, items, .. } => {
                    let members = items.iter().map(|m| m.name.clone()).collect();
                    self.cur_module = Some((name.clone(), members));
                    for member in items {
                        let mangled = format!("{name}_{}", py_value_name(&member.name));
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
        // In a multi-file project the nominal classes live in the shared runtime
        // (`_pyfun_rt.py`) so they are one type across files; a single file inlines
        // them as before. Imported *before* the `import geometry` lines below would
        // also be fine, but grouping the runtime import with the other froms reads
        // cleanly; emitted after plain `import`s.
        if self.needs_result {
            if self.use_runtime {
                body.push(PyStmt::ImportFrom {
                    module: "_pyfun_rt".to_string(),
                    names: vec!["Ok".to_string(), "Error".to_string()],
                });
            } else {
                body.extend(result_prelude(self.order.needs("Result")));
            }
        }
        if self.needs_option {
            if self.use_runtime {
                body.push(PyStmt::ImportFrom {
                    module: "_pyfun_rt".to_string(),
                    names: vec!["Some".to_string(), "None_".to_string()],
                });
            } else {
                body.extend(option_prelude(self.order.needs("Option")));
            }
        }
        if self.needs_exception {
            if self.use_runtime {
                body.push(PyStmt::ImportFrom {
                    module: "_pyfun_rt".to_string(),
                    names: vec!["_Exception".to_string()],
                });
            } else {
                body.extend(exception_prelude());
            }
        }
        // List-prelude helpers referenced by the program (deterministic order).
        body.extend(list_prelude(&self.needed_list_helpers));
        // Set/Map-prelude helpers referenced by the program.
        body.extend(collection_prelude(&self.needed_collection_helpers));
        // Standard-combinator helpers referenced by the program.
        body.extend(combinator_prelude(&self.needed_combinators));
        // Decode-module helpers referenced by the program.
        body.extend(decode_prelude(&self.needed_decode_helpers));
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

    /// Whether a top-level parameterless binding's value introduces frame-level
    /// binders (block `let`s, match binders) that collide with a module-level
    /// name — the case where module-scope lowering would rebind the original
    /// (`lower_module`'s wrap). Checked against every top-level `let` name, the
    /// (project-mode) extern bindings, and imported file-module names.
    fn top_binding_needs_frame(&self, binding: &LetBinding) -> bool {
        let mut binders = HashSet::new();
        fold_loop::collect_frame_binders(&binding.value, &mut binders);
        if binders.is_empty() {
            return false;
        }
        binders.iter().any(|b| {
            self.user_defs.contains(b)
                || self.extern_targets.contains_key(b)
                || self.imported_modules.contains(b)
        })
    }

    /// Record a block-level `let` in the local-folder registry (`local_fn_defs`):
    /// a non-`mut` 2-ary binding (two `let` params, or a bare 2-ary lambda) is a
    /// candidate named folder for the fold-loop pass; anything else *rebinding*
    /// the name evicts a previous entry (the name no longer means that folder).
    fn note_block_let(&mut self, b: &LetBinding) {
        let folder = if b.mutable {
            None
        } else if b.params.len() == 2 {
            Some((b.params.clone(), b.value.clone()))
        } else if b.params.is_empty()
            && let ExprKind::Fn { params, body } = &b.value.kind
            && params.len() == 2
        {
            Some((params.clone(), (**body).clone()))
        } else {
            None
        };
        match folder {
            Some(fb) => {
                self.local_fn_defs.insert(b.name.clone(), fb);
            }
            None => {
                self.local_fn_defs.remove(&b.name);
            }
        }
    }

    /// Temporarily displace registry entries shadowed by newly-introduced binders
    /// (function parameters / match-arm pattern bindings), returning them for
    /// [`Lowerer::unshadow_local_fns`]. Under a shadow, the name must not resolve
    /// to the outer folder — the fold pass would inline the wrong body.
    fn shadow_local_fns(&mut self, names: &[String]) -> Vec<(String, (Vec<Param>, Expr))> {
        names
            .iter()
            .filter_map(|n| self.local_fn_defs.remove(n).map(|e| (n.clone(), e)))
            .collect()
    }

    fn unshadow_local_fns(&mut self, saved: Vec<(String, (Vec<Param>, Expr))>) {
        for (n, e) in saved {
            self.local_fn_defs.insert(n, e);
        }
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
        // The emitted spelling of the binding (a `Module_member` name arrives here
        // already composed, and is never itself reserved).
        let py_name = py_value_name(name);
        if binding.params.is_empty() {
            let (mut stmts, value) = self.lower_value(&binding.value, locals)?;
            // A binding whose value already *is* the (already-assigned) target — an
            // in-place fold whose accumulator slot is named like the binding
            // (`let m = List.fold (fun m x -> …)`) — would emit a no-op `m = m`.
            // Suppress it: the hoisted statements have already bound the name.
            let redundant = !stmts.is_empty() && matches!(&value, PyExpr::Name(n) if *n == py_name);
            out.append(&mut stmts);
            if !redundant {
                out.push(PyStmt::Assign {
                    target: py_name,
                    value,
                });
            }
        } else {
            // A nested function captures the enclosing locals (Python closures),
            // so they count as locals when resolving names in its body.
            let names = param_names(&binding.params);
            let py_params = py_param_names(&names);
            let inner = extend(locals, &param_bindings(&binding.params));
            let body = self.lower_fn_body(&binding.params, &binding.value, &inner)?;
            // A function that calls itself in tail position loops instead of
            // recursing (`DESIGN.md` §5.4) — CPython has no TCE, so the recursive
            // form walks a stack it has no reason to build. Returns the body
            // untouched when any precondition fails.
            let rewritten = self_tail_call::rewrite(&py_name, &py_params, body);
            if let Some(note) = rewritten.note {
                self.notes.push(note);
            }
            let body = rewritten.body;
            out.push(PyStmt::FuncDef {
                name: py_name,
                params: py_params,
                body,
                is_async: false,
            });
        }
        Ok(())
    }

    /// Lower a function body in tail position, prefixing `global`/`nonlocal`
    /// declarations for any `mut` bindings the body reassigns (`<-`) but does not
    /// itself declare — i.e. captured from an enclosing scope. A captured name found
    /// in an enclosing *function* scope is `nonlocal`; otherwise it is module-level,
    /// so `global` (Python's rule: assigning a name makes it local unless declared).
    fn lower_fn_body(
        &mut self,
        params: &[Param],
        body: &Expr,
        inner: &HashSet<String>,
    ) -> Result<Vec<PyStmt>, LowerError> {
        let prelude_params = params;
        let bindings = param_bindings(params);
        let params: &[String] = &bindings;
        let mut assigned = HashSet::new();
        let mut bound: HashSet<String> = params.iter().cloned().collect();
        scan_scope(body, &mut assigned, &mut bound);
        // Captured = reassigned here but not bound here.
        let mut nonlocals: Vec<String> = Vec::new();
        let mut globals: Vec<String> = Vec::new();
        for name in &assigned {
            if bound.contains(name) {
                continue;
            }
            if self.fn_local_stack.iter().any(|f| f.contains(name)) {
                nonlocals.push(name.clone());
            } else {
                globals.push(name.clone());
            }
        }
        nonlocals.sort();
        globals.sort();

        // Parameters shadow any same-named local folder for the body's duration.
        let shadowed = self.shadow_local_fns(params);
        self.fn_local_stack.push(bound);
        let lowered = self.lower_return(body, inner);
        self.fn_local_stack.pop();
        self.unshadow_local_fns(shadowed);
        let mut stmts = lowered?;

        let mut decls = Vec::new();
        if !globals.is_empty() {
            decls.push(PyStmt::Global(py_param_names(&globals)));
        }
        if !nonlocals.is_empty() {
            decls.push(PyStmt::Nonlocal(py_param_names(&nonlocals)));
        }
        // Destructuring parameters unpack first, after the `global`/`nonlocal`
        // declarations Python requires at the top of the block.
        decls.append(&mut destructure_params(
            prelude_params,
            &param_names(prelude_params),
        ));
        decls.append(&mut stmts);
        Ok(decls)
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
                // A match with active-pattern arms lowers to an if/elif chain
                // (an active pattern is a function call, not a structural test).
                if self.match_uses_ap(arms) {
                    return self.lower_ap_match(scrutinee, arms, locals, None);
                }
                let (mut stmts, subject) = self.lower_value(scrutinee, locals)?;
                let mut cases = Vec::new();
                for arm in arms {
                    let pattern = self.lower_pattern(&arm.pattern);
                    let bindings = pattern_bindings(&arm.pattern);
                    let arm_locals = extend(locals, &bindings);
                    // Pattern binders shadow same-named local folders (fold pass).
                    let shadowed = self.shadow_local_fns(&bindings);
                    let guard = self.lower_guard(&arm.guard, &arm_locals)?;
                    let body = self.lower_return(&arm.body, &arm_locals)?;
                    self.unshadow_local_fns(shadowed);
                    cases.push(PyCase {
                        pattern,
                        guard,
                        body,
                    });
                }
                seal_cases(arms, &mut cases);
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
        // Block scope for the local-folder registry: entries this block adds
        // (or evicts) must not outlive it.
        let saved_local_fns = self.local_fn_defs.clone();
        for (i, stmt) in stmts.iter().enumerate() {
            match stmt {
                BlockStmt::Let(b) => {
                    self.lower_let(b, &locals, &mut out)?;
                    self.note_block_let(b);
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
        self.local_fn_defs = saved_local_fns;
        Ok(out)
    }

    /// Lower `expr` in value position: a list of statements to run first, plus a
    /// Python expression denoting the value.
    fn lower_value(&mut self, expr: &Expr, locals: &HashSet<String>) -> Lowered {
        match &expr.kind {
            // A hole never reaches lowering — `compile`/`run` are gated on a clean
            // type check, which reports holes and blocks. Defensive.
            ExprKind::Hole { name } => Err(LowerError {
                message: match name {
                    Some(n) => format!("cannot compile: unfilled hole `?{n}`"),
                    None => "cannot compile: unfilled hole `?`".to_string(),
                },
            }),
            ExprKind::Int(n) => {
                // An integer literal that inference resolved to `float` is emitted
                // as a Python float, so the runtime value matches its type.
                if self.float_literals.contains(&expr.span()) {
                    Ok((vec![], PyExpr::Float(*n as f64)))
                } else {
                    Ok((vec![], PyExpr::Int(*n)))
                }
            }
            ExprKind::Float(f) => Ok((vec![], PyExpr::Float(*f))),
            ExprKind::Str(s) => Ok((vec![], PyExpr::Str(s.clone()))),
            ExprKind::Bool(b) => Ok((vec![], PyExpr::Bool(*b))),
            ExprKind::Unit => Ok((vec![], PyExpr::NoneLit)),
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

            // A chained comparison lowers 1:1 to Python's native chained comparison
            // (evaluate-once, short-circuit) — no desugaring to `and` needed.
            ExprKind::Compare { first, rest } => {
                let (mut stmts, left) = self.lower_value(first, locals)?;
                let mut ops = Vec::with_capacity(rest.len());
                let mut comparators = Vec::with_capacity(rest.len());
                for (op, operand) in rest {
                    let (s, v) = self.lower_value(operand, locals)?;
                    stmts.extend(s);
                    ops.push(lower_binop(*op));
                    comparators.push(v);
                }
                Ok((
                    stmts,
                    PyExpr::Compare {
                        left: Box::new(left),
                        ops,
                        comparators,
                    },
                ))
            }

            // `(op)` lowers as its desugared curried lambda `fun a b -> a op b`,
            // so partial application (`(*) 2`) and first-class use go through the
            // ordinary function-lowering path.
            ExprKind::OpFunc(op) => {
                let lam = crate::desugar::op_func(*op, expr.span());
                self.lower_value(&lam, locals)
            }

            // `f >> g` / `f << g` — desugar to a composition lambda, then lower.
            ExprKind::Compose {
                lhs,
                rhs,
                right_to_left,
            } => {
                let lam = crate::desugar::compose(
                    (**lhs).clone(),
                    (**rhs).clone(),
                    *right_to_left,
                    expr.span(),
                );
                self.lower_value(&lam, locals)
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
                // Active-pattern arms: the if/elif chain assigns a temp.
                if self.match_uses_ap(arms) {
                    let tmp = self.fresh_tmp();
                    let stmts = self.lower_ap_match(scrutinee, arms, locals, Some(&tmp))?;
                    return Ok((stmts, PyExpr::Name(tmp)));
                }
                // Python `match` is a statement, so always hoist into a temp.
                let (mut stmts, subject) = self.lower_value(scrutinee, locals)?;
                let tmp = self.fresh_tmp();
                let mut cases = Vec::new();
                for arm in arms {
                    let pattern = self.lower_pattern(&arm.pattern);
                    let bindings = pattern_bindings(&arm.pattern);
                    let arm_locals = extend(locals, &bindings);
                    // Pattern binders shadow same-named local folders (fold pass).
                    let shadowed = self.shadow_local_fns(&bindings);
                    let guard = self.lower_guard(&arm.guard, &arm_locals)?;
                    let (arm_stmts, arm_val) = self.lower_value(&arm.body, &arm_locals)?;
                    self.unshadow_local_fns(shadowed);
                    cases.push(PyCase {
                        pattern,
                        guard,
                        body: with_assign(arm_stmts, &tmp, arm_val),
                    });
                }
                seal_cases(arms, &mut cases);
                stmts.push(PyStmt::Match { subject, cases });
                Ok((stmts, PyExpr::Name(tmp)))
            }

            ExprKind::Fn { params, body } => {
                let names = param_names(params);
                let inner = extend(locals, &param_bindings(params));
                // Lambda parameters shadow any same-named local folder while the
                // body lowers (`lower_fn_body` re-guards the statement-body pass).
                let shadowed = self.shadow_local_fns(&names);
                let lowered = self.lower_value(body, &inner);
                self.unshadow_local_fns(shadowed);
                let (body_stmts, body_val) = lowered?;
                // A destructuring parameter needs an unpacking statement in the
                // body, and Python 3 removed tuple parameters, so such a lambda
                // takes the named-def path below however simple its body is.
                let all_named = params.iter().all(|p| p.name().is_some());
                if body_stmts.is_empty() && all_named {
                    Ok((
                        vec![],
                        PyExpr::Lambda {
                            params: py_param_names(&names),
                            body: Box::new(body_val),
                        },
                    ))
                } else {
                    // Body needs statements: emit a named nested def and use it.
                    let name = self.fresh_fn();
                    let def_body = self.lower_fn_body(params, body, &inner)?;
                    let def = PyStmt::FuncDef {
                        name: name.clone(),
                        params: py_param_names(&names),
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
                    crate::parser::ast::UnOp::Neg => PyExpr::Neg(Box::new(value)),
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

            ExprKind::Tuple { elems } => {
                let mut stmts = Vec::new();
                let mut vals = Vec::with_capacity(elems.len());
                for e in elems {
                    let (s, v) = self.lower_value(e, locals)?;
                    stmts.extend(s);
                    vals.push(v);
                }
                Ok((stmts, PyExpr::Tuple(vals)))
            }

            // An interpolated string lowers ~1:1 to a Python f-string: literal chunks
            // pass through, holes become `{expr}` (Python stringifies each). Any
            // statements a hole hoists run first.
            ExprKind::Interp { parts } => {
                let mut stmts = Vec::new();
                let mut py_parts = Vec::with_capacity(parts.len());
                for part in parts {
                    match part {
                        InterpPart::Lit(s) => py_parts.push(PyFStrPart::Lit(s.clone())),
                        InterpPart::Expr(e) => {
                            let (s, v) = self.lower_value(e, locals)?;
                            stmts.extend(s);
                            py_parts.push(PyFStrPart::Expr(v));
                        }
                    }
                }
                Ok((stmts, PyExpr::FStr(py_parts)))
            }

            // `try body` → run the body in a `try`, assigning `Ok(value)`; a caught
            // exception becomes `Error(_Exception(type(e).__name__, str(e)))`. The
            // result lands in a temp that becomes the expression's value.
            ExprKind::Try { body } => {
                self.needs_result = true;
                self.needs_exception = true;
                let (body_stmts, body_val) = self.lower_value(body, locals)?;
                let result_tmp = self.fresh_tmp();
                let exc = self.fresh_tmp(); // the `except ... as <exc>` binding
                let mut try_body = body_stmts;
                try_body.push(PyStmt::Assign {
                    target: result_tmp.clone(),
                    value: call1("Ok", body_val),
                });
                // _Exception(type(e).__name__, str(e))
                let kind = PyExpr::Attribute {
                    value: Box::new(PyExpr::Call {
                        func: Box::new(PyExpr::Name("type".to_string())),
                        args: vec![PyExpr::Name(exc.clone())],
                    }),
                    attr: "__name__".to_string(),
                };
                let message = PyExpr::Call {
                    func: Box::new(PyExpr::Name("str".to_string())),
                    args: vec![PyExpr::Name(exc.clone())],
                };
                let payload = PyExpr::Call {
                    func: Box::new(PyExpr::Name(py_record_class("Exception"))),
                    args: vec![kind, message],
                };
                let handler = vec![PyStmt::Assign {
                    target: result_tmp.clone(),
                    value: call1("Error", payload),
                }];
                let try_stmt = PyStmt::Try {
                    body: try_body,
                    exc_type: Some("Exception".to_string()),
                    binding: Some(exc),
                    handler,
                };
                Ok((vec![try_stmt], PyExpr::Name(result_tmp)))
            }

            ExprKind::Record { ty, fields, .. } => self.lower_record(ty, fields, locals),
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
                        attr: py_field_name(name),
                    },
                ))
            }

            ExprKind::Block { stmts } => self.lower_block_value(stmts, locals),

            ExprKind::Assign { target, value } => {
                let (mut stmts, v) = self.lower_value(value, locals)?;
                stmts.push(PyStmt::Assign {
                    target: py_value_name(target),
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
        // Block scope for the local-folder registry (see `lower_block_return`).
        let saved_local_fns = self.local_fn_defs.clone();
        for (i, stmt) in stmts.iter().enumerate() {
            match stmt {
                BlockStmt::Let(b) => {
                    self.lower_let(b, &locals, &mut out)?;
                    self.note_block_let(b);
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
        self.local_fn_defs = saved_local_fns;
        Ok((out, value))
    }

    /// `Point { x = a, y = b }` → `Point(a, b)` — a positional constructor call in the
    /// class's declared field order (the type checker guarantees the literal names
    /// exactly the record's fields). The record is the literal's **tag** (which may be
    /// qualified for an imported record, `Geometry.Point` → `geometry.Point(...)`).
    fn lower_record(
        &mut self,
        ty: &str,
        fields: &[FieldInit],
        locals: &HashSet<String>,
    ) -> Lowered {
        let order = self.record_fields[ty].clone();
        let class = self.record_class_name(ty);
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
                func: Box::new(PyExpr::Name(class)),
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
        fields: &[FieldUpdate],
        locals: &HashSet<String>,
    ) -> Lowered {
        // The base is bound to a temp so it is evaluated **once**; every field of the
        // reconstruction (updated or copied, at any depth) then reads from the temp.
        let (mut stmts, base_val) = self.lower_value(base, locals)?;
        let tmp = self.fresh_tmp();
        stmts.push(PyStmt::Assign {
            target: tmp.clone(),
            value: base_val,
        });
        // Lower each update's value (hoisting any statements), keeping its path.
        let mut updates: Vec<(Vec<String>, PyExpr)> = Vec::with_capacity(fields.len());
        for fu in fields {
            let (mut s, v) = self.lower_value(&fu.value, locals)?;
            stmts.append(&mut s);
            updates.push((fu.path.clone(), v));
        }
        // The outer record's tag is resolved from the first path's first segment
        // (the type checker has verified all paths start in the base's record).
        let tag = self.field_to_record[&fields[0].path[0]].clone();
        let value = self.build_record_update(PyExpr::Name(tmp.clone()), &tag, updates);
        Ok((stmts, value))
    }

    /// Reconstruct a record value `class(...)` from `base_py` (a pure expression —
    /// the shared temp or an attribute chain off it), replacing the given field
    /// paths. Nested paths (`a.b = v`) recurse, reconstructing the sub-record from
    /// `base_py.a`; paths sharing a prefix are grouped so a field is rebuilt once.
    fn build_record_update(
        &mut self,
        base_py: PyExpr,
        tag: &str,
        updates: Vec<(Vec<String>, PyExpr)>,
    ) -> PyExpr {
        let order = self.record_fields[tag].clone();
        let class = self.record_class_name(tag);
        // Group updates by their first path segment; the remaining path (possibly
        // empty, meaning a wholesale set of that field) goes to the recursion.
        let mut by_field: HashMap<String, Vec<(Vec<String>, PyExpr)>> = HashMap::new();
        for (path, val) in updates {
            let (head, rest) = path.split_first().expect("non-empty update path");
            by_field
                .entry(head.clone())
                .or_default()
                .push((rest.to_vec(), val));
        }
        let mut args = Vec::with_capacity(order.len());
        for field in &order {
            let attr = PyExpr::Attribute {
                value: Box::new(base_py.clone()),
                attr: py_field_name(field),
            };
            match by_field.remove(field) {
                // Not updated: copy from the base.
                None => args.push(attr),
                Some(group) => {
                    if let Some(pos) = group.iter().position(|(p, _)| p.is_empty()) {
                        // A wholesale set (`a = v`, empty remaining path) replaces the
                        // field; the checker forbids mixing it with a sub-path update.
                        args.push(group.into_iter().nth(pos).unwrap().1);
                    } else {
                        // Nested: rebuild the sub-record from `base_py.field`. The
                        // sub-record's tag is the owner of the next path segment.
                        let sub_tag = self.field_to_record[&group[0].0[0]].clone();
                        args.push(self.build_record_update(attr, &sub_tag, group));
                    }
                }
            }
        }
        PyExpr::Call {
            func: Box::new(PyExpr::Name(class)),
            args,
        }
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

    /// The Python name this module's emitted code uses for an imported file
    /// module — normally the lowercase module name (`Geometry` → `geometry`,
    /// matching `import geometry`). When ANY binder anywhere in the module
    /// claims that name (`binder_names`: top-level bindings, parameters, block
    /// `let`s, lambda parameters, match-pattern captures, CE binders — Python
    /// hoists locals, so even a binder *after* the reference shadows it for its
    /// whole function), the import is aliased (`import ids as _pf_ids`) and
    /// every qualified reference routes through the alias. The decision is
    /// deliberately whole-module: one collision anywhere costs only the alias
    /// spelling, and keeps this check independent of the scope machinery the
    /// fold pass and closure capture rely on.
    fn py_module_ref(&mut self, base: &str) -> String {
        let module = base.to_lowercase();
        if self.binder_names.contains(&module) {
            let alias = format!("_pf_{module}");
            self.needed_imports.insert(format!("{module} as {alias}"));
            alias
        } else {
            self.needed_imports.insert(module.clone());
            module
        }
    }

    /// Register the Python import a used `extern` target needs, and return the
    /// path its emitted reference should be built from.
    ///
    /// A plain `import math` is shadowed by any module-level binding named `math`,
    /// and Python resolves `math.sqrt` at *call* time, so the call finds the user's
    /// value: `'int' object has no attribute 'sqrt'`. When a binder anywhere in the
    /// module claims the name the reference is rooted at, the import is aliased
    /// (`import math as _pf_math`) and the path re-rooted — the same dodge
    /// [`Lowerer::py_module_ref`] applies to imported *Pyfun* modules. A dotted
    /// import binds only its first segment (`import os.path` binds `os`), so the
    /// alias replaces however many segments the module spec covers
    /// (`import os.path as _pf_os` ⇒ `_pf_os.join`).
    fn extern_path(&mut self, target: &[String]) -> Vec<String> {
        let Some(spec) = self.extern_import_spec(target) else {
            // A bare builtin root (`bytes.decode`) — nothing to import, and
            // `py_value_name` keeps user bindings off those names.
            return target.to_vec();
        };
        let Some(root) = target.first() else {
            return target.to_vec();
        };
        if !self.binder_names.contains(root) {
            self.needed_imports.insert(spec);
            return target.to_vec();
        }
        // `spec` is either `module` or `module as alias`; an aliased import is
        // referenced through its alias, which is one segment.
        let (module, consumed) = match spec.split_once(" as ") {
            Some((module, _)) => (module.to_string(), 1),
            None => (spec.clone(), spec.split('.').count()),
        };
        let alias = format!("_pf_{root}");
        self.needed_imports.insert(format!("{module} as {alias}"));
        let mut path = vec![alias];
        path.extend(target.iter().skip(consumed).cloned());
        path
    }

    /// The import spec a used extern target needs — `"datetime"` or
    /// `"numpy as np"` — or `None` for a bare builtin. Declared `extern import`s
    /// are consulted first (`DESIGN.md` §6): an *aliased* declaration matches a
    /// target rooted at its alias name; an unaliased one matches the longest
    /// declared path that strictly prefixes the target. Only when no declaration
    /// matches does the lowercase-prefix heuristic ([`extern_import`]) decide.
    fn extern_import_spec(&self, target: &[String]) -> Option<String> {
        let mut best: Option<&(Vec<String>, Option<String>)> = None;
        for decl in &self.extern_module_imports {
            let (path, alias) = decl;
            let hit = match alias {
                Some(a) => target.first() == Some(a),
                None => target.len() > path.len() && target.starts_with(path),
            };
            if hit && best.is_none_or(|(b, _)| path.len() > b.len()) {
                best = Some(decl);
            }
        }
        match best {
            Some((path, Some(a))) => Some(format!("{} as {a}", path.join("."))),
            Some((path, None)) => Some(path.join(".")),
            None => extern_import(target),
        }
    }

    fn lower_application(&mut self, expr: &Expr, locals: &HashSet<String>) -> Lowered {
        let mut args_ast = Vec::new();
        let head = flatten_app(expr, &mut args_ast);

        // Tier-1 in-place linear accumulation (`DESIGN.md` §5): a qualifying
        // fully-applied `Seq.fold`/`List.fold` inlines to a `for`-loop over a
        // mutable accumulator. On any doubt the analysis returns `None` and we fall
        // through to the byte-identical `_pf_fold` lowering below.
        if self.fold_opt
            && args_ast.len() == 3
            && crate::types::qualified_name(head)
                .is_some_and(|q| q == "Seq.fold" || q == "List.fold")
            && let Some(result) = self.try_lower_fold_loop(&args_ast, locals)?
        {
            return Ok(result);
        }

        // Decode specialization (`DESIGN.md` §5.3): a fully-applied
        // `Decode.decodeString` over a statically-known decoder composition
        // deforests the combinator interpreter into direct dict/list access.
        // On any doubt the analysis returns `None` and we fall through to the
        // byte-identical interpreter lowering.
        if self.decode_opt
            && args_ast.len() == 2
            && crate::types::qualified_name(head).is_some_and(|q| q == "Decode.decodeString")
            && let Some(result) = self.try_lower_decode_spec(&args_ast, locals)?
        {
            return Ok(result);
        }

        // Inline fully-applied pure 1:1 stdlib helpers (`ROADMAP.md` Lever A,
        // `DESIGN.md` §6): a fully-applied call to a pure, total one-liner wrapper
        // over a Python idiom emits that idiom directly (`needle in s`) instead of a
        // `_pf_*` helper call — one fewer function call per invocation and more
        // readable. Partial application / a bare value reference is deliberately NOT
        // matched here (arity is exact), so it falls through to the helper below —
        // `List.map (String.contains "x") xs` keeps working via `_pf_str_contains`.
        if let ExprKind::Field { .. } = &head.kind
            && let Some(q) = crate::types::qualified_name(head)
            && let Some(result) = self.try_inline_stdlib(&q, &args_ast, locals)?
        {
            return Ok(result);
        }

        // An instance-access extern applies to a receiver: `= .read()` calls
        // `recv.read(args)`, `= .text` reads `recv.text`. The first argument is the
        // receiver; for a method the rest are its arguments.
        if let ExprKind::Var(name) = &head.kind
            && !locals.contains(name)
            && let Some(kind) = self.receiver_externs.get(name).copied()
        {
            let member = self.extern_targets[name].clone();
            let arity = self.arities.get(name).copied();
            // A method extern may pin fixed Python kwargs (`= .write_text(encoding=…)`);
            // a property never does (the parser forbids parens on it).
            let kwargs = self.extern_kwargs.get(name).cloned().unwrap_or_default();
            let mut stmts = Vec::new();
            let mut arg_vals = Vec::with_capacity(args_ast.len());
            for arg in &args_ast {
                let (arg_stmts, arg_val) = self.lower_value(arg, locals)?;
                stmts.extend(arg_stmts);
                arg_vals.push(arg_val);
            }
            // A bare (unapplied) reference becomes a receiver-taking lambda (which
            // pins any kwargs itself, `lambda r, a: r.write_text(a, encoding=…)`).
            if arg_vals.is_empty() {
                return Ok((
                    stmts,
                    receiver_lambda(&member, arity.unwrap_or(1), kind, kwargs),
                ));
            }
            let mut recv = arg_vals.remove(0);
            let method_arity = arity.map(|k| k.saturating_sub(1));
            // An under-applied `...` slot puts the call inside a lambda, so the
            // receiver must be bound out here to keep evaluating at application time.
            let defers =
                slot_count(&kwargs) > 0 && method_arity.is_some_and(|k| arg_vals.len() < k);
            if defers {
                recv = self.hoist_tmp(recv, &mut stmts);
            }
            let accessed = attr_path(recv, &member);
            let result = match kind {
                // Property: `recv.text`; any further args are over-application calls.
                Receiver::Property => arg_vals.into_iter().fold(accessed, |f, a| PyExpr::Call {
                    func: Box::new(f),
                    args: vec![a],
                }),
                // A method extern with kwargs routes every arity through
                // `build_call_kw` so they are appended (full/over) or carried by
                // `functools.partial` / a lambda (receiver-only, method-partial) —
                // never lost.
                Receiver::Method if !kwargs.is_empty() => {
                    let mut hoist = Vec::new();
                    let call =
                        self.build_call_kw(accessed, method_arity, arg_vals, kwargs, &mut hoist);
                    stmts.extend(hoist);
                    call
                }
                Receiver::Method => {
                    // The method itself takes one fewer argument than the arity.
                    if arg_vals.is_empty() {
                        match method_arity {
                            // A nullary method: call it now (`resp.read()`).
                            Some(0) => PyExpr::Call {
                                func: Box::new(accessed),
                                args: vec![],
                            },
                            // Receiver-only partial: the bound method *is* the
                            // partial (`execute conn` → `conn.execute`).
                            _ => accessed,
                        }
                    } else {
                        self.build_call(accessed, method_arity, arg_vals)
                    }
                }
            };
            return Ok((stmts, result));
        }

        // A nullary extern (`unit -> a`) applied to `()` is a zero-argument Python
        // call: `now ()` → `time.time()`, never `time.time(None)`. The unit argument
        // is evaluated for any effects but dropped from the call.
        if let ExprKind::Var(name) = &head.kind
            && !locals.contains(name)
            && self.nullary_externs.contains(name)
        {
            let target = self.extern_path(&self.extern_targets[name].clone());
            let mut stmts = Vec::new();
            let mut arg_vals = Vec::with_capacity(args_ast.len());
            for arg in &args_ast {
                let (arg_stmts, arg_val) = self.lower_value(arg, locals)?;
                stmts.extend(arg_stmts);
                arg_vals.push(arg_val);
            }
            // Drop the leading unit argument; call the target with no arguments
            // (plus any pinned kwargs, `time.time()` → `f(tz=…)`).
            let base = match self.extern_kwargs.get(name).cloned() {
                // A nullary extern has no argument to spare, so the parser rejects a
                // `...` slot on one and these kwargs are all pinned literals.
                Some(spec) => {
                    let (_, kwargs) = bind_kwargs(&spec, Vec::new());
                    PyExpr::CallKw {
                        func: Box::new(dotted_path(&target)),
                        args: vec![],
                        kwargs,
                    }
                }
                None => PyExpr::Call {
                    func: Box::new(dotted_path(&target)),
                    args: vec![],
                },
            };
            // Any further arguments (a `unit -> b -> c` extern) apply to the result.
            let result = arg_vals
                .into_iter()
                .skip(1)
                .fold(base, |f, a| PyExpr::Call {
                    func: Box::new(f),
                    args: vec![a],
                });
            return Ok((stmts, result));
        }

        // A plain (non-receiver, non-nullary) extern carrying Python kwargs:
        // `openText path` → `builtins.open(path, mode="rt", encoding="utf-8")`.
        // Full/over-application places them on the direct call, a `...` slot taking
        // its value from the trailing arguments; under-application carries pinned
        // literals through `functools.partial` and an unfilled slot through a lambda
        // (`build_call_kw`), so nothing is ever silently dropped (`DESIGN.md` §6).
        if let ExprKind::Var(name) = &head.kind
            && !locals.contains(name)
            && !self.user_defs.contains(name)
            && let Some(kwargs) = self.extern_kwargs.get(name).cloned()
        {
            let target = self.extern_path(&self.extern_targets[name].clone());
            let arity = self.arities.get(name).copied();
            let mut stmts = Vec::new();
            let mut arg_vals = Vec::with_capacity(args_ast.len());
            for arg in &args_ast {
                let (arg_stmts, arg_val) = self.lower_value(arg, locals)?;
                stmts.extend(arg_stmts);
                arg_vals.push(arg_val);
            }
            let mut hoist = Vec::new();
            let call =
                self.build_call_kw(dotted_path(&target), arity, arg_vals, kwargs, &mut hoist);
            stmts.extend(hoist);
            return Ok((stmts, call));
        }

        // A fully-applied newtype wrap erases: `UserId x` (or the imported
        // `Ids.UserId x`) lowers to `x`. Any extra arguments (an underlying
        // function value being applied) chain onto the unwrapped value as an
        // ordinary call.
        let newtype_head = match &head.kind {
            ExprKind::Var(name) => self.newtype_ctors.contains(name),
            ExprKind::Field { .. } => {
                crate::types::qualified_name(head).is_some_and(|q| self.newtype_ctors.contains(&q))
            }
            _ => false,
        };
        if newtype_head && !args_ast.is_empty() {
            let (mut stmts, first) = self.lower_value(args_ast[0], locals)?;
            if args_ast.len() == 1 {
                return Ok((stmts, first));
            }
            let mut rest = Vec::with_capacity(args_ast.len() - 1);
            for arg in &args_ast[1..] {
                let (arg_stmts, arg_val) = self.lower_value(arg, locals)?;
                stmts.extend(arg_stmts);
                rest.push(arg_val);
            }
            return Ok((stmts, self.build_call(first, None, rest)));
        }

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
            // An operator section `(op)` is the binary lambda `fun a b -> a op b`,
            // so partial application (`(*) 2`) curries like any 2-arity function.
            ExprKind::OpFunc(_) => Some(2),
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

    /// Lever A (`ROADMAP.md`): inline a fully-applied pure 1:1 stdlib helper to the
    /// Python idiom it wraps (`String.contains n s` → `n in s`) instead of emitting a
    /// `_pf_*` helper call. Returns `Some(lowered)` only when `qualified` is one of
    /// the inlinable members AND the call supplies its exact arity (partial
    /// application returns `None`, so the caller falls back to the helper). The
    /// argument order matches each helper body verbatim — see `list_prelude`,
    /// `collection_prelude`, `string_prelude` — so this can never invert operands.
    /// `List`/`Set`/`Map.len` are *not* here: they already lower to a bare `len`, so
    /// a fully-applied call is already `len(xs)` with no helper in between.
    fn try_inline_stdlib(
        &mut self,
        qualified: &str,
        args_ast: &[&Expr],
        locals: &HashSet<String>,
    ) -> Result<Option<(Vec<PyStmt>, PyExpr)>, LowerError> {
        // Required arity for each inlinable member; anything else (a partial
        // application) is left to the helper path.
        let arity = match qualified {
            "String.contains" | "String.startsWith" | "String.endsWith" | "List.contains"
            | "Set.contains" | "Map.contains" => 2,
            "List.isEmpty" => 1,
            _ => return Ok(None),
        };
        if args_ast.len() != arity {
            return Ok(None);
        }
        // Lower the argument expressions once, hoisting any statements they produce.
        let mut stmts = Vec::new();
        let mut vals = Vec::with_capacity(args_ast.len());
        for arg in args_ast {
            let (arg_stmts, arg_val) = self.lower_value(arg, locals)?;
            stmts.extend(arg_stmts);
            vals.push(arg_val);
        }
        // `x in container` via `Compare` (comparison precedence + parenthesization
        // for free). `a`, `b` are consumed in helper-body order.
        let membership = |a: PyExpr, b: PyExpr| PyExpr::Compare {
            left: Box::new(a),
            ops: vec![PyBinOp::In],
            comparators: vec![b],
        };
        // `recv.method(arg)`.
        let method = |recv: PyExpr, m: &str, arg: PyExpr| PyExpr::Call {
            func: Box::new(PyExpr::Attribute {
                value: Box::new(recv),
                attr: m.to_string(),
            }),
            args: vec![arg],
        };
        let mut it = vals.into_iter();
        let a = it.next().unwrap();
        let value = match qualified {
            // `_pf_str_contains(sub, s) -> sub in s`
            "String.contains" => membership(a, it.next().unwrap()),
            // `_pf_str_starts_with(pre, s) -> s.startswith(pre)`
            "String.startsWith" => method(it.next().unwrap(), "startswith", a),
            // `_pf_str_ends_with(suf, s) -> s.endswith(suf)`
            "String.endsWith" => method(it.next().unwrap(), "endswith", a),
            // `_pf_list_contains(x, xs) -> x in xs`
            "List.contains" => membership(a, it.next().unwrap()),
            // `_pf_set_contains(x, s) -> x in s`
            "Set.contains" => membership(a, it.next().unwrap()),
            // `_pf_map_contains(k, m) -> k in m`
            "Map.contains" => membership(a, it.next().unwrap()),
            // `_pf_is_empty(xs) -> len(xs) == 0`, inlined to the equivalent `not xs`.
            "List.isEmpty" => PyExpr::Not(Box::new(a)),
            _ => unreachable!("try_inline_stdlib arity gate admitted {qualified}"),
        };
        Ok(Some((stmts, value)))
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
            return PyExpr::Name(format!("{m}_{}", py_value_name(name)));
        }
        // A local parameter or a user top-level binding shadows a seeded name
        // (extern routing), so skip rerouting in that case. Module members
        // (`List.map`, …) are field-access nodes, routed in `lower_value`, not here.
        if !locals.contains(name) && !self.user_defs.contains(name) {
            // A bare reference to an instance-access extern becomes a
            // receiver-taking lambda (`read` → `lambda r: r.read()`, `text` →
            // `lambda r: r.text`); applied references are handled in
            // `lower_application`.
            if let Some(kind) = self.receiver_externs.get(name).copied() {
                let arity = self.arities.get(name).copied().unwrap_or(1);
                let member = self.extern_targets[name].clone();
                let kwargs = self.extern_kwargs.get(name).cloned().unwrap_or_default();
                return receiver_lambda(&member, arity, kind, kwargs);
            }
            // A bare reference to a nullary extern is a unit-taking lambda that
            // ignores its argument (`now` → `lambda *_: time.time()`); applied
            // references are handled directly in `lower_application`.
            if self.nullary_externs.contains(name) {
                let target = self.extern_path(&self.extern_targets[name].clone());
                let kwargs = self.extern_kwargs.get(name).cloned().unwrap_or_default();
                return nullary_lambda(&target, kwargs);
            }
            // A bare reference to a plain extern carrying kwargs keeps them via
            // `functools.partial` (`openText` → `functools.partial(builtins.open,
            // mode="rt", encoding="utf-8")`), or via a lambda when a `...` slot has
            // yet to be filled, so they survive later application. Applied
            // references are handled in `lower_application`.
            if let Some(kwargs) = self.extern_kwargs.get(name).cloned() {
                let target = self.extern_path(&self.extern_targets[name].clone());
                let arity = self.arities.get(name).copied();
                return self.build_call_kw_bare(dotted_path(&target), arity, kwargs);
            }
            // An `extern` reference lowers to its dotted Python target (e.g.
            // `math.sqrt`), recording any module that must be imported.
            if let Some(target) = self.extern_targets.get(name).cloned() {
                let path = self.extern_path(&target);
                return dotted_path(&path);
            }
            // Prelude functions that live in Python's `math` (not bare builtins):
            // `floor`/`ceil`/`truncate` → `math.floor`/`ceil`/`trunc`, and the
            // unit-aware roots `sqrt`/`cbrt` → `math.sqrt`/`math.cbrt` (units erase;
            // + import). `round` is a bare builtin, so it falls through to `Name`.
            let math_fn = match name {
                "floor" => Some("floor"),
                "ceil" => Some("ceil"),
                "truncate" => Some("trunc"),
                "sqrt" => Some("sqrt"),
                "cbrt" => Some("cbrt"),
                _ => None,
            };
            if let Some(py) = math_fn {
                self.needed_imports.insert("math".to_string());
                return PyExpr::Attribute {
                    value: Box::new(PyExpr::Name("math".to_string())),
                    attr: py.to_string(),
                };
            }
            // Standard combinators route to emitted `_pf_*` helpers — Python's
            // `id` is taken (returns a memory address) and the rest have no
            // builtin, so none can lower name-for-name. Their PRELUDE arities
            // feed the same partial-application path as the other builtins.
            let combinator = match name {
                "id" => Some("_pf_id"),
                "const" => Some("_pf_const"),
                "ignore" => Some("_pf_ignore"),
                "flip" => Some("_pf_flip"),
                "fst" => Some("_pf_fst"),
                "snd" => Some("_pf_snd"),
                "sign" => Some("_pf_sign"),
                _ => None,
            };
            if let Some(helper) = combinator {
                self.needed_combinators.insert(helper);
                return PyExpr::Name(helper.to_string());
            }
        }
        // A total active-pattern case constructed inside its recognizer's body
        // routes to the hidden `_Case` class (the checker rejects any use of a
        // case as a value outside its own declaration).
        if let Some(u) = self.ap_uses.get(name)
            && u.total
        {
            let class = ap_case_class(name);
            return match self.ctor_arity.get(name) {
                Some(0) => PyExpr::Call {
                    func: Box::new(PyExpr::Name(class)),
                    args: vec![],
                },
                _ => PyExpr::Name(class),
            };
        }
        // A first-class reference to a newtype constructor (`List.map UserId xs`)
        // is the identity function — the wrap is erased.
        if self.newtype_ctors.contains(name) {
            self.needed_combinators.insert("_pf_id");
            return PyExpr::Name("_pf_id".to_string());
        }
        match self.ctor_arity.get(name) {
            Some(0) => PyExpr::Call {
                func: Box::new(PyExpr::Name(py_ctor_name(name))),
                args: vec![],
            },
            Some(_) => PyExpr::Name(py_ctor_name(name)),
            None => PyExpr::Name(py_value_name(name)),
        }
    }

    /// Flag a `Decode`-module helper as needed and route a reference to it. The
    /// helper is an emitted `_pf_dec_*` function ([`decode_prelude`]).
    fn decode_helper(&mut self, helper: &'static str) -> PyExpr {
        self.needed_decode_helpers.insert(helper);
        PyExpr::Name(helper.to_string())
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
            "List.zip" => list(self, "_pf_zip"),
            "List.isEmpty" => list(self, "_pf_is_empty"),
            "List.contains" => list(self, "_pf_list_contains"),
            "List.concat" => list(self, "_pf_concat"),
            "List.sort" => list(self, "_pf_sort"),
            // `get`/`find` construct `Some`/`None`, so flag the Option prelude.
            "List.get" => {
                self.needs_option = true;
                coll(self, "_pf_list_get")
            }
            "List.find" => {
                self.needs_option = true;
                coll(self, "_pf_list_find")
            }
            // `choose` discriminates `Some`/`None_` results, so flag the Option
            // prelude (the classes must exist for the isinstance check).
            "List.choose" => {
                self.needs_option = true;
                coll(self, "_pf_choose")
            }
            "List.collect" => list(self, "_pf_collect"),
            // Access, slicing, and the rest of the sweep. Those that answer with
            // `Option` construct `Some`/`None_`, so they flag the Option prelude
            // and live with the other collection helpers.
            "List.head" => {
                self.needs_option = true;
                coll(self, "_pf_head")
            }
            "List.last" => {
                self.needs_option = true;
                coll(self, "_pf_last")
            }
            "List.tail" => {
                self.needs_option = true;
                coll(self, "_pf_tail")
            }
            "List.findIndex" => {
                self.needs_option = true;
                coll(self, "_pf_find_index")
            }
            "List.max" => {
                self.needs_option = true;
                coll(self, "_pf_max")
            }
            "List.min" => {
                self.needs_option = true;
                coll(self, "_pf_min")
            }
            "List.maxBy" => {
                self.needs_option = true;
                coll(self, "_pf_max_by")
            }
            "List.minBy" => {
                self.needs_option = true;
                coll(self, "_pf_min_by")
            }
            "List.average" => {
                self.needs_option = true;
                coll(self, "_pf_average")
            }
            "List.reduce" => {
                self.needs_option = true;
                self.needs_functools = true;
                coll(self, "_pf_reduce")
            }
            "List.take" => list(self, "_pf_take"),
            "List.drop" => list(self, "_pf_drop"),
            "List.splitAt" => list(self, "_pf_split_at"),
            "List.map2" => list(self, "_pf_map2"),
            "List.indexed" => list(self, "_pf_indexed"),
            "List.iter" => list(self, "_pf_iter"),
            "List.exists" => list(self, "_pf_exists"),
            "List.forall" => list(self, "_pf_forall"),
            "List.sortBy" => list(self, "_pf_sort_by"),
            "List.sortDescending" => list(self, "_pf_sort_desc"),
            "List.distinct" => list(self, "_pf_distinct"),
            "List.distinctBy" => list(self, "_pf_distinct_by"),
            "List.groupBy" => list(self, "_pf_group_by"),
            "List.sumBy" => list(self, "_pf_sum_by"),
            "List.partition" => list(self, "_pf_partition"),
            "List.unzip" => list(self, "_pf_unzip"),
            "List.flatten" => list(self, "_pf_flatten"),
            "List.init" => list(self, "_pf_init"),
            "List.replicate" => list(self, "_pf_replicate"),
            "List.updateAt" => list(self, "_pf_update_at"),
            "List.insertAt" => list(self, "_pf_insert_at"),
            "List.removeAt" => list(self, "_pf_remove_at"),
            "List.pairwise" => list(self, "_pf_pairwise"),
            "List.windowed" => list(self, "_pf_windowed"),
            "List.chunkBySize" => list(self, "_pf_chunk"),
            "List.takeWhile" => {
                self.needed_imports.insert("itertools".to_string());
                list(self, "_pf_take_while")
            }
            "List.dropWhile" => {
                self.needed_imports.insert("itertools".to_string());
                list(self, "_pf_drop_while")
            }
            "List.sortByDescending" => list(self, "_pf_sort_by_desc"),
            "List.countBy" => list(self, "_pf_count_by"),
            // Set
            "Set.empty" => empty("set"),
            "Set.len" => bare("len"),
            "Set.ofList" => bare("set"),
            "Set.isEmpty" => coll(self, "_pf_set_is_empty"),
            "Set.iter" => coll(self, "_pf_set_iter"),
            "Set.map" => coll(self, "_pf_set_map"),
            "Set.filter" => coll(self, "_pf_set_filter"),
            "Set.fold" => {
                self.needs_functools = true;
                list(self, "_pf_fold")
            }
            "Set.exists" => coll(self, "_pf_set_exists"),
            "Set.forall" => coll(self, "_pf_set_forall"),
            "Set.partition" => coll(self, "_pf_set_partition"),
            "Set.isSubset" => coll(self, "_pf_set_is_subset"),
            "Set.isSuperset" => coll(self, "_pf_set_is_superset"),
            "Set.max" => {
                self.needs_option = true;
                coll(self, "_pf_set_max")
            }
            "Set.min" => {
                self.needs_option = true;
                coll(self, "_pf_set_min")
            }
            "Map.isEmpty" => coll(self, "_pf_map_is_empty"),
            "Map.iter" => coll(self, "_pf_map_iter"),
            "Map.map" => coll(self, "_pf_map_map"),
            "Map.filter" => coll(self, "_pf_map_filter"),
            "Map.fold" => coll(self, "_pf_map_fold"),
            "Map.exists" => coll(self, "_pf_map_exists"),
            "Map.forall" => coll(self, "_pf_map_forall"),
            "Map.partition" => coll(self, "_pf_map_partition"),
            "Map.union" => coll(self, "_pf_map_union"),
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
            // `dict([(k, v), ...])` builds straight from a list of pair tuples.
            "Map.ofList" => bare("dict"),
            "Map.toList" => coll(self, "_pf_map_to_list"),
            // Option (helpers construct `Some`/`None`, so flag the Option prelude)
            "Option.map" => {
                self.needs_option = true;
                coll(self, "_pf_option_map")
            }
            "Option.bind" => {
                self.needs_option = true;
                coll(self, "_pf_option_bind")
            }
            "Option.filter" => {
                self.needs_option = true;
                coll(self, "_pf_option_filter")
            }
            // toResult constructs Ok/Error (and inspects Some), so flag both.
            "Option.toResult" => {
                self.needs_option = true;
                self.needs_result = true;
                coll(self, "_pf_option_to_result")
            }
            "Option.withDefault" => {
                self.needs_option = true;
                coll(self, "_pf_option_with_default")
            }
            "Option.map2" => {
                self.needs_option = true;
                coll(self, "_pf_opt_map2")
            }
            "Option.orElse" => {
                self.needs_option = true;
                coll(self, "_pf_opt_or_else")
            }
            "Option.flatten" => {
                self.needs_option = true;
                coll(self, "_pf_opt_flatten")
            }
            "Option.iter" => {
                self.needs_option = true;
                coll(self, "_pf_opt_iter")
            }
            "Option.toList" => {
                self.needs_option = true;
                coll(self, "_pf_opt_to_list")
            }
            "Option.exists" => {
                self.needs_option = true;
                coll(self, "_pf_opt_exists")
            }
            "Option.forall" => {
                self.needs_option = true;
                coll(self, "_pf_opt_forall")
            }
            "Option.contains" => {
                self.needs_option = true;
                coll(self, "_pf_opt_contains")
            }
            "Result.exists" => {
                self.needs_result = true;
                coll(self, "_pf_res_exists")
            }
            "Result.forall" => {
                self.needs_result = true;
                coll(self, "_pf_res_forall")
            }
            "Result.contains" => {
                self.needs_result = true;
                coll(self, "_pf_res_contains")
            }
            "Result.map2" => {
                self.needs_result = true;
                coll(self, "_pf_res_map2")
            }
            "Result.orElse" => {
                self.needs_result = true;
                coll(self, "_pf_res_or_else")
            }
            "Result.iter" => {
                self.needs_result = true;
                coll(self, "_pf_res_iter")
            }
            "Result.toList" => {
                self.needs_result = true;
                coll(self, "_pf_res_to_list")
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
            // The lazy half routes to Python's own lazy machinery, so a sequence
            // stays a sequence and nothing is pulled until it is consumed.
            "Seq.zip" => bare("zip"),
            // `iter([])` — an exhausted iterator; bare `iter()` is a TypeError.
            "Seq.empty" => PyExpr::Call {
                func: Box::new(PyExpr::Name("iter".to_string())),
                args: vec![PyExpr::List(vec![])],
            },
            "Seq.distinctBy" => coll(self, "_pf_seq_distinct_by"),
            "Seq.replicate" => {
                self.needed_imports.insert("itertools".to_string());
                coll(self, "_pf_seq_replicate")
            }
            "Seq.sumBy" => coll(self, "_pf_seq_sum_by"),
            "Seq.get" => {
                self.needs_option = true;
                self.needed_imports.insert("itertools".to_string());
                coll(self, "_pf_seq_get")
            }
            "Seq.last" => {
                self.needs_option = true;
                coll(self, "_pf_seq_last")
            }
            "Seq.max" => {
                self.needs_option = true;
                coll(self, "_pf_seq_max")
            }
            "Seq.min" => {
                self.needs_option = true;
                coll(self, "_pf_seq_min")
            }
            "Seq.reduce" => {
                self.needs_option = true;
                self.needs_functools = true;
                coll(self, "_pf_seq_reduce")
            }
            "Seq.indexed" => bare("enumerate"),
            "Seq.sum" => bare("sum"),
            "Seq.len" => coll(self, "_pf_seq_len"),
            "Seq.drop" => {
                self.needed_imports.insert("itertools".to_string());
                coll(self, "_pf_seq_drop")
            }
            "Seq.takeWhile" => {
                self.needed_imports.insert("itertools".to_string());
                coll(self, "_pf_seq_take_while")
            }
            "Seq.dropWhile" => {
                self.needed_imports.insert("itertools".to_string());
                coll(self, "_pf_seq_drop_while")
            }
            "Seq.concat" => {
                self.needed_imports.insert("itertools".to_string());
                coll(self, "_pf_seq_concat")
            }
            "Seq.flatten" => {
                self.needed_imports.insert("itertools".to_string());
                coll(self, "_pf_seq_flatten")
            }
            "Seq.collect" => {
                self.needed_imports.insert("itertools".to_string());
                coll(self, "_pf_seq_collect")
            }
            "Seq.pairwise" => {
                self.needed_imports.insert("itertools".to_string());
                coll(self, "_pf_seq_pairwise")
            }
            "Seq.init" => coll(self, "_pf_seq_init"),
            "Seq.initInfinite" => {
                self.needed_imports.insert("itertools".to_string());
                coll(self, "_pf_seq_init_inf")
            }
            "Seq.distinct" => coll(self, "_pf_seq_distinct"),
            "Seq.exists" => coll(self, "_pf_seq_exists"),
            "Seq.forall" => coll(self, "_pf_seq_forall"),
            "Seq.contains" => coll(self, "_pf_seq_contains"),
            "Seq.iter" => coll(self, "_pf_seq_iter"),
            // `head`/`find`/`unfold` construct `Some`/`None_`.
            "Seq.head" => {
                self.needs_option = true;
                coll(self, "_pf_seq_head")
            }
            "Seq.find" => {
                self.needs_option = true;
                coll(self, "_pf_seq_find")
            }
            "Seq.isEmpty" => {
                self.needs_option = true;
                coll(self, "_pf_seq_is_empty")
            }
            "Seq.unfold" => {
                self.needs_option = true;
                coll(self, "_pf_seq_unfold")
            }
            // String — text ops over the built-in `string` (Python `str`). Bare
            // routes reuse Python builtins (`len`/`str`/`list`); the rest lower to
            // emitted `_pf_str_*` helpers so each curried function is one callable.
            "String.len" => bare("len"),
            "String.fromInt" | "String.fromFloat" => bare("str"),
            "String.toList" => bare("list"),
            "String.concat" => coll(self, "_pf_str_concat"),
            "String.join" => coll(self, "_pf_str_join"),
            "String.split" => coll(self, "_pf_str_split"),
            "String.upper" => coll(self, "_pf_str_upper"),
            "String.lower" => coll(self, "_pf_str_lower"),
            "String.strip" => coll(self, "_pf_str_strip"),
            "String.contains" => coll(self, "_pf_str_contains"),
            "String.startsWith" => coll(self, "_pf_str_starts_with"),
            "String.endsWith" => coll(self, "_pf_str_ends_with"),
            "String.replace" => coll(self, "_pf_str_replace"),
            "String.isEmpty" => coll(self, "_pf_str_is_empty"),
            "String.repeat" => coll(self, "_pf_str_repeat"),
            "String.trimStart" => coll(self, "_pf_str_trim_start"),
            "String.trimEnd" => coll(self, "_pf_str_trim_end"),
            "String.splitLines" => coll(self, "_pf_str_split_lines"),
            "String.rev" => coll(self, "_pf_str_rev"),
            "String.ofList" => coll(self, "_pf_str_of_list"),
            // `get` answers with `Option`, like `List.get`.
            "String.get" => {
                self.needs_option = true;
                coll(self, "_pf_str_get")
            }
            "String.slice" => coll(self, "_pf_str_slice"),
            "String.tryIndexOf" => {
                self.needs_option = true;
                coll(self, "_pf_str_index_of")
            }
            // `String.toFloat` is total (guarded `float(s)`), like `toInt`.
            "String.toFloat" => {
                self.needs_option = true;
                coll(self, "_pf_str_to_float")
            }
            // `String.toInt` is total (guarded `int(s)`), so it constructs Some/None.
            "String.toInt" => {
                self.needs_option = true;
                coll(self, "_pf_str_to_int")
            }
            // Format — checked formatting, the typed alternative to `:.2f` specifiers.
            // Each lowers to a `format(x, spec)` / `str.rjust`/`ljust` helper; the spec
            // is built from the checked `int` decimal count, never a user spec string.
            "Format.fixed" => coll(self, "_pf_fmt_fixed"),
            "Format.thousands" => coll(self, "_pf_fmt_thousands"),
            "Format.percent" => coll(self, "_pf_fmt_percent"),
            "Format.currency" => coll(self, "_pf_fmt_currency"),
            "Format.grouped" => coll(self, "_pf_fmt_grouped"),
            "Format.padLeft" => coll(self, "_pf_fmt_pad_left"),
            "Format.padRight" => coll(self, "_pf_fmt_pad_right"),
            // Decode — JSON decoder combinators. Each member is (or builds) a Python
            // callable `parsed_json -> value` that raises on mismatch; `decodeString`
            // parses + runs one and catches into a `Result`. Routed to emitted
            // `_pf_dec_*` helpers (recorded so they are defined). `nullable` builds
            // `Some`/`None_`; `decodeString` builds `Ok`/`Error`/`_Exception` and
            // needs `json` — so flag the corresponding preludes/imports.
            "Decode.string" => self.decode_helper("_pf_dec_string"),
            "Decode.int" => self.decode_helper("_pf_dec_int"),
            "Decode.float" => self.decode_helper("_pf_dec_float"),
            "Decode.bool" => self.decode_helper("_pf_dec_bool"),
            "Decode.field" => self.decode_helper("_pf_dec_field"),
            "Decode.list" => self.decode_helper("_pf_dec_list"),
            "Decode.nullable" => {
                self.needs_option = true;
                self.decode_helper("_pf_dec_nullable")
            }
            "Decode.map" => self.decode_helper("_pf_dec_map"),
            "Decode.map2" => self.decode_helper("_pf_dec_map2"),
            "Decode.map3" => self.decode_helper("_pf_dec_map3"),
            "Decode.map4" => self.decode_helper("_pf_dec_map4"),
            "Decode.succeed" => self.decode_helper("_pf_dec_succeed"),
            "Decode.fail" => self.decode_helper("_pf_dec_fail"),
            "Decode.andThen" => self.decode_helper("_pf_dec_and_then"),
            "Decode.oneOf" => self.decode_helper("_pf_dec_one_of"),
            "Decode.decodeString" => {
                self.needs_result = true;
                self.needs_exception = true;
                self.needed_imports.insert("json".to_string());
                self.decode_helper("_pf_dec_decode_string")
            }
            // A user module member (`Geometry.area`). An imported *file* module
            // lowers to Python attribute access on the imported module
            // (`geometry.area`, with `import geometry` hoisted); an in-file
            // `module` declaration uses the flat mangled name (`Geometry_area`).
            other => {
                // An imported newtype constructor (`Ids.UserId`) referenced
                // first-class erases to the identity — there is no class in the
                // exporting module either.
                if self.newtype_ctors.contains(other) {
                    self.needed_combinators.insert("_pf_id");
                    return PyExpr::Name("_pf_id".to_string());
                }
                let (base, member) = other.split_once('.').unwrap_or((other, ""));
                if self.imported_modules.contains(base) {
                    let module = self.py_module_ref(base);
                    let attr = PyExpr::Attribute {
                        value: Box::new(PyExpr::Name(module)),
                        // A member may be a constructor (`Geometry.Circle`) or a
                        // value (`Geometry.set`), so mangle it exactly as its
                        // defining module did. `py_value_name` covers both: it
                        // agrees with `py_ctor_name` on `None`/`True`/`False` and
                        // leaves every other (capitalized) constructor alone.
                        attr: py_value_name(member),
                    };
                    // A nullary constructor used as a value is an instance, so call
                    // it (`palette.Red()`), matching the single-module behavior.
                    if self.imported_nullary_ctors.contains(other) {
                        PyExpr::Call {
                            func: Box::new(attr),
                            args: vec![],
                        }
                    } else {
                        attr
                    }
                } else {
                    // An in-file `module` member: the flat `Geometry_area` name its
                    // definition emitted (mangled the same way).
                    PyExpr::Name(match other.split_once('.') {
                        Some((m, member)) => format!("{m}_{}", py_value_name(member)),
                        None => py_value_name(other),
                    })
                }
            }
        }
    }

    /// The Python class name a constructor pattern matches against. A qualified
    /// constructor from an imported file module (`Geometry.Circle`) becomes dotted
    /// attribute access on the imported module (`geometry.Circle`, with `import
    /// geometry` hoisted) so it matches the *same* class the module defines; a bare
    /// constructor is just mangled away from Python keywords.
    fn ctor_class_name(&mut self, name: &str) -> String {
        if let Some((base, member)) = name.split_once('.')
            && self.imported_modules.contains(base)
        {
            let module = self.py_module_ref(base);
            format!("{module}.{}", py_ctor_name(member))
        } else {
            py_ctor_name(name)
        }
    }

    /// The Python class name for a record **tag**. A qualified tag from an imported
    /// file module (`Geometry.Point`) becomes dotted attribute access on that module
    /// (`geometry.Point`, with `import geometry` hoisted) so it references the *same*
    /// class the module defines (the consumer never redefines it); a bare tag is the
    /// record class name (mangled for the reserved `Exception`).
    fn record_class_name(&mut self, tag: &str) -> String {
        if let Some((base, rec)) = tag.split_once('.')
            && self.imported_modules.contains(base)
        {
            let module = self.py_module_ref(base);
            format!("{module}.{}", py_record_class(rec))
        } else {
            py_record_class(tag)
        }
    }

    fn lower_pattern(&mut self, pattern: &Pattern) -> PyPattern {
        match pattern {
            Pattern::Wildcard => PyPattern::Wildcard,
            Pattern::Var { name, .. } => PyPattern::Capture(py_value_name(name)),
            Pattern::Int(n) => PyPattern::Literal(PyExpr::Int(*n)),
            Pattern::Str(s) => PyPattern::Literal(PyExpr::Str(s.clone())),
            Pattern::Bool(b) => PyPattern::Literal(PyExpr::Bool(*b)),
            Pattern::Ctor { name, args, .. } => {
                // A newtype pattern erases: `case UserId s:` matches the bare
                // underlying value, so lower straight into the payload pattern.
                // (The checker guarantees exactly one argument.)
                if self.newtype_ctors.contains(name)
                    && let [payload] = args.as_slice()
                {
                    return self.lower_pattern(payload);
                }
                // Matching a built-in constructor needs its classes as much as
                // constructing one does: the emitted `case Some(x)` / `case None_()`
                // names them. Only construction sites used to flag `Option`, so a
                // module that merely *consumed* one — the producer being an import
                // or another module's stdlib call — emitted `None_` with no import.
                if name == "Ok" || name == "Error" {
                    self.needs_result = true;
                }
                if name == "Some" || name == "None" {
                    self.needs_option = true;
                }
                let mut lowered = Vec::with_capacity(args.len());
                for arg in args {
                    lowered.push(self.lower_pattern(arg));
                }
                PyPattern::Class {
                    name: self.ctor_class_name(name),
                    args: lowered,
                }
            }
            Pattern::Record { ty, fields, .. } => {
                // Records lower to a class named after the record type (the tag); the
                // field names match its attributes, so emit a keyword class pattern.
                let lowered = fields
                    .iter()
                    .map(|f| (py_field_name(&f.name), self.lower_pattern(&f.pattern)))
                    .collect();
                PyPattern::ClassKw {
                    name: self.record_class_name(ty),
                    fields: lowered,
                }
            }
            Pattern::Tuple { elems } => {
                let lowered = elems.iter().map(|e| self.lower_pattern(e)).collect();
                PyPattern::Sequence(lowered)
            }
            // `[a, b, *mid, z]` → a Python list sequence pattern (brackets). The
            // star becomes a capture name (`*mid`) or `*_` for a wildcard rest;
            // Python allows the star anywhere, so suffix elements lower 1:1 after it.
            Pattern::List {
                prefix,
                rest,
                suffix,
            } => {
                let elems = prefix.iter().map(|p| self.lower_pattern(p)).collect();
                let star = rest.as_deref().map(|r| match r {
                    Pattern::Var { name, .. } => py_value_name(name),
                    // `*_` and any other rest binder discard into a wildcard capture.
                    _ => "_".to_string(),
                });
                let suffix = suffix.iter().map(|p| self.lower_pattern(p)).collect();
                PyPattern::ListSeq {
                    elems,
                    star,
                    suffix,
                }
            }
            Pattern::Or(alts) => {
                let lowered = alts.iter().map(|p| self.lower_pattern(p)).collect();
                PyPattern::Or(lowered)
            }
            Pattern::As { pattern, name, .. } => PyPattern::As {
                pattern: Box::new(self.lower_pattern(pattern)),
                name: py_value_name(name),
            },
        }
    }

    /// Lower an optional `case` guard to a Python guard expression. A guard runs
    /// inside the arm (after the pattern binds), so it must be a pure expression —
    /// Python allows no statements in a `case … if …:` guard (`DESIGN.md` §7.2).
    fn lower_guard(
        &mut self,
        guard: &Option<Expr>,
        locals: &HashSet<String>,
    ) -> Result<Option<PyExpr>, LowerError> {
        match guard {
            None => Ok(None),
            Some(g) => {
                let (stmts, val) = self.lower_value(g, locals)?;
                if !stmts.is_empty() {
                    return Err(LowerError {
                        message: "a `case` guard must be a simple expression".to_string(),
                    });
                }
                Ok(Some(val))
            }
        }
    }

    // ----- active patterns (`DESIGN.md` §7.2) -----

    /// Does any arm of this match use an active-pattern case at its top level —
    /// bare (`case Even:`) or as an or-pattern of cases (`case Even | Odd:`)?
    fn match_uses_ap(&self, arms: &[crate::parser::ast::MatchArm]) -> bool {
        arms.iter().any(|a| match &a.pattern {
            Pattern::Ctor { name, .. } => self.ap_uses.contains_key(name),
            Pattern::Or(alts) => alts.iter().any(
                |p| matches!(p, Pattern::Ctor { name, .. } if self.ap_uses.contains_key(name)),
            ),
            _ => false,
        })
    }

    /// Whether an arm pattern is expressible as one *condition* in the AP
    /// if/elif chain: a literal, a catch-all, an active-pattern case, or an
    /// or-pattern of (binder-free, checker-enforced) active-pattern cases.
    /// Anything else is a *structural* arm, lowered as a one-armed native
    /// `match` inside the fall-through sequence (`lower_ap_match_seq`).
    fn ap_chain_supported(&self, pattern: &Pattern) -> bool {
        match pattern {
            Pattern::Wildcard
            | Pattern::Var { .. }
            | Pattern::Int(_)
            | Pattern::Str(_)
            | Pattern::Bool(_) => true,
            Pattern::Ctor { name, .. } => self.ap_uses.contains_key(name),
            Pattern::Or(alts) => alts.iter().all(
                |p| matches!(p, Pattern::Ctor { name, .. } if self.ap_uses.contains_key(name)),
            ),
            _ => false,
        }
    }

    /// Lower a `match` that uses active patterns to an honest **if/elif chain**
    /// (`DESIGN.md` §7.2) — an active pattern is a *function call*, not a
    /// structural test, so Python's `match` cannot express it. The scrutinee is
    /// evaluated once, and each **distinct** recognizer application (same
    /// function + same parameter arguments) is hoisted to a temp before the
    /// chain, so its side effects happen at most once per match. `assign_to`
    /// selects value position (arm bodies assign that temp) vs return position
    /// (arm bodies return). The flat chain handles arms that are one condition
    /// each — an active-pattern case, an or-pattern of cases, a literal, a
    /// variable, or `_`; a guard or a *structural* arm routes to the
    /// fall-through sequence (`lower_ap_match_seq`).
    fn lower_ap_match(
        &mut self,
        scrutinee: &Expr,
        arms: &[crate::parser::ast::MatchArm],
        locals: &HashSet<String>,
        assign_to: Option<&str>,
    ) -> Result<Vec<PyStmt>, LowerError> {
        let (mut stmts, subject_val) = self.lower_value(scrutinee, locals)?;
        // A bare name is reused; anything else is bound once.
        let subject = match &subject_val {
            PyExpr::Name(n) => n.clone(),
            _ => {
                let tmp = self.fresh_tmp();
                stmts.push(PyStmt::Assign {
                    target: tmp.clone(),
                    value: subject_val,
                });
                tmp
            }
        };
        // A guard can fail *after* binding names, and a *structural* arm (a
        // constructor/tuple/record/list pattern beside the AP arms) is a native
        // `match`, not a chain condition — both need the fall-through lowering
        // (a forward `if`-sequence with early exit). The guard-free, chain-only
        // shape keeps the flat if/elif chain below unchanged.
        if arms
            .iter()
            .any(|a| a.guard.is_some() || !self.ap_chain_supported(&a.pattern))
        {
            return self.lower_ap_match_seq(&subject, arms, locals, assign_to, stmts);
        }
        // (recognizer, lowered args, temp) per distinct hoisted application.
        let mut hoisted: Vec<(String, Vec<PyExpr>, String)> = Vec::new();
        // (condition, binder assigns, body) per arm; `None` = catch-all.
        let mut chain: Vec<(Option<PyExpr>, Vec<PyStmt>, Vec<PyStmt>)> = Vec::new();
        for arm in arms {
            let (cond, binds) =
                self.ap_arm_test(&arm.pattern, &subject, locals, &mut hoisted, &mut stmts)?;
            let bindings = pattern_bindings(&arm.pattern);
            let arm_locals = extend(locals, &bindings);
            // Pattern binders shadow same-named local folders (fold pass).
            let shadowed = self.shadow_local_fns(&bindings);
            let body = match assign_to {
                None => self.lower_return(&arm.body, &arm_locals)?,
                Some(tmp) => {
                    let (s, v) = self.lower_value(&arm.body, &arm_locals)?;
                    with_assign(s, tmp, v)
                }
            };
            self.unshadow_local_fns(shadowed);
            let catch_all = cond.is_none();
            chain.push((cond, binds, body));
            if catch_all {
                break; // any later arm is unreachable
            }
        }
        // Assemble the chain back-to-front: the trailing catch-all (if any)
        // becomes the final `else`; otherwise a defensive raise (the checker has
        // already proven exhaustiveness or demanded a wildcard).
        let mut else_body: Vec<PyStmt> = match chain.last() {
            Some((None, _, _)) => {
                let (_, mut binds, body) = chain.pop().expect("non-empty chain");
                binds.extend(body);
                binds
            }
            _ => vec![PyStmt::RaiseRuntimeError(
                "non-exhaustive match".to_string(),
            )],
        };
        while let Some((cond, mut binds, body)) = chain.pop() {
            let test = cond.expect("only the last chain arm can be a catch-all");
            binds.extend(body);
            else_body = vec![PyStmt::If {
                test,
                body: binds,
                orelse: else_body,
            }];
        }
        stmts.extend(else_body);
        Ok(stmts)
    }

    /// Lower a `match` that uses active patterns **and has a guard** to a forward
    /// `if`-sequence with early exit (`DESIGN.md` §7.2). Each arm computes its
    /// recognizer **lazily** — only when the arm is reached, memoized (via
    /// `hoisted`) so a repeated application runs at most once — then, on a full
    /// match (structural test *and* guard), exits: by `return` in return position,
    /// or by setting a `_done` sentinel that gates the remaining arms in value
    /// position. A failing guard binds nothing durable and falls through.
    fn lower_ap_match_seq(
        &mut self,
        subject: &str,
        arms: &[crate::parser::ast::MatchArm],
        locals: &HashSet<String>,
        assign_to: Option<&str>,
        mut stmts: Vec<PyStmt>,
    ) -> Result<Vec<PyStmt>, LowerError> {
        let mut hoisted: Vec<(String, Vec<PyExpr>, String)> = Vec::new();
        // Value position has no `return`, so a sentinel stops later arms once one
        // has matched; return position needs none.
        let done = match assign_to {
            Some(_) => {
                let d = self.fresh_tmp();
                stmts.push(PyStmt::Assign {
                    target: d.clone(),
                    value: PyExpr::Bool(false),
                });
                Some(d)
            }
            None => None,
        };
        for arm in arms {
            // A structural arm (constructor/tuple/record/list pattern) is a
            // one-armed native `match`: the pattern tests and binds Python-side,
            // a failing case (or guard) falls out of the `match` and on to the
            // next arm. No recognizer is involved.
            if !self.ap_chain_supported(&arm.pattern) {
                let bindings = pattern_bindings(&arm.pattern);
                let arm_locals = extend(locals, &bindings);
                // Pattern binders shadow same-named local folders (fold pass).
                let shadowed = self.shadow_local_fns(&bindings);
                let guard = self.lower_guard(&arm.guard, &arm_locals)?;
                let body: Vec<PyStmt> = match (assign_to, &done) {
                    (None, _) => self.lower_return(&arm.body, &arm_locals)?,
                    (Some(tmp), Some(d)) => {
                        let (s, v) = self.lower_value(&arm.body, &arm_locals)?;
                        let mut b = with_assign(s, tmp, v);
                        b.push(PyStmt::Assign {
                            target: d.clone(),
                            value: PyExpr::Bool(true),
                        });
                        b
                    }
                    (Some(_), None) => {
                        unreachable!("value position always allocates a sentinel")
                    }
                };
                self.unshadow_local_fns(shadowed);
                let stmt = PyStmt::Match {
                    subject: PyExpr::Name(subject.to_string()),
                    cases: vec![PyCase {
                        pattern: self.lower_pattern(&arm.pattern),
                        guard,
                        body,
                    }],
                };
                match &done {
                    Some(d) => stmts.push(PyStmt::If {
                        test: PyExpr::Not(Box::new(PyExpr::Name(d.clone()))),
                        body: vec![stmt],
                        orelse: vec![],
                    }),
                    None => stmts.push(stmt),
                }
                continue;
            }
            // The recognizer application is hoisted into this arm's own block, so
            // it runs only when the arm is reached (lazy).
            let mut arm_block: Vec<PyStmt> = Vec::new();
            let (cond, binds) =
                self.ap_arm_test(&arm.pattern, subject, locals, &mut hoisted, &mut arm_block)?;
            let bindings = pattern_bindings(&arm.pattern);
            let arm_locals = extend(locals, &bindings);
            // Pattern binders shadow same-named local folders (fold pass).
            let shadowed = self.shadow_local_fns(&bindings);
            let guard = self.lower_guard(&arm.guard, &arm_locals)?;
            // The arm body: return it, or assign the temp and mark the sentinel.
            let body: Vec<PyStmt> = match (assign_to, &done) {
                (None, _) => self.lower_return(&arm.body, &arm_locals)?,
                (Some(tmp), Some(d)) => {
                    let (s, v) = self.lower_value(&arm.body, &arm_locals)?;
                    let mut b = with_assign(s, tmp, v);
                    b.push(PyStmt::Assign {
                        target: d.clone(),
                        value: PyExpr::Bool(true),
                    });
                    b
                }
                (Some(_), None) => unreachable!("value position always allocates a sentinel"),
            };
            self.unshadow_local_fns(shadowed);
            // binds, then the (optionally guarded) body.
            let mut inner = binds;
            match guard {
                Some(g) => inner.push(PyStmt::If {
                    test: g,
                    body,
                    orelse: vec![],
                }),
                None => inner.extend(body),
            }
            // Gate by the structural condition (a catch-all arm has none).
            let unconditional = cond.is_none() && arm.guard.is_none();
            match cond {
                Some(c) => arm_block.push(PyStmt::If {
                    test: c,
                    body: inner,
                    orelse: vec![],
                }),
                None => arm_block.extend(inner),
            }
            // In value position, run the arm only while still unmatched.
            match &done {
                Some(d) => stmts.push(PyStmt::If {
                    test: PyExpr::Not(Box::new(PyExpr::Name(d.clone()))),
                    body: arm_block,
                    orelse: vec![],
                }),
                None => stmts.extend(arm_block),
            }
            // A guard-free catch-all matches unconditionally; in return position it
            // returns, making any later arm (and a trailing raise) dead code.
            if unconditional && assign_to.is_none() {
                return Ok(stmts);
            }
        }
        // Return position without an unconditional catch-all: exhaustiveness is
        // proven (or a wildcard was demanded), so this is defensive.
        if assign_to.is_none() {
            stmts.push(PyStmt::RaiseRuntimeError(
                "non-exhaustive match".to_string(),
            ));
        }
        Ok(stmts)
    }

    /// The chain condition and binder assignments for one arm of an
    /// active-pattern match. A `None` condition marks a catch-all arm
    /// (wildcard / variable — it becomes the chain's `else`). New recognizer
    /// applications are hoisted into `out` and remembered in `hoisted`.
    fn ap_arm_test(
        &mut self,
        pattern: &Pattern,
        subject: &str,
        locals: &HashSet<String>,
        hoisted: &mut Vec<(String, Vec<PyExpr>, String)>,
        out: &mut Vec<PyStmt>,
    ) -> Result<(Option<PyExpr>, Vec<PyStmt>), LowerError> {
        let subj = || PyExpr::Name(subject.to_string());
        let eq_lit = |lit: PyExpr| PyExpr::BinOp {
            op: PyBinOp::Eq,
            left: Box::new(subj()),
            right: Box::new(lit),
        };
        match pattern {
            Pattern::Wildcard => Ok((None, vec![])),
            Pattern::Var { name, .. } => Ok((
                None,
                vec![PyStmt::Assign {
                    target: py_value_name(name),
                    value: subj(),
                }],
            )),
            Pattern::Int(n) => Ok((Some(eq_lit(PyExpr::Int(*n))), vec![])),
            Pattern::Str(s) => Ok((Some(eq_lit(PyExpr::Str(s.clone()))), vec![])),
            Pattern::Bool(b) => Ok((Some(eq_lit(PyExpr::Bool(*b))), vec![])),
            Pattern::Ctor { name, args, .. } if self.ap_uses.contains_key(name) => {
                let u = self.ap_uses[name].clone();
                // The recognizer call: leading parameter arguments (literals /
                // variables — checker-enforced), then the scrutinee.
                let mut call_args = Vec::with_capacity(u.extra + 1);
                for a in &args[..u.extra] {
                    call_args.push(self.ap_arg_pyexpr(a, locals)?);
                }
                call_args.push(subj());
                let tmp = match hoisted
                    .iter()
                    .find(|(f, a, _)| *f == u.py_fn && *a == call_args)
                {
                    Some((_, _, t)) => t.clone(),
                    None => {
                        let t = self.fresh_tmp();
                        out.push(PyStmt::Assign {
                            target: t.clone(),
                            value: PyExpr::Call {
                                func: Box::new(PyExpr::Name(u.py_fn.clone())),
                                args: call_args.clone(),
                            },
                        });
                        hoisted.push((u.py_fn.clone(), call_args, t.clone()));
                        t
                    }
                };
                let binders = &args[u.extra..];
                let isinstance = |class: String| PyExpr::Call {
                    func: Box::new(PyExpr::Name("isinstance".to_string())),
                    args: vec![PyExpr::Name(tmp.clone()), PyExpr::Name(class)],
                };
                let bind_attr = |target: &str, attr: String| PyStmt::Assign {
                    target: target.to_string(),
                    value: PyExpr::Attribute {
                        value: Box::new(PyExpr::Name(tmp.clone())),
                        attr,
                    },
                };
                if u.total {
                    // A hidden-ADT case: isinstance test + field binds.
                    let binds = binders
                        .iter()
                        .enumerate()
                        .filter_map(|(i, p)| match p {
                            Pattern::Var { name, .. } => Some(bind_attr(name, format!("_{i}"))),
                            _ => None,
                        })
                        .collect();
                    Ok((Some(isinstance(ap_case_class(name))), binds))
                } else if binders.len() == 1 {
                    // Option-flavored partial: test `Some`, bind the payload.
                    self.needs_option = true;
                    let binds = match &binders[0] {
                        Pattern::Var { name, .. } => vec![bind_attr(name, "_0".to_string())],
                        _ => vec![],
                    };
                    Ok((Some(isinstance("Some".to_string())), binds))
                } else {
                    // Bool-flavored partial: the recognizer's result *is* the test.
                    Ok((Some(PyExpr::Name(tmp)), vec![]))
                }
            }
            // An or-pattern of binder-free active-pattern cases: the disjunction
            // of the alternatives' tests (the checker enforces binder-freedom,
            // so no alternative contributes binds). A shared recognizer is still
            // hoisted once — the memo in `hoisted` recognizes the repeat.
            Pattern::Or(alts) => {
                let mut cond: Option<PyExpr> = None;
                for alt in alts {
                    let (c, binds) = self.ap_arm_test(alt, subject, locals, hoisted, out)?;
                    let c = c.ok_or_else(|| LowerError {
                        message: "an or-pattern alternative in an active-pattern match \
                                  must be a testable case"
                            .to_string(),
                    })?;
                    debug_assert!(binds.is_empty(), "or-alternatives are binder-free");
                    cond = Some(match cond {
                        None => c,
                        Some(prev) => PyExpr::BinOp {
                            op: PyBinOp::Or,
                            left: Box::new(prev),
                            right: Box::new(c),
                        },
                    });
                }
                Ok((cond, vec![]))
            }
            _ => Err(LowerError {
                message: "unsupported pattern in a match using active patterns".to_string(),
            }),
        }
    }

    /// Lower an active-pattern *parameter argument* (the `3` in
    /// `case DivisibleBy 3:`) — a literal or a variable reference.
    fn ap_arg_pyexpr(
        &mut self,
        pat: &Pattern,
        locals: &HashSet<String>,
    ) -> Result<PyExpr, LowerError> {
        match pat {
            Pattern::Int(n) => Ok(PyExpr::Int(*n)),
            Pattern::Str(s) => Ok(PyExpr::Str(s.clone())),
            Pattern::Bool(b) => Ok(PyExpr::Bool(*b)),
            Pattern::Var { name, .. } => Ok(self.lower_var(name, locals)),
            _ => Err(LowerError {
                message: "an active-pattern parameter argument must be a literal or a variable"
                    .to_string(),
            }),
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
                        target: py_value_name(name),
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
                    target: py_value_name(name),
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
                s.push(self.result_bind_match(
                    v,
                    PyPattern::Capture(py_value_name(name)),
                    rest_stmts,
                ));
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
                    guard: None,
                    body: rest,
                },
                PyCase {
                    pattern: PyPattern::Class {
                        name: "Error".to_string(),
                        args: vec![PyPattern::Capture(e_tmp.clone())],
                    },
                    guard: None,
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
                        target: py_value_name(name),
                        value: PyExpr::Await(Box::new(v)),
                    });
                    locals.insert(name.clone());
                }
                CeItem::Let { name, value, .. } => {
                    let (mut s, v) = self.lower_value(value, &locals)?;
                    body.append(&mut s);
                    body.push(PyStmt::Assign {
                        target: py_value_name(name),
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

    /// Like [`Self::build_call`] but for an `extern` whose target carries Python
    /// keyword arguments. The `spec` rides along at every arity, so nothing is ever
    /// dropped: full/over-application emits the direct call (`f(a, kw=v)`), and
    /// under-application either hands the pinned literals to `functools.partial`
    /// (`functools.partial(f, a, kw=v)`) or, when a `...` slot is still unfilled,
    /// closes over a lambda that takes the remaining arguments.
    ///
    /// Any statements needed to keep the already-supplied arguments evaluating at
    /// application time (rather than inside a lambda body) are pushed onto `hoist`.
    fn build_call_kw(
        &mut self,
        head: PyExpr,
        arity: Option<usize>,
        args: Vec<PyExpr>,
        spec: Vec<(String, KwSource)>,
        hoist: &mut Vec<PyStmt>,
    ) -> PyExpr {
        let n = args.len();
        let slots = slot_count(&spec);
        // An unknown arity is treated as n-ary, but it still has to leave room for
        // the slots, so a bare reference to a slot extern becomes a lambda.
        let k = arity.unwrap_or(n.max(slots));
        if n < k {
            if slots == 0 {
                // Partial application: `functools.partial` carries the positional
                // args *and* the pinned keyword args.
                self.needs_functools = true;
                let mut partial_args = Vec::with_capacity(n + 1);
                partial_args.push(head);
                partial_args.extend(args);
                let (_, kwargs) = bind_kwargs(&spec, Vec::new());
                return PyExpr::CallKw {
                    func: Box::new(PyExpr::Attribute {
                        value: Box::new(PyExpr::Name("functools".to_string())),
                        attr: "partial".to_string(),
                    }),
                    args: partial_args,
                    kwargs,
                };
            }
            // `functools.partial` cannot carry a keyword whose value has not arrived,
            // so a slot extern's partial application is a lambda over the missing
            // arguments. Bind what was supplied first, so it evaluates now — exactly
            // when `functools.partial` would have evaluated it.
            let bound: Vec<PyExpr> = args.into_iter().map(|a| self.hoist_tmp(a, hoist)).collect();
            let params: Vec<String> = (0..k - n).map(|i| format!("_pf_k{i}")).collect();
            let mut all = bound;
            all.extend(params.iter().cloned().map(PyExpr::Name));
            let (positional, kwargs) = bind_kwargs(&spec, all);
            return PyExpr::Lambda {
                params,
                body: Box::new(PyExpr::CallKw {
                    func: Box::new(head),
                    args: positional,
                    kwargs,
                }),
            };
        }
        if n > k {
            // Over-application: full (kw-carrying) call, then apply the rest.
            let mut rest = args;
            let first: Vec<PyExpr> = rest.drain(..k).collect();
            let (positional, kwargs) = bind_kwargs(&spec, first);
            let mut call = PyExpr::CallKw {
                func: Box::new(head),
                args: positional,
                kwargs,
            };
            for extra in rest {
                call = PyExpr::Call {
                    func: Box::new(call),
                    args: vec![extra],
                };
            }
            return call;
        }
        let (positional, kwargs) = bind_kwargs(&spec, args);
        PyExpr::CallKw {
            func: Box::new(head),
            args: positional,
            kwargs,
        }
    }

    /// [`Self::build_call_kw`] for a reference that supplies no arguments, and so
    /// has nothing to hoist.
    fn build_call_kw_bare(
        &mut self,
        head: PyExpr,
        arity: Option<usize>,
        spec: Vec<(String, KwSource)>,
    ) -> PyExpr {
        let mut hoist = Vec::new();
        let call = self.build_call_kw(head, arity, Vec::new(), spec, &mut hoist);
        debug_assert!(
            hoist.is_empty(),
            "a bare reference supplies no arguments to hoist"
        );
        call
    }

    /// Bind `value` to a fresh temporary (pushed onto `hoist`) so that placing it
    /// inside a lambda body does not defer its evaluation. A literal is already
    /// stable and is returned unchanged.
    fn hoist_tmp(&mut self, value: PyExpr, hoist: &mut Vec<PyStmt>) -> PyExpr {
        if matches!(
            value,
            PyExpr::Str(_) | PyExpr::Int(_) | PyExpr::Float(_) | PyExpr::Bool(_)
        ) {
            return value;
        }
        let tmp = self.fresh_tmp();
        hoist.push(PyStmt::Assign {
            target: tmp.clone(),
            value,
        });
        PyExpr::Name(tmp)
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
        ExprKind::Pipe { lhs, rhs, backward } => {
            // `lhs |> rhs` == `rhs lhs`; `lhs <| rhs` == `lhs rhs`. Flatten the
            // callee spine, then push the argument.
            let (callee, arg) = if *backward { (lhs, rhs) } else { (rhs, lhs) };
            let head = flatten_app(callee, args);
            args.push(arg);
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
        BinOp::Mod => PyBinOp::Mod,
        BinOp::Pow => PyBinOp::Pow,
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
/// used (as for the prelude) to decide full vs partial application. Public so the
/// project driver can export an imported extern's arity for cross-module currying.
pub fn arrow_arity(ty: &TypeExpr) -> usize {
    match ty {
        TypeExpr::Fun(_, ret, _) => 1 + arrow_arity(ret),
        TypeExpr::Con(..) | TypeExpr::Tuple(_) => 0,
    }
}

/// Whether an `extern`'s first parameter is `unit` — a *nullary* Python callable
/// (`unit -> a`, e.g. `time.time`). Applying it to `()` must emit a zero-argument
/// Python call (`time.time()`), not pass `None` (`time.time(None)`).
fn is_unit_domain(ty: &TypeExpr) -> bool {
    matches!(ty, TypeExpr::Fun(domain, _, _)
        if matches!(domain.as_ref(),
            TypeExpr::Con(name, _, args) if name == "unit" && args.is_empty()))
}

/// Python builtin *type* names — available without an `import`, so a dotted extern
/// target rooted at one (`bytes.decode`, `int.from_bytes`) must not emit an import.
const PY_BUILTIN_TYPES: &[&str] = &[
    "bool",
    "int",
    "float",
    "complex",
    "str",
    "bytes",
    "bytearray",
    "memoryview",
    "list",
    "tuple",
    "dict",
    "set",
    "frozenset",
    "range",
    "slice",
    "object",
    "type",
];

/// Lower a pinned `extern` keyword-argument literal to its Python IR expression.
/// A negative int/float is emitted as a `Neg` of the magnitude, matching how the
/// emitter renders unary minus (`compresslevel=-1`).
fn lower_extern_arg(arg: &ExternArg) -> KwSource {
    match arg {
        ExternArg::Str(s) => KwSource::Lit(PyExpr::Str(s.clone())),
        ExternArg::Int(n) if *n < 0 => KwSource::Lit(PyExpr::Neg(Box::new(PyExpr::Int(-n)))),
        ExternArg::Int(n) => KwSource::Lit(PyExpr::Int(*n)),
        ExternArg::Float(f) if *f < 0.0 => KwSource::Lit(PyExpr::Neg(Box::new(PyExpr::Float(-f)))),
        ExternArg::Float(f) => KwSource::Lit(PyExpr::Float(*f)),
        ExternArg::Bool(b) => KwSource::Lit(PyExpr::Bool(*b)),
        ExternArg::Slot => KwSource::Slot,
    }
}

/// Where an `extern` target's keyword argument gets its value: a literal pinned at
/// the declaration, or a `...` slot filled from the call's arguments
/// (`DESIGN.md` §6).
#[derive(Debug, Clone, PartialEq)]
enum KwSource {
    Lit(PyExpr),
    Slot,
}

/// How many of an extern's arguments its `...` slots claim.
fn slot_count(spec: &[(String, KwSource)]) -> usize {
    spec.iter().filter(|(_, v)| *v == KwSource::Slot).count()
}

/// Bind a keyword spec to concrete values: the leading `args` fill the positional
/// parameters and the rest fill the `...` slots in written order. Returns the
/// positional arguments and the resolved `kw=value` pairs.
///
/// `args` must hold exactly the positional count plus the slot count; callers
/// arrange that (by padding with lambda parameters when under-applied).
fn bind_kwargs(
    spec: &[(String, KwSource)],
    args: Vec<PyExpr>,
) -> (Vec<PyExpr>, Vec<(String, PyExpr)>) {
    let positional = args.len() - slot_count(spec);
    let mut rest = args;
    let leading: Vec<PyExpr> = rest.drain(..positional).collect();
    let mut fills = rest.into_iter();
    let kwargs = spec
        .iter()
        .map(|(k, v)| {
            let value = match v {
                KwSource::Lit(e) => e.clone(),
                KwSource::Slot => fills.next().expect("a fill per slot"),
            };
            (k.clone(), value)
        })
        .collect();
    (leading, kwargs)
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

/// `base.seg1.seg2…` — attribute access down a path (empty path returns `base`).
fn attr_path(base: PyExpr, segs: &[String]) -> PyExpr {
    segs.iter().fold(base, |value, seg| PyExpr::Attribute {
        value: Box::new(value),
        attr: seg.clone(),
    })
}

/// A receiver-taking lambda for a bare reference to an instance-access extern of
/// the given `arity` (counting the receiver). A property reads the attribute
/// (`lambda r: r.text`); a method calls it (`lambda r: r.read()`, or
/// `lambda r, a: r.method(a)` when it takes arguments). The method lambda is n-ary
/// in its trailing parameters, matching Pyfun's collapse of full application to a
/// direct call. Any pinned `kwargs` are appended to the method call (`lambda r, a:
/// r.write_text(a, encoding="utf-8")`); a property takes no call, so it has none.
fn receiver_lambda(
    member: &[String],
    arity: usize,
    kind: Receiver,
    spec: Vec<(String, KwSource)>,
) -> PyExpr {
    let recv = "_pf_recv".to_string();
    let accessed = attr_path(PyExpr::Name(recv.clone()), member);
    if kind == Receiver::Property {
        return PyExpr::Lambda {
            params: vec![recv],
            body: Box::new(accessed),
        };
    }
    // The lambda takes every argument after the receiver; a `...` slot claims one of
    // them and lands as a keyword instead of a positional.
    let args: Vec<String> = (1..arity.max(1)).map(|i| format!("_pf_a{i}")).collect();
    let call_args: Vec<PyExpr> = args.iter().cloned().map(PyExpr::Name).collect();
    let body = if spec.is_empty() {
        PyExpr::Call {
            func: Box::new(accessed),
            args: call_args,
        }
    } else {
        let (positional, kwargs) = bind_kwargs(&spec, call_args);
        PyExpr::CallKw {
            func: Box::new(accessed),
            args: positional,
            kwargs,
        }
    };
    let mut params = vec![recv];
    params.extend(args);
    PyExpr::Lambda {
        params,
        body: Box::new(body),
    }
}

/// A lambda for a bare reference to a nullary extern: `lambda *_: time.time()`. The
/// `*_` swallows the unit argument Pyfun passes at a `unit -> a` call site, so the
/// value works however it is later applied. Any pinned `kwargs` are appended (a
/// nullary extern has no argument to spare, so the parser rejects `...` on one).
fn nullary_lambda(target: &[String], spec: Vec<(String, KwSource)>) -> PyExpr {
    let body = if spec.is_empty() {
        PyExpr::Call {
            func: Box::new(dotted_path(target)),
            args: vec![],
        }
    } else {
        let (_, kwargs) = bind_kwargs(&spec, Vec::new());
        PyExpr::CallKw {
            func: Box::new(dotted_path(target)),
            args: vec![],
            kwargs,
        }
    };
    PyExpr::Lambda {
        params: vec!["*_pf_a".to_string()],
        body: Box::new(body),
    }
}

/// The Python module a referenced `extern` target must import, or `None` for a
/// bare builtin (`str`, a single segment — nothing to import).
///
/// The dotted target mixes a module path and an attribute path
/// (`urllib.request.urlopen` is module `urllib.request` + attr `urlopen`;
/// `sqlite3.Connection.execute` is module `sqlite3` + attrs `Connection.execute`),
/// and only the shape tells them apart. We follow PEP 8: packages/modules are
/// lowercase, classes are capitalized. So the module to import is the **maximal
/// leading run of lowercase-initial segments** among everything before the final
/// referenced name — but always at least the top-level package. This imports
/// `urllib.request` (submodule) yet stops at `sqlite3` before the `Connection`
/// class. The one shape it can't see through is a *lowercase attribute* that is a
/// value or class rather than a submodule (`sys.stdout.write`,
/// `datetime.datetime.now`) — declare those with an explicit `extern import`
/// (consulted first, in [`Lowerer::extern_import_spec`]) instead of relying on
/// this heuristic (`DESIGN.md` §6).
///
/// A target rooted at a builtin *type* (`bytes.decode`, `int.from_bytes`) imports
/// nothing — those names are always in scope.
fn extern_import(target: &[String]) -> Option<String> {
    if target.len() < 2 || PY_BUILTIN_TYPES.contains(&target[0].as_str()) {
        return None;
    }
    let prefix = &target[..target.len() - 1];
    let lower_run = prefix
        .iter()
        .take_while(|seg| seg.chars().next().is_some_and(char::is_lowercase))
        .count()
        .max(1); // always import at least the top-level package
    Some(prefix[..lower_run].join("."))
}

/// The deterministic Python name of an active pattern's recognizer function:
/// `_ap_` + its case names joined by `_` (`_ap_Even_Odd`, `_ap_Prime`). Case
/// names are globally unique across constructors and active patterns, so two
/// recognizers can never collide.
fn ap_py_fn(decl: &ActivePatternDecl) -> String {
    let names: Vec<&str> = decl.cases.iter().map(|c| c.name.as_str()).collect();
    format!("_ap_{}", names.join("_"))
}

/// The hidden Python class of a total active-pattern case (`Even` → `_Even`) —
/// underscore-prefixed to signal it is compiler-generated, and to keep it out of
/// the user constructor namespace.
fn ap_case_class(case: &str) -> String {
    format!("_{case}")
}

/// Mangle a constructor name to a valid, non-keyword Python identifier.
fn py_ctor_name(name: &str) -> String {
    if matches!(name, "None" | "True" | "False") {
        format!("{name}_")
    } else {
        name.to_string()
    }
}

/// Python's reserved words (plus the soft keywords that are reserved in the
/// positions Pyfun emits into). A user binding named after one of these cannot be
/// emitted verbatim at all — `lambda = 1` is a `SyntaxError`, not a shadowing.
const PY_KEYWORDS: &[&str] = &[
    "False", "None", "True", "and", "as", "assert", "async", "await", "break", "class", "continue",
    "def", "del", "elif", "else", "except", "finally", "for", "from", "global", "if", "import",
    "in", "is", "lambda", "nonlocal", "not", "or", "pass", "raise", "return", "try", "while",
    "with", "yield",
];

/// Python builtins **this emitter calls** in code that runs after user bindings
/// are in scope: prelude helper bodies (`_pf_map` calls `list`/`map`) and inline
/// stdlib expansions (`Set.ofList` → `set([…])`). A user binding claiming one of
/// these names shadows it at module scope, so the emitted call finds the user's
/// value instead of the builtin — `'int' object is not callable`, far from the
/// binding that caused it.
///
/// Deliberately *only* the names that are emitted with no Pyfun name of their own.
/// `print`/`abs`/`min`/`max`/`round` are absent on purpose: those lower name-for-
/// name from an identically spelled Pyfun prelude binding, so a user binding of
/// that name shadows the Pyfun name too and the checker settles it. `id` is absent
/// because Pyfun's `id` lowers to `_pf_id` (Python's `id` returns an address), and
/// `let id x = x` is common enough that mangling it would be a visible tax for no
/// collision.
const PY_EMITTED_BUILTINS: &[&str] = &[
    "filter",
    "isinstance",
    "iter",
    "len",
    "map",
    "next",
    "reversed",
    "sorted",
    "sum",
    "zip",
];

/// The emitted Python name for a user **value** binding or reference: the name as
/// written, unless Python's own namespace claims it, in which case it gains a
/// trailing underscore (`set` → `set_`, `lambda` → `lambda_`) — the same dodge
/// [`py_ctor_name`] applies to `None`, and PEP 8's own convention.
///
/// A pure function of the name, so a definition and every reference to it — in
/// this module or across a module boundary — mangle identically without any
/// coordination. `_pf`-prefixed names are the emitter's own namespace and cannot
/// be written in Pyfun source, so they need no protection here.
fn py_value_name(name: &str) -> String {
    // The builtin *types* join the list because an `extern` may be rooted at one
    // (`bytes.decode`, `int.from_bytes`) — and because the emitter names several
    // of them directly (`set([…])`, `dict(…)`, `list(…)`).
    if PY_KEYWORDS.contains(&name)
        || PY_EMITTED_BUILTINS.contains(&name)
        || PY_BUILTIN_TYPES.contains(&name)
    {
        format!("{name}_")
    } else {
        name.to_string()
    }
}

/// The emitted Python name for a **record field**. An attribute lives in its
/// object's namespace, so it cannot collide with a builtin (`q.set` is fine) —
/// only Python's keywords are unusable (`p.lambda` does not parse).
fn py_field_name(name: &str) -> String {
    if PY_KEYWORDS.contains(&name) {
        format!("{name}_")
    } else {
        name.to_string()
    }
}

/// The Python class name for a record type. Almost always the type name verbatim,
/// but the reserved built-in `Exception` is emitted as `_Exception` so it does not
/// shadow Python's builtin `Exception` (which `try` catches with `except Exception`).
fn py_record_class(name: &str) -> String {
    if name == "Exception" {
        "_Exception".to_string()
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

/// The Python type annotation for a record/variant field, for its emitted `@dataclass`
/// field. A concrete builtin maps to its Python type (`int`/`float`/`str`/`list`/…);
/// anything else — a type variable, a user ADT/record, `Option`/`Result`, a function —
/// maps to `object`. The annotation only lets the dataclass recognize the field (the
/// value is erased), and mapping a user type *name* here would risk a forward reference.
fn py_annotation(ty: &crate::parser::ast::TypeExpr) -> String {
    use crate::parser::ast::TypeExpr;
    match ty {
        TypeExpr::Con(name, _, args) => match (name.as_str(), args.len()) {
            ("int", 0) => "int",
            ("float", 0) => "float",
            ("bool", 0) => "bool",
            ("string", 0) => "str",
            ("List", 1) => "list",
            ("Set", 1) => "set",
            ("Map", 2) => "dict",
            _ => "object",
        }
        .to_string(),
        TypeExpr::Tuple(_) => "tuple".to_string(),
        TypeExpr::Fun(..) => "object".to_string(),
    }
}

/// The `Ok`/`Error` classes backing the `result` computation expression.
fn result_prelude(ordered: bool) -> Vec<PyStmt> {
    // Ordered `Ok < Error` (Ok is variant 0) when the program compares `Result` values
    // (§7.1); otherwise the comparison methods are omitted like any never-sorted type.
    vec![
        PyStmt::ClassDef {
            name: "Ok".to_string(),
            fields: vec!["_0".to_string()],
            field_types: vec!["object".to_string()],
            order: ordered.then_some(0),
            record: false,
        },
        PyStmt::ClassDef {
            name: "Error".to_string(),
            fields: vec!["_0".to_string()],
            field_types: vec!["object".to_string()],
            order: ordered.then_some(1),
            record: false,
        },
    ]
}

/// The `_Exception` record class — the `Error` payload of a `try` (`DESIGN.md` §6).
/// Emitted as `_Exception` (not `Exception`) so it does not shadow Python's builtin
/// `Exception`, which `try` lowering catches with `except Exception`. Its structural
/// `__eq__`/`__repr__`/`__match_args__` come from `emit_class`, like any record.
fn exception_prelude() -> Vec<PyStmt> {
    vec![PyStmt::ClassDef {
        name: "_Exception".to_string(),
        fields: vec!["errorKind".to_string(), "errorMessage".to_string()],
        field_types: vec!["str".to_string(), "str".to_string()],
        order: None,
        record: false,
    }]
}

/// The `Some`/`None_` classes backing the built-in `Option` type (`None` is mangled
/// to dodge the Python keyword). Structural `__eq__`/`__repr__`/`__match_args__` come
/// from `emit_class`, like any data constructor.
fn option_prelude(ordered: bool) -> Vec<PyStmt> {
    // Ordered `None < Some` (None is variant 0) when the program compares `Option`
    // values (§7.1); otherwise the comparison methods are omitted.
    vec![
        PyStmt::ClassDef {
            name: "Some".to_string(),
            fields: vec!["_0".to_string()],
            field_types: vec!["object".to_string()],
            order: ordered.then_some(1),
            record: false,
        },
        PyStmt::ClassDef {
            name: "None_".to_string(),
            fields: vec![],
            field_types: vec![],
            order: ordered.then_some(0),
            record: false,
        },
    ]
}

/// The list-prelude helper definitions actually referenced (`DESIGN.md` §6). Each
/// keeps eager-list semantics that Python's lazy `map`/`filter` would not: they
/// force results into a `list`. `_pf_fold` reuses `functools.reduce` with an
/// initial accumulator (a *total* left fold). Built from the IR (no string
/// splicing); emitted in the helper-name's sorted order for deterministic output.
/// The standard-combinator helper definitions actually referenced
/// (`id`/`const`/`ignore`/`flip`, `DESIGN.md` §6). Each lowers to a tiny `_pf_*`
/// wrapper because none can lower to a bare Python name — `id` is taken (it
/// returns a memory address) and the rest have no builtin. `flip` calls its
/// function argument n-ary (`f(y, x)`), exactly as a hand-written `let flip f x y
/// = f y x` compiles, so it is neither more nor less capable than that definition.
fn combinator_prelude(used: &BTreeSet<&'static str>) -> Vec<PyStmt> {
    let name = |n: &str| PyExpr::Name(n.to_string());
    let def = |fn_name: &str, params: &[&str], ret: PyExpr| PyStmt::FuncDef {
        name: fn_name.to_string(),
        params: params.iter().map(|p| p.to_string()).collect(),
        body: vec![PyStmt::Return(ret)],
        is_async: false,
    };
    used.iter()
        .map(|&helper| match helper {
            // _pf_id(x) -> x
            "_pf_id" => def("_pf_id", &["x"], name("x")),
            // _pf_const(x, y) -> x
            "_pf_const" => def("_pf_const", &["x", "y"], name("x")),
            // _pf_ignore(x) -> None
            "_pf_ignore" => def("_pf_ignore", &["x"], PyExpr::NoneLit),
            // _pf_flip(f, x, y) -> f(y, x)
            "_pf_flip" => def(
                "_pf_flip",
                &["f", "x", "y"],
                PyExpr::Call {
                    func: Box::new(name("f")),
                    args: vec![name("y"), name("x")],
                },
            ),
            // _pf_fst(p) -> p[0]   /   _pf_snd(p) -> p[1]
            "_pf_fst" => def(
                "_pf_fst",
                &["p"],
                PyExpr::Subscript {
                    value: Box::new(name("p")),
                    index: Box::new(PyExpr::Int(0)),
                },
            ),
            "_pf_snd" => def(
                "_pf_snd",
                &["p"],
                PyExpr::Subscript {
                    value: Box::new(name("p")),
                    index: Box::new(PyExpr::Int(1)),
                },
            ),
            // _pf_sign(x) -> (x > 0) - (x < 0)   (Python bools are ints, so this is
            // -1, 0 or 1 without a branch)
            "_pf_sign" => def(
                "_pf_sign",
                &["x"],
                PyExpr::BinOp {
                    op: PyBinOp::Sub,
                    left: Box::new(PyExpr::Compare {
                        left: Box::new(name("x")),
                        ops: vec![PyBinOp::Gt],
                        comparators: vec![PyExpr::Int(0)],
                    }),
                    right: Box::new(PyExpr::Compare {
                        left: Box::new(name("x")),
                        ops: vec![PyBinOp::Lt],
                        comparators: vec![PyExpr::Int(0)],
                    }),
                },
            ),
            other => unreachable!("unknown combinator helper {other}"),
        })
        .collect()
}

/// The `Decode`-module helper definitions actually referenced (`DESIGN.md` §6). A
/// `Decoder a` is represented at runtime as a plain Python callable `parsed -> a`
/// that **raises** on a type/shape mismatch; the combinators build new such callables
/// (closures), and `_pf_dec_decode_string` parses a JSON string and *runs* one inside
/// `try`, catching any raise into a `Result` (`Ok`/`Error(_Exception(...))`). The
/// primitives are strict — `_pf_dec_int` rejects a JSON bool (a Python `bool` is an
/// `int` subclass) and `_pf_dec_float` accepts an int — so "parse, don't validate"
/// actually validates. Built from the IR, emitted in sorted helper-name order.
fn decode_prelude(used: &BTreeSet<&'static str>) -> Vec<PyStmt> {
    let name = |n: &str| PyExpr::Name(n.to_string());
    let str_ = |s: &str| PyExpr::Str(s.to_string());
    let int = PyExpr::Int;
    let call = |f: PyExpr, args: Vec<PyExpr>| PyExpr::Call {
        func: Box::new(f),
        args,
    };
    // `f(args...)` where `f` is a bare name.
    let calln = |f: &str, args: Vec<PyExpr>| PyExpr::Call {
        func: Box::new(PyExpr::Name(f.to_string())),
        args,
    };
    let attr = |v: PyExpr, a: &str| PyExpr::Attribute {
        value: Box::new(v),
        attr: a.to_string(),
    };
    let sub = |v: PyExpr, i: PyExpr| PyExpr::Subscript {
        value: Box::new(v),
        index: Box::new(i),
    };
    let binop = |op: PyBinOp, l: PyExpr, r: PyExpr| PyExpr::BinOp {
        op,
        left: Box::new(l),
        right: Box::new(r),
    };
    let not_ = |e: PyExpr| PyExpr::Not(Box::new(e));
    let ret = |e: PyExpr| PyStmt::Return(e);
    let raise_ = |e: PyExpr| PyStmt::Raise(e);
    // `if test: body` (no else) — the strict primitives fall through to a `raise`.
    let if_ = |test: PyExpr, body: Vec<PyStmt>| PyStmt::If {
        test,
        body,
        orelse: vec![],
    };
    let def = |n: &str, params: &[&str], body: Vec<PyStmt>| PyStmt::FuncDef {
        name: n.to_string(),
        params: params.iter().map(|p| p.to_string()).collect(),
        body,
        is_async: false,
    };
    // A decoder *factory*: an outer function that closes over its arguments and
    // returns the inner `go(v)` decoder callable.
    let factory = |n: &str, params: &[&str], go_body: Vec<PyStmt>| {
        def(
            n,
            params,
            vec![def("go", &["v"], go_body), PyStmt::Return(name("go"))],
        )
    };
    // A strict primitive `def _pf_dec_<t>(v): if <test>: return v; raise ValueError(...)`.
    // `noun` is the full article + type ("a string", "an int") for the error message.
    let primitive = |n: &str, noun: &str, test: PyExpr, ok: PyExpr| {
        def(
            n,
            &["v"],
            vec![
                if_(test, vec![PyStmt::Return(ok)]),
                raise_(calln(
                    "ValueError",
                    vec![binop(
                        PyBinOp::Add,
                        PyExpr::Str(format!("expected {noun}, got ")),
                        attr(calln("type", vec![name("v")]), "__name__"),
                    )],
                )),
            ],
        )
    };
    // `isinstance(v, <ty>)`.
    let isinst = |ty: PyExpr| calln("isinstance", vec![name("v"), ty]);
    // `f(da(v), db(v), …)` — apply a fan-in function to each decoder run on `v`.
    let fan_in = |fields: &[&str]| {
        call(
            name("f"),
            fields
                .iter()
                .map(|d| call(name(d), vec![name("v")]))
                .collect(),
        )
    };
    used.iter()
        .map(|&helper| match helper {
            // Primitives — strict about the parsed JSON type.
            "_pf_dec_string" => {
                primitive("_pf_dec_string", "a string", isinst(name("str")), name("v"))
            }
            // A JSON bool is a Python `bool`, an `int` subclass — exclude it.
            "_pf_dec_int" => primitive(
                "_pf_dec_int",
                "an int",
                binop(
                    PyBinOp::And,
                    isinst(name("int")),
                    not_(isinst(name("bool"))),
                ),
                name("v"),
            ),
            // Accept a JSON int or float (but not bool); normalize to a Python float.
            "_pf_dec_float" => primitive(
                "_pf_dec_float",
                "a float",
                binop(
                    PyBinOp::And,
                    isinst(PyExpr::Tuple(vec![name("int"), name("float")])),
                    not_(isinst(name("bool"))),
                ),
                calln("float", vec![name("v")]),
            ),
            "_pf_dec_bool" => primitive("_pf_dec_bool", "a bool", isinst(name("bool")), name("v")),
            // Decode.field name dec -> go(v) = dec(v[name]), guarded to a JSON object.
            "_pf_dec_field" => factory(
                "_pf_dec_field",
                &["name", "dec"],
                vec![
                    if_(
                        not_(isinst(name("dict"))),
                        vec![raise_(calln(
                            "ValueError",
                            vec![str_("expected a JSON object")],
                        ))],
                    ),
                    ret(call(name("dec"), vec![sub(name("v"), name("name"))])),
                ],
            ),
            // Decode.list dec -> go(v) = list(map(dec, v)), guarded to a JSON array.
            "_pf_dec_list" => factory(
                "_pf_dec_list",
                &["dec"],
                vec![
                    if_(
                        not_(isinst(name("list"))),
                        vec![raise_(calln(
                            "ValueError",
                            vec![str_("expected a JSON array")],
                        ))],
                    ),
                    ret(calln(
                        "list",
                        vec![calln("map", vec![name("dec"), name("v")])],
                    )),
                ],
            ),
            // Decode.nullable dec -> None_() when JSON null, else Some(dec(v)).
            "_pf_dec_nullable" => factory(
                "_pf_dec_nullable",
                &["dec"],
                vec![ret(PyExpr::IfExp {
                    body: Box::new(calln("None_", vec![])),
                    test: Box::new(binop(PyBinOp::Is, name("v"), PyExpr::NoneLit)),
                    orelse: Box::new(calln("Some", vec![call(name("dec"), vec![name("v")])])),
                })],
            ),
            // Decode.map f dec -> f(dec(v)).
            "_pf_dec_map" => factory(
                "_pf_dec_map",
                &["f", "dec"],
                vec![ret(call(
                    name("f"),
                    vec![call(name("dec"), vec![name("v")])],
                ))],
            ),
            // Decode.map2/3/4 — fan several decoders into one n-ary function.
            "_pf_dec_map2" => factory(
                "_pf_dec_map2",
                &["f", "da", "db"],
                vec![ret(fan_in(&["da", "db"]))],
            ),
            "_pf_dec_map3" => factory(
                "_pf_dec_map3",
                &["f", "da", "db", "dc"],
                vec![ret(fan_in(&["da", "db", "dc"]))],
            ),
            "_pf_dec_map4" => factory(
                "_pf_dec_map4",
                &["f", "da", "db", "dc", "dd"],
                vec![ret(fan_in(&["da", "db", "dc", "dd"]))],
            ),
            // Decode.succeed x -> a decoder that ignores its input.
            "_pf_dec_succeed" => factory("_pf_dec_succeed", &["x"], vec![ret(name("x"))]),
            // Decode.fail msg -> a decoder that always raises.
            "_pf_dec_fail" => factory(
                "_pf_dec_fail",
                &["msg"],
                vec![raise_(calln("ValueError", vec![name("msg")]))],
            ),
            // Decode.andThen f dec -> f(dec(v))(v): pick the next decoder from the value.
            "_pf_dec_and_then" => factory(
                "_pf_dec_and_then",
                &["f", "dec"],
                vec![ret(call(
                    call(name("f"), vec![call(name("dec"), vec![name("v")])]),
                    vec![name("v")],
                ))],
            ),
            // Decode.oneOf decs -> the first decoder that does not raise (recursive so
            // no `for`-loop IR node is needed).
            "_pf_dec_one_of" => factory(
                "_pf_dec_one_of",
                &["decs"],
                vec![
                    def(
                        "_try",
                        &["i"],
                        vec![
                            if_(
                                binop(PyBinOp::Ge, name("i"), calln("len", vec![name("decs")])),
                                vec![raise_(calln(
                                    "ValueError",
                                    vec![str_("Decode.oneOf: no decoder matched")],
                                ))],
                            ),
                            PyStmt::Try {
                                body: vec![ret(call(
                                    sub(name("decs"), name("i")),
                                    vec![name("v")],
                                ))],
                                exc_type: Some("Exception".to_string()),
                                binding: None,
                                handler: vec![ret(call(
                                    name("_try"),
                                    vec![binop(PyBinOp::Add, name("i"), int(1))],
                                ))],
                            },
                        ],
                    ),
                    ret(call(name("_try"), vec![int(0)])),
                ],
            ),
            // Decode.decodeString dec s -> Ok(dec(json.loads(s))) / Error(_Exception(...)).
            "_pf_dec_decode_string" => def(
                "_pf_dec_decode_string",
                &["dec", "s"],
                vec![PyStmt::Try {
                    body: vec![ret(calln(
                        "Ok",
                        vec![call(
                            name("dec"),
                            vec![call(attr(name("json"), "loads"), vec![name("s")])],
                        )],
                    ))],
                    exc_type: Some("Exception".to_string()),
                    binding: Some("e".to_string()),
                    handler: vec![ret(calln(
                        "Error",
                        vec![calln(
                            "_Exception",
                            vec![
                                attr(calln("type", vec![name("e")]), "__name__"),
                                calln("str", vec![name("e")]),
                            ],
                        )],
                    ))],
                }],
            ),
            other => unreachable!("unknown decode helper {other}"),
        })
        .collect()
}

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
    // A helper whose body needs statements rather than one `return`.
    let defs = |fn_name: &str, params: &[&str], body: Vec<PyStmt>| PyStmt::FuncDef {
        name: fn_name.to_string(),
        params: params.iter().map(|p| p.to_string()).collect(),
        body,
        is_async: false,
    };
    // `f(args...)` where `f` is any expression (a parameter holding a function).
    let callx = |f: PyExpr, args: Vec<PyExpr>| PyExpr::Call {
        func: Box::new(f),
        args,
    };
    let method = |recv: PyExpr, m: &str, args: Vec<PyExpr>| PyExpr::Call {
        func: Box::new(PyExpr::Attribute {
            value: Box::new(recv),
            attr: m.to_string(),
        }),
        args,
    };
    let slice = |value: PyExpr, lower: PyExpr, upper: PyExpr| PyExpr::Slice {
        value: Box::new(value),
        lower: Box::new(lower),
        upper: Box::new(upper),
    };
    let index = |value: PyExpr, i: PyExpr| PyExpr::Subscript {
        value: Box::new(value),
        index: Box::new(i),
    };
    let binop = |op: PyBinOp, left: PyExpr, right: PyExpr| PyExpr::BinOp {
        op,
        left: Box::new(left),
        right: Box::new(right),
    };
    let assign = |target: &str, value: PyExpr| PyStmt::Assign {
        target: target.to_string(),
        value,
    };
    let for_ = |target: &str, iter: PyExpr, body: Vec<PyStmt>| PyStmt::For {
        target: target.to_string(),
        iter,
        body,
    };
    // `max(n, 0)` — every count argument is clamped, so a negative one reads as
    // "none of it" rather than slicing from the far end the way Python would.
    let clamped = |n: &str| call("max", vec![name(n), PyExpr::Int(0)]);
    let len_of = |n: &str| call("len", vec![name(n)]);
    let empty_list = || PyExpr::List(vec![]);
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
            // _pf_zip(xs, ys) -> list(zip(xs, ys))  (a list of (x, y) tuples)
            "_pf_zip" => def(
                "_pf_zip",
                &["xs", "ys"],
                call("list", vec![call("zip", vec![name("xs"), name("ys")])]),
            ),
            // _pf_is_empty(xs) -> len(xs) == 0   (O(1))
            "_pf_is_empty" => def(
                "_pf_is_empty",
                &["xs"],
                PyExpr::Compare {
                    left: Box::new(call("len", vec![name("xs")])),
                    ops: vec![PyBinOp::Eq],
                    comparators: vec![PyExpr::Int(0)],
                },
            ),
            // _pf_list_contains(x, xs) -> x in xs   (O(n) linear scan)
            "_pf_list_contains" => def(
                "_pf_list_contains",
                &["x", "xs"],
                PyExpr::BinOp {
                    op: PyBinOp::In,
                    left: Box::new(name("x")),
                    right: Box::new(name("xs")),
                },
            ),
            // _pf_concat(xs, ys) -> xs + ys   (a fresh list, O(n+m))
            "_pf_concat" => def(
                "_pf_concat",
                &["xs", "ys"],
                PyExpr::BinOp {
                    op: PyBinOp::Add,
                    left: Box::new(name("xs")),
                    right: Box::new(name("ys")),
                },
            ),
            // _pf_sort(xs) -> sorted(xs)   (a fresh list, O(n log n))
            "_pf_sort" => def("_pf_sort", &["xs"], call("sorted", vec![name("xs")])),
            // _pf_collect(f, xs): map + concatenate (F#'s List.collect), an eager
            // extend-loop rather than a chained iterator.
            //   out = []
            //   for x in xs: out.extend(f(x))
            //   return out
            "_pf_collect" => PyStmt::FuncDef {
                name: "_pf_collect".to_string(),
                params: vec!["f".to_string(), "xs".to_string()],
                body: vec![
                    PyStmt::Assign {
                        target: "out".to_string(),
                        value: PyExpr::List(vec![]),
                    },
                    PyStmt::For {
                        target: "x".to_string(),
                        iter: name("xs"),
                        body: vec![PyStmt::Expr(PyExpr::Call {
                            func: Box::new(PyExpr::Attribute {
                                value: Box::new(name("out")),
                                attr: "extend".to_string(),
                            }),
                            args: vec![call("f", vec![name("x")])],
                        })],
                    },
                    PyStmt::Return(name("out")),
                ],
                is_async: false,
            },
            // _pf_take(n, xs) -> xs[0:max(n, 0)]   (total: a count past the end
            // simply takes everything, like `String.slice`)
            "_pf_take" => def(
                "_pf_take",
                &["n", "xs"],
                slice(name("xs"), PyExpr::Int(0), clamped("n")),
            ),
            // _pf_drop(n, xs) -> xs[max(n, 0):len(xs)]
            "_pf_drop" => def(
                "_pf_drop",
                &["n", "xs"],
                slice(name("xs"), clamped("n"), len_of("xs")),
            ),
            // _pf_split_at(n, xs) -> (xs[0:m], xs[m:len(xs)])
            "_pf_split_at" => defs(
                "_pf_split_at",
                &["n", "xs"],
                vec![
                    assign("m", clamped("n")),
                    PyStmt::Return(PyExpr::Tuple(vec![
                        slice(name("xs"), PyExpr::Int(0), name("m")),
                        slice(name("xs"), name("m"), len_of("xs")),
                    ])),
                ],
            ),
            // _pf_map2(f, xs, ys): stops at the shorter input, like `zip`
            "_pf_map2" => defs(
                "_pf_map2",
                &["f", "xs", "ys"],
                vec![
                    assign("out", empty_list()),
                    for_(
                        "p",
                        call("zip", vec![name("xs"), name("ys")]),
                        vec![PyStmt::Expr(method(
                            name("out"),
                            "append",
                            vec![callx(
                                name("f"),
                                vec![
                                    index(name("p"), PyExpr::Int(0)),
                                    index(name("p"), PyExpr::Int(1)),
                                ],
                            )],
                        ))],
                    ),
                    PyStmt::Return(name("out")),
                ],
            ),
            // _pf_indexed(xs) -> list(enumerate(xs))
            "_pf_indexed" => def(
                "_pf_indexed",
                &["xs"],
                call("list", vec![call("enumerate", vec![name("xs")])]),
            ),
            // _pf_iter(f, xs): run f for its effect, answer unit
            "_pf_iter" => defs(
                "_pf_iter",
                &["f", "xs"],
                vec![
                    for_(
                        "x",
                        name("xs"),
                        vec![PyStmt::Expr(callx(name("f"), vec![name("x")]))],
                    ),
                    PyStmt::Return(PyExpr::NoneLit),
                ],
            ),
            // _pf_exists(f, xs) -> any(map(f, xs))   (short-circuits)
            "_pf_exists" => def(
                "_pf_exists",
                &["f", "xs"],
                call("any", vec![call("map", vec![name("f"), name("xs")])]),
            ),
            // _pf_forall(f, xs) -> all(map(f, xs))   (short-circuits)
            "_pf_forall" => def(
                "_pf_forall",
                &["f", "xs"],
                call("all", vec![call("map", vec![name("f"), name("xs")])]),
            ),
            // _pf_sort_by(f, xs) -> sorted(xs, key=f)
            "_pf_sort_by" => def(
                "_pf_sort_by",
                &["f", "xs"],
                PyExpr::CallKw {
                    func: Box::new(name("sorted")),
                    args: vec![name("xs")],
                    kwargs: vec![("key".to_string(), name("f"))],
                },
            ),
            // _pf_sort_desc(xs) -> sorted(xs, reverse=True)
            "_pf_sort_desc" => def(
                "_pf_sort_desc",
                &["xs"],
                PyExpr::CallKw {
                    func: Box::new(name("sorted")),
                    args: vec![name("xs")],
                    kwargs: vec![("reverse".to_string(), PyExpr::Bool(true))],
                },
            ),
            // _pf_distinct(xs) -> list(dict.fromkeys(xs))   (first occurrence wins
            // and order is kept — dicts preserve insertion order)
            "_pf_distinct" => def(
                "_pf_distinct",
                &["xs"],
                call(
                    "list",
                    vec![method(name("dict"), "fromkeys", vec![name("xs")])],
                ),
            ),
            // _pf_distinct_by(f, xs): first element per key, order kept
            "_pf_distinct_by" => defs(
                "_pf_distinct_by",
                &["f", "xs"],
                vec![
                    assign("seen", call("set", vec![])),
                    assign("out", empty_list()),
                    for_(
                        "x",
                        name("xs"),
                        vec![
                            assign("k", callx(name("f"), vec![name("x")])),
                            PyStmt::If {
                                test: PyExpr::Not(Box::new(binop(
                                    PyBinOp::In,
                                    name("k"),
                                    name("seen"),
                                ))),
                                body: vec![
                                    PyStmt::Expr(method(name("seen"), "add", vec![name("k")])),
                                    PyStmt::Expr(method(name("out"), "append", vec![name("x")])),
                                ],
                                orelse: vec![],
                            },
                        ],
                    ),
                    PyStmt::Return(name("out")),
                ],
            ),
            // _pf_group_by(f, xs) -> list of (key, members), in first-seen key order
            "_pf_group_by" => defs(
                "_pf_group_by",
                &["f", "xs"],
                vec![
                    assign("groups", call("dict", vec![])),
                    for_(
                        "x",
                        name("xs"),
                        vec![PyStmt::Expr(method(
                            method(
                                name("groups"),
                                "setdefault",
                                vec![callx(name("f"), vec![name("x")]), empty_list()],
                            ),
                            "append",
                            vec![name("x")],
                        ))],
                    ),
                    PyStmt::Return(call("list", vec![method(name("groups"), "items", vec![])])),
                ],
            ),
            // _pf_sum_by(f, xs) -> sum(map(f, xs))
            "_pf_sum_by" => def(
                "_pf_sum_by",
                &["f", "xs"],
                call("sum", vec![call("map", vec![name("f"), name("xs")])]),
            ),
            // _pf_partition(f, xs) -> (kept, rejected), testing each element once
            "_pf_partition" => defs(
                "_pf_partition",
                &["f", "xs"],
                vec![
                    assign("yes", empty_list()),
                    assign("no", empty_list()),
                    for_(
                        "x",
                        name("xs"),
                        vec![PyStmt::If {
                            test: callx(name("f"), vec![name("x")]),
                            body: vec![PyStmt::Expr(method(
                                name("yes"),
                                "append",
                                vec![name("x")],
                            ))],
                            orelse: vec![PyStmt::Expr(method(
                                name("no"),
                                "append",
                                vec![name("x")],
                            ))],
                        }],
                    ),
                    PyStmt::Return(PyExpr::Tuple(vec![name("yes"), name("no")])),
                ],
            ),
            // _pf_unzip(xs) -> (firsts, seconds)
            "_pf_unzip" => defs(
                "_pf_unzip",
                &["xs"],
                vec![
                    assign("a", empty_list()),
                    assign("b", empty_list()),
                    for_(
                        "p",
                        name("xs"),
                        vec![
                            PyStmt::Expr(method(
                                name("a"),
                                "append",
                                vec![index(name("p"), PyExpr::Int(0))],
                            )),
                            PyStmt::Expr(method(
                                name("b"),
                                "append",
                                vec![index(name("p"), PyExpr::Int(1))],
                            )),
                        ],
                    ),
                    PyStmt::Return(PyExpr::Tuple(vec![name("a"), name("b")])),
                ],
            ),
            // _pf_flatten(xs) -> one list of every element of every element
            "_pf_flatten" => defs(
                "_pf_flatten",
                &["xs"],
                vec![
                    assign("out", empty_list()),
                    for_(
                        "x",
                        name("xs"),
                        vec![PyStmt::Expr(method(name("out"), "extend", vec![name("x")]))],
                    ),
                    PyStmt::Return(name("out")),
                ],
            ),
            // _pf_init(n, f) -> [f(0), …, f(n-1)]
            "_pf_init" => defs(
                "_pf_init",
                &["n", "f"],
                vec![
                    assign("out", empty_list()),
                    for_(
                        "i",
                        call("range", vec![clamped("n")]),
                        vec![PyStmt::Expr(method(
                            name("out"),
                            "append",
                            vec![callx(name("f"), vec![name("i")])],
                        ))],
                    ),
                    PyStmt::Return(name("out")),
                ],
            ),
            // _pf_replicate(n, x) -> [x] * max(n, 0)
            "_pf_replicate" => def(
                "_pf_replicate",
                &["n", "x"],
                binop(PyBinOp::Mul, PyExpr::List(vec![name("x")]), clamped("n")),
            ),
            // _pf_update_at(i, v, xs): a fresh list with index i replaced; an index
            // outside the list leaves it unchanged (total, like the accessors)
            "_pf_update_at" => defs(
                "_pf_update_at",
                &["i", "v", "xs"],
                vec![
                    assign("out", call("list", vec![name("xs")])),
                    PyStmt::If {
                        test: PyExpr::Compare {
                            left: Box::new(PyExpr::Int(0)),
                            ops: vec![PyBinOp::Le, PyBinOp::Lt],
                            comparators: vec![name("i"), len_of("out")],
                        },
                        body: vec![PyStmt::SubscriptAssign {
                            obj: name("out"),
                            index: name("i"),
                            value: name("v"),
                        }],
                        orelse: vec![],
                    },
                    PyStmt::Return(name("out")),
                ],
            ),
            // _pf_insert_at(i, v, xs): index clamped to the ends
            "_pf_insert_at" => defs(
                "_pf_insert_at",
                &["i", "v", "xs"],
                vec![
                    assign("out", call("list", vec![name("xs")])),
                    PyStmt::Expr(method(name("out"), "insert", vec![clamped("i"), name("v")])),
                    PyStmt::Return(name("out")),
                ],
            ),
            // _pf_remove_at(i, xs): an index outside the list leaves it unchanged
            "_pf_remove_at" => defs(
                "_pf_remove_at",
                &["i", "xs"],
                vec![
                    assign("out", call("list", vec![name("xs")])),
                    PyStmt::If {
                        test: PyExpr::Compare {
                            left: Box::new(PyExpr::Int(0)),
                            ops: vec![PyBinOp::Le, PyBinOp::Lt],
                            comparators: vec![name("i"), len_of("out")],
                        },
                        body: vec![PyStmt::Expr(method(name("out"), "pop", vec![name("i")]))],
                        orelse: vec![],
                    },
                    PyStmt::Return(name("out")),
                ],
            ),
            // _pf_pairwise(xs) -> list(zip(xs, xs[1:len(xs)]))
            "_pf_pairwise" => def(
                "_pf_pairwise",
                &["xs"],
                call(
                    "list",
                    vec![call(
                        "zip",
                        vec![name("xs"), slice(name("xs"), PyExpr::Int(1), len_of("xs"))],
                    )],
                ),
            ),
            // _pf_windowed(n, xs): full windows only; a non-positive size gives []
            "_pf_windowed" => defs(
                "_pf_windowed",
                &["n", "xs"],
                vec![
                    PyStmt::If {
                        test: PyExpr::Compare {
                            left: Box::new(name("n")),
                            ops: vec![PyBinOp::Le],
                            comparators: vec![PyExpr::Int(0)],
                        },
                        body: vec![PyStmt::Return(empty_list())],
                        orelse: vec![],
                    },
                    assign("out", empty_list()),
                    for_(
                        "i",
                        call(
                            "range",
                            vec![binop(
                                PyBinOp::Add,
                                binop(PyBinOp::Sub, len_of("xs"), name("n")),
                                PyExpr::Int(1),
                            )],
                        ),
                        vec![PyStmt::Expr(method(
                            name("out"),
                            "append",
                            vec![slice(
                                name("xs"),
                                name("i"),
                                binop(PyBinOp::Add, name("i"), name("n")),
                            )],
                        ))],
                    ),
                    PyStmt::Return(name("out")),
                ],
            ),
            // _pf_chunk(n, xs): consecutive chunks, the last one short if the
            // length does not divide evenly; a non-positive size gives []
            "_pf_chunk" => defs(
                "_pf_chunk",
                &["n", "xs"],
                vec![
                    PyStmt::If {
                        test: PyExpr::Compare {
                            left: Box::new(name("n")),
                            ops: vec![PyBinOp::Le],
                            comparators: vec![PyExpr::Int(0)],
                        },
                        body: vec![PyStmt::Return(empty_list())],
                        orelse: vec![],
                    },
                    assign("out", empty_list()),
                    for_(
                        "i",
                        call("range", vec![PyExpr::Int(0), len_of("xs"), name("n")]),
                        vec![PyStmt::Expr(method(
                            name("out"),
                            "append",
                            vec![slice(
                                name("xs"),
                                name("i"),
                                binop(PyBinOp::Add, name("i"), name("n")),
                            )],
                        ))],
                    ),
                    PyStmt::Return(name("out")),
                ],
            ),
            // _pf_take_while(f, xs) -> list(itertools.takewhile(f, xs))
            "_pf_take_while" => def(
                "_pf_take_while",
                &["f", "xs"],
                call(
                    "list",
                    vec![PyExpr::Call {
                        func: Box::new(PyExpr::Attribute {
                            value: Box::new(name("itertools")),
                            attr: "takewhile".to_string(),
                        }),
                        args: vec![name("f"), name("xs")],
                    }],
                ),
            ),
            // _pf_drop_while(f, xs) -> list(itertools.dropwhile(f, xs))
            "_pf_drop_while" => def(
                "_pf_drop_while",
                &["f", "xs"],
                call(
                    "list",
                    vec![PyExpr::Call {
                        func: Box::new(PyExpr::Attribute {
                            value: Box::new(name("itertools")),
                            attr: "dropwhile".to_string(),
                        }),
                        args: vec![name("f"), name("xs")],
                    }],
                ),
            ),
            // _pf_sort_by_desc(f, xs) -> sorted(xs, key=f, reverse=True)
            "_pf_sort_by_desc" => def(
                "_pf_sort_by_desc",
                &["f", "xs"],
                PyExpr::CallKw {
                    func: Box::new(name("sorted")),
                    args: vec![name("xs")],
                    kwargs: vec![
                        ("key".to_string(), name("f")),
                        ("reverse".to_string(), PyExpr::Bool(true)),
                    ],
                },
            ),
            // _pf_count_by(f, xs) -> [(key, how many), …] in first-seen key order
            "_pf_count_by" => defs(
                "_pf_count_by",
                &["f", "xs"],
                vec![
                    assign("counts", call("dict", vec![])),
                    for_(
                        "x",
                        name("xs"),
                        vec![
                            assign("k", callx(name("f"), vec![name("x")])),
                            PyStmt::SubscriptAssign {
                                obj: name("counts"),
                                index: name("k"),
                                value: binop(
                                    PyBinOp::Add,
                                    method(name("counts"), "get", vec![name("k"), PyExpr::Int(0)]),
                                    PyExpr::Int(1),
                                ),
                            },
                        ],
                    ),
                    PyStmt::Return(call("list", vec![method(name("counts"), "items", vec![])])),
                ],
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
            // Map.toList(m) -> list(m.items())  (a list of (k, v) tuples)
            "_pf_map_to_list" => def1(
                helper,
                &["m"],
                call("list", vec![method(name("m"), "items", vec![])]),
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
            // Option.bind(f, o) -> f(o._0) if isinstance(o, Some) else None_()
            // (f already returns an Option, so it is returned as-is.)
            "_pf_option_bind" => def(
                helper,
                &["f", "o"],
                vec![
                    PyStmt::If {
                        test: is_some("o"),
                        body: vec![PyStmt::Return(call("f", vec![attr(name("o"), "_0")]))],
                        orelse: vec![],
                    },
                    PyStmt::Return(call0("None_")),
                ],
            ),
            // Option.filter(f, o) -> o if isinstance(o, Some) and f(o._0) else None_()
            // `and` short-circuits, so f isn't called on a None.
            "_pf_option_filter" => def1(
                helper,
                &["f", "o"],
                PyExpr::IfExp {
                    body: Box::new(name("o")),
                    test: Box::new(binop(
                        PyBinOp::And,
                        is_some("o"),
                        call("f", vec![attr(name("o"), "_0")]),
                    )),
                    orelse: Box::new(call0("None_")),
                },
            ),
            // Option.toResult(err, o) -> Ok(o._0) if isinstance(o, Some) else Error(err)
            "_pf_option_to_result" => def(
                helper,
                &["err", "o"],
                vec![
                    PyStmt::If {
                        test: is_some("o"),
                        body: vec![PyStmt::Return(call1("Ok", attr(name("o"), "_0")))],
                        orelse: vec![],
                    },
                    PyStmt::Return(call1("Error", name("err"))),
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
            // List.get(i, xs) -> Some(xs[i]) if 0 <= i < len(xs) else None_()
            // Bounds-checked (negatives too), so it's total — no Python IndexError.
            "_pf_list_get" => def1(
                helper,
                &["i", "xs"],
                PyExpr::IfExp {
                    body: Box::new(call1(
                        "Some",
                        PyExpr::Subscript {
                            value: Box::new(name("xs")),
                            index: Box::new(name("i")),
                        },
                    )),
                    test: Box::new(PyExpr::Compare {
                        left: Box::new(PyExpr::Int(0)),
                        ops: vec![PyBinOp::Le, PyBinOp::Lt],
                        comparators: vec![name("i"), call("len", vec![name("xs")])],
                    }),
                    orelse: Box::new(call0("None_")),
                },
            ),
            // List.find(f, xs) -> next(map(Some, filter(f, xs)), None_())
            // Lazy: `filter` + `next` stop at the first match (no full scan).
            "_pf_list_find" => def1(
                helper,
                &["f", "xs"],
                call(
                    "next",
                    vec![
                        call(
                            "map",
                            vec![name("Some"), call("filter", vec![name("f"), name("xs")])],
                        ),
                        call0("None_"),
                    ],
                ),
            ),
            // List.choose(f, xs): keep the payloads of the `Some` results of `f`
            // (F#'s List.choose — filter + map fused into one eager pass).
            //   out = []
            //   for x in xs:
            //       y = f(x)
            //       if isinstance(y, Some): out.append(y._0)
            //   return out
            "_pf_choose" => def(
                helper,
                &["f", "xs"],
                vec![
                    PyStmt::Assign {
                        target: "out".to_string(),
                        value: PyExpr::List(vec![]),
                    },
                    PyStmt::For {
                        target: "x".to_string(),
                        iter: name("xs"),
                        body: vec![
                            PyStmt::Assign {
                                target: "y".to_string(),
                                value: call("f", vec![name("x")]),
                            },
                            PyStmt::If {
                                test: is_some("y"),
                                body: vec![PyStmt::Expr(method(
                                    name("out"),
                                    "append",
                                    vec![attr(name("y"), "_0")],
                                ))],
                                orelse: vec![],
                            },
                        ],
                    },
                    PyStmt::Return(name("out")),
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
            // String.concat(a, b) -> a + b
            "_pf_str_concat" => def1(
                helper,
                &["a", "b"],
                binop(PyBinOp::Add, name("a"), name("b")),
            ),
            // String.join(sep, xs) -> sep.join(xs)
            "_pf_str_join" => def1(
                helper,
                &["sep", "xs"],
                method(name("sep"), "join", vec![name("xs")]),
            ),
            // String.split(sep, s) -> s.split(sep)
            "_pf_str_split" => def1(
                helper,
                &["sep", "s"],
                method(name("s"), "split", vec![name("sep")]),
            ),
            // String.toUpper(s) -> s.upper()
            "_pf_str_upper" => def1(helper, &["s"], method(name("s"), "upper", vec![])),
            // String.toLower(s) -> s.lower()
            "_pf_str_lower" => def1(helper, &["s"], method(name("s"), "lower", vec![])),
            // String.strip(s) -> s.strip()
            "_pf_str_strip" => def1(helper, &["s"], method(name("s"), "strip", vec![])),
            // String.contains(sub, s) -> sub in s
            "_pf_str_contains" => def1(
                helper,
                &["sub", "s"],
                binop(PyBinOp::In, name("sub"), name("s")),
            ),
            // String.startsWith(pre, s) -> s.startswith(pre)
            "_pf_str_starts_with" => def1(
                helper,
                &["pre", "s"],
                method(name("s"), "startswith", vec![name("pre")]),
            ),
            // String.endsWith(suf, s) -> s.endswith(suf)
            "_pf_str_ends_with" => def1(
                helper,
                &["suf", "s"],
                method(name("s"), "endswith", vec![name("suf")]),
            ),
            // String.replace(old, new, s) -> s.replace(old, new)
            "_pf_str_replace" => def1(
                helper,
                &["old", "new", "s"],
                method(name("s"), "replace", vec![name("old"), name("new")]),
            ),
            // String.slice(start, end, s) -> s[start:end]  (total, Python slicing)
            "_pf_str_slice" => def1(
                helper,
                &["start", "end", "s"],
                PyExpr::Slice {
                    value: Box::new(name("s")),
                    lower: Box::new(name("start")),
                    upper: Box::new(name("end")),
                },
            ),
            // String.tryIndexOf(sub, s): i = s.find(sub); Some(i) if i >= 0 else None_
            // (`find` returns -1 when absent, so this is total.)
            "_pf_str_index_of" => def(
                helper,
                &["sub", "s"],
                vec![
                    PyStmt::Assign {
                        target: "i".to_string(),
                        value: method(name("s"), "find", vec![name("sub")]),
                    },
                    PyStmt::Return(PyExpr::IfExp {
                        body: Box::new(call1("Some", name("i"))),
                        test: Box::new(PyExpr::Compare {
                            left: Box::new(name("i")),
                            ops: vec![PyBinOp::Ge],
                            comparators: vec![PyExpr::Int(0)],
                        }),
                        orelse: Box::new(call0("None_")),
                    }),
                ],
            ),
            // String.toInt(s) -> total parse: Some(int(s)) or None_ on ValueError
            "_pf_str_to_int" => def(
                helper,
                &["s"],
                vec![PyStmt::Try {
                    body: vec![PyStmt::Return(call1("Some", call("int", vec![name("s")])))],
                    exc_type: Some("ValueError".to_string()),
                    binding: None,
                    handler: vec![PyStmt::Return(call0("None_"))],
                }],
            ),
            // String.toFloat(s) -> total parse: Some(float(s)) or None_ on ValueError
            "_pf_str_to_float" => def(
                helper,
                &["s"],
                vec![PyStmt::Try {
                    body: vec![PyStmt::Return(call1(
                        "Some",
                        call("float", vec![name("s")]),
                    ))],
                    exc_type: Some("ValueError".to_string()),
                    binding: None,
                    handler: vec![PyStmt::Return(call0("None_"))],
                }],
            ),
            // Format helpers — the format spec is assembled from the checked `int`
            // decimal count (a nested f-string), so a `.2f` -> `.f2` typo can't arise.
            // Format.fixed(n, x) -> format(x, f".{n}f")
            "_pf_fmt_fixed" => def1(
                helper,
                &["n", "x"],
                call(
                    "format",
                    vec![
                        name("x"),
                        PyExpr::FStr(vec![
                            PyFStrPart::Lit(".".to_string()),
                            PyFStrPart::Expr(name("n")),
                            PyFStrPart::Lit("f".to_string()),
                        ]),
                    ],
                ),
            ),
            // Format.thousands(n, x) -> format(x, f",.{n}f")
            "_pf_fmt_thousands" => def1(
                helper,
                &["n", "x"],
                call(
                    "format",
                    vec![
                        name("x"),
                        PyExpr::FStr(vec![
                            PyFStrPart::Lit(",.".to_string()),
                            PyFStrPart::Expr(name("n")),
                            PyFStrPart::Lit("f".to_string()),
                        ]),
                    ],
                ),
            ),
            // Format.percent(n, x) -> format(x, f".{n}%")  (Python `%` scales by 100)
            "_pf_fmt_percent" => def1(
                helper,
                &["n", "x"],
                call(
                    "format",
                    vec![
                        name("x"),
                        PyExpr::FStr(vec![
                            PyFStrPart::Lit(".".to_string()),
                            PyFStrPart::Expr(name("n")),
                            PyFStrPart::Lit("%".to_string()),
                        ]),
                    ],
                ),
            ),
            // Format.currency(sym, n, x) -> sym + format(x, f",.{n}f")
            "_pf_fmt_currency" => def1(
                helper,
                &["sym", "n", "x"],
                binop(
                    PyBinOp::Add,
                    name("sym"),
                    call(
                        "format",
                        vec![
                            name("x"),
                            PyExpr::FStr(vec![
                                PyFStrPart::Lit(",.".to_string()),
                                PyFStrPart::Expr(name("n")),
                                PyFStrPart::Lit("f".to_string()),
                            ]),
                        ],
                    ),
                ),
            ),
            // Format.grouped(x) -> format(x, ",")  (thousands-grouped integer)
            "_pf_fmt_grouped" => def1(
                helper,
                &["x"],
                call("format", vec![name("x"), PyExpr::Str(",".to_string())]),
            ),
            // Format.padLeft(w, fill, s) -> s.rjust(w, fill)
            "_pf_fmt_pad_left" => def1(
                helper,
                &["w", "fill", "s"],
                method(name("s"), "rjust", vec![name("w"), name("fill")]),
            ),
            // Format.padRight(w, fill, s) -> s.ljust(w, fill)
            "_pf_fmt_pad_right" => def1(
                helper,
                &["w", "fill", "s"],
                method(name("s"), "ljust", vec![name("w"), name("fill")]),
            ),
            // List.head(xs) -> Some(xs[0]) if xs else None_()
            // Every accessor that an empty list has no answer for reports it the
            // same way `get`/`find` do, rather than raising as F# does.
            "_pf_head" => def1(
                "_pf_head",
                &["xs"],
                PyExpr::IfExp {
                    body: Box::new(call(
                        "Some",
                        vec![PyExpr::Subscript {
                            value: Box::new(name("xs")),
                            index: Box::new(PyExpr::Int(0)),
                        }],
                    )),
                    test: Box::new(name("xs")),
                    orelse: Box::new(call("None_", vec![])),
                },
            ),
            // List.last(xs) -> Some(xs[-1]) if xs else None_()
            "_pf_last" => def1(
                "_pf_last",
                &["xs"],
                PyExpr::IfExp {
                    body: Box::new(call(
                        "Some",
                        vec![PyExpr::Subscript {
                            value: Box::new(name("xs")),
                            index: Box::new(PyExpr::Neg(Box::new(PyExpr::Int(1)))),
                        }],
                    )),
                    test: Box::new(name("xs")),
                    orelse: Box::new(call("None_", vec![])),
                },
            ),
            // List.tail(xs) -> Some(xs[1:len(xs)]) if xs else None_()
            "_pf_tail" => def1(
                "_pf_tail",
                &["xs"],
                PyExpr::IfExp {
                    body: Box::new(call(
                        "Some",
                        vec![PyExpr::Slice {
                            value: Box::new(name("xs")),
                            lower: Box::new(PyExpr::Int(1)),
                            upper: Box::new(call("len", vec![name("xs")])),
                        }],
                    )),
                    test: Box::new(name("xs")),
                    orelse: Box::new(call("None_", vec![])),
                },
            ),
            // List.findIndex(f, xs): the first index whose element passes, if any
            "_pf_find_index" => def(
                "_pf_find_index",
                &["f", "xs"],
                vec![
                    PyStmt::For {
                        target: "p".to_string(),
                        iter: call("enumerate", vec![name("xs")]),
                        body: vec![PyStmt::If {
                            test: PyExpr::Call {
                                func: Box::new(name("f")),
                                args: vec![PyExpr::Subscript {
                                    value: Box::new(name("p")),
                                    index: Box::new(PyExpr::Int(1)),
                                }],
                            },
                            body: vec![PyStmt::Return(call(
                                "Some",
                                vec![PyExpr::Subscript {
                                    value: Box::new(name("p")),
                                    index: Box::new(PyExpr::Int(0)),
                                }],
                            ))],
                            orelse: vec![],
                        }],
                    },
                    PyStmt::Return(call("None_", vec![])),
                ],
            ),
            // List.max(xs) -> Some(max(xs)) if xs else None_()
            "_pf_max" => def1(
                "_pf_max",
                &["xs"],
                PyExpr::IfExp {
                    body: Box::new(call("Some", vec![call("max", vec![name("xs")])])),
                    test: Box::new(name("xs")),
                    orelse: Box::new(call("None_", vec![])),
                },
            ),
            // List.min(xs) -> Some(min(xs)) if xs else None_()
            "_pf_min" => def1(
                "_pf_min",
                &["xs"],
                PyExpr::IfExp {
                    body: Box::new(call("Some", vec![call("min", vec![name("xs")])])),
                    test: Box::new(name("xs")),
                    orelse: Box::new(call("None_", vec![])),
                },
            ),
            // List.maxBy(f, xs) -> Some(max(xs, key=f)) if xs else None_()
            "_pf_max_by" => def1(
                "_pf_max_by",
                &["f", "xs"],
                PyExpr::IfExp {
                    body: Box::new(call(
                        "Some",
                        vec![PyExpr::CallKw {
                            func: Box::new(name("max")),
                            args: vec![name("xs")],
                            kwargs: vec![("key".to_string(), name("f"))],
                        }],
                    )),
                    test: Box::new(name("xs")),
                    orelse: Box::new(call("None_", vec![])),
                },
            ),
            // List.minBy(f, xs) -> Some(min(xs, key=f)) if xs else None_()
            "_pf_min_by" => def1(
                "_pf_min_by",
                &["f", "xs"],
                PyExpr::IfExp {
                    body: Box::new(call(
                        "Some",
                        vec![PyExpr::CallKw {
                            func: Box::new(name("min")),
                            args: vec![name("xs")],
                            kwargs: vec![("key".to_string(), name("f"))],
                        }],
                    )),
                    test: Box::new(name("xs")),
                    orelse: Box::new(call("None_", vec![])),
                },
            ),
            // List.average(xs) -> Some(sum(xs) / len(xs)) if xs else None_()
            "_pf_average" => def1(
                "_pf_average",
                &["xs"],
                PyExpr::IfExp {
                    body: Box::new(call(
                        "Some",
                        vec![binop(
                            PyBinOp::Div,
                            call("sum", vec![name("xs")]),
                            call("len", vec![name("xs")]),
                        )],
                    )),
                    test: Box::new(name("xs")),
                    orelse: Box::new(call("None_", vec![])),
                },
            ),
            // List.reduce(f, xs) -> Some(functools.reduce(f, xs)) if xs else None_()
            // A fold with no seed, so an empty list genuinely has no answer.
            "_pf_reduce" => def1(
                "_pf_reduce",
                &["f", "xs"],
                PyExpr::IfExp {
                    body: Box::new(call(
                        "Some",
                        vec![PyExpr::Call {
                            func: Box::new(attr(name("functools"), "reduce")),
                            args: vec![name("f"), name("xs")],
                        }],
                    )),
                    test: Box::new(name("xs")),
                    orelse: Box::new(call("None_", vec![])),
                },
            ),
            // ---- Seq: the lazy half routes to Python's own lazy machinery ----
            // Seq.drop(n, xs) -> itertools.islice(xs, max(n, 0), None)
            "_pf_seq_drop" => def1(
                "_pf_seq_drop",
                &["n", "xs"],
                PyExpr::Call {
                    func: Box::new(attr(name("itertools"), "islice")),
                    args: vec![
                        name("xs"),
                        call("max", vec![name("n"), PyExpr::Int(0)]),
                        PyExpr::NoneLit,
                    ],
                },
            ),
            // Seq.takeWhile(f, xs) -> itertools.takewhile(f, xs)
            "_pf_seq_take_while" => def1(
                "_pf_seq_take_while",
                &["f", "xs"],
                PyExpr::Call {
                    func: Box::new(attr(name("itertools"), "takewhile")),
                    args: vec![name("f"), name("xs")],
                },
            ),
            // Seq.dropWhile(f, xs) -> itertools.dropwhile(f, xs)
            "_pf_seq_drop_while" => def1(
                "_pf_seq_drop_while",
                &["f", "xs"],
                PyExpr::Call {
                    func: Box::new(attr(name("itertools"), "dropwhile")),
                    args: vec![name("f"), name("xs")],
                },
            ),
            // Seq.concat(xs, ys) -> itertools.chain(xs, ys)
            "_pf_seq_concat" => def1(
                "_pf_seq_concat",
                &["xs", "ys"],
                PyExpr::Call {
                    func: Box::new(attr(name("itertools"), "chain")),
                    args: vec![name("xs"), name("ys")],
                },
            ),
            // Seq.flatten(xs) -> itertools.chain.from_iterable(xs)
            "_pf_seq_flatten" => def1(
                "_pf_seq_flatten",
                &["xs"],
                PyExpr::Call {
                    func: Box::new(attr(attr(name("itertools"), "chain"), "from_iterable")),
                    args: vec![name("xs")],
                },
            ),
            // Seq.collect(f, xs) -> itertools.chain.from_iterable(map(f, xs))
            "_pf_seq_collect" => def1(
                "_pf_seq_collect",
                &["f", "xs"],
                PyExpr::Call {
                    func: Box::new(attr(attr(name("itertools"), "chain"), "from_iterable")),
                    args: vec![call("map", vec![name("f"), name("xs")])],
                },
            ),
            // Seq.pairwise(xs) -> itertools.pairwise(xs)
            "_pf_seq_pairwise" => def1(
                "_pf_seq_pairwise",
                &["xs"],
                PyExpr::Call {
                    func: Box::new(attr(name("itertools"), "pairwise")),
                    args: vec![name("xs")],
                },
            ),
            // Seq.init(n, f) -> map(f, range(max(n, 0)))
            "_pf_seq_init" => def1(
                "_pf_seq_init",
                &["n", "f"],
                call(
                    "map",
                    vec![
                        name("f"),
                        call("range", vec![call("max", vec![name("n"), PyExpr::Int(0)])]),
                    ],
                ),
            ),
            // Seq.initInfinite(f) -> map(f, itertools.count())
            "_pf_seq_init_inf" => def1(
                "_pf_seq_init_inf",
                &["f"],
                call(
                    "map",
                    vec![
                        name("f"),
                        PyExpr::Call {
                            func: Box::new(attr(name("itertools"), "count")),
                            args: vec![],
                        },
                    ],
                ),
            ),
            // Seq.distinct(xs): a generator, so it stays lazy while remembering
            // what it has already yielded.
            "_pf_seq_distinct" => def(
                "_pf_seq_distinct",
                &["xs"],
                vec![
                    PyStmt::Assign {
                        target: "seen".to_string(),
                        value: call("set", vec![]),
                    },
                    PyStmt::For {
                        target: "x".to_string(),
                        iter: name("xs"),
                        body: vec![PyStmt::If {
                            test: PyExpr::Not(Box::new(binop(
                                PyBinOp::In,
                                name("x"),
                                name("seen"),
                            ))),
                            body: vec![
                                PyStmt::Expr(method(name("seen"), "add", vec![name("x")])),
                                PyStmt::Yield(name("x")),
                            ],
                            orelse: vec![],
                        }],
                    },
                ],
            ),
            // Seq.unfold(f, state): yield until the function answers None.
            "_pf_seq_unfold" => def(
                "_pf_seq_unfold",
                &["f", "state"],
                vec![PyStmt::WhileTrue {
                    body: vec![
                        PyStmt::Assign {
                            target: "step".to_string(),
                            value: PyExpr::Call {
                                func: Box::new(name("f")),
                                args: vec![name("state")],
                            },
                        },
                        PyStmt::If {
                            test: PyExpr::Not(Box::new(is_some("step"))),
                            body: vec![PyStmt::Return(PyExpr::NoneLit)],
                            orelse: vec![],
                        },
                        PyStmt::Assign {
                            target: "pair".to_string(),
                            value: attr(name("step"), "_0"),
                        },
                        PyStmt::Yield(PyExpr::Subscript {
                            value: Box::new(name("pair")),
                            index: Box::new(PyExpr::Int(0)),
                        }),
                        PyStmt::Assign {
                            target: "state".to_string(),
                            value: PyExpr::Subscript {
                                value: Box::new(name("pair")),
                                index: Box::new(PyExpr::Int(1)),
                            },
                        },
                    ],
                }],
            ),
            // ---- Seq: the consuming half ----
            // Seq.len(xs) -> len(list(xs))   (forces; there is no length without
            // walking an iterator)
            "_pf_seq_len" => def1(
                "_pf_seq_len",
                &["xs"],
                call("len", vec![call("list", vec![name("xs")])]),
            ),
            // Seq.head(xs) -> next(map(Some, xs), None_())
            "_pf_seq_head" => def1(
                "_pf_seq_head",
                &["xs"],
                call(
                    "next",
                    vec![
                        call("map", vec![name("Some"), name("xs")]),
                        call("None_", vec![]),
                    ],
                ),
            ),
            // Seq.find(f, xs) -> next(map(Some, filter(f, xs)), None_())
            "_pf_seq_find" => def1(
                "_pf_seq_find",
                &["f", "xs"],
                call(
                    "next",
                    vec![
                        call(
                            "map",
                            vec![name("Some"), call("filter", vec![name("f"), name("xs")])],
                        ),
                        call("None_", vec![]),
                    ],
                ),
            ),
            // Seq.isEmpty(xs) -> not isinstance(next(map(Some, xs), None_()), Some)
            // One element is pulled to find out, which is the honest cost of asking
            // an iterator whether it is empty.
            "_pf_seq_is_empty" => def1(
                "_pf_seq_is_empty",
                &["xs"],
                PyExpr::Not(Box::new(call(
                    "isinstance",
                    vec![
                        call(
                            "next",
                            vec![
                                call("map", vec![name("Some"), name("xs")]),
                                call("None_", vec![]),
                            ],
                        ),
                        name("Some"),
                    ],
                ))),
            ),
            // Seq.exists(f, xs) -> any(map(f, xs))   (short-circuits)
            "_pf_seq_exists" => def1(
                "_pf_seq_exists",
                &["f", "xs"],
                call("any", vec![call("map", vec![name("f"), name("xs")])]),
            ),
            // Seq.forall(f, xs) -> all(map(f, xs))   (short-circuits)
            "_pf_seq_forall" => def1(
                "_pf_seq_forall",
                &["f", "xs"],
                call("all", vec![call("map", vec![name("f"), name("xs")])]),
            ),
            // Seq.contains(x, xs) -> x in xs   (short-circuits on an iterator)
            "_pf_seq_contains" => def1(
                "_pf_seq_contains",
                &["x", "xs"],
                binop(PyBinOp::In, name("x"), name("xs")),
            ),
            // Seq.iter(f, xs): run f for its effect over the whole sequence.
            "_pf_seq_iter" => def(
                "_pf_seq_iter",
                &["f", "xs"],
                vec![
                    PyStmt::For {
                        target: "x".to_string(),
                        iter: name("xs"),
                        body: vec![PyStmt::Expr(PyExpr::Call {
                            func: Box::new(name("f")),
                            args: vec![name("x")],
                        })],
                    },
                    PyStmt::Return(PyExpr::NoneLit),
                ],
            ),
            // ---- Set: the traversal half (a Python `set`) ----
            // Set.isEmpty(s) -> not s
            "_pf_set_is_empty" => {
                def1("_pf_set_is_empty", &["s"], PyExpr::Not(Box::new(name("s"))))
            }
            // Set.map(f, s) -> set(map(f, s))
            "_pf_set_map" => def1(
                "_pf_set_map",
                &["f", "s"],
                call("set", vec![call("map", vec![name("f"), name("s")])]),
            ),
            // Set.filter(f, s) -> set(filter(f, s))
            "_pf_set_filter" => def1(
                "_pf_set_filter",
                &["f", "s"],
                call("set", vec![call("filter", vec![name("f"), name("s")])]),
            ),
            // Set.exists(f, s) -> any(map(f, s))
            "_pf_set_exists" => def1(
                "_pf_set_exists",
                &["f", "s"],
                call("any", vec![call("map", vec![name("f"), name("s")])]),
            ),
            // Set.forall(f, s) -> all(map(f, s))
            "_pf_set_forall" => def1(
                "_pf_set_forall",
                &["f", "s"],
                call("all", vec![call("map", vec![name("f"), name("s")])]),
            ),
            // Set.partition(f, s) -> (passing, failing), testing each element once
            "_pf_set_partition" => def(
                "_pf_set_partition",
                &["f", "s"],
                vec![
                    PyStmt::Assign {
                        target: "yes".to_string(),
                        value: call("set", vec![]),
                    },
                    PyStmt::Assign {
                        target: "no".to_string(),
                        value: call("set", vec![]),
                    },
                    PyStmt::For {
                        target: "x".to_string(),
                        iter: name("s"),
                        body: vec![PyStmt::If {
                            test: PyExpr::Call {
                                func: Box::new(name("f")),
                                args: vec![name("x")],
                            },
                            body: vec![PyStmt::Expr(method(name("yes"), "add", vec![name("x")]))],
                            orelse: vec![PyStmt::Expr(method(name("no"), "add", vec![name("x")]))],
                        }],
                    },
                    PyStmt::Return(PyExpr::Tuple(vec![name("yes"), name("no")])),
                ],
            ),
            // Set.isSubset(a, b) -> a.issubset(b)
            "_pf_set_is_subset" => def1(
                "_pf_set_is_subset",
                &["a", "b"],
                method(name("a"), "issubset", vec![name("b")]),
            ),
            // Set.isSuperset(a, b) -> a.issuperset(b)
            "_pf_set_is_superset" => def1(
                "_pf_set_is_superset",
                &["a", "b"],
                method(name("a"), "issuperset", vec![name("b")]),
            ),
            // Set.max(s) -> Some(max(s)) if s else None_()
            "_pf_set_max" => def1(
                "_pf_set_max",
                &["s"],
                PyExpr::IfExp {
                    body: Box::new(call("Some", vec![call("max", vec![name("s")])])),
                    test: Box::new(name("s")),
                    orelse: Box::new(call("None_", vec![])),
                },
            ),
            // Set.min(s) -> Some(min(s)) if s else None_()
            "_pf_set_min" => def1(
                "_pf_set_min",
                &["s"],
                PyExpr::IfExp {
                    body: Box::new(call("Some", vec![call("min", vec![name("s")])])),
                    test: Box::new(name("s")),
                    orelse: Box::new(call("None_", vec![])),
                },
            ),
            // ---- Map: the traversal half (a Python `dict`) ----
            // Every one takes the key *and* the value, since a map's element is the
            // pair, and every one walks `m.items()` in insertion order.
            // Map.isEmpty(m) -> not m
            "_pf_map_is_empty" => {
                def1("_pf_map_is_empty", &["m"], PyExpr::Not(Box::new(name("m"))))
            }
            // Map.map(f, m): fresh dict, same keys, f(k, v) values
            "_pf_map_map" => def(
                "_pf_map_map",
                &["f", "m"],
                vec![
                    PyStmt::Assign {
                        target: "out".to_string(),
                        value: call("dict", vec![]),
                    },
                    PyStmt::For {
                        target: "kv".to_string(),
                        iter: method(name("m"), "items", vec![]),
                        body: vec![PyStmt::SubscriptAssign {
                            obj: name("out"),
                            index: PyExpr::Subscript {
                                value: Box::new(name("kv")),
                                index: Box::new(PyExpr::Int(0)),
                            },
                            value: PyExpr::Call {
                                func: Box::new(name("f")),
                                args: vec![
                                    PyExpr::Subscript {
                                        value: Box::new(name("kv")),
                                        index: Box::new(PyExpr::Int(0)),
                                    },
                                    PyExpr::Subscript {
                                        value: Box::new(name("kv")),
                                        index: Box::new(PyExpr::Int(1)),
                                    },
                                ],
                            },
                        }],
                    },
                    PyStmt::Return(name("out")),
                ],
            ),
            // Map.filter(f, m): the entries whose key and value pass
            "_pf_map_filter" => def(
                "_pf_map_filter",
                &["f", "m"],
                vec![
                    PyStmt::Assign {
                        target: "out".to_string(),
                        value: call("dict", vec![]),
                    },
                    PyStmt::For {
                        target: "kv".to_string(),
                        iter: method(name("m"), "items", vec![]),
                        body: vec![PyStmt::If {
                            test: PyExpr::Call {
                                func: Box::new(name("f")),
                                args: vec![
                                    PyExpr::Subscript {
                                        value: Box::new(name("kv")),
                                        index: Box::new(PyExpr::Int(0)),
                                    },
                                    PyExpr::Subscript {
                                        value: Box::new(name("kv")),
                                        index: Box::new(PyExpr::Int(1)),
                                    },
                                ],
                            },
                            body: vec![PyStmt::SubscriptAssign {
                                obj: name("out"),
                                index: PyExpr::Subscript {
                                    value: Box::new(name("kv")),
                                    index: Box::new(PyExpr::Int(0)),
                                },
                                value: PyExpr::Subscript {
                                    value: Box::new(name("kv")),
                                    index: Box::new(PyExpr::Int(1)),
                                },
                            }],
                            orelse: vec![],
                        }],
                    },
                    PyStmt::Return(name("out")),
                ],
            ),
            // Map.fold(f, acc, m) -> acc threaded through f(acc, k, v)
            "_pf_map_fold" => def(
                "_pf_map_fold",
                &["f", "acc", "m"],
                vec![
                    PyStmt::For {
                        target: "kv".to_string(),
                        iter: method(name("m"), "items", vec![]),
                        body: vec![PyStmt::Assign {
                            target: "acc".to_string(),
                            value: PyExpr::Call {
                                func: Box::new(name("f")),
                                args: vec![
                                    name("acc"),
                                    PyExpr::Subscript {
                                        value: Box::new(name("kv")),
                                        index: Box::new(PyExpr::Int(0)),
                                    },
                                    PyExpr::Subscript {
                                        value: Box::new(name("kv")),
                                        index: Box::new(PyExpr::Int(1)),
                                    },
                                ],
                            },
                        }],
                    },
                    PyStmt::Return(name("acc")),
                ],
            ),
            // Map.exists(f, m) / Map.forall(f, m): short-circuiting walks
            "_pf_map_exists" => def(
                "_pf_map_exists",
                &["f", "m"],
                vec![
                    PyStmt::For {
                        target: "kv".to_string(),
                        iter: method(name("m"), "items", vec![]),
                        body: vec![PyStmt::If {
                            test: PyExpr::Call {
                                func: Box::new(name("f")),
                                args: vec![
                                    PyExpr::Subscript {
                                        value: Box::new(name("kv")),
                                        index: Box::new(PyExpr::Int(0)),
                                    },
                                    PyExpr::Subscript {
                                        value: Box::new(name("kv")),
                                        index: Box::new(PyExpr::Int(1)),
                                    },
                                ],
                            },
                            body: vec![PyStmt::Return(PyExpr::Bool(true))],
                            orelse: vec![],
                        }],
                    },
                    PyStmt::Return(PyExpr::Bool(false)),
                ],
            ),
            "_pf_map_forall" => def(
                "_pf_map_forall",
                &["f", "m"],
                vec![
                    PyStmt::For {
                        target: "kv".to_string(),
                        iter: method(name("m"), "items", vec![]),
                        body: vec![PyStmt::If {
                            test: PyExpr::Not(Box::new(PyExpr::Call {
                                func: Box::new(name("f")),
                                args: vec![
                                    PyExpr::Subscript {
                                        value: Box::new(name("kv")),
                                        index: Box::new(PyExpr::Int(0)),
                                    },
                                    PyExpr::Subscript {
                                        value: Box::new(name("kv")),
                                        index: Box::new(PyExpr::Int(1)),
                                    },
                                ],
                            })),
                            body: vec![PyStmt::Return(PyExpr::Bool(false))],
                            orelse: vec![],
                        }],
                    },
                    PyStmt::Return(PyExpr::Bool(true)),
                ],
            ),
            // Map.partition(f, m) -> (passing, failing), testing each entry once
            "_pf_map_partition" => def(
                "_pf_map_partition",
                &["f", "m"],
                vec![
                    PyStmt::Assign {
                        target: "yes".to_string(),
                        value: call("dict", vec![]),
                    },
                    PyStmt::Assign {
                        target: "no".to_string(),
                        value: call("dict", vec![]),
                    },
                    PyStmt::For {
                        target: "kv".to_string(),
                        iter: method(name("m"), "items", vec![]),
                        body: vec![PyStmt::If {
                            test: PyExpr::Call {
                                func: Box::new(name("f")),
                                args: vec![
                                    PyExpr::Subscript {
                                        value: Box::new(name("kv")),
                                        index: Box::new(PyExpr::Int(0)),
                                    },
                                    PyExpr::Subscript {
                                        value: Box::new(name("kv")),
                                        index: Box::new(PyExpr::Int(1)),
                                    },
                                ],
                            },
                            body: vec![PyStmt::SubscriptAssign {
                                obj: name("yes"),
                                index: PyExpr::Subscript {
                                    value: Box::new(name("kv")),
                                    index: Box::new(PyExpr::Int(0)),
                                },
                                value: PyExpr::Subscript {
                                    value: Box::new(name("kv")),
                                    index: Box::new(PyExpr::Int(1)),
                                },
                            }],
                            orelse: vec![PyStmt::SubscriptAssign {
                                obj: name("no"),
                                index: PyExpr::Subscript {
                                    value: Box::new(name("kv")),
                                    index: Box::new(PyExpr::Int(0)),
                                },
                                value: PyExpr::Subscript {
                                    value: Box::new(name("kv")),
                                    index: Box::new(PyExpr::Int(1)),
                                },
                            }],
                        }],
                    },
                    PyStmt::Return(PyExpr::Tuple(vec![name("yes"), name("no")])),
                ],
            ),
            // Map.union(a, b): a fresh dict, b winning a shared key
            "_pf_map_union" => def(
                "_pf_map_union",
                &["a", "b"],
                vec![
                    PyStmt::Assign {
                        target: "out".to_string(),
                        value: call("dict", vec![name("a")]),
                    },
                    PyStmt::Expr(method(name("out"), "update", vec![name("b")])),
                    PyStmt::Return(name("out")),
                ],
            ),
            // ---- String ----
            // String.isEmpty(s) -> not s
            "_pf_str_is_empty" => {
                def1("_pf_str_is_empty", &["s"], PyExpr::Not(Box::new(name("s"))))
            }
            // String.get(i, s) -> Some(s[i]) if 0 <= i < len(s) else None_()
            // Bounds-checked and total, like `List.get`: there is no `char` type, so
            // the answer is a one-character string.
            "_pf_str_get" => def1(
                "_pf_str_get",
                &["i", "s"],
                PyExpr::IfExp {
                    body: Box::new(call(
                        "Some",
                        vec![PyExpr::Subscript {
                            value: Box::new(name("s")),
                            index: Box::new(name("i")),
                        }],
                    )),
                    test: Box::new(PyExpr::Compare {
                        left: Box::new(PyExpr::Int(0)),
                        ops: vec![PyBinOp::Le, PyBinOp::Lt],
                        comparators: vec![name("i"), call("len", vec![name("s")])],
                    }),
                    orelse: Box::new(call("None_", vec![])),
                },
            ),
            // String.repeat(n, s) -> s * max(n, 0)
            "_pf_str_repeat" => def1(
                "_pf_str_repeat",
                &["n", "s"],
                binop(
                    PyBinOp::Mul,
                    name("s"),
                    call("max", vec![name("n"), PyExpr::Int(0)]),
                ),
            ),
            // String.trimStart(s) -> s.lstrip()
            "_pf_str_trim_start" => def1(
                "_pf_str_trim_start",
                &["s"],
                method(name("s"), "lstrip", vec![]),
            ),
            // String.trimEnd(s) -> s.rstrip()
            "_pf_str_trim_end" => def1(
                "_pf_str_trim_end",
                &["s"],
                method(name("s"), "rstrip", vec![]),
            ),
            // String.splitLines(s) -> s.splitlines()
            "_pf_str_split_lines" => def1(
                "_pf_str_split_lines",
                &["s"],
                method(name("s"), "splitlines", vec![]),
            ),
            // String.rev(s) -> "".join(reversed(s))
            "_pf_str_rev" => def1(
                "_pf_str_rev",
                &["s"],
                method(
                    PyExpr::Str(String::new()),
                    "join",
                    vec![call("reversed", vec![name("s")])],
                ),
            ),
            // String.ofList(xs) -> "".join(xs)
            "_pf_str_of_list" => def1(
                "_pf_str_of_list",
                &["xs"],
                method(PyExpr::Str(String::new()), "join", vec![name("xs")]),
            ),
            // ---- Option / Result: combining and bridging ----
            // Option.map2(f, a, b) -> Some(f(a._0, b._0)) when both are Some
            "_pf_opt_map2" => def(
                "_pf_opt_map2",
                &["f", "a", "b"],
                vec![
                    PyStmt::If {
                        test: binop(PyBinOp::And, is_some("a"), is_some("b")),
                        body: vec![PyStmt::Return(call(
                            "Some",
                            vec![PyExpr::Call {
                                func: Box::new(name("f")),
                                args: vec![attr(name("a"), "_0"), attr(name("b"), "_0")],
                            }],
                        ))],
                        orelse: vec![],
                    },
                    PyStmt::Return(call("None_", vec![])),
                ],
            ),
            // Option.orElse(fallback, o) -> o if it has a payload, else fallback
            "_pf_opt_or_else" => def1(
                "_pf_opt_or_else",
                &["fallback", "o"],
                PyExpr::IfExp {
                    body: Box::new(name("o")),
                    test: Box::new(is_some("o")),
                    orelse: Box::new(name("fallback")),
                },
            ),
            // Option.flatten(o) -> o._0 if isinstance(o, Some) else None_()
            // The payload is itself an Option, so this is the payload as-is.
            "_pf_opt_flatten" => def1(
                "_pf_opt_flatten",
                &["o"],
                PyExpr::IfExp {
                    body: Box::new(attr(name("o"), "_0")),
                    test: Box::new(is_some("o")),
                    orelse: Box::new(call("None_", vec![])),
                },
            ),
            // Option.iter(f, o): run f on the payload, if there is one
            "_pf_opt_iter" => def(
                "_pf_opt_iter",
                &["f", "o"],
                vec![
                    PyStmt::If {
                        test: is_some("o"),
                        body: vec![PyStmt::Expr(PyExpr::Call {
                            func: Box::new(name("f")),
                            args: vec![attr(name("o"), "_0")],
                        })],
                        orelse: vec![],
                    },
                    PyStmt::Return(PyExpr::NoneLit),
                ],
            ),
            // Option.toList(o) -> [o._0] if isinstance(o, Some) else []
            "_pf_opt_to_list" => def1(
                "_pf_opt_to_list",
                &["o"],
                PyExpr::IfExp {
                    body: Box::new(PyExpr::List(vec![attr(name("o"), "_0")])),
                    test: Box::new(is_some("o")),
                    orelse: Box::new(PyExpr::List(vec![])),
                },
            ),
            // Option.exists(f, o) -> isinstance(o, Some) and f(o._0)
            "_pf_opt_exists" => def1(
                "_pf_opt_exists",
                &["f", "o"],
                binop(
                    PyBinOp::And,
                    is_some("o"),
                    PyExpr::Call {
                        func: Box::new(name("f")),
                        args: vec![attr(name("o"), "_0")],
                    },
                ),
            ),
            // Result.map2(f, a, b): Ok only when both are, else the *first* Error,
            // so the earliest failure is the one reported.
            "_pf_res_map2" => def(
                "_pf_res_map2",
                &["f", "a", "b"],
                vec![
                    PyStmt::If {
                        test: is_err("a"),
                        body: vec![PyStmt::Return(name("a"))],
                        orelse: vec![],
                    },
                    PyStmt::If {
                        test: is_err("b"),
                        body: vec![PyStmt::Return(name("b"))],
                        orelse: vec![],
                    },
                    PyStmt::Return(call(
                        "Ok",
                        vec![PyExpr::Call {
                            func: Box::new(name("f")),
                            args: vec![attr(name("a"), "_0"), attr(name("b"), "_0")],
                        }],
                    )),
                ],
            ),
            // Result.orElse(fallback, r) -> r if it is Ok, else fallback
            "_pf_res_or_else" => def1(
                "_pf_res_or_else",
                &["fallback", "r"],
                PyExpr::IfExp {
                    body: Box::new(name("r")),
                    test: Box::new(is_ok("r")),
                    orelse: Box::new(name("fallback")),
                },
            ),
            // Result.iter(f, r): run f on the Ok value; an Error does nothing
            "_pf_res_iter" => def(
                "_pf_res_iter",
                &["f", "r"],
                vec![
                    PyStmt::If {
                        test: is_ok("r"),
                        body: vec![PyStmt::Expr(PyExpr::Call {
                            func: Box::new(name("f")),
                            args: vec![attr(name("r"), "_0")],
                        })],
                        orelse: vec![],
                    },
                    PyStmt::Return(PyExpr::NoneLit),
                ],
            ),
            // Result.toList(r) -> [r._0] if isinstance(r, Ok) else []
            "_pf_res_to_list" => def1(
                "_pf_res_to_list",
                &["r"],
                PyExpr::IfExp {
                    body: Box::new(PyExpr::List(vec![attr(name("r"), "_0")])),
                    test: Box::new(is_ok("r")),
                    orelse: Box::new(PyExpr::List(vec![])),
                },
            ),
            // ---- from the FSharp.Core audit ----
            // Seq.distinctBy(f, xs): lazy, remembering the keys it has yielded
            "_pf_seq_distinct_by" => def(
                "_pf_seq_distinct_by",
                &["f", "xs"],
                vec![
                    PyStmt::Assign {
                        target: "seen".to_string(),
                        value: call("set", vec![]),
                    },
                    PyStmt::For {
                        target: "x".to_string(),
                        iter: name("xs"),
                        body: vec![
                            PyStmt::Assign {
                                target: "k".to_string(),
                                value: PyExpr::Call {
                                    func: Box::new(name("f")),
                                    args: vec![name("x")],
                                },
                            },
                            PyStmt::If {
                                test: PyExpr::Not(Box::new(binop(
                                    PyBinOp::In,
                                    name("k"),
                                    name("seen"),
                                ))),
                                body: vec![
                                    PyStmt::Expr(method(name("seen"), "add", vec![name("k")])),
                                    PyStmt::Yield(name("x")),
                                ],
                                orelse: vec![],
                            },
                        ],
                    },
                ],
            ),
            // Seq.replicate(n, x) -> itertools.repeat(x, max(n, 0))
            "_pf_seq_replicate" => def1(
                "_pf_seq_replicate",
                &["n", "x"],
                PyExpr::Call {
                    func: Box::new(attr(name("itertools"), "repeat")),
                    args: vec![name("x"), call("max", vec![name("n"), PyExpr::Int(0)])],
                },
            ),
            // Seq.sumBy(f, xs) -> sum(map(f, xs))
            "_pf_seq_sum_by" => def1(
                "_pf_seq_sum_by",
                &["f", "xs"],
                call("sum", vec![call("map", vec![name("f"), name("xs")])]),
            ),
            // Seq.get(i, xs) -> the element at i, consuming up to it
            "_pf_seq_get" => def1(
                "_pf_seq_get",
                &["i", "xs"],
                call(
                    "next",
                    vec![
                        call(
                            "map",
                            vec![
                                name("Some"),
                                PyExpr::Call {
                                    func: Box::new(attr(name("itertools"), "islice")),
                                    args: vec![
                                        name("xs"),
                                        call("max", vec![name("i"), PyExpr::Int(0)]),
                                        PyExpr::NoneLit,
                                    ],
                                },
                            ],
                        ),
                        call("None_", vec![]),
                    ],
                ),
            ),
            // Seq.last(xs): walk to the end, keeping the most recent
            "_pf_seq_last" => def(
                "_pf_seq_last",
                &["xs"],
                vec![
                    PyStmt::Assign {
                        target: "out".to_string(),
                        value: call("None_", vec![]),
                    },
                    PyStmt::For {
                        target: "x".to_string(),
                        iter: name("xs"),
                        body: vec![PyStmt::Assign {
                            target: "out".to_string(),
                            value: call("Some", vec![name("x")]),
                        }],
                    },
                    PyStmt::Return(name("out")),
                ],
            ),
            // Seq.max / Seq.min: force into a list first, so emptiness is knowable
            "_pf_seq_max" => def(
                "_pf_seq_max",
                &["xs"],
                vec![
                    PyStmt::Assign {
                        target: "items".to_string(),
                        value: call("list", vec![name("xs")]),
                    },
                    PyStmt::Return(PyExpr::IfExp {
                        body: Box::new(call("Some", vec![call("max", vec![name("items")])])),
                        test: Box::new(name("items")),
                        orelse: Box::new(call("None_", vec![])),
                    }),
                ],
            ),
            "_pf_seq_min" => def(
                "_pf_seq_min",
                &["xs"],
                vec![
                    PyStmt::Assign {
                        target: "items".to_string(),
                        value: call("list", vec![name("xs")]),
                    },
                    PyStmt::Return(PyExpr::IfExp {
                        body: Box::new(call("Some", vec![call("min", vec![name("items")])])),
                        test: Box::new(name("items")),
                        orelse: Box::new(call("None_", vec![])),
                    }),
                ],
            ),
            // Seq.reduce(f, xs): a fold with no seed, so empty has no answer
            "_pf_seq_reduce" => def(
                "_pf_seq_reduce",
                &["f", "xs"],
                vec![
                    PyStmt::Assign {
                        target: "items".to_string(),
                        value: call("list", vec![name("xs")]),
                    },
                    PyStmt::Return(PyExpr::IfExp {
                        body: Box::new(call(
                            "Some",
                            vec![PyExpr::Call {
                                func: Box::new(attr(name("functools"), "reduce")),
                                args: vec![name("f"), name("items")],
                            }],
                        )),
                        test: Box::new(name("items")),
                        orelse: Box::new(call("None_", vec![])),
                    }),
                ],
            ),
            // Set.iter(f, s) / Map.iter(f, m): run f for its effect
            "_pf_set_iter" => def(
                "_pf_set_iter",
                &["f", "s"],
                vec![
                    PyStmt::For {
                        target: "x".to_string(),
                        iter: name("s"),
                        body: vec![PyStmt::Expr(PyExpr::Call {
                            func: Box::new(name("f")),
                            args: vec![name("x")],
                        })],
                    },
                    PyStmt::Return(PyExpr::NoneLit),
                ],
            ),
            "_pf_map_iter" => def(
                "_pf_map_iter",
                &["f", "m"],
                vec![
                    PyStmt::For {
                        target: "kv".to_string(),
                        iter: method(name("m"), "items", vec![]),
                        body: vec![PyStmt::Expr(PyExpr::Call {
                            func: Box::new(name("f")),
                            args: vec![
                                PyExpr::Subscript {
                                    value: Box::new(name("kv")),
                                    index: Box::new(PyExpr::Int(0)),
                                },
                                PyExpr::Subscript {
                                    value: Box::new(name("kv")),
                                    index: Box::new(PyExpr::Int(1)),
                                },
                            ],
                        })],
                    },
                    PyStmt::Return(PyExpr::NoneLit),
                ],
            ),
            // Option.forall(f, o) -> not isinstance(o, Some) or f(o._0)
            // `None` passes, the empty case, exactly as `List.forall` over `[]`.
            "_pf_opt_forall" => def1(
                "_pf_opt_forall",
                &["f", "o"],
                binop(
                    PyBinOp::Or,
                    PyExpr::Not(Box::new(is_some("o"))),
                    PyExpr::Call {
                        func: Box::new(name("f")),
                        args: vec![attr(name("o"), "_0")],
                    },
                ),
            ),
            // Option.contains(x, o) -> isinstance(o, Some) and o._0 == x
            "_pf_opt_contains" => def1(
                "_pf_opt_contains",
                &["x", "o"],
                binop(
                    PyBinOp::And,
                    is_some("o"),
                    binop(PyBinOp::Eq, attr(name("o"), "_0"), name("x")),
                ),
            ),
            // Result.exists / forall / contains — an Error fails exists and passes
            // forall, matching Option's None.
            "_pf_res_exists" => def1(
                "_pf_res_exists",
                &["f", "r"],
                binop(
                    PyBinOp::And,
                    is_ok("r"),
                    PyExpr::Call {
                        func: Box::new(name("f")),
                        args: vec![attr(name("r"), "_0")],
                    },
                ),
            ),
            "_pf_res_forall" => def1(
                "_pf_res_forall",
                &["f", "r"],
                binop(
                    PyBinOp::Or,
                    PyExpr::Not(Box::new(is_ok("r"))),
                    PyExpr::Call {
                        func: Box::new(name("f")),
                        args: vec![attr(name("r"), "_0")],
                    },
                ),
            ),
            "_pf_res_contains" => def1(
                "_pf_res_contains",
                &["x", "r"],
                binop(
                    PyBinOp::And,
                    is_ok("r"),
                    binop(PyBinOp::Eq, attr(name("r"), "_0"), name("x")),
                ),
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
        Pattern::Record { fields, .. } => fields
            .iter()
            .flat_map(|f| pattern_bindings(&f.pattern))
            .collect(),
        Pattern::Tuple { elems } => elems.iter().flat_map(pattern_bindings).collect(),
        // A list pattern binds its prefix/suffix elements' vars plus the rest binder.
        Pattern::List {
            prefix,
            rest,
            suffix,
        } => {
            let mut v: Vec<String> = prefix.iter().flat_map(pattern_bindings).collect();
            if let Some(r) = rest {
                v.extend(pattern_bindings(r));
            }
            v.extend(suffix.iter().flat_map(pattern_bindings));
            v
        }
        // Every alternative binds the same variables (enforced by the checker), so
        // the first alternative's bindings are representative.
        Pattern::Or(alts) => alts.first().map(pattern_bindings).unwrap_or_default(),
        // `p as x` binds `x` plus whatever `p` binds.
        Pattern::As { pattern, name, .. } => {
            let mut v = pattern_bindings(pattern);
            v.push(name.clone());
            v
        }
        _ => vec![],
    }
}

/// A `match` is exhaustive at lowering time only if some *unguarded* arm is
/// irrefutable (a wildcard, a variable, a record pattern with all-irrefutable
/// fields, or an or-pattern with an irrefutable alternative). A guarded arm can
/// fail at runtime, so it never makes the match exhaustive (`DESIGN.md` §7.2).
fn has_catch_all(arms: &[crate::parser::ast::MatchArm]) -> bool {
    arms.iter()
        .any(|arm| arm.guard.is_none() && is_irrefutable(&arm.pattern))
}

fn is_irrefutable(pattern: &Pattern) -> bool {
    match pattern {
        Pattern::Wildcard | Pattern::Var { .. } => true,
        Pattern::Record { fields, .. } => fields.iter().all(|f| is_irrefutable(&f.pattern)),
        Pattern::Tuple { elems } => elems.iter().all(is_irrefutable),
        // A list pattern is irrefutable only when it is a lone star `[*rest]`/`[*_]`
        // (which matches any list); `[]`, fixed lengths, and any prefix/suffix
        // elements (which require a minimum length) are refutable.
        Pattern::List {
            prefix,
            rest,
            suffix,
        } => prefix.is_empty() && suffix.is_empty() && rest.as_deref().is_some_and(is_irrefutable),
        Pattern::Or(alts) => alts.iter().any(is_irrefutable),
        // The `x` binding is irrefutable; refutability is the inner pattern's.
        Pattern::As { pattern, .. } => is_irrefutable(pattern),
        Pattern::Int(_) | Pattern::Str(_) | Pattern::Bool(_) | Pattern::Ctor { .. } => false,
    }
}

/// Walk a function body's *own* scope (not descending into nested functions),
/// collecting `<-` reassignment targets into `assigned` and `let`-bound names into
/// `bound`. Python has no block scope, so `if`/`match`/nested blocks are the same
/// function scope; a nested `fun`, a parameterized `let` (a nested function), and a
/// CE (its own generator/coroutine) introduce new scopes and are not entered.
fn scan_scope(expr: &Expr, assigned: &mut HashSet<String>, bound: &mut HashSet<String>) {
    match &expr.kind {
        ExprKind::Assign { target, value } => {
            assigned.insert(target.clone());
            scan_scope(value, assigned, bound);
        }
        ExprKind::Block { stmts } => {
            for stmt in stmts {
                match stmt {
                    BlockStmt::Let(b) => {
                        bound.insert(b.name.clone());
                        // A value binding's RHS is in this scope; a nested function's
                        // body (params > 0) is its own scope — don't enter it.
                        if b.params.is_empty() {
                            scan_scope(&b.value, assigned, bound);
                        }
                    }
                    BlockStmt::Expr(e) => scan_scope(e, assigned, bound),
                }
            }
        }
        ExprKind::If { cond, then, else_ } => {
            scan_scope(cond, assigned, bound);
            scan_scope(then, assigned, bound);
            scan_scope(else_, assigned, bound);
        }
        ExprKind::Match { scrutinee, arms } => {
            scan_scope(scrutinee, assigned, bound);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    scan_scope(guard, assigned, bound);
                }
                scan_scope(&arm.body, assigned, bound);
            }
        }
        ExprKind::App { func, arg } => {
            scan_scope(func, assigned, bound);
            scan_scope(arg, assigned, bound);
        }
        ExprKind::Pipe { lhs, rhs, .. } | ExprKind::Compose { lhs, rhs, .. } => {
            scan_scope(lhs, assigned, bound);
            scan_scope(rhs, assigned, bound);
        }
        ExprKind::Binary { lhs, rhs, .. } => {
            scan_scope(lhs, assigned, bound);
            scan_scope(rhs, assigned, bound);
        }
        ExprKind::Unary { expr, .. } => scan_scope(expr, assigned, bound),
        ExprKind::Compare { first, rest } => {
            scan_scope(first, assigned, bound);
            for (_, operand) in rest {
                scan_scope(operand, assigned, bound);
            }
        }
        ExprKind::Try { body } => scan_scope(body, assigned, bound),
        ExprKind::Annot { value, .. } => scan_scope(value, assigned, bound),
        ExprKind::List { elems } | ExprKind::Tuple { elems } => {
            for e in elems {
                scan_scope(e, assigned, bound);
            }
        }
        ExprKind::Interp { parts } => {
            for part in parts {
                if let InterpPart::Expr(e) = part {
                    scan_scope(e, assigned, bound);
                }
            }
        }
        ExprKind::Record { fields, .. } => {
            for f in fields {
                scan_scope(&f.value, assigned, bound);
            }
        }
        ExprKind::RecordUpdate { base, fields } => {
            scan_scope(base, assigned, bound);
            for f in fields {
                scan_scope(&f.value, assigned, bound);
            }
        }
        ExprKind::Field { base, .. } => scan_scope(base, assigned, bound),
        // New scopes (not entered) and leaves.
        ExprKind::Fn { .. }
        | ExprKind::Ce { .. }
        | ExprKind::Int(_)
        | ExprKind::Float(_)
        | ExprKind::Str(_)
        | ExprKind::Bool(_)
        | ExprKind::Unit
        | ExprKind::OpFunc(_)
        | ExprKind::Hole { .. }
        | ExprKind::Var(_) => {}
    }
}

/// Collect every binder name in an expression tree, ENTERING nested scopes
/// (functions, lambdas, CE bodies) — unlike [`scan_scope`], which stops at
/// scope boundaries because it feeds per-scope machinery. This feeds the
/// module-alias shadow check ([`Lowerer::py_module_ref`]), where a deliberate
/// whole-module overapproximation is what keeps the check simple and total:
/// block `let`s, match-pattern captures, lambda/function parameters, and CE
/// binders all lower to Python assignments or parameters that shadow a plain
/// `import <name>` somewhere.
fn collect_binders(expr: &Expr, out: &mut HashSet<String>) {
    match &expr.kind {
        ExprKind::Assign { value, .. } => collect_binders(value, out),
        ExprKind::Block { stmts } => {
            for stmt in stmts {
                match stmt {
                    BlockStmt::Let(b) => {
                        out.insert(b.name.clone());
                        out.extend(param_names(&b.params));
                        collect_binders(&b.value, out);
                    }
                    BlockStmt::Expr(e) => collect_binders(e, out),
                }
            }
        }
        ExprKind::If { cond, then, else_ } => {
            collect_binders(cond, out);
            collect_binders(then, out);
            collect_binders(else_, out);
        }
        ExprKind::Match { scrutinee, arms } => {
            collect_binders(scrutinee, out);
            for arm in arms {
                out.extend(pattern_bindings(&arm.pattern));
                if let Some(guard) = &arm.guard {
                    collect_binders(guard, out);
                }
                collect_binders(&arm.body, out);
            }
        }
        ExprKind::App { func, arg } => {
            collect_binders(func, out);
            collect_binders(arg, out);
        }
        ExprKind::Pipe { lhs, rhs, .. } | ExprKind::Compose { lhs, rhs, .. } => {
            collect_binders(lhs, out);
            collect_binders(rhs, out);
        }
        ExprKind::Binary { lhs, rhs, .. } => {
            collect_binders(lhs, out);
            collect_binders(rhs, out);
        }
        ExprKind::Unary { expr, .. } => collect_binders(expr, out),
        ExprKind::Compare { first, rest } => {
            collect_binders(first, out);
            for (_, operand) in rest {
                collect_binders(operand, out);
            }
        }
        ExprKind::Try { body } => collect_binders(body, out),
        ExprKind::Annot { value, .. } => collect_binders(value, out),
        ExprKind::List { elems } | ExprKind::Tuple { elems } => {
            for e in elems {
                collect_binders(e, out);
            }
        }
        ExprKind::Interp { parts } => {
            for part in parts {
                if let InterpPart::Expr(e) = part {
                    collect_binders(e, out);
                }
            }
        }
        ExprKind::Record { fields, .. } => {
            for f in fields {
                collect_binders(&f.value, out);
            }
        }
        ExprKind::RecordUpdate { base, fields } => {
            collect_binders(base, out);
            for f in fields {
                collect_binders(&f.value, out);
            }
        }
        ExprKind::Field { base, .. } => collect_binders(base, out),
        ExprKind::Fn { params, body } => {
            out.extend(param_names(params));
            collect_binders(body, out);
        }
        ExprKind::Ce { items, .. } => {
            for item in items {
                match item {
                    CeItem::LetBang { name, value, .. } | CeItem::Let { name, value, .. } => {
                        out.insert(name.clone());
                        collect_binders(value, out);
                    }
                    CeItem::DoBang(e)
                    | CeItem::Return(e)
                    | CeItem::ReturnBang(e)
                    | CeItem::Yield(e)
                    | CeItem::YieldBang(e) => collect_binders(e, out),
                }
            }
        }
        ExprKind::Int(_)
        | ExprKind::Float(_)
        | ExprKind::Str(_)
        | ExprKind::Bool(_)
        | ExprKind::Unit
        | ExprKind::OpFunc(_)
        | ExprKind::Hole { .. }
        | ExprKind::Var(_) => {}
    }
}

fn non_exhaustive_guard() -> PyCase {
    PyCase {
        pattern: PyPattern::Wildcard,
        guard: None,
        body: vec![PyStmt::RaiseRuntimeError(
            "non-exhaustive match".to_string(),
        )],
    }
}

/// Whether a lowered case matches unconditionally: guard-free with a pattern
/// Python itself deems irrefutable (a bare capture or wildcard). Newtype erasure
/// can turn a source-refutable constructor pattern (`case UserId s:`) into a bare
/// capture, and Python rejects any `case` after an irrefutable one as a
/// SyntaxError ("makes remaining patterns unreachable") — so this must be judged
/// on the *lowered* pattern, not the source arm.
fn py_catch_all(case: &PyCase) -> bool {
    case.guard.is_none() && py_irrefutable(&case.pattern)
}

fn py_irrefutable(pattern: &PyPattern) -> bool {
    match pattern {
        PyPattern::Wildcard | PyPattern::Capture(_) => true,
        PyPattern::As { pattern, .. } => py_irrefutable(pattern),
        PyPattern::Or(alts) => alts.iter().any(py_irrefutable),
        // Class/sequence/literal patterns are never syntactically irrefutable to
        // Python, whatever the checker proved about them.
        _ => false,
    }
}

/// Finalize a lowered `match`'s case list: drop cases made unreachable by an
/// earlier unconditional case (Python would reject them), and append the
/// defensive non-exhaustive raise only when neither the lowered cases nor the
/// source arms already catch everything.
fn seal_cases(arms: &[crate::parser::ast::MatchArm], cases: &mut Vec<PyCase>) {
    if let Some(i) = cases.iter().position(py_catch_all) {
        cases.truncate(i + 1);
    } else if !has_catch_all(arms) {
        cases.push(non_exhaustive_guard());
    }
}

fn extend(base: &HashSet<String>, names: &[String]) -> HashSet<String> {
    let mut out = base.clone();
    out.extend(names.iter().cloned());
    out
}

/// The names of a parameter list — parameters lower to plain Python argument
/// names; their source spans (carried for the LSP) are erased here. A
/// *destructuring* parameter has no name of its own, so it takes a synthetic one
/// that [`destructure_params`] unpacks at the top of the body.
fn param_names(params: &[Param]) -> Vec<String> {
    let mut wildcard_used = false;
    params
        .iter()
        .enumerate()
        .map(|(i, p)| match p.name() {
            Some(name) => name.to_string(),
            // The first `_` parameter keeps Python's own throwaway name; a second
            // one cannot (duplicate argument names are a SyntaxError).
            None if matches!(p.pattern, Pattern::Wildcard) && !wildcard_used => {
                wildcard_used = true;
                "_".to_string()
            }
            None => format!("_pf_arg{i}"),
        })
        .collect()
}

/// Every Pyfun name a parameter list binds: the plain parameters plus the names
/// inside each destructuring pattern. This is the set the body can *refer* to,
/// which is what scope tracking needs — [`param_names`] is the narrower list of
/// Python argument names.
fn param_bindings(params: &[Param]) -> Vec<String> {
    let mut out = param_names(params);
    for p in params {
        if p.name().is_none() {
            out.extend(pattern_bindings(&p.pattern));
        }
    }
    out
}

/// The unpacking statements a destructuring parameter list needs at the top of its
/// body: `fun (t, sq) -> …` lowers to `def _f(_pf_arg0): t, sq = _pf_arg0; …`,
/// since Python 3 removed tuple parameters. A nested tuple unpacks through a temp
/// (`_pf_arg0_0`), one statement per level, which keeps each emitted line the
/// obvious Python for that level.
fn destructure_params(params: &[Param], args: &[String]) -> Vec<PyStmt> {
    let mut out = Vec::new();
    for (p, arg) in params.iter().zip(args) {
        if p.name().is_none() {
            unpack_into(&p.pattern, arg, &mut out);
        }
    }
    out
}

/// Emit the unpacking of `pattern` from the value already bound to `source`.
fn unpack_into(pattern: &Pattern, source: &str, out: &mut Vec<PyStmt>) {
    match pattern {
        // A tuple unpacks in one statement, the way Python spells it.
        Pattern::Tuple { elems } => {
            let mut targets = Vec::with_capacity(elems.len());
            let mut nested = Vec::new();
            for (i, elem) in elems.iter().enumerate() {
                match elem {
                    Pattern::Var { name, .. } => targets.push(py_value_name(name.as_str())),
                    Pattern::Wildcard => targets.push("_".to_string()),
                    _ => {
                        let temp = format!("{source}_{i}");
                        targets.push(temp.clone());
                        nested.push((elem, temp));
                    }
                }
            }
            out.push(PyStmt::UnpackAssign {
                targets,
                value: PyExpr::Name(py_value_name(source)),
            });
            for (elem, temp) in nested {
                unpack_into(elem, &temp, out);
            }
        }
        // A record reads the fields it names, one attribute each — the same
        // attributes a `p.field` access reads, and only the named subset.
        Pattern::Record { fields, .. } => {
            for (i, field) in fields.iter().enumerate() {
                let value = PyExpr::Attribute {
                    value: Box::new(PyExpr::Name(py_value_name(source))),
                    attr: py_field_name(&field.name),
                };
                match &field.pattern {
                    Pattern::Wildcard => {}
                    Pattern::Var { name, .. } => out.push(PyStmt::Assign {
                        target: py_value_name(name.as_str()),
                        value,
                    }),
                    nested => {
                        let temp = format!("{source}_{i}");
                        out.push(PyStmt::Assign {
                            target: temp.clone(),
                            value,
                        });
                        unpack_into(nested, &temp, out);
                    }
                }
            }
        }
        // A `_` parameter binds nothing, and a plain name is the argument itself.
        _ => {}
    }
}

/// Emitted Python parameter names ([`py_value_name`] over each). Scope tracking
/// (`locals`, `scan_scope`, the fold pass) works in *Pyfun* name space, so names
/// are mangled only where they are written into the Python IR — never in the sets
/// those passes consult.
fn py_param_names(names: &[String]) -> Vec<String> {
    names.iter().map(|n| py_value_name(n)).collect()
}

/// Append `target = value` to a (possibly empty) statement list.
fn with_assign(mut stmts: Vec<PyStmt>, target: &str, value: PyExpr) -> Vec<PyStmt> {
    stmts.push(PyStmt::Assign {
        target: target.to_string(),
        value,
    });
    stmts
}
