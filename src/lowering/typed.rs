//! Typed output (`pyfun compile --typed`, `DESIGN.md` §5.7): the emitted Python
//! carries the types the checker inferred, as annotations a Python type checker
//! reads. A post-pass over the lowered module. The code that runs is the plain
//! output's, but for a `unit` value, which is written as Python writes a
//! `None` ([`settle_unit_values`], [`settle_unit_returns`]).
//!
//! - A top-level function gets every parameter and its result annotated, with
//!   its type variables as its own type parameters (`def pair[A, B](x: A, y:
//!   B) -> tuple[A, B]`).
//! - A top-level value gets its type (`origin: Point = …`).
//! - A class gets its fields' declared types and, when the type is generic,
//!   type parameters (`class Box[A]:`); a sum type gets an alias over its
//!   constructors' classes (`type Shape = Circle | Rect`).
//! - An `extern` a project binds at top level gets its declared type
//!   (`to_number: Callable[[str], int] = int`).
//! - A local whose type a type checker cannot work out is declared
//!   ([`annotate_locals`]), so that a typed project is clean inside as well as
//!   on its surface.
//!
//! The spelling follows what the lowering produces at run time: a `List` is a
//! `list`, a `Map` a `dict`, an `Option a` is `Some[A] | None_`, a function is
//! a `Callable` of all its arguments at once (`DESIGN.md` §5), an `Async a` is
//! an `Awaitable[A]`, a unit of measure is gone. A number the function is
//! polymorphic in is a type parameter held to `int` and `float` (`def
//! twice[A: (int, float)](x: A) -> A`), so a call at `int` gives an `int`.
//! What a function searches with `in` is a `Container` or an `Iterable` of
//! what it is searched for. What Python cannot say is `Any`.

use std::collections::{BTreeSet, HashMap, HashSet};

use crate::lexer::Span;
use crate::parser::ast::{ExternDecl, Item, Module, TypeDecl, TypeDeclKind, TypeExpr};
use crate::python_emitter::{PyCase, PyExpr, PyModule, PyPattern, PyStmt};
use crate::types::Ty;

use super::{py_ctor_name, py_record_class, py_value_name};

/// Annotate `py`, the lowering of `module`, from the type of each binding
/// (`binding_types`, by the span of the binding's target). `searched` holds
/// the type variables an `in` searches, each with what it is searched for
/// (`types::searched_vars`). `imported` holds the modules whose types this one
/// can name, each with its Python module name (`geo`), so that such a type is
/// spelled through its module.
pub(super) fn annotate(
    module: &Module,
    mut py: PyModule,
    binding_types: &HashMap<Span, Ty>,
    searched: &HashMap<u32, Vec<Ty>>,
    imported: &[(String, &Module)],
) -> PyModule {
    add_json_alias(&mut py);
    let decls = Decls::of(module, &py, imported);
    let mut uses = Uses::default();

    annotate_classes(&mut py, &decls, &mut uses);
    add_aliases(&mut py, &decls, &mut uses);
    annotate_externs(module, &mut py, &decls, &mut uses);
    annotate_values(module, &mut py, &decls, binding_types, &mut uses);
    annotate_functions(module, &mut py, &decls, binding_types, searched, &mut uses);
    settle_unit_values(&mut py);
    settle_unit_returns(&mut py);
    annotate_locals(&mut py, &mut uses);
    add_imports(&mut py, &uses);
    py
}

/// The same treatment for the shared runtime module of a project
/// (`_pyfun_rt.py`): its `Some`/`Ok`/`Error` classes become generic.
pub(super) fn annotate_runtime(mut py: PyModule) -> PyModule {
    let decls = Decls {
        types: HashMap::new(),
        imported: HashMap::new(),
        taken: HashSet::new(),
        reached: HashMap::new(),
    };
    let mut uses = Uses::default();
    add_json_alias(&mut py);
    annotate_classes(&mut py, &decls, &mut uses);
    // The runtime's one module-level variable: the task scope in force.
    for stmt in &mut py.body {
        if let PyStmt::Assign { target, .. } = stmt
            && target == "_pf_scope"
        {
            *target = "_pf_scope: contextvars.ContextVar[Any]".to_string();
            uses.any = true;
        }
    }
    add_imports(&mut py, &uses);
    py
}

/// The built-in `Json` type goes by one name in annotations: `type Json =
/// JNull | JBool | …` after its classes, where this file defines them, and a
/// `Json` beside them on the import where they come from the shared runtime.
/// The two classes that hold more JSON say so.
fn add_json_alias(py: &mut PyModule) {
    let classes: Vec<&str> = crate::types::JSON_CTORS
        .iter()
        .map(|(name, _)| *name)
        .collect();
    for stmt in &mut py.body {
        match stmt {
            PyStmt::ImportFrom { module, names }
                if module == "_pyfun_rt" && names.iter().any(|n| n == "JObject") =>
            {
                names.push("Json".to_string());
            }
            PyStmt::ClassDef {
                name, field_types, ..
            } if field_types.len() == 1 => match name.as_str() {
                "JList" => field_types[0] = "list[\"Json\"]".to_string(),
                "JObject" => field_types[0] = "dict[str, \"Json\"]".to_string(),
                _ => {}
            },
            _ => {}
        }
    }
    let Some(last) = py.body.iter().rposition(
        |s| matches!(s, PyStmt::ClassDef { name, .. } if classes.contains(&name.as_str())),
    ) else {
        return;
    };
    py.body.insert(
        last + 1,
        PyStmt::Assign {
            target: "type Json".to_string(),
            value: PyExpr::Name(classes.join(" | ")),
        },
    );
}

/// What the module declares, as the speller needs it.
struct Decls<'a> {
    /// The module's own type declarations, by name.
    types: HashMap<&'a str, &'a TypeDecl>,
    /// The type declarations of the modules this one imports, by the Python
    /// module that defines its classes and the type's name (`geometry.Point`),
    /// each with that module.
    imported: HashMap<String, (&'a str, &'a TypeDecl)>,
    /// Every name the emitted module binds at top level, which a type
    /// parameter must not shadow.
    taken: HashSet<String>,
    /// How this file reaches each imported Python module: the name to spell a
    /// type through, and the `import` that binds it. That is the module's own
    /// name, or the alias it goes by when a binding of the file took the name
    /// (`import shapes.geometry as _pf_shapes_geometry` beside a `shapes`).
    reached: HashMap<&'a str, (String, String)>,
}

