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

    // Identifiers & keywords
    Ident(String),
    Let,
    Mut,
    If,
    Then,
    Else,
    Match,
    With,
    Fun,
    Type,
    Return,
    Yield,
    Do,
    Measure,
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
    Slash,      // / (true division)
    SlashSlash, // // (floor division)
    PipeOp,     // |>
    Bar,        // |
    Arrow,      // ->
    Bang,       // !
    Caret,      // ^
    Lt,         // < (also opens a unit annotation when adjacent to a literal)
    Gt,         // >
    Le,         // <=
    Ge,         // >=
    LParen,     // (
    RParen,     // )
    LBrace,     // {
    RBrace,     // }
    Comma,      // ,
    Colon,      // :
    Dot,        // . (record field access)
    Underscore, // _

    /// A statement separator, inserted by the lexer's lightweight offside rule at
    /// a line break that returns to (or below) the enclosing item's indentation
    /// (outside any brackets). It delimits top-level items so consecutive
    /// expression statements don't merge into one juxtaposition. See the lexer.
    Sep,

    Eof,
}

impl Tok {
    /// Map an identifier spelling to its keyword token, if any.
    pub fn keyword(ident: &str) -> Option<Tok> {
        Some(match ident {
            "let" => Tok::Let,
            "mut" => Tok::Mut,
            "if" => Tok::If,
            "then" => Tok::Then,
            "else" => Tok::Else,
            "match" => Tok::Match,
            "with" => Tok::With,
            "fun" => Tok::Fun,
            "type" => Tok::Type,
            "return" => Tok::Return,
            "yield" => Tok::Yield,
            "do" => Tok::Do,
            "measure" => Tok::Measure,
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
