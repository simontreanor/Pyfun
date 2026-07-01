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
    /// `measure name` declares a base unit of measure; `measure name = <unit>`
    /// (e.g. `measure N = kg m / s^2`) declares a **derived alias** that expands to
    /// a compound of base measures (`DESIGN.md` §8.2). `definition` is `None` for a
    /// base measure, `Some(body)` for an alias.
    Measure {
        name: String,
        definition: Option<UnitExpr>,
        span: NodeSpan,
    },
    Type(TypeDecl),
    /// `extern [pure] name : type [= python.path]` — import a Python function or
    /// value with a declared Pyfun type (`DESIGN.md` §6). The boundary is
    /// effectful-by-default (`io` on the innermost arrow) unless `pure` is asserted.
    Extern(ExternDecl),
    Let(LetBinding),
    /// `module Name = <indented let bindings>` — an in-file namespace (`DESIGN.md`
    /// §6). Members are accessed `Name.member` outside and see each other
    /// unqualified inside. MVP: the body holds only `let` bindings.
    Module {
        name: String,
        name_span: NodeSpan,
        items: Vec<LetBinding>,
    },
    /// `import Name` — bring another source file's module into scope under its
    /// capitalized name, accessed `Name.member` (`DESIGN.md` §6.1). Top-of-file
    /// only. Slice 1 parses and pretty-prints it; the multi-file driver that
    /// resolves it lands in a later slice (semantics stubbed for now).
    Import {
        name: String,
        span: NodeSpan,
    },
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
    /// The span of the declared type name, so an editor can find-references /
    /// rename it. `NodeSpan` compares equal — invisible to roundtrip.
    pub name_span: NodeSpan,
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
    /// The span of the constructor name, so an editor can find-references / rename
    /// it. `NodeSpan` compares equal — invisible to roundtrip.
    pub name_span: NodeSpan,
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
    /// more arguments. `a`, `int`, `List a`, `Option int` are all `Con`. The
    /// `NodeSpan` is the span of the type *name* (for editor find-references /
    /// rename of a user type); it compares equal — invisible to roundtrip.
    Con(String, NodeSpan, Vec<TypeExpr>),
    /// A function type `arg -> result`.
    Fun(Box<TypeExpr>, Box<TypeExpr>),
    /// A tuple type `(a, b)` — a structural product of two or more types.
    Tuple(Vec<TypeExpr>),
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
    /// The span of the binding `name`, so an editor can hover the definition site
    /// to see its inferred type (`NodeSpan` compares equal, so this is invisible
    /// to structural/roundtrip equality).
    pub name_span: NodeSpan,
    pub params: Vec<Param>,
    pub value: Expr,
}

/// A function/value-binding parameter: its name and source span. The span lets an
/// editor jump to / hover the parameter (`NodeSpan` compares equal, so it is
/// invisible to structural/roundtrip equality).
#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    pub name: String,
    pub span: NodeSpan,
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
/// One segment of an interpolated string ([`ExprKind::Interp`]): a literal chunk
/// (escapes and `{{`/`}}` already resolved) or an embedded expression (a "hole").
#[derive(Debug, Clone, PartialEq)]
pub enum InterpPart {
    Lit(String),
    Expr(Box<Expr>),
}

/// `f a b` parses to `App(App(f, a), b)` (see `DESIGN.md` §7).
#[derive(Debug, Clone, PartialEq)]
pub enum ExprKind {
    Int(i64),
    Float(f64),
    Str(String),
    /// An interpolated string `f"...{expr}..."`: a sequence of literal chunks and
    /// embedded expressions. Evaluates to a `string`; each hole may be any type
    /// (stringified by the emitted Python f-string).
    Interp {
        parts: Vec<InterpPart>,
    },
    Bool(bool),
    /// The unit value `()` — the sole inhabitant of the `unit` type (lowers to
    /// Python `None`).
    Unit,
    Var(String),