impl<'a> Decls<'a> {
    fn of(module: &'a Module, py: &PyModule, others: &'a [(String, &'a Module)]) -> Decls<'a> {
        let types: HashMap<&str, &TypeDecl> = module
            .items
            .iter()
            .filter_map(|item| match item {
                Item::Type(decl) => Some((decl.name.as_str(), decl)),
                _ => None,
            })
            .collect();
        let mut imported = HashMap::new();
        for (py_module, other) in others {
            for item in &other.items {
                if let Item::Type(decl) = item {
                    imported.insert(
                        format!("{py_module}.{}", decl.name),
                        (py_module.as_str(), decl),
                    );
                }
            }
        }
        let mut taken = HashSet::new();
        for stmt in &py.body {
            match stmt {
                PyStmt::ClassDef { name, .. }
                | PyStmt::FuncDef { name, .. }
                | PyStmt::TypedFuncDef { name, .. }
                | PyStmt::Class { name, .. } => {
                    taken.insert(name.clone());
                }
                PyStmt::Assign { target, .. } => {
                    taken.insert(target.clone());
                }
                PyStmt::Import(name) => {
                    taken.insert(name.clone());
                }
                PyStmt::ImportFrom { names, .. } => taken.extend(names.iter().cloned()),
                _ => {}
            }
        }
        // The module's types are spelled by these names whether or not this
        // file binds them.
        taken.extend(module.items.iter().filter_map(|item| match item {
            Item::Type(decl) => Some(decl.name.clone()),
            _ => None,
        }));
        let mut bound: HashSet<&str> = HashSet::new();
        let mut aliases: HashMap<&str, &str> = HashMap::new();
        for stmt in &py.body {
            match stmt {
                PyStmt::ClassDef { name, .. }
                | PyStmt::FuncDef { name, .. }
                | PyStmt::TypedFuncDef { name, .. }
                | PyStmt::Class { name, .. } => {
                    bound.insert(name);
                }
                PyStmt::Assign { target, .. } => {
                    bound.insert(target);
                }
                PyStmt::Import(spec) => {
                    if let Some((module, alias)) = spec.split_once(" as ") {
                        aliases.insert(module, alias);
                    }
                }
                _ => {}
            }
        }
        let mut reached = HashMap::new();
        for (py_module, _) in others {
            let first = py_module.split('.').next().unwrap_or(py_module);
            let alias = match aliases.get(py_module.as_str()) {
                Some(alias) => Some(alias.to_string()),
                None if bound.contains(first) => {
                    Some(format!("_pf_{}", py_module.replace('.', "_")))
                }
                None => None,
            };
            let entry = match alias {
                Some(alias) => (alias.clone(), format!("{py_module} as {alias}")),
                None => (py_module.clone(), py_module.clone()),
            };
            reached.insert(py_module.as_str(), entry);
        }
        Decls {
            types,
            imported,
            taken,
            reached,
        }
    }

    /// The name of the `index`-th type parameter: `A`, `B`, …, stepping over
    /// any name the module already uses.
    fn param_name(&self, index: usize) -> String {
        (0..)
            .map(|n| {
                let letter = (b'A' + (n % 26) as u8) as char;
                if n < 26 {
                    letter.to_string()
                } else {
                    format!("{letter}{}", n / 26)
                }
            })
            .filter(|name| !self.taken.contains(name))
            .nth(index)
            .expect("an endless supply of names")
    }

    /// How a sum type is spelled: by its alias, by its one class when the
    /// constructor has the type's name, or as nothing Python can say.
    fn sum_spelling(&self, decl: &TypeDecl) -> SumSpelling {
        let TypeDeclKind::Sum(variants) = &decl.kind else {
            return SumSpelling::None;
        };
        if variants.is_empty() {
            return SumSpelling::None;
        }
        if variants.iter().all(|v| v.name != decl.name) {
            SumSpelling::Alias
        } else if variants.len() == 1 {
            SumSpelling::Class
        } else {
            // A constructor that shares its type's name, beside others: the
            // alias would shadow the class.
            SumSpelling::None
        }
    }
}

enum SumSpelling {
    Alias,
    Class,
    None,
}

/// The typing names an annotation used, to import.
#[derive(Default)]
struct Uses {
    any: bool,
    awaitable: bool,
    callable: bool,
    container: bool,
    iterable: bool,
    /// The imports a spelled type went through (`geo`, for `geo.Shape`), each
    /// as the text after `import`.
    modules: BTreeSet<String>,
}

/// The indices of `params` that occur in `fields`: the type parameters a
/// class over those fields needs.
fn params_used(params: &[String], fields: &[&TypeExpr]) -> Vec<usize> {
    fn mentions(te: &TypeExpr, name: &str) -> bool {
        match te {
            TypeExpr::Con(n, _, args) => {
                (args.is_empty() && n == name) || args.iter().any(|a| mentions(a, name))
            }
            TypeExpr::Fun(a, b, _) => mentions(a, name) || mentions(b, name),
            TypeExpr::Tuple(elems) => elems.iter().any(|e| mentions(e, name)),
        }
    }
    params
        .iter()
        .enumerate()
        .filter(|(_, p)| fields.iter().any(|f| mentions(f, p)))
        .map(|(i, _)| i)
        .collect()
}

/// Spells types as Python annotations.
struct Speller<'a> {
    decls: &'a Decls<'a>,
    uses: &'a mut Uses,
    /// The type variables met so far, in order, when they become type
    /// parameters; `None` where there is nowhere to declare one (a value),
    /// and a variable is `Any`.
    vars: Option<Vec<Param>>,
    /// The numeric variables a function is polymorphic in, which become type
    /// parameters held to `int` and `float`. Any other numeric variable
    /// defaults to `int`, as it does when it runs.
    numbers: HashSet<u32>,
    /// The variables an `in` searches, each with what it is searched for; and
    /// those of them the type being spelled names once, which are spelled in
    /// place as what Python's `in` takes.
    searched: Option<&'a HashMap<u32, Vec<Ty>>>,
    once: HashSet<u32>,
}

/// A type variable that became a type parameter: an ordinary one, a numeric
/// one (the two are numbered apart by the checker), or one that an `in`
/// searches and the type names more than once.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Param {
    Type(u32),
    Number(u32),
    Searched(u32),
}

/// The type variables `ty` names exactly once.
fn named_once(ty: &Ty) -> HashSet<u32> {
    fn count(ty: &Ty, counts: &mut HashMap<u32, usize>) {
        match ty {
            Ty::Var(v) => *counts.entry(*v).or_default() += 1,
            Ty::Fun(arg, result, _) => {
                count(arg, counts);
                count(result, counts);
            }
            Ty::Con(_, args) | Ty::Tuple(args) => args.iter().for_each(|a| count(a, counts)),
            Ty::Int(_) | Ty::Float(_) | Ty::Num(..) | Ty::Bool | Ty::Str | Ty::Unit => {}
        }
    }
    let mut counts = HashMap::new();
    count(ty, &mut counts);
    counts
        .into_iter()
        .filter(|(_, n)| *n == 1)
        .map(|(v, _)| v)
        .collect()
}

impl Speller<'_> {
    fn any(&mut self) -> String {
        self.uses.any = true;
        "Any".to_string()
    }

