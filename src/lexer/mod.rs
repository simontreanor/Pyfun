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

pub use token::{FStrPart, Span, Tok, Token};

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
///
/// The strict entry point used by the compiler: any lexing error fails the whole
/// tokenization (the first error is returned). The editor uses [`lex_recover`].
pub fn lex(source: &str) -> Result<Vec<Token>, LexError> {
    let (tokens, errors) = Lexer::new(source).run();
    match errors.into_iter().next() {
        Some(error) => Err(error),
        None => Ok(tokens),
    }
}

/// Tokenize `source`, **recovering** from lexing errors: a bad character or an
/// unterminated string is recorded and skipped, and tokenization continues. Always
/// returns a token stream (of what *did* lex) plus every error. This is the editor
/// entry point ([`crate::analyze`]); the compiler keeps the strict [`lex`].
pub fn lex_recover(source: &str) -> (Vec<Token>, Vec<LexError>) {
    Lexer::new(source).run()
}

struct Lexer<'a> {
    src: &'a [u8],
    pos: usize,
    out: Vec<Token>,
    /// Lexing errors collected during recovery (strict [`lex`] keeps only the first).
    errors: Vec<LexError>,
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
            errors: Vec::new(),
            depth: 0,
            layout: Vec::new(),
            pending_block: false,
        }
    }

    fn run(mut self) -> (Vec<Token>, Vec<LexError>) {
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
                return (self.out, self.errors);
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
            let before = self.pos;
            if let Err(error) = self.lex_one() {
                self.errors.push(error);
                // Recover: skip the offending character (a whole UTF-8 scalar, so a
                // multi-byte char yields one error, not one per byte) and carry on.
                // An unterminated string already consumed to EOF, so `before == pos`
                // is the bad-character case that needs a manual nudge.
                if self.pos == before {
                    self.skip_char();
                }
                continue;
            }
            // A block-opening token at bracket depth 0 primes a block to open if the
            // body begins on a deeper line: a `let` binding's `=`, a `match` arm or
            // lambda `->`, an `if`'s `then`/`else`, or a `match`/`case`'s `:`
            // (`DESIGN.md` §7.2). (Inline bodies cross no newline, so the priming
            // lapses and no block opens — this is why a single-line `extern name:
            // type = target` opens nothing.)
            if self.depth == 0
                && matches!(
                    self.out.last().map(|t| &t.tok),
                    Some(Tok::Eq | Tok::Arrow | Tok::Then | Tok::Else | Tok::Colon)
                )
            {
                self.pending_block = true;
            }
        }
    }

    /// Advance past one UTF-8 scalar value (the lead byte plus any continuation
    /// bytes), used to skip an un-lexable character during recovery.
    fn skip_char(&mut self) {
        self.pos += 1;
        while matches!(self.peek(), Some(b) if b & 0b1100_0000 == 0b1000_0000) {
            self.pos += 1;
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
    /// leads are infix operators, `|`, `.`, and the keywords `then`/`else`/`elif`/
    /// `with`/`and`/`or`/`in` — none of which can start an expression.
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
                "then" | "else" | "elif" | "with" | "and" | "or" | "in"
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
            // An adjacent `f"` (no space) opens an interpolated string. `f "x"` with a
            // space stays ordinary application, matching Python; only the pathological
            // adjacent `f"x"` changes meaning (it was `f` applied to `"x"`).
            b'f' if self.peek2() == Some(b'"') => self.lex_fstring(start),
            c if is_ident_start(c) => {
                self.lex_ident(start);
                Ok(())
            }
            _ => self.lex_symbol(start),
        }
    }

    /// Lex an interpolated string `f"...{expr}..."` into a single [`Tok::FStr`],
    /// splitting it into literal chunks and holes. `{{`/`}}` escape to literal
    /// braces; a lone `{` opens a hole scanned by [`Self::lex_hole`].
    fn lex_fstring(&mut self, start: usize) -> Result<(), LexError> {
        self.pos += 2; // skip `f"`
        let mut parts: Vec<FStrPart> = Vec::new();
        let mut lit = String::new();
        let flush = |lit: &mut String, parts: &mut Vec<FStrPart>| {
            if !lit.is_empty() {
                parts.push(FStrPart::Lit(std::mem::take(lit)));
            }
        };
        loop {
            match self.peek() {
                None => return Err(self.err(start, "unterminated f-string")),
                Some(b'"') => {
                    self.pos += 1;
                    flush(&mut lit, &mut parts);
                    self.push(Tok::FStr(parts), start);
                    return Ok(());
                }
                Some(b'\\') => {
                    self.pos += 1;
                    match self.peek() {
                        Some(b'"') => lit.push('"'),
                        Some(b'\\') => lit.push('\\'),
                        Some(b'n') => lit.push('\n'),
                        Some(b't') => lit.push('\t'),
                        _ => return Err(self.err(self.pos, "invalid escape sequence")),
                    }
                    self.pos += 1;
                }
                Some(b'{') if self.peek2() == Some(b'{') => {
                    lit.push('{');
                    self.pos += 2;
                }
                Some(b'}') if self.peek2() == Some(b'}') => {
                    lit.push('}');
                    self.pos += 2;
                }
                Some(b'{') => {
                    self.pos += 1; // consume `{`
                    let (hole, echo) = self.lex_hole(start)?;
                    // A self-documenting hole `{x=}` echoes its raw source text
                    // (including the `=`) before the value; the echo joins the
                    // pending literal chunk so adjacent literals stay merged.
                    if let Some(echo) = echo {
                        lit.push_str(&echo);
                    }
                    flush(&mut lit, &mut parts);
                    parts.push(FStrPart::Hole(hole));
                }
                Some(b'}') => {
                    return Err(self.err(
                        self.pos,
                        "single `}` in f-string; write `}}` for a literal brace",
                    ));
                }
                Some(_) => self.push_char(&mut lit),
            }
        }
    }

    /// Scan a hole body (the cursor is just past the opening `{`) up to its matching
    /// `}`, balancing nested `{}` and skipping string literals so a brace or quote
    /// inside them doesn't close the hole early, then lex the captured slice into the
    /// hole's tokens (with absolute spans). Also returns the raw text to echo when
    /// the hole carries a debug marker (see [`Self::debug_marker`]).
    fn lex_hole(&mut self, fstr_start: usize) -> Result<(Vec<Token>, Option<String>), LexError> {
        let hole_start = self.pos;
        let mut brace_depth = 0usize;
        loop {
            match self.peek() {
                None => return Err(self.err(fstr_start, "unterminated f-string (missing `}`)")),
                Some(b'"') => self.skip_string_in_hole()?,
                Some(b'{') => {
                    brace_depth += 1;
                    self.pos += 1;
                }
                Some(b'}') if brace_depth > 0 => {
                    brace_depth -= 1;
                    self.pos += 1;
                }
                Some(b'}') => {
                    let hole_end = self.pos;
                    self.pos += 1; // consume `}`
                    if hole_start == hole_end {
                        return Err(self.err(hole_start, "empty f-string hole `{}`"));
                    }
                    let (expr_end, echo) = self.debug_marker(hole_start, hole_end);
                    return Ok((self.lex_subrange(hole_start, expr_end), echo));
                }
                Some(_) => self.pos += 1,
            }
        }
    }

    /// Detect a self-documenting debug hole `f"{x=}"` (Python's `{expr=}` form): a
    /// single `=` as the hole's last non-whitespace character. The `=` must be a
    /// genuine marker, not the tail of an operator, so the character before it may
    /// not be one of `=`/`!`/`<`/`>` (`{x==y}`, `{x != y}`, `{x <= 1}`, `{x >= 1}`
    /// stay ordinary holes). Returns where the hole's *expression* ends (the marker
    /// excluded) plus, for a debug hole, the raw source text to echo — everything
    /// the user typed including the `=` and its surrounding whitespace, so
    /// `f"{x = }"` prints `x = <value>`.
    fn debug_marker(&self, hole_start: usize, hole_end: usize) -> (usize, Option<String>) {
        let text = &self.src[hole_start..hole_end];
        let mut last = text.len();
        while last > 0 && text[last - 1].is_ascii_whitespace() {
            last -= 1;
        }
        // A marker needs an `=` at the end and an expression (whose final
        // character isn't an operator tail) before it.
        if last < 2 || text[last - 1] != b'=' || matches!(text[last - 2], b'=' | b'!' | b'<' | b'>')
        {
            return (hole_end, None);
        }
        let echo = std::str::from_utf8(text)
            .expect("hole slice is valid utf-8")
            .to_string();
        (hole_start + last - 1, Some(echo))
    }

    /// Skip a string literal inside a hole (cursor at the opening `"`), honoring
    /// backslash escapes, so its contents can't prematurely close the hole.
    fn skip_string_in_hole(&mut self) -> Result<(), LexError> {
        let start = self.pos;
        self.pos += 1; // opening quote
        loop {
            match self.peek() {
                None => return Err(self.err(start, "unterminated string literal in f-string")),
                Some(b'"') => {
                    self.pos += 1;
                    return Ok(());
                }
                Some(b'\\') => self.pos += 2, // skip the escaped character
                Some(_) => self.pos += 1,
            }
        }
    }

    /// Lex the source range `[start, end)` (a hole's expression) into its own token
    /// stream, offsetting every span back to absolute source positions and forwarding
    /// any errors. Bracket depth is pre-set so the offside rule emits no layout tokens
    /// inside the hole.
    fn lex_subrange(&mut self, start: usize, end: usize) -> Vec<Token> {
        let sub = std::str::from_utf8(&self.src[start..end]).expect("hole slice is valid utf-8");
        let mut lexer = Lexer::new(sub);
        lexer.depth = 1; // suppress Indent/Dedent/Sep inside the hole
        let (mut tokens, errors) = lexer.run();
        for t in &mut tokens {
            t.span = Span::new(t.span.start + start, t.span.end + start);
        }
        for mut e in errors {
            e.span = Span::new(e.span.start + start, e.span.end + start);
            self.errors.push(e);
        }
        tokens
    }

    fn lex_number(&mut self, start: usize) -> Result<(), LexError> {
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.pos += 1;
        }
        // A '.' followed by a digit makes this a float; a trailing '.' is not
        // consumed (it isn't valid in the Phase 1 subset).
        let mut is_float = self.peek() == Some(b'.') && matches!(self.peek2(), Some(b'0'..=b'9'));
        if is_float {
            self.pos += 1; // consume '.'
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.pos += 1;
            }
        }
        // Optional exponent: `e`/`E`, an optional sign, then digits (`1e6`,
        // `2.5e-3`, `6.674e-11`). A number with an exponent is a float even without
        // a fractional part. The sign is consumed here (not left to unary minus).
        // Only consume `e` when a valid exponent follows, else it's an identifier
        // (so `x1` after a number, or `1en` with no digits, still lexes sensibly).
        if matches!(self.peek(), Some(b'e' | b'E')) {
            let has_exponent = match self.peek2() {
                Some(b'0'..=b'9') => true,
                Some(b'+' | b'-') => matches!(self.src.get(self.pos + 2), Some(b'0'..=b'9')),
                _ => false,
            };
            if has_exponent {
                is_float = true;
                self.pos += 1; // 'e' / 'E'
                if matches!(self.peek(), Some(b'+' | b'-')) {
                    self.pos += 1; // exponent sign
                }
                while matches!(self.peek(), Some(b'0'..=b'9')) {
                    self.pos += 1;
                }
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
                Some(_) => self.push_char(&mut value),
            }
        }
    }

    /// Append the (possibly multi-byte) UTF-8 character at the cursor to `out`,
    /// advancing past its whole byte sequence. `self.src` came from a `&str`, so it
    /// is valid UTF-8; pushing `b as char` per byte instead would turn every
    /// non-ASCII byte into its own Latin-1 codepoint, and re-encoding on emit would
    /// double-UTF-8-encode the text (`"café"` → mojibake).
    fn push_char(&mut self, out: &mut String) {
        let start = self.pos;
        let end = (start + utf8_len(self.src[start])).min(self.src.len());
        match std::str::from_utf8(&self.src[start..end]) {
            Ok(s) => out.push_str(s),
            Err(_) => out.push('\u{FFFD}'),
        }
        self.pos = end;
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
            b'%' => Tok::Percent,
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

/// The byte length of the UTF-8 sequence beginning with lead byte `b`. On a
/// continuation or invalid byte, returns 1 to guarantee forward progress (the
/// caller replaces the malformed slice with U+FFFD).
fn utf8_len(b: u8) -> usize {
    match b {
        _ if b < 0x80 => 1,
        _ if b >> 5 == 0b110 => 2,
        _ if b >> 4 == 0b1110 => 3,
        _ if b >> 3 == 0b11110 => 4,
        _ => 1,
    }
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
    fn fstring_splits_into_literals_and_holes() {
        let toks = kinds("f\"a {x} b\"");
        let Tok::FStr(parts) = &toks[0] else {
            panic!("expected FStr, got {:?}", toks[0]);
        };
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0], FStrPart::Lit("a ".to_string()));
        let FStrPart::Hole(hole) = &parts[1] else {
            panic!("expected a hole");
        };
        // The hole is lexed to its own tokens, terminated by `Eof`.
        assert_eq!(
            hole.iter().map(|t| t.tok.clone()).collect::<Vec<_>>(),
            vec![Tok::Ident("x".to_string()), Tok::Eof]
        );
        assert_eq!(parts[2], FStrPart::Lit(" b".to_string()));
    }

    #[test]
    fn fstring_hole_spans_are_absolute() {
        // `f"v={x}"` — the hole's `x` sits at byte offset 5 in the source.
        let toks = lex("f\"v={x}\"").unwrap();
        let Tok::FStr(parts) = &toks[0].tok else {
            panic!("expected FStr");
        };
        let FStrPart::Hole(hole) = &parts[1] else {
            panic!("expected a hole");
        };
        assert_eq!(hole[0].span, Span::new(5, 6));
    }

    #[test]
    fn fstring_escapes_and_nested_braces() {
        // `{{`/`}}` are literal braces; a nested string keeps its own `}`.
        let toks = kinds("f\"{{x}} {g \"}\"}\"");
        let Tok::FStr(parts) = &toks[0] else {
            panic!("expected FStr");
        };
        assert_eq!(parts[0], FStrPart::Lit("{x} ".to_string()));
        assert!(matches!(parts[1], FStrPart::Hole(_)));
    }

    #[test]
    fn fstring_debug_hole_echoes_its_source() {
        // `{x=}` echoes the raw hole text (incl. the `=`) as a literal chunk,
        // merged with any preceding literal, then the ordinary hole follows.
        let toks = kinds("f\"val {x=}\"");
        let Tok::FStr(parts) = &toks[0] else {
            panic!("expected FStr, got {:?}", toks[0]);
        };
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0], FStrPart::Lit("val x=".to_string()));
        let FStrPart::Hole(hole) = &parts[1] else {
            panic!("expected a hole");
        };
        // The hole's expression excludes the marker; spans stay absolute.
        assert_eq!(
            hole.iter().map(|t| t.tok.clone()).collect::<Vec<_>>(),
            vec![Tok::Ident("x".to_string()), Tok::Eof]
        );
    }

    #[test]
    fn fstring_debug_hole_preserves_whitespace() {
        // `{x = }` echoes the whitespace around the `=` verbatim (Python's rule).
        let toks = kinds("f\"{x = }\"");
        let Tok::FStr(parts) = &toks[0] else {
            panic!("expected FStr");
        };
        assert_eq!(parts[0], FStrPart::Lit("x = ".to_string()));
        assert!(matches!(parts[1], FStrPart::Hole(_)));
    }

    #[test]
    fn fstring_operator_equals_is_not_a_debug_marker() {
        // `==`/`!=`/`<=`/`>=` at the end of a hole are operators, not markers:
        // the hole keeps all its tokens and no literal chunk is echoed.
        for src in [
            "f\"{x==y}\"",
            "f\"{x != y}\"",
            "f\"{x >= 1}\"",
            "f\"{x <= 1}\"",
        ] {
            let toks = kinds(src);
            let Tok::FStr(parts) = &toks[0] else {
                panic!("expected FStr in {src}");
            };
            assert_eq!(parts.len(), 1, "{src}");
            let FStrPart::Hole(hole) = &parts[0] else {
                panic!("expected a hole in {src}");
            };
            assert_eq!(hole.len(), 4, "{src}"); // lhs, op, rhs, Eof
        }
    }

    #[test]
    fn f_with_space_is_not_an_fstring() {
        // `f "x"` (with a space) stays ordinary application; only adjacent `f"` opens
        // an interpolated string.
        assert_eq!(
            kinds("f \"x\""),
            vec![Tok::Ident("f".to_string()), Tok::Str("x".to_string()), Tok::Eof]
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

    #[test]
    fn scientific_notation_floats() {
        // Exponent (with optional sign, upper/lowercase e); a number with an
        // exponent is a float even without a fractional part.
        assert_eq!(kinds("1e6"), vec![Tok::Float(1e6), Tok::Eof]);
        assert_eq!(kinds("2.5e-3"), vec![Tok::Float(2.5e-3), Tok::Eof]);
        assert_eq!(kinds("1E3"), vec![Tok::Float(1e3), Tok::Eof]);
        assert_eq!(kinds("1e+4"), vec![Tok::Float(1e4), Tok::Eof]);
        // Back-compat: an `e` with no valid exponent is a separate identifier, not
        // part of the number (so `2exp` is `2` then `exp`, `1e` is `1` then `e`).
        assert_eq!(
            kinds("2exp"),
            vec![Tok::Int(2), Tok::Ident("exp".to_string()), Tok::Eof]
        );
        assert_eq!(
            kinds("1e"),
            vec![Tok::Int(1), Tok::Ident("e".to_string()), Tok::Eof]
        );
    }

    #[test]
    fn non_ascii_string_keeps_its_characters() {
        // Multi-byte UTF-8 (2-byte é, 3-byte →, 4-byte 🎉) must lex to the real
        // characters, not one Latin-1 codepoint per byte (which would double-encode
        // on emit). `String::len` counts chars, so this pins the char count too.
        let Tok::Str(s) = &kinds("\"café → 🎉\"")[0] else {
            panic!("expected a string literal")
        };
        assert_eq!(s, "café → 🎉");
        // café(4) + space + → + space + 🎉 = 8 characters (13 bytes).
        assert_eq!(s.chars().count(), 8);

        // Same for f-string literal chunks.
        let Tok::FStr(parts) = &kinds("f\"café {x}\"")[0] else {
            panic!("expected an f-string")
        };
        assert_eq!(parts[0], FStrPart::Lit("café ".to_string()));
    }

    #[test]
    fn recovers_from_a_bad_character() {
        // `@` is not a valid token; recovery skips it and lexes the rest.
        let (tokens, errors) = lex_recover("let x = 1 @ 2");
        assert_eq!(errors.len(), 1, "errors: {errors:?}");
        let toks: Vec<Tok> = tokens.into_iter().map(|t| t.tok).collect();
        assert_eq!(
            toks,
            vec![
                Tok::Let,
                Tok::Ident("x".to_string()),
                Tok::Eq,
                Tok::Int(1),
                Tok::Int(2),
                Tok::Eof
            ]
        );
    }

    #[test]
    fn recovers_from_an_unterminated_string() {
        // The string runs to EOF; recovery records the error and still yields the
        // leading tokens (so the editor sees `let s =`).
        let (tokens, errors) = lex_recover("let s = \"oops");
        assert_eq!(errors.len(), 1);
        let toks: Vec<Tok> = tokens.into_iter().map(|t| t.tok).collect();
        assert_eq!(
            toks,
            vec![Tok::Let, Tok::Ident("s".to_string()), Tok::Eq, Tok::Eof]
        );
    }

    #[test]
    fn multibyte_bad_char_is_one_error() {
        // A non-ASCII bad character is skipped as a single scalar, not per-byte.
        let (_, errors) = lex_recover("let x = §");
        assert_eq!(errors.len(), 1, "errors: {errors:?}");
    }

    #[test]
    fn strict_lex_still_fails_on_the_first_error() {
        assert!(lex("let x = 1 @ 2").is_err());
    }
}
