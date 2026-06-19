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
use ast::{
    BinOp, CeBuilder, CeItem, Expr, ExprKind, Item, LetBinding, MatchArm, Module, NodeSpan,
    Pattern, TypeDecl, TypeExpr, UnitExpr, VariantDecl,
};

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

    fn peek2(&self) -> &Tok {
        &self.tokens[(self.pos + 1).min(self.tokens.len() - 1)].tok
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

    /// Start offset of the current (next-to-consume) token.
    fn cur_start(&self) -> usize {
        self.tokens[self.pos].span.start
    }

    /// End offset of the most recently consumed token.
    fn prev_end(&self) -> usize {
        if self.pos == 0 {
            0
        } else {
            self.tokens[self.pos - 1].span.end
        }
    }

    /// Build an expression spanning from `start` to the end of the last token.
    fn mk(&self, start: usize, kind: ExprKind) -> Expr {
        Expr::new(kind, Span::new(start, self.prev_end()))
    }

    // ----- grammar -----

    fn parse_module(&mut self) -> Result<Module, ParseError> {
        let mut items = Vec::new();
        self.skip_seps();
        while !self.at_eof() {
            items.push(self.parse_item()?);
            // An item is delimited by the offside-inserted separators (or EOF).
            self.skip_seps();
        }
        Ok(Module { items })
    }

    /// Consume any statement separators between top-level items.
    fn skip_seps(&mut self) {
        while matches!(self.peek(), Tok::Sep) {
            self.bump();
        }
    }

    fn parse_item(&mut self) -> Result<Item, ParseError> {
        match self.peek() {
            Tok::Measure => self.parse_measure(),
            Tok::Type => Ok(Item::Type(self.parse_type_decl()?)),
            Tok::Let => Ok(Item::Let(self.parse_let_binding()?)),
            _ => Ok(Item::Expr(self.parse_expr()?)),
        }
    }

    fn parse_measure(&mut self) -> Result<Item, ParseError> {
        let start = self.cur_start();
        self.expect(&Tok::Measure, "`measure`")?;
        let name = self.parse_ident("measure name")?;
        let span = NodeSpan::new(Span::new(start, self.prev_end()));
        Ok(Item::Measure { name, span })
    }

    fn parse_type_decl(&mut self) -> Result<TypeDecl, ParseError> {
        let start = self.cur_start();
        self.expect(&Tok::Type, "`type`")?;
        let name = self.parse_upper_ident("type name")?;
        let mut params = Vec::new();
        while let Tok::Ident(_) = self.peek() {
            params.push(self.parse_ident("type parameter")?);
        }
        self.expect(&Tok::Eq, "`=`")?;
        self.eat(&Tok::Bar); // optional leading bar
        let mut variants = vec![self.parse_variant()?];
        while self.eat(&Tok::Bar) {
            variants.push(self.parse_variant()?);
        }
        let span = crate::parser::ast::NodeSpan::new(Span::new(start, self.prev_end()));
        Ok(TypeDecl {
            name,
            params,
            variants,
            span,
        })
    }

    fn parse_variant(&mut self) -> Result<VariantDecl, ParseError> {
        let name = self.parse_upper_ident("constructor name")?;
        let mut fields = Vec::new();
        while starts_type_atom(self.peek()) {
            fields.push(self.parse_type_atom()?);
        }
        Ok(VariantDecl { name, fields })
    }

    /// A type expression: an application optionally followed by `-> result`.
    fn parse_type(&mut self) -> Result<TypeExpr, ParseError> {
        let head = self.parse_type_app()?;
        if self.eat(&Tok::Arrow) {
            let result = self.parse_type()?;
            Ok(TypeExpr::Fun(Box::new(head), Box::new(result)))
        } else {
            Ok(head)
        }
    }

    fn parse_type_app(&mut self) -> Result<TypeExpr, ParseError> {
        // A capitalized head may be applied to argument atoms (`List a`).
        if let Tok::Ident(name) = self.peek().clone()
            && is_upper(&name)
        {
            self.bump();
            let mut args = Vec::new();
            while starts_type_atom(self.peek()) {
                args.push(self.parse_type_atom()?);
            }
            return Ok(TypeExpr::Con(name, args));
        }
        self.parse_type_atom()
    }

    fn parse_type_atom(&mut self) -> Result<TypeExpr, ParseError> {
        match self.peek().clone() {
            Tok::Ident(name) => {
                self.bump();
                Ok(TypeExpr::Con(name, Vec::new()))
            }
            Tok::LParen => {
                self.bump();
                let inner = self.parse_type()?;
                self.expect(&Tok::RParen, "`)`")?;
                Ok(inner)
            }
            _ => Err(self.error("expected a type")),
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
        let start = self.cur_start();
        self.expect(&Tok::Fun, "`fun`")?;
        let mut params = vec![self.parse_ident("parameter name")?];
        while let Tok::Ident(_) = self.peek() {
            params.push(self.parse_ident("parameter name")?);
        }
        self.expect(&Tok::Arrow, "`->`")?;
        let body = Box::new(self.parse_expr()?);
        Ok(self.mk(start, ExprKind::Fn { params, body }))
    }

    fn parse_if(&mut self) -> Result<Expr, ParseError> {
        let start = self.cur_start();
        self.expect(&Tok::If, "`if`")?;
        let cond = Box::new(self.parse_expr()?);
        self.expect(&Tok::Then, "`then`")?;
        let then = Box::new(self.parse_expr()?);
        self.expect(&Tok::Else, "`else`")?;
        let else_ = Box::new(self.parse_expr()?);
        Ok(self.mk(start, ExprKind::If { cond, then, else_ }))
    }

    fn parse_match(&mut self) -> Result<Expr, ParseError> {
        let start = self.cur_start();
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
        Ok(self.mk(start, ExprKind::Match { scrutinee, arms }))
    }

    fn parse_pipe(&mut self) -> Result<Expr, ParseError> {
        let start = self.cur_start();
        let mut lhs = self.parse_additive()?;
        while self.eat(&Tok::PipeOp) {
            let rhs = self.parse_additive()?;
            lhs = self.mk(
                start,
                ExprKind::Pipe {
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
            );
        }
        Ok(lhs)
    }

    fn parse_additive(&mut self) -> Result<Expr, ParseError> {
        let start = self.cur_start();
        let mut lhs = self.parse_multiplicative()?;
        loop {
            let op = match self.peek() {
                Tok::Plus => BinOp::Add,
                Tok::Minus => BinOp::Sub,
                _ => break,
            };
            self.bump();
            let rhs = self.parse_multiplicative()?;
            lhs = self.mk(
                start,
                ExprKind::Binary {
                    op,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
            );
        }
        Ok(lhs)
    }

    fn parse_multiplicative(&mut self) -> Result<Expr, ParseError> {
        let start = self.cur_start();
        let mut lhs = self.parse_application()?;
        loop {
            let op = match self.peek() {
                Tok::Star => BinOp::Mul,
                Tok::Slash => BinOp::Div,
                _ => break,
            };
            self.bump();
            let rhs = self.parse_application()?;
            lhs = self.mk(
                start,
                ExprKind::Binary {
                    op,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
            );
        }
        Ok(lhs)
    }

    fn parse_application(&mut self) -> Result<Expr, ParseError> {
        let start = self.cur_start();
        let mut func = self.parse_atom()?;
        while starts_atom(self.peek()) {
            let arg = self.parse_atom()?;
            func = self.mk(
                start,
                ExprKind::App {
                    func: Box::new(func),
                    arg: Box::new(arg),
                },
            );
        }
        Ok(func)
    }

    fn parse_atom(&mut self) -> Result<Expr, ParseError> {
        let start = self.cur_start();
        let kind = match self.peek().clone() {
            Tok::Int(n) => {
                self.bump();
                return self.maybe_unit(start, ExprKind::Int(n));
            }
            Tok::Float(f) => {
                self.bump();
                return self.maybe_unit(start, ExprKind::Float(f));
            }
            Tok::Str(s) => {
                self.bump();
                ExprKind::Str(s)
            }
            Tok::True => {
                self.bump();
                ExprKind::Bool(true)
            }
            Tok::False => {
                self.bump();
                ExprKind::Bool(false)
            }
            Tok::Ident(name) => {
                // `async`/`seq`/`result` are computation-expression builders only
                // when immediately followed by `{`; otherwise they are ordinary
                // identifiers.
                if let Some(builder) = CeBuilder::from_name(&name)
                    && *self.peek2() == Tok::LBrace
                {
                    self.bump(); // builder name
                    return self.parse_ce(builder, start);
                }
                self.bump();
                ExprKind::Var(name)
            }
            Tok::LParen => {
                self.bump();
                let inner = self.parse_expr()?;
                self.expect(&Tok::RParen, "`)`")?;
                // Keep the inner node's own (paren-free) span.
                return Ok(inner);
            }
            _ => return Err(self.error("expected an expression")),
        };
        Ok(self.mk(start, kind))
    }

    /// Wrap a freshly-parsed numeric literal in a unit annotation if one follows.
    fn maybe_unit(&mut self, start: usize, kind: ExprKind) -> Result<Expr, ParseError> {
        let literal = self.mk(start, kind);
        if matches!(self.peek(), Tok::Lt) {
            let unit = self.parse_unit_annotation()?;
            Ok(self.mk(
                start,
                ExprKind::Annot {
                    value: Box::new(literal),
                    unit,
                },
            ))
        } else {
            Ok(literal)
        }
    }

    /// Parse `<unit>`, e.g. `<m>`, `<m s>`, `<m/s^2>`, or `<1>` (dimensionless).
    fn parse_unit_annotation(&mut self) -> Result<UnitExpr, ParseError> {
        self.expect(&Tok::Lt, "`<`")?;
        if matches!(self.peek(), Tok::Int(1)) {
            self.bump();
            self.expect(&Tok::Gt, "`>`")?;
            return Ok(UnitExpr {
                factors: Vec::new(),
            });
        }
        let mut factors = Vec::new();
        while matches!(self.peek(), Tok::Ident(_)) {
            factors.push(self.parse_unit_factor(1)?);
        }
        if self.eat(&Tok::Slash) {
            while matches!(self.peek(), Tok::Ident(_)) {
                factors.push(self.parse_unit_factor(-1)?);
            }
        }
        self.expect(&Tok::Gt, "`>`")?;
        if factors.is_empty() {
            return Err(self.error("expected a unit (e.g. `m`, `m/s`, or `1`)"));
        }
        Ok(UnitExpr { factors })
    }

    fn parse_unit_factor(&mut self, sign: i32) -> Result<(String, i32), ParseError> {
        let name = self.parse_ident("measure name")?;
        let exp = if self.eat(&Tok::Caret) {
            match self.peek().clone() {
                Tok::Int(n) => {
                    self.bump();
                    n as i32
                }
                _ => return Err(self.error("expected an integer exponent after `^`")),
            }
        } else {
            1
        };
        Ok((name, sign * exp))
    }

    /// Parse `builder { items }`. Items are delimited by their leading keyword
    /// (`let!`, `let`, `do!`, `return`, `yield`), so no separators are needed.
    fn parse_ce(&mut self, builder: CeBuilder, start: usize) -> Result<Expr, ParseError> {
        self.expect(&Tok::LBrace, "`{`")?;
        let mut items = Vec::new();
        while !matches!(self.peek(), Tok::RBrace) {
            if self.at_eof() {
                return Err(self.error("unterminated computation expression"));
            }
            items.push(self.parse_ce_item()?);
        }
        self.expect(&Tok::RBrace, "`}`")?;
        Ok(self.mk(start, ExprKind::Ce { builder, items }))
    }

    fn parse_ce_item(&mut self) -> Result<CeItem, ParseError> {
        match self.peek() {
            Tok::Let => {
                self.bump();
                let bang = self.eat(&Tok::Bang);
                let name = self.parse_ident("binding name")?;
                self.expect(&Tok::Eq, "`=`")?;
                let value = self.parse_expr()?;
                Ok(if bang {
                    CeItem::LetBang { name, value }
                } else {
                    CeItem::Let { name, value }
                })
            }
            Tok::Return => {
                self.bump();
                let bang = self.eat(&Tok::Bang);
                let value = self.parse_expr()?;
                Ok(if bang {
                    CeItem::ReturnBang(value)
                } else {
                    CeItem::Return(value)
                })
            }
            Tok::Yield => {
                self.bump();
                let bang = self.eat(&Tok::Bang);
                let value = self.parse_expr()?;
                Ok(if bang {
                    CeItem::YieldBang(value)
                } else {
                    CeItem::Yield(value)
                })
            }
            Tok::Do => {
                self.bump();
                self.expect(&Tok::Bang, "`!`")?;
                Ok(CeItem::DoBang(self.parse_expr()?))
            }
            _ => Err(self.error("expected `let!`, `let`, `do!`, `return`, or `yield`")),
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

    fn parse_upper_ident(&mut self, what: &str) -> Result<String, ParseError> {
        match self.peek().clone() {
            Tok::Ident(name) if is_upper(&name) => {
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

fn starts_type_atom(tok: &Tok) -> bool {
    matches!(tok, Tok::Ident(_) | Tok::LParen)
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
        Tok::Sep => "end of statement".to_string(),
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
        Tok::Type => "type",
        Tok::Return => "return",
        Tok::Yield => "yield",
        Tok::Do => "do",
        Tok::Measure => "measure",
        Tok::Bang => "!",
        Tok::Caret => "^",
        Tok::Lt => "<",
        Tok::Gt => ">",
        Tok::LBrace => "{",
        Tok::RBrace => "}",
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