    /// The name of a type parameter, declaring it on first sight.
    fn param(&mut self, param: Param) -> String {
        let vars = self.vars.as_mut().expect("type parameters are in use");
        let index = vars
            .iter()
            .position(|seen| *seen == param)
            .unwrap_or_else(|| {
                vars.push(param);
                vars.len() - 1
            });
        self.decls.param_name(index)
    }

    /// The type parameter list the spelled types need (`A`, `B: (int,
    /// float)`).
    fn type_params(&self) -> Vec<String> {
        self.vars
            .iter()
            .flatten()
            .enumerate()
            .map(|(index, param)| {
                let name = self.decls.param_name(index);
                match param {
                    Param::Type(_) => name,
                    Param::Number(_) => format!("{name}: (int, float)"),
                    Param::Searched(_) => format!("{name}: Iterable[Any]"),
                }
            })
            .collect()
    }

    /// A variable that an `in` searches for `sought`. Python's `in` takes a
    /// `Container` of what is sought, or an `Iterable` of it (a `Seq` is an
    /// iterator, which has no `__contains__`): where the type names the
    /// variable once it is spelled as that. Where the type names it again
    /// (the function hands back what it searched), it stays a type parameter
    /// so that the two agree, held to `Iterable`, which every type `in`
    /// searches is; a bound cannot name another type parameter, so what is
    /// sought goes unsaid there.
    fn searched_var(&mut self, var: u32, sought: &[Ty]) -> String {
        if !self.once.contains(&var) {
            self.uses.iterable = true;
            self.uses.any = true;
            return self.param(Param::Searched(var));
        }
        let mut parts: Vec<String> = Vec::new();
        for elem in sought {
            let spelled = self.ty(elem);
            if !parts.contains(&spelled) {
                parts.push(spelled);
            }
        }
        let elem = parts.join(" | ");
        self.uses.iterable = true;
        // A file with a `Container` of its own keeps the name; every type
        // `in` searches is an `Iterable` too.
        if self.decls.taken.contains("Container") {
            return format!("Iterable[{elem}]");
        }
        self.uses.container = true;
        format!("Container[{elem}] | Iterable[{elem}]")
    }

    /// The declared type of an `extern`, as the type of its binding: a
    /// `Callable` of all its arguments at once, which is how the binding is
    /// called. A function's `unit` result is whatever the Python target hands
    /// back, which nothing reads: `object`. An attribute read (`= .with_name`)
    /// is bound `by_receiver`: the binding takes the receiver alone and hands
    /// back the attribute, which takes the rest.
    fn extern_type(&mut self, te: &TypeExpr, by_receiver: bool) -> String {
        let env = HashMap::new();
        if !matches!(te, TypeExpr::Fun(..)) {
            return self.type_expr(te, &env);
        }
        if let TypeExpr::Fun(receiver, attribute, _) = te
            && by_receiver
        {
            let receiver = self.type_expr(receiver, &env);
            let attribute = self.extern_type(attribute, false);
            self.uses.callable = true;
            return format!("Callable[[{receiver}], {attribute}]");
        }
        let mut params = Vec::new();
        let mut rest = te;
        while let TypeExpr::Fun(arg, result, _) = rest {
            params.push(self.type_expr(arg, &env));
            rest = result;
        }
        let result = match rest {
            TypeExpr::Con(name, _, args) if name == "unit" && args.is_empty() => {
                "object".to_string()
            }
            other => self.type_expr(other, &env),
        };
        self.uses.callable = true;
        format!("Callable[[{}], {result}]", params.join(", "))
    }

    /// A checker type.
    fn ty(&mut self, ty: &Ty) -> String {
        match ty {
            Ty::Int(_) => "int".to_string(),
            Ty::Num(v, _) => {
                if self.numbers.contains(v) && self.vars.is_some() {
                    self.param(Param::Number(*v))
                } else {
                    "int".to_string()
                }
            }
            Ty::Float(_) => "float".to_string(),
            Ty::Bool => "bool".to_string(),
            Ty::Str => "str".to_string(),
            Ty::Unit => "None".to_string(),
            Ty::Var(v) => {
                if self.vars.is_none() {
                    return self.any();
                }
                match self.searched.and_then(|searched| searched.get(v)) {
                    Some(sought) => self.searched_var(*v, sought),
                    None => self.param(Param::Type(*v)),
                }
            }
            Ty::Tuple(elems) => {
                let parts: Vec<String> = elems.iter().map(|e| self.ty(e)).collect();
                format!("tuple[{}]", parts.join(", "))
            }
            // A function is called with all of its arguments at once.
            Ty::Fun(..) => {
                let mut params = Vec::new();
                let mut rest = ty;
                while let Ty::Fun(arg, result, _) = rest {
                    params.push(self.ty(arg));
                    rest = result;
                }
                let result = self.ty(rest);
                self.uses.callable = true;
                format!("Callable[[{}], {result}]", params.join(", "))
            }
            Ty::Con(name, args) => {
                let args: Vec<String> = args.iter().map(|a| self.ty(a)).collect();
                self.constructor(name, &args)
            }
        }
    }

    /// A declared type, with the declaration's parameters standing for the
    /// spellings in `env`.
    fn type_expr(&mut self, te: &TypeExpr, env: &HashMap<&str, String>) -> String {
        match te {
            TypeExpr::Con(name, _, args) => {
                if args.is_empty()
                    && let Some(spelled) = env.get(name.as_str())
                {
                    return spelled.clone();
                }
                // A unit of measure is written on the name (`float<m>`).
                let base = name.split('<').next().unwrap_or(name);
                match base {
                    "int" => "int".to_string(),
                    "float" => "float".to_string(),
                    "bool" => "bool".to_string(),
                    "str" => "str".to_string(),
                    "unit" => "None".to_string(),
                    _ => {
                        let args: Vec<String> =
                            args.iter().map(|a| self.type_expr(a, env)).collect();
                        self.constructor(base, &args)
                    }
                }
            }
            TypeExpr::Fun(..) => {
                let mut params = Vec::new();
                let mut rest = te;
                while let TypeExpr::Fun(arg, result, _) = rest {
                    params.push(self.type_expr(arg, env));
                    rest = result;
                }
                let result = self.type_expr(rest, env);
                self.uses.callable = true;
                format!("Callable[[{}], {result}]", params.join(", "))
            }
            TypeExpr::Tuple(elems) => {
                let parts: Vec<String> = elems.iter().map(|e| self.type_expr(e, env)).collect();
                format!("tuple[{}]", parts.join(", "))
            }
        }
    }

