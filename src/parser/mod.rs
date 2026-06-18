//! Recursive-descent parser for the Phase 1 Pyfun subset.
//!
//! Grammar (informal):
//! ```text
//! module      := item*
//! item        := "let" ["mut"] ident ident* "=" expr | expr
//! expr        := fun | if | match | pipe
//! fun         := "fun" ident+ "->" expr
//! if          := "if" expr "then" expr "else" expr
//! match       := "match" expr "with" ("|" pattern "->" expr)+
//! pipe        := additive ("|>" additive)*
//! additive    := multiplicative (("+"|"-") multiplicative)*
//! multiplicative := application (("*"|"/") application)*
//! application := atom atom*          // juxtaposition; curried, left-assoc
//! atom        := int | float | string | "true" | "false" | ident | "(" expr ")"
//! pattern     := ctor | atom_pattern
//! ctor        := UpperIdent atom_pattern*
//! atom_pattern:= "_" | ident | int | "true" | "false" | "(" pattern ")"
//! ```
//! Operator precedence, lowest to highest: `|>` < `+ -` < `* /` < application.

pub mod ast;

use crate::lexer::{Span, Tok, Token};
use ast::{BinOp, Expr, Item, LetBinding, MatchArm, Module, Pattern};

/// An error produced during parsing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub message: String,
    pub span: Span,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} (at {}..{})",
            self.message, self.span.start, self.span.end
        )
    }
}

