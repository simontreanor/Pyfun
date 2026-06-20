//! Pyfun abstract syntax.
//!
//! Per `DESIGN.md` §9 the AST lives under `parser/`. Every [`Expr`] carries a
//! source [`NodeSpan`] so the type checker can point diagnostics at the offending
//! code (Phase 3). Spans compare *equal to each other unconditionally*
//! ([`NodeSpan`]'s `PartialEq`), so derived structural equality on the AST ignores
//! them — which is what keeps the parse→print→parse roundtrip tests meaningful.

use crate::lexer::Span;

/// A source span attached to an AST node. Two `NodeSpan`s are always considered
/// equal, so `#[derive(PartialEq)]` on the AST compares structure only.
#[derive(Debug, Clone, Copy)]
pub struct NodeSpan(pub Span);

impl NodeSpan {
    pub fn new(span: Span) -> Self {
        NodeSpan(span)
    }

    pub fn span(self) -> Span {
        self.0
    }
}

impl PartialEq for NodeSpan {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl Eq for NodeSpan {}

/// A whole compilation unit: a sequence of top-level items.
#[derive(Debug, Clone, PartialEq)]
pub struct Module {
    pub items: Vec<Item>,
}

/// A top-level item: a measure/type declaration, a binding, or a bare expression.
#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    /// `measure name` — declares a base unit of measure (`DESIGN.md` §8.2).
    Measure {
        name: String,
        span: NodeSpan,
    },
    Type(TypeDecl),
    Let(LetBinding),
    Expr(Expr),
}

/// A surface unit expression, e.g. `<m/s^2>`, stored as `(measure, exponent)`
/// factors (denominator factors carry negative exponents; dimensionless is empty).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitExpr {
    pub factors: Vec<(String, i32)>,
}

/// `type Name params... = V1 fields | V2 fields | ...`
///
/// An algebraic data type: a named, possibly parameterized sum of constructors.
/// Type names and constructor names are capitalized; type parameters are
/// lowercase (`DESIGN.md` §7 convention).
#[derive(Debug, Clone, PartialEq)]
pub struct TypeDecl {
    pub name: String,
    pub params: Vec<String>,
    pub variants: Vec<VariantDecl>,
    pub span: NodeSpan,
}

/// One constructor of a [`TypeDecl`], e.g. `Cons a (List a)`.
#[derive(Debug, Clone, PartialEq)]
pub struct VariantDecl {
    pub name: String,
    pub fields: Vec<TypeExpr>,
}

/// A type expression appearing in a constructor field.
#[derive(Debug, Clone, PartialEq)]
pub enum TypeExpr {
    /// A named type — variable, builtin, or type constructor — applied to zero or
    /// more arguments. `a`, `int`, `List a`, `Option int` are all `Con`.
    Con(String, Vec<TypeExpr>),
    /// A function type `arg -> result`.
    Fun(Box<TypeExpr>, Box<TypeExpr>),
}

/// `let [mut] name params... = value`.
///
/// Parameters make this a (curried) function definition; with no parameters it
/// is a value binding. `mutable` records the `let mut` opt-in (§3); the
/// immutability *check* itself is a later phase.
#[derive(Debug, Clone, PartialEq)]
pub struct LetBinding {
    pub mutable: bool,
    pub name: String,
    pub params: Vec<String>,
    pub value: Expr,
}

/// An expression node: its [`ExprKind`] plus the source span it came from.
#[derive(Debug, Clone, PartialEq)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: NodeSpan,
}

impl Expr {
    pub fn new(kind: ExprKind, span: Span) -> Self {
        Expr {
            kind,
            span: NodeSpan::new(span),
        }
    }

    /// The underlying source span.
    pub fn span(&self) -> Span {
        self.span.span()
    }
}

/// Expression shapes. Application is single-argument so currying is structural:
/// `f a b` parses to `App(App(f, a), b)` (see `DESIGN.md` §7).
#[derive(Debug, Clone, PartialEq)]
pub enum ExprKind {
    Int(i64),
    Float(f64),
    Str(String),
    Bool(bool),
    Var(String),

    /// `fun a b -> body`
    Fn {
        params: Vec<String>,
        body: Box<Expr>,
    },

    /// Curried application of a single argument.
    App {
        func: Box<Expr>,
        arg: Box<Expr>,
    },

    /// `if cond then a else b` — an expression, so the `else` branch is required.
    If {
        cond: Box<Expr>,
        then: Box<Expr>,
        else_: Box<Expr>,
    },

    /// `match scrutinee with | pat -> body ...`
    Match {
        scrutinee: Box<Expr>,
        arms: Vec<MatchArm>,
    },

    /// Arithmetic binary operator application.
    Binary {
        op: BinOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },

    /// `lhs |> rhs` — pipe (sugar for `rhs lhs`, kept explicit in the AST).
    Pipe {
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },

    /// A computation expression: `builder { items }` (`DESIGN.md` §8.1).
    Ce {
        builder: CeBuilder,
        items: Vec<CeItem>,
    },

    /// A unit-annotated numeric literal: `value<unit>` (`DESIGN.md` §8.2).
    Annot {
        value: Box<Expr>,
        unit: UnitExpr,
    },
}

/// The three built-in computation-expression builders.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CeBuilder {
    Async,
    Seq,
    Result,
}

impl CeBuilder {
    /// The builder keyword, if `name` is one (builder names are contextual, not
    /// reserved words).
    pub fn from_name(name: &str) -> Option<CeBuilder> {
        match name {
            "async" => Some(CeBuilder::Async),
            "seq" => Some(CeBuilder::Seq),
            "result" => Some(CeBuilder::Result),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            CeBuilder::Async => "async",
            CeBuilder::Seq => "seq",
            CeBuilder::Result => "result",
        }
    }
}

/// One item inside a computation-expression block.
#[derive(Debug, Clone, PartialEq)]
pub enum CeItem {
    /// `let! name = value` — monadic bind.
    LetBang { name: String, value: Expr },
    /// `let name = value` — ordinary binding.
    Let { name: String, value: Expr },
    /// `do! value` — monadic bind discarding the result.
    DoBang(Expr),
    /// `return value` — wrap a value into the monad.
    Return(Expr),
    /// `return! value` — yield an already-monadic value.
    ReturnBang(Expr),
    /// `yield value` — emit one element (seq).
    Yield(Expr),
    /// `yield! value` — splice a sub-sequence (seq).
    YieldBang(Expr),
}

/// One arm of a `match` expression.
#[derive(Debug, Clone, PartialEq)]
pub struct MatchArm {
    pub pattern: Pattern,
    pub body: Expr,
}

/// Patterns. Constructor names are capitalized identifiers; lowercase
/// identifiers bind variables (`DESIGN.md` §7 convention).
#[derive(Debug, Clone, PartialEq)]
pub enum Pattern {
    Wildcard,
    Var(String),
    Int(i64),
    Bool(bool),
    Ctor { name: String, args: Vec<Pattern> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    /// `/` — true division (Python `/`), result is `float`.
    Div,
    /// `//` — floor division (Python `//`), result is `int`.
    FloorDiv,
}

impl BinOp {
    /// The source spelling, used by the pretty-printer.
    pub fn symbol(self) -> &'static str {
        match self {
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Div => "/",
            BinOp::FloorDiv => "//",
        }
    }
}