    /// The type constructor `name` applied to already-spelled `args`.
    fn constructor(&mut self, name: &str, args: &[String]) -> String {
        let applied = |base: &str, args: &[String]| {
            if args.is_empty() {
                base.to_string()
            } else {
                format!("{base}[{}]", args.join(", "))
            }
        };
        match (name, args) {
            ("List", [a]) => format!("list[{a}]"),
            ("Set", [a]) => format!("set[{a}]"),
            ("Map", [k, v]) => format!("dict[{k}, {v}]"),
            ("Option", [a]) => format!("Some[{a}] | None_"),
            ("Result", [a, e]) => format!("Ok[{a}] | Error[{e}]"),
            ("Seq", [a]) => {
                self.uses.iterable = true;
                format!("Iterable[{a}]")
            }
            // What `async { }` builds is awaited; it is not itself a coroutine.
            ("Async", [a]) => {
                self.uses.awaitable = true;
                format!("Awaitable[{a}]")
            }
            // Parsed JSON goes by its alias, where this file has it.
            ("Json", []) if self.decls.taken.contains("JObject") => "Json".to_string(),
            // A caught exception is the prelude's record of it.
            ("Exception", []) if self.decls.taken.contains("_Exception") => {
                "_Exception".to_string()
            }
            _ => {
                // The module's own type, or one of an imported module's, which
                // is reached through that module.
                let (decl, prefix, import) = match self.decls.types.get(name) {
                    Some(decl) => (*decl, String::new(), None),
                    // An imported type is known by its module, `Geometry.Point`.
                    // Written bare in a declaration, it is the one import's
                    // type of that name.
                    None => match name
                        .rsplit_once('.')
                        .and_then(|(m, bare)| {
                            let key = format!("{}.{bare}", crate::project::module_stem(m));
                            self.decls.imported.get(&key)
                        })
                        .or_else(|| {
                            let mut of_name =
                                self.decls.imported.values().filter(|(_, d)| d.name == name);
                            match (of_name.next(), of_name.next()) {
                                (Some(only), None) => Some(only),
                                _ => None,
                            }
                        }) {
                        Some((module, decl)) => match self.decls.reached.get(module) {
                            Some((by, import)) => (*decl, format!("{by}."), Some(import)),
                            None => return self.any(),
                        },
                        None => return self.any(),
                    },
                };
                if decl.params.len() != args.len() {
                    return self.any();
                }
                let through = |uses: &mut Uses| {
                    if let Some(import) = import {
                        uses.modules.insert(import.clone());
                    }
                };
                match &decl.kind {
                    TypeDeclKind::Record(fields) => {
                        let fields: Vec<&TypeExpr> = fields.iter().map(|f| &f.ty).collect();
                        let used = params_used(&decl.params, &fields);
                        let args: Vec<String> = used.iter().map(|i| args[*i].clone()).collect();
                        through(self.uses);
                        applied(&format!("{prefix}{}", py_record_class(&decl.name)), &args)
                    }
                    TypeDeclKind::Sum(variants) => match self.decls.sum_spelling(decl) {
                        SumSpelling::Alias => {
                            through(self.uses);
                            applied(&format!("{prefix}{}", decl.name), args)
                        }
                        SumSpelling::Class => {
                            let fields: Vec<&TypeExpr> = variants[0].fields.iter().collect();
                            let used = params_used(&decl.params, &fields);
                            let args: Vec<String> = used.iter().map(|i| args[*i].clone()).collect();
                            through(self.uses);
                            applied(
                                &format!("{prefix}{}", py_ctor_name(&variants[0].name)),
                                &args,
                            )
                        }
                        SumSpelling::None => self.any(),
                    },
                    // A newtype is its underlying value at run time.
                    TypeDeclKind::Newtype(underlying) => {
                        let env: HashMap<&str, String> = decl
                            .params
                            .iter()
                            .map(String::as_str)
                            .zip(args.iter().cloned())
                            .collect();
                        self.type_expr(underlying, &env)
                    }
                    TypeDeclKind::Opaque => self.any(),
                }
            }
        }
    }
}

/// Give each class its fields' types and its type parameters: the classes of
/// the module's own types, and the prelude's `Some`/`Ok`/`Error`.
fn annotate_classes(py: &mut PyModule, decls: &Decls, uses: &mut Uses) {
    // Each class this module's types lower to: its declaration, and the
    // field types in the class's field order.
    let mut shapes: HashMap<String, (&TypeDecl, Vec<&TypeExpr>)> = HashMap::new();
    for decl in decls.types.values() {
        match &decl.kind {
            TypeDeclKind::Record(fields) => {
                shapes.insert(
                    py_record_class(&decl.name),
                    (decl, fields.iter().map(|f| &f.ty).collect()),
                );
            }
            TypeDeclKind::Sum(variants) => {
                for v in variants {
                    shapes.insert(py_ctor_name(&v.name), (decl, v.fields.iter().collect()));
                }
            }
            TypeDeclKind::Newtype(_) | TypeDeclKind::Opaque => {}
        }
    }
    for stmt in &mut py.body {
        let PyStmt::ClassDef {
            name,
            field_types,
            type_params,
            ..
        } = stmt
        else {
            continue;
        };
        // The prelude's own generic classes.
        let prelude = match name.as_str() {
            "Some" | "Ok" => Some("A"),
            "Error" => Some("E"),
            _ => None,
        };
        if let Some(param) = prelude
            && !shapes.contains_key(name.as_str())
            && field_types.len() == 1
        {
            *type_params = vec![param.to_string()];
            field_types[0] = param.to_string();
            continue;
        }
        let Some((decl, fields)) = shapes.get(name.as_str()) else {
            continue;
        };
        if fields.len() != field_types.len() {
            continue;
        }
        let used = params_used(&decl.params, fields);
        let names: Vec<String> = (0..used.len()).map(|i| decls.param_name(i)).collect();
        let env: HashMap<&str, String> = used
            .iter()
            .zip(&names)
            .map(|(i, name)| (decl.params[*i].as_str(), name.clone()))
            .collect();
        let mut speller = Speller {
            decls,
            uses,
            vars: None,
            numbers: HashSet::new(),
            searched: None,
            once: HashSet::new(),
        };
        for (slot, field) in field_types.iter_mut().zip(fields) {
            *slot = speller.type_expr(field, &env);
        }
        *type_params = names;
    }
}