    /// `fun a b -> body`
    Fn {
        params: Vec<Param>,
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

    /// A tuple literal: `(a, b, c)` — a structural (anonymous) product of two or
    /// more values. `()` is the unit value (not a 0-tuple) and `(x)` is grouping
    /// (not a 1-tuple), so a tuple always has at least two elements. Lowers ~1:1 to
    /// a Python tuple.
    Tuple {
        elems: Vec<Expr>,
    },

    /// A record literal: `Point { x = 1, y = 2 }` — constructor-tagged
    /// (`DESIGN.md` §8.3). `ty` names the record type; `ty_span` is the tag's
    /// span (for editor nav; `NodeSpan` compares equal — roundtrip-invisible).
    Record {
        ty: String,
        ty_span: NodeSpan,
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

/// A computation-expression builder: one of the three built-ins (each with a
/// bespoke native Python lowering) or a user-defined builder named by an in-file
/// `module` (desugared to that module's `bind`/`return_`/`yield_`/… functions).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CeBuilder {
    Async,
    Seq,
    Result,
    /// A user builder, named by an (uppercase) module.
    User(String),
}

impl CeBuilder {
    /// The built-in builder keyword, if `name` is one (builder names are
    /// contextual, not reserved words). User builders are resolved by the parser
    /// from an uppercase name, not here.
    pub fn from_name(name: &str) -> Option<CeBuilder> {
        match name {
            "async" => Some(CeBuilder::Async),
            "seq" => Some(CeBuilder::Seq),
            "result" => Some(CeBuilder::Result),
            _ => None,
        }
    }

    pub fn name(&self) -> &str {
        match self {
            CeBuilder::Async => "async",
            CeBuilder::Seq => "seq",
            CeBuilder::Result => "result",
            CeBuilder::User(name) => name,
        }
    }
}

/// One item inside a computation-expression block.
#[derive(Debug, Clone, PartialEq)]
pub enum CeItem {
    /// `let! name = value` — monadic bind. `name_span` is the binding name's span
    /// (for editor jump/hover; `NodeSpan` compares equal, so roundtrip-invisible).
    LetBang {
        name: String,
        name_span: NodeSpan,
        value: Expr,
    },
    /// `let name = value` — ordinary binding.
    Let {
        name: String,
        name_span: NodeSpan,
        value: Expr,
    },
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

/// One arm of a `match` expression: `case pattern [if guard]: body`
/// (`DESIGN.md` §7.2). A guarded arm does not count toward exhaustiveness (its
/// `guard` may be false), so the checker excludes it from coverage.
#[derive(Debug, Clone, PartialEq)]
pub struct MatchArm {
    pub pattern: Pattern,
    pub guard: Option<Expr>,
    pub body: Expr,
}

/// Patterns. Constructor names are capitalized identifiers; lowercase
/// identifiers bind variables (`DESIGN.md` §7 convention).
#[derive(Debug, Clone, PartialEq)]
pub enum Pattern {
    Wildcard,
    /// A variable binding pattern. Carries the binding's span so an editor can
    /// jump to / hover it (`NodeSpan` compares equal — invisible to roundtrip).
    Var {
        name: String,
        span: NodeSpan,
    },
    Int(i64),
    Bool(bool),
    Ctor {
        name: String,
        /// The span of the constructor name (qualified or bare), so an editor can
        /// find-references / rename it. `NodeSpan` compares equal — invisible to
        /// roundtrip.
        name_span: NodeSpan,
        args: Vec<Pattern>,
    },
    /// `Point { x = p, y }` — a constructor-tagged record pattern (`DESIGN.md`
    /// §8.3). `ty` names the record type (`ty_span` is the tag's span). May mention
    /// a subset of fields; an omitted field is left unmatched. `{ x }` shorthand
    /// binds field `x` to the variable `x` (a `Var` sub-pattern).
    Record {
        ty: String,
        ty_span: NodeSpan,
        fields: Vec<FieldPattern>,
    },
    /// `(a, b)` — a tuple pattern (two or more sub-patterns). Irrefutable iff every
    /// element is; lowers to a Python sequence pattern `case (a, b):`.
    Tuple {
        elems: Vec<Pattern>,
    },
    /// `a | b | c` — an or-pattern (`DESIGN.md` §7.2): matches if any alternative
    /// does. All alternatives must bind the same variables at the same types; lowers
    /// to a Python or-pattern `case a | b | c:`. Two or more alternatives.
    Or(Vec<Pattern>),
}

/// One `name [= pattern]` entry in a record pattern. Shorthand `{ x }` carries a
/// `Pattern::Var { name: "x" }` so it binds the field to a same-named variable.
#[derive(Debug, Clone, PartialEq)]
pub struct FieldPattern {
    pub name: String,
    pub name_span: NodeSpan,
    pub pattern: Pattern,
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
