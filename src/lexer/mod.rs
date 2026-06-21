//! Hand-written lexer for the Pyfun subset.
//!
//! Mostly whitespace-insensitive, with an **offside rule** that turns indentation
//! into block structure (outside any `()`/`{}` brackets, where line breaks are
//! always continuations). A layout stack of block columns drives three synthetic
//! tokens:
//! - [`Tok::Indent`] — a `let … =` whose body begins on a *deeper* line opens a
//!   block (the only block opener; `=` at bracket depth 0 primes it).
//! - [`Tok::Dedent`] — a line dedents below the current block's column, closing it.
//! - [`Tok::Sep`] — a line lands on the current block's column and the next token
//!   can begin a statement, separating two statements.
//!
//! A deeper line, or one led by a continuation token (an infix operator, `|`,
//! `then`/`else`/`with`/…), continues the current statement — so multi-line
//! `match`/`if`/CE blocks keep working while statement sequences (and nested let
//! bodies) split correctly. The top level is the outermost block. Line comments
//! start with `#` (Python-style — `//` is the floor-division operator); spans are
//! byte offsets.

mod token;

pub use token::{Span, Tok, Token};

/// An error produced during lexing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LexError {
    pub message: String,
    pub span: Span,
}

impl std::fmt::Display for LexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} (at {}..{})",
            self.message, self.span.start, self.span.end
        )
    }
}

/// Tokenize `source` into a flat token stream terminated by [`Tok::Eof`].
pub fn lex(source: &str) -> Result<Vec<Token>, LexError> {
    Lexer::new(source).run()
}

struct Lexer<'a> {
    src: &'a [u8],
    pos: usize,
    out: Vec<Token>,
    /// Nesting depth of `()`/`{}`; line breaks inside brackets never separate.
    depth: usize,
    /// Stack of active layout (block) columns, innermost last. The first entry is
    /// the top-level column; deeper blocks (let bodies) push their own column.
    layout: Vec<usize>,
    /// Set right after an `=` at bracket depth 0: the next token, if it begins a
    /// deeper line, opens an indentation block (the let binding's body).
    pending_block: bool,
}

impl<'a> Lexer<'a> {
    fn new(source: &'a str) -> Self {
        Lexer {
            src: source.as_bytes(),
            pos: 0,
            out: Vec::new(),
            depth: 0,
            layout: Vec::new(),
            pending_block: false,
        }
    }

    fn run(mut self) -> Result<Vec<Token>, LexError> {
        loop {
            let crossed_newline = self.skip_trivia();
            if self.pos >= self.src.len() {
                // Close any still-open blocks before EOF.
                while self.layout.len() > 1 {
                    self.layout.pop();
                    self.push_layout(Tok::Dedent);
                }
                let end = self.pos;
                self.out.push(Token {
                    tok: Tok::Eof,
                    span: Span::new(end, end),
                });
                return Ok(self.out);
            }
            // The offside rule (see module docs): at bracket depth 0 a line break
            // opens a block (after `=`), closes blocks (dedent), or separates
            // statements (same column), via Indent/Dedent/Sep tokens.
            let col = self.column();
            if self.layout.is_empty() {
                self.layout.push(col);
            }
            let pending = std::mem::take(&mut self.pending_block);
            if !self.out.is_empty() && self.depth == 0 && crossed_newline {
                let top = *self.layout.last().unwrap();
                if pending && col > top {
                    self.layout.push(col);
                    self.push_layout(Tok::Indent);
                } else {
                    self.offside(col);
                }
            }
            self.lex_one()?;
            // An `=` at bracket depth 0 (a `let` binding's `=`) primes a block to
            // open if its body begins on a deeper line.
            if self.depth == 0 && matches!(self.out.last().map(|t| &t.tok), Some(Tok::Eq)) {
                self.pending_block = true;
            }
        }
    }

    /// Close any blocks deeper than `col`, then — if `col` lands exactly on the
    /// enclosing block's column and the next token can begin a statement — emit a
    /// statement separator. A line that is deeper than the current block, or one
    /// that leads with a continuation token (an infix operator, `|`, `then`,
    /// `else`, …), continues the current statement instead.
    fn offside(&mut self, col: usize) {
        while self.layout.len() > 1 && col < *self.layout.last().unwrap() {
            self.layout.pop();
            self.push_layout(Tok::Dedent);
        }
        let top = *self.layout.last().unwrap();
        if col == top && self.upcoming_starts_stmt() {
            self.push_layout(Tok::Sep);
        }
    }

