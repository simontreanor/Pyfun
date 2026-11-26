//! Property tests (`pyfun test`, `DESIGN.md` §9): a test that reads its
//! parameters is run by Hypothesis, with a strategy for each parameter derived
//! from the type the checker inferred for it.
//!
//! A strategy is built as a Python expression, the one a person would write:
//! `st.lists(st.integers())` for a `List int`, `st.builds(Point, x=…, y=…)` for
//! a record, `st.one_of(…)` over the constructors of a sum type. A type that
//! holds itself is generated with `st.recursive`, whose base is the type with
//! every way back into it taken away (`st.just(Leaf())`), so that a value is
//! always finite. A newtype is its underlying value, as it is at run time, and
//! a unit of measure is gone. A type variable left open is generated as `int`.
//! What cannot be generated (a function, an extern type, an `Async`, a caught
//! `Exception`, a type with no finite value) is an error at the parameter.

use std::collections::{BTreeSet, HashMap, HashSet};

use crate::lexer::Span;
use crate::parser::ast::{Item, LetBinding, Module, TypeDecl, TypeDeclKind, TypeExpr};
use crate::python_emitter::{PyExpr, PyModule, PyStmt};
use crate::types::Ty;

use super::typed::Decls;
use super::{LowerError, py_ctor_name, py_field_name, py_record_class, py_value_name};

/// How a test is run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestKind {
    /// Run once, as written: a test that reads none of its parameters, and
    /// so takes the one parameter it ignores (`let test_rect _ = …`).
    Example,
    /// Run by Hypothesis over generated values: a test that reads a
    /// parameter (`let test_reverse xs = …`). Every parameter is generated.
    Property,
}

/// Each top-level test function of a test module, by its name, with how it is
/// run. A test that reads none of its parameters and takes more than one is
/// the error, at its name: there is nothing to pass them.
pub fn test_kinds(module: &Module) -> Result<Vec<(String, TestKind)>, (Span, String)> {
    let read: HashSet<Span> = crate::lsp::resolve::references(module)
        .into_iter()
        .filter_map(|reference| match reference.target {
            crate::lsp::resolve::Target::Local(span) => Some(span),
            crate::lsp::resolve::Target::Module(_) => None,
        })
        .collect();
    let mut kinds = Vec::new();
    for item in &module.items {
        let Item::Let(binding) = item else { continue };
        let Some(name) = binding.name() else { continue };
        if !crate::types::is_test_name(name) {
            continue;
        }
        let reads = binding.params.iter().any(|param| {
            param
                .pattern
                .bound_vars()
                .iter()
                .any(|(_, span)| read.contains(span))
        });
        let kind = if reads {
            TestKind::Property
        } else if binding.params.len() == 1 {
            TestKind::Example
        } else if binding.params.is_empty() {
            return Err((
                binding.target_span.span(),
                format!(
                    "`{name}` is named as a test, and a test is a function, which pytest calls: \
                     write `let {name} _ = …`"
                ),
            ));
        } else {
            return Err((
                binding.target_span.span(),
                format!(
                    "`{name}` reads none of its parameters: write `let {name} _ = …` for a test \
                     run once, or read the parameters for a property run on generated values"
                ),
            ));
        };
        kinds.push((name.to_string(), kind));
    }
    Ok(kinds)
}

/// What property tests need to know of a test module beyond its syntax.
pub struct PropertyTypes<'a> {
    /// The type of each binding, by the span of its target (and of each
    /// parameter's name), as the checker recorded it.
    pub binding_types: &'a HashMap<Span, Ty>,
    /// The same types as the checker displays them, for a message.
    pub shown: &'a HashMap<Span, String>,
    /// The variables an `in` searches, each with what it is searched for.
    pub searched: &'a HashMap<u32, Vec<Ty>>,
    /// The modules whose types this one can name, each with its Python name.
    pub imported: &'a [(String, &'a Module)],
}

/// The names Hypothesis is reached by in an emitted test file.
struct Names {
    given: String,
    st: String,
}

