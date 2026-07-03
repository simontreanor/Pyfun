//! The multi-file driver for Phase 2 file-based modules (`DESIGN.md` §6.1).
//!
//! From an entry module it follows `import` edges, builds the dependency
//! **graph**, rejects cycles and missing files, and returns the modules in
//! **topological order** (every module after the ones it imports, the entry
//! last) — the order in which later slices type-check and emit them.
//!
//! The graph logic is decoupled from the filesystem: [`build`] takes a *loader*
//! closure mapping a module name to its source (so it is unit-testable with an
//! in-memory map), and [`build_from_path`] is the thin wrapper that resolves
//! names to `<root>/<name>.pyfun` files for the CLI.
//!
//! This slice resolves and orders the graph only; cross-module type-checking
//! (slice 3) and multi-file lowering/emit (slice 4) build on `Project`.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::CompileError;
use crate::parser::ast::{Item, Module};

/// One module, loaded, parsed, and with its import edges extracted.
#[derive(Debug)]
pub struct LoadedModule {
    /// The capitalized module name (`Geometry`), as written in `import`.
    pub name: String,
    /// The original source text (kept so diagnostics can be rendered against it).
    pub source: String,
    /// The parsed AST.
    pub ast: Module,
    /// The names this module imports, in source order.
    pub imports: Vec<String>,
}

/// A resolved project: its modules in **topological order** — each module
/// appears after every module it imports, and the entry module is last.
#[derive(Debug)]
pub struct Project {
    pub modules: Vec<LoadedModule>,
}

impl Project {
    /// The entry module (the last in topological order).
    pub fn entry(&self) -> &LoadedModule {
        self.modules
            .last()
            .expect("a project always has at least its entry module")
    }
}

/// A failure while resolving the module graph.
#[derive(Debug, Clone, PartialEq)]
pub enum ProjectError {
    /// A module's source could not be found. `importer` is the module whose
    /// `import` referenced it (`None` for the entry module itself).
    Missing {
        name: String,
        importer: Option<String>,
    },
    /// A module failed to lex or parse.
    Compile { name: String, error: CompileError },
    /// The import graph has a cycle; `names` lists it in order, the first module
    /// repeated at the end (`A -> B -> A`).
    Cycle { names: Vec<String> },
}

impl std::fmt::Display for ProjectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProjectError::Missing { name, importer } => {
                let file = module_file_name(name);
                match importer {
                    Some(by) => write!(
                        f,
                        "cannot find module `{name}` (expected `{file}`), imported by `{by}`"
                    ),
                    None => write!(f, "cannot find module `{name}` (expected `{file}`)"),
                }
            }
            ProjectError::Compile { name, error } => write!(f, "in module `{name}`: {error}"),
            ProjectError::Cycle { names } => {
                write!(f, "import cycle: {}", names.join(" -> "))
            }
        }
    }
}

impl std::error::Error for ProjectError {}

/// The source file name a module name resolves to: lowercase stem + `.pyfun`
/// (the flat, single-directory convention of `DESIGN.md` §6.1).
pub fn module_file_name(name: &str) -> String {
    format!("{}.pyfun", name.to_lowercase())
}

/// The module name for a source path: its file stem with the first letter
/// uppercased (`geometry.pyfun` → `Geometry`). `None` if the path has no stem.
pub fn module_name_from_path(path: &Path) -> Option<String> {
    Some(capitalize(path.file_stem()?.to_str()?))
}

/// Resolve the module graph rooted at `entry`, loading sources through `load`.
///
/// `load(name)` returns the module's source text, or `None` if it cannot be
/// found. The graph is walked depth-first: a back-edge to a module currently on
/// the path is a [`ProjectError::Cycle`], and the post-order of the walk is the
/// returned topological order (dependencies first, entry last).
pub fn build<F>(entry: &str, load: F) -> Result<Project, ProjectError>
where
    F: Fn(&str) -> Option<String>,
{
    let mut ctx = Builder {
        load,
        loaded: HashMap::new(),
        state: HashMap::new(),
        order: Vec::new(),
        stack: Vec::new(),
    };
    ctx.visit(entry, None)?;
    let mut loaded = ctx.loaded;
    let modules = ctx
        .order
        .into_iter()
        .map(|name| loaded.remove(&name).expect("every ordered name was loaded"))
        .collect();
    Ok(Project { modules })
}