    /// Push a zero-width layout token (`Sep`/`Indent`/`Dedent`) at the cursor.
    fn push_layout(&mut self, tok: Tok) {
        self.out.push(Token {
            tok,
            span: Span::new(self.pos, self.pos),
        });
    }

    /// Whether the upcoming token can begin a new statement (so a same-column line
    /// is a separate statement) rather than continue the current one. Continuation
    /// leads are infix operators, `|`, `.`, and the keywords `then`/`else`/`with`/
    /// `and`/`or`/`in` — none of which can start an expression.
    fn upcoming_starts_stmt(&self) -> bool {
        let Some(c) = self.peek() else { return false };
        if c.is_ascii_digit() || c == b'"' || c == b'(' || c == b'{' || c == b'[' {
            return true;
        }
        if c == b'_' {
            return false;
        }
        if is_ident_start(c) {
            let ident = self.peek_ident();
            return !matches!(
                ident.as_str(),
                "then" | "else" | "with" | "and" | "or" | "in"
            );
        }
        // Operators, `|`, `.`, `,`, closing brackets, etc. all continue a statement.
        false
    }

    /// Read (without consuming) the identifier starting at the cursor.
    fn peek_ident(&self) -> String {
        let mut i = self.pos;
        while i < self.src.len() && is_ident_continue(self.src[i]) {
            i += 1;
        }
        String::from_utf8_lossy(&self.src[self.pos..i]).into_owned()
    }

    /// The column (0-based) of the current position, i.e. bytes since the last
    /// newline. Assumes space indentation (a tab counts as one column).
    fn column(&self) -> usize {
        let mut i = self.pos;
        while i > 0 && self.src[i - 1] != b'\n' {
            i -= 1;
        }
        self.pos - i
    }

    fn peek(&self) -> Option<u8> {
        self.src.get(self.pos).copied()
    }

    fn peek2(&self) -> Option<u8> {
        self.src.get(self.pos + 1).copied()
    }

    /// Skip whitespace and `#` line comments, reporting whether at least one
    /// newline was crossed (so the caller can apply the offside rule).
    fn skip_trivia(&mut self) -> bool {
        let mut newline = false;
        loop {
            match self.peek() {
                Some(b'\n') => {
                    newline = true;
                    self.pos += 1;
                }
                Some(b) if b.is_ascii_whitespace() => self.pos += 1,
                // A `#` line comment runs to end of line; its terminating newline
                // is handled by the `\n` arm on the next iteration, so it counts.
                Some(b'#') => {
                    while let Some(b) = self.peek() {
                        if b == b'\n' {
                            break;
                        }
                        self.pos += 1;
                    }
                }
                _ => return newline,
            }
        }
    }

    fn push(&mut self, tok: Tok, start: usize) {
        self.out.push(Token {
            tok,
            span: Span::new(start, self.pos),
        });
    }

    fn lex_one(&mut self) -> Result<(), LexError> {
        let start = self.pos;
        let c = self.peek().unwrap();
        match c {
            b'0'..=b'9' => self.lex_number(start),
            b'"' => self.lex_string(start),
            c if is_ident_start(c) => {
                self.lex_ident(start);
                Ok(())
            }
            _ => self.lex_symbol(start),
        }
    }