/// Put `@given(…)` above each property test of `py`, the lowering of the test
/// module `module`, and import what the strategies name. `module_name` is the
/// module's name, for an error's place. Returns whether any test is a
/// property, and the names the strategies take from the shared runtime
/// (`Some`, `None_`, …).
pub(super) fn add_given(
    module: &Module,
    module_name: &str,
    py: &mut PyModule,
    kinds: &[(String, TestKind)],
    types: &PropertyTypes,
) -> Result<(bool, BTreeSet<&'static str>), LowerError> {
    let decls = Decls::of(module, py, types.imported);
    let names = Names {
        given: free_name("given", &decls.taken),
        st: free_name("st", &decls.taken),
    };
    let mut deriver = Deriver {
        decls: &decls,
        searched: types.searched,
        st: names.st.clone(),
        runtime: BTreeSet::new(),
        modules: BTreeSet::new(),
        frames: Vec::new(),
    };
    let properties: HashMap<String, &LetBinding> = module
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Let(binding) => binding.name().and_then(|name| {
                kinds
                    .iter()
                    .any(|(test, kind)| test == name && *kind == TestKind::Property)
                    .then(|| (py_value_name(name), binding))
            }),
            _ => None,
        })
        .collect();
    let mut any = false;
    for stmt in &mut py.body {
        let PyStmt::FuncDef { name, params, .. } = stmt else {
            continue;
        };
        let Some(binding) = properties.get(name.as_str()) else {
            continue;
        };
        if params.len() != binding.params.len() {
            continue;
        }
        let Some(mut rest) = types.binding_types.get(&binding.target_span.span()) else {
            continue;
        };
        let mut kwargs = Vec::new();
        for (param, source) in params.iter().zip(&binding.params) {
            let Ty::Fun(arg, result, _) = rest else { break };
            rest = result;
            let strategy = deriver.ty(arg).map_err(|why| {
                let span = source.span.span();
                let whole = show(arg) == why.ty;
                let shown = types.shown.get(&span).cloned().unwrap_or_else(|| show(arg));
                LowerError {
                    message: why.message(&param_text(source), &shown, whole),
                    at: Some((module_name.to_string(), span)),
                }
            })?;
            let strategy = strategy.ok_or_else(|| LowerError {
                message: "internal error: a parameter's strategy is empty".to_string(),
                at: None,
            })?;
            kwargs.push((param.clone(), strategy));
        }
        if kwargs.len() != params.len() {
            continue;
        }
        let def = std::mem::replace(stmt, PyStmt::Continue);
        *stmt = PyStmt::Decorated {
            decorator: PyExpr::CallKw {
                func: Box::new(PyExpr::Name(names.given.clone())),
                args: Vec::new(),
                kwargs,
            },
            def: Box::new(def),
        };
        any = true;
    }
    if any {
        let mut imports = vec![PyStmt::ImportFrom {
            module: "hypothesis".to_string(),
            names: vec![alias("given", &names.given), alias("strategies", &names.st)],
        }];
        for module in &deriver.modules {
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
    Ok((any, deriver.runtime))
}

/// `name`, or `_pf_name` where the module already binds `name`.
fn free_name(name: &str, taken: &HashSet<String>) -> String {
    if taken.contains(name) {
        format!("_pf_{name}")
    } else {
        name.to_string()
    }
}

/// `name` imported as `local`: `strategies as st`.
fn alias(name: &str, local: &str) -> String {
    if name == local {
        name.to_string()
    } else {
        format!("{name} as {local}")
    }
}

/// A parameter as a message names it: its name, or the names it binds.
fn param_text(param: &crate::parser::ast::Param) -> String {
    match param.name() {
        Some(name) => format!("`{name}`"),
        None => {
            let names: Vec<String> = param
                .pattern
                .bound_names()
                .iter()
                .map(|n| format!("`{n}`"))
                .collect();
            if names.is_empty() {
                "a parameter".to_string()
            } else {
                format!("the parameter that binds {}", names.join(" and "))
            }
        }
    }
}

/// Why a type cannot be generated: the part of it that cannot, as a short
/// account and the fix.
struct Why {
    /// The type that cannot be generated, as a message names it.
    ty: String,
    kind: WhyKind,
}

enum WhyKind {
    /// A function.
    Function,
    /// A type whose values come only from Python or from running something:
    /// an extern type, `Async`, `Exception`.
    Opaque(&'static str),
    /// A type every value of which holds another value of it.
    Endless,
    /// A type that holds itself at another type (`Nest a = Nest (Nest (List a))`).
    Irregular,
}

impl Why {
    /// The message for `param`, whose whole type the checker shows as
    /// `shown`; `whole` says that what cannot be generated is that whole
    /// type, and not a part of it.
    fn message(&self, param: &str, shown: &str, whole: bool) -> String {
        let (ty, within) = if whole {
            (shown, String::new())
        } else {
            // The parameter's whole type, where the part that cannot be
            // generated is inside it.
            (self.ty.as_str(), format!("it is `{shown}`, and "))
        };
        let cannot = format!("a property cannot generate {param}: {within}");
        match &self.kind {
            WhyKind::Function => format!(
                "{cannot}`{ty}` is a function. Take what the function is applied to as the \
                 parameter instead, or try each function you mean in the body (`[double, negate] \
                 |> List.iter (fun f -> …)`)"
            ),
            WhyKind::Opaque(what) => format!(
                "{cannot}`{ty}` {what}. Build the value in the body from parameters that can be \
                 generated"
            ),
            WhyKind::Endless => format!(
                "{cannot}every `{ty}` holds another `{ty}`, so there is no value to start from. \
                 Give `{ty}` a constructor that holds no `{ty}`"
            ),
            WhyKind::Irregular => format!(
                "{cannot}`{ty}` holds itself at another type, so its values have no strategy \
                 that ends. Take the parts of it as parameters and build it in the body"
            ),
        }
    }
}

/// A type being generated that holds itself: the name its strategy goes by
/// inside `st.recursive`, or none while its base (the values that do not hold
/// it) is being derived.
struct Frame {
    name: String,
    args: Vec<Ty>,
    var: Option<String>,
    used: bool,
}

/// One way to build a value of a sum type or record: the class, and its
/// fields with their names when it is built by name.
struct Shape {
    class: PyExpr,
    fields: Vec<(Option<String>, Ty)>,
}

struct Deriver<'a> {
    decls: &'a Decls<'a>,
    searched: &'a HashMap<u32, Vec<Ty>>,
    /// The name the strategies module is bound to (`st`).
    st: String,
    /// The shared runtime's classes the strategies name.
    runtime: BTreeSet<&'static str>,
    /// The modules a strategy reached a class through (`shapes`), each as the
    /// text after `import`.
    modules: BTreeSet<String>,
    frames: Vec<Frame>,
}

impl<'a> Deriver<'a> {
    /// `st.name(args…)`.
    fn st(&self, name: &str, args: Vec<PyExpr>) -> PyExpr {
        PyExpr::Call {
            func: Box::new(self.st_attr(name)),
            args,
        }
    }

    fn st_attr(&self, name: &str) -> PyExpr {
        PyExpr::Attribute {
            value: Box::new(PyExpr::Name(self.st.clone())),
            attr: name.to_string(),
        }
    }

    /// The strategy for `ty`, or `None` when there is no value of it to
    /// generate here: a way back into a type whose base is being derived.
    fn ty(&mut self, ty: &Ty) -> Result<Option<PyExpr>, Why> {
        Ok(Some(match ty {
            Ty::Int(_) | Ty::Num(..) => self.st("integers", vec![]),
            // A NaN is not equal to itself, so every `Test.equal` on one
            // would fail; Pyfun's own arithmetic raises before it makes one.
            Ty::Float(_) => PyExpr::CallKw {
                func: Box::new(self.st_attr("floats")),
                args: vec![],
                kwargs: vec![
                    ("allow_nan".to_string(), PyExpr::Bool(false)),
                    ("allow_infinity".to_string(), PyExpr::Bool(false)),
                ],
            },
            Ty::Bool => self.st("booleans", vec![]),
            Ty::Str => self.st("text", vec![]),
            Ty::Unit => self.st("none", vec![]),
            // What an `in` searches is a list of what it searches for; any
            // other variable is an `int`, as an unresolved number is.
            Ty::Var(v) => match self.searched.get(v).and_then(|sought| sought.first()) {
                Some(elem) => {
                    let list = Ty::Con("List".to_string(), vec![elem.clone()]);
                    return self.ty(&list);
                }
                None => self.st("integers", vec![]),
            },
            Ty::Tuple(elems) => {
                let mut parts = Vec::new();
                for elem in elems {
                    match self.ty(elem)? {
                        Some(part) => parts.push(part),
                        None => return Ok(None),
                    }
                }
                self.st("tuples", parts)
            }
            Ty::Fun(..) => {
                return Err(Why {
                    ty: show(ty),
                    kind: WhyKind::Function,
                });
            }
            Ty::Con(name, args) => return self.con(name, args),
        }))
    }

    /// The strategy for an element of a collection, which may have none.
    fn elem(&mut self, ty: &Ty) -> Result<PyExpr, Why> {
        Ok(match self.ty(ty)? {
            Some(strategy) => strategy,
            None => self.st("nothing", vec![]),
        })
    }

    fn con(&mut self, name: &str, args: &[Ty]) -> Result<Option<PyExpr>, Why> {
        let opaque = |what: &'static str| {
            Err(Why {
                ty: name.to_string(),
                kind: WhyKind::Opaque(what),
            })
        };
        let strategy = match (name, args) {
            ("List", [a]) => {
                let a = self.elem(a)?;
                self.st("lists", vec![a])
            }
            ("Set", [a]) => {
                let a = self.elem(a)?;
                self.st("sets", vec![a])
            }
            ("Map", [k, v]) => {
                let k = self.elem(k)?;
                let v = self.elem(v)?;
                self.st("dictionaries", vec![k, v])
            }
            // A `Seq` is an iterator, which `st.iterables` gives.
            ("Seq", [a]) => {
                let a = self.elem(a)?;
                self.st("iterables", vec![a])
            }
            ("Option", [a]) => {
                let shapes = vec![
                    Shape {
                        class: self.runtime_class("None_"),
                        fields: vec![],
                    },
                    Shape {
                        class: self.runtime_class("Some"),
                        fields: vec![(None, a.clone())],
                    },
                ];
                return self.one_of(shapes);
            }
            ("Result", [a, e]) => {
                let shapes = vec![
                    Shape {
                        class: self.runtime_class("Ok"),
                        fields: vec![(None, a.clone())],
                    },
                    Shape {
                        class: self.runtime_class("Error"),
                        fields: vec![(None, e.clone())],
                    },
                ];
                return self.one_of(shapes);
            }
            ("Json", []) => return self.recursive(name, args, Self::json_shapes),
            ("Async", [_]) => return opaque("is work to run rather than a value"),
            ("Exception", []) => return opaque("is made only by an error that was raised"),
            _ => {
                let Some((decl, prefix)) = self.decl(name) else {
                    return opaque("has no constructor a strategy can call");
                };
                if decl.params.len() != args.len() {
                    return opaque("has no constructor a strategy can call");
                }
                let env: HashMap<&str, Ty> = decl
                    .params
                    .iter()
                    .map(String::as_str)
                    .zip(args.iter().cloned())
                    .collect();
                match &decl.kind {
                    // A newtype is its underlying value at run time.
                    TypeDeclKind::Newtype(underlying) => {
                        return self.ty(&of_expr(underlying, &env));
                    }
                    TypeDeclKind::Opaque => {
                        return opaque("is an extern type, whose values come from Python");
                    }
                    TypeDeclKind::Record(_) | TypeDeclKind::Sum(_) => {
                        return self.recursive(name, args, |_| Ok(shapes_of(decl, &prefix, &env)));
                    }
                }
            }
        };
        Ok(Some(strategy))
    }

    /// The declaration of the type `name`, the module's own or an imported
    /// one, with what its classes are reached through (`shapes.`).
    fn decl(&mut self, name: &str) -> Option<(&'a TypeDecl, String)> {
        let decls: &'a Decls<'a> = self.decls;
        if let Some(decl) = decls.types.get(name) {
            return Some((decl, String::new()));
        }
        let (module, decl) = decls.imported.get(name)?;
        let (by, import) = decls.reached.get(module)?;
        self.modules.insert(import.clone());
        Some((decl, format!("{by}.")))
    }

    fn runtime_class(&mut self, name: &'static str) -> PyExpr {
        self.runtime.insert(name);
        PyExpr::Name(name.to_string())
    }

    /// The built-in `Json` type's constructors, which the shared runtime
    /// defines.
    fn json_shapes(&mut self) -> Result<Vec<Shape>, Why> {
        let json = Ty::Con("Json".to_string(), vec![]);
        let fields = [
            ("JNull", None),
            ("JBool", Some(Ty::Bool)),
            ("JInt", Some(Ty::Int(Default::default()))),
            ("JFloat", Some(Ty::Float(Default::default()))),
            ("JStr", Some(Ty::Str)),
            (
                "JList",
                Some(Ty::Con("List".to_string(), vec![json.clone()])),
            ),
            (
                "JObject",
                Some(Ty::Con("Map".to_string(), vec![Ty::Str, json.clone()])),
            ),
        ];
        Ok(fields
            .into_iter()
            .map(|(class, field)| Shape {
                class: self.runtime_class(class),
                fields: field.into_iter().map(|ty| (None, ty)).collect(),
            })
            .collect())
    }

    /// The strategy for the type `name` applied to `args`, whose ways to be
    /// built `shapes` gives. A type that holds itself is `st.recursive` over
    /// its values that do not.
    fn recursive(
        &mut self,
        name: &str,
        args: &[Ty],
        shapes: impl Fn(&mut Self) -> Result<Vec<Shape>, Why>,
    ) -> Result<Option<PyExpr>, Why> {
        if let Some(index) = self.frames.iter().rposition(|f| f.name == name) {
            let frame = &mut self.frames[index];
            if frame.args != args {
                return Err(Why {
                    ty: name.to_string(),
                    kind: WhyKind::Irregular,
                });
            }
            frame.used = true;
            return Ok(frame.var.clone().map(PyExpr::Name));
        }
        let var = self.lambda_name(name);
        self.frames.push(Frame {
            name: name.to_string(),
            args: args.to_vec(),
            var: Some(var.clone()),
            used: false,
        });
        let built = shapes(self).and_then(|shapes| self.one_of(shapes));
        let frame = self.frames.pop().expect("the frame pushed above");
        let extended = built?;
        if !frame.used {
            return Ok(extended);
        }
        self.frames.push(Frame {
            name: name.to_string(),
            args: args.to_vec(),
            var: None,
            used: false,
        });
        let base = shapes(self).and_then(|shapes| self.one_of(shapes));
        self.frames.pop();
        let (Some(base), Some(extended)) = (base?, extended) else {
            return Err(Why {
                ty: name.to_string(),
                kind: WhyKind::Endless,
            });
        };
        let extend = PyExpr::Lambda {
            params: vec![var],
            body: Box::new(extended),
        };
        Ok(Some(self.st("recursive", vec![base, extend])))
    }

    /// The name a recursive strategy goes by inside its `st.recursive`: the
    /// type's name in snake case (`tree` for `Tree`), clear of every name
    /// the module binds and of the ones around it.
    fn lambda_name(&self, ty: &str) -> String {
        let mut snake = String::new();
        for (i, c) in ty.chars().enumerate() {
            if c.is_uppercase() {
                if i > 0 {
                    snake.push('_');
                }
                snake.extend(c.to_lowercase());
            } else {
                snake.push(c);
            }
        }
        let mut name = py_value_name(&snake);
        let clashes = |name: &str| {
            self.decls.taken.contains(name)
                || name == self.st
                || self.decls.reached.values().any(|(by, _)| by == name)
                || self.frames.iter().any(|f| f.var.as_deref() == Some(name))
        };
        while clashes(&name) {
            name.push('_');
        }
        name
    }

    /// One strategy over the ways a value can be built, the first ones first
    /// (Hypothesis shrinks towards them). A way that needs a value there is
    /// none of here is left out; `None` when that leaves none.
    fn one_of(&mut self, shapes: Vec<Shape>) -> Result<Option<PyExpr>, Why> {
        let mut options = Vec::new();
        'shapes: for shape in shapes {
            if shape.fields.is_empty() {
                let value = PyExpr::Call {
                    func: Box::new(shape.class),
                    args: vec![],
                };
                options.push(self.st("just", vec![value]));
                continue;
            }
            let mut args = vec![shape.class];
            let mut kwargs = Vec::new();
            for (field, ty) in &shape.fields {
                let Some(strategy) = self.ty(ty)? else {
                    continue 'shapes;
                };
                match field {
                    Some(field) => kwargs.push((field.clone(), strategy)),
                    None => args.push(strategy),
                }
            }
            options.push(if kwargs.is_empty() {
                self.st("builds", args)
            } else {
                PyExpr::CallKw {
                    func: Box::new(self.st_attr("builds")),
                    args,
                    kwargs,
                }
            });
        }
        Ok(match options.len() {
            0 => None,
            1 => options.pop(),
            _ => Some(self.st("one_of", options)),
        })
    }
}

