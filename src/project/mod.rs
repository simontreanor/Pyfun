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
use std::path::{Path, PathBuf};

pub mod manifest;

use crate::CompileError;
use crate::parser::ast::{Item, Module};

/// One module, loaded, parsed, and with its import edges extracted.
#[derive(Debug)]
pub struct LoadedModule {
    /// The module's name as an `import` writes it: `Geometry`, or its path
    /// from the project's top folder, `Shapes.Geometry`.
    pub name: String,
    /// The file the module was read from, as a diagnostic names it: the entry
    /// as it was given, and an import beside it (`src/shapes/geometry.pyfun`
    /// for an entry `src/main.pyfun`).
    pub file: String,
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
    /// One or more modules failed to lex or parse: every syntax error the
    /// recovering front end finds in each, in the order the modules were
    /// reached.
    Syntax(Vec<SyntaxErrors>),
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
            ProjectError::Syntax(modules) => match modules.first() {
                Some(SyntaxErrors { name, errors, .. }) => match errors.first() {
                    Some(error) => write!(f, "in module `{name}`: {error}"),
                    None => write!(f, "in module `{name}`: syntax error"),
                },
                None => write!(f, "syntax error"),
            },
            ProjectError::Cycle { names } => {
                write!(f, "import cycle: {}", names.join(" -> "))
            }
        }
    }
}

impl std::error::Error for ProjectError {}

/// The lex and parse errors of one module of a project.
#[derive(Debug, Clone, PartialEq)]
pub struct SyntaxErrors {
    /// The module the errors were found in.
    pub name: String,
    /// The file it was read from ([`LoadedModule::file`]).
    pub file: String,
    /// Its source text, which the errors' spans point into.
    pub source: String,
    /// The errors, in source order.
    pub errors: Vec<CompileError>,
}

/// The Python name of a module: each part of its path in snake_case, as PEP 8
/// names a module and a package (`Geometry` → `geometry`, `TrainTimes` →
/// `train_times`, `Shapes.Geometry` → `shapes.geometry`). One function serves
/// the `.pyfun` source, the emitted `.py` and the Python `import`, so the three
/// can never disagree (`DESIGN.md` §6.1).
pub fn module_stem(name: &str) -> String {
    name.split('.')
        .map(crate::types::to_snake_case)
        .collect::<Vec<_>>()
        .join(".")
}

/// The name code uses a module by: the last part of its path (`Shapes.Geometry`
/// → `Geometry`), which is all of it for a module at the top.
pub fn module_short_name(name: &str) -> &str {
    name.rsplit('.').next().unwrap_or(name)
}

/// The source file a module name resolves to, from the project's top folder:
/// `Geometry` → `geometry.pyfun`, `Shapes.Geometry` → `shapes/geometry.pyfun`.
pub fn module_file_name(name: &str) -> String {
    format!("{}.pyfun", module_stem(name).replace('.', "/"))
}

/// The emitted Python file for a module, from the output's top folder:
/// `Geometry` → `geometry.py`, `Shapes.Geometry` → `shapes/geometry.py`.
pub fn module_py_name(name: &str) -> String {
    format!("{}.py", module_stem(name).replace('.', "/"))
}

/// An empty `__init__.py` for each folder the emitted `files` sit in, nearest
/// the top first: what makes `shapes/` the package `import shapes.geometry`
/// names, for Python and for the tools that read a project (pytest, mypy).
pub fn package_inits(files: &[(String, String)]) -> Vec<(String, String)> {
    let mut inits: Vec<String> = Vec::new();
    for (name, _) in files {
        let mut folder = name.as_str();
        while let Some((parent, _)) = folder.rsplit_once('/') {
            let init = format!("{parent}/__init__.py");
            if !inits.contains(&init) {
                inits.push(init);
            }
            folder = parent;
        }
    }
    inits.sort();
    inits
        .into_iter()
        .map(|name| (name, String::new()))
        .collect()
}

