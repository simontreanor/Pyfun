//! Hand-written lexer for the Phase 1 Pyfun subset.
//!
//! Whitespace-insensitive for now (Pyfun's offside/whitespace rules are a later
//! phase). Line comments start with `//` (F#-style). Spans are byte offsets into
//! the original `&str`.

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
}

impl<'a> Lexer<'a> {
    fn new(source: &'a str) -> Self {
        Lexer {
            src: source.as_bytes(),
            pos: 0,
            out: Vec::new(),
        }
    }

    fn run(mut self) -> Result<Vec<Token>, LexError> {
        loop {
            self.skip_trivia();
            if self.pos >= self.src.len() {
                let end = self.pos;
                self.out.push(Token {
                    tok: Tok::Eof,
                    span: Span::new(end, end),
                });
                return Ok(self.out);
            }
            self.lex_one()?;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.src.get(self.pos).copied()
    }

    fn peek2(&self) -> Option<u8> {
        self.src.get(self.pos + 1).copied()
    }

    /// Skip whitespace and `//` line comments.
    fn skip_trivia(&mut self) {
        loop {
            match self.peek() {
                Some(b) if b.is_ascii_whitespace() => self.pos += 1,
                Some(b'/') if self.peek2() == Some(b'/') => {
                    while let Some(b) = self.peek() {
                        if b == b'\n' {
                            break;
                        }
                        self.pos += 1;
                    }
                }
                _ => return,
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
            b',' => Tok::Comma,
            b':' => Tok::Colon,
            _ => return Err(self.err(start, &format!("unexpected character {:?}", c as char))),
        };
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
        assert_eq!(
            kinds("1 // ignored\n2"),
            vec![Tok::Int(1), Tok::Int(2), Tok::Eof]
        );
    }

    #[test]
    fn string_escapes() {
        assert_eq!(
            kinds(r#""a\nb""#),
            vec![Tok::Str("a\nb".to_string()), Tok::Eof]
        );
    }
}
