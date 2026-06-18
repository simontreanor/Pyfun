//! Pyfun abstract syntax for the Phase 1 subset.
//!
//! Per `DESIGN.md` §9 the AST lives under `parser/`. Nodes are deliberately
//! span-free in Phase 1 so that `PartialEq` gives us structural equality for the
//! parse→print→parse roundtrip tests. Spans move onto the AST when diagnostics
//! land in Phase 3.

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
/// immutability *check* itself is Phase 3.
#[derive(Debug, Clone, PartialEq)]
pub struct LetBinding {
    pub mutable: bool,
    pub name: String,
    pub params: Vec<String>,
    pub value: Expr,
}

/// Expressions. Application is single-argument so currying is structural:
/// `f a b` parses to `App(App(f, a), b)` (see `DESIGN.md` §7).
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
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
