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

/// A top-level item: either a binding or a bare expression.
#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    Let(LetBinding),
    Expr(Expr),
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
    Div,
}

impl BinOp {
    /// The source spelling, used by the pretty-printer.
    pub fn symbol(self) -> &'static str {
        match self {
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Div => "/",
        }
    }
}
