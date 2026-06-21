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
    /// `extern [pure] name : type [= python.path]` — import a Python function or
    /// value with a declared Pyfun type (`DESIGN.md` §6). The boundary is
    /// effectful-by-default (`io` on the innermost arrow) unless `pure` is asserted.
    Extern(ExternDecl),
    Let(LetBinding),
    Expr(Expr),
}

/// `extern [pure] name : type [= a.b.c]`.
///
/// `target` is the dotted Python path to reference (e.g. `["math", "sqrt"]`); when
/// the `= …` clause is omitted it defaults to `[name]` (Pyfun name = Python name,
/// the existing prelude convention). Type variables in `ty` are bare lowercase
/// identifiers (as in `type` declarations); they are collected and generalized.
#[derive(Debug, Clone, PartialEq)]
pub struct ExternDecl {
    /// `extern pure …` — assert the boundary introduces no effect (no `io`).
    pub pure: bool,
    pub name: String,
    pub ty: TypeExpr,
    pub target: Vec<String>,
    pub span: NodeSpan,
}

/// A surface unit expression, e.g. `<m/s^2>`, stored as `(measure, exponent)`
/// factors (denominator factors carry negative exponents; dimensionless is empty).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitExpr {
    pub factors: Vec<(String, i32)>,
}

/// `type Name params... = ...` — either a sum of constructors or a record.
///
/// A named, possibly parameterized type. Type names and constructor names are
/// capitalized; type parameters and record field names are lowercase
/// (`DESIGN.md` §7 convention).
#[derive(Debug, Clone, PartialEq)]
pub struct TypeDecl {
    pub name: String,
    pub params: Vec<String>,
    pub kind: TypeDeclKind,
    pub span: NodeSpan,
}

/// The right-hand side of a `type` declaration.
#[derive(Debug, Clone, PartialEq)]
pub enum TypeDeclKind {
    /// `V1 fields | V2 fields | ...` — an algebraic data type (sum of constructors).
    Sum(Vec<VariantDecl>),
    /// `{ x: t, y: t }` — a record (named-field product type).
    Record(Vec<FieldDecl>),
}

/// One constructor of a sum [`TypeDecl`], e.g. `Cons a (List a)`.
#[derive(Debug, Clone, PartialEq)]
pub struct VariantDecl {
    pub name: String,
    pub fields: Vec<TypeExpr>,
}

/// One field of a record [`TypeDecl`], e.g. `x: int`.
#[derive(Debug, Clone, PartialEq)]
pub struct FieldDecl {
    pub name: String,
    pub ty: TypeExpr,
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
/// is a value binding. `mutable` records the `let mut` opt-in (§3); a non-`mut`
/// binding cannot be the target of `<-` (checked in the type phase). A binding
/// appears both as a top-level [`Item`] and as a local [`BlockStmt`].
#[derive(Debug, Clone, PartialEq)]
pub struct LetBinding {
    pub mutable: bool,
    /// `let pure …` — an opt-in assertion that the binding introduces no effect
    /// (no `io`). Checked in the type phase; erased at lowering.
    pub pure: bool,
    pub name: String,
    pub params: Vec<String>,
    pub value: Expr,
}

/// One statement inside a block (an indented `let … =` body). The final statement
/// is the block's value and must be an expression.
#[derive(Debug, Clone, PartialEq)]
pub enum BlockStmt {
    /// A local binding: `let [mut] name params... = value`.
    Let(LetBinding),
    /// An expression evaluated for its effect (non-final) or value (final).
    Expr(Expr),
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

    /// Binary operator application (arithmetic, comparison, equality, logical).
    Binary {
        op: BinOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },

    /// Unary operator application (`not e`).
    Unary {
        op: UnOp,
        expr: Box<Expr>,
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

    /// A list literal: `[1, 2, 3]` (comma-separated, possibly empty). Lowers to a
    /// Python `list`; its element type is inferred by unifying the elements.
    List {
        elems: Vec<Expr>,
    },

    /// A record literal: `{ x = 1, y = 2 }`. The (nominal) record type is
    /// resolved from the set of field names.
    Record {
        fields: Vec<FieldInit>,
    },

    /// A functional record update: `{ base with x = 3 }` — a copy of `base` with
    /// the listed fields replaced.
    RecordUpdate {
        base: Box<Expr>,
        fields: Vec<FieldInit>,
    },

    /// Record field access: `base.name`.
    Field {
        base: Box<Expr>,
        name: String,
    },

    /// An indentation block: a sequence of statements whose final statement is
    /// the value. Introduced by an indented `let … =` body. A block with a single
    /// expression statement is unwrapped to that expression at parse time, so a
    /// `Block` always has either multiple statements or a leading binding.
    Block {
        stmts: Vec<BlockStmt>,
    },

    /// Reassignment of a `mut` binding: `target <- value` (type `unit`).
    Assign {
        target: String,
        value: Box<Expr>,
    },
}

/// One `name = value` initializer in a record literal or update.
#[derive(Debug, Clone, PartialEq)]
pub struct FieldInit {
    pub name: String,
    pub value: Expr,
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
    /// `==` / `!=` — equality, result `bool` (operands of the same type).
    Eq,
    Ne,
    /// `< > <= >=` — ordering comparison, result `bool` (orderable operands).
    Lt,
    Gt,
    Le,
    Ge,
    /// `and` / `or` — logical and/or, result `bool` (short-circuiting in Python).
    And,
    Or,
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
            BinOp::Eq => "==",
            BinOp::Ne => "!=",
            BinOp::Lt => "<",
            BinOp::Gt => ">",
            BinOp::Le => "<=",
            BinOp::Ge => ">=",
            BinOp::And => "and",
            BinOp::Or => "or",
        }
    }
}

/// Unary operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    /// `not e` — logical negation.
    Not,
}

impl UnOp {
    /// The source spelling, used by the pretty-printer.
    pub fn symbol(self) -> &'static str {
        match self {
            UnOp::Not => "not",
        }
    }
}