/// The ways to build a value of the record or sum type `decl`, its type
/// parameters standing for `env`, its classes reached through `prefix`.
fn shapes_of(decl: &TypeDecl, prefix: &str, env: &HashMap<&str, Ty>) -> Vec<Shape> {
    let class = |name: String| PyExpr::Name(format!("{prefix}{name}"));
    match &decl.kind {
        TypeDeclKind::Record(fields) => vec![Shape {
            class: class(py_record_class(&decl.name)),
            fields: fields
                .iter()
                .map(|f| (Some(py_field_name(&f.name)), of_expr(&f.ty, env)))
                .collect(),
        }],
        TypeDeclKind::Sum(variants) => variants
            .iter()
            .map(|v| Shape {
                class: class(py_ctor_name(&v.name)),
                fields: v
                    .fields
                    .iter()
                    .enumerate()
                    .map(|(i, te)| {
                        let name = v
                            .field_names
                            .as_ref()
                            .and_then(|names| names.get(i))
                            .map(|n| py_field_name(n));
                        (name, of_expr(te, env))
                    })
                    .collect(),
            })
            .collect(),
        TypeDeclKind::Newtype(_) | TypeDeclKind::Opaque => vec![],
    }
}

/// A declared type as a checker type, its parameters standing for `env`. A
/// unit of measure is dropped, as it is at run time; a qualified name is the
/// type's own (type names are one namespace across a project).
fn of_expr(te: &TypeExpr, env: &HashMap<&str, Ty>) -> Ty {
    match te {
        TypeExpr::Con(name, _, args) => {
            if args.is_empty()
                && let Some(ty) = env.get(name.as_str())
            {
                return ty.clone();
            }
            let base = name.split('<').next().unwrap_or(name);
            match base {
                "int" => Ty::Int(Default::default()),
                "float" => Ty::Float(Default::default()),
                "bool" => Ty::Bool,
                "str" => Ty::Str,
                "unit" => Ty::Unit,
                _ => {
                    let short = base.rsplit('.').next().unwrap_or(base);
                    Ty::Con(
                        short.to_string(),
                        args.iter().map(|a| of_expr(a, env)).collect(),
                    )
                }
            }
        }
        TypeExpr::Fun(a, b, _) => Ty::Fun(
            Box::new(of_expr(a, env)),
            Box::new(of_expr(b, env)),
            Default::default(),
        ),
        TypeExpr::Tuple(elems) => Ty::Tuple(elems.iter().map(|e| of_expr(e, env)).collect()),
    }
}