    fn lex_number(&mut self, start: usize) -> Result<(), LexError> {
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.pos += 1;
        }
        // A '.' followed by a digit makes this a float; a trailing '.' is not
        // consumed (it isn't valid in the Phase 1 subset).
        let is_float = self.peek() == Some(b'.') && matches!(self.peek2(), Some(b'0'..=b'9'));
        if is_float {
            self.pos += 1; // consume '.'
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.pos += 1;
            }
        }
        let text = self.slice(start, self.pos);
        if is_float {
            let value: f64 = text
                .parse()
                .map_err(|_| self.err(start, "invalid float literal"))?;
            self.push(Tok::Float(value), start);
        } else {
            let value: i64 = text
                .parse()
                .map_err(|_| self.err(start, "invalid integer literal"))?;
            self.push(Tok::Int(value), start);
        }
        Ok(())
    }

    fn lex_string(&mut self, start: usize) -> Result<(), LexError> {
        self.pos += 1; // opening quote
        let mut value = String::new();
        loop {
            match self.peek() {
                None => return Err(self.err(start, "unterminated string literal")),
                Some(b'"') => {
                    self.pos += 1;
                    self.push(Tok::Str(value), start);
                    return Ok(());
                }
                Some(b'\\') => {
                    self.pos += 1;
                    match self.peek() {
                        Some(b'"') => value.push('"'),
                        Some(b'\\') => value.push('\\'),
                        Some(b'n') => value.push('\n'),
                        Some(b't') => value.push('\t'),
                        _ => return Err(self.err(self.pos, "invalid escape sequence")),
                    }
                    self.pos += 1;
                }
                Some(b) => {
                    value.push(b as char);
                    self.pos += 1;
                }
            }
        }
    }

    fn lex_ident(&mut self, start: usize) {
        self.pos += 1;
        while matches!(self.peek(), Some(b) if is_ident_continue(b)) {
            self.pos += 1;
        }
        let text = self.slice(start, self.pos);
        // A lone underscore is the wildcard, not an identifier.
        let tok = if text == "_" {
            Tok::Underscore
        } else if let Some(kw) = Tok::keyword(text) {
            kw
        } else {
            Tok::Ident(text.to_string())
        };
        self.push(tok, start);
    }

    fn lex_symbol(&mut self, start: usize) -> Result<(), LexError> {
        let c = self.peek().unwrap();
        // Two-character operators first.
        if c == b'|' && self.peek2() == Some(b'>') {
            self.pos += 2;
            self.push(Tok::PipeOp, start);
            return Ok(());
        }
        if c == b'-' && self.peek2() == Some(b'>') {
            self.pos += 2;
            self.push(Tok::Arrow, start);
            return Ok(());
        }
        if c == b'/' && self.peek2() == Some(b'/') {
            self.pos += 2;
            self.push(Tok::SlashSlash, start);
            return Ok(());
        }
        // Two-char comparison / equality operators (checked before `=` `!` `<` `>`).
        if let Some(tok) = match (c, self.peek2()) {
            (b'=', Some(b'=')) => Some(Tok::EqEq),
            (b'!', Some(b'=')) => Some(Tok::BangEq),
            (b'<', Some(b'=')) => Some(Tok::Le),
            (b'>', Some(b'=')) => Some(Tok::Ge),
            (b'<', Some(b'-')) => Some(Tok::LArrow),
            _ => None,
        } {
            self.pos += 2;
            self.push(tok, start);
            return Ok(());
        }
        let tok = match c {
            b'=' => Tok::Eq,
            b'+' => Tok::Plus,
            b'-' => Tok::Minus,
            b'*' => Tok::Star,
            b'/' => Tok::Slash,
            b'|' => Tok::Bar,
            b'!' => Tok::Bang,
            b'^' => Tok::Caret,
            b'<' => Tok::Lt,
            b'>' => Tok::Gt,
            b'(' => Tok::LParen,
            b')' => Tok::RParen,
            b'{' => Tok::LBrace,
            b'}' => Tok::RBrace,
            b'[' => Tok::LBracket,
            b']' => Tok::RBracket,
            b',' => Tok::Comma,
            b':' => Tok::Colon,
            b'.' => Tok::Dot,
            _ => return Err(self.err(start, &format!("unexpected character {:?}", c as char))),
        };
        // Track bracket nesting so the offside rule ignores line breaks inside
        // `(...)` / `{...}` (implicit line continuation).
        match tok {
            Tok::LParen | Tok::LBrace | Tok::LBracket => self.depth += 1,
            Tok::RParen | Tok::RBrace | Tok::RBracket => self.depth = self.depth.saturating_sub(1),
            _ => {}
        }
        self.pos += 1;
        self.push(tok, start);
        Ok(())
    }

    fn slice(&self, start: usize, end: usize) -> &str {
        // The lexer only advances over ASCII bytes for tokens, and string
        // contents are pushed char-by-char, so this slice is always valid UTF-8.
        std::str::from_utf8(&self.src[start..end]).expect("token slice is valid utf-8")
    }

    fn err(&self, start: usize, message: &str) -> LexError {
        LexError {
            message: message.to_string(),
            span: Span::new(start, self.pos),
        }
    }
}

fn is_ident_start(c: u8) -> bool {
    c == b'_' || c.is_ascii_alphabetic()
}