/// What an import that names no file under `root` may have meant, for the
/// message: the module sits beside the one that asked for it, or the file
/// being run is itself inside a package.
pub fn missing_hint(root: &Path, name: &str, importer: Option<&str>) -> Option<String> {
    let file = module_file_name(name);
    if let Some(by) = importer
        && let Some((package, _)) = by.rsplit_once('.')
    {
        let beside = format!("{package}.{name}");
        if root.join(module_file_name(&beside)).is_file() {
            return Some(format!(
                "`{}` sits beside `{by}`. An import names a module from the top folder of the \
                 program, so write `import {beside}`",
                module_file_name(&beside)
            ));
        }
    }
    let root = std::path::absolute(root).ok()?;
    let above = root
        .ancestors()
        .skip(1)
        .take(MAX_ROOT_LEVELS)
        .find(|folder| folder.join(&file).is_file())?;
    Some(format!(
        "`{file}` is found from `{}`. An import names a module from the folder of the file \
         being run, so run or check the program's main file there",
        above.display()
    ))
}

/// Write one emitted file under `dir`, making the package folders its name
/// sits in (`shapes/geometry.py`).
pub fn write_file(dir: &Path, name: &str, contents: &str) -> std::io::Result<()> {
    let path = dir.join(name);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, contents)
}

/// How far above a file's own folder its imports are read from, when nothing
/// names the file being run (the editor): the nearest folder, at or above the
/// file's, from which each import is found, and the furthest of those. A file
/// in `shapes/` that imports `Shapes.Util` is one folder below the top. `found`
/// answers whether a path is a module's source.
pub fn import_root_level(dir: &Path, imports: &[String], found: impl Fn(&Path) -> bool) -> usize {
    let mut level = 0;
    for name in imports {
        let file = module_file_name(name);
        let nearest = dir
            .ancestors()
            .take(MAX_ROOT_LEVELS + 1)
            .position(|folder| found(&folder.join(&file)));
        level = level.max(nearest.unwrap_or(0));
    }
    level
}

/// How many folders above a file the top of its project is looked for.
pub const MAX_ROOT_LEVELS: usize = 6;

/// The module name for a source path: its file stem in PascalCase, each
/// underscore-separated word capitalized (`geometry.pyfun` → `Geometry`,
/// `train_times.pyfun` → `TrainTimes`). An underscore before a digit is kept
/// (`day_1.pyfun` → `Day_1`), so the name maps back to the same stem. `None` if
/// the path has no stem.
pub fn module_name_from_path(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_str()?;
    let mut name = String::with_capacity(stem.len());
    for (i, word) in stem.split('_').enumerate() {
        if i > 0 && !word.starts_with(|c: char| c.is_ascii_alphabetic()) {
            name.push('_');
        }
        name.push_str(&capitalize(word));
    }
    Some(name)
}

/// The module name of a source file from its path under the project's top
/// folder, written with `/`: `shapes/geometry.pyfun` → `Shapes.Geometry`.
pub fn module_name_from_rel(rel: &str) -> Option<String> {
    let parts: Option<Vec<String>> = rel
        .split('/')
        .map(|part| module_name_from_path(Path::new(part)))
        .collect();
    Some(parts?.join("."))
}

/// The modules a source text imports, read off its `import` and `from` lines
/// without parsing it, so a file that is half written still says what it
/// imports. Only a line that opens at the margin with a capitalized module
/// path counts.
pub fn import_names_in(source: &str) -> Vec<String> {
    let mut names = Vec::new();
    for line in source.lines() {
        let rest = if let Some(rest) = line.strip_prefix("import ") {
            rest
        } else if let Some(rest) = line.strip_prefix("from ") {
            rest
        } else {
            continue;
        };
        let rest = rest.trim_start();
        let end = rest
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '.'))
            .unwrap_or(rest.len());
        let name = &rest[..end];
        let is_from = line.starts_with("from ");
        let then_import = rest[end..].trim_start().starts_with("import");
        if name.starts_with(|c: char| c.is_ascii_uppercase())
            && !name.ends_with('.')
            && (!is_from || then_import)
        {
            names.push(name.to_string());
        }
    }
    names
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
    build_named(entry, |name| {
        load(name).map(|source| (module_file_name(name), source))
    })
}