/// Resolve the module graph rooted at a source file on disk, reading imported
/// modules from sibling `<name>.pyfun` files in the entry's directory.
pub fn build_from_path(entry: &Path) -> Result<Project, ProjectError> {
    let entry_name = module_name_from_path(entry).ok_or_else(|| ProjectError::Missing {
        name: entry.display().to_string(),
        importer: None,
    })?;
    let root = entry.parent().map(Path::to_path_buf).unwrap_or_default();
    build(&entry_name, |name| {
        std::fs::read_to_string(root.join(module_file_name(name))).ok()
    })
}

/// Type errors found in one module of a project.
#[derive(Debug)]
pub struct ModuleErrors {
    /// The module the errors were found in.
    pub name: String,
    pub errors: Vec<crate::types::TypeError>,
}

/// Type-check every module of `project` in topological order (`DESIGN.md` §6.1).
///
/// Each module is checked with its imports' exported value schemes seeded into
/// scope (so `Geometry.area` resolves), and its own exports are recorded for the
/// modules that depend on it — the topological order guarantees a module's
/// imports are already checked when it is reached. Errors are returned grouped by
/// module; an empty vec means the whole project type-checks. A module with errors
/// still contributes its (best-effort) exports so dependents can be checked too.
pub fn check(project: &Project) -> Vec<ModuleErrors> {
    use crate::types::{self, ModuleExports};

    let mut exports: HashMap<String, ModuleExports> = HashMap::new();
    let mut all = Vec::new();
    for module in &project.modules {
        let imports: HashMap<String, ModuleExports> = module
            .imports
            .iter()
            .filter_map(|name| exports.get(name).map(|e| (name.clone(), e.clone())))
            .collect();
        let (errors, module_exports) = types::check_module(&module.ast, &imports);
        if !errors.is_empty() {
            all.push(ModuleErrors {
                name: module.name.clone(),
                errors,
            });
        }
        exports.insert(module.name.clone(), module_exports);
    }
    all
}

/// A compiled project: each module's emitted Python source, keyed by file name
/// (`geometry.py`, `main.py`), plus the shared runtime `_pyfun_rt.py` when any
/// module needs the nominal `Option`/`Result` classes.
#[derive(Debug)]
pub struct CompiledProject {
    /// `(file name, Python source)` pairs — the modules in topological order, with
    /// `_pyfun_rt.py` (when present) last.
    pub files: Vec<(String, String)>,
}