/// Add `type Shape = Circle | Rect` after the classes of each sum type that
/// takes an alias.
fn add_aliases(py: &mut PyModule, decls: &Decls, uses: &mut Uses) {
    let mut names: Vec<&str> = decls.types.keys().copied().collect();
    names.sort_unstable();
    for name in names {
        let decl = decls.types[name];
        let TypeDeclKind::Sum(variants) = &decl.kind else {
            continue;
        };
        if !matches!(decls.sum_spelling(decl), SumSpelling::Alias) {
            continue;
        }
        let classes: Vec<String> = variants.iter().map(|v| py_ctor_name(&v.name)).collect();
        // Only where every constructor's class is in this file.
        let Some(last) = py
            .body
            .iter()
            .rposition(|s| matches!(s, PyStmt::ClassDef { name, .. } if classes.contains(name)))
        else {
            continue;
        };
        let present = classes.iter().all(|class| {
            py.body
                .iter()
                .any(|s| matches!(s, PyStmt::ClassDef { name, .. } if name == class))
        });
        if !present {
            continue;
        }
        let params: Vec<String> = (0..decl.params.len())
            .map(|i| decls.param_name(i))
            .collect();
        let members: Vec<String> = variants
            .iter()
            .map(|v| {
                let fields: Vec<&TypeExpr> = v.fields.iter().collect();
                let used: Vec<String> = params_used(&decl.params, &fields)
                    .into_iter()
                    .map(|i| params[i].clone())
                    .collect();
                let class = py_ctor_name(&v.name);
                if used.is_empty() {
                    class
                } else {
                    format!("{class}[{}]", used.join(", "))
                }
            })
            .collect();
        let head = if params.is_empty() {
            format!("type {}", decl.name)
        } else {
            format!("type {}[{}]", decl.name, params.join(", "))
        };
        // A nullary constructor's singleton follows its class; the alias
        // goes after it.
        let at = last
            + 1
            + py.body[last + 1..]
                .iter()
                .take_while(
                    |s| matches!(s, PyStmt::Assign { target, .. } if target.starts_with('_')),
                )
                .count();
        py.body.insert(
            at,
            PyStmt::Assign {
                target: head,
                value: PyExpr::Name(members.join(" | ")),
            },
        );
        let _ = &uses;
    }
}