/// [`build`], with a loader that also says which file each source came from
/// ([`LoadedModule::file`]).
///
/// A module that does not lex or parse does not stop the walk: its errors are
/// kept, its imports are read off the items that did parse, and the rest of the
/// graph is loaded, so one run reports the syntax errors of every module
/// ([`ProjectError::Syntax`]).
pub fn build_named<F>(entry: &str, load: F) -> Result<Project, ProjectError>
where
    F: Fn(&str) -> Option<(String, String)>,
{
    let mut ctx = Builder {
        load,
        loaded: HashMap::new(),
        state: HashMap::new(),
        order: Vec::new(),
        stack: Vec::new(),
        broken: Vec::new(),
    };
    let walked = ctx.visit(entry, None);
    if !ctx.broken.is_empty() {
        return Err(ProjectError::Syntax(ctx.broken));
    }
    walked?;
    let mut loaded = ctx.loaded;
    let modules = ctx
        .order
        .into_iter()
        .map(|name| loaded.remove(&name).expect("every ordered name was loaded"))
        .collect();
    Ok(Project { modules })
}

/// The file module `name` resolves to from the project folder `dir`: its own
/// `<name>.pyfun` first (`shapes/geometry.pyfun` for `Shapes.Geometry`), then
/// the same file in each façade vendored into the enclosing project
/// (`<root>/.pyfun/facades/*/`, `DESIGN.md` §6.2), so a project's own module
/// always wins over a façade of the same name.
pub fn locate_module(dir: &Path, name: &str) -> Option<PathBuf> {
    let file = module_file_name(name);
    let sibling = dir.join(&file);
    if sibling.is_file() {
        return Some(sibling);
    }
    let root = manifest::find_root(dir)?;
    manifest::facade_dirs(&root)
        .into_iter()
        .map(|d| d.join(&file))
        .find(|p| p.is_file())
}