fn is_ident_continue(c: u8) -> bool {
    c == b'_' || c.is_ascii_alphanumeric()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(src: &str) -> Vec<Tok> {
        lex(src).unwrap().into_iter().map(|t| t.tok).collect()
    }

    #[test]
    fn distinguishes_pipe_from_bar() {
        assert_eq!(kinds("|> |"), vec![Tok::PipeOp, Tok::Bar, Tok::Eof]);
    }

    #[test]
    fn ints_floats_and_wildcard() {
        assert_eq!(
            kinds("1 2.5 _"),
            vec![Tok::Int(1), Tok::Float(2.5), Tok::Underscore, Tok::Eof]
        );
    }

    #[test]
    fn keywords_vs_idents() {
        assert_eq!(
            kinds("let x"),
            vec![Tok::Let, Tok::Ident("x".to_string()), Tok::Eof]
        );
    }

    #[test]
    fn skips_line_comments() {
        // `#` starts a line comment; the newline after it returns to the baseline
        // column, so the offside rule separates the two statements with a `Sep`.
        assert_eq!(
            kinds("1 # ignored\n2"),
            vec![Tok::Int(1), Tok::Sep, Tok::Int(2), Tok::Eof]
        );
    }

    #[test]
    fn distinguishes_division_operators() {
        assert_eq!(
            kinds("7 / 2"),
            vec![Tok::Int(7), Tok::Slash, Tok::Int(2), Tok::Eof]
        );
        assert_eq!(
            kinds("7 // 2"),
            vec![Tok::Int(7), Tok::SlashSlash, Tok::Int(2), Tok::Eof]
        );
    }

    #[test]
    fn offside_separates_top_level_items_but_not_continuations() {
        // Same-column lines separate; an indented continuation does not.
        assert_eq!(
            kinds("a\nb"),
            vec![
                Tok::Ident("a".to_string()),
                Tok::Sep,
                Tok::Ident("b".to_string()),
                Tok::Eof
            ]
        );
        assert_eq!(
            kinds("a\n  b"),
            vec![
                Tok::Ident("a".to_string()),
                Tok::Ident("b".to_string()),
                Tok::Eof
            ]
        );
        // Line breaks inside brackets never separate.
        assert_eq!(
            kinds("(a\nb)"),
            vec![
                Tok::LParen,
                Tok::Ident("a".to_string()),
                Tok::Ident("b".to_string()),
                Tok::RParen,
                Tok::Eof
            ]
        );
    }

    #[test]
    fn lexes_field_access_dot() {
        assert_eq!(
            kinds("p.x"),
            vec![
                Tok::Ident("p".to_string()),
                Tok::Dot,
                Tok::Ident("x".to_string()),
                Tok::Eof
            ]
        );
        // A float still wins over a leading-digit `.`; `.` only stands alone
        // between identifiers.
        assert_eq!(kinds("2.5"), vec![Tok::Float(2.5), Tok::Eof]);
    }

    #[test]
    fn lexes_reassignment_arrow() {
        assert_eq!(
            kinds("x <- 5"),
            vec![
                Tok::Ident("x".to_string()),
                Tok::LArrow,
                Tok::Int(5),
                Tok::Eof
            ]
        );
    }

    #[test]
    fn opens_a_block_after_indented_let_body() {
        // `let f =` with a deeper body opens a block; statements at the body
        // column are separated; the block closes (Dedent) at EOF.
        assert_eq!(
            kinds("let f =\n    a\n    b"),
            vec![
                Tok::Let,
                Tok::Ident("f".to_string()),
                Tok::Eq,
                Tok::Indent,
                Tok::Ident("a".to_string()),
                Tok::Sep,
                Tok::Ident("b".to_string()),
                Tok::Dedent,
                Tok::Eof
            ]
        );
    }

    #[test]
    fn inline_let_body_opens_no_block() {
        assert_eq!(
            kinds("let x = 1\nlet y = 2"),
            vec![
                Tok::Let,
                Tok::Ident("x".to_string()),
                Tok::Eq,
                Tok::Int(1),
                Tok::Sep,
                Tok::Let,
                Tok::Ident("y".to_string()),
                Tok::Eq,
                Tok::Int(2),
                Tok::Eof
            ]
        );
    }

    #[test]
    fn match_arms_stay_one_statement_in_a_block() {
        // The body block opens after `=`; the `|` arms lead with a continuation
        // token, so no `Sep` splits them — the match is a single statement.
        let toks = kinds("let f n =\n  match n with\n  | 0 -> 1\n  | _ -> 2");
        assert_eq!(toks.iter().filter(|t| **t == Tok::Sep).count(), 0);
        assert_eq!(toks.iter().filter(|t| **t == Tok::Indent).count(), 1);
        assert_eq!(toks.iter().filter(|t| **t == Tok::Dedent).count(), 1);
    }

    #[test]
    fn string_escapes() {
        assert_eq!(
            kinds(r#""a\nb""#),
            vec![Tok::Str("a\nb".to_string()), Tok::Eof]
        );
    }
}
