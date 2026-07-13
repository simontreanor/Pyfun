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

mod fold_loop;

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
    let mut lowerer = Lowerer::new(module);
    lowerer.float_literals = float_literals.clone();
    lowerer.order = order;
    lowerer.lower_module(module)
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
    Ok(LoweredModule { py, uses_runtime })
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
    /// `extern` name → its pinned Python keyword arguments (already lowered to their
    /// `PyExpr` literals), appended to every emitted call (`open(path,
    /// encoding="utf-8")`). Under-application routes them through
    /// `functools.partial` so they are never dropped (`DESIGN.md` §6).
    extern_kwargs: std::collections::HashMap<String, Vec<(String, PyExpr)>>,
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
    /// Whether the in-place linear-fold optimization is enabled. Defaults to on;
    /// the `PYFUN_NO_FOLD_OPT` environment variable turns it off (a kill switch for
    /// differential testing — the rejected path is byte-identical to no-opt).
    fold_opt: bool,
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
        let mut extern_kwargs: std::collections::HashMap<String, Vec<(String, PyExpr)>> =
            std::collections::HashMap::new();
        let mut user_defs = HashSet::new();
        let mut top_fn_defs: HashMap<String, (Vec<Param>, Expr)> = HashMap::new();
        let mut ap_uses = std::collections::HashMap::new();
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
            arities,
            ap_uses,
            ctor_arity,
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
            // Default the optimization on; `PYFUN_NO_FOLD_OPT` (any value) disables
            // it. Read once per module lowering — cheap, and covers every entry
            // point (`compile`/`run`/project/tests) for differential testing.
            fold_opt: std::env::var_os("PYFUN_NO_FOLD_OPT").is_none(),
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
                            fields: fields.iter().map(|f| f.name.clone()).collect(),
                            field_types: fields.iter().map(|f| py_annotation(&f.ty)).collect(),
                            order: ordered.then_some(0),
                            record: true,
                        });
                    }
                    // An opaque handle type erases — it emits no Python class.
                    TypeDeclKind::Opaque => {}
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
                            if let Some(module) = self.extern_import_spec(&decl.target) {
                                self.needed_imports.insert(module);
                            }
                            nullary_lambda(&decl.target, kwargs)
                        } else {
                            if let Some(module) = self.extern_import_spec(&decl.target) {
                                self.needed_imports.insert(module);
                            }
                            // A plain extern with pinned kwargs binds to a
                            // `functools.partial` that carries them; otherwise to the
                            // bare dotted target.
                            if kwargs.is_empty() {
                                dotted_path(&decl.target)
                            } else {
                                self.build_call_kw(
                                    dotted_path(&decl.target),
                                    Some(arrow_arity(&decl.ty)),
                                    vec![],
                                    kwargs,
                                )
                            }
                        };
                        code.push(PyStmt::Assign {
                            target: decl.name.clone(),
                            value,
                        });
                    }
                }
                Item::Let(binding) => self.lower_let(binding, &HashSet::new(), &mut code)?,
                // The active-pattern recognizer lowers to a plain Python def
                // under its deterministic `_ap_…` name; inside its body the
                // (total) cases construct the hidden `_Case` classes via the
                // ordinary constructor path (registered in `new`).
                Item::ActivePattern(decl) => {
                    let names = param_names(&decl.params);
                    let inner = extend(&HashSet::new(), &names);
                    let body = self.lower_fn_body(&names, &decl.value, &inner)?;
                    code.push(PyStmt::FuncDef {
                        name: ap_py_fn(decl),
                        params: names,
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
            // A binding whose value already *is* the (already-assigned) target — an
            // in-place fold whose accumulator slot is named like the binding
            // (`let m = List.fold (fun m x -> …)`) — would emit a no-op `m = m`.
            // Suppress it: the hoisted statements have already bound the name.
            let redundant = !stmts.is_empty() && matches!(&value, PyExpr::Name(n) if n == name);
            out.append(&mut stmts);
            if !redundant {
                out.push(PyStmt::Assign {
                    target: name.to_string(),
                    value,
                });
            }
        } else {
            // A nested function captures the enclosing locals (Python closures),
            // so they count as locals when resolving names in its body.
            let names = param_names(&binding.params);
            let inner = extend(locals, &names);
            let body = self.lower_fn_body(&names, &binding.value, &inner)?;
            out.push(PyStmt::FuncDef {
                name: name.to_string(),
                params: names,
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
        params: &[String],
        body: &Expr,
        inner: &HashSet<String>,
    ) -> Result<Vec<PyStmt>, LowerError> {
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

        self.fn_local_stack.push(bound);
        let lowered = self.lower_return(body, inner);
        self.fn_local_stack.pop();
        let mut stmts = lowered?;

        let mut decls = Vec::new();
        if !globals.is_empty() {
            decls.push(PyStmt::Global(globals));
        }
        if !nonlocals.is_empty() {
            decls.push(PyStmt::Nonlocal(nonlocals));
        }
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
                    let arm_locals = extend(locals, &pattern_bindings(&arm.pattern));
                    let guard = self.lower_guard(&arm.guard, &arm_locals)?;
                    let body = self.lower_return(&arm.body, &arm_locals)?;
                    cases.push(PyCase {
                        pattern,
                        guard,
                        body,
                    });
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
                    let arm_locals = extend(locals, &pattern_bindings(&arm.pattern));
                    let guard = self.lower_guard(&arm.guard, &arm_locals)?;
                    let (arm_stmts, arm_val) = self.lower_value(&arm.body, &arm_locals)?;
                    cases.push(PyCase {
                        pattern,
                        guard,
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
                    let def_body = self.lower_fn_body(&names, body, &inner)?;
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
                attr: field.clone(),
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
            let recv = arg_vals.remove(0);
            let accessed = attr_path(recv, &member);
            let result = match kind {
                // Property: `recv.text`; any further args are over-application calls.
                Receiver::Property => arg_vals.into_iter().fold(accessed, |f, a| PyExpr::Call {
                    func: Box::new(f),
                    args: vec![a],
                }),
                // A method extern with pinned kwargs routes every arity through
                // `build_call_kw` so the kwargs are appended (full/over) or carried
                // by `functools.partial` (receiver-only / method-partial) — never lost.
                Receiver::Method if !kwargs.is_empty() => {
                    let method_arity = arity.map(|k| k.saturating_sub(1));
                    self.build_call_kw(accessed, method_arity, arg_vals, kwargs)
                }
                Receiver::Method => {
                    // The method itself takes one fewer argument than the arity.
                    let method_arity = arity.map(|k| k.saturating_sub(1));
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
            let target = self.extern_targets[name].clone();
            if let Some(module) = self.extern_import_spec(&target) {
                self.needed_imports.insert(module);
            }
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
                Some(kwargs) => PyExpr::CallKw {
                    func: Box::new(dotted_path(&target)),
                    args: vec![],
                    kwargs,
                },
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

        // A plain (non-receiver, non-nullary) extern that pins fixed Python kwargs:
        // `openText path` → `builtins.open(path, mode="rt", encoding="utf-8")`.
        // Full/over-application appends the kwargs to the direct call; under-
        // application carries them through `functools.partial` (`build_call_kw`), so
        // a partial or bare reference never silently drops them (`DESIGN.md` §6).
        if let ExprKind::Var(name) = &head.kind
            && !locals.contains(name)
            && !self.user_defs.contains(name)
            && let Some(kwargs) = self.extern_kwargs.get(name).cloned()
        {
            let target = self.extern_targets[name].clone();
            let arity = self.arities.get(name).copied();
            if let Some(module) = self.extern_import_spec(&target) {
                self.needed_imports.insert(module);
            }
            let mut stmts = Vec::new();
            let mut arg_vals = Vec::with_capacity(args_ast.len());
            for arg in &args_ast {
                let (arg_stmts, arg_val) = self.lower_value(arg, locals)?;
                stmts.extend(arg_stmts);
                arg_vals.push(arg_val);
            }
            let call = self.build_call_kw(dotted_path(&target), arity, arg_vals, kwargs);
            return Ok((stmts, call));
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
            return PyExpr::Name(format!("{m}_{name}"));
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
                let target = self.extern_targets[name].clone();
                if let Some(module) = self.extern_import_spec(&target) {
                    self.needed_imports.insert(module);
                }
                let kwargs = self.extern_kwargs.get(name).cloned().unwrap_or_default();
                return nullary_lambda(&target, kwargs);
            }
            // A bare reference to a plain extern that pins kwargs carries them via
            // `functools.partial` (`openText` → `functools.partial(builtins.open,
            // mode="rt", encoding="utf-8")`), so the kwargs survive later
            // application. Applied references are handled in `lower_application`.
            if let Some(kwargs) = self.extern_kwargs.get(name).cloned() {
                let target = self.extern_targets[name].clone();
                if let Some(module) = self.extern_import_spec(&target) {
                    self.needed_imports.insert(module);
                }
                let arity = self.arities.get(name).copied();
                return self.build_call_kw(dotted_path(&target), arity, vec![], kwargs);
            }
            // An `extern` reference lowers to its dotted Python target (e.g.
            // `math.sqrt`), recording any module that must be imported.
            if let Some(target) = self.extern_targets.get(name).cloned() {
                if let Some(module) = self.extern_import_spec(&target) {
                    self.needed_imports.insert(module);
                }
                return dotted_path(&target);
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
        match self.ctor_arity.get(name) {
            Some(0) => PyExpr::Call {
                func: Box::new(PyExpr::Name(py_ctor_name(name))),
                args: vec![],
            },
            Some(_) => PyExpr::Name(py_ctor_name(name)),
            None => PyExpr::Name(name.to_string()),
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
                let (base, member) = other.split_once('.').unwrap_or((other, ""));
                if self.imported_modules.contains(base) {
                    let module = base.to_lowercase();
                    self.needed_imports.insert(module.clone());
                    let attr = PyExpr::Attribute {
                        value: Box::new(PyExpr::Name(module)),
                        // A member may be a constructor (`Geometry.Circle`), so
                        // mangle it the same way its defining module did
                        // (`None` → `None_`); value members are unaffected.
                        attr: py_ctor_name(member),
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
                    PyExpr::Name(other.replace('.', "_"))
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
            let module = base.to_lowercase();
            self.needed_imports.insert(module.clone());
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
            let module = base.to_lowercase();
            self.needed_imports.insert(module.clone());
            format!("{module}.{}", py_record_class(rec))
        } else {
            py_record_class(tag)
        }
    }

    fn lower_pattern(&mut self, pattern: &Pattern) -> PyPattern {
        match pattern {
            Pattern::Wildcard => PyPattern::Wildcard,
            Pattern::Var { name, .. } => PyPattern::Capture(name.clone()),
            Pattern::Int(n) => PyPattern::Literal(PyExpr::Int(*n)),
            Pattern::Str(s) => PyPattern::Literal(PyExpr::Str(s.clone())),
            Pattern::Bool(b) => PyPattern::Literal(PyExpr::Bool(*b)),
            Pattern::Ctor { name, args, .. } => {
                if name == "Ok" || name == "Error" {
                    self.needs_result = true;
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
                    .map(|f| (f.name.clone(), self.lower_pattern(&f.pattern)))
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
                    Pattern::Var { name, .. } => name.clone(),
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
                name: name.clone(),
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

    /// Does any arm of this match use an active-pattern case at its top level?
    fn match_uses_ap(&self, arms: &[crate::parser::ast::MatchArm]) -> bool {
        arms.iter().any(
            |a| matches!(&a.pattern, Pattern::Ctor { name, .. } if self.ap_uses.contains_key(name)),
        )
    }

    /// Lower a `match` that uses active patterns to an honest **if/elif chain**
    /// (`DESIGN.md` §7.2) — an active pattern is a *function call*, not a
    /// structural test, so Python's `match` cannot express it. The scrutinee is
    /// evaluated once, and each **distinct** recognizer application (same
    /// function + same parameter arguments) is hoisted to a temp before the
    /// chain, so its side effects happen at most once per match. `assign_to`
    /// selects value position (arm bodies assign that temp) vs return position
    /// (arm bodies return). The checker's shape rules guarantee every arm is an
    /// active-pattern case, a literal, a variable, or `_`, with no guards.
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
        // A guard can fail *after* binding names, so a guarded match needs a
        // fall-through lowering (a forward `if`-sequence with early exit); the
        // guard-free shape keeps the flat if/elif chain below unchanged.
        if arms.iter().any(|a| a.guard.is_some()) {
            return self.lower_ap_match_seq(&subject, arms, locals, assign_to, stmts);
        }
        // (recognizer, lowered args, temp) per distinct hoisted application.
        let mut hoisted: Vec<(String, Vec<PyExpr>, String)> = Vec::new();
        // (condition, binder assigns, body) per arm; `None` = catch-all.
        let mut chain: Vec<(Option<PyExpr>, Vec<PyStmt>, Vec<PyStmt>)> = Vec::new();
        for arm in arms {
            let (cond, binds) =
                self.ap_arm_test(&arm.pattern, &subject, locals, &mut hoisted, &mut stmts)?;
            let arm_locals = extend(locals, &pattern_bindings(&arm.pattern));
            let body = match assign_to {
                None => self.lower_return(&arm.body, &arm_locals)?,
                Some(tmp) => {
                    let (s, v) = self.lower_value(&arm.body, &arm_locals)?;
                    with_assign(s, tmp, v)
                }
            };
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
            // The recognizer application is hoisted into this arm's own block, so
            // it runs only when the arm is reached (lazy).
            let mut arm_block: Vec<PyStmt> = Vec::new();
            let (cond, binds) =
                self.ap_arm_test(&arm.pattern, subject, locals, &mut hoisted, &mut arm_block)?;
            let arm_locals = extend(locals, &pattern_bindings(&arm.pattern));
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
                    target: name.clone(),
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

    /// Like [`Self::build_call`] but for an `extern` whose target pins fixed Python
    /// keyword arguments. The pinned `kwargs` ride along at every arity:
    /// full/over-application appends them to the direct call (`f(a, kw=v)`); under-
    /// application hands them to `functools.partial` (`functools.partial(f, a,
    /// kw=v)`), so a later application supplies the remaining positional arguments
    /// and the kwargs are never dropped.
    fn build_call_kw(
        &mut self,
        head: PyExpr,
        arity: Option<usize>,
        args: Vec<PyExpr>,
        kwargs: Vec<(String, PyExpr)>,
    ) -> PyExpr {
        let n = args.len();
        match arity {
            Some(k) if n < k => {
                // Partial application: `functools.partial` carries the positional
                // args *and* the pinned keyword args.
                self.needs_functools = true;
                let mut partial_args = Vec::with_capacity(n + 1);
                partial_args.push(head);
                partial_args.extend(args);
                PyExpr::CallKw {
                    func: Box::new(PyExpr::Attribute {
                        value: Box::new(PyExpr::Name("functools".to_string())),
                        attr: "partial".to_string(),
                    }),
                    args: partial_args,
                    kwargs,
                }
            }
            Some(k) if n > k => {
                // Over-application: full (kw-carrying) call, then apply the rest.
                let mut rest = args;
                let first = rest.drain(..k).collect();
                let mut call = PyExpr::CallKw {
                    func: Box::new(head),
                    args: first,
                    kwargs,
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
            _ => PyExpr::CallKw {
                func: Box::new(head),
                args,
                kwargs,
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
fn lower_extern_arg(arg: &ExternArg) -> PyExpr {
    match arg {
        ExternArg::Str(s) => PyExpr::Str(s.clone()),
        ExternArg::Int(n) if *n < 0 => PyExpr::Neg(Box::new(PyExpr::Int(-n))),
        ExternArg::Int(n) => PyExpr::Int(*n),
        ExternArg::Float(f) if *f < 0.0 => PyExpr::Neg(Box::new(PyExpr::Float(-f))),
        ExternArg::Float(f) => PyExpr::Float(*f),
        ExternArg::Bool(b) => PyExpr::Bool(*b),
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
    kwargs: Vec<(String, PyExpr)>,
) -> PyExpr {
    let recv = "_pf_recv".to_string();
    let accessed = attr_path(PyExpr::Name(recv.clone()), member);
    if kind == Receiver::Property {
        return PyExpr::Lambda {
            params: vec![recv],
            body: Box::new(accessed),
        };
    }
    let args: Vec<String> = (1..arity.max(1)).map(|i| format!("_pf_a{i}")).collect();
    let call_args: Vec<PyExpr> = args.iter().cloned().map(PyExpr::Name).collect();
    let body = if kwargs.is_empty() {
        PyExpr::Call {
            func: Box::new(accessed),
            args: call_args,
        }
    } else {
        PyExpr::CallKw {
            func: Box::new(accessed),
            args: call_args,
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
/// value works however it is later applied. Any pinned `kwargs` are appended.
fn nullary_lambda(target: &[String], kwargs: Vec<(String, PyExpr)>) -> PyExpr {
    let body = if kwargs.is_empty() {
        PyExpr::Call {
            func: Box::new(dotted_path(target)),
            args: vec![],
        }
    } else {
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

fn non_exhaustive_guard() -> PyCase {
    PyCase {
        pattern: PyPattern::Wildcard,
        guard: None,
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