/// Every binding that lowers to a top-level Python name, with that name: the
/// module's own `let`s, and the members of an in-file `module`, which are
/// `Module_member` in the emitted file.
fn top_bindings(module: &Module) -> Vec<(String, &crate::parser::ast::LetBinding)> {
    let mut out = Vec::new();
    for item in &module.items {
        match item {
            Item::Let(binding) => {
                if let Some(name) = binding.name() {
                    out.push((py_value_name(name), binding));
                }
            }
            Item::Module { name, items, .. } => {
                for binding in items {
                    if let Some(member) = binding.name() {
                        out.push((format!("{name}_{}", py_value_name(member)), binding));
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// Annotate each top-level value bound once: `origin: Point = Point(0, 0)`.
fn annotate_values(
    module: &Module,
    py: &mut PyModule,
    decls: &Decls,
    binding_types: &HashMap<Span, Ty>,
    uses: &mut Uses,
) {
    let mut types: HashMap<String, &Ty> = HashMap::new();
    let mut bound: HashMap<String, usize> = HashMap::new();
    for item in &module.items {
        // A destructuring `let` binds names too, and none of them is annotated.
        if let Item::Let(binding) = item
            && binding.name().is_none()
        {
            for name in binding.bound_names() {
                *bound.entry(py_value_name(&name)).or_default() += 2;
            }
        }
    }
    for (py_name, binding) in top_bindings(module) {
        *bound.entry(py_name.clone()).or_default() += 1;
        if binding.params.is_empty()
            && let Some(ty) = binding_types.get(&binding.target_span.span())
        {
            types.insert(py_name, ty);
        }
    }
    let mut seen: HashSet<String> = HashSet::new();
    for stmt in &mut py.body {
        let PyStmt::Assign { target, .. } = stmt else {
            continue;
        };
        // Only a name's first assignment (a `let mut` is assigned again).
        if !seen.insert(target.clone()) || bound.get(target.as_str()) != Some(&1) {
            continue;
        }
        let Some(ty) = types.get(target.as_str()) else {
            continue;
        };
        let mut speller = Speller {
            decls,
            uses,
            vars: None,
            numbers: HashSet::new(),
            searched: None,
            once: HashSet::new(),
        };
        let spelled = speller.ty(ty);
        *target = format!("{target}: {spelled}");
    }
}

/// The numeric variables in an argument position of a function's type, at
/// any depth: the ones its caller chooses.
fn argument_numbers(ty: &Ty, in_argument: bool, out: &mut HashSet<u32>) {
    match ty {
        Ty::Num(v, _) => {
            if in_argument {
                out.insert(*v);
            }
        }
        Ty::Fun(arg, result, _) => {
            argument_numbers(arg, true, out);
            argument_numbers(result, in_argument, out);
        }
        Ty::Con(_, args) => args
            .iter()
            .for_each(|a| argument_numbers(a, in_argument, out)),
        Ty::Tuple(elems) => elems
            .iter()
            .for_each(|e| argument_numbers(e, in_argument, out)),
        Ty::Int(_) | Ty::Float(_) | Ty::Bool | Ty::Str | Ty::Unit | Ty::Var(_) => {}
    }
}

/// Annotate each top-level function from its binding's type.
fn annotate_functions(
    module: &Module,
    py: &mut PyModule,
    decls: &Decls,
    binding_types: &HashMap<Span, Ty>,
    searched: &HashMap<u32, Vec<Ty>>,
    uses: &mut Uses,
) {
    let mut functions: HashMap<String, (usize, &Ty)> = HashMap::new();
    for (py_name, binding) in top_bindings(module) {
        if !binding.params.is_empty()
            && let Some(ty) = binding_types.get(&binding.target_span.span())
        {
            functions.insert(py_name, (binding.params.len(), ty));
        }
    }
    for stmt in &mut py.body {
        let PyStmt::FuncDef {
            name,
            params,
            body,
            is_async,
        } = stmt
        else {
            continue;
        };
        let Some((arity, ty)) = functions.get(name.as_str()) else {
            continue;
        };
        if params.len() != *arity {
            continue;
        }
        let mut numbers = HashSet::new();
        argument_numbers(ty, false, &mut numbers);
        let mut speller = Speller {
            decls,
            uses,
            vars: Some(Vec::new()),
            numbers,
            searched: Some(searched),
            once: named_once(ty),
        };
        let mut annotated = Vec::with_capacity(params.len());
        let mut rest: &Ty = ty;
        for param in params.iter() {
            let Ty::Fun(arg, result, _) = rest else { break };
            annotated.push((param.clone(), Some(speller.ty(arg))));
            rest = result;
        }
        if annotated.len() != params.len() {
            continue;
        }
        // An `async def` hands back a coroutine of what it returns.
        let returns = match rest {
            Ty::Con(name, args) if *is_async && name == "Async" && args.len() == 1 => {
                speller.ty(&args[0])
            }
            other => speller.ty(other),
        };
        let type_params = speller.type_params();
        *stmt = PyStmt::TypedFuncDef {
            name: name.clone(),
            type_params,
            params: annotated,
            returns: Some(returns),
            body: std::mem::take(body),
            is_async: *is_async,
        };
    }
}

/// Annotate each `extern` the module binds at top level with the type it was
/// declared with. A project binds them, so that another module can reach one
/// (`DESIGN.md` §6.1); a single file reads each through its Python target and
/// has no binding to annotate.
fn annotate_externs(module: &Module, py: &mut PyModule, decls: &Decls, uses: &mut Uses) {
    let declared: HashMap<String, &ExternDecl> = module
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Extern(decl) => Some((py_value_name(&decl.name), decl)),
            _ => None,
        })
        .collect();
    if declared.is_empty() {
        return;
    }
    let mut bound: HashMap<String, usize> = HashMap::new();
    for stmt in &py.body {
        if let PyStmt::Assign { target, .. } = stmt {
            *bound.entry(target.clone()).or_default() += 1;
        }
    }
    for stmt in &mut py.body {
        let PyStmt::Assign { target, value } = stmt else {
            continue;
        };
        let Some(decl) = declared.get(target.as_str()) else {
            continue;
        };
        if bound.get(target.as_str()) != Some(&1) {
            continue;
        }
        // How the binding is called is read off the binding itself. A lambda
        // of fixed parameters takes them all at once, one for each arrow; the
        // one that reads an attribute takes its receiver alone.
        let arrows = {
            let mut count = 0;
            let mut rest = &decl.ty;
            while let TypeExpr::Fun(_, result, _) = rest {
                count += 1;
                rest = result;
            }
            count
        };
        let by_receiver = match value {
            PyExpr::Lambda { params, .. } if params.iter().any(|p| p.starts_with('*')) => false,
            PyExpr::Lambda { params, .. } if params.len() == arrows => false,
            PyExpr::Lambda { params, .. } if params.len() == 1 && arrows > 1 => true,
            PyExpr::Lambda { .. } => continue,
            _ => false,
        };
        let mut speller = Speller {
            decls,
            uses,
            vars: None,
            numbers: HashSet::new(),
            searched: None,
            once: HashSet::new(),
        };
        let spelled = speller.extern_type(&decl.ty, by_receiver);
        *target = format!("{target}: {spelled}");
    }
}

/// Whether `name` is a temporary of the lowering's (`_pf_t3`).
fn is_temp(name: &str) -> bool {
    name.strip_prefix("_pf_t")
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// The statement lists nested in `stmt` that belong to its own scope: the
/// branches of an `if`, a `match` or a `try`, and the body of a loop or a
/// `with`. A `def` or a class opens a scope of its own and has none.
fn blocks(stmt: &mut PyStmt) -> Vec<&mut Vec<PyStmt>> {
    match stmt {
        PyStmt::If { body, orelse, .. } => vec![body, orelse],
        PyStmt::Match { cases, .. } => cases.iter_mut().map(|case| &mut case.body).collect(),
        PyStmt::Try { body, handler, .. } => vec![body, handler],
        PyStmt::For { body, .. } | PyStmt::WhileTrue { body } | PyStmt::AsyncWith { body, .. } => {
            vec![body]
        }
        _ => Vec::new(),
    }
}

/// A top-level value of type `unit` is `None`, and what computes it runs as a
/// statement for its effect: `say(1)` and then `start: None = None`, where the
/// plain output writes `start = say(1)`. A type checker refuses the result of
/// a function that hands nothing back, and what an `extern` hands back there
/// need not be `None` at all. A value built by a `match` or an `if` reaches
/// its name through a temporary assigned in each branch; each branch then
/// runs its call and assigns nothing.
fn settle_unit_values(py: &mut PyModule) {
    /// Turn each `temp = value` into the statement `value`.
    fn run_for_effect(stmts: &mut [PyStmt], temp: &str) {
        for stmt in stmts {
            if let PyStmt::Assign { target, value } = stmt
                && target == temp
            {
                *stmt = PyStmt::Expr(std::mem::replace(value, PyExpr::NoneLit));
                continue;
            }
            for block in blocks(stmt) {
                run_for_effect(block, temp);
            }
        }
    }
    let mut i = 0;
    while i < py.body.len() {
        let PyStmt::Assign { target, value } = &mut py.body[i] else {
            i += 1;
            continue;
        };
        if !target.ends_with(": None") {
            i += 1;
            continue;
        }
        match value {
            // Already `None`, or another name for it.
            PyExpr::NoneLit => {}
            PyExpr::Name(name) if !is_temp(name) => {}
            PyExpr::Name(name) => {
                let temp = name.clone();
                *value = PyExpr::NoneLit;
                run_for_effect(&mut py.body[..i], &temp);
            }
            _ => {
                let run = std::mem::replace(value, PyExpr::NoneLit);
                py.body.insert(i, PyStmt::Expr(run));
                i += 1;
            }
        }
        i += 1;
    }
}

/// A function that returns `unit` returns `None`. Where its last expression is
/// a call that a type checker knows to hand back `None` (one of this file's
/// own `-> None` functions, or `print`), `return call(...)` stands as the
/// plain output has it. Any other is run as a statement, with the `return`
/// after it (left out at the end of the function): the call may be to an
/// `extern` declared `unit` whose Python target hands something back, which a
/// type checker refuses as the result of a `-> None` function. A name is not
/// run at all: `return r` is `return`, whatever `r` holds.
fn settle_unit_returns(py: &mut PyModule) {
    fn hands_back_none(value: &PyExpr, nothing: &HashSet<String>) -> bool {
        match value {
            PyExpr::NoneLit => true,
            PyExpr::Await(inner) => hands_back_none(inner, nothing),
            PyExpr::Call { func, .. } => {
                matches!(&**func, PyExpr::Name(f) if f == "print" || nothing.contains(f))
            }
            _ => false,
        }
    }
    /// Returns whether the last statement of `stmts` is a `return` put there.
    fn split(stmts: &mut Vec<PyStmt>, nothing: &HashSet<String>) -> bool {
        let mut added_last = false;
        let mut i = 0;
        while i < stmts.len() {
            added_last = false;
            if let PyStmt::Return(value) = &mut stmts[i] {
                if matches!(value, PyExpr::Name(_)) {
                    // A name for what an earlier statement computed: reading
                    // it does nothing, so the `return` alone is left.
                    *value = PyExpr::NoneLit;
                    added_last = true;
                } else if !hands_back_none(value, nothing) {
                    let run = std::mem::replace(value, PyExpr::NoneLit);
                    stmts.insert(i, PyStmt::Expr(run));
                    i += 1;
                    added_last = true;
                }
            } else {
                for block in blocks(&mut stmts[i]) {
                    split(block, nothing);
                }
            }
            i += 1;
        }
        added_last
    }
    let nothing: HashSet<String> = py
        .body
        .iter()
        .filter_map(|stmt| match stmt {
            PyStmt::TypedFuncDef {
                name,
                returns: Some(returns),
                ..
            } if returns == "None" => Some(name.clone()),
            _ => None,
        })
        .collect();
    for stmt in &mut py.body {
        let PyStmt::TypedFuncDef {
            returns: Some(returns),
            body,
            ..
        } = stmt
        else {
            continue;
        };
        if returns != "None" {
            continue;
        }
        // The `return` put at the very end says nothing; a body keeps a
        // statement to stand on.
        if split(body, &nothing) && body.len() > 1 {
            body.pop();
        }
    }
}

/// An empty collection as a value, by the head of its type and that type
/// with nothing said of what it holds.
fn empty_collection(value: &PyExpr) -> Option<(&'static str, &'static str)> {
    let kinds = [
        ("dict", "dict[", "dict[Any, Any]"),
        ("list", "list[", "list[Any]"),
        ("set", "set[", "set[Any]"),
    ];
    match value {
        PyExpr::List(elems) if elems.is_empty() => Some(("list[", "list[Any]")),
        PyExpr::Dict(entries) if entries.is_empty() => Some(("dict[", "dict[Any, Any]")),
        PyExpr::Call { func, args } if args.is_empty() => match &**func {
            PyExpr::Name(f) => kinds
                .iter()
                .find(|(name, _, _)| name == f)
                .map(|(_, head, open)| (*head, *open)),
            _ => None,
        },
        _ => None,
    }
}

/// One Python scope being read top to bottom: the module, or a function.
struct Scope<'a> {
    /// The names bound so far. A name is declared at its first binding only.
    seen: HashSet<String>,
    /// The names the scope declares `global` or `nonlocal`, which take no
    /// annotation here.
    elsewhere: HashSet<String>,
    /// The function's result type, and the names it returns: such a name has
    /// that type.
    returns: Option<&'a str>,
    returned: HashSet<String>,
}

impl<'a> Scope<'a> {
    fn of(params: &[String], returns: Option<&'a str>, body: &mut [PyStmt]) -> Scope<'a> {
        fn survey(stmts: &mut [PyStmt], scope: &mut Scope) {
            for stmt in stmts {
                match stmt {
                    PyStmt::Global(names) | PyStmt::Nonlocal(names) => {
                        scope.elsewhere.extend(names.iter().cloned());
                    }
                    PyStmt::Return(PyExpr::Name(name)) => {
                        scope.returned.insert(name.clone());
                    }
                    _ => {}
                }
                for block in blocks(stmt) {
                    survey(block, scope);
                }
            }
        }
        let mut scope = Scope {
            seen: params
                .iter()
                .map(|p| p.trim_start_matches('*').to_string())
                .collect(),
            elsewhere: HashSet::new(),
            returns,
            returned: HashSet::new(),
        };
        survey(body, &mut scope);
        scope
    }

    /// Whether `target` is a bare name meeting its first binding here.
    fn declares(&self, target: &str) -> bool {
        !target.contains([':', ' '])
            && !self.seen.contains(target)
            && !self.elsewhere.contains(target)
    }

    fn bind_pattern(&mut self, pattern: &PyPattern) {
        match pattern {
            PyPattern::Wildcard | PyPattern::Literal(_) => {}
            PyPattern::Capture(name) => {
                self.seen.insert(name.clone());
            }
            PyPattern::Class { args, .. } => args.iter().for_each(|p| self.bind_pattern(p)),
            PyPattern::ClassKw { fields, .. } => {
                fields.iter().for_each(|(_, p)| self.bind_pattern(p));
            }
            PyPattern::Sequence(elems) | PyPattern::Or(elems) => {
                elems.iter().for_each(|p| self.bind_pattern(p));
            }
            PyPattern::ListSeq {
                elems,
                star,
                suffix,
            } => {
                elems.iter().for_each(|p| self.bind_pattern(p));
                self.seen.extend(star.iter().cloned());
                suffix.iter().for_each(|p| self.bind_pattern(p));
            }
            PyPattern::As { pattern, name } => {
                self.bind_pattern(pattern);
                self.seen.insert(name.clone());
            }
        }
    }

    /// Read `stmts` in order, declaring the two kinds of local a type checker
    /// cannot work out for itself (see [`annotate_locals`]).
    fn read(&mut self, stmts: &mut [PyStmt], uses: &mut Uses) {
        for stmt in stmts {
            match stmt {
                PyStmt::Assign { target, value } => {
                    if self.declares(target)
                        && let Some((head, open)) = empty_collection(value)
                    {
                        let spelled = match self.returns {
                            Some(returns)
                                if self.returned.contains(target.as_str())
                                    && returns.starts_with(head) =>
                            {
                                returns.to_string()
                            }
                            _ => {
                                uses.any = true;
                                open.to_string()
                            }
                        };
                        self.seen.insert(target.clone());
                        *target = format!("{target}: {spelled}");
                    } else {
                        let name = target.split(':').next().unwrap_or(target).to_string();
                        self.seen.insert(name);
                    }
                }
                PyStmt::UnpackAssign { targets, .. } => self.seen.extend(targets.iter().cloned()),
                PyStmt::For { target, body, .. } => {
                    self.seen.extend(target.names());
                    self.read(body, uses);
                }
                PyStmt::AsyncWith { binding, body, .. } => {
                    self.seen.insert(binding.clone());
                    self.read(body, uses);
                }
                PyStmt::Match { cases, .. } => {
                    for PyCase { pattern, body, .. } in cases {
                        self.bind_pattern(pattern);
                        self.read(body, uses);
                    }
                }
                PyStmt::Try {
                    body,
                    binding,
                    handler,
                    ..
                } => {
                    self.declare_tried(body, handler, uses);
                    self.read(body, uses);
                    self.seen.extend(binding.iter().cloned());
                    self.read(handler, uses);
                }
                PyStmt::FuncDef { name, .. }
                | PyStmt::TypedFuncDef { name, .. }
                | PyStmt::Class { name, .. }
                | PyStmt::ClassDef { name, .. } => {
                    self.seen.insert(name.clone());
                }
                PyStmt::Import(spec) => {
                    let name = match spec.split_once(" as ") {
                        Some((_, alias)) => alias,
                        None => spec.split('.').next().unwrap_or(spec),
                    };
                    self.seen.insert(name.to_string());
                }
                PyStmt::ImportFrom { names, .. } => self.seen.extend(names.iter().cloned()),
                other => {
                    for block in blocks(other) {
                        self.read(block, uses);
                    }
                }
            }
        }
    }

    /// `try e` lowers to a temporary assigned an `Ok` in the `try` and an
    /// `Error` in the `except`: declare it as either, at the first.
    fn declare_tried(&mut self, body: &mut [PyStmt], handler: &[PyStmt], uses: &mut Uses) {
        fn built_by<'e>(value: &'e PyExpr, class: &str) -> Option<&'e [PyExpr]> {
            match value {
                PyExpr::Call { func, args } if matches!(&**func, PyExpr::Name(f) if f == class) => {
                    Some(args)
                }
                _ => None,
            }
        }
        let (
            Some(PyStmt::Assign { target, value }),
            [
                PyStmt::Assign {
                    target: caught,
                    value: error,
                },
            ],
        ) = (body.last_mut(), handler)
        else {
            return;
        };
        if target != caught || !is_temp(target) || !self.declares(target) {
            return;
        }
        let (Some(_), Some(payload)) = (built_by(value, "Ok"), built_by(error, "Error")) else {
            return;
        };
        let exception = py_record_class("Exception");
        let failed = match payload {
            [inner] if built_by(inner, &exception).is_some() => exception,
            _ => "Any".to_string(),
        };
        uses.any = true;
        *target = format!("{target}: Ok[Any] | Error[{failed}]");
    }
}

/// Declare the locals whose types a type checker cannot work out, in the
/// module's own statements and in each annotated function:
///
/// - an accumulator that starts as an empty collection and is read before
///   anything is put in it (the loop a `List.fold` becomes): the function's
///   result type where the function returns it, and otherwise the collection
///   with nothing said of what it holds (`m: dict[Any, Any] = dict()`);
/// - the temporary of a `try e`, which is an `Ok` on one path and an `Error`
///   on the other (`_pf_t0: Ok[Any] | Error[_Exception]`).
///
/// A function the pass left without annotations is not read: a type checker
/// does not look inside one either.
fn annotate_locals(py: &mut PyModule, uses: &mut Uses) {
    let mut module = Scope::of(&[], None, &mut py.body);
    module.read(&mut py.body, uses);
    for stmt in &mut py.body {
        let PyStmt::TypedFuncDef {
            params,
            returns,
            body,
            ..
        } = stmt
        else {
            continue;
        };
        let names: Vec<String> = params.iter().map(|(name, _)| name.clone()).collect();
        let mut scope = Scope::of(&names, returns.as_deref(), body);
        scope.read(body, uses);
    }
}

/// Import what the annotations named, and make every annotation lazy so that
/// one may name a class or an alias defined further down the file.
fn add_imports(py: &mut PyModule, uses: &Uses) {
    let mut abc = BTreeSet::new();
    if uses.awaitable {
        abc.insert("Awaitable");
    }
    if uses.callable {
        abc.insert("Callable");
    }
    if uses.container {
        abc.insert("Container");
    }
    if uses.iterable {
        abc.insert("Iterable");
    }
    let mut imports = vec![PyStmt::ImportFrom {
        module: "__future__".to_string(),
        names: vec!["annotations".to_string()],
    }];
    if !abc.is_empty() {
        imports.push(PyStmt::ImportFrom {
            module: "collections.abc".to_string(),
            names: abc.into_iter().map(str::to_string).collect(),
        });
    }
    let has_any = py.body.iter().any(|s| {
        matches!(s, PyStmt::ImportFrom { module, names }
            if module == "typing" && names.iter().any(|n| n == "Any"))
    });
    if uses.any && !has_any {
        imports.push(PyStmt::ImportFrom {
            module: "typing".to_string(),
            names: vec!["Any".to_string()],
        });
    }
    // A module a type was spelled through, when nothing in the code already
    // made the file import it.
    for module in &uses.modules {
        let present = py
            .body
            .iter()
            .any(|s| matches!(s, PyStmt::Import(name) if name == module));
        if !present {
            imports.push(PyStmt::Import(module.clone()));
        }
    }
    py.body.splice(0..0, imports);
}

/// Give each documented top-level function its `##` comment as a docstring,
/// and each documented record its class's. Runs for every lowering, typed or
/// not (`DESIGN.md` §9).
pub(super) fn docstrings(module: &Module, mut py: PyModule) -> PyModule {
    let mut functions: HashMap<String, &str> = HashMap::new();
    let mut classes: HashMap<String, &str> = HashMap::new();
    for item in &module.items {
        match item {
            Item::Let(binding) if !binding.params.is_empty() => {
                if let (Some(name), Some(doc)) = (binding.name(), binding.doc.as_deref()) {
                    functions.insert(py_value_name(name), doc);
                }
            }
            Item::Type(decl) if matches!(decl.kind, TypeDeclKind::Record(_)) => {
                if let Some(doc) = decl.doc.as_deref() {
                    classes.insert(py_record_class(&decl.name), doc);
                }
            }
            _ => {}
        }
    }
    if functions.is_empty() && classes.is_empty() {
        return py;
    }
    for stmt in &mut py.body {
        match stmt {
            PyStmt::FuncDef { name, body, .. } | PyStmt::TypedFuncDef { name, body, .. } => {
                if let Some(doc) = functions.remove(name.as_str()) {
                    body.insert(0, PyStmt::Doc(doc.to_string()));
                }
            }
            PyStmt::ClassDef { name, doc, .. } => {
                if let Some(text) = classes.remove(name.as_str()) {
                    *doc = Some(text.to_string());
                }
            }
            _ => {}
        }
    }
    py
}
