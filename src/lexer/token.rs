//! Token and span types produced by the [`crate::lexer`].

/// A byte range into the original source, retained for future diagnostics
/// (Phase 3). The AST itself is span-free in Phase 1 so structural equality
/// drives the roundtrip tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub fn new(start: usize, end: usize) -> Self {
        Span { start, end }
    }
}

/// The lexical token kinds for the Phase 1 subset (`let`, `if`, `match`, `fun`,
/// curried application, and the pipe operator `|>`).
#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    // Literals
    Int(i64),
    Float(f64),
    Str(String),
    /// An interpolated string `f"...{expr}..."`, pre-split by the lexer into literal
    /// chunks and holes. Each hole carries the already-lexed tokens of its embedded
    /// expression (spans absolute into the original source, terminated by `Eof`), so
    /// the parser re-parses them in place with the ordinary expression grammar.
    FStr(Vec<FStrPart>),

    // Identifiers & keywords
    Ident(String),
    Let,
    Mut,
    Pure,
    If,
    Then,
    Else,
    Elif,
    Match,
    Case,
    With,
    Fun,
    Type,
    Return,
    Yield,
    Do,
    Measure,
    Extern,
    Module,
    Import,
    Try,
    As, // `as` — the binder in an as-pattern (`case P as x:`)
    Not,
    And,
    Or,
    True,
    False,

    // Operators & punctuation
    Eq,         // =
    EqEq,       // ==
    BangEq,     // !=
    Plus,       // +
    Minus,      // -
    Star,       // *
    StarStar,   // ** (exponentiation)
    Slash,      // / (true division)
    SlashSlash, // // (floor division)
    Percent,    // % (modulo)
    PipeOp,     // |>
    PipeLeft,   // <| (backward pipe: `f <| x` == `f x`)
    Bar,        // |
    Arrow,      // ->
    Bang,       // !
    Caret,      // ^
    Lt,         // < (also opens a unit annotation when adjacent to a literal)
    Gt,         // >
    Le,         // <=
    Ge,         // >=
    GtGt,       // >> (function composition, left-to-right)
    LtLt,       // << (function composition, right-to-left)
    LArrow,     // <- (reassignment of a `mut` binding)
    LParen,     // (
    RParen,     // )
    LBrace,     // {
    RBrace,     // }
    LBracket,   // [
    RBracket,   // ]
    Comma,      // ,
    Colon,      // :
    Dot,        // . (record field access)
    /// `...` — the caller-supplied slot marker in an `extern` target's keyword
    /// arguments (`= requests.get(timeout = ...)`, `DESIGN.md` §6), spelled as in a
    /// Python stub file. Lexed as one token so `. . .` is not the same thing, and
    /// unused elsewhere in the grammar.
    Ellipsis,
    Underscore, // _

    /// A typed hole in expression position: `?` (anonymous) or `?name` (named, the
    /// identifier lexed adjacently, like `f"`/`r"`). A placeholder the type checker
    /// accepts and reports the inferred type of (`DESIGN.md` §9). `?` is otherwise
    /// unused. The payload is the hole's name, or `None` for a bare `?`.
    Hole(Option<String>),

    /// A doc-comment line: `## text` at column 0 (bracket depth 0). Ordinary `#`
    /// comments — and `##` anywhere else (indented, trailing, inside brackets) —
    /// remain trivia; only this top-level form is attached, by the parser, to the
    /// following `let`/`type`/`extern` declaration (`DESIGN.md` §7). The payload
    /// is the line's text with the `## ` marker stripped.
    Doc(String),

    /// A statement separator, inserted by the lexer's offside rule between
    /// statements at the same layout column (outside any brackets) so consecutive
    /// statements don't merge into one juxtaposition. See the lexer.
    Sep,
    /// Opens an indentation block (the body of a `let … =` that begins on a
    /// deeper line). Inserted by the offside rule. See the lexer.
    Indent,
    /// Closes an indentation block (a line dedents below the block's column).
    Dedent,

    Eof,
}

impl Tok {
    /// Map an identifier spelling to its keyword token, if any.
    pub fn keyword(ident: &str) -> Option<Tok> {
        Some(match ident {
            "let" => Tok::Let,
            "mut" => Tok::Mut,
            "pure" => Tok::Pure,
            "if" => Tok::If,
            "then" => Tok::Then,
            "else" => Tok::Else,
            "elif" => Tok::Elif,
            "match" => Tok::Match,
            "case" => Tok::Case,
            "with" => Tok::With,
            "fun" => Tok::Fun,
            "type" => Tok::Type,
            "return" => Tok::Return,
            "yield" => Tok::Yield,
            "do" => Tok::Do,
            "measure" => Tok::Measure,
            "extern" => Tok::Extern,
            "module" => Tok::Module,
            "import" => Tok::Import,
            "try" => Tok::Try,
            "as" => Tok::As,
            "not" => Tok::Not,
            "and" => Tok::And,
            "or" => Tok::Or,
            "true" => Tok::True,
            "false" => Tok::False,
            _ => return None,
        })
    }
}

/// The position of a token that opens its source line: which line (1-based, as
/// diagnostics count) and at what column (0-based bytes from the line start).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LineStart {
    pub line: u32,
    pub col: u32,
}

/// A token paired with its source span, plus the two facts of line layout the
/// parser needs inside brackets, where the offside rule emits no layout tokens:
/// whether the token opens its source line (and where), and the bracket depth
/// in force where it starts. The computation-expression item rules read both
/// (`parser::Parser::ce_item_ends_here`).
#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub tok: Tok,
    pub span: Span,
    /// The token's line and column when it is the first token on its source
    /// line; `None` mid-line. Layout tokens (`Sep`/`Indent`/`Dedent`) carry
    /// `None` — the line's first real token holds the position.
    pub line_start: Option<LineStart>,
    /// Nesting depth of `()`/`{}`/`[]` where the token starts, so an opening
    /// bracket carries the depth outside itself.
    pub depth: u32,
}

/// One segment of an interpolated `f"..."` string ([`Tok::FStr`]): a literal chunk
/// (with escapes and `{{`/`}}` already resolved), or a hole holding the pre-lexed
/// tokens of its embedded expression.
#[derive(Debug, Clone, PartialEq)]
pub enum FStrPart {
    Lit(String),
    Hole(Vec<Token>),
}
