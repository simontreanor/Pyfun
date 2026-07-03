//! Token and span types produced by the [`crate::lexer`].

/// A byte range into the original source, retained for future diagnostics
/// (Phase 3). The AST itself is span-free in Phase 1 so structural equality
/// drives the roundtrip tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    Underscore, // _

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

/// A token paired with its source span.
#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub tok: Tok,
    pub span: Span,
}

/// One segment of an interpolated `f"..."` string ([`Tok::FStr`]): a literal chunk
/// (with escapes and `{{`/`}}` already resolved), or a hole holding the pre-lexed
/// tokens of its embedded expression.
#[derive(Debug, Clone, PartialEq)]
pub enum FStrPart {
    Lit(String),
    Hole(Vec<Token>),
}