/// Lower and emit every module of `project` to Python (`DESIGN.md` §6.1).
///
/// Each module compiles to its own `<name>.py`; a cross-module `Geometry.area`
/// emits `geometry.area` with `import geometry` hoisted, and the nominal
/// `Option`/`Result` classes are hoisted into a shared `_pyfun_rt.py` that every
/// module needing them imports (so such values are `isinstance`-compatible across
/// files). Imported functions' arities are threaded through so a partial
/// application across modules still curries.
///
/// This lowers only; the caller is expected to have run [`check`] first (the CLI
/// gates compilation on a clean check, like the single-file path).
pub fn compile(project: &Project) -> Result<CompiledProject, crate::lowering::LowerError> {
    use crate::lowering::{self, ImportContext};
    use crate::python_emitter;

    // Each module's exported arities (functions + ctors with args) and nullary
    // constructors, for cross-module partial application and nullary-ctor calls.
    let arities: HashMap<String, HashMap<String, usize>> = project
        .modules
        .iter()
        .map(|m| (m.name.clone(), export_arities(&m.ast)))
        .collect();
    let nullary: HashMap<String, HashSet<String>> = project
        .modules
        .iter()
        .map(|m| (m.name.clone(), export_nullary_ctors(&m.ast)))
        .collect();
    // Each module's public records (name → declared field order), so a cross-module
    // literal/update lowers to the exporting class in `__init__` order.
    let records: HashMap<String, HashMap<String, Vec<String>>> = project
        .modules
        .iter()
        .map(|m| (m.name.clone(), export_records(&m.ast)))
        .collect();

    let mut files = Vec::new();
    let mut needs_runtime = false;
    for module in &project.modules {
        let mut ctx = ImportContext::default();
        for import in &module.imports {
            ctx.modules.insert(import.clone());
            if let Some(members) = arities.get(import) {
                for (member, arity) in members {
                    ctx.member_arities
                        .insert(format!("{import}.{member}"), *arity);
                }
            }
            if let Some(ctors) = nullary.get(import) {
                for ctor in ctors {
                    ctx.nullary_ctors.insert(format!("{import}.{ctor}"));
                }
            }
            if let Some(recs) = records.get(import) {
                for (rec, fields) in recs {
                    let tag = format!("{import}.{rec}");
                    for field in fields {
                        ctx.record_field_owners.insert(field.clone(), tag.clone());
                    }
                    ctx.record_fields.insert(tag, fields.clone());
                }
            }
        }
        let lowered = lowering::lower_in_project(&module.ast, &ctx)?;
        needs_runtime |= lowered.uses_runtime;
        files.push((
            module_py_name(&module.name),
            python_emitter::emit(&lowered.py),
        ));
    }
    if needs_runtime {
        files.push((
            "_pyfun_rt.py".to_string(),
            python_emitter::emit(&lowering::runtime_module()),
        ));
    }
    Ok(CompiledProject { files })
}

/// Best-effort resolution of `module`'s imports' export interfaces, reading
/// sibling `<name>.pyfun` files from `dir` (`DESIGN.md` §6.1). For the **LSP**:
/// returns the `ModuleExports` of each *directly* imported module that could be
/// loaded and checked (transitively resolving each import's own imports), so an
/// in-editor multi-module file type-checks `Geometry.area` cleanly. Unlike the
/// compile/check driver this is *forgiving* — a missing file, a parse error, or
/// an import cycle simply omits that module rather than erroring, so a
/// half-written project still gives useful diagnostics for the file in hand.
pub fn resolve_imports(
    dir: &Path,
    module: &Module,
) -> HashMap<String, crate::types::ModuleExports> {
    let mut cache = HashMap::new();
    let mut visiting = HashSet::new();
    let mut out = HashMap::new();
    for name in collect_imports(module) {
        if let Some(exports) = resolve_exports(dir, &name, &mut cache, &mut visiting) {
            out.insert(name, exports);
        }
    }
    out
}

/// Load, recursively resolve the imports of, and check the module `name` from
/// `dir`, returning its exports. Memoized in `cache`; `visiting` breaks cycles.
/// Any failure (missing file, parse error, cycle) yields `None`.
fn resolve_exports(
    dir: &Path,
    name: &str,
    cache: &mut HashMap<String, crate::types::ModuleExports>,
    visiting: &mut HashSet<String>,
) -> Option<crate::types::ModuleExports> {
    if let Some(exports) = cache.get(name) {
        return Some(exports.clone());
    }
    if visiting.contains(name) {
        return None; // a cycle — bail rather than recurse forever
    }
    let source = std::fs::read_to_string(dir.join(module_file_name(name))).ok()?;
    let module = crate::parse(&source).ok()?;

    visiting.insert(name.to_string());
    let mut imports = HashMap::new();
    for import in collect_imports(&module) {
        if let Some(exports) = resolve_exports(dir, &import, cache, visiting) {
            imports.insert(import, exports);
        }
    }
    visiting.remove(name);

    let (_errors, exports) = crate::types::check_module(&module, &imports);
    cache.insert(name.to_string(), exports.clone());
    Some(exports)
}