/// Resolve the module graph rooted at a source file on disk, reading imported
/// modules from sibling `<name>.pyfun` files in the entry's directory, or from
/// the project's vendored façades ([`locate_module`]).
pub fn build_from_path(entry: &Path) -> Result<Project, ProjectError> {
    let entry_name = module_name_from_path(entry).ok_or_else(|| ProjectError::Missing {
        name: entry.display().to_string(),
        importer: None,
    })?;
    let root = entry.parent().map(Path::to_path_buf).unwrap_or_default();
    let typed = entry.display().to_string();
    build_named(&entry_name, |name| {
        if name == entry_name {
            return Some((typed.clone(), std::fs::read_to_string(entry).ok()?));
        }
        let path = locate_module(&root, name)?;
        let file = path.display().to_string();
        // A module's path is spelled with the entry's slashes.
        let file = if typed.contains('/') {
            file.replace('\\', "/")
        } else {
            file.replace('/', std::path::MAIN_SEPARATOR_STR)
        };
        Some((file, std::fs::read_to_string(path).ok()?))
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
    check_reporting(project).0
}

/// Warnings found in one module of a project.
#[derive(Debug)]
pub struct ModuleWarnings {
    /// The module the warnings were found in.
    pub name: String,
    pub warnings: Vec<crate::diagnostics::Warning>,
}

/// [`check`], with each module's warnings beside the errors. A module appears in
/// a list only when it has something for it.
pub fn check_reporting(project: &Project) -> (Vec<ModuleErrors>, Vec<ModuleWarnings>) {
    use crate::types::{self, ModuleExports};

    let mut exports: HashMap<String, ModuleExports> = HashMap::new();
    let mut all = Vec::new();
    let mut all_warnings = Vec::new();
    for module in &project.modules {
        let imports: HashMap<String, ModuleExports> = module
            .imports
            .iter()
            .filter_map(|name| exports.get(name).map(|e| (name.clone(), e.clone())))
            .collect();
        let (errors, module_exports, warnings) =
            types::check_module_reporting(&module.ast, &imports);
        if !errors.is_empty() {
            all.push(ModuleErrors {
                name: module.name.clone(),
                errors,
            });
        }
        if !warnings.is_empty() {
            all_warnings.push(ModuleWarnings {
                name: module.name.clone(),
                warnings,
            });
        }
        exports.insert(module.name.clone(), module_exports);
    }
    (all, all_warnings)
}

/// A compiled project: each module's emitted Python source, keyed by file name
/// (`geometry.py`, `main.py`), plus the shared runtime `_pyfun_rt.py` when any
/// module needs the nominal `Option`/`Result` classes.
#[derive(Debug)]
pub struct CompiledProject {
    /// `(file name, Python source)` pairs — the modules in topological order, with
    /// `_pyfun_rt.py` (when present) last.
    pub files: Vec<(String, String)>,
    /// Lowering notes from every module, each prefixed with its module name.
    pub notes: Vec<String>,
    /// Each module's line map, by module name: where its emitted lines came
    /// from, for a traceback in `.pyfun` lines (`DESIGN.md` §9). Filled only by
    /// [`compile_traced`] and [`compile_tests`], and only for a module whose
    /// map could be built.
    pub line_maps: Vec<(String, crate::LineMap)>,
    /// Whether a test module has a property test, which Hypothesis runs
    /// (`pyfun test`, `DESIGN.md` §9).
    pub has_properties: bool,
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
    compile_targeting(project, crate::python_emitter::PyTarget::default())
}

/// [`compile`] with an explicit emission target — every module (and the shared
/// `_pyfun_rt.py`) is emitted for `target`.
pub fn compile_targeting(
    project: &Project,
    target: crate::python_emitter::PyTarget,
) -> Result<CompiledProject, crate::lowering::LowerError> {
    compile_with(project, target, false)
}

/// [`compile_targeting`] with the native lowering on or off (`--native`,
/// `DESIGN.md` §5.6) for every module of the project.
pub fn compile_with(
    project: &Project,
    target: crate::python_emitter::PyTarget,
    native: bool,
) -> Result<CompiledProject, crate::lowering::LowerError> {
    compile_project(project, target, native, false, false, false)
}

/// [`compile`] for `pyfun test` (`DESIGN.md` §9): in each test module (one
/// whose file is `test_*.pyfun`), a `let test_name _ = …` is emitted as the
/// `def test_name():` pytest collects. Each module's line map is built as
/// [`compile_traced`] builds it, so a failure is reported in `.pyfun` lines.
pub fn compile_tests(project: &Project) -> Result<CompiledProject, crate::lowering::LowerError> {
    compile_project(
        project,
        crate::python_emitter::PyTarget::default(),
        false,
        true,
        false,
        true,
    )
}

/// Whether the module named `name` is a test module: its file is
/// `test_*.pyfun`, the name pytest looks for.
pub fn is_test_module(name: &str) -> bool {
    module_stem(module_short_name(name)).starts_with("test_")
}

/// [`compile`] with **typed output** (`--typed`, `DESIGN.md` §5.7): every
/// module's functions, values and classes carry their types as annotations,
/// and the shared runtime's `Some`/`Ok`/`Error` are generic.
pub fn compile_typed(project: &Project) -> Result<CompiledProject, crate::lowering::LowerError> {
    compile_project(
        project,
        crate::python_emitter::PyTarget::default(),
        false,
        false,
        true,
        false,
    )
}

/// [`compile`], also building each module's line map (see
/// [`crate::compile_with_line_map`]): what `pyfun run` needs to report a
/// runtime error in `.pyfun` lines.
pub fn compile_traced(project: &Project) -> Result<CompiledProject, crate::lowering::LowerError> {
    compile_project(
        project,
        crate::python_emitter::PyTarget::default(),
        false,
        true,
        false,
        false,
    )
}

fn compile_project(
    project: &Project,
    target: crate::python_emitter::PyTarget,
    native: bool,
    trace: bool,
    typed: bool,
    tests: bool,
) -> Result<CompiledProject, crate::lowering::LowerError> {
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
    // Which of those the defining module backs with a `_Red = Red()` singleton
    // (all of them, unless the module itself binds the singleton's name).
    let singletons: HashMap<String, HashSet<String>> = project
        .modules
        .iter()
        .map(|m| {
            let binders = lowering::module_binder_names(&m.ast);
            let set = nullary[&m.name]
                .iter()
                .filter(|c| !binders.contains(&lowering::nullary_singleton_name(c)))
                .cloned()
                .collect();
            (m.name.clone(), set)
        })
        .collect();
    let newtypes: HashMap<String, HashSet<String>> = project
        .modules
        .iter()
        .map(|m| (m.name.clone(), export_newtypes(&m.ast)))
        .collect();
    // Each module's public records (name → declared field order), so a cross-module
    // literal/update lowers to the exporting class in `__init__` order.
    let records: HashMap<String, HashMap<String, Vec<String>>> = project
        .modules
        .iter()
        .map(|m| (m.name.clone(), export_records(&m.ast)))
        .collect();
    // Each module's record-bodied constructors (name → declared field names),
    // so a cross-module literal is a keyword call and a derived codec keys its
    // fields by name.
    let ctor_fields: HashMap<String, HashMap<String, Vec<String>>> = project
        .modules
        .iter()
        .map(|m| (m.name.clone(), export_ctor_fields(&m.ast)))
        .collect();

    // Per-module spans of integer literals that inference resolved to `float`, so
    // lowering emits them as `7.0`. Computed in topological order (like `check`) so
    // each module's imports are seeded before it — a literal's float-ness can depend
    // on a cross-module call. Exports are threaded forward in the same pass and kept:
    // an import's interface also carries the records it references transitively
    // (`DESIGN.md` §6.1), which the lowering context below needs.
    let mut exports: HashMap<String, crate::types::ModuleExports> = HashMap::new();
    let mut codecs_by_module: HashMap<String, crate::types::Codecs> = HashMap::new();
    // Native mode annotates functions from their types (`DESIGN.md` §5.6).
    let mut types_by_module: HashMap<String, HashMap<crate::lexer::Span, crate::types::Ty>> =
        HashMap::new();
    // Typed output spells a searched variable as what `in` takes (§5.7).
    let mut searched_by_module: HashMap<String, HashMap<u32, Vec<crate::types::Ty>>> =
        HashMap::new();
    // A property test's parameters are generated from their types, and a
    // message about one shows the type as the checker does (`DESIGN.md` §9).
    let mut shown_by_module: HashMap<String, HashMap<crate::lexer::Span, String>> = HashMap::new();
    let float_spans: HashMap<String, std::collections::HashSet<crate::lexer::Span>> = {
        let mut spans = HashMap::new();
        for module in &project.modules {
            let imports: HashMap<String, crate::types::ModuleExports> = module
                .imports
                .iter()
                .filter_map(|n| exports.get(n).map(|e| (n.clone(), e.clone())))
                .collect();
            let (_errors, types, module_exports, codecs) =
                crate::types::check_module_collecting(&module.ast, &imports);
            spans.insert(module.name.clone(), crate::float_literal_spans(&types));
            let properties = tests && is_test_module(&module.name);
            if typed || properties {
                searched_by_module.insert(module.name.clone(), crate::types::searched_vars(&types));
            }
            if properties {
                shown_by_module.insert(
                    module.name.clone(),
                    types.iter().map(|t| (t.span, t.ty.clone())).collect(),
                );
            }
            if native || typed || properties {
                types_by_module.insert(
                    module.name.clone(),
                    types.iter().map(|t| (t.span, t.raw.clone())).collect(),
                );
            }
            codecs_by_module.insert(module.name.clone(), codecs);
            exports.insert(module.name.clone(), module_exports);
        }
        spans
    };
    let no_floats = std::collections::HashSet::new();
    let no_codecs = crate::types::Codecs::default();

    let mut files = Vec::new();
    let mut needs_runtime = false;
    let mut has_properties = false;
    let mut notes: Vec<String> = Vec::new();
    let mut line_maps = Vec::new();
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
            if let Some(ctors) = singletons.get(import) {
                for ctor in ctors {
                    ctx.nullary_singletons.insert(format!("{import}.{ctor}"));
                }
            }
            if let Some(names) = newtypes.get(import) {
                for name in names {
                    ctx.newtype_ctors.insert(format!("{import}.{name}"));
                }
            }
            if let Some(ctors) = ctor_fields.get(import) {
                for (ctor, fields) in ctors {
                    ctx.ctor_fields
                        .insert(format!("{import}.{ctor}"), fields.clone());
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
        // Records an import's interface carries transitively (declared in a
        // module this one does not import directly): keyed by the *declaring*
        // module's tag, so an update on such a record reconstructs via the right
        // class (`inner.Config(...)`, with `import inner` hoisted). Registered
        // after the direct entries, which keep precedence for shared field names.
        for import in &module.imports {
            let Some(exp) = exports.get(import) else {
                continue;
            };
            for (origin, rec, fields) in exp.carried_records() {
                let tag = format!("{origin}.{rec}");
                for field in &fields {
                    ctx.record_field_owners
                        .entry(field.clone())
                        .or_insert_with(|| tag.clone());
                }
                ctx.record_fields.entry(tag).or_insert(fields);
                ctx.record_class_modules.insert(origin);
            }
        }
        let floats = float_spans.get(&module.name).unwrap_or(&no_floats);
        let codecs = codecs_by_module.get(&module.name).unwrap_or(&no_codecs);
        let recorded_types = types_by_module.remove(&module.name).unwrap_or_default();
        // Native lowering reads the types as it goes; typed output is a pass
        // over the finished module.
        let binding_types = if native {
            recorded_types.clone()
        } else {
            HashMap::new()
        };
        let test_module = tests && is_test_module(&module.name);
        // Every module this one imports, directly or through another: a type
        // of any of them can turn up in a signature here, or be generated
        // for a property's parameter.
        let mut reach: Vec<&LoadedModule> = Vec::new();
        let mut pending: Vec<&String> = if typed || test_module {
            module.imports.iter().collect()
        } else {
            Vec::new()
        };
        while let Some(name) = pending.pop() {
            if reach.iter().any(|m| &m.name == name) {
                continue;
            }
            if let Some(found) = project.modules.iter().find(|m| &m.name == name) {
                reach.push(found);
                pending.extend(found.imports.iter());
            }
        }
        let imported: Vec<(String, &crate::syntax::Module)> = reach
            .iter()
            .map(|m| (module_stem(&m.name), &m.ast))
            .collect();
        let searched = searched_by_module.remove(&module.name).unwrap_or_default();
        let shown = shown_by_module.remove(&module.name).unwrap_or_default();
        let property_types = lowering::PropertyTypes {
            binding_types: &recorded_types,
            shown: &shown,
            searched: &searched,
            imported: &imported,
        };
        let traced = trace
            .then(|| {
                lowering::lower_in_project_tracing(
                    &module.ast,
                    &ctx,
                    floats,
                    codecs,
                    native,
                    binding_types.clone(),
                    true,
                )
                .ok()
            })
            .flatten()
            .and_then(|mut traced| {
                // The traced module takes every step the real one does, or
                // the two would not compare.
                if test_module {
                    traced.py =
                        lowering::as_pytest(&module.ast, &module.name, traced.py, &property_types)
                            .ok()?
                            .py;
                }
                Some(traced)
            });
        let lowered =
            lowering::lower_in_project(&module.ast, &ctx, floats, codecs, native, binding_types)?;
        needs_runtime |= lowered.uses_runtime;
        notes.extend(
            lowered
                .notes
                .iter()
                .map(|n| format!("in `{}`: {n}", module.name)),
        );
        let lowered_py = if typed {
            lowering::annotate_typed(
                &module.ast,
                lowered.py,
                &recorded_types,
                &searched,
                &imported,
            )
        } else {
            lowered.py
        };
        let lowered_py = if test_module {
            let pytest =
                lowering::as_pytest(&module.ast, &module.name, lowered_py, &property_types)?;
            needs_runtime |= pytest.uses_runtime;
            has_properties |= pytest.properties;
            pytest.py
        } else {
            lowered_py
        };
        let python = python_emitter::emit_for(&lowered_py, target);
        if let Some(map) = traced
            .and_then(|t| crate::line_map_of(&python_emitter::emit_for(&t.py, target), &python))
        {
            line_maps.push((module.name.clone(), map));
        }
        files.push((module_py_name(&module.name), python));
    }
    files.extend(package_inits(&files));
    if needs_runtime {
        files.push((
            "_pyfun_rt.py".to_string(),
            python_emitter::emit_for(
                &if native {
                    lowering::runtime_module_native()
                } else if typed {
                    lowering::annotate_typed_runtime(lowering::runtime_module())
                } else {
                    lowering::runtime_module()
                },
                target,
            ),
        ));
    }
    Ok(CompiledProject {
        files,
        notes,
        line_maps,
        has_properties,
    })
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
    let imports = collect_imports(module);
    // A file inside a package names its imports from the top of the project.
    let level = import_root_level(dir, &imports, |file| file.is_file());
    let dir = dir.ancestors().nth(level).unwrap_or(dir);
    for name in imports {
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
    let source = std::fs::read_to_string(locate_module(dir, name)?).ok()?;
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
                if let (Some(a), Some(name)) = (arity, binding.name()) {
                    out.insert(name.to_string(), a);
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

/// The bare names of a module's newtype declarations (`opaque type UserId =
/// string`), so an importing module erases `Ids.UserId` wraps/patterns exactly as
/// the defining module does — there is no class on either side.
fn export_newtypes(module: &Module) -> HashSet<String> {
    use crate::parser::ast::TypeDeclKind;
    let mut out = HashSet::new();
    for item in &module.items {
        if let Item::Type(decl) = item
            && matches!(decl.kind, TypeDeclKind::Newtype(_))
        {
            out.insert(decl.name.clone());
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

/// A module's record-bodied constructors: name → declared field names.
fn export_ctor_fields(module: &Module) -> HashMap<String, Vec<String>> {
    use crate::parser::ast::TypeDeclKind;
    let mut out = HashMap::new();
    for item in &module.items {
        if let Item::Type(decl) = item
            && let TypeDeclKind::Sum(variants) = &decl.kind
        {
            for variant in variants {
                if let Some(names) = &variant.field_names {
                    out.insert(variant.name.clone(), names.clone());
                }
            }
        }
    }
    out
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
    /// The modules that did not lex or parse, with every error of each.
    broken: Vec<SyntaxErrors>,
}

impl<F> Builder<F>
where
    F: Fn(&str) -> Option<(String, String)>,
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

        let (file, source) = (self.load)(name).ok_or_else(|| ProjectError::Missing {
            name: name.to_string(),
            importer: importer.map(str::to_string),
        })?;
        let mut broken = None;
        let ast = match crate::parse(&source) {
            Ok(ast) => ast,
            Err(first) => {
                let (partial, mut errors) = crate::syntax_errors(&source);
                if errors.is_empty() {
                    errors.push(first);
                }
                broken = Some(SyntaxErrors {
                    name: name.to_string(),
                    file: file.clone(),
                    source: source.clone(),
                    errors,
                });
                partial
            }
        };
        let imports = collect_imports(&ast);

        self.state.insert(name.to_string(), Visit::Visiting);
        self.stack.push(name.to_string());
        let walked = imports
            .iter()
            .try_for_each(|import| self.visit(import, Some(name)));
        // After its imports, so the modules are listed in the order their type
        // errors would be: each below what it imports.
        self.broken.extend(broken);
        walked?;
        self.stack.pop();
        self.state.insert(name.to_string(), Visit::Done);
        self.order.push(name.to_string());
        self.loaded.insert(
            name.to_string(),
            LoadedModule {
                name: name.to_string(),
                file,
                source,
                ast,
                imports,
            },
        );
        Ok(())
    }
}

/// The module names imported by a parsed module, in source order.
pub fn collect_imports(module: &Module) -> Vec<String> {
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
        let ProjectError::Syntax(modules) = err else {
            panic!("expected syntax errors, got {err:?}");
        };
        assert_eq!(modules.len(), 1);
        assert_eq!(modules[0].name, "Broken");
        assert_eq!(modules[0].file, "broken.pyfun");
    }

    #[test]
    fn every_module_with_syntax_errors_is_reported_with_all_of_them() {
        // One walk of the graph: the entry does not parse, and neither does
        // one of the two modules it imports. Each is listed below what it
        // imports, with every error it has.
        let err = build(
            "Main",
            loader(&[
                (
                    "Main",
                    "import Left\nimport Right\nlet a = true\nlet b = false",
                ),
                ("Left", "let l = 1"),
                ("Right", "let r = true"),
            ]),
        )
        .unwrap_err();
        let ProjectError::Syntax(modules) = err else {
            panic!("expected syntax errors, got {err:?}");
        };
        let found: Vec<(&str, usize)> = modules
            .iter()
            .map(|m| (m.name.as_str(), m.errors.len()))
            .collect();
        assert_eq!(found, [("Right", 1), ("Main", 2)]);
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
        // A multi-word module is a snake_case file, the way PEP 8 names one.
        assert_eq!(module_file_name("TrainTimes"), "train_times.pyfun");
        assert_eq!(module_py_name("TrainTimes"), "train_times.py");
        for (stem, name) in [
            ("train_times", "TrainTimes"),
            ("json_to_adt", "JsonToAdt"),
            ("day_1", "Day_1"),
            ("geometry", "Geometry"),
        ] {
            let path = format!("{stem}.pyfun");
            assert_eq!(
                module_name_from_path(Path::new(&path)).as_deref(),
                Some(name)
            );
            assert_eq!(module_stem(name), stem, "{name} maps back to its file");
        }
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
