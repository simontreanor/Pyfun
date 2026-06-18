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
    True,
    False,

    // Operators & punctuation
    Eq,         // =
    Plus,       // +
    Minus,      // -
    Star,       // *
    Slash,      // /
    PipeOp,     // |>
    Bar,        // |
    Arrow,      // ->
    Bang,       // !
    LParen,     // (
    RParen,     // )
    LBrace,     // {
    RBrace,     // }
    Comma,      // ,
    Colon,      // :
    Underscore, // _

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