/// Lowering-relevant arities a module exports, keyed by bare name: top-level `let`
/// *functions* and *constructors that take arguments* (so a cross-module partial
/// application lowers to `functools.partial`). Parameterless values and nullary
/// constructors have no arity entry (the latter are listed by [`export_nullary_ctors`]).
fn export_arities(module: &Module) -> HashMap<String, usize> {
    use crate::parser::ast::{ExprKind, TypeDeclKind};
    let mut out = HashMap::new();
    for item in &module.items {
        match item {
            Item::Let(binding) => {
                let arity = if !binding.params.is_empty() {
                    Some(binding.params.len())
                } else if let ExprKind::Fn { params, .. } = &binding.value.kind {
                    Some(params.len())
                } else {
                    None
                };
                if let Some(a) = arity {
                    out.insert(binding.name.clone(), a);
                }
            }
            Item::Type(decl) => {
                if let TypeDeclKind::Sum(variants) = &decl.kind {
                    for v in variants {
                        if !v.fields.is_empty() {
                            out.insert(v.name.clone(), v.fields.len());
                        }
                    }
                }
            }
            // An imported `extern`'s arity (its leading arrows) so a *partial*
            // application across modules (`List.map Mathx.sqrt xs`) still curries.
            Item::Extern(decl) => {
                let arity = crate::lowering::arrow_arity(&decl.ty);
                if arity > 0 {
                    out.insert(decl.name.clone(), arity);
                }
            }
            _ => {}
        }
    }
    out
}

/// The bare names of a module's **nullary** sum-type constructors, so an importing
/// module lowers `Palette.Red` (used as a value) to a call `palette.Red()`.
fn export_nullary_ctors(module: &Module) -> HashSet<String> {
    use crate::parser::ast::TypeDeclKind;
    let mut out = HashSet::new();
    for item in &module.items {
        if let Item::Type(decl) = item
            && let TypeDeclKind::Sum(variants) = &decl.kind
        {
            for v in variants {
                if v.fields.is_empty() {
                    out.insert(v.name.clone());
                }
            }
        }
    }
    out
}

/// A module's public **record** types (name → declared field order), so an
/// importing module lowers `Geometry.Point { … }` / `{ p with x = … }` to the
/// exporting class in `__init__` order (`DESIGN.md` §8.3).
fn export_records(module: &Module) -> HashMap<String, Vec<String>> {
    use crate::parser::ast::TypeDeclKind;
    let mut out = HashMap::new();
    for item in &module.items {
        if let Item::Type(decl) = item
            && let TypeDeclKind::Record(fields) = &decl.kind
        {
            out.insert(
                decl.name.clone(),
                fields.iter().map(|f| f.name.clone()).collect(),
            );
        }
    }
    out
}

/// The emitted Python file name for a module (`Geometry` → `geometry.py`).
pub fn module_py_name(name: &str) -> String {
    format!("{}.py", name.to_lowercase())
}

/// Whether a module visit is in progress (on the DFS path) or finished.
enum Visit {
    Visiting,
    Done,
}

/// Mutable state threaded through the depth-first graph walk.
struct Builder<F> {
    load: F,
    loaded: HashMap<String, LoadedModule>,
    state: HashMap<String, Visit>,
    order: Vec<String>,
    stack: Vec<String>,
}

