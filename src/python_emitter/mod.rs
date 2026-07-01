//! Python abstract syntax (a small IR) plus a readable source emitter.
//!
//! Per `DESIGN.md` §5, lowering targets this structured IR rather than splicing
//! strings, and the emitter turns it into human-readable Python. The IR covers
//! only what Phase 2 lowering produces; it grows as the language does.

/// A Python module: a flat list of top-level statements.
#[derive(Debug, Clone, PartialEq)]
pub struct PyModule {
    pub body: Vec<PyStmt>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PyStmt {
    /// `import <module>`
    Import(String),
    /// `from <module> import a, b` — used for the shared runtime
    /// (`from _pyfun_rt import Some, None_`) in multi-file projects.
    ImportFrom { module: String, names: Vec<String> },
    /// `nonlocal a, b` — declare captured names from an enclosing *function* scope
    /// that this function reassigns.
    Nonlocal(Vec<String>),
    /// `global a, b` — declare module-level names that this function reassigns.
    Global(Vec<String>),
    /// `target = value`
    Assign { target: String, value: PyExpr },
    /// `return value`
    Return(PyExpr),
    /// A bare expression evaluated for its (side) effect.
    Expr(PyExpr),
    /// `def name(params): body` (or `async def` when `is_async`).
    FuncDef {
        name: String,
        params: Vec<String>,
        body: Vec<PyStmt>,
        is_async: bool,
    },
    /// `yield value`
    Yield(PyExpr),
    /// `yield from value`
    YieldFrom(PyExpr),
    /// `if test: body [else: orelse]`
    If {
        test: PyExpr,
        body: Vec<PyStmt>,
        orelse: Vec<PyStmt>,
    },
    /// `match subject: cases`
    Match { subject: PyExpr, cases: Vec<PyCase> },
    /// `raise RuntimeError(message)` — used for non-exhaustive-match guards.
    RaiseRuntimeError(String),
    /// A data-constructor class with positional fields and `__match_args__`.
    ClassDef { name: String, fields: Vec<String> },
}

#[derive(Debug, Clone, PartialEq)]
pub struct PyCase {
    pub pattern: PyPattern,
    /// `case pat if guard:` — an optional guard expression (`DESIGN.md` §7.2).
    pub guard: Option<PyExpr>,
    pub body: Vec<PyStmt>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PyPattern {
    /// `case _`
    Wildcard,
    /// `case name`
    Capture(String),
    /// `case <literal>`
    Literal(PyExpr),
    /// `case Name(arg, ...)` — a class pattern with positional sub-patterns.
    Class { name: String, args: Vec<PyPattern> },
    /// `case Name(field=pat, ...)` — a class pattern with keyword sub-patterns,
    /// used for record patterns (which name a subset of fields, in any order).
    ClassKw {
        name: String,
        fields: Vec<(String, PyPattern)>,
    },
    /// `case (a, b)` — a sequence pattern, used for tuple patterns.
    Sequence(Vec<PyPattern>),
    /// `case a | b | c` — an or-pattern.
    Or(Vec<PyPattern>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum PyExpr {
    Int(i64),
    Float(f64),
    Str(String),
    Bool(bool),
    Name(String),
    /// `left <op> right` — arithmetic only in Phase 2.
    BinOp {
        op: PyBinOp,
        left: Box<PyExpr>,
        right: Box<PyExpr>,
    },
    /// `func(args...)`
    Call {
        func: Box<PyExpr>,
        args: Vec<PyExpr>,
    },
    /// `body if test else orelse`
    IfExp {
        body: Box<PyExpr>,
        test: Box<PyExpr>,
        orelse: Box<PyExpr>,
    },
    /// `lambda params: body`
    Lambda {
        params: Vec<String>,
        body: Box<PyExpr>,
    },
    /// `value.attr`
    Attribute {
        value: Box<PyExpr>,
        attr: String,
    },
    /// `await value`
    Await(Box<PyExpr>),
    /// `not value`
    Not(Box<PyExpr>),
    /// A list display `[a, b, c]`.
    List(Vec<PyExpr>),
    /// A tuple display `(a, b, c)` (always two or more elements in Pyfun).
    Tuple(Vec<PyExpr>),
    /// The `None` literal — the unit value (e.g. the result of an assignment).
    NoneLit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PyBinOp {
    Add,
    Sub,
    Mul,
    Div,
    FloorDiv,
    Eq,
    Ne,
    Lt,
    Gt,
    Le,
    Ge,
    And,
    Or,
    /// `x in container` — membership, used by the collection prelude
    /// (`set_contains`/`map_contains`). Comparison-precedence, like in Python.
    In,
}

impl PyBinOp {
    fn symbol(self) -> &'static str {
        match self {
            PyBinOp::Add => "+",
            PyBinOp::Sub => "-",
            PyBinOp::Mul => "*",
            // Pyfun mirrors Python: `/` is true division, `//` floors.
            PyBinOp::Div => "/",
            PyBinOp::FloorDiv => "//",
            PyBinOp::Eq => "==",
            PyBinOp::Ne => "!=",
            PyBinOp::Lt => "<",
            PyBinOp::Gt => ">",
            PyBinOp::Le => "<=",
            PyBinOp::Ge => ">=",
            // Pyfun `&&`/`||` lower to Python's keyword operators.
            PyBinOp::And => "and",
            PyBinOp::Or => "or",
            PyBinOp::In => "in",
        }
    }

    /// Binding power, higher = tighter. Mirrors Python so emitted code needs
    /// minimal parentheses: `or` < `and` < `not` (4) < comparison < `+ -` < `* /`.
    fn precedence(self) -> u8 {
        match self {
            PyBinOp::Or => 2,
            PyBinOp::And => 3,
            PyBinOp::Eq
            | PyBinOp::Ne
            | PyBinOp::Lt
            | PyBinOp::Gt
            | PyBinOp::Le
            | PyBinOp::Ge
            | PyBinOp::In => 5,
            PyBinOp::Add | PyBinOp::Sub => 10,
            PyBinOp::Mul | PyBinOp::Div | PyBinOp::FloorDiv => 20,
        }
    }
}

/// Render a module to Python source text.
pub fn emit(module: &PyModule) -> String {
    let mut out = String::new();
    emit_block(&module.body, 0, &mut out);
    out
}

const INDENT: &str = "    ";

fn emit_block(stmts: &[PyStmt], depth: usize, out: &mut String) {
    if stmts.is_empty() {
        // An empty suite still needs a body in Python.
        line(out, depth, "pass");
        return;
    }
    for stmt in stmts {
        emit_stmt(stmt, depth, out);
    }
}

fn emit_stmt(stmt: &PyStmt, depth: usize, out: &mut String) {
    match stmt {
        PyStmt::Import(module) => line(out, depth, &format!("import {module}")),
        PyStmt::ImportFrom { module, names } => {
            line(
                out,
                depth,
                &format!("from {module} import {}", names.join(", ")),
            );
        }
        PyStmt::Nonlocal(names) => line(out, depth, &format!("nonlocal {}", names.join(", "))),
        PyStmt::Global(names) => line(out, depth, &format!("global {}", names.join(", "))),
        PyStmt::Assign { target, value } => {
            line(out, depth, &format!("{target} = {}", expr(value)));
        }
        PyStmt::Return(value) => line(out, depth, &format!("return {}", expr(value))),
        PyStmt::Expr(value) => line(out, depth, &expr(value)),
        PyStmt::FuncDef {
            name,
            params,
            body,
            is_async,
        } => {
            let kw = if *is_async { "async def" } else { "def" };
            line(out, depth, &format!("{kw} {name}({}):", params.join(", ")));
            emit_block(body, depth + 1, out);
        }
        PyStmt::Yield(value) => line(out, depth, &format!("yield {}", expr(value))),
        PyStmt::YieldFrom(value) => line(out, depth, &format!("yield from {}", expr(value))),
        PyStmt::If { test, body, orelse } => {
            line(out, depth, &format!("if {}:", expr(test)));
            emit_block(body, depth + 1, out);
            if !orelse.is_empty() {
                line(out, depth, "else:");
                emit_block(orelse, depth + 1, out);
            }
        }
        PyStmt::Match { subject, cases } => {
            line(out, depth, &format!("match {}:", expr(subject)));
            for case in cases {
                let guard = match &case.guard {
                    Some(g) => format!(" if {}", expr(g)),
                    None => String::new(),
                };
                line(
                    out,
                    depth + 1,
                    &format!("case {}{guard}:", pattern(&case.pattern)),
                );
                emit_block(&case.body, depth + 2, out);
            }
        }
        PyStmt::RaiseRuntimeError(message) => {
            line(
                out,
                depth,
                &format!("raise RuntimeError({})", string_literal(message)),
            );
        }
        PyStmt::ClassDef { name, fields } => emit_class(name, fields, depth, out),
    }
}

fn emit_class(name: &str, fields: &[String], depth: usize, out: &mut String) {
    line(out, depth, &format!("class {name}:"));
    // `__match_args__` is a tuple of the positional field names; a single-element
    // tuple needs a trailing comma.
    let names: Vec<String> = fields.iter().map(|f| format!("'{f}'")).collect();
    let trailing = if fields.len() == 1 { "," } else { "" };
    line(
        out,
        depth + 1,
        &format!("__match_args__ = ({}{trailing})", names.join(", ")),
    );

    let params = std::iter::once("self".to_string())
        .chain(fields.iter().cloned())
        .collect::<Vec<_>>()
        .join(", ");
    line(out, depth + 1, &format!("def __init__({params}):"));
    if fields.is_empty() {
        line(out, depth + 2, "pass");
    } else {
        for f in fields {
            line(out, depth + 2, &format!("self.{f} = {f}"));
        }
    }

    // `__repr__` so values print readably (`Some(1)`, `Red`, `Ok("x")`) instead
    // of `<Some object at 0x…>`. Fields use `!r` so nested values and strings are
    // shown quoted.
    line(out, depth + 1, "def __repr__(self):");
    if fields.is_empty() {
        line(out, depth + 2, &format!("return {}", string_literal(name)));
    } else {
        let parts = fields
            .iter()
            .map(|f| format!("{{self.{f}!r}}"))
            .collect::<Vec<_>>()
            .join(", ");
        line(out, depth + 2, &format!("return f\"{name}({parts})\""));
    }

    // Structural `__eq__` so `==` compares by constructor + fields (recursively),
    // not object identity — matching FP expectations and Pyfun's equality typing.
    line(out, depth + 1, "def __eq__(self, other):");
    if fields.is_empty() {
        line(out, depth + 2, "return type(self) is type(other)");
    } else {
        line(
            out,
            depth + 2,
            "return type(self) is type(other) and self.__dict__ == other.__dict__",
        );
    }

    // Structural `__hash__`, consistent with `__eq__` (equal values hash equally):
    // a tuple of the type and the field values. Defining `__eq__` otherwise makes a
    // class unhashable in Python, so without this an ADT/record could not be a `Set`
    // element or `Map` key. A field whose value is itself unhashable raises at hash
    // time — the same way Python rejects an unhashable key.
    line(out, depth + 1, "def __hash__(self):");
    if fields.is_empty() {
        line(out, depth + 2, "return hash(type(self))");
    } else {
        let parts = std::iter::once("type(self)".to_string())
            .chain(fields.iter().map(|f| format!("self.{f}")))
            .collect::<Vec<_>>()
            .join(", ");
        line(out, depth + 2, &format!("return hash(({parts}))"));
    }
}

fn pattern(pat: &PyPattern) -> String {
    match pat {
        PyPattern::Wildcard => "_".to_string(),
        PyPattern::Capture(name) => name.clone(),
        PyPattern::Literal(value) => expr(value),
        PyPattern::Class { name, args } => {
            let args: Vec<String> = args.iter().map(pattern).collect();
            format!("{name}({})", args.join(", "))
        }
        PyPattern::ClassKw { name, fields } => {
            let parts: Vec<String> = fields
                .iter()
                .map(|(f, p)| format!("{f}={}", pattern(p)))
                .collect();
            format!("{name}({})", parts.join(", "))
        }
        PyPattern::Sequence(elems) => {
            let elems: Vec<String> = elems.iter().map(pattern).collect();
            format!("({})", elems.join(", "))
        }
        PyPattern::Or(alts) => {
            let alts: Vec<String> = alts.iter().map(pattern).collect();
            alts.join(" | ")
        }
    }
}

fn line(out: &mut String, depth: usize, text: &str) {
    for _ in 0..depth {
        out.push_str(INDENT);
    }
    out.push_str(text);
    out.push('\n');
}

/// Render an expression at the top precedence level.
fn expr(e: &PyExpr) -> String {
    emit_expr(e, 0)
}

/// Precedence of an expression for deciding when to parenthesize.
fn prec(e: &PyExpr) -> u8 {
    match e {
        PyExpr::IfExp { .. } => 1,
        PyExpr::Lambda { .. } => 2,
        PyExpr::Await(_) => 3,
        // `not` sits between `and` (3) and comparison (5), as in Python.
        PyExpr::Not(_) => 4,
        PyExpr::BinOp { op, .. } => op.precedence(),
        // Atoms / calls / attributes never need wrapping.
        _ => 100,
    }
}

fn emit_expr(e: &PyExpr, parent_prec: u8) -> String {
    let text = match e {
        PyExpr::Int(n) => n.to_string(),
        PyExpr::Float(f) => format!("{f:?}"),
        PyExpr::Str(s) => string_literal(s),
        PyExpr::Bool(b) => if *b { "True" } else { "False" }.to_string(),
        PyExpr::Name(name) => name.clone(),
        PyExpr::BinOp { op, left, right } => {
            let p = op.precedence();
            // Left-associative: left child at same precedence is fine; right
            // child must bind strictly tighter to avoid reassociation.
            format!(
                "{} {} {}",
                emit_expr(left, p),
                op.symbol(),
                emit_expr(right, p + 1)
            )
        }
        PyExpr::Call { func, args } => {
            let args: Vec<String> = args.iter().map(expr).collect();
            format!("{}({})", emit_expr(func, 100), args.join(", "))
        }
        PyExpr::IfExp { body, test, orelse } => {
            format!(
                "{} if {} else {}",
                emit_expr(body, 2),
                emit_expr(test, 2),
                emit_expr(orelse, 1)
            )
        }
        PyExpr::Lambda { params, body } => {
            format!("lambda {}: {}", params.join(", "), emit_expr(body, 2))
        }
        PyExpr::Attribute { value, attr } => format!("{}.{attr}", emit_expr(value, 100)),
        PyExpr::Await(inner) => format!("await {}", emit_expr(inner, 100)),
        // Emit the operand at `not`'s own level so comparisons stay bare
        // (`not a == b`) while looser `and`/`or` get parenthesized.
        PyExpr::Not(inner) => format!("not {}", emit_expr(inner, 4)),
        PyExpr::List(elems) => {
            let elems: Vec<String> = elems.iter().map(expr).collect();
            format!("[{}]", elems.join(", "))
        }
        PyExpr::Tuple(elems) => {
            let elems: Vec<String> = elems.iter().map(expr).collect();
            format!("({})", elems.join(", "))
        }
        PyExpr::NoneLit => "None".to_string(),
    };
    if prec(e) < parent_prec {
        format!("({text})")
    } else {
        text
    }
}

fn string_literal(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}