/// Parse a token stream (terminated by [`Tok::Eof`]) into a [`Module`].
pub fn parse(tokens: Vec<Token>) -> Result<Module, ParseError> {
    Parser { tokens, pos: 0 }.parse_module()
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> &Tok {
        &self.tokens[self.pos].tok
    }

    fn span(&self) -> Span {
        self.tokens[self.pos].span
    }

    fn at_eof(&self) -> bool {
        matches!(self.peek(), Tok::Eof)
    }

    /// Consume and return the current token kind.
    fn bump(&mut self) -> Tok {
        let tok = self.tokens[self.pos].tok.clone();
        if self.pos + 1 < self.tokens.len() {
            self.pos += 1;
        }
        tok
    }

    fn eat(&mut self, expected: &Tok) -> bool {
        if self.peek() == expected {
            self.bump();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, expected: &Tok, what: &str) -> Result<(), ParseError> {
        if self.eat(expected) {
            Ok(())
        } else {
            Err(self.error(&format!("expected {what}")))
        }
    }

    fn error(&self, message: &str) -> ParseError {
        ParseError {
            message: format!("{message}, found {}", describe(self.peek())),
            span: self.span(),
        }
    }

    // ----- grammar -----

    fn parse_module(&mut self) -> Result<Module, ParseError> {
        let mut items = Vec::new();
        while !self.at_eof() {
            items.push(self.parse_item()?);
        }
        Ok(Module { items })
    }

    fn parse_item(&mut self) -> Result<Item, ParseError> {
        if matches!(self.peek(), Tok::Let) {
            Ok(Item::Let(self.parse_let_binding()?))
        } else {
            Ok(Item::Expr(self.parse_expr()?))
        }
    }

    fn parse_let_binding(&mut self) -> Result<LetBinding, ParseError> {
        self.expect(&Tok::Let, "`let`")?;
        let mutable = self.eat(&Tok::Mut);
        let name = self.parse_ident("binding name")?;
        let mut params = Vec::new();
        while let Tok::Ident(_) = self.peek() {
            params.push(self.parse_ident("parameter name")?);
        }
        self.expect(&Tok::Eq, "`=`")?;
        let value = self.parse_expr()?;
        Ok(LetBinding {
            mutable,
            name,
            params,
            value,
        })
    }

    fn parse_expr(&mut self) -> Result<Expr, ParseError> {
        match self.peek() {
            Tok::Fun => self.parse_fun(),
            Tok::If => self.parse_if(),
            Tok::Match => self.parse_match(),
            _ => self.parse_pipe(),
        }
    }

    fn parse_fun(&mut self) -> Result<Expr, ParseError> {
        self.expect(&Tok::Fun, "`fun`")?;
        let mut params = vec![self.parse_ident("parameter name")?];
        while let Tok::Ident(_) = self.peek() {
            params.push(self.parse_ident("parameter name")?);
        }
        self.expect(&Tok::Arrow, "`->`")?;
        let body = Box::new(self.parse_expr()?);
        Ok(Expr::Fn { params, body })
    }

    fn parse_if(&mut self) -> Result<Expr, ParseError> {
        self.expect(&Tok::If, "`if`")?;
        let cond = Box::new(self.parse_expr()?);
        self.expect(&Tok::Then, "`then`")?;
        let then = Box::new(self.parse_expr()?);
        self.expect(&Tok::Else, "`else`")?;
        let else_ = Box::new(self.parse_expr()?);
        Ok(Expr::If { cond, then, else_ })
    }

    fn parse_match(&mut self) -> Result<Expr, ParseError> {
        self.expect(&Tok::Match, "`match`")?;
        let scrutinee = Box::new(self.parse_expr()?);
        self.expect(&Tok::With, "`with`")?;
        let mut arms = Vec::new();
        while self.eat(&Tok::Bar) {
            let pattern = self.parse_pattern()?;
            self.expect(&Tok::Arrow, "`->`")?;
            let body = self.parse_expr()?;
            arms.push(MatchArm { pattern, body });
        }
        if arms.is_empty() {
            return Err(self.error("expected at least one `| pattern -> expr` arm"));
        }
        Ok(Expr::Match { scrutinee, arms })
    }

    fn parse_pipe(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_additive()?;
        while self.eat(&Tok::PipeOp) {
            let rhs = self.parse_additive()?;
            lhs = Expr::Pipe {
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            };
        }
        Ok(lhs)
    }

    fn parse_additive(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_multiplicative()?;
        loop {
            let op = match self.peek() {
                Tok::Plus => BinOp::Add,
                Tok::Minus => BinOp::Sub,
                _ => break,
            };
            self.bump();
            let rhs = self.parse_multiplicative()?;
            lhs = Expr::Binary {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            };
        }
        Ok(lhs)
    }

    fn parse_multiplicative(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.parse_application()?;
        loop {
            let op = match self.peek() {
                Tok::Star => BinOp::Mul,
                Tok::Slash => BinOp::Div,
                _ => break,
            };
            self.bump();
            let rhs = self.parse_application()?;
            lhs = Expr::Binary {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            };
        }
        Ok(lhs)
    }

    fn parse_application(&mut self) -> Result<Expr, ParseError> {
        let mut func = self.parse_atom()?;
        while starts_atom(self.peek()) {
            let arg = self.parse_atom()?;
            func = Expr::App {
                func: Box::new(func),
                arg: Box::new(arg),
            };
        }
        Ok(func)
    }

    fn parse_atom(&mut self) -> Result<Expr, ParseError> {
        match self.peek().clone() {
            Tok::Int(n) => {
                self.bump();
                Ok(Expr::Int(n))
            }
            Tok::Float(f) => {
                self.bump();
                Ok(Expr::Float(f))
            }
            Tok::Str(s) => {
                self.bump();
                Ok(Expr::Str(s))
            }
            Tok::True => {
                self.bump();
                Ok(Expr::Bool(true))
            }
            Tok::False => {
                self.bump();
                Ok(Expr::Bool(false))
            }
            Tok::Ident(name) => {
                self.bump();
                Ok(Expr::Var(name))
            }
            Tok::LParen => {
                self.bump();
                let inner = self.parse_expr()?;
                self.expect(&Tok::RParen, "`)`")?;
                Ok(inner)
            }
            _ => Err(self.error("expected an expression")),
        }
    }

    fn parse_pattern(&mut self) -> Result<Pattern, ParseError> {
        // A capitalized identifier at this level is a constructor that may take
        // argument patterns; everything else is a single atom pattern.
        if let Tok::Ident(name) = self.peek().clone()
            && is_upper(&name)
        {
            self.bump();
            let mut args = Vec::new();
            while starts_atom_pattern(self.peek()) {
                args.push(self.parse_atom_pattern()?);
            }
            return Ok(Pattern::Ctor { name, args });
        }
        self.parse_atom_pattern()
    }

    fn parse_atom_pattern(&mut self) -> Result<Pattern, ParseError> {
        match self.peek().clone() {
            Tok::Underscore => {
                self.bump();
                Ok(Pattern::Wildcard)
            }
            Tok::Ident(name) => {
                self.bump();
                if is_upper(&name) {
                    Ok(Pattern::Ctor {
                        name,
                        args: Vec::new(),
                    })
                } else {
                    Ok(Pattern::Var(name))
                }
            }
            Tok::Int(n) => {
                self.bump();
                Ok(Pattern::Int(n))
            }
            Tok::True => {
                self.bump();
                Ok(Pattern::Bool(true))
            }
            Tok::False => {
                self.bump();
                Ok(Pattern::Bool(false))
            }
            Tok::LParen => {
                self.bump();
                let inner = self.parse_pattern()?;
                self.expect(&Tok::RParen, "`)`")?;
                Ok(inner)
            }
            _ => Err(self.error("expected a pattern")),
        }
    }

    fn parse_ident(&mut self, what: &str) -> Result<String, ParseError> {
        match self.peek().clone() {
            Tok::Ident(name) => {
                self.bump();
                Ok(name)
            }
            _ => Err(self.error(&format!("expected {what}"))),
        }
    }
}

fn starts_atom(tok: &Tok) -> bool {
    matches!(
        tok,
        Tok::Int(_)
            | Tok::Float(_)
            | Tok::Str(_)
            | Tok::True
            | Tok::False
            | Tok::Ident(_)
            | Tok::LParen
    )
}

fn starts_atom_pattern(tok: &Tok) -> bool {
    matches!(
        tok,
        Tok::Underscore | Tok::Ident(_) | Tok::Int(_) | Tok::True | Tok::False | Tok::LParen
    )
}

fn is_upper(name: &str) -> bool {
    name.chars().next().is_some_and(|c| c.is_uppercase())
}

/// A short human-readable name for a token, used in error messages.
fn describe(tok: &Tok) -> String {
    match tok {
        Tok::Int(n) => format!("integer `{n}`"),
        Tok::Float(f) => format!("float `{f}`"),
        Tok::Str(_) => "string literal".to_string(),
        Tok::Ident(name) => format!("identifier `{name}`"),
        Tok::Eof => "end of input".to_string(),
        other => format!("`{}`", token_symbol(other)),
    }
}

fn token_symbol(tok: &Tok) -> &'static str {
    match tok {
        Tok::Let => "let",
        Tok::Mut => "mut",
        Tok::If => "if",
        Tok::Then => "then",
        Tok::Else => "else",
        Tok::Match => "match",
        Tok::With => "with",
        Tok::Fun => "fun",
        Tok::True => "true",
        Tok::False => "false",
        Tok::Eq => "=",
        Tok::Plus => "+",
        Tok::Minus => "-",
        Tok::Star => "*",
        Tok::Slash => "/",
        Tok::PipeOp => "|>",
        Tok::Bar => "|",
        Tok::Arrow => "->",
        Tok::LParen => "(",
        Tok::RParen => ")",
        Tok::Comma => ",",
        Tok::Colon => ":",
        Tok::Underscore => "_",
        _ => "token",
    }
}