/// A type as a message names the part of a parameter that cannot be
/// generated: its constructor's name, or the arrow.
fn show(ty: &Ty) -> String {
    match ty {
        Ty::Fun(..) => {
            let mut parts = Vec::new();
            let mut rest = ty;
            while let Ty::Fun(arg, result, _) = rest {
                parts.push(match **arg {
                    Ty::Fun(..) => format!("({})", show(arg)),
                    _ => show(arg),
                });
                rest = result;
            }
            parts.push(show(rest));
            parts.join(" -> ")
        }
        Ty::Int(_) | Ty::Num(..) => "int".to_string(),
        Ty::Float(_) => "float".to_string(),
        Ty::Bool => "bool".to_string(),
        Ty::Str => "str".to_string(),
        Ty::Unit => "unit".to_string(),
        Ty::Var(_) => "a".to_string(),
        Ty::Tuple(elems) => format!(
            "({})",
            elems.iter().map(show).collect::<Vec<_>>().join(", ")
        ),
        Ty::Con(name, args) if args.is_empty() => name.clone(),
        Ty::Con(name, args) => {
            let args: Vec<String> = args
                .iter()
                .map(|a| match a {
                    Ty::Con(_, inner) if !inner.is_empty() => format!("({})", show(a)),
                    Ty::Fun(..) => format!("({})", show(a)),
                    _ => show(a),
                })
                .collect();
            format!("{name} {}", args.join(" "))
        }
    }
}