impl<F> Builder<F>
where
    F: Fn(&str) -> Option<String>,
{
    fn visit(&mut self, name: &str, importer: Option<&str>) -> Result<(), ProjectError> {
        match self.state.get(name) {
            Some(Visit::Done) => return Ok(()),
            Some(Visit::Visiting) => {
                // A back-edge to a module already on the path: report the cycle
                // from where the name first appears, repeating it at the end.
                let from = self.stack.iter().position(|n| n == name).unwrap_or(0);
                let mut names: Vec<String> = self.stack[from..].to_vec();
                names.push(name.to_string());
                return Err(ProjectError::Cycle { names });
            }
            None => {}
        }

        let source = (self.load)(name).ok_or_else(|| ProjectError::Missing {
            name: name.to_string(),
            importer: importer.map(str::to_string),
        })?;
        let ast = crate::parse(&source).map_err(|error| ProjectError::Compile {
            name: name.to_string(),
            error,
        })?;
        let imports = collect_imports(&ast);

        self.state.insert(name.to_string(), Visit::Visiting);
        self.stack.push(name.to_string());
        for import in &imports {
            self.visit(import, Some(name))?;
        }
        self.stack.pop();
        self.state.insert(name.to_string(), Visit::Done);
        self.order.push(name.to_string());
        self.loaded.insert(
            name.to_string(),
            LoadedModule {
                name: name.to_string(),
                source,
                ast,
                imports,
            },
        );
        Ok(())
    }
}

/// The module names imported by a parsed module, in source order.
fn collect_imports(module: &Module) -> Vec<String> {
    module
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Import { name, .. } => Some(name.clone()),
            _ => None,
        })
        .collect()
}

/// Uppercase the first character of `s`, leaving the rest unchanged.
fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An in-memory loader from a list of `(name, source)` pairs.
    fn loader<'a>(files: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| {
            files
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, src)| src.to_string())
        }
    }

    fn names(project: &Project) -> Vec<&str> {
        project.modules.iter().map(|m| m.name.as_str()).collect()
    }

    #[test]
    fn single_module_with_no_imports() {
        let project = build("Main", loader(&[("Main", "let x = 1")])).unwrap();
        assert_eq!(names(&project), ["Main"]);
        assert_eq!(project.entry().name, "Main");
    }

    #[test]
    fn dependencies_come_before_dependents() {
        // Main imports Geometry; Geometry must be ordered first, Main (entry) last.
        let project = build(
            "Main",
            loader(&[
                ("Main", "import Geometry\nlet a = Geometry.area 2 3"),
                ("Geometry", "let area w h = w * h"),
            ]),
        )
        .unwrap();
        assert_eq!(names(&project), ["Geometry", "Main"]);
        assert_eq!(project.entry().name, "Main");
        assert_eq!(project.modules[1].imports, ["Geometry"]);
    }

    #[test]
    fn diamond_loads_a_shared_dependency_once() {
        // Main -> {Left, Right} -> Shared. Shared appears once, before its users.
        let project = build(
            "Main",
            loader(&[
                ("Main", "import Left\nimport Right\nlet x = 1"),
                ("Left", "import Shared\nlet l = 1"),
                ("Right", "import Shared\nlet r = 1"),
                ("Shared", "let s = 1"),
            ]),
        )
        .unwrap();
        let order = names(&project);
        assert_eq!(order.len(), 4, "Shared is loaded once: {order:?}");
        let pos = |n: &str| order.iter().position(|m| *m == n).unwrap();
        assert!(pos("Shared") < pos("Left"));
        assert!(pos("Shared") < pos("Right"));
        assert!(pos("Left") < pos("Main"));
        assert!(pos("Right") < pos("Main"));
    }

    #[test]
    fn a_missing_module_is_reported_with_its_importer() {
        let err = build("Main", loader(&[("Main", "import Geometry\nlet x = 1")])).unwrap_err();
        assert_eq!(
            err,
            ProjectError::Missing {
                name: "Geometry".to_string(),
                importer: Some("Main".to_string()),
            }
        );
        assert!(err.to_string().contains("geometry.pyfun"));
    }

    #[test]
    fn a_missing_entry_has_no_importer() {
        let err = build("Main", loader(&[])).unwrap_err();
        assert_eq!(
            err,
            ProjectError::Missing {
                name: "Main".to_string(),
                importer: None,
            }
        );
    }

    #[test]
    fn a_cycle_is_an_error() {
        let err = build(
            "A",
            loader(&[("A", "import B\nlet a = 1"), ("B", "import A\nlet b = 1")]),
        )
        .unwrap_err();
        let ProjectError::Cycle { names } = err else {
            panic!("expected a cycle error, got {err:?}");
        };
        assert_eq!(names, ["A", "B", "A"]);
    }

    #[test]
    fn a_self_import_is_a_cycle() {
        let err = build("A", loader(&[("A", "import A\nlet a = 1")])).unwrap_err();
        assert_eq!(
            err,
            ProjectError::Cycle {
                names: vec!["A".to_string(), "A".to_string()],
            }
        );
    }

    #[test]
    fn a_parse_error_names_the_offending_module() {
        let err = build(
            "Main",
            loader(&[("Main", "import Broken\nlet x = 1"), ("Broken", "let = ")]),
        )
        .unwrap_err();
        let ProjectError::Compile { name, .. } = err else {
            panic!("expected a compile error, got {err:?}");
        };
        assert_eq!(name, "Broken");
    }

    #[test]
    fn a_well_typed_project_checks_clean() {
        let project = build(
            "Main",
            loader(&[
                ("Geometry", "let area w h = w * h"),
                ("Main", "import Geometry\nlet floor = Geometry.area 4 5"),
            ]),
        )
        .unwrap();
        let errors = check(&project);
        assert!(errors.is_empty(), "unexpected errors: {errors:?}");
    }

    #[test]
    fn using_an_unexported_member_is_an_error() {
        // Geometry exports `area`, not `volume`; Main referencing `Geometry.volume`
        // gets the ordinary "not a member" error, located in Main.
        let project = build(
            "Main",
            loader(&[
                ("Geometry", "let area w h = w * h"),
                ("Main", "import Geometry\nlet v = Geometry.volume 1 2"),
            ]),
        )
        .unwrap();
        let errors = check(&project);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].name, "Main");
        assert!(
            errors[0].errors[0]
                .message
                .contains("not a member of `Geometry`"),
            "got: {}",
            errors[0].errors[0].message
        );
    }

    #[test]
    fn a_cross_module_type_mismatch_is_caught() {
        // Geometry.area : num 'a => 'a -> 'a -> 'a; applying it to a string is a
        // type error in the importing module.
        let project = build(
            "Main",
            loader(&[
                ("Geometry", "let area w h = w * h"),
                (
                    "Main",
                    "import Geometry\nlet bad = Geometry.area 4 \"five\"",
                ),
            ]),
        )
        .unwrap();
        let errors = check(&project);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].name, "Main");
    }

    #[test]
    fn an_imported_polymorphic_value_instantiates_freshly() {
        // `id : 'a -> 'a` imported and used at two different types must check.
        let project = build(
            "Main",
            loader(&[
                ("Util", "let id x = x"),
                (
                    "Main",
                    "import Util\nlet n = Util.id 1\nlet s = Util.id \"hi\"",
                ),
            ]),
        )
        .unwrap();
        let errors = check(&project);
        assert!(errors.is_empty(), "unexpected errors: {errors:?}");
    }

    #[test]
    fn a_member_not_imported_is_unavailable() {
        // Without `import Geometry`, Main never seeds Geometry's members, so
        // `Geometry.area` is unresolved at check time (no import edge, no graph
        // error — just an unknown qualified reference).
        let project = build("Main", loader(&[("Main", "let x = Geometry.area 1 2")])).unwrap();
        let errors = check(&project);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].name, "Main");
        assert!(
            errors[0].errors[0]
                .message
                .contains("not a member of `Geometry`"),
            "got: {}",
            errors[0].errors[0].message
        );
    }

    #[test]
    fn module_name_and_file_name_round_trip() {
        assert_eq!(module_file_name("Geometry"), "geometry.pyfun");
        assert_eq!(
            module_name_from_path(Path::new("src/geometry.pyfun")).as_deref(),
            Some("Geometry")
        );
        assert_eq!(
            module_name_from_path(Path::new("main.pyfun")).as_deref(),
            Some("Main")
        );
    }
}
